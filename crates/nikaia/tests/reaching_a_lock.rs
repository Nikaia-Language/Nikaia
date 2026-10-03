//! A call into foreign code from which a **lock** is reachable — `NK2503`.
//!
//! [ADR-281](../../../docs/specification/adr/adr-281.md) D32 and
//! [Part III 15.2](../../../docs/specification/30-nikaia-tooling.md), worked
//! through in C.6: *foreign code touches only what it reaches, and the language
//! has no global mutable data. Where no lock is reachable from the arguments,
//! transitively and through the fields of a struct, the call is allowed and the
//! compiler says nothing.*
//!
//! **The refusal was built and the number was not.** D6 says the check *is*
//! `NK2502`'s walk generalised and never a copy of it, and the walk has been
//! finding locks through fields since ADR-312 D8 — what came out was `NK2502`'s
//! diagnostic, about a value crossing a thread, where Part III C.6 writes a
//! refusal about the **call**. So what is here is one branch at the site that
//! already walks every argument, and these are the sentences it prints.
//!
//! **Which is why `Shared` had to be told from a lock first.** Three things
//! reach the refusing arm now — a lock, a shared count
//! ([ADR-312](../../../docs/specification/adr/adr-312.md) D9), and a described
//! type that says `crosses = false` — and only the first is this code. The
//! verdict carries the word; nothing here guesses it from a type's name.

use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn crossings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.code.starts_with("NK25"))
        .collect()
}

fn only(source: &str) -> Finding {
    let found = crossings(source);
    assert_eq!(found.len(), 1, "exactly one refusal: {found:#?}");
    found.into_iter().next().expect("one")
}

/// The lock handed straight in: the headline is about the **call**.
///
/// `NK2502` would say *`counter` may not cross a thread, and `fremd::irgendwas`
/// may put it on one* — true, and about the value. C.6's refusal is about what
/// the call can reach, which is the sentence a reader can act on: the way out
/// is keeping the lock out of its reach rather than not calling.
#[test]
fn a_lock_handed_straight_in_names_the_call() {
    let finding = only(
        "fn ueber(counter: SharedMut[i32]) {\n\
             fremd::irgendwas(counter)\n\
         }",
    );
    assert_eq!(finding.code, "NK2503");
    assert_eq!(
        finding.message,
        "`fremd::irgendwas` could reach a lock through `counter`."
    );
    let notes = finding.notes.join("\n");
    assert!(
        notes.contains("The compiler knows nothing about `fremd::irgendwas`"),
        "{notes}"
    );
    assert!(
        notes.contains("`counter` is a `SharedMut[i32]`"),
        "the note names the type the source wrote (ADR-281 D1): {notes}"
    );
    assert!(
        notes.contains("code that might keep a lock could deadlock"),
        "{notes}"
    );
    // Part III C.2: no Rust vocabulary in a diagnostic, ever.
    for word in ["Rc", "Arc", "Send", "Mutex", "E0277", "borrow"] {
        assert!(!notes.contains(word), "`{word}` in: {notes}");
    }
    let help = finding.help.expect("a refusal has a way out");
    assert!(help.contains("fremd::irgendwas(counter.get())"), "{help}");
}

/// **Part III C.6's own program**: the lock is a field, and the note names the
/// field rather than the struct.
///
/// The fields come from the ledger (13.5), so the rule reaches a type declared
/// in another file — and the path the way out prints is the argument's own name
/// with that field behind it, which is a line the program can be edited into.
#[test]
fn a_lock_through_a_field_names_the_field() {
    let finding = only(
        "pub struct Stats { counts: SharedMut[i64] }\n\
         fn ueber(state: Stats) {\n\
             hyper_shim::render(state)\n\
         }",
    );
    assert_eq!(finding.code, "NK2503");
    assert_eq!(
        finding.message,
        "`hyper_shim::render` could reach a lock through `state`."
    );
    assert!(
        finding
            .notes
            .join("\n")
            .contains("`state.counts` is a `SharedMut[i64]`"),
        "{:#?}",
        finding.notes
    );
    let help = finding.help.expect("a refusal has a way out");
    assert!(
        help.contains("hyper_shim::render(state.counts.get())"),
        "{help}"
    );
}

/// And through a struct of structs, which is the transitive case — the walk is
/// `NK2502`'s and reaches as far as it ever did.
#[test]
fn a_lock_two_structs_deep_is_still_reached() {
    let finding = only(
        "pub struct Stats { counts: SharedMut[i64] }\n\
         pub struct Report { stats: Stats }\n\
         fn ueber(r: Report) {\n\
             fremd::schreib(r)\n\
         }",
    );
    assert_eq!(finding.code, "NK2503");
    // The **innermost** field is the one named: it is the one a reader can look
    // at, and the path to it is not rebuilt from a walk that did not keep it.
    assert!(
        finding
            .notes
            .join("\n")
            .contains("`r.counts` is a `SharedMut[i64]`"),
        "{:#?}",
        finding.notes
    );
}

/// **A call that can reach no lock is allowed without a word** (15.2), which is
/// the half of the rule that a refusal makes expensive to get wrong.
#[test]
fn a_call_that_reaches_no_lock_is_silent() {
    for source in [
        "fn ueber(n: i64) { fremd::irgendwas(n) }",
        "fn ueber(v: Vec[String]) { fremd::irgendwas(v) }",
        "pub struct Reading { name: String, temp: f64 }\n\
         fn ueber(r: Reading) { fremd::irgendwas(r) }",
    ] {
        let found = crossings(source);
        assert!(found.is_empty(), "{source}: {found:#?}");
    }
}

/// **A shared count is not this refusal.** It may not go into code nothing
/// describes either ([ADR-312](../../../docs/specification/adr/adr-312.md) D9),
/// for a reason that has nothing to do with a lock — so it keeps `NK2502` and
/// the sentence that belongs to it.
///
/// This is the pair that makes the branch worth having: one verdict, two codes,
/// and the word on the verdict is what tells them apart.
#[test]
fn a_shared_count_is_not_this_refusal() {
    let finding = only(
        "fn ueber(handle: Shared[String]) {\n\
             fremd::irgendwas(handle)\n\
         }",
    );
    assert_eq!(finding.code, "NK2502");
    assert!(!finding.notes.join("\n").contains("lock"), "{finding:#?}");
}

/// A lock named at an **option** rather than positionally: the headline says the
/// option and the way out says the value behind it, because `counter:` is not
/// something a program can put a `.get()` on.
#[test]
fn a_lock_at_an_option_names_the_option_and_the_value() {
    let finding = only(
        "fn ueber(counter: SharedMut[i32]) {\n\
             fremd::irgendwas(1; wert: counter)\n\
         }",
    );
    assert_eq!(finding.code, "NK2503");
    assert_eq!(
        finding.message,
        "`fremd::irgendwas` could reach a lock through `wert`."
    );
    let help = finding.help.expect("a refusal has a way out");
    assert!(help.contains("fremd::irgendwas(counter.get())"), "{help}");
}

/// **A described call is not asked**, which is ADR-303 D7's own words and the
/// limit `NK2503` inherits with the walk: the rule is about a body this
/// compiler cannot see, and `std`'s are all written down.
#[test]
fn a_call_this_compiler_can_see_reaches_nothing() {
    let found = crossings(
        "fn halte(c: SharedMut[i32]) { }\n\
         fn ueber(counter: SharedMut[i32]) { halte(counter) }",
    );
    assert!(found.is_empty(), "{found:#?}");
}
