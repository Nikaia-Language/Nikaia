//! Three shapes the ledger's reader met when it moved into Nikaia
//! (0.0.292, ADR-294 step (c)), each a compiler defect a program could meet
//! the same way.
//!
//! What the programs compute - whether the lowering compiles is the language
//! below's question - is `tests/language/src/reader_shapes.nika`; this file
//! keeps what the lowering writes.

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(purpose: &str, source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let found =
        check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings;
    assert!(
        found.is_empty(),
        "{purpose} is a correct program and the checker says otherwise: {found:#?}"
    );
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// **A text's characters are listed at their count**: `nikaia_std::list::chars`
/// asks for the room once, where `collect` over `Chars` grew the list step by
/// step - the largest single cost of reading a ledger in Nikaia.
#[test]
fn the_characters_of_a_text_are_listed_at_their_size() {
    let source = r#"fn main() {
    let word = "héllo"
    let c: Vec[scalar] = word.chars().collect()
    println(f"{c.len()} {c[1]}")
}
"#;
    let rust = lowered("chars", source);
    assert!(rust.contains("nikaia_std::list::chars("), "{rust}");
}

/// **A part an arm binds `ref` is handed on as it is** (issue #270):
/// ADR-291 binds `key` as a view where the arm only reads it, so a call that
/// lends it writes `key` and not `&key`, a view of a view.
#[test]
fn a_part_bound_as_a_view_is_lent_as_it_is() {
    let source = r#"enum Line {
    Pair { key: String, value: String },
    Skip,
}

fn shout(text: ref String) -> String {
    return text.to_uppercase()
}

fn main() {
    let mut lines = [Line::Pair { key: "a", value: "b" }, Line::Skip]
    for one in lines.drain() {
        match one {
            Line::Pair { key, value } => {
                let said = shout(key)
                println(f"{said}={shout(value)}")
            }
            Line::Skip => {}
        }
    }
}
"#;
    let rust = lowered("ref-part", source);
    assert!(rust.contains("ref key"), "{rust}");
    assert!(!rust.contains("shout(&key)"), "{rust}");
    assert!(!rust.contains("shout(&value)"), "{rust}");
}

/// **An element of a view of a run is its item** (ADR-179 D1): `xs[0]` for an
/// `xs: ref Array[Row]` is a `Row`, so a method on it is one the ledger
/// describes. It was `?`, and a method nothing describes may pause: the
/// function around it became `async` (found moving `Ty::fits` into Nikaia).
#[test]
fn an_element_of_a_view_of_a_run_is_its_item() {
    let source = r#"struct Row {
    value: i64,
}

impl Row {
    fn doubled(ref self) -> i64 {
        return self.value * 2
    }
}

fn first(rows: ref Array[Row]) -> i64 {
    return rows[0].doubled() + rows[1..<2][0].doubled()
}

fn main() {
    let rows = [Row { value: 1 }, Row { value: 5 }]
    println(f"{first(rows)}")
}
"#;
    let rust = lowered("run-element", source);
    assert!(!rust.contains("async fn first"), "{rust}");
}
