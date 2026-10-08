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
//   call is `E_PANICKED` (D8);
// * its line in `<package>.h`, which also carries the seven status codes the
//   boundary owns and the hash of the ledger the header was derived from (D12).
//
// **What is built of D5's table**: numbers, `bool` and `scalar` in and out;
// text (`ref String`) and bytes (`Bytes`) in as an address and a length, and
// out (`String`, `Bytes`, `Vec[u8]`) into the caller's buffer (D6); a run of
// numbers (`ref Array[T]`) in; an `enum` without payload in and out, as a C
// `enum` - of a function that does not pause, declared in the package's entry
// file, throwing at most one `enum` of it. Every raw pointer is read and written
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
}

/// What a result hands back (ADR-284 D5, D6).
enum Out {
    Value(ByValue),
    /// `String`, `Bytes` or `Vec[u8]`: into the caller's buffer.
    Buffer,
    /// An `enum` without payload: its number.
    Choice(usize),
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

fn taken(parsed: &crate::parser::Parsed, plains: &[Plain], ty: &Type) -> Option<In> {
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

fn handed(parsed: &crate::parser::Parsed, plains: &[Plain], ty: &Type) -> Option<Out> {
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

/// Every error `enum` the entry file declares and an entry point throws, with
/// the code of each variant: `1…` in declaration order across the library.
fn variant_codes(program: &Program) -> Vec<(String, Vec<(String, i64)>)> {
    let Some(unit) = program.units.first() else {
        return Vec::new();
    };
    let parsed = &unit.parsed;
    let thrown: std::collections::BTreeSet<String> = parsed
        .program
        .items
        .iter()
        .filter_map(|item| match &item.node {
            Item::Fn {
                name: Some(name),
                is_extern: true,
                ..
            } => program.contracts.functions.get(parsed.text(*name)),
            _ => None,
        })
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
pub fn export(program: &Program, prefix: &str, package: &str) -> Result<Option<Exported>> {
    let ledger_digest = LEDGER_DIGEST;
    let upper = prefix.to_uppercase();
    let mut rust = String::from(
        "\n// --- The C boundary (ADR-284): one wrapper per entry point. ---\n\
         \n\
         /// Set by a panic at the boundary; every call after it is `E_PANICKED` (ADR-284 D8).\n\
         static __NIKAIA_POISONED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);\n\
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
         }})\n}}\n"
    ));
    let mut declarations = String::new();
    // **A `throws` function's variants are `1…`, per library in declaration
    // order** (D7): every variant of every error `enum` the entry file
    // declares and an entry point throws.
    let codes = variant_codes(program);
    let plains = program
        .units
        .first()
        .map(|unit| plain_enums(&unit.parsed))
        .unwrap_or_default();
    // The ones an entry point's signature names, which the header declares.
    let mut crossing = vec![false; plains.len()];
    let mut constants = String::new();
    for (error, variants) in &codes {
        for (variant, code) in variants {
            constants.push_str(&format!(
                "#define {upper}_E_{} {code} /* {error}::{variant} */\n",
                variant.to_uppercase()
            ));
        }
    }
    for (at, unit) in program.units.iter().enumerate() {
        if unit.package.is_some() {
            continue;
        }
        for item in &unit.parsed.program.items {
            let Item::Fn {
                name: Some(name),
                args,
                ret_type,
                is_extern: true,
                ..
            } = &item.node
            else {
                continue;
            };
            let parsed = &unit.parsed;
            let written = parsed.text(*name).to_string();
            let not_yet = |what: &str| -> anyhow::Error {
                anyhow::anyhow!(
                    "`{written}` is an entry point whose {what} this compiler does not export yet: \
                     what is built is numbers, `bool`, `scalar` and an `enum` without payload in \
                     and out, text and bytes in and out, and a run of numbers in, of a function \
                     in `src/main.nika` that does not pause (ADR-284 D5)."
                )
            };
            if at != 0 {
                return Err(not_yet("file"));
            }
            let contract = program.contracts.functions.get(&written);
            if contract.is_some_and(|c| !c.sync_claim.is_sync()) {
                return Err(not_yet("pausing"));
            }
            // **What it throws is one `enum` of the entry file** (D7), whose
            // variants are the codes; anything else is not exported yet.
            let thrown: Option<&Vec<(String, i64)>> =
                match contract.map(|c| c.fails_with.as_slice()) {
                    None | Some([]) => None,
                    Some([one]) => match codes.iter().find(|(error, _)| error == one) {
                        Some((_, variants)) => Some(variants),
                        None => return Err(not_yet("failure")),
                    },
                    Some(_) => return Err(not_yet("failure")),
                };
            let symbol = format!("{prefix}_{written}");
            let mut rust_params = Vec::new();
            let mut c_params = Vec::new();
            let mut checks = String::new();
            let mut call_args = Vec::new();
            for (position, arg) in args.iter().enumerate() {
                let param = parsed.text(arg.name).to_string();
                let local = crate::emit::escaped(&param).into_owned();
                match taken(parsed, &plains, &arg.ty) {
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
                            .map(|(number, variant)| {
                                format!("{number} => {}::{variant},", plain.name)
                            })
                            .collect();
                        checks.push_str(&format!(
                            "    let {local} = match {local} {{ {} _ => return -1 }};\n",
                            arms.join(" ")
                        ));
                        call_args.push(local);
                    }
                    None => return Err(not_yet(&format!("parameter `{param}`"))),
                }
            }
            let result = match ret_type {
                Some(ty) => match handed(parsed, &plains, ty) {
                    Some(Out::Value(shape)) => {
                        rust_params.push(format!("out: *mut {}", shape.rust));
                        c_params.push(format!("{} *out", shape.c));
                        Some(Out::Value(shape))
                    }
                    Some(Out::Buffer) => {
                        rust_params
                            .push("out: *mut u8, cap: usize, written: *mut usize".to_string());
                        c_params.push("uint8_t *out, size_t cap, size_t *written".to_string());
                        Some(Out::Buffer)
                    }
                    Some(Out::Choice(at)) => {
                        crossing[at] = true;
                        rust_params.push("out: *mut std::ffi::c_int".to_string());
                        c_params.push(format!("{prefix}_{} *out", plains[at].name));
                        Some(Out::Choice(at))
                    }
                    None => return Err(not_yet("result")),
                },
                None => None,
            };
            let call = format!(
                "{}({})",
                crate::emit::escaped(&written),
                call_args.join(", ")
            );
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
                None => "Ok(()) => 0".to_string(),
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
            rust.push_str(&format!(
                "\n#[unsafe(no_mangle)]\npub extern \"C\" fn {symbol}({}) -> std::ffi::c_int {{\n    \
                 if __NIKAIA_POISONED.load(std::sync::atomic::Ordering::SeqCst) {{\n        return -4;\n    }}\n\
                 {checks}    \
                 match std::panic::catch_unwind(move || {call}) {{\n        {handed_back}{failed}\n        \
                 Err(_) => {{\n            __NIKAIA_POISONED.store(true, std::sync::atomic::Ordering::SeqCst);\n            \
                 __nikaia_failed(\"the library panicked; it answers E_PANICKED until it is started again\".to_string());\n            \
                 -4\n        }}\n    }}\n}}\n",
                rust_params.join(", "),
            ));
            let c_params = match c_params.is_empty() {
                true => "void".to_string(),
                false => c_params.join(", "),
            };
            declarations.push_str(&format!("int {symbol}({c_params});\n"));
        }
    }
    // **An `enum` without payload is a C `enum`**, numbered in declaration
    // order (D5), each value `<PREFIX>_<TYPE>_<VARIANT>` (D13).
    let mut types = String::new();
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
