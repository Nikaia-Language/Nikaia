//! `[1, 2, 3]`, and the two questions the shape asks
//! ([ADR-135](../../../docs/specification/adr/adr-135.md)).
//!
//! A list is the language's most-written container and there was no way to
//! write one down: every program in `examples/` that wanted one built it a
//! `push` at a time. The syntax was free — nothing in the grammar began with a
//! `[` in expression position — so the only cost of taking it was the record's
//! two rulings, and both are pinned here: what `[]` is (D2), and what a `[` at
//! the start of a line means (D3).

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust
}

/// **`[1, 2, 3]`**, and `vec![…]` below — which is what a `Vec` already is
/// there, so the lowering is a transcription (D1).
#[test]
fn a_list_literal_is_a_vec() {
    let source = "fn main() {\n\
                  \x20   let xs = [1, 2, 3]\n\
                  \x20   println(f\"{xs.len()}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("let xs = vec![1, 2, 3];"),
        "{}",
        lowered(source)
    );
}

/// **A trailing comma**, because a list is what a program grows a line at a
/// time and a diff that touches two lines to add one is the cost of forbidding
/// it.
#[test]
fn a_trailing_comma_is_allowed() {
    let source = "fn main() {\n\
                  \x20   let names = [\"a\", \"b\",]\n\
                  \x20   println(f\"{names.len()}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("vec![\"a\", \"b\"]"),
        "{}",
        lowered(source)
    );
}

/// **`[]` with the type beside it** (D2's first spelling).
#[test]
fn an_empty_list_takes_the_type_the_let_writes() {
    let source = "fn main() {\n\
                  \x20   let mut xs: Vec[i64] = []\n\
                  \x20   xs.push(4)\n\
                  \x20   println(f\"{xs.len()}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("let mut xs: Vec<i64> = vec![];"),
        "{}",
        lowered(source)
    );
}

/// **`[]` and a `push`** (D2's second): the first use that needs an element
/// type is what gives it one, so nothing here is refused.
#[test]
fn an_empty_list_a_push_answers_is_not_refused() {
    let source = "fn main() {\n\
                  \x20   let mut xs = []\n\
                  \x20   xs.push(1)\n\
                  \x20   println(f\"{xs.len()}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("let mut xs = vec![];"),
        "{}",
        lowered(source)
    );
}

/// **`[]` that nothing ever uses is refused, and not guessed** (D2). A default
/// element type would be a type nobody wrote.
#[test]
fn an_empty_list_nothing_uses_is_refused() {
    let found: Vec<_> = findings("fn main() {\n\x20   let xs = []\n}\n")
        .into_iter()
        .filter(|f| f.code == "NK1153")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0]
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("Vec[i64]"),
        "{found:#?}"
    );
}

/// **A number beside text**, where neither has a type yet: a bare `1` fits
/// every numeric type and arrives as `?`, so the **kind** is what answers.
#[test]
fn a_number_and_a_piece_of_text_do_not_agree() {
    let found: Vec<_> =
        findings("fn main() {\n\x20   let xs = [1, \"two\"]\n\x20   println(f\"{xs.len()}\")\n}\n")
            .into_iter()
            .filter(|f| f.code == "NK1154")
            .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("a number"), "{found:#?}");
}

/// **And where both have one, the message names both types.**
#[test]
fn two_types_that_do_not_agree_are_both_named() {
    let found: Vec<_> = findings(
        "fn main() {\n\
         \x20   let a: i64 = 1\n\
         \x20   let xs = [a, \"two\"]\n\
         \x20   println(f\"{xs.len()}\")\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1154")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("`i64`"), "{found:#?}");
    assert!(found[0].message.contains("`ref String`"), "{found:#?}");
}

/// **Once per literal.** Three elements that disagree with the first are one
/// mistake, not three.
#[test]
fn a_literal_that_disagrees_is_one_finding() {
    let found: Vec<_> = findings(
        "fn main() {\n\x20   let xs = [\"a\", 1, 2, 3]\n\x20   println(f\"{xs.len()}\")\n}\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1154")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
}

/// **D3: a `[` at the start of a line begins a literal.** Before this the same
/// two lines were an index of the line above — `println!(…)[…]` in the language
/// below, which is [Part III
/// C.1](../../../docs/specification/30-nikaia-tooling.md)'s class exactly.
#[test]
fn a_bracket_that_starts_a_line_is_a_literal() {
    let source = "fn main() {\n\
                  \x20   let n = 1\n\
                  \x20   println(f\"{n}\")\n\
                  \x20   [n].len()\n\
                  }\n";
    let rust = lowered(source);
    assert!(rust.contains("vec![n].len()"), "{rust}");
    assert!(!rust.contains("n)["), "{rust}");
}

/// **And an index is still an index**, because it is written on the line its
/// subject is on — including where *that* line is the first thing in a block.
#[test]
fn an_index_on_its_subjects_line_still_indexes() {
    let source = "fn main() {\n\
                  \x20   let xs = [10, 20, 30]\n\
                  \x20   println(f\"{xs[0]}\")\n\
                  \x20   let y = xs [2]\n\
                  \x20   println(f\"{y}\")\n\
                  }\n";
    let rust = lowered(source);
    // **A read is `index::get` since
    // [ADR-293](../../../docs/specification/adr/adr-293.md) D9**, so what this
    // asserts is the **subject** — that `xs [2]` on one line reads `xs` and is
    // not a list literal of its own.
    assert!(rust.contains("nikaia_std::index::get(&xs,"), "{rust}");
    assert!(
        rust.contains("let y = *nikaia_std::index::get(&xs, 2);"),
        "{rust}"
    );
}

/// **A comment between them changes nothing**, on either side of the line
/// break: what a `[` follows is the line it is on.
#[test]
fn a_comment_does_not_move_the_line() {
    let same = lowered(
        "fn main() {\n\x20   let xs = [1, 2]\n\x20   println(f\"{xs /* here */ [0]}\")\n}\n",
    );
    assert!(same.contains("nikaia_std::index::get(&xs,"), "{same}");
    let across = lowered(
        "fn main() {\n\x20   let n = 1\n\x20   println(f\"{n}\")\n\x20   /* here */ [n].len()\n}\n",
    );
    assert!(across.contains("vec![n].len()"), "{across}");
}

/// **A list of lists** is a list whose element type is a list, and nothing in
/// the rule is special about it.
#[test]
fn a_list_of_lists_is_a_list() {
    let source = "fn main() {\n\
                  \x20   let grid = [[1, 2], [3, 4]]\n\
                  \x20   println(f\"{grid.len()}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("vec![vec![1, 2], vec![3, 4]]"),
        "{}",
        lowered(source)
    );
}
