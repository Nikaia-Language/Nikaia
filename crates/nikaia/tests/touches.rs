//! `touches` is read off a body, like the three derived columns beside it
//! ([ADR-288](../../../docs/specification/adr/adr-288.md) D22).
//!
//! It was specified with the same fail-closed polarity as `sync`, `throws` and
//! `sharing` ([ADR-292](../../../docs/specification/adr/adr-292.md) D3) and was
//! the one of the four that never got an inference — so every function a `.nika`
//! file declared answered *"nobody said"*, which means *"it touches
//! everything"*. Safe, and useless: the walk stopped at the first call out of
//! `std`.

use nikaia::contracts::{Ledger, LedgerOps};
use nikaia::parser::parse_to_ast;

/// What the ledger says one function reaches, after the inference.
fn touches(source: &str, name: &str) -> (bool, Vec<String>) {
    let parsed = parse_to_ast(source).expect("the source parses");
    let ledger = Ledger::infer(&parsed);
    let contract = &ledger.functions[name];
    (
        contract.touches_known,
        contract
            .touches
            .iter()
            .map(|t| format!("{} {}", t.kind, if t.write { "write" } else { "read" }))
            .collect(),
    )
}

/// **A body that reaches nothing says so**, and that is a claim rather than a
/// blank: it orders against nothing.
#[test]
fn a_function_that_reaches_nothing_says_it_reaches_nothing() {
    let (known, found) = touches("fn double(n: i64) -> i64 { return n * 2 }", "double");
    assert!(known, "it is an answer and not an absence");
    assert!(found.is_empty(), "{found:?}");
}

/// **And what it reaches travels up the call graph**, which is the whole of why
/// the column needed an inference: `println` says it writes standard output, and
/// before this nothing carried that as far as the function around it.
#[test]
fn what_a_callee_reaches_is_what_the_caller_reaches() {
    let (known, found) = touches(
        "fn say(n: i64) { println(f\"{n}\") }\n\
         fn twice(n: i64) { say(n)\n say(n) }",
        "twice",
    );
    assert!(known, "every step of it is described");
    assert_eq!(found, vec!["stdout write".to_string()]);
}

/// **A lock is a resource** ([ADR-288](../../../docs/specification/adr/adr-288.md)
/// D3), named the day the doors existed and not before.
///
/// It says *a* lock and never *which*, which is
/// [ADR-281](../../../docs/specification/adr/adr-281.md) D30's decision: telling
/// two handles apart would make whether a program compiles depend on whether that
/// proof happened to succeed.
#[test]
fn a_door_reaches_a_lock() {
    let (known, found) = touches(
        "fn bump(k: SharedMut[i64]) { k.update fn(mut alt) { alt += 1 } }",
        "bump",
    );
    assert!(known, "{found:?}");
    assert_eq!(found, vec!["lock write".to_string()]);

    // Reading one is a read, so two of them do not conflict.
    let (_, reading) = touches(
        "fn peek(k: SharedMut[i64]) -> i64 { return k.get() }",
        "peek",
    );
    assert_eq!(reading, vec!["lock read".to_string()]);
}

/// **And the print inside a door is now visible to the compiler**, which is what
/// the whole column is for here: it reaches a lock *and* standard output, so
/// something that refuses one inside the other has the fact it needs.
#[test]
fn a_print_inside_a_door_reaches_both() {
    let (known, found) = touches(
        "fn noisy(k: SharedMut[i64]) { k.update fn(mut alt) { println(f\"{alt}\")\n alt += 1 } }",
        "noisy",
    );
    assert!(known, "{found:?}");
    assert_eq!(
        found,
        vec!["lock write".to_string(), "stdout write".to_string()]
    );
}

/// **Nobody said stays nobody said.** A call this compiler cannot name takes the
/// claim away, and no fixpoint brings it back — the polarity the column was
/// written with, kept by the inference that fills it.
#[test]
fn a_call_nothing_describes_takes_the_claim_away() {
    let (known, found) = touches("fn odd() { fremd::irgendwas() }", "odd");
    assert!(!known, "an unaccounted call is not an empty touch set");
    assert!(found.is_empty());
}

/// A resource named by a **parameter** does not travel, because the name is the
/// callee's: a caller's argument may be a literal or carry another name, and
/// mapping one onto the other is its own piece of work.
#[test]
fn a_resource_named_by_a_parameter_does_not_travel() {
    let (known, _) = touches(
        "use std::fs\nfn load(p: ref String) -> String { return fs::read_to_string(p, fs::Root::Anywhere) }",
        "load",
    );
    assert!(
        !known,
        "`file(path)` is `fs::read_to_string`'s parameter, not `load`'s"
    );
}

/// Two functions that call each other and touch nothing keep the claim, which is
/// what a **greatest** fixpoint is for — `sync::infer`'s reason, one column over.
#[test]
fn mutual_recursion_that_reaches_nothing_keeps_the_claim() {
    let (known, found) = touches(
        "fn even(n: i64) -> bool { if n == 0 { return true }\n return odd(n - 1) }\n\
         fn odd(n: i64) -> bool { if n == 0 { return false }\n return even(n - 1) }",
        "even",
    );
    assert!(known, "{found:?}");
    assert!(found.is_empty());
}
