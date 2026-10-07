//! **A float literal is an `f64`** (#503): `f64` is the only float type
//! (Part I 2.2), so a `let` without an annotation whose value is `0.0` is one,
//! and so is everything computed from it. It used to be unknown, as an integer
//! literal is until a use decides it, and a wrong use of it reached `rustc`.

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

/// `let mut px = 0.0`, added to, checks; and it is an `f64` where a `let`
/// says something else.
#[test]
fn a_float_let_is_an_f64() {
    let fine = r#"
fn main() {
    let mut px = 0.0
    let mass = 2.0
    px = px + 1.5 * mass
    println(f"{px}")
}
"#;
    assert!(findings(fine).is_empty(), "{:#?}", findings(fine));
    let wrong = r#"
fn main() {
    let px = 0.0
    let n: i64 = px
    println(f"{n}")
}
"#;
    let found = findings(wrong);
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK1103" && f.message.contains("`f64`")),
        "{found:#?}"
    );
}

/// `px + n` with an `i64` is the mixed-type refusal.
#[test]
fn a_float_let_and_an_integer_do_not_mix() {
    let source = r#"
fn main() {
    let px = 0.0
    let n: i64 = 3
    let sum = px + n
    println(f"{sum}")
}
"#;
    let found = findings(source);
    assert!(found.iter().any(|f| f.code == "NK1199"), "{found:#?}");
}

/// A list of float literals is a `Vec[f64]`, and its elements compare.
#[test]
fn a_list_of_floats_holds_f64s() {
    let fine = r#"
fn main() {
    let readings = [1.0, 2.5, 0.5]
    let mut peak = 0
    for day in 0..<readings.len() {
        if readings[day] > readings[peak] {
            peak = day
        }
    }
    println(f"{peak}")
}
"#;
    assert!(findings(fine).is_empty(), "{:#?}", findings(fine));
    let wrong = r#"
fn main() {
    let readings = [1.0, 2.5, 0.5]
    let first: bool = readings[0]
    println(f"{first}")
}
"#;
    let found = findings(wrong);
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK1103" && f.message.contains("`f64`")),
        "{found:#?}"
    );
}
