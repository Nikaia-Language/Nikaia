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

use nikaia_std::tools::cexport::{CX_LEDGER_DIGEST, cx_export};
use nikaia_std::tools::cexport_model::{CxEntry, cx_entries_in};
use nikaia_std::tools::cexport_node::cx_node_gyp;

use crate::modules::Program;

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
    cx_node_gyp(package)
}

/// Where the header names the ledger's digest, filled in once the lowering
/// has written the ledger.
pub const LEDGER_DIGEST: &str = CX_LEDGER_DIGEST;

/// The refusal for a library build whose entry package exports nothing.
pub fn nothing_exported() -> anyhow::Error {
    anyhow::anyhow!(nikaia_std::tools::cexport_model::cx_nothing_exported())
}

/// The wrappers and the header for every entry point the program's own files
/// declare.
///
/// `concurrent` is `user_parallelism = yes`: the runtime the library starts
/// has a pool for user code. Everything it writes is `tools/cexport*.nika`'s
/// (ADR-294, #125); this reads the program's files and hands them over.
pub fn export(
    program: &Program,
    prefix: &str,
    package: &str,
    concurrent: bool,
    target: &str,
) -> Result<Option<Exported>> {
    let mut found: Vec<CxEntry> = Vec::new();
    for (at, unit) in program.units.iter().enumerate() {
        if unit.package.is_some() {
            continue;
        }
        let parsed = &unit.parsed;
        cx_entries_in(&parsed.interner, &parsed.program.items, at == 0, &mut found)
            .map_err(|refusal| anyhow::anyhow!("{refusal}"))?;
    }
    let Some(unit) = program.units.first() else {
        return Ok(None);
    };
    let parsed = &unit.parsed;
    let made = cx_export(
        &parsed.interner,
        &parsed.program.items,
        &found,
        &program.contracts,
        prefix,
        package,
        concurrent,
        target,
    )
    .map_err(|refusal| anyhow::anyhow!("{refusal}"))?;
    Ok(made.map(|made| Exported {
        rust: made.rust,
        header: made.header,
        python: made.python,
        node: made.node,
    }))
}

#[cfg(test)]
mod tests {
    use nikaia_std::tools::cexport_model::cx_std_codes;

    /// **An `errno` is the target's** (ADR-284 D33): Linux's on both Linux
    /// targets, WASI's under WebAssembly; the rest of `std`'s numbers are fixed.
    #[test]
    fn std_codes_are_the_targets_errno_or_fixed() {
        let numbers = |target: &str| -> Vec<i64> {
            cx_std_codes("io::IoError", target)
                .iter()
                .map(|code| code.number)
                .collect()
        };
        assert_eq!(numbers("x86_64-linux"), [2, 13, 84, 1000, 1001]);
        assert_eq!(numbers("aarch64-linux"), [2, 13, 84, 1000, 1001]);
        assert_eq!(numbers("wasm32-unknown"), [44, 2, 25, 1000, 1001]);
        assert_eq!(
            cx_std_codes("task::Crashed", "x86_64-linux")[0].number,
            1100
        );
        assert!(cx_std_codes("ConfigError", "x86_64-linux").is_empty());
    }
}
