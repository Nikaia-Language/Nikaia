//! **A type that holds itself inline is refused here** — `NK1192` (0.0.234) —
//! rather than by `rustc`'s *recursive type has infinite size* about a file
//! nobody wrote.
//!
//! How the language should let a type hold itself is the owner's question
//! (`open-decisions.md`, *how a type holds itself*); until it is answered the
//! refusal names the way that works today, a list between them. It is found
//! by what writing a compiler in Nikaia needs first: a syntax tree is a type
//! that holds itself.

use nikaia::contracts::{Ledger, STD};
use nikaia::parser::parse_to_ast;

fn refused(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.code == "NK1192")
        .collect()
}

fn with_main(types: &str) -> String {
    format!("{types}\nfn main() {{ println(\"x\") }}\n")
}

#[test]
fn a_type_that_holds_itself_is_refused_by_how_it_does() {
    let direct = refused(&with_main("enum Expr { Leaf(i64), Add(Expr, Expr) }"));
    assert_eq!(direct.len(), 1, "{direct:#?}");
    assert_eq!(direct[0].message, "`Expr` holds itself, so it has no size");
    assert!(
        direct[0].notes[0].starts_with("`Expr` holds another `Expr`"),
        "{direct:#?}"
    );
    assert_eq!(
        direct[0].help.as_deref(),
        Some(
            "hold it through a list, which keeps its elements elsewhere: `Vec[Expr]` in \
             place of the `Expr` it holds"
        )
    );

    // Nullable is still inline: an absent `Node` has room for a present one.
    let nullable = refused(&with_main("struct Node { value: i64, next: Node? }"));
    assert_eq!(nullable.len(), 1, "{nullable:#?}");

    // And through another type, and through a tuple's part: both are inline,
    // and the note names the way round.
    let around = refused(&with_main(
        "struct A { b: B }\nstruct B { items: (i64, A) }",
    ));
    assert_eq!(around.len(), 2, "{around:#?}");
    assert!(
        around[0].notes[0].starts_with("`A` holds `B`, which holds `A`"),
        "{around:#?}"
    );
}

/// **What is not inline is not refused**: a list, a view and a function keep
/// what they refer to somewhere else, and a type that holds a *different*
/// type, even twice, has a size. None of these may be refused on a guess.
#[test]
fn a_type_held_through_something_else_is_left_alone() {
    for types in [
        "enum Expr { Leaf(i64), Add(Vec[Expr]) }",
        "struct Node { value: i64, next: ref Node }",
        "struct Handler { next: fn(i64) -> i64 }",
        "struct Pair { a: i64 }\nstruct Q { p: Pair, both: Array[Pair, 2], t: (Pair, Pair) }",
    ] {
        assert!(refused(&with_main(types)).is_empty(), "{types}");
    }
}
