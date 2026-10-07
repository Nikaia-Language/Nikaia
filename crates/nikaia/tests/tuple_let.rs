//! A `let` takes one name, or a flat tuple of them
//! ([ADR-291](../../../docs/specification/adr/adr-291.md)).
//!
//! Part I 8.1.2 writes `let (user, rights, prefs) = overlap { … }` and Part II
//! 12.5 writes `let (tx, rx) = channel::bounded(100)`. Neither parsed: Part I
//! 2.1 introduces `let` with a name and says nothing about a pattern, so the
//! specification used a form twice, for two different constructs, and defined
//! it nowhere.
//!
//! **It is not a pattern language.** `match` has patterns already and neither
//! site needs them; what the two write is a tuple whose arity is known, taken
//! apart by position.
//!
//! What the parts hold when it runs is `tests/language/src/tuple_let.nika`.

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

fn lowered(source: &str) -> String {
    let found = findings(source);
    assert!(found.is_empty(), "a correct program: {found:#?}");
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// **The whole shape, lowered** — taken apart by position. That the parts keep
/// their types when it runs is `tests/language/src/tuple_let.nika`.
#[test]
fn a_tuple_of_names_binds_the_parts() {
    let source = r#"
fn pair() -> (i64, String) {
    return (7, "seven".to_string())
}

fn main() {
    let (n, word) = pair()
    let (a, b, c) = (1, 2, 3)
    println(f"{n} {word} {a} {b} {c}")
}
"#;
    let rust = lowered(source);
    assert!(rust.contains("let (n, word) = pair();"), "{rust}");
    assert!(rust.contains("let (a, b, c) = "), "{rust}");
}

/// **The line Part I 8.1.2 writes**, which is one of the two sites this exists
/// for. It used to be reached by its tuple — `let r = overlap { … }`, then
/// `r.0` — which works and reads worse than what the page promises.
#[test]
fn part_one_eight_one_twos_own_line_is_a_program() {
    let source = r#"
fn one() -> i64 { return 1 }
fn two() -> i64 { return 2 }

fn main() {
    let (a, b) = overlap {
        one()
        two()
    }
    println(f"{a} {b}")
}
"#;
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(lowered(source).contains("let (a, b) = "), "taken apart");
}

/// **A part's type comes from the value's**, where the value's is a tuple of
/// the right width — so a part answers for itself afterwards.
#[test]
fn a_part_carries_the_type_it_was_taken_from() {
    let source = r#"
fn pair() -> (i64, String) {
    return (7, "seven".to_string())
}

fn takes(n: i64) -> i64 { return n }

fn main() {
    let (n, word) = pair()
    let m = takes(word)
    println(f"{n} {m}")
}
"#;
    let found = findings(source);
    assert!(
        found.iter().any(|f| f.code == "NK1102"),
        "`word` is the `String` half, and `takes` wants an `i64`: {found:#?}"
    );
}

/// **A written type is refused rather than ignored**: `let (a, b): T = …` would
/// have to say which name `T` is about and nothing decides that. Ignoring it
/// would take something the author wrote and drop it.
#[test]
fn a_tuple_let_takes_no_type() {
    let found =
        findings("fn main() {\n    let (a, b): i64 = (1, 2)\n    println(f\"{a}{b}\")\n}\n");
    assert!(found.iter().any(|f| f.code == "NK1136"), "{found:#?}");
}

/// **A nested tuple is refused with a sentence**, not with a list of tokens —
/// which is this compiler's rule for a form nobody decided.
#[test]
fn a_nested_tuple_is_refused_and_says_why() {
    let source = "fn main() {\n    let ((a, b), c) = ((1, 2), 3)\n}\n";
    let refused = parse_to_ast(source).expect_err("a nested tuple is not this form");
    let said = rendered(&refused, source);
    assert!(
        said.contains("A tuple inside a tuple can't be taken apart here"),
        "the shape is named, not the token:\n{said}"
    );
    assert!(
        said.contains("`let (tx, rx) = …`"),
        "and the form that does work is shown:\n{said}"
    );
}

/// **And an ordinary bracket that fails to parse gets no such note**, which is
/// what keeps the note from becoming noise on every unbalanced paren.
#[test]
fn an_unrelated_bracket_is_left_alone() {
    let source = "fn main() {\n    let x = (1 +\n    println(\"x\")\n}\n";
    let refused = parse_to_ast(source).expect_err("this does not parse either");
    let said = rendered(&refused, source);
    assert!(
        !said.contains("A tuple inside a tuple can't be taken apart here"),
        "{said}"
    );
}

/// The whole refusal as a reader sees it: the headline, and the note and help
/// the finding it carries holds.
fn rendered(refused: &anyhow::Error, source: &str) -> String {
    let finding = nikaia::diagnostics::refused_finding(refused)
        .unwrap_or_else(|| panic!("a parse error carries its finding: {refused:#}"));
    nikaia::diagnostics::render_finding(finding, "main.nika", source)
}

/// **One name still binds one name**, which is every `let` in the tree.
#[test]
fn the_single_name_form_is_untouched() {
    let rust = lowered("fn main() {\n    let mut x = 1\n    x += 1\n    println(f\"{x}\")\n}\n");
    assert!(rust.contains("let mut x = 1;"), "{rust}");
}
