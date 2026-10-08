//! Which functions **touch a lock**
//! ([ADR-281](../../../docs/specification/adr/adr-281.md) D29) — the second
//! derived property, propagated over the call graph `sync` uses and with the
//! opposite lattice.
//!
//! The column alone: inferred, recorded, and read by nothing. Every refusal
//! that entry lists — `NK2201`, `NK2203` — is still uncatalogued work, and the
//! point of landing this half on its own is that the answer can be **measured**
//! against the corpus before one of them reads it.
//!
//! **That measurement is why the column has three values and not two.** D3
//! says fail-closed, and it says it about `sync`, where the cost of doubt is a
//! caller writing `.await`. Taken literally here it gave the property to **16
//! of 59** functions in `examples/` — almost all of them `main`, and not one of
//! those programs opens a lock. A refusal reading that column would have
//! refused correct programs, which is the one thing
//! [Part III C.4](../../../docs/specification/30-nikaia-tooling.md) forbids. So
//! `Undecided` is its own answer, exactly as it is in
//! [`contracts::send`](../../src/contracts/send.rs): not permission, and not a
//! refusal either.

mod common;

use nikaia::contracts::{Ledger, LedgerOps, Lock};
use nikaia::parser::parse_to_ast;

/// What the ledger says one function's lock property is.
fn locks(source: &str, name: &str) -> Lock {
    let parsed = parse_to_ast(source).expect("the source parses");
    Ledger::infer(&parsed).functions[name].touches_a_lock
}

/// **A body that opens a door holds one**, which is the base case the whole
/// column is built out of.
#[test]
fn a_body_that_opens_a_door_holds_a_lock() {
    let source = "fn bump(counter: SharedMut[i64]) {\n\
                  \x20   counter.update fn(mut n) { n += 1 }\n\
                  }\n\
                  fn plain(n: i64) -> i64 sync { return n + 1 }\n";
    assert_eq!(locks(source, "bump"), Lock::Holds);
    assert_eq!(
        locks(source, "plain"),
        Lock::No,
        "arithmetic is not a lock, and a column that said otherwise would be useless"
    );
}

/// **And it travels up the call graph**, which is what makes this an answer
/// about a *program* rather than about one body — D2 needs it to catch chains
/// and self-calls rather than only what is written in one place.
#[test]
fn it_travels_up_the_call_graph() {
    let source = "fn bump(counter: SharedMut[i64]) {\n\
                  \x20   counter.update fn(mut n) { n += 1 }\n\
                  }\n\
                  fn twice(counter: SharedMut[i64]) {\n\
                  \x20   bump(counter)\n\
                  \x20   bump(counter)\n\
                  }\n\
                  fn apart(n: i64) -> i64 sync { return n * 2 }\n";
    assert_eq!(locks(source, "twice"), Lock::Holds);
    assert_eq!(
        locks(source, "apart"),
        Lock::No,
        "a caller that reaches no holder gets nothing, which is the half that \
         says the walk is not simply answering yes"
    );
}

/// **A least fixpoint, in the one shape that tells it from a greatest one.**
///
/// `sync` starts from *everyone is `sync`* and takes the claim away, so mutual
/// recursion keeps it. This starts from *nobody touches one* and adds, so
/// mutual recursion between two functions that open no door gets nothing. Both
/// are right, and they are right for opposite reasons
/// ([ADR-288](../../../docs/specification/adr/adr-288.md) D1).
#[test]
fn mutual_recursion_that_opens_no_door_holds_nothing() {
    let source = "fn ping(n: i64) -> i64 sync {\n\
                  \x20   if n == 0 { return 0 }\n\
                  \x20   return pong(n - 1)\n\
                  }\n\
                  fn pong(n: i64) -> i64 sync { return ping(n) }\n";
    assert_eq!(locks(source, "ping"), Lock::No);
    assert_eq!(locks(source, "pong"), Lock::No);
}

/// **A call nothing describes is `Undecided`, and `Undecided` is not `Holds`.**
///
/// This is the arm the corpus bought. It is not permission —
/// [ADR-010](../../../docs/specification/adr/adr-010.md) D1 says an analysis
/// that fails open is a vulnerability generator — and it is not a refusal
/// either, because Stage 0 knows the type of rather less than half of what a
/// program writes.
#[test]
fn a_call_nothing_describes_is_undecided() {
    let source = "struct Sink { n: i64 }\n\
                  fn hand(sink: Sink) { sink.swallow() }\n";
    assert_eq!(locks(source, "hand"), Lock::Undecided);
    assert!(
        !locks(source, "hand").holds(),
        "and `holds` is what a refusal asks, so doubt refuses nothing"
    );
}

/// **Doubt travels too, and is overtaken by certainty.** A caller that reaches
/// both an unresolvable call and a real door holds one: `Holds` is the worse
/// answer and the worse answer is what a caller takes.
#[test]
fn certainty_overtakes_doubt() {
    let source = "struct Sink { n: i64 }\n\
                  fn murky(sink: Sink) { sink.swallow() }\n\
                  fn sure(counter: SharedMut[i64]) { counter.update fn(mut n) { n += 1 } }\n\
                  fn both(sink: Sink, counter: SharedMut[i64]) {\n\
                  \x20   murky(sink)\n\
                  \x20   sure(counter)\n\
                  }\n\
                  fn only_doubt(sink: Sink) { murky(sink) }\n";
    assert_eq!(locks(source, "both"), Lock::Holds);
    assert_eq!(locks(source, "only_doubt"), Lock::Undecided);
}

/// **A `spawn`'s body does not count** (D3): it runs later and elsewhere, so
/// taking a lock in one is the ordinary case rather than a chain the caller is
/// answerable for.
#[test]
fn a_spawned_body_does_not_give_its_caller_the_property() {
    let source = "fn start(counter: SharedMut[i64]) {\n\
                  \x20   let h = spawn fn { counter.update fn(mut n) { n += 1 } }\n\
                  }\n";
    assert_eq!(
        locks(source, "start"),
        Lock::No,
        "a task started with `spawn` runs later and elsewhere"
    );
}

/// **A trailing lambda's body does count**, for the reason the `spawn` above
/// does not: it runs *during* the call
/// ([ADR-288](../../../docs/specification/adr/adr-288.md) D16), so what it does
/// is what this body does.
#[test]
fn a_lambda_that_runs_during_the_call_counts() {
    let source = "fn each(xs: Vec[i64], counter: SharedMut[i64]) {\n\
                  \x20   xs.sort_by_key fn(x) { counter.get() }\n\
                  }\n";
    assert_eq!(locks(source, "each"), Lock::Holds);
}

/// **A door over several locks is one too** — `access_all` and `update_all` are
/// free calls rather than methods
/// ([ADR-281](../../../docs/specification/adr/adr-281.md)), so they are found
/// where the free calls are and not among the resolved receivers.
#[test]
fn a_door_over_several_locks_counts() {
    let source = "fn both(a: SharedMut[i64], b: SharedMut[i64]) {\n\
                  \x20   update_all(a, b) fn(x, y) { (x + 1, y + 1) }\n\
                  }\n";
    assert_eq!(locks(source, "both"), Lock::Holds);
}

/// The column survives the round trip, which `--locked` needs: it compares
/// bytes, so a column that rendered differently than it parsed would fail a
/// build that changed nothing.
#[test]
fn the_column_renders_and_parses_back() {
    let parsed = parse_to_ast(
        "struct Sink { n: i64 }\n\
         pub fn bump(counter: SharedMut[i64]) { counter.update fn(mut n) { n += 1 } }\n\
         pub fn murky(sink: Sink) { sink.swallow() }\n\
         pub fn plain(n: i64) -> i64 sync { return n + 1 }\n",
    )
    .expect("the source parses");
    let ledger = Ledger::infer(&parsed);
    let rendered = ledger.render();
    assert!(rendered.contains("locks = true"), "{rendered}");
    assert!(rendered.contains("locks = \"?\""), "{rendered}");

    let read = Ledger::parse(&rendered).expect("its own output parses");
    assert_eq!(read.functions["bump"].touches_a_lock, Lock::Holds);
    assert_eq!(read.functions["murky"].touches_a_lock, Lock::Undecided);
    assert_eq!(read.functions["plain"].touches_a_lock, Lock::No);
    assert_eq!(read.render(), rendered);

    // And a function that touches none writes no line, so no existing ledger
    // grows a column of falses.
    let plain = parse_to_ast("pub fn width(n: i64) -> i64 sync { return n + 1 }")
        .expect("the source parses");
    assert!(!Ledger::infer(&plain).render().contains("locks"));
}

/// **A constructor and a variant are values being built, not doubts.**
///
/// `Stats(n)` is the anonymous constructor (Part I 4.2), recorded as
/// `Stats::new`; `String()` is `std`'s, recorded as `String::new`; and
/// `Shape::Circle(n)` runs no body at all. Read by the name as written, each was
/// *a callee nothing describes* and made its caller undecided - 11 of the 12
/// undecided functions in `examples/` at 0.0.244 were only that. The lock
/// column now resolves a call by the rule `sync` already used.
#[test]
fn a_constructor_or_a_variant_is_not_a_doubt() {
    let source = "enum Shape {\n\
                  \x20   Circle(i64),\n\
                  \x20   Square,\n\
                  }\n\
                  struct Stats { count: i64 }\n\
                  impl Stats {\n\
                  \x20   pub fn(first: i64) -> Stats { return Stats { count: first } }\n\
                  }\n\
                  fn stats(n: i64) -> Stats { return Stats(n) }\n\
                  fn shape(n: i64) -> Shape { return Shape::Circle(n) }\n\
                  fn text() -> String { return String() }\n";
    assert_eq!(
        locks(source, "stats"),
        Lock::No,
        "an own type's constructor"
    );
    assert_eq!(locks(source, "shape"), Lock::No, "a variant builds a value");
    assert_eq!(
        locks(source, "text"),
        Lock::No,
        "std's constructor, by its `::new` key"
    );
}

/// **What the corpus holds, and the number a refusal is read against.**
///
/// **18 hold, 7 are undecided, 34 are clear**, of 59 — and every one of the 18
/// is `println`. [ADR-288](../../../docs/specification/adr/adr-288.md) D21 is
/// where that was pinned down: a print never pauses, so `sync` says nothing
/// about it, and it takes standard output's **own lock** while yours is open.
/// Two conditions and not one, and this column answers the second.
///
/// **One example opens locks of its own**: `hello-http`'s supervised
/// counters (ADR-328), each door a single `update` or `get` whose block takes
/// no other lock, so `NK2203` refuses nothing there. Any other example that
/// writes `SharedMut` or `Locked` has to be read against the number above.
#[test]
fn what_the_corpus_holds_is_the_prints() {
    let mut held = Vec::new();
    for path in common::standalone_programs(std::path::Path::new("../../examples")) {
        if path.extension().is_none_or(|e| e != "nika") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read it");
        let Ok(parsed) = parse_to_ast(&text) else {
            continue;
        };
        for (name, contract) in &Ledger::infer(&parsed).functions {
            if contract.touches_a_lock.holds() {
                held.push(format!("{}:{name}", path.display()));
            }
        }
    }
    assert!(
        !held.is_empty(),
        "a print takes standard output's lock, so the `main` of every example \
         that prints holds one - an empty answer here means the column stopped \
         propagating"
    );
    // And none of them is a lock the program itself opened: nothing in
    // `examples/` writes `SharedMut` or `Locked`.
    for path in common::standalone_programs(std::path::Path::new("../../examples")) {
        if path.extension().is_none_or(|e| e != "nika") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read it");
        if path.ends_with("hello-http/src/main.nika") {
            continue;
        }
        assert!(
            !text.contains("SharedMut(") && !text.contains("Locked("),
            "{} opens a lock, so the measurement above has to be re-read",
            path.display()
        );
    }
}
