//! `a + b` where one side is text
//! ([ADR-081](../../../docs/specification/adr/adr-081.md)).
//!
//! **Rust's own rule reached the user unchanged**, and one of the four shapes a
//! program can write compiled:
//!
//! ```text
//! "a" + "b"  →  error[E0369]: cannot add `&str` to `&str`
//! "a" + s    →  error[E0369]: cannot add `String` to `&str`
//! s + "b"    →  compiles
//! s + s2     →  error[E0308]: mismatched types
//! ```
//!
//! The checker got it wrong first, which is the worse half: `"User: " +
//! self.username` came to a `&str`, so a function declaring `-> String` was
//! refused as `NK1104` — a **false** refusal, and
//! [Part III C.4](../../../docs/specification/30-nikaia-tooling.md) says this
//! compiler never refuses a correct program.
//!
//! Run rather than read wherever the question is about behaviour: whether a
//! shape works is the language below's answer, and comparing emitted text would
//! only say this compiler agrees with itself. The four shapes run in
//! `tests/language/src/concatenation.nika`.

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings
}

fn lowered(purpose: &str, source: &str) -> String {
    let found = findings(source);
    assert!(
        found.is_empty(),
        "{purpose} is a correct program and the checker says otherwise: {found:#?}"
    );
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// D2: a `+` over text becomes a call, and the checker's span is what says
/// which one — so a chain nests rather than collapsing into one key.
#[test]
fn a_chain_of_three_is_two_calls() {
    let rust = lowered(
        "a chain",
        r#"
fn main() {
    println("p" + "q" + "r")
}
"#,
    );
    assert!(
        rust.contains("nikaia_std::concat::plus(nikaia_std::concat::plus(\"p\", \"q\"), \"r\")"),
        "two operators, two calls, nested the way they were written:\n{rust}"
    );
}

/// **A number's `+` does not move**, and that is the decision this rests on.
///
/// Arithmetic inside `nikaia_std` would silently lose
/// [ADR-285](../../../docs/specification/adr/adr-285.md) D5's overflow abort:
/// `overflow-checks` is on per Nikaia crate and off for the profile, and
/// inlining does not carry the check across — measured, with `a + b` in a
/// checked crate aborting and the same `a + b` through an `#[inline]` helper in
/// an unchecked one wrapping to `-9223372036854775808`.
#[test]
fn a_number_keeps_its_operator() {
    let rust = lowered(
        "arithmetic",
        r#"
fn main() {
    let a: i64 = 2
    let b: i64 = 3
    println(f"{a + b}")
}
"#,
    );
    assert!(
        rust.contains("a + b"),
        "the operator stays where the overflow check is:\n{rust}"
    );
    assert!(
        !rust.contains("concat::plus"),
        "and nothing routes it through `std`:\n{rust}"
    );
}

/// The checker's half: a concatenation is a `String`, so a function that
/// declares one is not refused.
///
/// This is the `NK1104` that Part I 4.7's own body used to get, and the reason
/// it mattered more than the lowering: a **false** refusal is the direction
/// Part III C.4 forbids outright.
#[test]
fn a_concatenation_is_a_string_and_not_a_view() {
    let found = findings(
        r#"
fn greet(name: String) -> String {
    return "User: " + name
}

fn main() {
    println(greet("Ada".to_string()))
}
"#,
    );
    assert!(
        found.is_empty(),
        "a concatenation hands back a `String`: {found:#?}"
    );
}

/// And the refusal that must survive: a `+` over two things that are **not**
/// text still says nothing, so nothing is silently turned into a string.
#[test]
fn a_plus_over_two_unknowns_is_not_a_concatenation() {
    let rust = lowered(
        "two unknowns",
        r#"
fn main() {
    let mut xs = Vec()
    xs.push(1)
    let n = xs.len() + xs.len()
    println(f"{n}")
}
"#,
    );
    assert!(
        !rust.contains("concat::plus"),
        "nothing said either side was text:\n{rust}"
    );
}

/// The owned left side keeps its buffer, which is what makes this free.
///
/// Measured in `nikaia_std::concat`'s own tests by pointer identity; here the
/// question is only that the lowering reaches that impl rather than a
/// `format!`.
#[test]
fn the_lowering_is_the_trait_and_not_a_format() {
    let rust = lowered(
        "no format",
        r#"
fn main() {
    let s = "left-".to_string()
    println(s + "right")
}
"#,
    );
    assert!(
        rust.contains("nikaia_std::concat::plus(s, \"right\")"),
        "{rust}"
    );
    assert!(!rust.contains("format!"), "{rust}");
}
