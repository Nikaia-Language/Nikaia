//! An error that newly reaches a `catch` is named once — `NK2402`
//! ([ADR-101](../../../docs/specification/adr/adr-101.md)).
//!
//! Every failure in a Nikaia program is caught or declared, and a `catch`
//! handles **everything** that reaches it. `throws` names no types at a
//! signature, so the set arriving at a handler is open: it grows whenever a
//! callee gains a failure. The handler is then still a correct program and
//! still handles the new error — as it handles everything.
//!
//! **What was missing is not a refusal. It is that nobody was told.** So this
//! is a warning, printed and stopping nothing, and given **once**: the commit
//! of the ledger diff is the acknowledgement, and after it the new set is the
//! baseline.
//!
//! *And the record's own §5 said it could not be built yet* — *`std.contracts`
//! writes `throws = ["?"]` on every entry, so there is no set to diff*. That
//! stopped being true when [ADR-280](../../../docs/specification/adr/adr-280.md)
//! gave `std` its error type: the file has nine `throws` lines now, seven of
//! them `io::IoError` and two `Overtaken`, and **not one** is `["?"]`. What is
//! left of that spelling is a **grammar** rule's entry, which is
//! issue #179 and a question on `open-decisions.md`.

use std::collections::BTreeSet;

use nikaia::assets::Reads;
use nikaia::check::{self, Finding, Newly};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str, newly: &[(&str, &[&str])]) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let newly: Newly = newly
        .iter()
        .map(|(name, errors)| {
            (
                name.to_string(),
                errors.iter().map(|e| e.to_string()).collect(),
            )
        })
        .collect();
    check::check_against(
        &parsed,
        &[],
        &own,
        &library,
        &BTreeSet::new(),
        &newly,
        // No allowlist: a build given none reads nothing (ADR-072 D1).
        &Reads::none(),
    )
    .findings
}

const HANDLED: &str = "use std::io\n\n\
                       fn reads() -> String throws {\n\
                       \x20   return io::read_to_string()\n\
                       }\n\
                       \n\
                       fn main() {\n\
                       \x20   let text = reads() catch { \"nothing\".clone() }\n\
                       \x20   println(text)\n\
                       }\n";

/// **The note, and it is a warning.** A program is never refused for this
/// (ADR-101 §3), so the severity is what says so.
#[test]
fn a_handler_is_told_what_newly_reaches_it() {
    let found = findings(HANDLED, &[("reads", &["io::IoError"])]);
    let note = found
        .iter()
        .find(|f| f.code == "NK2402")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert_eq!(note.severity, check::Severity::Warning);
    assert_eq!(
        note.message,
        "This `catch` now also receives `io::IoError` from `reads`."
    );
    // Part III C.2: the reason, and a way out that is not "don't do that".
    assert!(
        note.notes
            .join(" ")
            .contains("takes everything that reaches it"),
        "{:#?}",
        note.notes
    );
    assert!(
        note.help
            .as_deref()
            .is_some_and(|h| h.contains("are both fine")),
        "{:?}",
        note.help
    );
    // …and nothing else is said about the program.
    assert!(
        !found.iter().any(|f| f.severity == check::Severity::Error),
        "{found:#?}"
    );
}

/// **Nothing gained, nothing said** — which is every build but the one after
/// the change, and is what makes the note worth reading when it comes.
#[test]
fn a_handler_over_an_unchanged_callee_is_left_alone() {
    assert!(findings(HANDLED, &[]).is_empty());
    // …including where some *other* callee gained something.
    assert!(findings(HANDLED, &[("elsewhere", &["io::IoError"])]).is_empty());
}

/// **Only inside a `catch`.** The same call in a `throws` function has gained
/// the same error, and there is nothing to tell anybody: the failure travels
/// on, and whoever handles it is who this note is for.
#[test]
fn a_call_outside_a_handler_is_not_noted() {
    let found = findings(
        "use std::io\n\n\
         fn reads() -> String throws {\n\
         \x20   return io::read_to_string()\n\
         }\n\
         \n\
         fn onwards() -> String throws {\n\
         \x20   return reads()\n\
         }\n\
         \n\
         fn main() { println(onwards() catch { \"\".clone() }) }\n",
        &[("reads", &["io::IoError"])],
    );
    assert!(
        !found.iter().any(|f| f.code == "NK2402"),
        "the `catch` is over `onwards`, not over `reads`:\n{found:#?}"
    );
}

/// **A handler that matches and one that does not are treated alike** (D3),
/// because the question is the same for both: is this new error right where it
/// landed?
#[test]
fn every_shape_of_handler_gets_the_same_note() {
    for handler in [
        "catch { \"nothing\".clone() }",
        "catch { f\"{error}\" }",
        "catch { match error { else => \"other\".clone() } }",
    ] {
        let source = format!(
            "use std::io\n\n\
             fn reads() -> String throws {{\n\
             \x20   return io::read_to_string()\n\
             }}\n\
             \n\
             fn main() {{\n\
             \x20   let text = reads() {handler}\n\
             \x20   println(text)\n\
             }}\n"
        );
        let found = findings(&source, &[("reads", &["io::IoError"])]);
        assert!(
            found.iter().any(|f| f.code == "NK2402"),
            "{handler}:\n{found:#?}"
        );
    }
}

/// **Every error it gained, in one sentence.** A set that grows by two is one
/// note and not two: the question a reader is being asked is about this
/// handler, once.
#[test]
fn a_set_that_gained_two_is_one_note() {
    let found = findings(HANDLED, &[("reads", &["io::IoError", "Overtaken"])]);
    let notes: Vec<&Finding> = found.iter().filter(|f| f.code == "NK2402").collect();
    assert_eq!(notes.len(), 1, "{found:#?}");
    assert!(notes[0].message.contains("`io::IoError`"), "{:?}", notes[0]);
    assert!(notes[0].message.contains("`Overtaken`"), "{:?}", notes[0]);
}

/// **The corpus says nothing**, which is the guard: there are 22 handlers in
/// `examples/` and `tests/samples/`, and an empty `newly` is what every build
/// of them has. A note that appeared without a change would be worse than no
/// note at all.
#[test]
fn no_handler_in_the_repository_is_noted_without_a_change() {
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut checked = 0;
    for path in every_program(&root) {
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(parsed) = parse_to_ast(&source) else {
            continue;
        };
        let own = Ledger::infer(&parsed);
        let found = check::check_against(
            &parsed,
            &[],
            &own,
            &library,
            &BTreeSet::new(),
            &Newly::new(),
            &Reads::none(),
        )
        .findings;
        assert!(
            !found.iter().any(|f| f.code == "NK2402"),
            "{}: {found:#?}",
            path.display()
        );
        checked += 1;
    }
    assert!(checked >= 12, "only {checked} programs were checked");
}

fn every_program(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![root.join("examples"), root.join("tests/samples")];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("nika") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}
