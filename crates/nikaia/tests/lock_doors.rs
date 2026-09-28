//! The two refusals that come with the lock's doors
//! ([ADR-099](../../../docs/specification/adr/adr-099.md)).
//!
//! [ADR-039](../../../docs/specification/adr/adr-039.md) D10 gave shared
//! mutable state four doors and stated the two mistakes that come with them.
//! Both were catalogued in Part III Appendix C and raised by nothing, which is
//! the state `open-work.md` §2 calls the one that rots fastest: a rule with no
//! program to be tested against quietly stops being true.
//!
//! **Both are local**, which is why they are these two and not the other three
//! the same entry lists: `NK2201` and `NK2203` need to know what is *inside a
//! door* and what a chain of calls reaches, and `NK2503` needs the reachability
//! walk. These need one statement each.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings
}

fn coded(source: &str, code: &str) -> Vec<check::Finding> {
    findings(source)
        .into_iter()
        .filter(|f| f.code == code)
        .collect()
}

fn program(body: &str) -> String {
    format!("fn main() {{\n    let kasse = SharedMut(0)\n{body}\n    println(\"x\")\n}}\n")
}

/// **`NK2204`, and the message names the door with the value in it.**
///
/// The repair is mechanical, and a help line that said *"use a door"* would
/// leave the reader to work out which of the four.
#[test]
fn a_write_straight_into_shared_mutable_state_names_the_door() {
    let found = coded(&program("    kasse = 42"), "NK2204");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0]
            .message
            .contains("holds shared mutable state, and this assigns to it directly"),
        "{:#?}",
        found[0]
    );
    assert_eq!(
        found[0].help.as_deref(),
        Some("write `kasse.set(42)`"),
        "the door, with the value the author wrote in it"
    );
}

/// **What this compiler cannot quote back, it does not quote.**
///
/// An expression has no span — [ADR-081](../../../docs/specification/adr/adr-081.md)
/// D2 gave one to `Binary` and to nothing else — so a value it cannot rebuild
/// becomes an ellipsis rather than a guess. A help line that quoted the wrong
/// thing would be worse than one that quotes nothing.
#[test]
fn a_value_that_cannot_be_rebuilt_is_an_ellipsis() {
    let found = coded(&program("    let n = 1\n    kasse = n"), "NK2204");
    assert_eq!(
        found[0].help.as_deref(),
        Some("write `kasse.set(n)`"),
        "a name is rebuilt"
    );
    let found = coded(&program("    kasse = 1 + 2"), "NK2204");
    assert_eq!(
        found[0].help.as_deref(),
        Some("write `kasse.set(…)`"),
        "and arithmetic is not guessed at"
    );
}

/// **`NK2205`: the shape people write**, and the message names the door that
/// takes the lock once.
#[test]
fn a_set_that_reads_what_it_writes_names_the_third_door() {
    let found = coded(&program("    kasse.set(kasse.get() + 100)"), "NK2205");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0]
            .message
            .contains("stores a value that was read from a lock"),
        "{:#?}",
        found[0]
    );
    assert!(
        found[0]
            .help
            .as_deref()
            .is_some_and(|h| h.contains("kasse.update fn(mut v)")),
        "{:#?}",
        found[0]
    );
}

/// **The `get` one operator down is still inside**, which is why this walks the
/// argument rather than reading the top of it — and the shape the
/// specification's own example writes puts it exactly there.
#[test]
fn the_read_is_found_however_deep_it_is_written() {
    for body in [
        "    kasse.set(kasse.get())",
        "    kasse.set(kasse.get() + 100)",
        "    kasse.set((kasse.get() * 2) - 1)",
    ] {
        assert_eq!(
            coded(&program(body), "NK2205").len(),
            1,
            "`{body}` reads what it writes"
        );
    }
}

/// **What must stay quiet**, which is the half that decides whether these
/// refusals cost anything.
///
/// A value computed outside the lock is what `set` is **for**: a starting
/// value, a configuration that arrived from outside, a reset an operator asked
/// for ([ADR-111](../../../docs/specification/adr/adr-111.md) D4).
#[test]
fn what_is_not_the_shape_is_left_alone() {
    for body in ["    kasse.set(42)", "    let n = 7\n    kasse.set(n * 6)"] {
        assert!(
            coded(&program(body), "NK2205").is_empty(),
            "`{body}` is a program:\n{:#?}",
            findings(&program(body))
        );
    }
}

/// **And the two shapes that used to be quiet and are not.**
///
/// `NK2205` used to ask whether the argument contained a `get` **on the same
/// container**, which caught the one line and nothing else. ADR-111 widened it
/// to *the value carries where it came from*, and two things follow that the
/// old rule let through:
///
/// * the same pair **spread over two lines**, which the old rule called a
///   question about what happened between two statements and D4 answers
///   outright: *whether `stand` was read on the line above, in another
///   function, or in another request*;
/// * a value read from **another** lock, because `set`'s own `touches` names
///   one (D2), so it takes a `Seen` only where its signature says `Seen` — and
///   it does not. The stamp does not record **which** lock, and that is the
///   design rather than a limit: the whole point is that no analysis follows
///   the value.
#[test]
fn a_stamp_from_anywhere_is_refused() {
    for body in [
        "    let old = kasse.get()\n    kasse.set(old + 1)",
        "    let other = SharedMut(1)\n    kasse.set(other.get() + 1)",
    ] {
        assert_eq!(
            coded(&program(body), "NK2205").len(),
            1,
            "`{body}` stores what a lock handed out"
        );
    }
}

/// **A decision read from a lock is stale too**
/// ([ADR-111](../../../docs/specification/adr/adr-111.md) D4's second shape):
/// the value stored is plain, and what may have changed is the condition.
#[test]
fn a_set_under_a_stamped_condition_is_refused() {
    for body in [
        "    let stand = kasse.get()\n    if stand > 100 { kasse.set(0) }",
        "    let stand = kasse.get()\n    if stand > 100 { if true { kasse.set(0) } }",
    ] {
        assert_eq!(
            coded(&program(body), "NK2205").len(),
            1,
            "`{body}` decides on what the lock said"
        );
    }

    // And a plain condition is not one, which is the half that says the rule
    // reads the condition rather than the `if`.
    assert!(
        coded(
            &program("    let n = 7\n    if n > 3 { kasse.set(0) }"),
            "NK2205"
        )
        .is_empty()
    );
}

/// **And an ordinary variable is not a hull**, which keeps `NK2204` off every
/// assignment in every program.
#[test]
fn an_ordinary_assignment_is_untouched() {
    assert!(
        coded(
            "fn main() {\n    let mut n = 1\n    n = 2\n    println(f\"{n}\")\n}\n",
            "NK2204"
        )
        .is_empty(),
        "a `mut` binding is not a door"
    );
}

/// **The corpus**, because a refusal added to a language with programs in it
/// has to be checked against them.
#[test]
fn nothing_in_the_corpus_is_newly_refused() {
    let mut refused = Vec::new();
    for directory in ["examples", "tests/samples", "crates/nikaia-std/src"] {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(directory);
        for path in common::standalone_programs(&root) {
            if path.extension().is_none_or(|e| e != "nika") {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            if parse_to_ast(&source).is_err() {
                continue;
            }
            for finding in findings(&source) {
                if matches!(finding.code, "NK2204" | "NK2205") {
                    refused.push(format!("{}: {}", path.display(), finding.message));
                }
            }
        }
    }
    assert!(refused.is_empty(), "{refused:#?}");
}
