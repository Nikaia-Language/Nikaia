//! **`panic(…)` never comes back** (#506), so it has the type a `return` has:
//! *never*, which fits every slot. `m[k] ?? panic(…)` is what the map holds,
//! and an arm that panics is not one of the answers that have to agree
//! ([ADR-276](../../../docs/specification/adr/adr-276.md) D1, D20).

mod common;

use nikaia::check::Finding;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = common::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    common::checked(&parsed, &own, &library).findings
}

/// Whether the `let b: bool` line is refused as a type it is not.
fn the_bool_let_is_refused(source: &str) -> bool {
    let line = source
        .lines()
        .position(|l| l.contains("let b: bool"))
        .expect("the source has the `let`");
    findings(source)
        .iter()
        .any(|f| f.code == "NK1103" && source[..f.span.at()].matches('\n').count() == line)
}

/// An `if` whose other arm panics is the first arm's type: the `let` below
/// knows it, and says so where it is wrong.
#[test]
fn an_if_arm_that_panics_gives_no_value() {
    let source = r#"
fn pick(c: bool) -> bool {
    let t = if c { "x" } else { panic("no") }
    let b: bool = t
    return b
}
"#;
    assert!(the_bool_let_is_refused(source), "{:#?}", findings(source));
}

/// A `match` arm that panics, the same.
#[test]
fn a_match_arm_that_panics_gives_no_value() {
    let source = r#"
fn pick(n: i64) -> bool {
    let t = match n {
        0 => "zero"
        else => panic("not zero")
    }
    let b: bool = t
    return b
}
"#;
    assert!(the_bool_let_is_refused(source), "{:#?}", findings(source));
}

/// `?? panic(…)` after a map read is what the map holds.
#[test]
fn a_map_read_or_a_panic_is_what_the_map_holds() {
    let source = r#"
use std::collections
fn pick(m: ref collections::HashMap[String, i64], k: ref String) -> bool {
    let b: bool = m[k] ?? panic(f"{k} was a key a moment ago")
    return b
}
"#;
    assert!(the_bool_let_is_refused(source), "{:#?}", findings(source));
}

/// **A function of the program's own called `panic` is not the prelude's**:
/// it comes back, so its arm is one of the answers.
#[test]
fn a_function_called_panic_comes_back() {
    let source = r#"
fn panic(why: ref String) -> bool { return true }
fn pick(c: bool) -> bool {
    let t = if c { "x" } else { panic("no") }
    return true
}
"#;
    assert!(
        !findings(source).iter().any(|f| f.code == "NK1103"),
        "{:#?}",
        findings(source)
    );
}
