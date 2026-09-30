//! `set(neu; after: seen)` is the one door for a stamped value
//! ([ADR-111](../../../docs/specification/adr/adr-111.md) D5).
//!
//! D1 to D4 made a value a lock hands out a `Seen[T]` and refused every `set`
//! that takes one — which left a stamped value exactly one way through, the
//! `update` block, and no way at all to write the *optimistic* form: read,
//! compute outside the lock, store if nothing moved.
//!
//! This is that form, and it is a **definition** rather than a new rule:
//!
//! ```text
//! kasse.set(neu; after: stand)
//! // is
//! kasse.update fn(mut v) { if v == stand { v = neu } else { throw Overtaken } }
//! ```
//!
//! The witness is the value itself. *Store `neu` if the lock still holds what
//! I saw* — and if it does, every decision taken on what was seen still holds,
//! which is why the witness covers both shapes `NK2205` refuses: the stamped
//! value and the stamped condition.

mod common;

use std::process::Command;

use nikaia::contracts::{Ledger, LedgerOps, STD};
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

/// Compile the lowering and run it. The door is a `std` function and a
/// comparison in the language below, so nothing but running it says the two
/// halves agree.
fn ran(purpose: &str, source: &str) -> String {
    let rust = lowered(source);
    let dir = common::scratch_dir(&format!("witness-{purpose}"));
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "{purpose} did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary).output().expect("run it");
    assert!(
        out.status.success(),
        "{purpose} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    std::fs::remove_dir_all(&dir).ok();
    printed
}

/// **The stamped value goes through** (D5). The same two lines `NK2205`
/// refuses are an ordinary program once the witness is written.
#[test]
fn a_witness_lets_a_stamped_value_be_stored() {
    let source = "fn main() throws {\n\
                  \x20   let kasse = SharedMut(0)\n\
                  \x20   let stand = kasse.get()\n\
                  \x20   kasse.set(stand + 100; after: stand)\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    // …and without it, the very same program is the refusal D4 wrote.
    assert_eq!(
        coded(
            "fn main() {\n\
             \x20   let kasse = SharedMut(0)\n\
             \x20   let stand = kasse.get()\n\
             \x20   kasse.set(stand + 100)\n\
             }\n",
            "NK2205"
        ),
        1,
        "the relaxation is the witness's and nothing else's"
    );
}

/// **And so does the stamped decision** (D5): *if the value is still the one
/// seen, every decision taken on it still holds*. One witness, both shapes.
#[test]
fn a_witness_covers_the_condition_too() {
    let source = "fn main() throws {\n\
                  \x20   let kasse = SharedMut(0)\n\
                  \x20   let stand = kasse.get()\n\
                  \x20   if stand > 100 { kasse.set(0; after: stand) }\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
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
}

/// **It can fail, and nothing marks a failing call** (ADR-023 D8): the
/// function around it has to declare `throws`, which is `NK2605` and not a
/// rule of its own.
#[test]
fn the_door_can_fail_and_the_caller_has_to_say_so() {
    let refused = findings(
        "fn main() {\n\
         \x20   let kasse = SharedMut(0)\n\
         \x20   let stand = kasse.get()\n\
         \x20   kasse.set(stand + 1; after: stand)\n\
         }\n",
    );
    let about = refused
        .iter()
        .find(|f| f.code == "NK2605")
        .expect("the door carries `throws`");
    assert!(
        about.message.contains("SharedMut::set(after)"),
        "the message names the entry it read: {}",
        about.message
    );
}

/// **A `catch` is the other way to answer it**, and it wants the `Result`
/// rather than a `?` — which is the ordinary machinery and is here because a
/// door reached through a rewritten name is exactly where it could have been
/// lost.
#[test]
fn a_catch_takes_the_failure_instead() {
    let source = "fn main() {\n\
                  \x20   let kasse = SharedMut(0)\n\
                  \x20   let stand = kasse.get()\n\
                  \x20   kasse.set(stand + 1; after: stand) catch { }\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(
        rust.contains("match kasse.set_after(stand + 1, &stand)"),
        "{rust}"
    );
}

/// **One acquisition and not two**, which is the whole of what the door is
/// for: the compare and the store happen while the lock is open once.
#[test]
fn it_lowers_to_one_call_with_the_witness_as_an_argument() {
    let rust = lowered(
        "fn main() throws {\n\
         \x20   let kasse = SharedMut(0)\n\
         \x20   let stand = kasse.get()\n\
         \x20   kasse.set(stand + 100; after: stand)\n\
         }\n",
    );
    assert!(
        rust.contains("kasse.set_after(stand + 100, &stand)?;"),
        "{rust}"
    );
    // The witness is an argument of the door and never also an option, so the
    // one `after` in the emitted file is the door's own name.
    assert_eq!(rust.matches("after").count(), 1, "{rust}");
}

/// **It stores, and the value is the one that was computed outside.**
#[test]
fn the_store_happens_when_nothing_moved() {
    let printed = ran(
        "stores",
        "fn main() throws {\n\
         \x20   let kasse = SharedMut(100)\n\
         \x20   let stand = kasse.get()\n\
         \x20   kasse.set(stand + 23; after: stand)\n\
         \x20   println(f\"{kasse.get()}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "123");
}

/// **And when something did move it fails rather than overwriting it**, which
/// is the failure the whole record exists for: the second store is written
/// against a witness the lock no longer holds.
#[test]
fn an_overtaken_witness_leaves_the_value_alone() {
    let printed = ran(
        "overtaken",
        "fn main() {\n\
         \x20   let kasse = SharedMut(0)\n\
         \x20   let stand = kasse.get()\n\
         \x20   kasse.set(1; after: stand) catch { println(\"first\") }\n\
         \x20   kasse.set(2; after: stand) catch { println(f\"overtaken: {error}\") }\n\
         \x20   println(f\"{kasse.get()}\")\n\
         }\n",
    );
    assert!(printed.contains("overtaken:"), "{printed}");
    assert!(
        printed.contains("the lock no longer holds the value that was seen"),
        "{printed}"
    );
    assert!(
        printed.trim().ends_with("1"),
        "the second store did not happen: {printed}"
    );
}

/// **`Locked[T]` is the same door**, which is what one surface over two shapes
/// means ([ADR-057](../../../docs/specification/adr/adr-057.md) D4).
#[test]
fn the_local_shape_has_the_same_door() {
    let printed = ran(
        "locked",
        "fn main() throws {\n\
         \x20   let kasse = Locked(7)\n\
         \x20   let stand = kasse.get()\n\
         \x20   kasse.set(stand * 6; after: stand)\n\
         \x20   println(f\"{kasse.get()}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "42");
}

/// **The door opens a lock like any other**, so a program that reaches it with
/// one already held is `NK2203` — and the message names the entry it read.
#[test]
fn the_door_is_a_lock_taken_inside_a_lock() {
    let refused = findings(
        "fn main() throws {\n\
         \x20   let a = SharedMut(0)\n\
         \x20   let b = SharedMut(0)\n\
         \x20   let s = b.get()\n\
         \x20   a.update fn(mut v) { b.set(s + 1; after: s) }\n\
         }\n",
    );
    let about = refused
        .iter()
        .find(|f| f.code == "NK2203" && f.message.contains("set(after)"))
        .unwrap_or_else(|| panic!("{refused:#?}"));
    assert!(
        about
            .message
            .contains("takes a lock, but you're already holding one here"),
        "{}",
        about.message
    );
}

/// **`NK2208`: the lowering, written as a door.**
///
/// `set_after` is a Rust method that exists and hands back a `Result`, so a
/// program that wrote it by hand would have had its failure dropped and
/// `rustc` would have warned about a file nobody wrote (Part III C.1). D5 says
/// one door, and this is what keeps it one.
#[test]
fn the_lowering_is_not_a_second_door() {
    let refused = findings(
        "fn main() {\n\
         \x20   let kasse = SharedMut(0)\n\
         \x20   let stand = kasse.get()\n\
         \x20   kasse.set_after(stand + 1, stand)\n\
         }\n",
    );
    let about = refused
        .iter()
        .find(|f| f.code == "NK2208")
        .unwrap_or_else(|| panic!("{refused:#?}"));
    assert!(about.message.contains("kasse"), "{}", about.message);
    assert!(
        about
            .help
            .as_deref()
            .is_some_and(|h| h.contains("set(neu; after: seen)")),
        "the help is paste-ready (Part III C.2): {:?}",
        about.help
    );
}

/// **A `?.` decides whether the call happens and never what a call is**
/// ([ADR-066](../../../docs/specification/adr/adr-066.md)), so the door is the
/// same door through one.
///
/// This is the shape that would have been a **silent wrong value**: a witness
/// that stayed an option on this path would have been dropped in the lowering
/// rather than compared, and the store would have happened whatever the lock
/// held.
#[test]
fn the_door_is_the_same_door_through_a_reach() {
    let source = "fn main() throws {\n\
                  \x20   let maybe: SharedMut[i64]? = SharedMut(0)\n\
                  \x20   let stand = 0\n\
                  \x20   maybe?.set(stand + 1; after: stand)\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("set_after(stand + 1, &stand)?"), "{rust}");
}

/// **The witness is a view and the value is not**, which is the difference
/// between what is stored and what is only read.
///
/// The case that says it: a witness larger than a word, read *again* after the
/// door has been asked whether it still holds. A witness taken by value would
/// have been moved out of the caller, and `rustc` would have said so about a
/// file nobody wrote ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)) —
/// which is what this did before the ledger said `seen: &$T`.
#[test]
fn a_witness_is_lent_and_can_be_read_again() {
    let printed = ran(
        "lent",
        "fn main() throws {\n\
         \x20   let start: Vec[i64] = Vec()\n\
         \x20   let xs = Locked(start)\n\
         \x20   let seen = xs.get()\n\
         \x20   let neu: Vec[i64] = Vec()\n\
         \x20   xs.set(neu; after: seen)\n\
         \x20   println(f\"{seen.len()}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "0");
}

/// **And the `&` that makes it one is the compiler's to write** (ADR-094 D1).
///
/// `lends` withholds its claim on every method argument, because the emitter
/// cannot resolve a receiver ([ADR-028](../../../docs/specification/adr/adr-028.md)),
/// so this one position is refused on its own — without it a written `&` comes
/// out `&&` and the reader meets `rustc` about the generated file.
#[test]
fn a_written_reference_on_the_witness_is_refused() {
    let refused = findings(
        "fn main() throws {\n\
         \x20   let kasse = SharedMut(0)\n\
         \x20   let stand = kasse.get()\n\
         \x20   kasse.set(stand + 1; after: ref stand)\n\
         }\n",
    );
    assert!(refused.iter().any(|f| f.code == "NK1137"), "{refused:#?}");
}

/// **`after:` is the lock's word and nobody else's.** A type of the program's
/// own that has a `set` and an option called `after` is untouched: it is not a
/// lock, so the witness door is never the call, and what it gets is the
/// ordinary option it wrote.
#[test]
fn an_after_on_a_type_of_the_programs_own_is_an_ordinary_option() {
    let source = "struct Slot { n: i64 }\n\
                  impl Slot {\n\
                  \x20   fn set(ref self, v: i64; after: i64 = 0) -> i64 { return v + after }\n\
                  }\n\
                  fn main() {\n\
                  \x20   let s = Slot { n: 1 }\n\
                  \x20   println(f\"{s.set(2; after: 3)}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(!rust.contains("set_after"), "{rust}");
}
