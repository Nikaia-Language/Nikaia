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
// **What is built is the first shape of D5's table**: numbers, `bool` and
// `scalar`, in and out, of a function that neither pauses nor throws, declared
// in the package's entry file. Every other entry point is refused here, saying
// which part is not built yet, rather than exported half-way.

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

/// The wrappers and the header for every entry point the program's own files
/// declare.
pub fn export(program: &Program, prefix: &str, package: &str) -> Result<Option<Exported>> {
    let ledger_digest = LEDGER_DIGEST;
    let upper = prefix.to_uppercase();
    let mut rust = String::from(
        "\n// --- The C boundary (ADR-284): one wrapper per entry point. ---\n\
         \n\
         /// Set by a panic at the boundary; every call after it is `E_PANICKED` (ADR-284 D8).\n\
         static __NIKAIA_POISONED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);\n",
    );
    let mut declarations = String::new();
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
                     what is built is numbers, `bool` and `scalar` in and out, of a function in \
                     `src/main.nika` that neither pauses nor throws (ADR-284 D5)."
                )
            };
            if at != 0 {
                return Err(not_yet("file"));
            }
            let contract = program.contracts.functions.get(&written);
            if contract.is_some_and(|c| !c.sync_claim.is_sync()) {
                return Err(not_yet("pausing"));
            }
            if contract.is_some_and(|c| !c.fails_with.is_empty()) {
                return Err(not_yet("failure"));
            }
            let symbol = format!("{prefix}_{written}");
            let mut rust_params = Vec::new();
            let mut c_params = Vec::new();
            let mut checks = String::new();
            let mut call_args = Vec::new();
            for arg in args {
                let param = parsed.text(arg.name).to_string();
                let Some(shape) = by_value(parsed, &arg.ty) else {
                    return Err(not_yet(&format!("parameter `{param}`")));
                };
                let local = crate::emit::escaped(&param).into_owned();
                rust_params.push(format!("{local}: {}", shape.rust));
                c_params.push(format!("{} {param}", shape.c));
                if shape.scalar {
                    checks.push_str(&format!(
                        "    let Some({local}) = char::from_u32({local}) else {{ return -1; }};\n"
                    ));
                }
                call_args.push(local);
            }
            let result = match ret_type {
                Some(ty) => {
                    let Some(shape) = by_value(parsed, ty) else {
                        return Err(not_yet("result"));
                    };
                    rust_params.push(format!("out: *mut {}", shape.rust));
                    c_params.push(format!("{} *out", shape.c));
                    Some(shape)
                }
                None => None,
            };
            let call = format!(
                "{}({})",
                crate::emit::escaped(&written),
                call_args.join(", ")
            );
            let handed_back = match &result {
                Some(shape) => {
                    let value = match shape.scalar {
                        true => "value as u32",
                        false => "value",
                    };
                    format!(
                        "Ok(value) => {{\n            if !out.is_null() {{\n                \
                         // SAFETY: the caller hands a place for one value, or NULL (ADR-284 D7).\n                \
                         unsafe {{ *out = {value} }};\n            }}\n            0\n        }}"
                    )
                }
                None => "Ok(()) => 0".to_string(),
            };
            rust.push_str(&format!(
                "\n#[unsafe(no_mangle)]\npub extern \"C\" fn {symbol}({}) -> std::ffi::c_int {{\n    \
                 if __NIKAIA_POISONED.load(std::sync::atomic::Ordering::SeqCst) {{\n        return -4;\n    }}\n\
                 {checks}    \
                 match std::panic::catch_unwind(move || {call}) {{\n        {handed_back}\n        \
                 Err(_) => {{\n            __NIKAIA_POISONED.store(true, std::sync::atomic::Ordering::SeqCst);\n            \
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
