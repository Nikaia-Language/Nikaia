//! What leaves a lock is stamped `Seen[T]`
//! ([ADR-111](../../../docs/specification/adr/adr-111.md)).
//!
//! `kasse.get()` is a `Seen[i64]`. The stamp sticks through arithmetic, through
//! a comparison, and through a call the callee's `touches` says reaches no
//! lock — so a value a lock handed out **carries where it came from**, all the
//! way to whatever tries to put it back.
//!
//! **It is a type here and not one below** (D1): the emitter erases it, so a
//! `Seen[i64]` is an `i64`, a field declared `Seen[i64]` is an `i64` field, and
//! a signature with `Seen` in it is one without. No counter, no marker, no
//! check at run time, no bytes.
//!
//! **What it buys is that no analysis follows the value.** The old `NK2205`
//! asked whether the argument contained a `get` on the same container, which
//! caught one line and nothing else. The stamp answers the same question
//! whether the read was on the line above, in another function, or in another
//! request — and it answers it with a type.

mod common;

use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

fn coded(source: &str, code: &str) -> usize {
    findings(source).iter().filter(|f| f.code == code).count()
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// **The stamp is erased below** (D1), which is what makes it free: a program
/// that reads a lock and prints the value is the same bytes it always was.
#[test]
fn the_stamp_costs_nothing_in_the_generated_file() {
    let rust = lowered(
        "fn main() {\n\
         \x20   let kasse = SharedMut(0)\n\
         \x20   let stand = kasse.get()\n\
         \x20   println(f\"{stand + 1}\")\n\
         }\n",
    );
    assert!(!rust.contains("Seen"), "{rust}");
    assert!(rust.contains("let stand = kasse.get();"), "{rust}");
}

/// **And a `Seen` in a declaration is erased too** (D1, D3): a struct field
/// that keeps a value read from a lock is where the type is **written**, and
/// below it is the field it always was.
#[test]
fn a_field_declared_seen_is_an_ordinary_field() {
    let rust = lowered(
        "struct Snapshot { at: Seen[i64] }\n\
         fn main() {\n\
         \x20   let kasse = SharedMut(0)\n\
         \x20   let s = Snapshot { at: kasse.get() }\n\
         \x20   println(f\"{s.at}\")\n\
         }\n",
    );
    assert!(!rust.contains("Seen"), "{rust}");
    assert!(rust.contains("at: i64"), "{rust}");
}

/// **A stamp reaches the world without a word written for it** (D2): every
/// sink a program has takes one, because the fit passes through.
#[test]
fn a_stamped_value_reaches_every_sink() {
    let clean = "fn width(n: i64) -> i64 sync { return n * 2 }\n\
                 fn main() {\n\
                 \x20   let kasse = SharedMut(0)\n\
                 \x20   let stand = kasse.get()\n\
                 \x20   println(f\"{stand}\")\n\
                 \x20   let n = width(stand)\n\
                 \x20   println(f\"{n} {stand > 100}\")\n\
                 }\n";
    assert!(findings(clean).is_empty(), "{:#?}", findings(clean));
}

/// **`NK2205`, first shape: a `set` given a stamped value** (D4) — and the
/// whole point is that the distance does not matter.
#[test]
fn a_set_given_a_stamped_value_is_refused() {
    // On the line above.
    assert_eq!(
        coded(
            "fn main() {\n\
             \x20   let kasse = SharedMut(0)\n\
             \x20   let stand = kasse.get()\n\
             \x20   kasse.set(stand + 100)\n\
             }\n",
            "NK2205"
        ),
        1
    );

    // Through a function, which the old rule could not see at all.
    assert_eq!(
        coded(
            "fn bumped(n: i64) -> i64 sync { return n + 1 }\n\
             fn main() {\n\
             \x20   let kasse = SharedMut(0)\n\
             \x20   let stand = kasse.get()\n\
             \x20   kasse.set(bumped(stand))\n\
             }\n",
            "NK2205"
        ),
        1,
        "a call whose argument is stamped hands back a stamped value (D2)"
    );
}

/// **`NK2205`, second shape: a `set` under a stamped condition** (D4). The
/// value stored is plain; the **decision** is the stale thing.
#[test]
fn a_set_under_a_stamped_condition_is_refused() {
    assert_eq!(
        coded(
            "fn main() {\n\
             \x20   let kasse = SharedMut(0)\n\
             \x20   let stand = kasse.get()\n\
             \x20   if stand > 100 { kasse.set(0) }\n\
             }\n",
            "NK2205"
        ),
        1
    );

    // A `match` is a condition alike.
    assert_eq!(
        coded(
            "fn main() {\n\
             \x20   let kasse = SharedMut(0)\n\
             \x20   let stand = kasse.get()\n\
             \x20   match stand {\n\
             \x20       0 => kasse.set(1),\n\
             \x20       else => kasse.set(2),\n\
             \x20   }\n\
             }\n",
            "NK2205"
        ),
        2
    );
}

/// **And a plain decision is not one**, which is the half that says the rule
/// reads the condition rather than the `if`.
#[test]
fn a_plain_condition_leaves_a_set_alone() {
    assert_eq!(
        coded(
            "fn main() {\n\
             \x20   let kasse = SharedMut(0)\n\
             \x20   let n = 7\n\
             \x20   if n > 3 { kasse.set(0) }\n\
             }\n",
            "NK2205"
        ),
        0
    );
}

/// **`NK2207`: an `update` block that replaces its `mut v` without reading it**
/// (D4) — a `set` through the back door.
#[test]
fn an_update_that_only_stores_is_refused() {
    assert_eq!(
        coded(
            "fn main() {\n\
             \x20   let kasse = SharedMut(0)\n\
             \x20   let n = 42\n\
             \x20   kasse.update fn(mut v) { v = n }\n\
             }\n",
            "NK2207"
        ),
        1
    );
}

/// **And a block that reads `v` is the door working**, in each of the three
/// shapes D4 names.
#[test]
fn an_update_that_reads_the_old_value_is_the_door() {
    for block in [
        "kasse.update fn(mut v) { v += 1 }",
        "kasse.update fn(mut v) { if v > 100 { v = 0 } }",
        "kasse.update fn(mut v) { v = v + 1 }",
    ] {
        let source = format!("fn main() {{\n    let kasse = SharedMut(0)\n    {block}\n}}\n");
        assert_eq!(
            coded(&source, "NK2207"),
            0,
            "`{block}` decides inside the lock"
        );
    }
}

/// **`set` is still for what it is for** (D4): a starting value, a
/// configuration that arrived from outside, a reset an operator asked for.
#[test]
fn an_unstamped_set_is_a_program() {
    assert!(
        findings(
            "fn main() {\n\
         \x20   let kasse = SharedMut(0)\n\
         \x20   kasse.set(42)\n\
         \x20   let n = 7\n\
         \x20   kasse.set(n * 6)\n\
         }\n"
        )
        .is_empty()
    );
}

/// **`access` hands back a stamp too** (D1): what the block computed came out
/// of the lock, and `Seen[?]` is the honest pair — the type is unknown
/// ([ADR-029](../../../docs/specification/adr/adr-029.md) D1) and where it came
/// from is not.
#[test]
fn what_access_hands_back_is_stamped() {
    assert_eq!(
        coded(
            "fn main() {\n\
             \x20   let kasse = SharedMut(0)\n\
             \x20   kasse.set(kasse.access fn(v) { v + 1 })\n\
             }\n",
            "NK2205"
        ),
        1
    );
}

/// **A bare `Seen` is the program's own type**: the stamp always says what it
/// stamps, and erasing a `struct Seen` to itself recursed until the compiler's
/// stack ran out - on a declaration and one parameter of it.
#[test]
fn a_type_the_program_calls_seen_is_its_own() {
    let source = "struct Seen {\n    at: i64,\n}\n\n\
                  fn place(s: Seen) -> i64 {\n    return s.at\n}\n\n\
                  fn main() {\n    println(f\"{place(Seen { at: 4 })}\")\n}\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("fn place(s: &Seen) -> i64"), "{rust}");
}
