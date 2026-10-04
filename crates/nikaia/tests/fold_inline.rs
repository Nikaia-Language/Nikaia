//! **What a fold's step calls directly is written `#[inline]`**
//! ([ADR-296](../../../docs/specification/adr/adr-296.md) D41, #433): the step
//! runs once per item, and LLVM left 1BRC's `Summary::record` out of line -
//! 319.1 instructions a row against 300.1 with the hint.

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// 1BRC's step calls `record`, and `record` alone carries the hint: `new`,
/// `add` and `merged` are not called by the step.
#[test]
fn the_method_a_fold_step_calls_is_inline_and_nothing_else_is() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/1brc.nika"
    ))
    .expect("read examples/1brc.nika");
    let rust = lowered(&source);
    let hinted: Vec<&str> = rust
        .lines()
        .zip(rust.lines().skip(1))
        .filter(|(line, _)| line.trim() == "#[inline]")
        .map(|(_, next)| next.trim())
        .collect();
    assert_eq!(hinted.len(), 1, "{hinted:#?}\n{rust}");
    assert!(hinted[0].starts_with("fn record("), "{hinted:#?}");
}

/// A program without a fold gains no hint.
#[test]
fn a_program_without_a_fold_has_no_hint() {
    let rust = lowered(
        "fn twice(x: i64) -> i64 {\n    return x * 2\n}\n\nfn main() {\n    println(f\"{twice(2)}\")\n}\n",
    );
    assert!(!rust.contains("#[inline]"), "{rust}");
}
