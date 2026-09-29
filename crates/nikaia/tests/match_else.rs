//! A `match`'s catch-all arm is `else`
//! ([ADR-145](../../../docs/specification/adr/adr-145.md)).
//!
//! The last arm is *everything else*, and it was written with a character
//! borrowed from the ignore pattern — which says something different:
//! [ADR-126](../../../docs/specification/adr/adr-126.md) is careful that `_`
//! means **ignore a value that arrived**, neither bind it nor use it, and that
//! record even argues against calling it a *wildcard*. Nothing arrives at a
//! catch-all arm.
//!
//! `else` is what the same idea is called one construct over, is already
//! reserved, and already reads as *the branch taken when nothing before it
//! matched* ([ADR-132](../../../docs/specification/adr/adr-132.md)) — so this
//! costs no word, which is most of the argument
//! ([ADR-084](../../../docs/specification/adr/adr-084.md)).

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> Result<String, String> {
    let parsed = parse_to_ast(source).map_err(|e| format!("{e:#}"))?;
    emit_program(&parsed, Build::default())
        .map(|it| it.rust)
        .map_err(|e| format!("{e:#}"))
}

/// **`else` is the arm, and it lowers to Rust's own `_`** (D1, D3).
#[test]
fn the_catch_all_arm_is_else_and_lowers_to_underscore() {
    let rust = lowered(
        "fn describe(n: i64) -> ref String {\n\
         \x20   return match n { 1 => \"one\", else => \"other\" }\n\
         }\n",
    )
    .expect("it lowers");
    assert!(rust.contains("_ => \"other\""), "{rust}");
    assert!(!rust.contains("else => "), "{rust}");
}

/// **`_` in that position is refused, and the message names the replacement**
/// (D1). From the parser, because that is where the two spellings meet.
#[test]
fn an_underscore_arm_is_refused() {
    let source = "fn f(n: i64) -> i64 { return match n { 1 => 1, _ => 2 } }";
    let said = lowered(source).expect_err("`_` is not the arm any more");
    assert!(said.contains("is written `else`"), "{said}");
    // The help says what `_` is for instead: ignoring a part of a value.
    let error = parse_to_ast(source).expect_err("refused by the parser");
    let finding = nikaia::diagnostics::refused_finding(&error).expect("a parse refusal");
    assert!(
        finding
            .help
            .as_deref()
            .is_some_and(|h| h.contains("`_` ignores a part of a value")),
        "{finding:#?}"
    );
}

/// **`_` keeps its other positions** (D2), which is the half that matters: what
/// goes is the arm, not the pattern.
#[test]
fn the_ignore_pattern_keeps_its_other_positions() {
    let rust = lowered(
        "fn pair() -> (i64, i64) { return (1, 2) }\n\
         fn handle(n: i64, _: i64) -> i64 { return n }\n\
         fn main() {\n\
         \x20   let (a, _) = pair()\n\
         \x20   println(f\"{handle(a, 3)}\")\n\
         }\n",
    )
    .expect("it lowers");
    assert!(rust.contains("let (a, _)"), "{rust}");
    assert!(
        rust.contains("_: i64") || rust.contains("_: &i64"),
        "{rust}"
    );
}

/// **A bare name still binds**, which `else` does not touch: `other => …` is
/// the arm that catches *and names*.
#[test]
fn a_bare_name_still_binds() {
    let rust = lowered(
        "fn describe(n: i64) -> i64 {\n\
         \x20   return match n { 1 => 0, other => other }\n\
         }\n",
    )
    .expect("it lowers");
    assert!(rust.contains("other => other"), "{rust}");
}

/// **`else` after a real pattern, and in a `match` that is a statement**, so the
/// arm is not only exercised where a value is taken.
#[test]
fn the_arm_works_in_statement_position_too() {
    let rust = lowered(
        "fn f(c: char) {\n\
         \x20   match c { 'n' => { println(\"newline\") } else => { } }\n\
         }\n",
    )
    .expect("it lowers");
    assert!(
        rust.contains("_ => {}") || rust.contains("_ => { }"),
        "{rust}"
    );
}

/// **And `_name` is still a name**, which is `UNDERSCORE`'s own condition and is
/// why the refusal above could be put in the parser at all
/// ([ADR-126](../../../docs/specification/adr/adr-126.md) §5).
#[test]
fn an_underscore_prefixed_name_is_a_binding_arm() {
    let rust = lowered(
        "fn describe(n: i64) -> i64 {\n\
         \x20   return match n { 1 => 0, _rest => _rest }\n\
         }\n",
    )
    .expect("it lowers");
    assert!(rust.contains("_rest => _rest"), "{rust}");
}
