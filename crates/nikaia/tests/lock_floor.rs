//! What an always-`Mutex` floor for `Locked[T]` would do to `sync`.
//!
//! **A probe, not a feature**, and it stays one now that the answer is decided
//! ([ADR-057](../../../docs/specification/adr/adr-057.md)). Nothing here emits
//! anything; what it does is hold on to the *other* answer's cost, so the
//! decision keeps its evidence rather than only its conclusion. `with_access`
//! therefore takes the shipped entry out before putting a variant in. What it does is put
//! Part II 12.2's open question to the *real* inference
//! ([`contracts::sync`](../src/contracts/sync.rs), ADR-027) instead of arguing
//! it: the one thing an always-`Mutex` floor could break is whether
//! `Locked::access` is still `sync`, and that turns entirely on one line in a
//! ledger.
//!
//! The type checker cannot resolve `counter.access` today (there is no
//! `Locked` type for it to resolve *through*), so its answer is handed in —
//! which is what `sync::infer` takes it as anyway (ADR-028). The library
//! ledger is `std`'s with one entry appended, and the two spellings of that
//! entry are the two answers the floor could take:
//!
//! | the entry | what the floor is claiming | what the inference then says |
//! | :--- | :--- | :--- |
//! | `sync = "from(f)"` | an acquisition adds no pausing of its own | 12.2's idiom is `sync` |
//! | no `sync` line | an acquisition can pause | 12.2's idiom is not, and every caller of it loses the claim too |
//!
//! The finding is in [`docs/mutex-floor.md`](../../../docs/mutex-floor.md).

use std::collections::BTreeMap;

use nikaia::check::MethodCalls;
use nikaia::contracts::{Ledger, LedgerOps, STD, Sync, sync};
use nikaia::parser::parse_to_ast;

/// Part II 12.2's idiom, with a helper below it so the propagation is visible.
const IDIOM: &str = "fn tally(counter: SharedMut[i32]) {\n\
                     \x20   counter.access fn { a += 1 }\n\
                     }\n\
                     fn outer(counter: SharedMut[i32]) {\n\
                     \x20   tally(counter)\n\
                     }";

/// `std`'s ledger with its **own** `Locked::access` entry replaced by the one
/// this probe wants to ask about.
///
/// The entry is real now ([ADR-057](../../../docs/specification/adr/adr-057.md)
/// §5) and says `sync = "from(f)"`. Appending to the shipped text would leave
/// the shipped answer standing and quietly turn the second row below into the
/// first, so the shipped one is taken out first — which is what keeps this file
/// a probe of both answers rather than a test of the decided one.
fn with_access(sync_line: &str) -> Ledger {
    let text = format!(
        "{}\n[fn.\"SharedMut::access\"]\npub = true\n{sync_line}signature = \"(ref SharedMut[$T], f: fn(ref $T))\"\n",
        without_access(STD)
    );
    Ledger::parse(&text).expect("std's ledger plus one entry still parses")
}

/// The shipped ledger with the `Locked::access` block cut out, comments and all.
fn without_access(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut skipping = false;
    for line in text.lines() {
        if line.starts_with('[') {
            // **Both doors**: the idiom writes `SharedMut` since
            // [ADR-064](../../../docs/specification/adr/adr-064.md), and leaving
            // the shipped entry for either standing would answer the question
            // this probe is asking.
            skipping = line.starts_with("[fn.\"Locked::access\"]")
                || line.starts_with("[fn.\"SharedMut::access\"]");
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// What the compiler concludes about `tally`, given a library ledger and the
/// type checker's answer for `counter.access`.
fn sync_of(source: &str, library: &Ledger, resolved: bool) -> BTreeMap<String, Sync> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let mut ledger = Ledger::infer(&parsed);
    // **Back to what the declarations say**, before the hypothetical library is
    // asked. `Ledger::infer` runs `sync::infer` itself, against the **shipped**
    // ledger - and since [ADR-064](../../../docs/specification/adr/adr-064.md)
    // that ledger describes `SharedMut::access`, so the shipped answer is already
    // in here. `sync::infer` only ever raises a claim, never lowers one, so
    // without this reset the second row of the table above could not be asked at
    // all: the probe would measure the shipped entry and call it the variant's.
    for contract in ledger.functions.values_mut() {
        if contract.sync_claim == Sync::Inferred {
            contract.sync_claim = Sync::No;
        }
    }

    let mut methods = BTreeMap::new();
    if resolved {
        for name in ["tally", "outer"] {
            let mut calls = MethodCalls::default();
            if name == "tally" {
                calls.resolved.insert("SharedMut::access".to_string());
            }
            methods.insert(name.to_string(), calls);
        }
    }
    sync::infer(&mut ledger, &[&parsed], library, &methods);

    ledger
        .functions
        .iter()
        .map(|(name, contract)| (name.clone(), contract.sync_claim.clone()))
        .collect()
}

/// **The baseline moved, and that is the finding**
/// ([ADR-064](../../../docs/specification/adr/adr-064.md)).
///
/// This used to assert that Part II 12.2's own idiom is *not* `sync`, for a
/// reason that had nothing to do with the representation: `counter.access` was a
/// method call on a type **no ledger described**, so ADR-027 D2's polarity took
/// the claim away. `SharedMut` is described now - four doors of its own - so the
/// idiom carries its claim, and the rows below measure a real difference rather
/// than one that was hidden behind an absence.
#[test]
fn the_idiom_is_sync_now_that_the_ledger_describes_its_door() {
    let answers = sync_of(IDIOM, &Ledger::parse(STD).expect("std's ledger"), true);
    assert_eq!(answers["tally"], Sync::Inferred);
    assert_eq!(answers["outer"], Sync::Inferred);
}

/// With an entry that says an acquisition adds no pausing of its own, the
/// idiom is `sync` — and so is everything above it.
///
/// This is the `sync = "from(f)"` shape ADR-029 D3 built for `and_modify`, and
/// it is the shape an acquisition fits: the lambda runs *during* the call, so
/// its body is already counted in the function that writes it (D4's bound).
/// **An always-`Mutex` floor changes nothing here** — a `Mutex` acquisition is
/// not a suspension point, which is the property `sync` is about (ADR-005 D6
/// names it "no *suspension point* while a lock is held").
#[test]
fn a_non_pausing_acquisition_leaves_the_idiom_sync() {
    let answers = sync_of(IDIOM, &with_access("sync = \"from(f)\"\n"), true);
    assert_eq!(answers["tally"], Sync::Inferred);
    assert_eq!(answers["outer"], Sync::Inferred);
}

/// And the lambda still decides, which is what keeps `from` from being a hole:
/// the same entry over a lambda that does I/O leaves the function not `sync`.
#[test]
fn the_lambda_still_decides_under_from() {
    let source = "use std::fs\n\
                  fn tally(counter: SharedMut[i32]) {\n\
                      counter.access fn { fs::write(\"log\", fs::Root::Anywhere, \"x\") }\n\
                  }\n\
                  fn outer(counter: SharedMut[i32]) { tally(counter) }";
    let answers = sync_of(source, &with_access("sync = \"from(f)\"\n"), true);
    assert_eq!(answers["tally"], Sync::No);
    assert_eq!(answers["outer"], Sync::No);
}

/// The other answer, and the one that would cost the language Chapter 12.
///
/// If a blocking acquisition is classified as a pause — no `sync` line, the
/// way `fs::write` carries none — then `access` is not `sync`, so a function
/// that uses a lock cannot appear in a `par_iter` body, in `access` itself, in
/// a scope's tasks or in the panic hook. Part II 12.2's own idiom stops
/// compiling, and the greatest fixpoint (ADR-027 D1) carries the loss up every
/// caller.
///
/// That is the load-bearing question the floor asks, and this test is what
/// makes the consequence a run rather than a claim.
#[test]
fn a_pausing_acquisition_costs_the_idiom_and_everything_above_it() {
    let answers = sync_of(IDIOM, &with_access(""), true);
    assert_eq!(answers["tally"], Sync::No);
    assert_eq!(
        answers["outer"],
        Sync::No,
        "the loss propagates: ADR-027 D1's fixpoint takes the claim from every caller"
    );
}

// --- and what `Locked[T]` is in the machine ---------------------------------

/// **At one user thread, every lock is the cheap shape**
/// ([ADR-057](../../../docs/specification/adr/adr-057.md) D2).
///
/// Nothing a user writes can cross a thread there, so the safe shape buys
/// nothing and costs 11.3 ns an acquisition — which is what that setting exists
/// to save. It also keeps the one diagnostic reachable at this setting: the
/// cheap shape says so when a program re-enters one lock, and a bare `Mutex`
/// hangs for ever without saying anything.
#[test]
fn at_one_user_thread_a_lock_is_the_cheap_shape() {
    let rust = lowered(
        "fn main() {\n    let n: i64 = 1\n    let c = SharedMut(n)\n}",
        false,
    );
    // The constructor is what writes the shape now
    // ([ADR-064](../../../docs/specification/adr/adr-064.md) D2), so the shape is
    // read off the `::new` rather than off an annotation the line no longer needs.
    assert!(
        rust.contains("nikaia_std::lock::Local::new"),
        "the cheap shape:\n{rust}"
    );
    assert!(
        !rust.contains("lock::Crossing"),
        "and not the other one:\n{rust}"
    );
}

/// **At several, it is decided per value by the analysis that decides the
/// count** ([ADR-057](../../../docs/specification/adr/adr-057.md) D3).
///
/// A lock is only reachable from two places through a shared handle, so the
/// count that handle was given is the answer for the lock inside it. Here one
/// value is used by a task and the other is not, and the two come out different
/// in one program — which is the whole of what "per value" means.
#[test]
fn at_several_threads_a_lock_follows_the_value() {
    let rust = lowered(
        "fn main() {\n\
        \x20   let n: i64 = 1\n\
        \x20   let crossing: SharedMut[i64] = n\n\
        \x20   let t = spawn fn { crossing.access fn { n } }\n\
         }",
        true,
    );
    assert!(
        rust.contains("std::sync::Arc<nikaia_std::lock::Crossing<i64>>"),
        "a value a task uses takes the shape that can cross:\n{rust}"
    );
}

/// And **one call writes both hulls**
/// ([ADR-064](../../../docs/specification/adr/adr-064.md) D2): `SharedMut(n)` is
/// one name above and a count around a lock below, so the two cannot be written
/// out of step with each other.
///
/// At this setting both are the cheap shape, and for one reason rather than two:
/// nothing a user writes can cross a thread here
/// ([ADR-061](../../../docs/specification/adr/adr-061.md) D2).
#[test]
fn one_call_allocates_the_lock_as_well_as_the_handle() {
    let rust = lowered(
        "fn main() {\n    let n: i64 = 1\n    let c = SharedMut(n)\n}",
        false,
    );
    assert!(
        rust.contains("::new(nikaia_std::lock::Local::new(n))"),
        "one line, two hulls:\n{rust}"
    );
}

fn lowered(source: &str, parallel: bool) -> String {
    let parsed = parse_to_ast(source).expect("the fixture parses");
    let build = match parallel {
        true => nikaia::emit::Build::parallel(),
        false => nikaia::emit::Build::default(),
    };
    nikaia::emit::emit_program(&parsed, build)
        .expect("the fixture lowers")
        .rust
}
