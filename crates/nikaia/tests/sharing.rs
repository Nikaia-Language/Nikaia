//! Which reference count a particular `Shared` value gets
//! ([ADR-037](../../../docs/specification/adr/adr-037.md) D7).
//!
//! **One rule, and every test here is about it:** *it may only ever take an
//! atomic away.* D6 makes the atomic count the floor, and this analysis is an
//! optimisation on top of it - it may lower a value to a plain count where it
//! **proves** nothing crosses a thread with it, and where it cannot prove that
//! the answer stays atomic. So the tests come in two kinds, and the second kind
//! is the one that matters: the base case, without which an analysis that
//! answered `atomic` for everything would pass every other test in the file;
//! and the fail-closed cases, where being wrong is a data race rather than a
//! missed 9 ns.
//!
//! [`docs/history/rc-or-arc.md`](../../../docs/history/rc-or-arc.md) is the experiment this grew
//! out of: what the atomic costs, and the fail-open bug the prototype had - a
//! `<struct>.<field>` slot guarded on "already known", which made the answer
//! depend on the order somebody wrote their functions in. That test is here, and
//! so are its siblings.
//!
//! **Every source here parses and is analysed for real, and nothing is
//! emitted.** `Shared` has no constructor and must not get one (ADR-040 §3), so
//! a `Shared` arrives as a *declared type* - which is how `tests/send.rs` reaches
//! the same type, and since Part I 6.2's annotated `let` is built it is also how
//! a program makes one. What the lowering does with the answers is
//! `tests/shared.rs`, which compiles and runs.

use nikaia::contracts::sharing::{self, Count, Fallback};
use nikaia::contracts::{Ledger, LedgerOps, STD, TypeContract};
use nikaia::parser::parse_to_ast;

/// A parsed program with the two ledgers every analysis here needs.
fn ledgers(source: &str) -> (nikaia::parser::Parsed, Ledger, Ledger) {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    (parsed, own, library)
}

fn decisions(source: &str) -> Vec<sharing::Decision> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    sharing::analyse(&parsed, &own, &library)
}

/// One value's answer, by the function it is written in and the name the source
/// gives it.
///
/// Two functions may name a handle the same thing - a caller's `db` and the
/// parameter it is handed to - and then the pair is what picks one out.
fn one_in(source: &str, function: &str, value: &str) -> sharing::Decision {
    let found = decisions(source);
    found
        .iter()
        .find(|d| d.function == function && d.value == value)
        .unwrap_or_else(|| panic!("no decision for `{function}::{value}` in {found:#?}"))
        .clone()
}

/// One value's answer, by the name the source gives it.
fn one(source: &str, value: &str) -> sharing::Decision {
    let found = decisions(source);
    found
        .iter()
        .find(|d| d.value == value)
        .unwrap_or_else(|| panic!("no decision for `{value}` in {found:#?}"))
        .clone()
}

// --- the base case, which is the only reason the rest is worth anything ------

/// A `Shared` that nothing crosses with gets a **plain** count.
///
/// Without this the analysis could answer `atomic` for everything and pass every
/// other test here, which is the failure mode a fail-closed analysis has.
#[test]
fn a_shared_that_never_crosses_gets_a_plain_count() {
    let decision = one(
        "fn zaehle(counts: Shared[Vec[i64]]) -> i64 {\n\
             let n = counts.len()\n\
             n\n\
         }",
        "counts",
    );
    assert_eq!(decision.count, Count::Plain, "{decision:#?}");
    assert!(decision.why.is_none(), "{decision:#?}");
}

/// Two names for one `Shared` are one allocation and therefore one count.
///
/// The count belongs to the allocation and not to the handle, which is why the
/// analysis is union-find over handles rather than a walk down a lattice - and
/// why one crossing anywhere in a class decides the whole class.
#[test]
fn two_handles_on_one_allocation_share_one_answer() {
    let found = decisions(
        "fn zaehle(counts: Shared[Vec[i64]]) {\n\
             let also = counts\n\
             spawn fn { println(f\"{also.len()}\") }\n\
         }",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
    for decision in &found {
        assert_eq!(decision.count, Count::Atomic, "{decision:#?}");
    }
}

// --- the crossings ----------------------------------------------------------

/// A `spawn` body that uses it forces the atomic count (`NK2501`'s crossing).
#[test]
fn a_spawn_forces_the_atomic_count() {
    let decision = one(
        "fn zaehle(counts: Shared[Vec[i64]]) {\n\
             spawn fn { println(f\"{counts.len()}\") }\n\
         }",
        "counts",
    );
    assert_eq!(decision.count, Count::Atomic);
    assert!(
        !decision.undecided(),
        "a `spawn` is a crossing, not a mystery"
    );
    assert!(
        decision.why.as_deref().unwrap().contains("spawn"),
        "{decision:#?}"
    );
}

/// A lambda handed to a parallel method forces it too.
#[test]
fn a_parallel_lambda_forces_the_atomic_count() {
    let decision = one(
        "fn zaehle(counts: Shared[Vec[i64]], rows: Vec[i64]) -> i64 {\n\
             rows.par_iter(fn(r) { counts.len() + r })\n\
         }",
        "counts",
    );
    assert_eq!(decision.count, Count::Atomic);
    assert!(!decision.undecided(), "{decision:#?}");
}

/// **Hard case three.** A call whose body this compiler cannot see comes out
/// atomic, which is the polarity the experiment was set to confirm.
///
/// `NK2502` refuses this crossing. Here it costs an atomic instead, because the
/// cost of being wrong in the safe direction is speed and not a rejected
/// program - and that is the one way this analysis has an easier job than the
/// check beside it.
#[test]
fn a_call_this_compiler_cannot_see_the_end_of_comes_out_atomic() {
    let decision = one(
        "fn zaehle(counts: Shared[Vec[i64]]) {\n\
             hyper_shim::across_a_thread(counts)\n\
         }",
        "counts",
    );
    assert_eq!(decision.count, Count::Atomic);
    assert!(decision.undecided(), "nothing decided it: {decision:#?}");
    assert!(
        decision
            .why
            .as_deref()
            .unwrap()
            .contains("hyper_shim::across_a_thread"),
        "{decision:#?}"
    );
}

/// **Hard case one.** A `Shared` in a published signature comes out atomic, and
/// comes out atomic as *undecided*.
///
/// A public function's callers are in a unit this build never sees. The
/// representation is in the artifact by the time one of them crosses, so nothing
/// here can answer - and ADR-037 D8 is why a ledger column would
/// not answer either.
#[test]
fn a_shared_in_a_published_signature_comes_out_atomic() {
    let decision = one(
        "pub fn zaehle(counts: Shared[Vec[i64]]) -> i64 {\n\
             counts.len()\n\
         }",
        "counts",
    );
    assert_eq!(decision.count, Count::Atomic);
    assert!(decision.undecided(), "{decision:#?}");
    assert!(
        decision.why.as_deref().unwrap().contains("public"),
        "{decision:#?}"
    );
}

/// A call *within* the unit decides nothing by itself: the two handles are one
/// allocation, and the answer is whatever the pair of them reaches.
///
/// This is what the whole-program ledger buys. The callee is not public, so its
/// parameter is not a boundary, and a plain count survives a call.
#[test]
fn a_call_inside_the_unit_keeps_a_plain_count() {
    let found = decisions(
        "fn laenge(counts: Shared[Vec[i64]]) -> i64 { counts.len() }\n\
         fn zaehle(counts: Shared[Vec[i64]]) -> i64 { laenge(counts) }",
    );
    assert!(!found.is_empty(), "nothing was analysed");
    for decision in &found {
        assert_eq!(decision.count, Count::Plain, "{decision:#?}");
    }
}

/// …and a crossing on one side of that call reaches the other side, because it
/// is one allocation.
#[test]
fn a_crossing_in_a_callee_reaches_the_callers_handle() {
    let found = decisions(
        "fn laenge(counts: Shared[Vec[i64]]) -> i64 {\n\
             spawn fn { println(f\"{counts.len()}\") }\n\
             1\n\
         }\n\
         fn zaehle(counts: Shared[Vec[i64]]) -> i64 { laenge(counts) }",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
    for decision in &found {
        assert_eq!(decision.count, Count::Atomic, "{decision:#?}");
    }
}

/// **Hard case two.** A `Shared` in a struct field, reached by a value of the
/// struct that crosses.
///
/// `send::crossing` walks a struct's `fields` through the ledger (ADR-024, the
/// walk ADR-029 established) to refuse the struct for its field's sake. This is
/// the same walk the other way round: the struct crosses, so the field's count
/// has to be atomic - and the handle that was *put into* the field is the same
/// allocation, so it is atomic too.
#[test]
fn a_shared_in_a_struct_field_follows_the_struct_across() {
    let found = decisions(
        "struct Counter { hits: Shared[i64] }\n\
         fn zaehle(hits: Shared[i64]) {\n\
             let c = Counter { hits: hits }\n\
             spawn fn { println(f\"{c.hits}\") }\n\
         }",
    );
    let decision = found
        .iter()
        .find(|d| d.value == "hits")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert_eq!(decision.count, Count::Atomic, "{found:#?}");
}

/// The same struct, with nothing crossing, keeps the plain count.
#[test]
fn a_shared_in_a_struct_field_that_stays_put_keeps_a_plain_count() {
    let found = decisions(
        "struct Counter { hits: Shared[i64] }\n\
         fn zaehle(hits: Shared[i64]) -> i64 {\n\
             let c = Counter { hits: hits }\n\
             1\n\
         }",
    );
    let decision = found
        .iter()
        .find(|d| d.value == "hits")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert_eq!(decision.count, Count::Plain, "{found:#?}");
}

/// The struct's field is decided the same way whichever function the walk sees
/// first.
///
/// The crossing is in one function and the handle is put into the field in
/// another, and here the crossing is written **first**. An analysis that only
/// forced slots it had already met would answer `plain` for this program and
/// `atomic` for the same two functions in the other order - a fail-open bug, and
/// the one direction the polarity forbids. It is a real one: the prototype had it
/// (`docs/history/rc-or-arc.md` §8).
#[test]
fn a_struct_field_is_decided_whatever_order_the_functions_are_written_in() {
    for source in [
        "struct Counter { hits: Shared[i64] }\n\
         fn cross(c: Counter) { spawn fn { println(f\"{c.hits}\") } }\n\
         fn build(hits: Shared[i64]) -> Counter { Counter { hits: hits } }",
        "struct Counter { hits: Shared[i64] }\n\
         fn build(hits: Shared[i64]) -> Counter { Counter { hits: hits } }\n\
         fn cross(c: Counter) { spawn fn { println(f\"{c.hits}\") } }",
    ] {
        let found = decisions(source);
        let decision = found
            .iter()
            .find(|d| d.value == "hits")
            .unwrap_or_else(|| panic!("{found:#?}"));
        assert_eq!(decision.count, Count::Atomic, "{found:#?}");
    }
}

/// A struct whose fields are Rust - an entry with no `fields` - cannot be walked,
/// so a crossing of it says nothing about the `Shared` it may hold.
///
/// This is the gap ADR-037 D8 names: the transitivity is exactly as
/// good as the ledger's `fields`, and `fields` is empty for every type whose
/// parts are Rust. A value *put into* such a type by a Nikaia program is still
/// caught, because the putting is visible; one built inside the Rust half is not.
#[test]
fn a_type_whose_fields_are_rust_hides_what_it_holds() {
    let mut own = Ledger::empty();
    own.types.insert(
        "Opaque".to_string(),
        TypeContract {
            fields: Vec::new(),
            ..TypeContract::empty()
        },
    );
    let source = "fn zaehle(o: Opaque) {\n\
             spawn fn { println(f\"{o}\") }\n\
         }";
    let parsed = parse_to_ast(source).expect("the source parses");
    let library = Ledger::parse(STD).expect("std ships a ledger");
    // Nothing to decide, and nothing claimed: the analysis reports no value,
    // which is honest and is also the hole.
    assert!(
        sharing::analyse(&parsed, &own, &library).is_empty(),
        "a type with no recorded fields cannot be walked into"
    );
}

// --- the corpus -------------------------------------------------------------

/// The repository writes **one** `Shared`, and the prototype makes it atomic
/// because it cannot decide - which is the experiment's own finding turned into
/// a test.
///
/// `examples/fortunes.nika` line 73 takes `db: Shared[postgres::Connection]` and
/// hands it to `all.fetch(db)`. Nothing written down describes `fetch` - its body
/// is the driver's Rust - so ADR-038 D7's case applies and the polarity decides:
/// atomic. The file's own comment says the opposite about the build it is written
/// for ("At `user_parallelism = no` ... `Shared` costs a non-atomic refcount"),
/// and both are right: the per-build expansion gives it `Rc`, and a per-value
/// inference that may not fail open cannot. That gap is `docs/history/rc-or-arc.md` §6's
/// finding, and this is where it is asserted rather than asserted about.
#[test]
fn the_one_shared_in_the_repository_comes_out_atomic_undecided() {
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut checked = 0;
    let mut found = Vec::new();

    let mut pending = vec![root.join("examples"), root.join("benches")];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("nika") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read the program");
            let Ok(parsed) = parse_to_ast(&source) else {
                continue;
            };
            let own = Ledger::infer(&parsed);
            for decision in sharing::analyse(&parsed, &own, &library) {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                found.push((name, decision));
            }
            checked += 1;
        }
    }

    assert!(checked >= 12, "only {checked} programs were analysed");
    assert_eq!(
        found.len(),
        1,
        "the corpus writes exactly one `Shared`: {found:#?}"
    );
    let (file, decision) = &found[0];
    assert_eq!(file, "fortunes.nika");
    assert_eq!(decision.value, "db");
    assert_eq!(decision.count, Count::Atomic, "{decision:#?}");
    assert!(
        decision.undecided(),
        "and it is atomic because nothing decided it: {decision:#?}"
    );
}

/// `--sharing` says so rather than printing an empty table.
#[test]
fn the_report_says_when_there_is_nothing_to_choose() {
    let source = "fn main() { println(\"hello\") }";
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let report = sharing::report(&parsed, &own, &library, true);
    assert!(report.contains("nothing to choose"), "{report}");
}

/// The report names the crossing beside every atomic count, and counts the ones
/// nothing decided - the list ADR-010 D1 asks to be named.
#[test]
fn the_report_names_what_it_could_not_decide() {
    let source = "pub fn zaehle(counts: Shared[Vec[i64]]) -> i64 { counts.len() }\n\
                  fn lokal(hits: Shared[i64]) -> i64 { 1 }";
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let report = sharing::report(&parsed, &own, &library, true);
    assert!(report.contains("could not decide:"), "{report}");
    assert!(report.contains("1 plain, 1 atomic"), "{report}");
}

// --- fail-closed, and every sibling of the guard that failed open -----------

/// A `Shared` that came out of a **call** is atomic, because the count belongs
/// to the allocation and this analysis did not watch it being made.
///
/// This is the seed ADR-037 D6 made necessary rather than one the experiment
/// found. While a `Shared` was `MayNot`, the overlapping analysis never put one
/// on another thread; now `contracts::send` answers `May`, so an overlapped
/// operation may run on a thread of its own and hand a `Shared` **back** across
/// one (ADR-005 §5.2's `task::both` row). A handle whose origin is a call is
/// exactly that value.
#[test]
fn a_shared_that_came_out_of_a_call_is_atomic() {
    let decision = one(
        "fn zaehle() -> i64 {\n\
             let counts: Shared[i64] = beschaffe()\n\
             counts.len()\n\
         }",
        "counts",
    );
    assert_eq!(decision.count, Count::Atomic, "{decision:#?}");
    assert_eq!(
        decision.fallback,
        Some(Fallback::UnseenOrigin),
        "{decision:#?}"
    );
    assert!(
        decision.why.as_deref().unwrap().contains("beschaffe"),
        "and it names the call it came out of: {decision:#?}"
    );
}

/// A `Shared` **read back out of a struct field** is the field's allocation, not
/// a fresh one.
///
/// The sibling of `docs/history/rc-or-arc.md` §8's guard, in the other direction: the
/// prototype joined a handle *put into* a field to the field's slot and did not
/// join one *taken out*, so reading it back produced a class nothing forced and
/// a plain count on a value that crosses. Both orders of the two functions are
/// run, for the same reason the original test runs both.
#[test]
fn a_shared_read_out_of_a_field_is_the_fields_allocation() {
    for source in [
        "struct Counter { hits: Shared[i64] }\n\
         fn hole(c: Counter) -> i64 {\n\
             let h: Shared[i64] = c.hits\n\
             1\n\
         }\n\
         fn kreuze(c: Counter) { spawn fn { println(f\"{c.hits}\") } }",
        "struct Counter { hits: Shared[i64] }\n\
         fn kreuze(c: Counter) { spawn fn { println(f\"{c.hits}\") } }\n\
         fn hole(c: Counter) -> i64 {\n\
             let h: Shared[i64] = c.hits\n\
             1\n\
         }",
    ] {
        let decision = one(source, "h");
        assert_eq!(decision.count, Count::Atomic, "{decision:#?}");
        // …and the reason is the crossing rather than the ignorance, because a
        // reader can act on the first (ADR-033 D9's rule).
        assert!(!decision.undecided(), "{decision:#?}");
        assert!(
            decision.why.as_deref().unwrap().contains("spawn"),
            "{decision:#?}"
        );
    }
}

/// …and the same field, with nothing crossing, still lowers - which is what
/// stops the test above from passing against an analysis that answers `atomic`.
#[test]
fn a_field_read_with_nothing_crossing_still_lowers() {
    let decision = one(
        "struct Counter { hits: Shared[i64] }\n\
         fn hole(c: Counter) -> i64 {\n\
             let h: Shared[i64] = c.hits\n\
             1\n\
         }",
        "h",
    );
    assert_eq!(decision.count, Count::Plain, "{decision:#?}");
}

/// A handle **captured by a lambda** handed to a call nothing describes is
/// atomic.
///
/// The prototype forced only a bare `Expr::Variable` argument, so a handle
/// inside a closure, a tuple or any other expression went unaccounted for. A
/// callee nothing describes may cross with whatever it is given, and what it is
/// given includes the lambda's captures - so the walk over an argument is
/// `send::names_used`, the same over-approximate one `spawn` uses.
#[test]
fn a_handle_captured_by_a_lambda_handed_to_an_unseen_call_is_atomic() {
    let decision = one(
        "fn zaehle(counts: Shared[Vec[i64]]) {\n\
             fremd::spaeter(fn() { counts.len() })\n\
         }",
        "counts",
    );
    assert_eq!(decision.count, Count::Atomic, "{decision:#?}");
    assert_eq!(
        decision.fallback,
        Some(Fallback::UnseenCall),
        "{decision:#?}"
    );
}

/// …and one inside a **tuple** handed to the same call, which is the same hole
/// with a different expression in it.
#[test]
fn a_handle_inside_a_tuple_handed_to_an_unseen_call_is_atomic() {
    let decision = one(
        "fn zaehle(counts: Shared[Vec[i64]]) {\n\
             fremd::nimm((1, counts))\n\
         }",
        "counts",
    );
    assert_eq!(decision.count, Count::Atomic, "{decision:#?}");
    assert!(decision.undecided(), "{decision:#?}");
}

/// A handle handed over as an **option** - after the `;` - is atomic, because no
/// contract says what happens to a `Shared` in one.
#[test]
fn a_handle_handed_over_as_an_option_is_atomic() {
    let decision = one(
        "fn zaehle(counts: Shared[Vec[i64]]) {\n\
             fremd::lauf(1; mit: counts)\n\
         }",
        "counts",
    );
    assert_eq!(decision.count, Count::Atomic, "{decision:#?}");
    assert_eq!(
        decision.fallback,
        Some(Fallback::UncoveredArgument),
        "{decision:#?}"
    );
}

/// A `Shared` **assigned into** a struct's field joins that field's allocation.
#[test]
fn a_shared_assigned_into_a_field_joins_the_fields_allocation() {
    for source in [
        "struct Counter { hits: Shared[i64] }\n\
         fn setze(c: Counter, h: Shared[i64]) { c.hits = h }\n\
         fn kreuze(c: Counter) { spawn fn { println(f\"{c.hits}\") } }",
        "struct Counter { hits: Shared[i64] }\n\
         fn kreuze(c: Counter) { spawn fn { println(f\"{c.hits}\") } }\n\
         fn setze(c: Counter, h: Shared[i64]) { c.hits = h }",
    ] {
        let decision = one(source, "h");
        assert_eq!(decision.count, Count::Atomic, "{decision:#?}");
    }
}

/// A `Shared` held by a **public field of a public type** is atomic, which is
/// the library boundary one level in from a signature.
///
/// A published field is a place code this build never reads can take the value
/// out of, so nothing here can say that none of those places crosses. The
/// private counterpart is what stops this from being "atomic always".
#[test]
fn a_public_field_of_a_public_type_is_atomic_and_a_private_one_is_not() {
    let published = one(
        "pub struct Counter { pub hits: Shared[i64] }\n\
         fn zaehle(hits: Shared[i64]) -> i64 {\n\
             let c = Counter { hits: hits }\n\
             1\n\
         }",
        "hits",
    );
    assert_eq!(published.count, Count::Atomic, "{published:#?}");
    assert_eq!(
        published.fallback,
        Some(Fallback::PublicField),
        "{published:#?}"
    );

    let private = one(
        "pub struct Counter { hits: Shared[i64] }\n\
         fn zaehle(hits: Shared[i64]) -> i64 {\n\
             let c = Counter { hits: hits }\n\
             1\n\
         }",
        "hits",
    );
    assert_eq!(private.count, Count::Plain, "{private:#?}");
}

/// A **public function whose parameter holds** a `Shared` exposes the field, not
/// only a parameter that *is* one.
///
/// `pub fn f(c: Counter)` hands a whole `Counter` to callers this build does not
/// read, and the `Shared` in its field goes with it. The prototype forced only
/// parameters that held a `Shared` directly, which left this one plain.
#[test]
fn a_public_parameter_that_holds_a_shared_exposes_the_field() {
    let found = decisions(
        "struct Counter { hits: Shared[i64] }\n\
         pub fn zeige(c: Counter) -> i64 { 1 }\n\
         fn baue(hits: Shared[i64]) -> Counter { Counter { hits: hits } }",
    );
    let decision = found
        .iter()
        .find(|d| d.value == "hits")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert_eq!(decision.count, Count::Atomic, "{found:#?}");
    assert_eq!(
        decision.fallback,
        Some(Fallback::PublicSignature),
        "{found:#?}"
    );
}

// --- the ledger column -------------------------------------------------------

/// The summary is a ledger column, beside `sync`, `throws`, `touches` and
/// `borrows`.
///
/// ADR-037 D7 names the shape: which parameters and result are one
/// class, and which count that class gets. The union-find that decides a count
/// computes it already, which is what makes this a column and not a mechanism
/// (ADR-020 D1).
#[test]
fn the_summary_is_a_ledger_column() {
    let (_, own, _) = ledgers(
        "fn weiter(counts: Shared[Vec[i64]]) -> Shared[Vec[i64]] { counts }\n\
         fn kreuze(hits: Shared[i64]) { spawn fn { println(f\"{hits}\") } }\n\
         fn schlicht(x: i64) -> i64 { x }",
    );

    // A parameter and the result it is handed back as are **one class**, and
    // that is the half a caller could not work out for itself.
    let weiter = &own.functions["weiter"].sharing;
    assert_eq!(weiter.len(), 1, "{weiter:#?}");
    assert_eq!(weiter[0].members, ["<result>", "counts"], "{weiter:#?}");
    assert_eq!(weiter[0].count, Count::Plain, "{weiter:#?}");

    let kreuze = &own.functions["kreuze"].sharing;
    assert_eq!(kreuze.len(), 1, "{kreuze:#?}");
    assert_eq!(kreuze[0].members, ["hits"], "{kreuze:#?}");
    assert_eq!(kreuze[0].count, Count::Atomic, "{kreuze:#?}");

    // ADR-020 D4: only what is true is written, and a function with no `Shared`
    // position has nothing to say.
    assert!(own.functions["schlicht"].sharing.is_empty());
}

/// …and it survives a round trip through the file, byte for byte.
///
/// Part III 13.5 makes the ledger a pure function of (source, toolchain) and
/// `--locked` compares bytes, so the column has to render and parse back to the
/// same thing and the classes have to be in a determined order.
#[test]
fn the_column_round_trips_through_the_file() {
    let (_, own, _) = ledgers(
        "fn weiter(counts: Shared[Vec[i64]]) -> Shared[Vec[i64]] { counts }\n\
         fn kreuze(hits: Shared[i64]) { spawn fn { println(f\"{hits}\") } }",
    );
    let rendered = own.render();
    assert!(
        rendered.contains("sharing = [\"<result> | counts: plain\"]"),
        "{rendered}"
    );
    assert!(
        rendered.contains("sharing = [\"hits: atomic\"]"),
        "{rendered}"
    );

    let read = Ledger::parse(&rendered).expect("a ledger this compiler wrote parses");
    assert_eq!(
        read.functions["weiter"].sharing,
        own.functions["weiter"].sharing
    );
    assert_eq!(
        read.functions["kreuze"].sharing,
        own.functions["kreuze"].sharing
    );
    assert_eq!(
        read.render(),
        rendered,
        "and rendering it again is the same bytes"
    );
}

/// A `sharing` line this compiler could not have written is refused rather than
/// read as something else.
#[test]
fn a_sharing_line_that_says_nothing_meaningful_is_refused() {
    for line in [
        "sharing = [\"counts\"]",
        "sharing = [\"counts: maybe\"]",
        "sharing = [\": plain\"]",
    ] {
        let text = format!(
            "version = 2\ntoolchain = \"probe\"\ninference = \"probe\"\n\n[fn.\"f\"]\n{line}\n"
        );
        assert!(Ledger::parse(&text).is_err(), "{line}");
    }
}

// --- the enumeration ---------------------------------------------------------

/// Every fallback is enumerable, enumerated by `--sharing`, and answered by
/// ADR-037 D8.
///
/// The closed list *is* the cost of the design, so the report prints all of it
/// and marks the rows this program hit. A row added to `Fallback` without a
/// remedy is a row D8 has not answered.
#[test]
fn every_fallback_is_enumerated_with_a_remedy() {
    assert_eq!(
        sharing::every_fallback().len(),
        7,
        "{:?}",
        sharing::every_fallback()
    );
    for fallback in sharing::every_fallback() {
        assert!(!fallback.as_str().is_empty(), "{fallback:?}");
        assert!(!fallback.remedy().is_empty(), "{fallback:?}");
    }
    // The two that want nothing, because the answer belongs to code this run
    // does not read: callers that do not exist yet, and the file that declares
    // the field.
    for fallback in [Fallback::PublicSignature, Fallback::ForeignFile] {
        assert!(
            fallback.remedy().contains("nothing here"),
            "{}",
            fallback.remedy()
        );
    }

    let (parsed, own, library) = ledgers(
        "pub fn zaehle(counts: Shared[Vec[i64]]) -> i64 { counts.len() }\n\
         fn lokal(hits: Shared[i64]) -> i64 { 1 }",
    );
    let report = sharing::report(&parsed, &own, &library, true);
    for fallback in sharing::every_fallback() {
        assert!(
            report.contains(&fallback.as_str()),
            "{fallback:?} in:\n{report}"
        );
    }
    // The one this program hit is marked, and the ones it did not are not.
    assert!(
        report.contains(&format!("* {}", Fallback::PublicSignature.as_str())),
        "{report}"
    );
    assert!(
        report.contains("An atomic count is the safe default. A plain count is an optimisation"),
        "and the report says which way the polarity runs: {report}"
    );
}

/// `--sharing` has `--overlaps`' shape: a heading per function, one indented
/// line per value, and the reason under it.
#[test]
fn the_report_has_the_shape_the_other_explanations_have() {
    let (parsed, own, library) =
        ledgers("fn kreuze(hits: Shared[i64]) { spawn fn { println(f\"{hits}\") } }");
    let report = sharing::report(&parsed, &own, &library, true);
    assert!(report.starts_with("kreuze:\n"), "{report}");
    assert!(
        report.contains("    atomic  `hits` (Shared[i64])"),
        "{report}"
    );
    assert!(
        report.contains("crosses: a `spawn` body uses it"),
        "{report}"
    );
}

/// No `std` entry can **keep** a handle on a `Shared`, and the day one could
/// somebody has to look at it.
///
/// The analysis treats a call the ledger describes as **accounted for**, which
/// is what ADR-005 §5.2 already does for the same question - a contract is an
/// account, and `std`'s are reviewed like code. What that rests on is that a
/// described callee does not quietly stash a handle and cross with it, which is
/// ADR-005 §5.3's "enforceable only as far as a foreign crate is honest".
///
/// **`Shared::deref` is the one entry that mentions the type, and it is the
/// shape this guard is not about.** It takes `&Shared[$T]` and hands back `&$T`:
/// a borrow duplicates nothing (ADR-040 D1's correction), and what comes out is a
/// view of the value inside rather than a handle on it - so there is no handle for
/// the entry to keep and nothing for it to cross with. It is there because
/// ADR-042 D2 sees a transparent container through to what it holds, which is
/// what makes `serve(&db)` mean something. So the guard asks the question it was
/// always for rather than the one its first spelling asked: a position that takes
/// or hands back a **handle** is the decision, and a borrow of one is not.
#[test]
fn a_std_entry_that_takes_a_shared_needs_a_second_look() {
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let mut carries: Vec<String> = Vec::new();
    for (name, contract) in &library.functions {
        let Some(signature) = contract.signature.as_ref() else {
            continue;
        };
        let positions = signature
            .params
            .iter()
            .map(|(_, ty)| ty.clone())
            .chain(signature.result.clone());
        for ty in positions {
            if carries_a_handle(&ty) {
                carries.push(format!("{name}{}", signature.text()));
                break;
            }
        }
    }
    assert!(
        carries.is_empty(),
        "a `std` entry now takes or hands back a handle on a `Shared`: {carries:?}. \
         `contracts::sharing` treats a described callee as accounted for, so somebody has to \
         decide whether that entry may keep the handle and cross a thread with it - see the \
         module header of `contracts::sharing`. A `ref Shared[$T]` receiver is not this case: a \
         borrow duplicates nothing (ADR-040 D1)."
    );
}

/// Whether a position is a **handle** on a shared value rather than a borrow of
/// one.
///
/// `Shared[T]` is, and so is anything holding one - a `Vec[Shared[T]]` a callee
/// is lent can still have a handle cloned out of it. `&Shared[T]` is not, and it
/// is the only exemption: the view reaches the value inside and produces no
/// second owner.
fn carries_a_handle(ty: &nikaia::contracts::ty::Ty) -> bool {
    use nikaia::contracts::ty::Ty;
    match ty {
        Ty::Named { name, args, view } => {
            if name == "Shared" {
                return !*view || args.iter().any(carries_a_handle);
            }
            args.iter().any(carries_a_handle)
        }
        Ty::Tuple(parts) => parts.iter().any(carries_a_handle),
        Ty::Fn { params, .. } => params.iter().any(carries_a_handle),
        // A `Shared[T]?` holds a handle or nothing, and the first is what
        // matters here (Part I 2.3).
        Ty::Nullable(inner) => carries_a_handle(inner),
        // A sequence of handles is one a handle can be taken out of, exactly as
        // a `Vec` of them is ([ADR-105](../../../docs/specification/adr/adr-105.md)
        // D1): what it produces is what it holds, for this question.
        Ty::Seq { item, .. } => carries_a_handle(item),
        // A count is a number the compiler knows and nothing at run time, so
        // there is nothing in it to hold a handle
        // ([ADR-152](../../../docs/specification/adr/adr-152.md) D1).
        // What the C boundary lends lives for the call
        // ([ADR-147](../../../docs/specification/adr/adr-147.md) D1), so a
        // handle cannot be taken out of one and kept.
        Ty::Pointed { .. } => false,
        Ty::Unknown | Ty::Var { .. } | Ty::Count(_) => false,
    }
}

// --- where the second handle is made (ADR-040 D1 and D5) ---------------------

/// A handle handed to a function that **keeps** it is duplicated, and the place
/// is named.
///
/// [ADR-040](../../../docs/specification/adr/adr-040.md) D1 leaves the place one
/// step of the count is paid unwritten in the source - there is no method to
/// call - so D5 asks for it beside the count, which is what this analysis feeds
/// `--sharing`.
#[test]
fn a_handle_handed_on_by_value_is_a_duplication_site() {
    let decision = one_in(
        "fn behalte(db: Shared[i64]) { }\n\
         fn main() { let db = Shared(1)\n behalte(db) }",
        "main",
        "db",
    );
    assert_eq!(
        decision.duplications,
        vec!["handed to `behalte` as `db`, which takes a handle of its own".to_string()],
        "{decision:#?}"
    );
}

/// **A borrow duplicates nothing**, which is D1's own correction and the half
/// most easily got wrong: a `&Shared[T]` parameter hands no handle on.
#[test]
fn a_borrowed_handle_is_not_a_duplication_site() {
    let decision = one_in(
        "fn schau(db: ref Shared[i64]) { }\n\
         fn main() { let db = Shared(1)\n schau(ref db) }",
        "main",
        "db",
    );
    assert!(
        decision.duplications.is_empty(),
        "lending the inner value out touches no count: {decision:#?}"
    );
}

/// A task takes a handle of its own, so the name outside it stays usable
/// (D1's second half).
#[test]
fn a_task_that_uses_a_handle_is_a_duplication_site() {
    let decision = one(
        "fn main() { let db = Shared(1)\n spawn fn { println(f\"{db}\") } }",
        "db",
    );
    assert!(
        decision
            .duplications
            .iter()
            .any(|site| site.contains("`spawn` body")),
        "{decision:#?}"
    );
    // And it is still a crossing, which is a separate question from how many
    // handles there are: the duplication changes no verdict.
    assert_eq!(decision.count, Count::Atomic, "{decision:#?}");
}

/// Two handovers to the same callee are one site, because the report is read:
/// the same sentence twice says nothing the once did not.
#[test]
fn the_same_handover_is_named_once() {
    let decision = one_in(
        "fn behalte(db: Shared[i64]) { }\n\
         fn main() { let db = Shared(1)\n behalte(db)\n behalte(db) }",
        "main",
        "db",
    );
    assert_eq!(decision.duplications.len(), 1, "{decision:#?}");
}

/// A handle read out of a **field** and handed on by value joins the callee's
/// class, exactly as a named one does.
///
/// Without the join the field's class and the parameter's could disagree about
/// which count they are, and the emitted Rust would then hand an `Rc` where an
/// `Arc` is wanted - a `rustc` error about a file nobody wrote, which is what
/// Part III C.1 forbids.
#[test]
fn a_handle_taken_out_of_a_field_joins_what_it_is_handed_to() {
    let source = "struct Halter { db: Shared[i64] }\n\
         fn kreuze(db: Shared[i64]) { spawn fn { println(f\"{db}\") } }\n\
         fn main() { let h = Halter { db: 1 }\n kreuze(h.db) }";
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let sharing = sharing::analyse_program(&parsed, &own, &library, true);
    assert_eq!(
        sharing.count_of("<field>", "Halter.db"),
        Count::Atomic,
        "the crossing in `kreuze` has to reach the field it was handed out of: {:#?}",
        sharing.counts
    );
}
