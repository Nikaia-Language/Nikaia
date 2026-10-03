//! `///` before an item is its documentation
//! ([ADR-307](../../../docs/specification/adr/adr-307.md) D4), and **the
//! ledger does not carry it**
//! ([ADR-251](../../../docs/specification/adr/adr-251.md) D2, which withdraws
//! ADR-307 D5): `nikaia.contracts` is what a caller's compiler reads about a
//! body it cannot see, and a sentence is not that. The parser still reads the
//! prose, for whatever reaches a dependency's documentation later.

use nikaia::contracts::{Ledger, LedgerOps};
use nikaia::parser::parse_to_ast;

fn items(source: &str) -> Vec<(String, Option<String>)> {
    let parsed = parse_to_ast(source).expect("the source parses");
    parsed
        .program
        .items
        .iter()
        .map(|item| {
            (
                format!("{:?}", std::mem::discriminant(&item.node)),
                item.doc.clone(),
            )
        })
        .collect()
}

fn ledger(source: &str) -> Ledger {
    Ledger::infer(&parse_to_ast(source).expect("the source parses"))
}

/// **A run of `///` before an item is its documentation** (D1), joined by its
/// own line breaks with the slashes and one space taken off.
#[test]
fn a_run_of_slashes_is_the_items_documentation() {
    let found = items(
        "/// The status line for a response code.\n\
         ///\n\
         /// An unknown code is `500`.\n\
         pub fn status_line(code: i32) -> ref String { return \"200 OK\" }\n",
    );
    assert_eq!(
        found[0].1.as_deref(),
        Some("The status line for a response code.\n\nAn unknown code is `500`.")
    );
}

/// **Anywhere else `///` is an ordinary comment** (D1), which is what
/// [ADR-307](../../../docs/specification/adr/adr-307.md) D3 said of it and what
/// this leaves true: a run with a statement between it and the next item
/// belongs to nothing.
#[test]
fn prose_a_token_stands_between_belongs_to_nothing() {
    let found = items(
        "fn first() -> i64 {\n\
         \x20   /// not an item's\n\
         \x20   return 0\n\
         }\n\
         fn second() -> i64 { return 0 }\n",
    );
    assert_eq!(found[1].1, None, "the run is inside the body above it");
}

/// **An ordinary comment between the prose and the item does not end it.** It
/// is trivia, so no token has been consumed — and the shape it allows is the
/// one this repository is written in: a sentence for whoever reaches the item,
/// then a note for whoever reads the source.
#[test]
fn an_ordinary_comment_between_them_is_a_note() {
    let found = items(
        "/// What it is for.\n\
         // How it works, which is the source's business.\n\
         pub fn f() -> i64 { return 0 }\n",
    );
    assert_eq!(found[0].1.as_deref(), Some("What it is for."));
}

/// **`////` is an ordinary comment**, as it is in every language that has
/// both.
#[test]
fn four_slashes_are_a_comment() {
    let found = items("//// a rule\npub fn f() -> i64 { return 0 }\n");
    assert_eq!(found[0].1, None);
}

/// **An item does not inherit the one above it.**
#[test]
fn the_next_item_gets_nothing() {
    let found = items(
        "/// About the first.\n\
         pub fn first() -> i64 { return 0 }\n\
         pub fn second() -> i64 { return 0 }\n",
    );
    assert_eq!(found[0].1.as_deref(), Some("About the first."));
    assert_eq!(found[1].1, None);
}

/// **The ledger carries no prose** (ADR-251 D2): a `pub` function and a
/// `pub` type with `///` in front of them render no `doc`, and a ledger that
/// still has one is refused as a key nothing reads.
#[test]
fn the_ledger_carries_no_prose() {
    let ledger = ledger(
        "/// What the function is for.\n\
         pub fn f() -> i64 { return 0 }\n\
         /// What the type is for.\n\
         pub struct Row { name: ref String }\n",
    );
    let text = ledger.render();
    assert!(!text.contains("doc ="), "{text}");
    assert!(!text.contains("What the function is for"), "{text}");
    let old = text.replace(
        "[fn.\"f\"]\n",
        "[fn.\"f\"]\ndoc = \"What the function is for.\"\n",
    );
    assert!(Ledger::parse(&old).is_err(), "{old}");
}
