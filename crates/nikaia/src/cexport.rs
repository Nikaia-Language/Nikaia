// crates/nikaia/src/cexport.rs
//
// **A package as a library C calls** ([ADR-284](../../docs/specification/adr/adr-284.md)):
// the wrapper the language below needs around each entry point, and the header
// a C program includes, both from one reading of the package.
//
// An entry point is a `pub extern fn` with a body (D4, ADR-324 D6). What this
// file writes for one:
//
// * a function with the C calling convention and the prefixed name (D13),
//   returning an `int` status and handing its value back through an
//   out-parameter (D7);
// * the panic caught at the boundary, which poisons the library: every later
//   call is `E_PANICKED` until `<prefix>_shutdown` and `<prefix>_init` have run
//   (D8, D10), and every call after `shutdown` is `E_NOT_RUNNING`;
// * its line in `<package>.h`, which also carries the seven status codes the
//   boundary owns and the hash of the ledger the header was derived from (D12).
//
// **What is built of D5's table**: numbers, `bool` and `scalar` in and out;
// text (`ref String`) and bytes (`Bytes`) in as an address and a length, and
// out (`String`, `Bytes`, `Vec[u8]`) into the caller's buffer (D6); a run of
// numbers (`ref Array[T]`) in; an `enum` without payload in and out, as a C
// `enum`; a `pub struct` as a handle - out as a new one, in as `ref T`, with
// its `extern` constructor and methods, a getter per `pub` field and `_free`,
// each call holding its lock (D11); `ref String?` and `ref T?` of a handle
// in, and `T?` of a handle out, NULL for `null`; a callback that does not pause or throw,
// as a function pointer and a `void *ctx`, taking values, enums and text and
// handing back a number or nothing; a function that may pause, blocking and
// `_async` with a ticket `<prefix>_cancel` cancels at its next pause point
// (D9, D19) - of a function or method declared in the package's entry file,
// throwing at most one `enum` of it. Every raw pointer is read and written
// through `c-boundary` (ADR-218). Every other entry point is refused here,
// saying which part is not built yet, rather than exported half-way.

use anyhow::Result;

use crate::ast::{Item, Type};
use crate::modules::Program;

/// The two halves of a library's boundary.
pub struct Exported {
    /// Appended to the entry file's lowering.
    pub rust: String,
    /// `<package>.h`.
    pub header: String,
}

/// Where the header names the ledger's digest, filled in once the lowering
/// has written the ledger.
pub const LEDGER_DIGEST: &str = "@LEDGER@";

/// One value crossing by value (ADR-284 D5's first row): its type in the
/// language below and in C.
struct ByValue {
    rust: &'static str,
    c: &'static str,
    /// A `scalar` crosses as a `uint32_t` and is checked on the way in: not
    /// every number is a Unicode scalar value.
    scalar: bool,
}

fn by_value(parsed: &crate::parser::Parsed, ty: &Type) -> Option<ByValue> {
    if ty.is_view || ty.is_nullable || ty.is_tuple || !ty.generics.is_empty() || ty.code.is_some() {
        return None;
    }
    let (rust, c, scalar) = match parsed.text(ty.name) {
        "i32" => ("i32", "int32_t", false),
        "i64" => ("i64", "int64_t", false),
        "u8" => ("u8", "uint8_t", false),
        "f64" => ("f64", "double", false),
        "bool" => ("bool", "bool", false),
        "scalar" | "char" => ("u32", "uint32_t", true),
        _ => return None,
    };
    Some(ByValue { rust, c, scalar })
}

/// What a parameter takes from C (ADR-284 D5).
enum In {
    Value(ByValue),
    /// `ref String`, or `ref Array[T]` of a number: an address and a length C
    /// keeps for the call.
    Run {
        rust: &'static str,
        c: &'static str,
        as_text: bool,
    },
    /// `Bytes`: the same, made a `Bytes` of its own.
    Bytes,
    /// An `enum` without payload: a C `enum`, its number checked on the way in.
    Choice(usize),
    /// `ref T` of a handle: held shared for the call.
    Handle(usize),
    /// `ref String?` or `ref T?` of a handle: NULL is `null` (D5).
    Absent(Box<In>),
    /// A `pub extern struct`, by value (D14).
    Record(usize),
    /// `fn(A…) -> R sync`: a function pointer and the `void *ctx` C hands it
    /// back (D5).
    Callback {
        args: Vec<Lent>,
        result: Option<ByValue>,
    },
}

/// What a callback hands C (ADR-284 D5, D6).
enum Lent {
    Value(ByValue),
    Choice(usize),
    /// `ref String`: an address and a length that live for the call.
    Text,
}

/// A callback C may be: one that does not pause or throw, takes values, enums
/// and text, and hands back a number or `bool`, or nothing.
fn callback(parsed: &crate::parser::Parsed, plains: &[Plain], ty: &Type) -> Option<In> {
    let code = (*ty.code).as_ref()?;
    if !code.is_sync || code.can_throw || ty.is_nullable {
        return None;
    }
    let args = ty
        .generics
        .iter()
        .map(|arg| {
            if let Some(shape) = by_value(parsed, arg) {
                return Some(Lent::Value(shape));
            }
            if let Some(at) = choice(parsed, plains, arg) {
                return Some(Lent::Choice(at));
            }
            let text = arg.is_view
                && arg.generics.is_empty()
                && matches!(parsed.text(arg.name), "String" | "str");
            text.then_some(Lent::Text)
        })
        .collect::<Option<Vec<_>>>()?;
    let result = match &*code.result {
        None => None,
        Some(result) => Some(by_value(parsed, result).filter(|shape| !shape.scalar)?),
    };
    Some(In::Callback { args, result })
}

/// What a result hands back (ADR-284 D5, D6).
enum Out {
    Value(ByValue),
    /// `String`, `Bytes` or `Vec[u8]`: into the caller's buffer.
    Buffer,
    /// An `enum` without payload: its number.
    Choice(usize),
    /// A `pub struct`: a new handle, which the caller frees.
    Handle(usize),
    /// `T?` of a handle: NULL for `null` (D5).
    AbsentHandle(usize),
    /// A `pub extern struct`, by value (D14).
    Record(usize),
}

/// **A `pub extern struct`** of the entry file (ADR-284 D14, D15): C's layout,
/// crossing by value. The wrapper reads and writes it through a mirror whose
/// enum and `scalar` fields are plain numbers, checked on the way in.
struct Record {
    name: String,
    fields: Vec<(String, Part)>,
}

/// One field of a [`Record`].
enum Part {
    Value(ByValue),
    Choice(usize),
    Record(usize),
}

fn records(parsed: &crate::parser::Parsed, plains: &[Plain]) -> Result<Vec<Record>> {
    let declared: Vec<(&str, &Vec<crate::ast::FieldDef>)> = parsed
        .program
        .items
        .iter()
        .filter_map(|item| match &item.node {
            Item::Struct {
                name,
                fields,
                is_extern: true,
                ..
            } => Some((parsed.text(*name), fields)),
            _ => None,
        })
        .collect();
    declared
        .iter()
        .map(|(name, fields)| {
            let fields = fields
                .iter()
                .map(|field| {
                    let field_name = parsed.text(field.name).to_string();
                    let part = if let Some(shape) = by_value(parsed, &field.ty) {
                        Part::Value(shape)
                    } else if let Some(at) = choice(parsed, plains, &field.ty) {
                        Part::Choice(at)
                    } else if let Some(at) = declared
                        .iter()
                        .position(|(other, _)| *other == parsed.text(field.ty.name))
                        .filter(|_| field.ty.generics.is_empty() && !field.ty.is_nullable)
                    {
                        Part::Record(at)
                    } else {
                        return Err(not_yet(
                            &format!("{name}.{field_name}"),
                            "field of an `extern` struct",
                        ));
                    };
                    Ok((field_name, part))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Record {
                name: name.to_string(),
                fields,
            })
        })
        .collect()
}

fn record_of(parsed: &crate::parser::Parsed, records: &[Record], ty: &Type) -> Option<usize> {
    if ty.is_view || ty.is_nullable || ty.is_tuple || !ty.generics.is_empty() || ty.code.is_some() {
        return None;
    }
    let name = parsed.text(ty.name);
    records.iter().position(|record| record.name == name)
}

/// The mirror of a [`Record`] the wrapper takes and hands back, and its two
/// conversions: `None` where C's value is no value of the struct.
fn mirror(record: &Record, records: &[Record], plains: &[Plain]) -> String {
    let ty = &record.name;
    let mut fields = String::new();
    let mut into = Vec::new();
    let mut from = Vec::new();
    for (field, part) in &record.fields {
        let local = crate::emit::escaped(field);
        let (lowered, inward, outward) = match part {
            Part::Value(shape) if shape.scalar => (
                "u32".to_string(),
                format!("char::from_u32(self.{local})?"),
                format!("value.{local} as u32"),
            ),
            Part::Value(shape) => (
                shape.rust.to_string(),
                format!("self.{local}"),
                format!("value.{local}"),
            ),
            Part::Choice(at) => {
                let plain = &plains[*at];
                let inward: Vec<String> = plain
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(number, variant)| format!("{number} => {}::{variant},", plain.name))
                    .collect();
                let outward: Vec<String> = plain
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(number, variant)| format!("{}::{variant} => {number},", plain.name))
                    .collect();
                (
                    "std::ffi::c_int".to_string(),
                    format!(
                        "match self.{local} {{ {} _ => return None }}",
                        inward.join(" ")
                    ),
                    format!("match value.{local} {{ {} }}", outward.join(" ")),
                )
            }
            Part::Record(at) => {
                let inner = &records[*at].name;
                (
                    format!("__nikaia_c_{inner}"),
                    format!("self.{local}.into_nikaia()?"),
                    format!("__nikaia_c_{inner}::from_nikaia(value.{local})"),
                )
            }
        };
        fields.push_str(&format!("    {local}: {lowered},\n"));
        into.push(format!("{local}: {inward}"));
        from.push(format!("{local}: {outward}"));
    }
    format!(
        "\n/// `{ty}` as C lays it out and hands it over (ADR-284 D14).\n\
         #[repr(C)]\n#[derive(Clone, Copy)]\n#[allow(non_camel_case_types)]\n\
         pub struct __nikaia_c_{ty} {{\n{fields}}}\n\
         \n#[allow(dead_code)]\nimpl __nikaia_c_{ty} {{\n    \
         fn into_nikaia(self) -> Option<{ty}> {{\n        Some({ty} {{ {} }})\n    }}\n    \
         fn from_nikaia(value: {ty}) -> Self {{\n        Self {{ {} }}\n    }}\n}}\n",
        into.join(", "),
        from.join(", ")
    )
}

/// An `enum` without payload the entry file declares (ADR-284 D5): its name
/// and its variants, numbered in declaration order.
struct Plain {
    name: String,
    variants: Vec<String>,
}

fn plain_enums(parsed: &crate::parser::Parsed) -> Vec<Plain> {
    parsed
        .program
        .items
        .iter()
        .filter_map(|item| match &item.node {
            Item::Enum { name, variants, .. }
                if variants
                    .iter()
                    .all(|variant| matches!(variant.fields, crate::ast::VariantFields::Unit)) =>
            {
                Some(Plain {
                    name: parsed.text(*name).to_string(),
                    variants: variants
                        .iter()
                        .map(|variant| parsed.text(variant.name).to_string())
                        .collect(),
                })
            }
            _ => None,
        })
        .collect()
}

/// Which of `plains` a type names, when it is one of them as written.
fn choice(parsed: &crate::parser::Parsed, plains: &[Plain], ty: &Type) -> Option<usize> {
    if ty.is_view || ty.is_nullable || ty.is_tuple || !ty.generics.is_empty() || ty.code.is_some() {
        return None;
    }
    let name = parsed.text(ty.name);
    plains.iter().position(|plain| plain.name == name)
}

/// Which of `handles` a type names, as a view or not, nullable or not.
fn handle_of(parsed: &crate::parser::Parsed, handles: &[Handled], ty: &Type) -> Option<usize> {
    if ty.is_tuple || !ty.generics.is_empty() || ty.code.is_some() {
        return None;
    }
    let name = parsed.text(ty.name);
    handles.iter().position(|handle| handle.name == name)
}

fn taken(
    parsed: &crate::parser::Parsed,
    plains: &[Plain],
    handles: &[Handled],
    records: &[Record],
    ty: &Type,
) -> Option<In> {
    if let Some(at) = record_of(parsed, records, ty) {
        return Some(In::Record(at));
    }
    if ty.code.is_some() {
        return callback(parsed, plains, ty);
    }
    if ty.is_nullable {
        let mut present = ty.clone();
        present.is_nullable = false;
        return match taken(parsed, plains, handles, records, &present)? {
            text @ In::Run { as_text: true, .. } => Some(In::Absent(Box::new(text))),
            handle @ In::Handle(_) => Some(In::Absent(Box::new(handle))),
            _ => None,
        };
    }
    if let Some(at) = handle_of(parsed, handles, ty) {
        // A handle taken by value would end it: not exported yet.
        return ty.is_view.then_some(In::Handle(at));
    }
    if let Some(shape) = by_value(parsed, ty) {
        return Some(In::Value(shape));
    }
    if let Some(at) = choice(parsed, plains, ty) {
        return Some(In::Choice(at));
    }
    if ty.is_nullable || ty.code.is_some() {
        return None;
    }
    let name = parsed.text(ty.name);
    match (name, ty.is_view, ty.generics.as_slice()) {
        ("String" | "str", true, []) => Some(In::Run {
            rust: "u8",
            c: "uint8_t",
            as_text: true,
        }),
        ("Array", true, [element]) => {
            let shape = by_value(parsed, element).filter(|shape| !shape.scalar)?;
            Some(In::Run {
                rust: shape.rust,
                c: shape.c,
                as_text: false,
            })
        }
        ("Bytes", _, []) => Some(In::Bytes),
        _ => None,
    }
}

fn handed(
    parsed: &crate::parser::Parsed,
    plains: &[Plain],
    handles: &[Handled],
    records: &[Record],
    ty: &Type,
) -> Option<Out> {
    if let Some(at) = record_of(parsed, records, ty) {
        return Some(Out::Record(at));
    }
    if ty.is_nullable && !ty.is_view {
        let mut present = ty.clone();
        present.is_nullable = false;
        return handle_of(parsed, handles, &present).map(Out::AbsentHandle);
    }
    if let Some(at) = handle_of(parsed, handles, ty) {
        return (!ty.is_view).then_some(Out::Handle(at));
    }
    if let Some(shape) = by_value(parsed, ty) {
        return Some(Out::Value(shape));
    }
    if let Some(at) = choice(parsed, plains, ty) {
        return Some(Out::Choice(at));
    }
    if ty.is_nullable || ty.is_view || ty.code.is_some() {
        return None;
    }
    let name = parsed.text(ty.name);
    match (name, ty.generics.as_slice()) {
        ("String" | "Bytes", []) => Some(Out::Buffer),
        ("Vec", [element]) if parsed.text(element.name) == "u8" => Some(Out::Buffer),
        _ => None,
    }
}

/// The names of a wrapper's parameters, as `name: type` lists write them -
/// a type may hold commas of its own (`fn(i64, *mut c_void)`), so only a
/// comma outside brackets ends one.
fn parameter_names(params: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for list in params {
        let mut depth = 0_i32;
        let mut start = 0;
        let bytes = list.as_bytes();
        for (at, byte) in bytes.iter().enumerate() {
            match byte {
                b'<' | b'(' | b'[' => depth += 1,
                b'>' if at > 0 && bytes[at - 1] == b'-' => {}
                b'>' | b')' | b']' => depth -= 1,
                b',' if depth == 0 => {
                    names.push(list[start..at].trim().to_string());
                    start = at + 1;
                }
                _ => {}
            }
        }
        names.push(list[start..].trim().to_string());
    }
    names
        .into_iter()
        .filter_map(|param| param.split(':').next().map(|name| name.trim().to_string()))
        .collect()
}

/// The refusal for an entry point whose `what` is not exported yet.
fn not_yet(written: &str, what: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "`{written}` is an entry point whose {what} this compiler does not export yet: \
         what is built is numbers, `bool`, `scalar` and an `enum` without payload in and \
         out, text and bytes in and out, a run of numbers in, and a `pub struct` as a \
         handle, of a function in `src/main.nika` (ADR-284 D5)."
    )
}

/// How a call holds a handle (ADR-284 D11): shared to read it, alone to change it.
#[derive(Clone, Copy, PartialEq)]
enum Hold {
    Shared,
    Exclusive,
}

/// **One function C calls**: an entry point, a handle's method or constructor,
/// or the getter of one of its `pub` fields.
struct Entry<'a> {
    /// The handle type it belongs to.
    owner: Option<String>,
    /// As written: `new` for the anonymous constructor, the field for a getter.
    name: String,
    /// How it holds `self`, where it has one.
    receiver: Option<Hold>,
    args: &'a [crate::ast::FnArg],
    ret_type: Option<&'a Type>,
    /// A getter reads its field rather than calling anything.
    getter: bool,
}

impl Entry<'_> {
    /// The name the ledger knows it by.
    fn key(&self) -> String {
        match &self.owner {
            Some(owner) => format!("{owner}::{}", self.name),
            None => self.name.clone(),
        }
    }

    /// As a refusal names it.
    fn written(&self) -> String {
        match &self.owner {
            Some(owner) => format!("{owner}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// A `pub struct` of the entry file C holds as a handle (ADR-284 D5), with
/// its `pub` fields, each of which gets a getter.
struct Handled<'a> {
    name: String,
    fields: Vec<(String, &'a Type)>,
}

fn handled_structs(parsed: &crate::parser::Parsed) -> Vec<Handled<'_>> {
    parsed
        .program
        .items
        .iter()
        .filter_map(|item| match &item.node {
            Item::Struct {
                name,
                generics,
                fields,
                is_public: true,
                is_extern: false,
            } if generics.is_empty() => Some(Handled {
                name: parsed.text(*name).to_string(),
                fields: fields
                    .iter()
                    .filter(|field| field.is_public)
                    .map(|field| (parsed.text(field.name).to_string(), &field.ty))
                    .collect(),
            }),
            _ => None,
        })
        .collect()
}

/// Every `extern` function and method the program's own files declare.
fn entries<'a>(program: &'a Program) -> Result<Vec<Entry<'a>>> {
    let mut out = Vec::new();
    for (at, unit) in program.units.iter().enumerate() {
        if unit.package.is_some() {
            continue;
        }
        let parsed = &unit.parsed;
        let mut take = |owner: Option<String>, item: &'a Item| -> Result<()> {
            let Item::Fn {
                name,
                receiver,
                args,
                ret_type,
                is_extern: true,
                ..
            } = item
            else {
                return Ok(());
            };
            let name = name
                .map(|n| parsed.text(n).to_string())
                .unwrap_or_else(|| "new".to_string());
            if at != 0 {
                return Err(not_yet(&name, "file"));
            }
            let receiver = match receiver {
                None => None,
                Some(r) if r.is_ref && r.is_mut => Some(Hold::Exclusive),
                Some(r) if r.is_ref => Some(Hold::Shared),
                Some(_) => return Err(not_yet(&name, "`self` taken by value")),
            };
            out.push(Entry {
                owner,
                name,
                receiver,
                args,
                ret_type: ret_type.as_ref(),
                getter: false,
            });
            Ok(())
        };
        for item in &parsed.program.items {
            match &item.node {
                Item::Impl {
                    trait_name: None,
                    target,
                    methods,
                } => {
                    for method in methods {
                        take(Some(parsed.text(target.name).to_string()), &method.node)?;
                    }
                }
                node => take(None, node)?,
            }
        }
    }
    Ok(out)
}

/// Every error `enum` the entry file declares and an entry point throws, with
/// the code of each variant: `1…` in declaration order across the library.
fn variant_codes(program: &Program, entries: &[Entry]) -> Vec<(String, Vec<(String, i64)>)> {
    let Some(unit) = program.units.first() else {
        return Vec::new();
    };
    let parsed = &unit.parsed;
    let thrown: std::collections::BTreeSet<String> = entries
        .iter()
        .filter_map(|entry| program.contracts.functions.get(&entry.key()))
        .flat_map(|contract| contract.fails_with.iter().cloned())
        .collect();
    let mut next = 1;
    let mut out = Vec::new();
    for item in &parsed.program.items {
        let Item::Enum { name, variants, .. } = &item.node else {
            continue;
        };
        let error = parsed.text(*name).to_string();
        if !thrown.contains(&error) {
            continue;
        }
        let numbered = variants
            .iter()
            .map(|variant| {
                let code = next;
                next += 1;
                (parsed.text(variant.name).to_string(), code)
            })
            .collect();
        out.push((error, numbered));
    }
    out
}

/// The wrappers and the header for every entry point the program's own files
/// declare.
///
/// `concurrent` is `user_parallelism = yes`: the runtime the library starts
/// has a pool for user code.
pub fn export(
    program: &Program,
    prefix: &str,
    package: &str,
    concurrent: bool,
) -> Result<Option<Exported>> {
    let user_code = match concurrent {
        true => "Concurrent",
        false => "Sequential",
    };
    let ledger_digest = LEDGER_DIGEST;
    let upper = prefix.to_uppercase();
    let mut rust = String::from(
        "\n// --- The C boundary (ADR-284): one wrapper per entry point. ---\n\
         \n\
         /// Set by a panic at the boundary; every call after it is `E_PANICKED` (ADR-284 D8).\n\
         static __NIKAIA_POISONED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);\n\
         \n\
         /// Set by `shutdown`, cleared by `init`; every call between them is `E_NOT_RUNNING` (ADR-284 D10).\n\
         static __NIKAIA_STOPPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);\n\
         \n\
         /// Whether a call may go in: `E_NOT_RUNNING` after `shutdown`, `E_PANICKED` after a panic (ADR-284 D8, D10).\n\
         fn __nikaia_enter() -> Result<(), std::ffi::c_int> {\n    \
         if __NIKAIA_STOPPED.load(std::sync::atomic::Ordering::SeqCst) {\n        return Err(-3);\n    }\n    \
         if __NIKAIA_POISONED.load(std::sync::atomic::Ordering::SeqCst) {\n        return Err(-4);\n    }\n    \
         Ok(())\n\
         }\n\
         \n\
         std::thread_local! {\n    \
         /// This thread's last failure, as `<prefix>_last_error` renders it (ADR-284 D7).\n    \
         static __NIKAIA_LAST_ERROR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };\n\
         }\n\
         \n\
         fn __nikaia_failed(said: String) {\n    \
         __NIKAIA_LAST_ERROR.with(|last| *last.borrow_mut() = said);\n\
         }\n",
    );
    rust.push_str(&format!(
        "\n/// This thread's last failure, with its site, into the caller's buffer (ADR-284 D7).\n\
         #[unsafe(no_mangle)]\npub extern \"C\" fn {prefix}_last_error(out: *mut u8, cap: usize, written: *mut usize) -> std::ffi::c_int {{\n    \
         __NIKAIA_LAST_ERROR.with(|last| {{\n        \
         // SAFETY: the C caller hands `cap` bytes at `out` and a place for the length, or NULL (ADR-284 D6).\n        \
         unsafe {{ nikaia_std::c_boundary::hand_back(last.borrow().as_bytes(), out, cap, written) }}\n    \
         }})\n}}\n\
         \n/// Starts the library, and starts it again after `shutdown`, which is what\n\
         /// clears a panic's poison (ADR-284 D8, D10). Idempotent, from any thread.\n\
         #[unsafe(no_mangle)]\npub extern \"C\" fn {prefix}_init() -> std::ffi::c_int {{\n    \
         let _ = nikaia_std::rt::start(nikaia_std::rt::UserCode::{user_code});\n    \
         if __NIKAIA_STOPPED.load(std::sync::atomic::Ordering::SeqCst) {{\n        \
         __NIKAIA_POISONED.store(false, std::sync::atomic::Ordering::SeqCst);\n        \
         __NIKAIA_STOPPED.store(false, std::sync::atomic::Ordering::SeqCst);\n    \
         }}\n    0\n}}\n\
         \n/// Stops the library: every call after it is `E_NOT_RUNNING` until `init`. What is\n\
         /// still running is drained within `cleanup-deadline`, and what is left is `E_CLEANUP`\n\
         /// - the process goes on (ADR-284 D10).\n\
         #[unsafe(no_mangle)]\npub extern \"C\" fn {prefix}_shutdown() -> std::ffi::c_int {{\n    \
         __NIKAIA_STOPPED.store(true, std::sync::atomic::Ordering::SeqCst);\n    \
         let left = nikaia_std::rt::start(nikaia_std::rt::UserCode::{user_code}).drain();\n    \
         if left > 0 {{\n        \
         __nikaia_failed(format!(\"{{left}} cleanups did not finish within the cleanup deadline\"));\n        \
         return -6;\n    }}\n    0\n}}\n"
    ));
    let mut declarations = String::new();
    let mut any_pause = false;
    let found = entries(program)?;
    let Some(unit) = program.units.first() else {
        return Ok(None);
    };
    let parsed = &unit.parsed;
    // **A `throws` function's variants are `1…`, per library in declaration
    // order** (D7): every variant of every error `enum` the entry file
    // declares and an entry point throws.
    let codes = variant_codes(program, &found);
    let plains = plain_enums(parsed);
    let handles = handled_structs(parsed);
    let records = records(parsed, &plains)?;
    // The enums and the handles an entry point names, which the header declares.
    let mut crossing = vec![false; plains.len()];
    let mut held = vec![false; handles.len()];
    for entry in &found {
        if let Some(owner) = &entry.owner {
            match handles.iter().position(|handle| handle.name == *owner) {
                Some(at) => held[at] = true,
                None => {
                    return Err(not_yet(
                        &entry.written(),
                        "type, which is no `pub struct` of the entry file,",
                    ));
                }
            }
        }
        for ty in entry.args.iter().map(|arg| &arg.ty).chain(entry.ret_type) {
            if let Some(at) = handle_of(parsed, &handles, ty) {
                held[at] = true;
            }
        }
    }
    // **A getter per `pub` field** of every handle (D5).
    let mut all = found;
    for (handle, _) in handles.iter().zip(&held).filter(|(_, held)| **held) {
        for (field, ty) in &handle.fields {
            all.push(Entry {
                owner: Some(handle.name.clone()),
                name: field.clone(),
                receiver: Some(Hold::Shared),
                args: &[],
                ret_type: Some(ty),
                getter: true,
            });
        }
    }
    // **Every `pub extern struct` is in the header**, as C lays it out (D14).
    for record in &records {
        for (_, part) in &record.fields {
            if let Part::Choice(at) = part {
                crossing[*at] = true;
            }
        }
        rust.push_str(&mirror(record, &records, &plains));
    }
    let mut constants = String::new();
    for (error, variants) in &codes {
        for (variant, code) in variants {
            constants.push_str(&format!(
                "#define {upper}_E_{} {code} /* {error}::{variant} */\n",
                variant.to_uppercase()
            ));
        }
    }
    // **No symbol twice** (D13): a method, a getter and `_free` share a
    // handle's names.
    let mut symbols: std::collections::BTreeSet<String> = ["last_error", "init", "shutdown"]
        .iter()
        .map(|own| format!("{prefix}_{own}"))
        .collect();
    let mut claim = |symbol: &str, written: &str| -> Result<()> {
        match symbols.insert(symbol.to_string()) {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "`{written}` would be exported as `{symbol}`, which another entry point, a getter, \
                 `_free` or the library's own `init`, `shutdown` or `last_error` is already: \
                 rename it (ADR-284 D13)."
            )),
        }
    };
    for (handle, _) in handles.iter().zip(&held).filter(|(_, held)| **held) {
        let ty = &handle.name;
        let symbol = format!("{prefix}_{ty}_free");
        claim(&symbol, ty)?;
        rust.push_str(&format!(
            "\n#[unsafe(no_mangle)]\npub extern \"C\" fn {symbol}(handle: *mut nikaia_std::c_boundary::Handle<{ty}>) -> std::ffi::c_int {{\n    \
             if let Err(status) = __nikaia_enter() {{\n        return status;\n    }}\n    \
             // SAFETY: the C caller hands a handle this library made, or NULL, and uses it no more (ADR-284 D6).\n    \
             unsafe {{ nikaia_std::c_boundary::free(handle) }}\n}}\n"
        ));
        declarations.push_str(&format!("int {symbol}({prefix}_{ty} *handle);\n"));
    }
    for entry in &all {
        let written = entry.written();
        let contract = match entry.getter {
            true => None,
            false => program.contracts.functions.get(&entry.key()),
        };
        // **A function that may pause is driven by the calling thread**, over
        // the shared reactor (D9): the blocking form. The `_async` one is not
        // built yet.
        let pauses = contract.is_some_and(|c| !c.sync_claim.is_sync());
        // **What it throws is one `enum` of the entry file** (D7), whose
        // variants are the codes; anything else is not exported yet.
        let thrown: Option<&Vec<(String, i64)>> = match contract.map(|c| c.fails_with.as_slice()) {
            None | Some([]) => None,
            Some([one]) => match codes.iter().find(|(error, _)| error == one) {
                Some((_, variants)) => Some(variants),
                None => return Err(not_yet(&written, "failure")),
            },
            Some(_) => return Err(not_yet(&written, "failure")),
        };
        let symbol = match &entry.owner {
            Some(owner) => format!("{prefix}_{owner}_{}", entry.name),
            None => format!("{prefix}_{}", entry.name),
        };
        claim(&symbol, &written)?;
        let mut rust_params = Vec::new();
        let mut c_params = Vec::new();
        let mut checks = String::new();
        let mut call_args = Vec::new();
        // **`self` is the handle, held for the call** (D11): shared where the
        // method only reads it, alone where it changes it.
        if let (Some(hold), Some(owner)) = (entry.receiver, &entry.owner) {
            rust_params.push(format!(
                "__nikaia_self: *const nikaia_std::c_boundary::Handle<{owner}>"
            ));
            let (constant, how, binding) = match hold {
                Hold::Shared => ("const ", "shared", ""),
                Hold::Exclusive => ("", "exclusive", "mut "),
            };
            c_params.push(format!("{constant}{prefix}_{owner} *self"));
            checks.push_str(&format!(
                "    // SAFETY: the C caller hands a handle this library made and has not freed, or NULL (ADR-284 D5).\n    \
                 let {binding}__nikaia_self = match unsafe {{ nikaia_std::c_boundary::{how}(__nikaia_self) }} {{ Ok(held) => held, Err(status) => return status }};\n"
            ));
        }
        for (position, arg) in entry.args.iter().enumerate() {
            let param = parsed.text(arg.name).to_string();
            let local = crate::emit::escaped(&param).into_owned();
            match taken(parsed, &plains, &handles, &records, &arg.ty) {
                Some(In::Value(shape)) => {
                    rust_params.push(format!("{local}: {}", shape.rust));
                    c_params.push(format!("{} {param}", shape.c));
                    if shape.scalar {
                        checks.push_str(&format!(
                            "    let Some({local}) = char::from_u32({local}) else {{ return -1; }};\n"
                        ));
                    }
                    call_args.push(local);
                }
                Some(In::Run {
                    rust: element,
                    c,
                    as_text,
                }) => {
                    rust_params.push(format!("{local}: *const {element}, {local}_len: usize"));
                    c_params.push(format!("const {c} *{param}, size_t {param}_len"));
                    let read = match as_text {
                        true => "text",
                        false => "run",
                    };
                    // A length with no address, or text that is not UTF-8,
                    // is `E_ARGUMENT` (ADR-284 D5).
                    checks.push_str(&format!(
                        "    // SAFETY: the C caller keeps `{param}_len` values at `{param}` for the call (ADR-284 D5).\n    \
                         let Some({local}) = (unsafe {{ nikaia_std::c_boundary::{read}({local}, {local}_len) }}) else {{ return -1; }};\n"
                    ));
                    call_args.push(local);
                }
                Some(In::Bytes) => {
                    rust_params.push(format!("{local}: *const u8, {local}_len: usize"));
                    c_params.push(format!("const uint8_t *{param}, size_t {param}_len"));
                    checks.push_str(&format!(
                        "    // SAFETY: the C caller keeps `{param}_len` bytes at `{param}` for the call (ADR-284 D5).\n    \
                         let Some({local}) = (unsafe {{ nikaia_std::c_boundary::run({local}, {local}_len) }}) else {{ return -1; }};\n    \
                         let {local} = nikaia_std::bytes::Bytes::from({local});\n"
                    ));
                    // **Lent where the function only reads it**, as the
                    // ledger says and the declaration was lowered.
                    let lent =
                        contract.is_some_and(|c| crate::contracts::keeps::lends(c, position));
                    call_args.push(match lent {
                        true => format!("&{local}"),
                        false => local,
                    });
                }
                Some(In::Choice(at)) => {
                    let plain = &plains[at];
                    crossing[at] = true;
                    rust_params.push(format!("{local}: std::ffi::c_int"));
                    c_params.push(format!("{prefix}_{} {param}", plain.name));
                    // A number no variant has is `E_ARGUMENT` (ADR-284 D5).
                    let arms: Vec<String> = plain
                        .variants
                        .iter()
                        .enumerate()
                        .map(|(number, variant)| format!("{number} => {}::{variant},", plain.name))
                        .collect();
                    checks.push_str(&format!(
                        "    let {local} = match {local} {{ {} _ => return -1 }};\n",
                        arms.join(" ")
                    ));
                    call_args.push(local);
                }
                Some(In::Callback { args, result }) => {
                    // **A function pointer and the context C hands it back**
                    // (D5); called on the calling thread, and NULL is
                    // `E_ARGUMENT`. Calling it is the C caller's contract that
                    // it is a function of this signature.
                    let mut rust_args = Vec::new();
                    let mut c_args = Vec::new();
                    let mut closure_params = Vec::new();
                    let mut passed = Vec::new();
                    for (at, lent) in args.iter().enumerate() {
                        let name = format!("__nikaia_{at}");
                        match lent {
                            Lent::Value(shape) => {
                                rust_args.push(shape.rust.to_string());
                                c_args.push(shape.c.to_string());
                                let lowered = match shape.scalar {
                                    true => "char",
                                    false => shape.rust,
                                };
                                closure_params.push(format!("{name}: {lowered}"));
                                passed.push(match shape.scalar {
                                    true => format!("{name} as u32"),
                                    false => name,
                                });
                            }
                            Lent::Choice(which) => {
                                let plain = &plains[*which];
                                crossing[*which] = true;
                                rust_args.push("std::ffi::c_int".to_string());
                                c_args.push(format!("{prefix}_{}", plain.name));
                                let arms: Vec<String> = plain
                                    .variants
                                    .iter()
                                    .enumerate()
                                    .map(|(number, variant)| {
                                        format!("{}::{variant} => {number},", plain.name)
                                    })
                                    .collect();
                                closure_params.push(format!("{name}: {}", plain.name));
                                passed.push(format!("match {name} {{ {} }}", arms.join(" ")));
                            }
                            Lent::Text => {
                                rust_args.push("*const u8, usize".to_string());
                                c_args.push("const uint8_t *, size_t".to_string());
                                closure_params.push(format!("{name}: &str"));
                                passed.push(format!("{name}.as_ptr(), {name}.len()"));
                            }
                        }
                    }
                    rust_args.push("*mut std::ffi::c_void".to_string());
                    c_args.push("void *ctx".to_string());
                    let (rust_result, c_result) = match &result {
                        Some(shape) => (format!(" -> {}", shape.rust), shape.c),
                        None => (String::new(), "void"),
                    };
                    rust_params.push(format!(
                        "{local}: Option<extern \"C\" fn({}){rust_result}>, {local}_ctx: *mut std::ffi::c_void",
                        rust_args.join(", ")
                    ));
                    c_params.push(format!(
                        "{c_result} (*{param})({}), void *{param}_ctx",
                        c_args.join(", ")
                    ));
                    passed.push(format!("{local}_ctx"));
                    checks.push_str(&format!(
                        "    let Some({local}) = {local} else {{ return -1; }};\n    \
                         let {local} = move |{}|{rust_result} {{ {local}({}) }};\n",
                        closure_params.join(", "),
                        passed.join(", ")
                    ));
                    call_args.push(format!("&{local}"));
                }
                Some(In::Record(at)) => {
                    let ty = &records[at].name;
                    rust_params.push(format!("{local}: __nikaia_c_{ty}"));
                    c_params.push(format!("{prefix}_{ty} {param}"));
                    // An enum field no variant has, or a `scalar` that is
                    // none, is `E_ARGUMENT` (ADR-284 D5).
                    checks.push_str(&format!(
                        "    let Some({local}) = {local}.into_nikaia() else {{ return -1; }};\n"
                    ));
                    let lent =
                        contract.is_some_and(|c| crate::contracts::keeps::lends(c, position));
                    call_args.push(match lent {
                        true => format!("&{local}"),
                        false => local,
                    });
                }
                Some(In::Absent(present)) => match *present {
                    In::Run { .. } => {
                        rust_params.push(format!("{local}: *const u8, {local}_len: usize"));
                        c_params.push(format!("const uint8_t *{param}, size_t {param}_len"));
                        checks.push_str(&format!(
                            "    let {local} = match {local}.is_null() {{\n        \
                             true => None,\n        \
                             // SAFETY: the C caller keeps `{param}_len` bytes at `{param}` for the call (ADR-284 D5).\n        \
                             false => match unsafe {{ nikaia_std::c_boundary::text({local}, {local}_len) }} {{ Some(text) => Some(text), None => return -1 }},\n    \
                             }};\n"
                        ));
                        call_args.push(local);
                    }
                    In::Handle(at) => {
                        let ty = &handles[at].name;
                        rust_params.push(format!(
                            "{local}: *const nikaia_std::c_boundary::Handle<{ty}>"
                        ));
                        c_params.push(format!("const {prefix}_{ty} *{param}"));
                        checks.push_str(&format!(
                            "    let {local} = match {local}.is_null() {{\n        \
                             true => None,\n        \
                             // SAFETY: the C caller hands a handle this library made and has not freed (ADR-284 D5).\n        \
                             false => match unsafe {{ nikaia_std::c_boundary::shared({local}) }} {{ Ok(held) => Some(held), Err(status) => return status }},\n    \
                             }};\n"
                        ));
                        call_args.push(format!("{local}.as_deref()"));
                    }
                    _ => return Err(not_yet(&written, &format!("parameter `{param}`"))),
                },
                Some(In::Handle(at)) => {
                    let ty = &handles[at].name;
                    rust_params.push(format!(
                        "{local}: *const nikaia_std::c_boundary::Handle<{ty}>"
                    ));
                    c_params.push(format!("const {prefix}_{ty} *{param}"));
                    checks.push_str(&format!(
                        "    // SAFETY: the C caller hands a handle this library made and has not freed, or NULL (ADR-284 D5).\n    \
                         let {local} = match unsafe {{ nikaia_std::c_boundary::shared({local}) }} {{ Ok(held) => held, Err(status) => return status }};\n"
                    ));
                    call_args.push(format!("&*{local}"));
                }
                None => return Err(not_yet(&written, &format!("parameter `{param}`"))),
            }
        }
        let result = match entry.ret_type {
            Some(ty) => match handed(parsed, &plains, &handles, &records, ty) {
                Some(Out::Value(shape)) => {
                    rust_params.push(format!("out: *mut {}", shape.rust));
                    c_params.push(format!("{} *out", shape.c));
                    Some(Out::Value(shape))
                }
                Some(Out::Buffer) => {
                    rust_params.push("out: *mut u8, cap: usize, written: *mut usize".to_string());
                    c_params.push("uint8_t *out, size_t cap, size_t *written".to_string());
                    Some(Out::Buffer)
                }
                Some(Out::Choice(at)) => {
                    crossing[at] = true;
                    rust_params.push("out: *mut std::ffi::c_int".to_string());
                    c_params.push(format!("{prefix}_{} *out", plains[at].name));
                    Some(Out::Choice(at))
                }
                Some(Out::Handle(at)) => {
                    let ty = &handles[at].name;
                    rust_params.push(format!(
                        "out: *mut *mut nikaia_std::c_boundary::Handle<{ty}>"
                    ));
                    c_params.push(format!("{prefix}_{ty} **out"));
                    Some(Out::Handle(at))
                }
                Some(Out::Record(at)) => {
                    let ty = &records[at].name;
                    rust_params.push(format!("out: *mut __nikaia_c_{ty}"));
                    c_params.push(format!("{prefix}_{ty} *out"));
                    Some(Out::Record(at))
                }
                Some(Out::AbsentHandle(at)) => {
                    let ty = &handles[at].name;
                    rust_params.push(format!(
                        "out: *mut *mut nikaia_std::c_boundary::Handle<{ty}>"
                    ));
                    c_params.push(format!("{prefix}_{ty} **out"));
                    Some(Out::AbsentHandle(at))
                }
                None => {
                    return Err(not_yet(
                        &written,
                        match entry.getter {
                            true => "field's type",
                            false => "result",
                        },
                    ));
                }
            },
            None => None,
        };
        let name = crate::emit::escaped(&entry.name);
        let call = match (&entry.owner, entry.receiver, entry.getter) {
            (_, _, true) => format!("__nikaia_self.{name}.clone()"),
            (_, Some(_), false) => format!("__nikaia_self.{name}({})", call_args.join(", ")),
            (Some(owner), None, false) => format!("{owner}::{name}({})", call_args.join(", ")),
            (None, None, false) => format!("{name}({})", call_args.join(", ")),
        };
        let call = match pauses {
            true => format!(
                "{{ let _ = nikaia_std::rt::start(nikaia_std::rt::UserCode::{user_code}); \
                 nikaia_std::rt::exec::block_on(__nikaia_cancellable({call})) }}"
            ),
            false => call,
        };
        let handed_back = match &result {
            Some(Out::Value(shape)) => {
                let value = match shape.scalar {
                    true => "value as u32",
                    false => "value",
                };
                format!(
                    "Ok(value) => {{\n            \
                     // SAFETY: the C caller hands a place for one value, or NULL (ADR-284 D7).\n            \
                     unsafe {{ nikaia_std::c_boundary::put(out, {value}) }};\n            0\n        }}"
                )
            }
            Some(Out::Buffer) => "Ok(value) => {\n            \
                 // SAFETY: the C caller hands `cap` bytes at `out` and a place for the length, or NULL (ADR-284 D6).\n            \
                 unsafe { nikaia_std::c_boundary::hand_back(AsRef::<[u8]>::as_ref(&value), out, cap, written) }\n        }"
                .to_string(),
            Some(Out::Choice(at)) => {
                let plain = &plains[*at];
                let arms: Vec<String> = plain
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(number, variant)| format!("{}::{variant} => {number},", plain.name))
                    .collect();
                format!(
                    "Ok(value) => {{\n            \
                     let number: std::ffi::c_int = match value {{ {} }};\n            \
                     // SAFETY: the C caller hands a place for one value, or NULL (ADR-284 D7).\n            \
                     unsafe {{ nikaia_std::c_boundary::put(out, number) }};\n            0\n        }}",
                    arms.join(" ")
                )
            }
            // A new handle, which the caller frees; none is made where `out`
            // is NULL.
            Some(Out::Handle(_)) => "Ok(value) => {\n            \
                 if !out.is_null() {\n                \
                 // SAFETY: the C caller hands a place for one handle (ADR-284 D7).\n                \
                 unsafe { nikaia_std::c_boundary::put(out, nikaia_std::c_boundary::handle(value)) };\n            \
                 }\n            0\n        }"
                .to_string(),
            Some(Out::Record(at)) => format!(
                "Ok(value) => {{\n            \
                 // SAFETY: the C caller hands a place for one value, or NULL (ADR-284 D7).\n            \
                 unsafe {{ nikaia_std::c_boundary::put(out, __nikaia_c_{}::from_nikaia(value)) }};\n            0\n        }}",
                records[*at].name
            ),
            // `null` is NULL (D5).
            Some(Out::AbsentHandle(_)) => "Ok(value) => {\n            \
                 if !out.is_null() {\n                \
                 let made = value.map_or(std::ptr::null_mut(), nikaia_std::c_boundary::handle);\n                \
                 // SAFETY: the C caller hands a place for one handle (ADR-284 D7).\n                \
                 unsafe { nikaia_std::c_boundary::put(out, made) };\n            \
                 }\n            0\n        }"
                .to_string(),
            None => "Ok(()) => 0,".to_string(),
        };
        // A failure is the variant's code, and its message, with its site,
        // is what `<prefix>_last_error` hands back (D7).
        let (handed_back, failed) = match thrown {
            None => (handed_back, String::new()),
            Some(variants) => {
                let error = contract
                    .and_then(|c| c.fails_with.first())
                    .cloned()
                    .unwrap_or_default();
                let arms: Vec<String> = variants
                    .iter()
                    .map(|(variant, code)| format!("{error}::{variant} {{ .. }} => {code},"))
                    .collect();
                (
                    handed_back
                        .replacen("Ok(", "Ok(Ok(", 1)
                        .replacen(") =>", ")) =>", 1),
                    format!(
                        "\n        Ok(Err(thrown)) => {{\n            \
                         __nikaia_failed(thrown.full());\n            \
                         match thrown.split().0 {{ {} }}\n        }}",
                        arms.join(" ")
                    ),
                )
            }
        };
        // `AssertUnwindSafe`: after a panic the library is poisoned, and
        // nothing it held is looked at again (D8). A call that pauses may be
        // cancelled at a pause point instead, which is `E_CANCELLED` (D19).
        let ran = match pauses {
            true => format!(
                "let __nikaia_ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {call}));\n    \
                 let __nikaia_ran = match __nikaia_ran {{\n        \
                 Ok(Some(value)) => Ok(value),\n        \
                 Ok(None) => {{\n            __nikaia_failed(\"the call was cancelled\".to_string());\n            return -7;\n        }}\n        \
                 Err(panic) => Err(panic),\n    }};\n    \
                 match __nikaia_ran"
            ),
            false => format!(
                "match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {call}))"
            ),
        };
        rust.push_str(&format!(
            "\n#[unsafe(no_mangle)]\npub extern \"C\" fn {symbol}({}) -> std::ffi::c_int {{\n    \
             if let Err(status) = __nikaia_enter() {{\n        return status;\n    }}\n\
             {checks}    \
             {ran} {{\n        {handed_back}{failed}\n        \
             Err(_) => {{\n            __NIKAIA_POISONED.store(true, std::sync::atomic::Ordering::SeqCst);\n            \
             __nikaia_failed(\"the library panicked; it answers E_PANICKED until shutdown and init have run\".to_string());\n            \
             -4\n        }}\n    }}\n}}\n",
            rust_params.join(", "),
        ));
        let c_params = match c_params.is_empty() {
            true => "void".to_string(),
            false => c_params.join(", "),
        };
        declarations.push_str(&format!("int {symbol}({c_params});\n"));
        // **The `_async` form** (D9, D19): the same call on a library thread,
        // `done` called exactly once with its status, and a ticket that
        // cancels it at its next pause point. What C handed it stays C's to
        // keep until `done`.
        if pauses {
            any_pause = true;
            let names = parameter_names(&rust_params);
            let forwarded = names.join(", ");
            let mut async_params = rust_params.clone();
            async_params.push(
                "__nikaia_done: Option<extern \"C\" fn(std::ffi::c_int, *mut std::ffi::c_void)>, \
                 __nikaia_done_ctx: *mut std::ffi::c_void, \
                 __nikaia_op: *mut *mut nikaia_std::c_boundary::Handle<std::sync::Arc<__NikaiaTicket>>"
                    .to_string(),
            );
            let async_symbol = format!("{symbol}_async");
            claim(&async_symbol, &written)?;
            rust.push_str(&format!(
                "\n#[unsafe(no_mangle)]\npub extern \"C\" fn {async_symbol}({}) -> std::ffi::c_int {{\n    \
                 if let Err(status) = __nikaia_enter() {{\n        return status;\n    }}\n    \
                 let Some(__nikaia_done) = __nikaia_done else {{ return -1; }};\n    \
                 let ticket = std::sync::Arc::new(__NikaiaTicket::default());\n    \
                 if !__nikaia_op.is_null() {{\n        \
                 // SAFETY: the C caller hands a place for one ticket (ADR-284 D19).\n        \
                 unsafe {{ nikaia_std::c_boundary::put(__nikaia_op, nikaia_std::c_boundary::handle(ticket.clone())) }};\n    \
                 }}\n    \
                 // SAFETY: the C caller keeps what it handed this call, and `done`'s context, until `done` (ADR-284 D9).\n    \
                 let sent = unsafe {{ nikaia_std::c_boundary::sent(({forwarded}, __nikaia_done_ctx)) }};\n    \
                 std::thread::spawn(move || {{\n        \
                 let ({forwarded}, __nikaia_done_ctx) = sent.into_inner();\n        \
                 __NIKAIA_TICKET.with(|current| *current.borrow_mut() = Some(ticket.clone()));\n        \
                 let status = {symbol}({forwarded});\n        \
                 ticket.done.store(true, std::sync::atomic::Ordering::SeqCst);\n        \
                 __nikaia_done(status, __nikaia_done_ctx);\n    \
                 }});\n    0\n}}\n",
                async_params.join(", "),
            ));
            let mut c_async = match c_params.as_str() {
                "void" => Vec::new(),
                some => vec![some.to_string()],
            };
            c_async.push(format!(
                "void (*done)(int status, void *ctx), void *ctx, {prefix}_op **op"
            ));
            declarations.push_str(&format!("int {async_symbol}({});\n", c_async.join(", ")));
        }
    }
    // **The ticket of an `_async` call** (D19): `cancel` and `op_free`, and the
    // cancellation every pausing call is driven under.
    if any_pause {
        for own in ["cancel", "op_free"] {
            claim(&format!("{prefix}_{own}"), own)?;
        }
        rust.push_str(&format!(
            "\n/// An `_async` call's ticket (ADR-284 D19).\n\
             #[derive(Default)]\npub struct __NikaiaTicket {{\n    \
             cancelled: std::sync::atomic::AtomicBool,\n    \
             done: std::sync::atomic::AtomicBool,\n    \
             waker: std::sync::Mutex<Option<std::task::Waker>>,\n}}\n\
             \nimpl __NikaiaTicket {{\n    \
             fn cancel(&self) {{\n        \
             self.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);\n        \
             let waker = self.waker.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();\n        \
             if let Some(waker) = waker {{\n            waker.wake();\n        }}\n    }}\n}}\n\
             \nstd::thread_local! {{\n    \
             /// The ticket of the `_async` call this library thread runs.\n    \
             static __NIKAIA_TICKET: std::cell::RefCell<Option<std::sync::Arc<__NikaiaTicket>>> = const {{ std::cell::RefCell::new(None) }};\n}}\n\
             \n/// `work`, or `None` once its ticket is cancelled: at the next pause point (ADR-284 D19).\n\
             async fn __nikaia_cancellable<T>(work: impl std::future::Future<Output = T>) -> Option<T> {{\n    \
             let ticket = __NIKAIA_TICKET.with(|current| current.borrow().clone());\n    \
             let mut work = std::pin::pin!(work);\n    \
             std::future::poll_fn(|context| {{\n        \
             if let Some(ticket) = &ticket {{\n            \
             *ticket.waker.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(context.waker().clone());\n            \
             if ticket.cancelled.load(std::sync::atomic::Ordering::SeqCst) {{\n                \
             return std::task::Poll::Ready(None);\n            }}\n        }}\n        \
             work.as_mut().poll(context).map(Some)\n    \
             }})\n    .await\n}}\n\
             \n/// Cancels an `_async` call at its next pause point; after `done`, or twice, it is `OK` (ADR-284 D19).\n\
             #[unsafe(no_mangle)]\npub extern \"C\" fn {prefix}_cancel(op: *const nikaia_std::c_boundary::Handle<std::sync::Arc<__NikaiaTicket>>) -> std::ffi::c_int {{\n    \
             // SAFETY: the C caller hands a ticket this library made and has not freed, or NULL (ADR-284 D19).\n    \
             match unsafe {{ nikaia_std::c_boundary::shared(op) }} {{\n        \
             Ok(ticket) => {{\n            ticket.cancel();\n            0\n        }}\n        \
             Err(status) => status,\n    }}\n}}\n\
             \n/// Frees a ticket after `done`; before it, `E_ARGUMENT`, and the ticket is kept (ADR-284 D19).\n\
             #[unsafe(no_mangle)]\npub extern \"C\" fn {prefix}_op_free(op: *mut nikaia_std::c_boundary::Handle<std::sync::Arc<__NikaiaTicket>>) -> std::ffi::c_int {{\n    \
             if op.is_null() {{\n        return 0;\n    }}\n    \
             // SAFETY: as `cancel`'s.\n    \
             let finished = match unsafe {{ nikaia_std::c_boundary::shared(op) }} {{\n        \
             Ok(ticket) => ticket.done.load(std::sync::atomic::Ordering::SeqCst),\n        \
             Err(status) => return status,\n    }};\n    \
             if !finished {{\n        return -1;\n    }}\n    \
             // SAFETY: the C caller uses the ticket no more (ADR-284 D19).\n    \
             unsafe {{ nikaia_std::c_boundary::free(op) }}\n}}\n"
        ));
        declarations.push_str(&format!(
            "\n/* An _async call's ticket: cancel it at its next pause point, free it after done (ADR-284 D19). */\n\
             int {prefix}_cancel({prefix}_op *op);\nint {prefix}_op_free({prefix}_op *op);\n"
        ));
    }
    // **A handle is an opaque struct** C holds by its address (D5).
    let mut types = String::new();
    if any_pause {
        types.push_str(&format!("typedef struct {prefix}_op {prefix}_op;\n\n"));
    }
    for (handle, _) in handles.iter().zip(&held).filter(|(_, held)| **held) {
        types.push_str(&format!(
            "typedef struct {prefix}_{0} {prefix}_{0};\n\n",
            handle.name
        ));
    }
    // **An `enum` without payload is a C `enum`**, numbered in declaration
    // order (D5), each value `<PREFIX>_<TYPE>_<VARIANT>` (D13).
    for (plain, _) in plains
        .iter()
        .zip(&crossing)
        .filter(|(_, crosses)| **crosses)
    {
        let values: Vec<String> = plain
            .variants
            .iter()
            .enumerate()
            .map(|(number, variant)| {
                format!(
                    "    {upper}_{}_{} = {number}",
                    plain.name.to_uppercase(),
                    variant.to_uppercase()
                )
            })
            .collect();
        types.push_str(&format!(
            "typedef enum {{\n{}\n}} {prefix}_{};\n\n",
            values.join(",\n"),
            plain.name
        ));
    }
    // A struct a field holds comes first: C wants it complete.
    // One that holds itself has no size, which rustc says; this only must
    // not walk it forever.
    fn ordered(at: usize, records: &[Record], seen: &mut Vec<usize>, walking: &mut Vec<usize>) {
        if seen.contains(&at) || walking.contains(&at) {
            return;
        }
        walking.push(at);
        for (_, part) in &records[at].fields {
            if let Part::Record(inner) = part {
                ordered(*inner, records, seen, walking);
            }
        }
        seen.push(at);
    }
    let mut order = Vec::new();
    for at in 0..records.len() {
        ordered(at, &records, &mut order, &mut Vec::new());
    }
    for record in order.iter().map(|at| &records[*at]) {
        let fields: Vec<String> = record
            .fields
            .iter()
            .map(|(field, part)| {
                let c = match part {
                    Part::Value(shape) => shape.c.to_string(),
                    Part::Choice(at) => format!("{prefix}_{}", plains[*at].name),
                    Part::Record(at) => format!("{prefix}_{}", records[*at].name),
                };
                format!("    {c} {field};")
            })
            .collect();
        types.push_str(&format!(
            "typedef struct {{\n{}\n}} {prefix}_{};\n\n",
            fields.join("\n"),
            record.name
        ));
    }
    let guard = format!("{upper}_H");
    let header = format!(
        "/* {package}.h - GENERATED by nikaia from {package}'s ledger. Do not edit.\n \
         * ledger: {ledger_digest}\n */\n\
         #ifndef {guard}\n#define {guard}\n\n\
         #include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\n\n\
         #ifdef __cplusplus\nextern \"C\" {{\n#endif\n\n\
         /* Every entry point returns a status (ADR-284 D7). */\n\
         #define {upper}_OK 0\n\
         #define {upper}_E_ARGUMENT (-1)\n\
         #define {upper}_E_TOO_SMALL (-2)\n\
         #define {upper}_E_NOT_RUNNING (-3)\n\
         #define {upper}_E_PANICKED (-4)\n\
         #define {upper}_E_REENTRANT (-5)\n\
         #define {upper}_E_CLEANUP (-6)\n\
         #define {upper}_E_CANCELLED (-7)\n\n\
         /* What the entry points throw, numbered per library (ADR-284 D7). */\n\
         {constants}\n\
         {types}\
         /* Start the library, and start it again after shutdown (ADR-284 D10). */\n\
         int {prefix}_init(void);\n\
         /* Stop it: every call after this is {upper}_E_NOT_RUNNING until init. */\n\
         int {prefix}_shutdown(void);\n\n\
         /* This thread's last failure, with its site (ADR-284 D7). */\n\
         int {prefix}_last_error(uint8_t *out, size_t cap, size_t *written);\n\n\
         {declarations}\n\
         #ifdef __cplusplus\n}}\n#endif\n\n#endif\n"
    );
    Ok(Exported {
        rust: match declarations.is_empty() {
            true => String::new(),
            false => rust,
        },
        header,
    }
    .exporting(!declarations.is_empty()))
}

impl Exported {
    /// `None` for a package that declares no entry point: a package the
    /// library depends on exports nothing of its own.
    fn exporting(self, any: bool) -> Option<Exported> {
        any.then_some(self)
    }
}

/// The refusal for a library build whose entry package exports nothing.
pub fn nothing_exported() -> anyhow::Error {
    anyhow::anyhow!(
        "`artifact = \"c-library\"`, and the package exports nothing: write `pub extern fn` \
         before a function C may call (ADR-284 D4)."
    )
}
