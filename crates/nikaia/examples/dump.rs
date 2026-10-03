//! Lower a .nika file to Rust and print it.
//!
//! The library-only path to the emitter: the `nikaia` binary links the rustc
//! backend, which this does not need. It is also what regenerates the checked-in
//! output in `tests/fixtures/`:
//!
//! ```text
//! cargo run -p nikaia --example dump -- crates/nikaia/tests/fixtures/measurements.nika \
//!     > crates/nikaia/tests/fixtures/measurements_expected.rs
//! ```

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: dump <file.nika> [target] [user-parallelism]"))?;
    // `no`, not `0`: `user-parallelism` is a yes-or-no question and a count is
    // refused (ADR-037 D2). The default here was still a count, so the command a
    // failing fixture test tells you to run did not run.
    // The third switch takes its default here rather than a positional
    // argument: it changes nothing a dump shows (ADR-281 D36 — the switch
    // decides whether a violation is *noticed*, and this prints the lowering).
    let build = Build::parse(
        &args.next().unwrap_or_else(|| "x86_64-linux".to_string()),
        &args.next().unwrap_or_else(|| "no".to_string()),
        "yes",
    )?;

    let source = std::fs::read_to_string(&path)?;
    let parsed = parse_to_ast(&source)?;
    print!("{}", emit_program(&parsed, build)?.rust);
    Ok(())
}
