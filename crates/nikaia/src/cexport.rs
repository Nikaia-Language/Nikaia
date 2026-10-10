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

use crate::ast::{Item, Type, VariantFields};
use crate::modules::Program;

mod node;

/// The two halves of a library's boundary.
pub struct Exported {
    /// Appended to the entry file's lowering.
    pub rust: String,
    /// `<package>.h`.
    pub header: String,
    /// `<package>/__init__.py`, what `nikaia bind python` writes (D26).
    pub python: String,
    /// `node/<package>.c`, what `nikaia bind node` writes (D28).
    pub node: String,
}

/// `node/binding.gyp`, which builds `nikaia bind node`'s module (D28).
pub fn node_gyp(package: &str) -> String {
    node::gyp(package)
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
    /// `ref Array[T]` or `Vec[T]` of a `pub extern struct`: an address and a
    /// count C keeps for the call (D16). `true` where the callee takes it whole.
    Records(usize, bool),
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
    /// `Vec[T]` of a `pub extern struct`: the caller's buffer, counted in
    /// structs (D16).
    Records(usize),
    /// `T?` of a number: the value's out-parameter, then `bool *present`,
    /// the status `OK` either way (D30).
    AbsentValue(ByValue),
    /// `T?` of a `pub extern struct`, the same way (D30).
    AbsentRecord(usize),
}

impl Out {
    /// Whether a `bool *present` follows the value's out-parameter (D30).
    fn flagged(&self) -> bool {
        matches!(self, Out::AbsentValue(_) | Out::AbsentRecord(_))
    }
}

/// **A `pub extern struct`** of the entry file (ADR-284 D14, D15): C's layout,
/// crossing by value. The wrapper reads and writes it through a mirror whose
/// enum and `scalar` fields are plain numbers, checked on the way in.
struct Record {
    name: String,
    fields: Vec<(String, Part)>,
    /// **A `pub extern enum`** (D32): its variants, as the tagged union lays
    /// them out. Empty for a struct, whose fields are `fields`.
    variants: Vec<Alternative>,
}

/// One variant of a `pub extern enum` (D32): its member of the union, its
/// name in lower case, and its fields - `_0`, `_1`, … where it has no names.
struct Alternative {
    name: String,
    member: String,
    positional: bool,
    fields: Vec<(String, Part)>,
}

impl Record {
    fn is_enum(&self) -> bool {
        !self.variants.is_empty()
    }

    /// Every field it holds, of every variant.
    fn parts(&self) -> impl Iterator<Item = &Part> {
        self.fields
            .iter()
            .chain(self.variants.iter().flat_map(|variant| &variant.fields))
            .map(|(_, part)| match part {
                Part::Array(element, _) => element.as_ref(),
                other => other,
            })
    }
}

/// One field of a [`Record`].
enum Part {
    Value(ByValue),
    Choice(usize),
    Record(usize),
    /// `Array[T, N]` of any of the others: laid out as C lays out `T x[N]`
    /// (ADR-152 D2, ADR-284 D15), each element converted as a field would be.
    Array(Box<Part>, i64),
}

fn records(parsed: &crate::parser::Parsed, plains: &[Plain]) -> Result<Vec<Record>> {
    enum Declared<'a> {
        Struct(&'a Vec<crate::ast::FieldDef>),
        Enum(&'a Vec<crate::ast::EnumVariant>),
    }
    let declared: Vec<(&str, Declared)> = parsed
        .program
        .items
        .iter()
        .filter_map(|item| match &item.node {
            Item::Struct {
                name,
                fields,
                is_extern: true,
                ..
            } => Some((parsed.text(*name), Declared::Struct(fields))),
            Item::Enum {
                name,
                variants,
                is_extern: true,
                ..
            } => Some((parsed.text(*name), Declared::Enum(variants))),
            _ => None,
        })
        .collect();
    // One field, or one element of an array field.
    let one = |ty: &Type| -> Option<Part> {
        if let Some(shape) = by_value(parsed, ty) {
            return Some(Part::Value(shape));
        }
        if let Some(at) = choice(parsed, plains, ty) {
            return Some(Part::Choice(at));
        }
        declared
            .iter()
            .position(|(other, _)| *other == parsed.text(ty.name))
            .filter(|_| ty.generics.is_empty() && !ty.is_nullable)
            .map(Part::Record)
    };
    let part = |ty: &Type, written: String| -> Result<Part> {
        let part = match (parsed.text(ty.name), ty.generics.as_slice()) {
            ("Array", [element, size]) => one(element)
                .zip(size.count)
                .map(|(element, count)| Part::Array(Box::new(element), count)),
            _ => one(ty),
        };
        part.ok_or_else(|| not_yet(&written, "field of an `extern` struct or enum"))
    };
    declared
        .iter()
        .map(|(name, declared)| match declared {
            Declared::Struct(fields) => Ok(Record {
                name: name.to_string(),
                fields: fields
                    .iter()
                    .map(|field| {
                        let field_name = parsed.text(field.name).to_string();
                        let at = part(&field.ty, format!("{name}.{field_name}"))?;
                        Ok((field_name, at))
                    })
                    .collect::<Result<Vec<_>>>()?,
                variants: Vec::new(),
            }),
            Declared::Enum(variants) => Ok(Record {
                name: name.to_string(),
                fields: Vec::new(),
                variants: variants
                    .iter()
                    .map(|variant| {
                        let variant_name = parsed.text(variant.name).to_string();
                        let (positional, fields) = match &variant.fields {
                            VariantFields::Unit => (false, Vec::new()),
                            VariantFields::Tuple(types) => (
                                true,
                                types
                                    .iter()
                                    .enumerate()
                                    .map(|(at, ty)| {
                                        Ok((
                                            format!("_{at}"),
                                            part(ty, format!("{name}::{variant_name}.{at}"))?,
                                        ))
                                    })
                                    .collect::<Result<Vec<_>>>()?,
                            ),
                            VariantFields::Named(named) => (
                                false,
                                named
                                    .iter()
                                    .map(|field| {
                                        let field_name = parsed.text(field.name).to_string();
                                        let at = part(
                                            &field.ty,
                                            format!("{name}::{variant_name}.{field_name}"),
                                        )?;
                                        Ok((field_name, at))
                                    })
                                    .collect::<Result<Vec<_>>>()?,
                            ),
                        };
                        Ok(Alternative {
                            member: variant_name.to_lowercase(),
                            name: variant_name,
                            positional,
                            fields,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            }),
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

/// **The records, each after every record its fields hold**: C, and Python's
/// `ctypes`, want a struct complete before another holds it. One that holds
/// itself has no size, which rustc says; this only must not walk it forever.
fn records_in_order(records: &[Record]) -> Vec<usize> {
    fn ordered(at: usize, records: &[Record], seen: &mut Vec<usize>, walking: &mut Vec<usize>) {
        if seen.contains(&at) || walking.contains(&at) {
            return;
        }
        walking.push(at);
        for part in records[at].parts() {
            if let Part::Record(inner) = part {
                ordered(*inner, records, seen, walking);
            }
        }
        seen.push(at);
    }
    let mut order = Vec::new();
    for at in 0..records.len() {
        ordered(at, records, &mut order, &mut Vec::new());
    }
    order
}

/// **One field's, or one element's, conversion** between the mirror and the
/// struct: the type in the mirror, what `of` is in the language - `None` where
/// C's value is none - and what it is in the mirror.
fn converted(
    part: &Part,
    records: &[Record],
    plains: &[Plain],
    of: &str,
) -> (String, String, String) {
    match part {
        Part::Value(shape) if shape.scalar => (
            "u32".to_string(),
            format!("char::from_u32({of})"),
            format!("{of} as u32"),
        ),
        Part::Value(shape) => (
            shape.rust.to_string(),
            format!("Some({of})"),
            of.to_string(),
        ),
        Part::Choice(at) => {
            let plain = &plains[*at];
            let inward: Vec<String> = plain
                .variants
                .iter()
                .enumerate()
                .map(|(number, variant)| format!("{number} => Some({}::{variant}),", plain.name))
                .collect();
            let outward: Vec<String> = plain
                .variants
                .iter()
                .enumerate()
                .map(|(number, variant)| format!("{}::{variant} => {number},", plain.name))
                .collect();
            (
                "std::ffi::c_int".to_string(),
                format!("match {of} {{ {} _ => None }}", inward.join(" ")),
                format!("match {of} {{ {} }}", outward.join(" ")),
            )
        }
        Part::Record(at) => {
            let inner = &records[*at].name;
            (
                format!("__nikaia_c_{inner}"),
                format!("{of}.into_nikaia()"),
                format!("__nikaia_c_{inner}::from_nikaia({of})"),
            )
        }
        Part::Array(..) => unreachable!("an array's element is no array"),
    }
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
            Part::Array(element, count) => {
                let (lowered, inward, outward) = converted(element, records, plains, "e");
                (
                    format!("[{lowered}; {count}]"),
                    format!(
                        "self.{local}.iter().map(|&e| {inward}).collect::<Option<Vec<_>>>()?.try_into().ok()?"
                    ),
                    format!("value.{local}.map(|e| {outward})"),
                )
            }
            one => {
                let (lowered, inward, _) =
                    converted(one, records, plains, &format!("self.{local}"));
                let (_, _, outward) = converted(one, records, plains, &format!("value.{local}"));
                (lowered, format!("({inward})?"), outward)
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

/// **The mirror of a `pub extern enum`** (D32): the tag as a C `int` and a
/// union with one struct per variant that has fields, as `#[repr(C)]` lays out
/// the enum - and its conversions, `None` for a tag that names no variant.
fn tagged_mirror(record: &Record, records: &[Record], plains: &[Plain]) -> String {
    let ty = &record.name;
    let mut out = String::new();
    let mut members = Vec::new();
    let mut into = Vec::new();
    let mut from = Vec::new();
    for (tag, variant) in record.variants.iter().enumerate() {
        let path = format!("{ty}::{}", crate::emit::escaped(&variant.name));
        if variant.fields.is_empty() {
            into.push(format!("{tag} => Some({path}),"));
            from.push(format!(
                "{path} => Self {{ tag: {tag}, ..Self::zeroed() }},"
            ));
            continue;
        }
        let shape = format!("__nikaia_cv_{ty}_{}", variant.name);
        let member = crate::emit::escaped(&variant.member).into_owned();
        members.push(format!("{member}: {shape}"));
        let mut fields = String::new();
        let mut taken = Vec::new();
        let mut made = Vec::new();
        let mut bound = Vec::new();
        for (field, part) in &variant.fields {
            let local = crate::emit::escaped(field).into_owned();
            let named = match variant.positional {
                true => format!("__nikaia{local}"),
                false => local.clone(),
            };
            let (lowered, inward, outward) = match part {
                Part::Array(element, count) => {
                    let (lowered, inward, outward) = converted(element, records, plains, "e");
                    (
                        format!("[{lowered}; {count}]"),
                        format!(
                            "v.{local}.iter().map(|&e| {inward}).collect::<Option<Vec<_>>>()?.try_into().ok()?"
                        ),
                        format!("{named}.map(|e| {outward})"),
                    )
                }
                one => {
                    let (lowered, inward, _) =
                        converted(one, records, plains, &format!("v.{local}"));
                    let (_, _, outward) = converted(one, records, plains, &named);
                    (lowered, format!("({inward})?"), outward)
                }
            };
            fields.push_str(&format!("    {local}: {lowered},\n"));
            taken.push(match variant.positional {
                true => inward,
                false => format!("{local}: {inward}"),
            });
            made.push(format!("{local}: {outward}"));
            bound.push(named);
        }
        out.push_str(&format!(
            "\n#[repr(C)]\n#[derive(Clone, Copy)]\n#[allow(non_camel_case_types)]\n\
             pub struct {shape} {{\n{fields}}}\n"
        ));
        let (built, pattern) = match variant.positional {
            true => (
                format!("{path}({})", taken.join(", ")),
                format!("{path}({})", bound.join(", ")),
            ),
            false => (
                format!("{path} {{ {} }}", taken.join(", ")),
                format!("{path} {{ {} }}", bound.join(", ")),
            ),
        };
        into.push(format!(
            "{tag} => {{\n            \
             // SAFETY: the tag names this variant, so C wrote this member (ADR-284 D32).\n            \
             let v = unsafe {{ self.u.{member} }};\n            \
             Some({built})\n        }}"
        ));
        from.push(format!(
            "{pattern} => {{\n            \
             let mut made = Self {{ tag: {tag}, ..Self::zeroed() }};\n            \
             made.u.{member} = {shape} {{ {} }};\n            \
             made\n        }}",
            made.join(", ")
        ));
    }
    let union = match members.is_empty() {
        true => String::new(),
        false => format!(
            "\n#[repr(C)]\n#[derive(Clone, Copy)]\n#[allow(non_camel_case_types)]\n\
             pub union __nikaia_cu_{ty} {{ {} }}\n",
            members.join(", ")
        ),
    };
    let held = match members.is_empty() {
        true => String::new(),
        false => format!("    u: __nikaia_cu_{ty},\n"),
    };
    format!(
        "{out}{union}\n/// `{ty}` as C lays it out and hands it over: a tag and a union (ADR-284 D32).\n\
         #[repr(C)]\n#[derive(Clone, Copy)]\n#[allow(non_camel_case_types)]\n\
         pub struct __nikaia_c_{ty} {{\n    tag: std::ffi::c_int,\n{held}}}\n\
         \n#[allow(dead_code, unreachable_patterns)]\nimpl __nikaia_c_{ty} {{\n    \
         /// All zero: every member of the union is numbers, for which zero is a value.\n    \
         fn zeroed() -> Self {{\n        \
         // SAFETY: a C int and a union of `#[repr(C)]` structs of numbers and `bool`; all-zero bytes are a value of each.\n        \
         unsafe {{ std::mem::zeroed() }}\n    }}\n    \
         fn into_nikaia(self) -> Option<{ty}> {{\n        \
         match self.tag {{\n        {}\n        _ => None,\n        }}\n    }}\n    \
         fn from_nikaia(value: {ty}) -> Self {{\n        \
         match value {{\n        {}\n        }}\n    }}\n}}\n",
        into.join("\n        "),
        from.join("\n        ")
    )
}

/// An `enum` without payload the entry file declares (ADR-284 D5): its name
/// and its variants, numbered in declaration order.
struct Plain {
    name: String,
    variants: Vec<String>,
    /// What its C constants start with after the prefix: its name, or for
    /// the kind of an `enum` with payload that enum's (D31).
    stem: String,
}

fn plain_enums(parsed: &crate::parser::Parsed) -> Vec<Plain> {
    parsed
        .program
        .items
        .iter()
        .filter_map(|item| match &item.node {
            Item::Enum {
                name,
                variants,
                is_extern: false,
                ..
            } if variants
                .iter()
                .all(|variant| matches!(variant.fields, crate::ast::VariantFields::Unit)) =>
            {
                Some(Plain {
                    name: parsed.text(*name).to_string(),
                    variants: variants
                        .iter()
                        .map(|variant| parsed.text(variant.name).to_string())
                        .collect(),
                    stem: parsed.text(*name).to_uppercase(),
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
    if let (name @ ("Array" | "Vec"), [element]) = (parsed.text(ty.name), ty.generics.as_slice())
        && !ty.is_nullable
        && let Some(at) = record_of(parsed, records, element)
    {
        return match (name, ty.is_view) {
            ("Array", true) => Some(In::Records(at, false)),
            ("Vec", false) => Some(In::Records(at, true)),
            _ => None,
        };
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
    if let ("Vec", [element], false, false) = (
        parsed.text(ty.name),
        ty.generics.as_slice(),
        ty.is_view,
        ty.is_nullable,
    ) && let Some(at) = record_of(parsed, records, element)
    {
        return Some(Out::Records(at));
    }
    if ty.is_nullable && !ty.is_view {
        let mut present = ty.clone();
        present.is_nullable = false;
        if let Some(at) = record_of(parsed, records, &present) {
            return Some(Out::AbsentRecord(at));
        }
        if let Some(shape) = by_value(parsed, &present) {
            return Some(Out::AbsentValue(shape));
        }
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
    /// A getter of an enum's variant: the variant, and the pattern that binds
    /// the field as `__nikaia_v` (D31).
    of_variant: Option<(String, String)>,
    /// `_kind` of an enum, which writes the kind `plains[at]` (D31).
    kind_of: Option<usize>,
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
/// its `pub` fields, each of which gets a getter - or a `pub enum` with
/// payload, read through `_kind` and a getter per field of each variant (D31).
struct Handled<'a> {
    name: String,
    fields: Vec<(String, &'a Type)>,
    /// An enum's variants, each with the pattern that tells it, and its
    /// fields: the getter's name, the pattern that binds the field as
    /// `__nikaia_v`, and its type. Empty for a struct.
    variants: Vec<Shown<'a>>,
}

struct Shown<'a> {
    name: String,
    pattern: String,
    fields: Vec<(String, String, &'a Type)>,
}

impl Handled<'_> {
    fn is_enum(&self) -> bool {
        !self.variants.is_empty()
    }
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
                variants: Vec::new(),
            }),
            Item::Enum {
                name,
                variants,
                is_public: true,
                is_extern: false,
            } if variants
                .iter()
                .any(|variant| !matches!(variant.fields, VariantFields::Unit)) =>
            {
                let ty = parsed.text(*name).to_string();
                let variants = variants
                    .iter()
                    .map(|variant| {
                        let name = parsed.text(variant.name).to_string();
                        let path = format!("{ty}::{}", crate::emit::escaped(&name));
                        let (pattern, fields) = match &variant.fields {
                            VariantFields::Unit => (path.clone(), Vec::new()),
                            VariantFields::Tuple(types) => (
                                format!("{path}(..)"),
                                types
                                    .iter()
                                    .enumerate()
                                    .map(|(at, field)| {
                                        let places: Vec<&str> = (0..types.len())
                                            .map(|other| match other == at {
                                                true => "__nikaia_v",
                                                false => "_",
                                            })
                                            .collect();
                                        (
                                            format!("{name}_{at}"),
                                            format!("{path}({})", places.join(", ")),
                                            field,
                                        )
                                    })
                                    .collect(),
                            ),
                            VariantFields::Named(named) => (
                                format!("{path} {{ .. }}"),
                                named
                                    .iter()
                                    .map(|field| {
                                        let label = parsed.text(field.name);
                                        (
                                            format!("{name}_{label}"),
                                            format!(
                                                "{path} {{ {}: __nikaia_v, .. }}",
                                                crate::emit::escaped(label)
                                            ),
                                            &field.ty,
                                        )
                                    })
                                    .collect(),
                            ),
                        };
                        Shown {
                            name,
                            pattern,
                            fields,
                        }
                    })
                    .collect();
                Some(Handled {
                    name: ty,
                    fields: Vec::new(),
                    variants,
                })
            }
            _ => None,
        })
        .collect()
}

/// What an entry hands back: its result's shape, or the kind `_kind` writes
/// (D31). `Some(None)` is a result that does not cross.
fn result_of(
    entry: &Entry,
    parsed: &crate::parser::Parsed,
    plains: &[Plain],
    handles: &[Handled],
    records: &[Record],
) -> Option<Option<Out>> {
    match (entry.kind_of, entry.ret_type) {
        (Some(at), _) => Some(Some(Out::Choice(at))),
        (None, Some(ty)) => Some(handed(parsed, plains, handles, records, ty)),
        (None, None) => None,
    }
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
                of_variant: None,
                kind_of: None,
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

/// **One error type an entry point throws**, with the status each of its
/// values is (D7, D33).
struct Raising {
    /// As the ledger names it: `ConfigError`, `io::IoError`.
    error: String,
    codes: Vec<Code>,
}

/// One status a thrown error is.
struct Code {
    /// The variant, where the type has variants.
    variant: Option<String>,
    number: i64,
    /// What follows `<PACKAGE>_E_` in the header.
    constant: String,
    /// The `errno` it is, for a `std` error that is one (D33).
    errno: Option<&'static str>,
}

impl Code {
    /// The pattern of the error's type that is this code, in the wrapper.
    fn pattern(&self, error: &str) -> String {
        match &self.variant {
            Some(variant) => format!("{error}::{variant} {{ .. }}"),
            None => "_".to_string(),
        }
    }
}

/// **The library's own variants are `100 000…`** (D33): every variant of
/// every error `enum` the entry file declares and an entry point throws, in
/// declaration order across the library. After them, every `std` error an
/// entry point throws, with `std`'s numbers for the target.
fn variant_codes(program: &Program, entries: &[Entry], target: &str) -> Vec<Raising> {
    let Some(unit) = program.units.first() else {
        return Vec::new();
    };
    let parsed = &unit.parsed;
    let thrown: std::collections::BTreeSet<String> = entries
        .iter()
        .filter_map(|entry| program.contracts.functions.get(&entry.key()))
        .flat_map(|contract| contract.fails_with.iter().cloned())
        .collect();
    let mut next = 100_000;
    let mut out = Vec::new();
    for item in &parsed.program.items {
        let Item::Enum { name, variants, .. } = &item.node else {
            continue;
        };
        let error = parsed.text(*name).to_string();
        if !thrown.contains(&error) {
            continue;
        }
        let codes = variants
            .iter()
            .map(|variant| {
                let variant = parsed.text(variant.name).to_string();
                next += 1;
                Code {
                    constant: variant.to_uppercase(),
                    variant: Some(variant),
                    number: next - 1,
                    errno: None,
                }
            })
            .collect();
        out.push(Raising { error, codes });
    }
    for error in &thrown {
        if let Some(codes) = std_codes(error, target) {
            out.push(Raising {
                error: error.clone(),
                codes,
            });
        }
    }
    out
}

/// **`std`'s errors, below `100 000`** (D33, Part III 15.1): an `errno` has
/// the number of the target the library is built for; the rest are fixed, a
/// block of a hundred per type, appended and never moved.
fn std_codes(error: &str, target: &str) -> Option<Vec<Code>> {
    let one = |variant: Option<&str>, number: i64, constant: &str, errno| Code {
        variant: variant.map(str::to_string),
        number,
        constant: constant.to_string(),
        errno,
    };
    let numbered = |name: &'static str| one(None, errno(target, name), "", Some(name));
    Some(match error {
        "io::IoError" => vec![
            Code {
                variant: Some("NotFound".to_string()),
                constant: "IO_NOT_FOUND".to_string(),
                ..numbered("ENOENT")
            },
            Code {
                variant: Some("PermissionDenied".to_string()),
                constant: "IO_PERMISSION_DENIED".to_string(),
                ..numbered("EACCES")
            },
            Code {
                variant: Some("NotText".to_string()),
                constant: "IO_NOT_TEXT".to_string(),
                ..numbered("EILSEQ")
            },
            one(Some("Outside"), 1000, "IO_OUTSIDE", None),
            one(Some("Other"), 1001, "IO_OTHER", None),
        ],
        "task::Crashed" => vec![one(None, 1100, "TASK_CRASHED", None)],
        "supervisor::Escalated" => vec![one(None, 1200, "SUPERVISOR_ESCALATED", None)],
        "cleanup::Failure" => vec![one(None, 1300, "CLEANUP_FAILURE", None)],
        "Overtaken" => vec![one(None, 1400, "OVERTAKEN", None)],
        _ => return None,
    })
}

/// An `errno`'s number on the target: Linux's on both Linux targets, WASI's
/// under WebAssembly (D33).
fn errno(target: &str, name: &str) -> i64 {
    let wasi = target.starts_with("wasm32");
    match (name, wasi) {
        ("ENOENT", false) => 2,
        ("EACCES", false) => 13,
        ("EILSEQ", false) => 84,
        ("ENOENT", true) => 44,
        ("EACCES", true) => 2,
        ("EILSEQ", true) => 25,
        _ => unreachable!("an errno this table has: {name}"),
    }
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
    target: &str,
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
         /// What the library holds comes from the caller's allocator if it was set first,\n\
         /// the platform heap otherwise (ADR-284 D6).\n\
         #[global_allocator]\n\
         static __NIKAIA_HEAP: nikaia_std::c_boundary::Heap = nikaia_std::c_boundary::Heap::new();\n\
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
         }\n\
         \n\
         /// A handle the call could not hold, said for `<prefix>_last_error` (ADR-284 D7, D11).\n\
         #[allow(dead_code)]\n\
         fn __nikaia_refused(status: std::ffi::c_int) -> std::ffi::c_int {\n    \
         __nikaia_failed(match status {\n        \
         -5 => \"a call on a handle this thread holds already, from a callback of a call on it\",\n        \
         _ => \"no handle where one is needed, or one that is closed\",\n    \
         }.to_string());\n    \
         status\n\
         }\n",
    );
    rust.push_str(&format!(
        "\n/// This thread's last failure, with its site, into the caller's buffer (ADR-284 D7).\n\
         #[unsafe(no_mangle)]\npub extern \"C\" fn {prefix}_last_error(out: *mut u8, cap: usize, written: *mut usize) -> std::ffi::c_int {{\n    \
         __NIKAIA_LAST_ERROR.with(|last| {{\n        \
         // SAFETY: the C caller hands `cap` bytes at `out` and a place for the length, or NULL (ADR-284 D6).\n        \
         unsafe {{ nikaia_std::c_boundary::hand_back(last.borrow().as_bytes(), out, cap, written) }}\n    \
         }})\n}}\n\
         \n/// Hands what the library holds to the caller's allocator: before `init` and\n\
         /// before any other call, or it is `E_ARGUMENT` (ADR-284 D6, D10).\n\
         #[unsafe(no_mangle)]\npub extern \"C\" fn {prefix}_set_allocator(alloc: Option<nikaia_std::c_boundary::Alloc>, free: Option<nikaia_std::c_boundary::Free>, ctx: *mut std::ffi::c_void) -> std::ffi::c_int {{\n    \
         __NIKAIA_HEAP.set(alloc, free, ctx)\n}}\n\
         \n/// Starts the library, and starts it again after `shutdown`, which is what\n\
         /// clears a panic's poison (ADR-284 D8, D10). Idempotent, from any thread.\n\
         #[unsafe(no_mangle)]\npub extern \"C\" fn {prefix}_init() -> std::ffi::c_int {{\n    \
         __NIKAIA_HEAP.settle();\n    \
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
    let mut python = Python::default();
    let mut node = node::Node::default();
    let mut any_pause = false;
    let found = entries(program)?;
    let Some(unit) = program.units.first() else {
        return Ok(None);
    };
    let parsed = &unit.parsed;
    // **What the entry points throw, as codes** (D7, D33): the library's own
    // variants from `100 000`, `std`'s errors below.
    let codes = variant_codes(program, &found, target);
    let mut plains = plain_enums(parsed);
    let handles = handled_structs(parsed);
    let records = records(parsed, &plains)?;
    // **The kind of each `enum` with payload** C holds is a C `enum` of its
    // own, `<prefix>_<Type>_Kind`, its values `<PREFIX>_<TYPE>_<VARIANT>`
    // (D31), and a Rust `enum` the wrapper hands it back through.
    let kinds: Vec<Option<usize>> = handles
        .iter()
        .map(|handle| {
            handle.is_enum().then(|| {
                plains.push(Plain {
                    name: format!("{}_Kind", handle.name),
                    variants: handle.variants.iter().map(|v| v.name.clone()).collect(),
                    stem: handle.name.to_uppercase(),
                });
                plains.len() - 1
            })
        })
        .collect();
    // The enums and the handles an entry point names, which the header declares.
    let mut crossing = vec![false; plains.len()];
    let mut held = vec![false; handles.len()];
    for entry in &found {
        if let Some(owner) = &entry.owner {
            match handles.iter().position(|handle| handle.name == *owner) {
                Some(at) => held[at] = true,
                // **A method of a struct C holds by value** (D16).
                None if records.iter().any(|record| record.name == *owner) => {}
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
    // **A getter per `pub` field** of every handle (D5), and of an enum
    // `_kind` and a getter per field of each variant (D31).
    let mut all = found;
    for ((handle, _), kind) in handles
        .iter()
        .zip(&held)
        .zip(&kinds)
        .filter(|((_, held), _)| **held)
    {
        for (field, ty) in &handle.fields {
            all.push(Entry {
                owner: Some(handle.name.clone()),
                name: field.clone(),
                receiver: Some(Hold::Shared),
                args: &[],
                ret_type: Some(ty),
                getter: true,
                of_variant: None,
                kind_of: None,
            });
        }
        let Some(kind) = *kind else { continue };
        let ty = &handle.name;
        let arms: Vec<String> = handle
            .variants
            .iter()
            .map(|variant| {
                format!(
                    "{} => ({ty}_Kind::{}, \"{}\"),",
                    variant.pattern,
                    crate::emit::escaped(&variant.name),
                    variant.name
                )
            })
            .collect();
        rust.push_str(&format!(
            "\n/// The kinds of `{ty}`, as `{prefix}_{ty}_kind` writes them (ADR-284 D31).\n\
             #[allow(non_camel_case_types, dead_code)]\n\
             #[derive(Clone, Copy)]\n\
             enum {ty}_Kind {{ {} }}\n\
             \n\
             #[allow(non_snake_case, dead_code)]\n\
             fn __nikaia_kind_{ty}(value: &{ty}) -> ({ty}_Kind, &'static str) {{\n    \
             match value {{ {} }}\n\
             }}\n",
            handle
                .variants
                .iter()
                .map(|variant| crate::emit::escaped(&variant.name).into_owned())
                .collect::<Vec<_>>()
                .join(", "),
            arms.join(" ")
        ));
        all.push(Entry {
            owner: Some(ty.clone()),
            name: "kind".to_string(),
            receiver: Some(Hold::Shared),
            args: &[],
            ret_type: None,
            getter: true,
            of_variant: None,
            kind_of: Some(kind),
        });
        for variant in &handle.variants {
            for (getter, pattern, field) in &variant.fields {
                all.push(Entry {
                    owner: Some(ty.clone()),
                    name: getter.clone(),
                    receiver: Some(Hold::Shared),
                    args: &[],
                    ret_type: Some(field),
                    getter: true,
                    of_variant: Some((variant.name.clone(), pattern.clone())),
                    kind_of: None,
                });
            }
        }
    }
    // **Every `pub extern struct` is in the header**, as C lays it out (D14).
    for record in &records {
        for part in record.parts() {
            if let Part::Choice(at) = part {
                crossing[*at] = true;
            }
        }
        rust.push_str(&match record.is_enum() {
            true => tagged_mirror(record, &records, &plains),
            false => mirror(record, &records, &plains),
        });
    }
    let mut constants = String::new();
    for raising in &codes {
        for code in &raising.codes {
            let what = match &code.variant {
                Some(variant) => format!("{}::{variant}", raising.error),
                None => raising.error.clone(),
            };
            let errno = code.errno.map(|e| format!(", {e}")).unwrap_or_default();
            constants.push_str(&format!(
                "#define {upper}_E_{} {} /* {what}{errno} */\n",
                code.constant, code.number
            ));
        }
    }
    // **No symbol twice** (D13): a method, a getter and `_free` share a
    // handle's names.
    let mut symbols: std::collections::BTreeSet<String> = [
        "last_error",
        "init",
        "shutdown",
        "set_allocator",
        "alloc_fn",
        "free_fn",
    ]
    .iter()
    .map(|own| format!("{prefix}_{own}"))
    .collect();
    let mut claim = |symbol: &str, written: &str| -> Result<()> {
        match symbols.insert(symbol.to_string()) {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "`{written}` would be exported as `{symbol}`, which another entry point, a getter, \
                 `_free` or the library's own `init`, `shutdown`, `set_allocator` or `last_error` is already: \
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
        // **What it throws is the entry file's `enum`s and `std`'s errors**
        // (D7, D33), each of whose values is a code; anything else is not
        // exported yet.
        let thrown: Vec<&Raising> = contract
            .map(|c| c.fails_with.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|one| codes.iter().find(|raising| raising.error == *one))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| not_yet(&written, "failure"))?;
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
        // **`self` of a struct C holds by value** (D16): `const T*` where the
        // method reads it, `T*` where it changes it - and no lock, since it is
        // the caller's memory. A change is written back only where the method
        // returned.
        let by_value = entry
            .owner
            .as_ref()
            .is_some_and(|owner| records.iter().any(|record| record.name == *owner));
        if let (Some(hold), Some(owner), true) = (entry.receiver, &entry.owner, by_value) {
            if hold == Hold::Exclusive && pauses {
                return Err(not_yet(&written, "changing `self` across a pause"));
            }
            let (constant, pointer) = match hold {
                Hold::Shared => ("const ", "*const"),
                Hold::Exclusive => ("", "*mut"),
            };
            rust_params.push(format!("__nikaia_at: {pointer} __nikaia_c_{owner}"));
            c_params.push(format!("{constant}{prefix}_{owner} *self"));
            let binding = match hold {
                Hold::Shared => "",
                Hold::Exclusive => "mut ",
            };
            checks.push_str(&format!(
                "    // SAFETY: the C caller hands its struct, or NULL, and leaves it alone for the call (ADR-284 D16).\n    \
                 let Some(__nikaia_self) = (unsafe {{ nikaia_std::c_boundary::read(__nikaia_at) }}) else {{ return -1; }};\n    \
                 let Some({binding}__nikaia_self) = __nikaia_self.into_nikaia() else {{ return -1; }};\n"
            ));
        } else if let (Some(hold), Some(owner)) = (entry.receiver, &entry.owner) {
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
                 let {binding}__nikaia_self = match unsafe {{ nikaia_std::c_boundary::{how}(__nikaia_self) }} {{ Ok(held) => held, Err(status) => return __nikaia_refused(status) }};\n"
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
                Some(In::Records(at, whole)) => {
                    let ty = &records[at].name;
                    rust_params.push(format!(
                        "{local}: *const __nikaia_c_{ty}, {local}_len: usize"
                    ));
                    c_params.push(format!("const {prefix}_{ty} *{param}, size_t {param}_len"));
                    // A length with no address, or a struct no value of the
                    // type is, is `E_ARGUMENT` (ADR-284 D5, D16).
                    checks.push_str(&format!(
                        "    // SAFETY: the C caller keeps `{param}_len` structs at `{param}` for the call (ADR-284 D16).\n    \
                         let Some({local}) = (unsafe {{ nikaia_std::c_boundary::run({local}, {local}_len) }}) else {{ return -1; }};\n    \
                         let Some({local}) = {local}.iter().map(|one| one.into_nikaia()).collect::<Option<Vec<{ty}>>>() else {{ return -1; }};\n"
                    ));
                    let lent = !whole
                        || contract.is_some_and(|c| crate::contracts::keeps::lends(c, position));
                    call_args.push(match lent {
                        true => format!("&{local}"),
                        false => local,
                    });
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
                             false => match unsafe {{ nikaia_std::c_boundary::shared({local}) }} {{ Ok(held) => Some(held), Err(status) => return __nikaia_refused(status) }},\n    \
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
                         let {local} = match unsafe {{ nikaia_std::c_boundary::shared({local}) }} {{ Ok(held) => held, Err(status) => return __nikaia_refused(status) }};\n"
                    ));
                    call_args.push(format!("&*{local}"));
                }
                None => return Err(not_yet(&written, &format!("parameter `{param}`"))),
            }
        }
        let result = match result_of(entry, parsed, &plains, &handles, &records) {
            Some(made) => match made {
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
                Some(Out::Records(at)) => {
                    let ty = &records[at].name;
                    rust_params.push(format!(
                        "out: *mut __nikaia_c_{ty}, cap: usize, written: *mut usize"
                    ));
                    c_params.push(format!("{prefix}_{ty} *out, size_t cap, size_t *written"));
                    Some(Out::Records(at))
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
                Some(Out::AbsentValue(shape)) => {
                    rust_params.push(format!("out: *mut {}, present: *mut bool", shape.rust));
                    c_params.push(format!("{} *out, bool *present", shape.c));
                    Some(Out::AbsentValue(shape))
                }
                Some(Out::AbsentRecord(at)) => {
                    let ty = &records[at].name;
                    rust_params.push(format!("out: *mut __nikaia_c_{ty}, present: *mut bool"));
                    c_params.push(format!("{prefix}_{ty} *out, bool *present"));
                    Some(Out::AbsentRecord(at))
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
        // **A getter of a variant** answers `E_ARGUMENT` on a handle that
        // holds another, and `last_error` names the one it holds (D31).
        if let (Some((variant, pattern)), Some(owner)) = (&entry.of_variant, &entry.owner) {
            checks.push_str(&format!(
                "    if !matches!(&*__nikaia_self, {pattern}) {{\n        \
                 __nikaia_failed(format!(\"the {owner} holds `{{}}`, not `{variant}`\", __nikaia_kind_{owner}(&__nikaia_self).1));\n        \
                 return -1;\n    \
                 }}\n"
            ));
        }
        let call = match (&entry.owner, entry.receiver, entry.getter) {
            (Some(owner), _, true) if entry.kind_of.is_some() => {
                format!("__nikaia_kind_{owner}(&__nikaia_self).0")
            }
            (_, _, true) if entry.of_variant.is_some() => {
                let (_, pattern) = entry.of_variant.as_ref().expect("a variant's getter");
                format!(
                    "match &*__nikaia_self {{ {pattern} => __nikaia_v.clone(), _ => unreachable!(\"checked above\") }}"
                )
            }
            (_, _, true) => format!("__nikaia_self.{name}.clone()"),
            (_, Some(_), false) => format!("__nikaia_self.{name}({})", call_args.join(", ")),
            (Some(owner), None, false) => format!("{owner}::{name}({})", call_args.join(", ")),
            (None, None, false) => format!("{name}({})", call_args.join(", ")),
        };
        let call = match (by_value, entry.receiver) {
            (true, Some(Hold::Exclusive)) => format!(
                "{{ let __nikaia_result = {call}; \
                 // SAFETY: as the read above; the struct is written back as the method left it (ADR-284 D16).\n\
                 unsafe {{ nikaia_std::c_boundary::put(__nikaia_at, __nikaia_c_{owner}::from_nikaia(__nikaia_self)) }}; \
                 __nikaia_result }}",
                owner = entry.owner.as_deref().unwrap_or_default()
            ),
            _ => call,
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
            Some(Out::Records(at)) => format!(
                "Ok(value) => {{\n            \
                 let values: Vec<__nikaia_c_{0}> = value.into_iter().map(__nikaia_c_{0}::from_nikaia).collect();\n            \
                 // SAFETY: the C caller hands room for `cap` structs at `out` and a place for the count, or NULL (ADR-284 D16).\n            \
                 unsafe {{ nikaia_std::c_boundary::hand_back_run(&values, out, cap, written) }}\n        }}",
                records[*at].name
            ),
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
            // `null` leaves `out` as it was and writes `false` (D30).
            Some(Out::AbsentValue(shape)) => {
                let value = match shape.scalar {
                    true => "value as u32",
                    false => "value",
                };
                format!(
                    "Ok(value) => {{\n            \
                     let held = value.is_some();\n            \
                     if let Some(value) = value {{\n                \
                     // SAFETY: the C caller hands a place for one value, or NULL (ADR-284 D7).\n                \
                     unsafe {{ nikaia_std::c_boundary::put(out, {value}) }};\n            \
                     }}\n            \
                     // SAFETY: the C caller hands a place for the flag, or NULL (ADR-284 D30).\n            \
                     unsafe {{ nikaia_std::c_boundary::put(present, held) }};\n            0\n        }}"
                )
            }
            Some(Out::AbsentRecord(at)) => format!(
                "Ok(value) => {{\n            \
                 let held = value.is_some();\n            \
                 if let Some(value) = value {{\n                \
                 // SAFETY: the C caller hands a place for one value, or NULL (ADR-284 D7).\n                \
                 unsafe {{ nikaia_std::c_boundary::put(out, __nikaia_c_{}::from_nikaia(value)) }};\n            \
                 }}\n            \
                 // SAFETY: the C caller hands a place for the flag, or NULL (ADR-284 D30).\n            \
                 unsafe {{ nikaia_std::c_boundary::put(present, held) }};\n            0\n        }}",
                records[*at].name
            ),
            None => "Ok(()) => 0,".to_string(),
        };
        // A failure is the variant's code, and its message, with its site,
        // is what `<prefix>_last_error` hands back (D7).
        // One type's codes, matched on its value.
        let arms = |raising: &Raising| -> String {
            raising
                .codes
                .iter()
                .map(|code| format!("{} => {},", code.pattern(&raising.error), code.number))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let (handed_back, failed) = match thrown.as_slice() {
            [] => (handed_back, String::new()),
            // Several types travel in the generated sum, one variant per type
            // (ADR-280 D15).
            several => {
                let matched = match several {
                    [one] => format!("match thrown.split().0 {{ {} }}", arms(one)),
                    _ => {
                        let members: Vec<String> = several
                            .iter()
                            .map(|raising| raising.error.clone())
                            .collect();
                        let sum = crate::emit::sum_name(&members);
                        format!(
                            "match thrown {{ {} }}",
                            several
                                .iter()
                                .map(|raising| format!(
                                    "crate::{sum}::{}(thrown) => match thrown.split().0 {{ {} }},",
                                    crate::emit::sum_variant(&raising.error),
                                    arms(raising)
                                ))
                                .collect::<Vec<_>>()
                                .join(" ")
                        )
                    }
                };
                (
                    handed_back
                        .replacen("Ok(", "Ok(Ok(", 1)
                        .replacen(") =>", ")) =>", 1),
                    format!(
                        "\n        Ok(Err(thrown)) => {{\n            \
                         __nikaia_failed(thrown.full());\n            \
                         {matched}\n        }}"
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
        let text_out = entry
            .ret_type
            .is_some_and(|ty| parsed.text(ty.name) == "String");
        python.entry(
            entry, &symbol, parsed, &plains, &handles, &records, text_out, pauses,
        );
        node.entry(
            entry, &symbol, prefix, parsed, &plains, &handles, &records, text_out, pauses,
        );
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
                 // A library thread, kept for the next call (`rt::library`).\n    \
                 nikaia_std::rt::library::run(Box::new(move || {{\n        \
                 let ({forwarded}, __nikaia_done_ctx) = sent.into_inner();\n        \
                 __NIKAIA_TICKET.with(|current| *current.borrow_mut() = Some(ticket.clone()));\n        \
                 let status = {symbol}({forwarded});\n        \
                 __NIKAIA_TICKET.with(|current| *current.borrow_mut() = None);\n        \
                 ticket.done.store(true, std::sync::atomic::Ordering::SeqCst);\n        \
                 __nikaia_done(status, __nikaia_done_ctx);\n    \
                 }}));\n    0\n}}\n",
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
                    plain.stem,
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
    let order = records_in_order(&records);
    for record in order.iter().map(|at| &records[*at]) {
        let c_field = |field: &str, part: &Part| -> String {
            let (c, size) = match part {
                Part::Value(shape) => (shape.c.to_string(), String::new()),
                Part::Choice(at) => (format!("{prefix}_{}", plains[*at].name), String::new()),
                Part::Record(at) => (format!("{prefix}_{}", records[*at].name), String::new()),
                Part::Array(element, count) => (
                    match element.as_ref() {
                        Part::Value(shape) => shape.c.to_string(),
                        Part::Choice(at) => format!("{prefix}_{}", plains[*at].name),
                        Part::Record(at) => format!("{prefix}_{}", records[*at].name),
                        Part::Array(..) => unreachable!("an array's element is no array"),
                    },
                    format!("[{count}]"),
                ),
            };
            format!("{c} {field}{size};")
        };
        // **A `pub extern enum`** (D32): its tag, and a union with a struct
        // per variant that has fields, named as the variant in lower case.
        if record.is_enum() {
            let ty = &record.name;
            let tags: Vec<String> = record
                .variants
                .iter()
                .enumerate()
                .map(|(tag, variant)| {
                    format!(
                        "    {upper}_{}_{} = {tag}",
                        ty.to_uppercase(),
                        variant.name.to_uppercase()
                    )
                })
                .collect();
            let members: Vec<String> = record
                .variants
                .iter()
                .filter(|variant| !variant.fields.is_empty())
                .map(|variant| {
                    let fields: Vec<String> = variant
                        .fields
                        .iter()
                        .map(|(field, part)| c_field(field, part))
                        .collect();
                    format!(
                        "        struct {{ {} }} {};",
                        fields.join(" "),
                        variant.member
                    )
                })
                .collect();
            let union = match members.is_empty() {
                true => String::new(),
                false => format!("    union {{\n{}\n    }};\n", members.join("\n")),
            };
            types.push_str(&format!(
                "typedef enum {{\n{}\n}} {prefix}_{ty}_Tag;\n\n\
                 typedef struct {{\n    {prefix}_{ty}_Tag tag;\n{union}}} {prefix}_{ty};\n\n",
                tags.join(",\n")
            ));
            continue;
        }
        let fields: Vec<String> = record
            .fields
            .iter()
            .map(|(field, part)| format!("    {}", c_field(field, part)))
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
         /* What the entry points throw: the library's own from 100000, std's below (ADR-284 D33). */\n\
         {constants}\n\
         {types}\
         /* What the library holds across calls comes from alloc and free, with ctx,\n\
            when this is called before init and before any other call; it is\n\
            {upper}_E_ARGUMENT after (ADR-284 D6). */\n\
         typedef void *(*{prefix}_alloc_fn)(size_t size, size_t align, void *ctx);\n\
         typedef void (*{prefix}_free_fn)(void *at, size_t size, size_t align, void *ctx);\n\
         int {prefix}_set_allocator({prefix}_alloc_fn alloc, {prefix}_free_fn free, void *ctx);\n\n\
         /* Start the library, and start it again after shutdown (ADR-284 D10). */\n\
         int {prefix}_init(void);\n\
         /* Stop it: every call after this is {upper}_E_NOT_RUNNING until init. */\n\
         int {prefix}_shutdown(void);\n\n\
         /* This thread's last failure, with its site (ADR-284 D7). */\n\
         int {prefix}_last_error(uint8_t *out, size_t cap, size_t *written);\n\n\
         {declarations}\n\
         #ifdef __cplusplus\n}}\n#endif\n\n#endif\n"
    );
    let python = python.module(
        package, prefix, &codes, &plains, &handles, &held, &records, any_pause,
    );
    let node = node.module(package, prefix, &codes, &plains, &handles, &held, &records);
    Ok(Exported {
        rust: match declarations.is_empty() {
            true => String::new(),
            false => rust,
        },
        header,
        python,
        node,
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

// --- `nikaia bind python` (ADR-284 D26, D27) -----------------------------------
//
// **A `ctypes` module over the library**, written from the same reading of the
// package as the header, so that the two cannot disagree: a status is an
// exception, a buffer is `str` or `bytes` with the size query done here, a
// handle is a class with its methods, a property per `pub` field, `close()` and
// a context manager, an `extern` struct a `ctypes.Structure`, an `enum` an
// `IntEnum`, `null` is `None`, and a callback a callable `CFUNCTYPE` keeps alive
// for the call. Only the entry points (D29). Not written yet: the `_async`
// form's awaitable and a stream as a generator; a pausing function is called
// in its blocking form.

/// The Python of one library, gathered entry by entry.
#[derive(Default)]
struct Python {
    /// Module-level functions.
    functions: Vec<String>,
    /// Each handle's class body: its constructor, methods and properties.
    classes: std::collections::BTreeMap<String, Vec<String>>,
    /// `_lib.<symbol>.argtypes = …` lines.
    signatures: Vec<String>,
    /// Whether any entry has an `_async` form, which needs the ticket.
    any_async: bool,
    /// Whether any entry is a stream, which needs `_stream`.
    any_stream: bool,
}

/// A name Python may bind: one of its words gets a `_` after it.
fn python_name(name: &str) -> String {
    const WORDS: &[&str] = &[
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
        "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
        "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return",
        "try", "while", "with", "yield", "self",
    ];
    match WORDS.contains(&name) {
        true => format!("{name}_"),
        false => name.to_string(),
    }
}

/// The `ctypes` type of a value that crosses by value.
fn python_scalar(shape: &ByValue) -> &'static str {
    match shape.rust {
        "i32" => "ctypes.c_int32",
        "i64" => "ctypes.c_int64",
        "u8" => "ctypes.c_uint8",
        "f64" => "ctypes.c_double",
        "bool" => "ctypes.c_bool",
        _ => "ctypes.c_uint32",
    }
}

impl Python {
    /// One entry point: a function, or a handle's constructor, method or
    /// property.
    #[allow(clippy::too_many_arguments)]
    fn entry(
        &mut self,
        entry: &Entry,
        symbol: &str,
        parsed: &crate::parser::Parsed,
        plains: &[Plain],
        handles: &[Handled],
        records: &[Record],
        text_out: bool,
        pauses: bool,
    ) {
        let mut def_params: Vec<String> = Vec::new();
        let mut argtypes: Vec<String> = Vec::new();
        let mut before: Vec<String> = Vec::new();
        let mut args: Vec<String> = Vec::new();
        let mut keep: Vec<String> = Vec::new();
        let mut raising: Vec<String> = Vec::new();
        let by_value = entry
            .owner
            .as_ref()
            .is_some_and(|owner| records.iter().any(|record| record.name == *owner));
        match (entry.receiver.is_some(), by_value, &entry.owner) {
            (true, true, Some(owner)) => {
                argtypes.push(format!("ctypes.POINTER({owner})"));
                args.push("ctypes.byref(self)".to_string());
            }
            (true, _, _) => {
                argtypes.push("ctypes.c_void_p".to_string());
                args.push("self._live()".to_string());
            }
            _ => {}
        }
        for arg in entry.args {
            let name = python_name(parsed.text(arg.name));
            def_params.push(name.clone());
            match taken(parsed, plains, handles, records, &arg.ty) {
                Some(In::Value(shape)) => {
                    argtypes.push(python_scalar(&shape).to_string());
                    args.push(match shape.scalar {
                        true => format!("ord({name})"),
                        false => name,
                    });
                }
                Some(In::Run { rust, as_text, .. }) => {
                    let element = match (as_text, rust) {
                        (true, _) => "ctypes.c_uint8",
                        (false, "i32") => "ctypes.c_int32",
                        (false, "i64") => "ctypes.c_int64",
                        (false, "u8") => "ctypes.c_uint8",
                        (false, "f64") => "ctypes.c_double",
                        (false, _) => "ctypes.c_bool",
                    };
                    argtypes.push(format!("ctypes.POINTER({element})"));
                    argtypes.push("ctypes.c_size_t".to_string());
                    match as_text {
                        true => before.push(format!("{name}_at, {name}_len = _text({name})")),
                        false => {
                            before.push(format!("{name}_at, {name}_len = _run({element}, {name})"))
                        }
                    }
                    args.push(format!("{name}_at"));
                    args.push(format!("{name}_len"));
                }
                Some(In::Bytes) => {
                    argtypes.push("ctypes.POINTER(ctypes.c_uint8)".to_string());
                    argtypes.push("ctypes.c_size_t".to_string());
                    before.push(format!("{name}_at, {name}_len = _bytes({name})"));
                    args.push(format!("{name}_at"));
                    args.push(format!("{name}_len"));
                }
                Some(In::Choice(at)) => {
                    argtypes.push("ctypes.c_int".to_string());
                    args.push(format!("int({}({name}))", plains[at].name));
                }
                Some(In::Record(at)) => {
                    argtypes.push(records[at].name.clone());
                    args.push(name);
                }
                Some(In::Records(at, _)) => {
                    let ty = &records[at].name;
                    argtypes.push(format!("ctypes.POINTER({ty})"));
                    argtypes.push("ctypes.c_size_t".to_string());
                    before.push(format!("{name}_at, {name}_len = _run({ty}, {name})"));
                    args.push(format!("{name}_at"));
                    args.push(format!("{name}_len"));
                }
                Some(In::Handle(_)) => {
                    argtypes.push("ctypes.c_void_p".to_string());
                    args.push(format!("{name}._live()"));
                }
                Some(In::Absent(present)) => match *present {
                    In::Handle(_) => {
                        argtypes.push("ctypes.c_void_p".to_string());
                        args.push(format!("None if {name} is None else {name}._live()"));
                    }
                    _ => {
                        argtypes.push("ctypes.POINTER(ctypes.c_uint8)".to_string());
                        argtypes.push("ctypes.c_size_t".to_string());
                        before.push(format!(
                            "{name}_at, {name}_len = (None, 0) if {name} is None else _text({name})"
                        ));
                        args.push(format!("{name}_at"));
                        args.push(format!("{name}_len"));
                    }
                },
                Some(In::Callback { args: lent, result }) => {
                    let mut c_args: Vec<String> = Vec::new();
                    let mut params: Vec<String> = Vec::new();
                    let mut handed: Vec<String> = Vec::new();
                    for (at, part) in lent.iter().enumerate() {
                        match part {
                            Lent::Value(shape) => {
                                c_args.push(python_scalar(shape).to_string());
                                params.push(format!("a{at}"));
                                handed.push(match shape.scalar {
                                    true => format!("chr(a{at})"),
                                    false => format!("a{at}"),
                                });
                            }
                            Lent::Choice(which) => {
                                c_args.push("ctypes.c_int".to_string());
                                params.push(format!("a{at}"));
                                handed.push(format!("{}(a{at})", plains[*which].name));
                            }
                            Lent::Text => {
                                c_args.push("ctypes.POINTER(ctypes.c_uint8)".to_string());
                                c_args.push("ctypes.c_size_t".to_string());
                                params.push(format!("a{at}, a{at}_len"));
                                handed.push(format!(
                                    "ctypes.string_at(a{at}, a{at}_len).decode(\"utf-8\")"
                                ));
                            }
                        }
                    }
                    c_args.push("ctypes.c_void_p".to_string());
                    params.push("ctx".to_string());
                    let restype = match &result {
                        Some(shape) => python_scalar(shape).to_string(),
                        None => "None".to_string(),
                    };
                    let kind = format!("ctypes.CFUNCTYPE({restype}, {})", c_args.join(", "));
                    argtypes.push(kind.clone());
                    argtypes.push("ctypes.c_void_p".to_string());
                    // **What the callback raises is raised by the call**, once
                    // it has returned: `ctypes` cannot carry an exception out
                    // through C, so it is held and the callback answers its
                    // default.
                    let default = match &result {
                        Some(_) => "0",
                        None => "None",
                    };
                    before.push(format!("{name}_raised = []"));
                    before.push(format!(
                        "{name}_c = {kind}(lambda {}: _calling({name}_raised, {default}, {name}, {}))",
                        params.join(", "),
                        handed.join(", ")
                    ));
                    raising.push(format!("{name}_raised"));
                    // Kept alive until the call has returned (D27).
                    keep.push(format!("{name}_c"));
                    args.push(format!("{name}_c"));
                    args.push("None".to_string());
                }
                None => {}
            }
        }
        // What comes back, through the out-parameter.
        let mut after: Vec<String> = Vec::new();
        let result = result_of(entry, parsed, plains, handles, records).flatten();
        let call = |args: &[String]| format!("_lib.{symbol}({})", args.join(", "));
        let body_call: String;
        match &result {
            None => {
                body_call = format!("_check({})", call(&args));
            }
            Some(Out::Buffer) => {
                argtypes.push("ctypes.POINTER(ctypes.c_uint8)".to_string());
                argtypes.push("ctypes.c_size_t".to_string());
                argtypes.push("ctypes.POINTER(ctypes.c_size_t)".to_string());
                let mut asked = args.clone();
                asked.push("out".to_string());
                asked.push("cap".to_string());
                asked.push("written".to_string());
                body_call = format!("got = _buffer(lambda out, cap, written: {})", call(&asked));
                after.push(match text_out {
                    true => "return got.decode(\"utf-8\")".to_string(),
                    false => "return got".to_string(),
                });
            }
            Some(Out::Records(at)) => {
                let ty = &records[*at].name;
                argtypes.push(format!("ctypes.POINTER({ty})"));
                argtypes.push("ctypes.c_size_t".to_string());
                argtypes.push("ctypes.POINTER(ctypes.c_size_t)".to_string());
                let mut asked = args.clone();
                asked.push("out".to_string());
                asked.push("cap".to_string());
                asked.push("written".to_string());
                body_call = format!(
                    "got = _structs({ty}, lambda out, cap, written: {})",
                    call(&asked)
                );
                after.push("return got".to_string());
            }
            Some(out) => {
                let value = |shape: &ByValue| match (shape.scalar, shape.rust) {
                    (true, _) => "chr(out.value)".to_string(),
                    (false, "bool") => "bool(out.value)".to_string(),
                    _ => "out.value".to_string(),
                };
                let (ctype, back) = match out {
                    Out::Value(shape) => (python_scalar(shape).to_string(), value(shape)),
                    // `null` is `None` (D30).
                    Out::AbsentValue(shape) => (
                        python_scalar(shape).to_string(),
                        format!("{} if present.value else None", value(shape)),
                    ),
                    Out::AbsentRecord(at) => (
                        records[*at].name.clone(),
                        "out if present.value else None".to_string(),
                    ),
                    Out::Choice(at) => (
                        "ctypes.c_int".to_string(),
                        format!("{}(out.value)", plains[*at].name),
                    ),
                    Out::Record(at) => (records[*at].name.clone(), "out".to_string()),
                    Out::Handle(at) => (
                        "ctypes.c_void_p".to_string(),
                        format!("{}._from(out.value)", handles[*at].name),
                    ),
                    Out::AbsentHandle(at) => (
                        "ctypes.c_void_p".to_string(),
                        format!(
                            "None if not out.value else {}._from(out.value)",
                            handles[*at].name
                        ),
                    ),
                    Out::Buffer | Out::Records(_) => unreachable!("handled above"),
                };
                argtypes.push(format!("ctypes.POINTER({ctype})"));
                before.push(format!("out = {ctype}()"));
                let mut asked = args.clone();
                asked.push("ctypes.byref(out)".to_string());
                if out.flagged() {
                    argtypes.push("ctypes.POINTER(ctypes.c_bool)".to_string());
                    before.push("present = ctypes.c_bool()".to_string());
                    asked.push("ctypes.byref(present)".to_string());
                }
                body_call = format!("_check({})", call(&asked));
                after.push(format!("return {back}"));
            }
        }
        self.signatures.push(format!(
            "_lib.{symbol}.argtypes = [{}]\n_lib.{symbol}.restype = ctypes.c_int",
            argtypes.join(", ")
        ));
        // **The `_async` form** (D9, D19, D27): `done` called with the value or
        // the exception on a library thread, or - under a running `asyncio`
        // loop and without `done` - an awaitable whose `cancel()` cancels the
        // call at its next pause point. Not for a result into a buffer, whose
        // size cannot be asked before the call has run, nor for a callback,
        // whose exception has nowhere to go.
        let asyncable =
            pauses && raising.is_empty() && !matches!(result, Some(Out::Buffer | Out::Records(_)));
        let async_lines: Vec<String> = match asyncable {
            false => Vec::new(),
            true => {
                self.any_async = true;
                let mut async_types = argtypes.clone();
                async_types.push("_DONE".to_string());
                async_types.push("ctypes.c_void_p".to_string());
                async_types.push("ctypes.POINTER(ctypes.c_void_p)".to_string());
                self.signatures.push(format!(
                    "_lib.{symbol}_async.argtypes = [{}]\n_lib.{symbol}_async.restype = ctypes.c_int",
                    async_types.join(", ")
                ));
                let mut lines = before.clone();
                let mut handed_args = args.clone();
                if result.is_some() {
                    handed_args.push("ctypes.byref(out)".to_string());
                }
                if result.as_ref().is_some_and(Out::flagged) {
                    handed_args.push("ctypes.byref(present)".to_string());
                }
                lines.push("def finish(status):".to_string());
                lines.push("    if status != 0:".to_string());
                lines.push("        return _ERRORS.get(status, Error)(_said())".to_string());
                match after.first() {
                    Some(back) => lines.push(format!("    {back}")),
                    None => lines.push("    return None".to_string()),
                }
                lines.push(format!(
                    "return _start(_lib.{symbol}_async, [{}], finish, done)",
                    handed_args.join(", ")
                ));
                lines
            }
        };
        let mut lines: Vec<String> = before;
        match raising.is_empty() {
            true => lines.push(body_call),
            false => {
                lines.push("try:".to_string());
                lines.push(format!("    {body_call}"));
                lines.push("finally:".to_string());
                for held in &raising {
                    lines.push(format!("    if {held}:"));
                    lines.push(format!("        raise {held}[0]"));
                }
            }
        }
        for kept in &keep {
            lines.push(format!("del {kept}"));
        }
        lines.extend(after);
        let indent = |lines: &[String], by: &str| -> String {
            lines
                .iter()
                .map(|line| format!("{by}{line}\n"))
                .collect::<String>()
        };
        // **A stream is a generator** (D20, D21, D27): a function taking one
        // callback that answers `bool` is also `<name>_iter(…)`, its other
        // parameters the same, and the items yielded one at a time.
        let streams: Vec<(usize, String)> = entry
            .args
            .iter()
            .enumerate()
            .filter_map(
                |(at, arg)| match taken(parsed, plains, handles, records, &arg.ty) {
                    Some(In::Callback {
                        result: Some(shape),
                        ..
                    }) if shape.rust == "bool" => Some((at, python_name(parsed.text(arg.name)))),
                    _ => None,
                },
            )
            .collect();
        if let ([(at, each)], None) = (streams.as_slice(), &entry.owner) {
            self.any_stream = true;
            let others: Vec<String> = def_params
                .iter()
                .enumerate()
                .filter(|(position, _)| position != at)
                .map(|(_, name)| name.clone())
                .collect();
            let forwarded: Vec<String> = def_params
                .iter()
                .enumerate()
                .map(|(position, name)| match position == *at {
                    true => "each".to_string(),
                    false => name.clone(),
                })
                .collect();
            let _ = each;
            self.functions.push(format!(
                "def {}_iter({}):\n    return _stream(lambda each: {}({}))\n",
                entry.name,
                others.join(", "),
                python_name(&entry.name),
                forwarded.join(", ")
            ));
        }
        if !async_lines.is_empty() {
            let mut params = def_params.clone();
            params.push("done=None".to_string());
            match (&entry.owner, entry.receiver) {
                (None, _) => self.functions.push(format!(
                    "def {}_async({}):\n{}",
                    entry.name,
                    params.join(", "),
                    indent(&async_lines, "    ")
                )),
                (Some(owner), Some(_)) => {
                    params.insert(0, "self".to_string());
                    self.classes.entry(owner.clone()).or_default().push(format!(
                        "    def {}_async({}):\n{}",
                        entry.name,
                        params.join(", "),
                        indent(&async_lines, "        ")
                    ));
                }
                (Some(_), None) => {}
            }
        }
        match (&entry.owner, entry.receiver, entry.getter) {
            (None, _, _) => {
                self.functions.push(format!(
                    "def {}({}):\n{}",
                    python_name(&entry.name),
                    def_params.join(", "),
                    indent(&lines, "    ")
                ));
            }
            (Some(owner), _, true) => {
                self.classes.entry(owner.clone()).or_default().push(format!(
                    "    @property\n    def {}(self):\n{}",
                    python_name(&entry.name),
                    indent(&lines, "        ")
                ));
            }
            (Some(owner), Some(_), false) => {
                let mut params = vec!["self".to_string()];
                params.extend(def_params);
                self.classes.entry(owner.clone()).or_default().push(format!(
                    "    def {}({}):\n{}",
                    python_name(&entry.name),
                    params.join(", "),
                    indent(&lines, "        ")
                ));
            }
            // The anonymous constructor is the class's own `__init__`; any
            // other function of the type is a static method.
            (Some(owner), None, false) if entry.name == "new" => {
                let mut params = vec!["self".to_string()];
                params.extend(def_params);
                let mut made = lines.clone();
                if let Some(last) = made.last_mut() {
                    *last = "self._ptr = out.value".to_string();
                }
                self.classes.entry(owner.clone()).or_default().push(format!(
                    "    def __init__({}):\n{}",
                    params.join(", "),
                    indent(&made, "        ")
                ));
            }
            (Some(owner), None, false) => {
                self.classes.entry(owner.clone()).or_default().push(format!(
                    "    @staticmethod\n    def {}({}):\n{}",
                    python_name(&entry.name),
                    def_params.join(", "),
                    indent(&lines, "        ")
                ));
            }
        }
    }

    /// The module.
    #[allow(clippy::too_many_arguments)]
    fn module(
        &self,
        package: &str,
        prefix: &str,
        codes: &[Raising],
        plains: &[Plain],
        handles: &[Handled],
        held: &[bool],
        records: &[Record],
        any_pause: bool,
    ) -> String {
        let library = package.replace('-', "_");
        let mut out = format!(
            "\"\"\"{package} - GENERATED by `nikaia bind python` from {package}'s ledger. Do not edit.\n\
             \n\
             ledger: {LEDGER_DIGEST}\n\
             \"\"\"\n\
             \n\
             import ctypes\n\
             import enum\n\
             import os\n\
             import sys\n\
             \n\
             _here = os.path.dirname(os.path.abspath(__file__))\n\
             if sys.platform == \"darwin\":\n    _file = \"lib{library}.dylib\"\n\
             elif sys.platform == \"win32\":\n    _file = \"{library}.dll\"\n\
             else:\n    _file = \"lib{library}.so\"\n\
             _lib = ctypes.CDLL(os.path.join(_here, \"..\", _file))\n\
             \n\
             \n\
             class Error(Exception):\n    \
             \"\"\"A status other than OK, with what `{prefix}_last_error` said.\"\"\"\n    \
             code = None\n\
             \n\
             \n\
             class BoundaryError(Error):\n    \
             \"\"\"One of the seven statuses the boundary owns.\"\"\"\n\
             \n\
             \n"
        );
        let boundary = [
            ("ArgumentError", -1),
            ("TooSmallError", -2),
            ("NotRunningError", -3),
            ("PanickedError", -4),
            ("ReentrantError", -5),
            ("CleanupError", -6),
            ("CancelledError", -7),
        ];
        let mut errors: Vec<(String, i64)> = Vec::new();
        for (name, code) in boundary {
            out.push_str(&format!(
                "class {name}(BoundaryError):\n    code = {code}\n\n\n"
            ));
            errors.push((name.to_string(), code));
        }
        let mut taken: std::collections::BTreeSet<String> =
            boundary.iter().map(|(name, _)| name.to_string()).collect();
        for raising in codes {
            let error = &raising.error;
            let spelled = error.replace("::", "");
            for code in &raising.codes {
                let (name, what) = match &code.variant {
                    Some(variant) => match taken.insert(variant.clone()) {
                        true => (variant.clone(), format!("{error}::{variant}")),
                        false => (format!("{spelled}{variant}"), format!("{error}::{variant}")),
                    },
                    None => {
                        let mut name = spelled.clone();
                        if let Some(first) = name.get_mut(..1) {
                            first.make_ascii_uppercase();
                        }
                        taken.insert(name.clone());
                        (name, error.clone())
                    }
                };
                let number = code.number;
                // **An `errno` is the matching `OSError` too** (D33): `ENOENT`
                // a `FileNotFoundError`, `EACCES` a `PermissionError`, each with
                // `errno` set.
                match code.errno {
                    Some(errno) => {
                        let os = match errno {
                            "ENOENT" => "FileNotFoundError",
                            "EACCES" => "PermissionError",
                            _ => "OSError",
                        };
                        out.push_str(&format!(
                            "class {name}(Error, {os}):\n    \
                             \"\"\"`{what}`, `{errno}`.\"\"\"\n    \
                             code = {number}\n\n    \
                             def __init__(self, said):\n        \
                             {os}.__init__(self, {number}, said)\n\n\n"
                        ));
                    }
                    None => out.push_str(&format!(
                        "class {name}(Error):\n    \"\"\"`{what}`.\"\"\"\n    code = {number}\n\n\n"
                    )),
                }
                errors.push((name, number));
            }
        }
        let table: Vec<String> = errors
            .iter()
            .map(|(name, code)| format!("{code}: {name}"))
            .collect();
        out.push_str(&format!(
            "_ERRORS = {{{}}}\n\n\
             _lib.{prefix}_last_error.argtypes = [ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)]\n\
             _lib.{prefix}_last_error.restype = ctypes.c_int\n\
             \n\
             \n\
             def _buffer(call):\n    \
             \"\"\"Asks the size, then hands a buffer of it (ADR-284 D6).\"\"\"\n    \
             written = ctypes.c_size_t(0)\n    \
             _check(call(None, 0, ctypes.byref(written)))\n    \
             room = (ctypes.c_uint8 * max(written.value, 1))()\n    \
             _check(call(room, written.value, ctypes.byref(written)))\n    \
             return bytes(room[: written.value])\n\
             \n\
             \n\
             def _structs(kind, call):\n    \
             \"\"\"Asks the count, then hands room for it, counted in structs (ADR-284 D16).\"\"\"\n    \
             written = ctypes.c_size_t(0)\n    \
             _check(call(None, 0, ctypes.byref(written)))\n    \
             room = (kind * max(written.value, 1))()\n    \
             _check(call(room, written.value, ctypes.byref(written)))\n    \
             return list(room[: written.value])\n\
             \n\
             \n\
                          def _said():\n    \
             written = ctypes.c_size_t(0)\n    \
             _lib.{prefix}_last_error(None, 0, ctypes.byref(written))\n    \
             room = (ctypes.c_uint8 * max(written.value, 1))()\n    \
             _lib.{prefix}_last_error(room, written.value, ctypes.byref(written))\n    \
             return bytes(room[: written.value]).decode(\"utf-8\", \"replace\")\n\
             \n\
             \n\
             def _check(status):\n    \
             if status != 0:\n        \
             raise _ERRORS.get(status, Error)(_said())\n\
             \n\
             \n\
             def _calling(raised, default, call, *args):\n    \
             try:\n        \
             return call(*args)\n    \
             except BaseException as error:\n        \
             raised.append(error)\n        \
             return default\n\
             \n\
             \n\
             def _bytes(data):\n    \
             data = bytes(data)\n    \
             return (ctypes.c_uint8 * len(data)).from_buffer_copy(data), len(data)\n\
             \n\
             \n\
             def _text(text):\n    \
             return _bytes(text.encode(\"utf-8\"))\n\
             \n\
             \n\
             def _run(kind, values):\n    \
             values = list(values)\n    \
             return (kind * len(values))(*values), len(values)\n\
             \n\
             \n\
             _lib.{prefix}_init.restype = ctypes.c_int\n\
             _lib.{prefix}_shutdown.restype = ctypes.c_int\n\
             \n\
             \n\
             def init():\n    \
             \"\"\"Starts the library, and starts it again after `shutdown` (ADR-284 D10).\"\"\"\n    \
             _check(_lib.{prefix}_init())\n\
             \n\
             \n\
             def shutdown():\n    \
             \"\"\"Stops the library; every call after it raises `NotRunningError` until `init`.\"\"\"\n    \
             _check(_lib.{prefix}_shutdown())\n\
             \n\
             \n",
            table.join(", ")
        ));
        if self.any_stream {
            out.push_str(
                "def _stream(call):\n    \
                 \"\"\"The items a callback is handed, yielded one at a time: the next is made\n    \
                 after this one was asked for, and closing the generator is the stop (ADR-284 D21).\"\"\"\n    \
                 import queue\n    \
                 import threading\n\
                 \n    \
                 items = queue.Queue(maxsize=1)\n    \
                 go = queue.Queue(maxsize=1)\n    \
                 end = object()\n    \
                 outcome = []\n\
                 \n    \
                 def each(*item):\n        \
                 items.put(item[0] if len(item) == 1 else item)\n        \
                 return go.get()\n\
                 \n    \
                 def run():\n        \
                 try:\n            \
                 outcome.append(call(each))\n        \
                 except BaseException as error:\n            \
                 outcome.append(error)\n        \
                 items.put(end)\n\
                 \n    \
                 threading.Thread(target=run, daemon=True).start()\n    \
                 ended = False\n    \
                 try:\n        \
                 while True:\n            \
                 item = items.get()\n            \
                 if item is end:\n                \
                 ended = True\n                \
                 break\n            \
                 yield item\n            \
                 go.put(True)\n    \
                 finally:\n        \
                 if not ended:\n            \
                 go.put(False)\n            \
                 while items.get() is not end:\n                \
                 go.put(False)\n    \
                 if outcome and isinstance(outcome[0], BaseException):\n        \
                 raise outcome[0]\n\
                 \n\
                 \n",
            );
        }
        if any_pause && self.any_async {
            out.push_str(&format!(
                "_DONE = ctypes.CFUNCTYPE(None, ctypes.c_int, ctypes.c_void_p)\n\
                 _lib.{prefix}_cancel.argtypes = [ctypes.c_void_p]\n\
                 _lib.{prefix}_cancel.restype = ctypes.c_int\n\
                 _lib.{prefix}_op_free.argtypes = [ctypes.c_void_p]\n\
                 _lib.{prefix}_op_free.restype = ctypes.c_int\n\
                 # Every call in flight, with what it must keep alive until `done`.\n\
                 _HELD = {{}}\n\
                 \n\
                 \n\
                 class Ticket:\n    \
                 \"\"\"An `_async` call: `cancel()` cancels it at its next pause point (ADR-284 D19).\"\"\"\n\
                 \n    \
                 def __init__(self):\n        \
                 self._op = ctypes.c_void_p()\n\
                 \n    \
                 def cancel(self):\n        \
                 if self._op:\n            \
                 _check(_lib.{prefix}_cancel(self._op))\n\
                 \n    \
                 def __del__(self):\n        \
                 # Only once `done` has run: until then `_HELD` holds this.\n        \
                 if self._op:\n            \
                 _lib.{prefix}_op_free(self._op)\n\
                 \n\
                 \n\
                 def _settle(future, value):\n    \
                 if future.done():\n        \
                 return\n    \
                 if isinstance(value, BaseException):\n        \
                 future.set_exception(value)\n    \
                 else:\n        \
                 future.set_result(value)\n\
                 \n\
                 \n\
                 def _start(function, args, finish, done):\n    \
                 loop = None\n    \
                 if done is None:\n        \
                 import asyncio\n\
                 \n        \
                 loop = asyncio.get_running_loop()\n        \
                 future = loop.create_future()\n    \
                 ticket = Ticket()\n    \
                 held = []\n\
                 \n    \
                 def landed(status, ctx):\n        \
                 value = finish(status)\n        \
                 _HELD.pop(id(held), None)\n        \
                 if loop is None:\n            \
                 done(value)\n        \
                 else:\n            \
                 try:\n                \
                 loop.call_soon_threadsafe(_settle, future, value)\n            \
                 except RuntimeError:\n                \
                 pass  # the loop has closed: nobody waits for it\n\
                 \n    \
                 callback = _DONE(landed)\n    \
                 held.extend([callback, args, ticket])\n    \
                 _HELD[id(held)] = held\n    \
                 _check(function(*args, callback, None, ctypes.byref(ticket._op)))\n    \
                 if loop is None:\n        \
                 return ticket\n    \
                 future.add_done_callback(lambda f: f.cancelled() and ticket.cancel())\n    \
                 return future\n\
                 \n\
                 \n"
            ));
        }
        for plain in plains {
            let values: Vec<String> = plain
                .variants
                .iter()
                .enumerate()
                .map(|(number, variant)| format!("    {variant} = {number}\n"))
                .collect();
            out.push_str(&format!(
                "class {}(enum.IntEnum):\n{}\n\n",
                plain.name,
                values.concat()
            ));
        }
        for record in records_in_order(records).iter().map(|at| &records[*at]) {
            let laid_out = |fields: &[(String, Part)]| -> String {
                fields
                    .iter()
                    .map(|(field, part)| {
                        let ty = match part {
                            Part::Value(shape) => python_scalar(shape).to_string(),
                            Part::Choice(_) => "ctypes.c_int".to_string(),
                            Part::Record(at) => records[*at].name.clone(),
                            Part::Array(element, count) => format!(
                                "{} * {count}",
                                match element.as_ref() {
                                    Part::Value(shape) => python_scalar(shape).to_string(),
                                    Part::Choice(_) => "ctypes.c_int".to_string(),
                                    Part::Record(at) => records[*at].name.clone(),
                                    Part::Array(..) => {
                                        unreachable!("an array's element is no array")
                                    }
                                }
                            ),
                        };
                        format!("(\"{field}\", {ty})")
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            // **An `extern` enum** (D32): a structure per variant with fields,
            // their union, and the tag beside it; the union is anonymous, so
            // `figure.frame.w` reads as C's does, and each variant's tag is a
            // class attribute.
            if record.is_enum() {
                let ty = &record.name;
                let mut members = Vec::new();
                for variant in record.variants.iter().filter(|v| !v.fields.is_empty()) {
                    out.push_str(&format!(
                        "class _{ty}_{}(ctypes.Structure):\n    _fields_ = [{}]\n\n\n",
                        variant.name,
                        laid_out(&variant.fields)
                    ));
                    members.push(format!("(\"{}\", _{ty}_{})", variant.member, variant.name));
                }
                let tags: String = record
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(tag, variant)| format!("    {} = {tag}\n", variant.name))
                    .collect();
                match members.is_empty() {
                    true => out.push_str(&format!(
                        "class {ty}(ctypes.Structure):\n    _fields_ = [(\"tag\", ctypes.c_int)]\n{tags}\n"
                    )),
                    false => out.push_str(&format!(
                        "class _{ty}_Union(ctypes.Union):\n    _fields_ = [{}]\n\n\n\
                         class {ty}(ctypes.Structure):\n    \
                         _anonymous_ = (\"_u\",)\n    \
                         _fields_ = [(\"tag\", ctypes.c_int), (\"_u\", _{ty}_Union)]\n{tags}\n",
                        members.join(", ")
                    )),
                }
            } else {
                out.push_str(&format!(
                    "class {}(ctypes.Structure):\n    _fields_ = [{}]\n\n",
                    record.name,
                    laid_out(&record.fields)
                ));
            }
            // Its methods (ADR-284 D16): `self` is handed by address.
            for member in self.classes.get(&record.name).into_iter().flatten() {
                out.push_str(member);
                out.push('\n');
            }
            out.push('\n');
        }
        for (handle, _) in handles.iter().zip(held).filter(|(_, held)| **held) {
            let ty = &handle.name;
            out.push_str(&format!(
                "_lib.{prefix}_{ty}_free.argtypes = [ctypes.c_void_p]\n\
                 _lib.{prefix}_{ty}_free.restype = ctypes.c_int\n\
                 \n\
                 \n\
                 class {ty}:\n    \
                 \"\"\"A handle (ADR-284 D5): `close()` frees it, as leaving a `with` does.\"\"\"\n\
                 \n    \
                 _ptr = None\n\
                 \n    \
                 @classmethod\n    \
                 def _from(cls, ptr):\n        \
                 made = cls.__new__(cls)\n        \
                 made._ptr = ptr\n        \
                 return made\n\
                 \n    \
                 def _live(self):\n        \
                 if not self._ptr:\n            \
                 raise ArgumentError(\"the handle is closed\")\n        \
                 return self._ptr\n\
                 \n    \
                 def close(self):\n        \
                 if self._ptr:\n            \
                 ptr, self._ptr = self._ptr, None\n            \
                 _check(_lib.{prefix}_{ty}_free(ptr))\n\
                 \n    \
                 def __enter__(self):\n        \
                 return self\n\
                 \n    \
                 def __exit__(self, *_):\n        \
                 self.close()\n\
                 \n    \
                 def __del__(self):\n        \
                 try:\n            \
                 self.close()\n        \
                 except Exception:\n            \
                 pass\n\
                 \n"
            ));
            for member in self.classes.get(ty).into_iter().flatten() {
                out.push_str(member);
                out.push('\n');
            }
            out.push('\n');
        }
        for signature in &self.signatures {
            out.push_str(signature);
            out.push('\n');
        }
        out.push_str("\n\n");
        for function in &self.functions {
            out.push_str(function);
            out.push_str("\n\n");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **An `errno` is the target's** (ADR-284 D33): Linux's on both Linux
    /// targets, WASI's under WebAssembly; the rest of `std`'s numbers are fixed.
    #[test]
    fn std_codes_are_the_targets_errno_or_fixed() {
        let numbers = |target: &str| -> Vec<i64> {
            std_codes("io::IoError", target)
                .expect("io::IoError has codes")
                .iter()
                .map(|code| code.number)
                .collect()
        };
        assert_eq!(numbers("x86_64-linux"), [2, 13, 84, 1000, 1001]);
        assert_eq!(numbers("aarch64-linux"), [2, 13, 84, 1000, 1001]);
        assert_eq!(numbers("wasm32-unknown"), [44, 2, 25, 1000, 1001]);
        assert_eq!(
            std_codes("task::Crashed", "x86_64-linux").expect("fixed")[0].number,
            1100
        );
        assert!(std_codes("ConfigError", "x86_64-linux").is_none());
    }
}
