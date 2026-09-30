//! A channel is `std`'s, and only bounded
//! ([ADR-149](../../../docs/specification/adr/adr-149.md), Part II 12.5).
//!
//! Nothing here is syntax: two values, two methods, and the tuple `let` that
//! binds them was already built
//! ([ADR-098](../../../docs/specification/adr/adr-098.md)). What the record had
//! to decide was where it lives and what it promises.

mod common;

use nikaia::contracts::ty::TyOps;
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

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

fn output(purpose: &str, source: &str) -> String {
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    assert!(
        ran.status.success(),
        "the program runs:\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8_lossy(&ran.stdout).trim().to_string()
}

/// **Part II 12.5's own example, as a program that runs** (§5 step 3).
#[test]
fn the_pages_own_example_runs() {
    let printed = output(
        "channel-page",
        "use std::channel\n\
         \n\
         fn main() {\n\
         \x20   let (tx, rx) = channel::bounded(100)\n\
         \x20   spawn fn {\n\
         \x20       tx.send(\"Calculation complete\")\n\
         \x20   }\n\
         \x20   let msg = rx.recv() ?? \"nobody sent anything\"\n\
         \x20   println(msg)\n\
         }\n",
    );
    assert_eq!(printed, "Calculation complete");
}

/// **D2: `send` pauses**, which is the missing `sync` line in the ledger and
/// the `.await` here. **D3: `recv` hands back a `T?`**, which is why `??`
/// reads it.
#[test]
fn both_ends_pause_and_the_receiver_hands_back_a_nullable() {
    let rust = lowered(
        "use std::channel\n\
         \n\
         fn main() {\n\
         \x20   let (tx, rx) = channel::bounded(4)\n\
         \x20   spawn fn { tx.send(1) }\n\
         \x20   let n = rx.recv() ?? 0\n\
         \x20   println(f\"{n}\")\n\
         }\n",
    );
    assert!(rust.contains("tx.send(1).await"), "{rust}");
    assert!(rust.contains("rx.recv().await"), "{rust}");
    assert!(
        rust.contains("nikaia_std::index::or("),
        "a `T?` is read with `??`\n{rust}"
    );
}

/// **A capacity is handed over whole**, and the `&` a value that moves gets is
/// not there — which is how the signature defect below was found.
#[test]
fn the_capacity_is_not_lent() {
    let rust = lowered(
        "use std::channel\n\
         \n\
         fn main() {\n\
         \x20   let (tx, rx) = channel::bounded(100)\n\
         \x20   spawn fn { tx.send(1) }\n\
         \x20   let n = rx.recv() ?? 0\n\
         \x20   println(f\"{n}\")\n\
         }\n",
    );
    assert!(rust.contains("channel::bounded(100)"), "{rust}");
    assert!(!rust.contains("channel::bounded(&"), "{rust}");
}

/// **A signature whose result has parentheses in it is read to the end of its
/// parameter list and no further.**
///
/// `rfind(')')` was there, and `channel::bounded` is the first entry in `std`
/// to hand back a **tuple** — so the last `)` in the text was the *result's*,
/// the parameter list became everything up to it, and the whole signature was
/// nonsense. Silently: a garbage parameter list still parses, and what it cost
/// was every argument to the call being lent, because a parameter whose type is
/// not known is one that moves.
#[test]
fn a_signature_that_hands_back_a_tuple_parses() {
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let bounded = library
        .functions
        .get("channel::bounded")
        .expect("`channel::bounded` is in the ledger");
    let signature = bounded.signature.as_ref().expect("it carries a signature");
    assert_eq!(signature.arguments().len(), 1);
    assert_eq!(signature.arguments()[0].0, "capacity");
    assert_eq!(signature.arguments()[0].1.to_string(), "i64");
    assert_eq!(
        signature.result.as_ref().map(|t| t.to_string()),
        Some("(channel::Sender[$T], channel::Receiver[$T])".to_string())
    );
}

/// **D3: `null` means every sender is gone**, and it is the ordinary end of a
/// stream rather than a failure — so a program reads it with `??` and never
/// with a `catch`.
#[test]
fn a_closed_channel_hands_back_nothing() {
    let printed = output(
        "channel-closed",
        "use std::channel\n\
         \n\
         fn main() {\n\
         \x20   let (tx, rx) = channel::bounded(4)\n\
         \x20   spawn fn { tx.send(5) }\n\
         \x20   let first = rx.recv() ?? 0\n\
         \x20   let second = rx.recv() ?? -1\n\
         \x20   println(f\"{first} {second}\")\n\
         }\n",
    );
    assert_eq!(printed, "5 -1");
}

/// **D2, where it is meant to be read: a `sync` body cannot send.**
///
/// And the compiler names the promise in the way rather than saying anything
/// about channels, because the ledger's column *is* the rule.
#[test]
fn a_sync_body_may_not_send() {
    let source = "use std::channel\n\
         \n\
         fn quiet(tx: channel::Sender[i64]) sync {\n\
                  \x20   tx.send(1)\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let (tx, rx) = channel::bounded(4)\n\
                  \x20   quiet(tx)\n\
                  \x20   let n = rx.recv() ?? 0\n\
                  \x20   println(f\"{n}\")\n\
                  }\n";
    let found = findings(source);
    assert!(
        found.iter().any(|f| f.message.contains("sync")),
        "a `sync` body cannot send on a channel (ADR-149 D2)\n{found:#?}"
    );
}

/// **D5: there is no unbounded channel.** A capacity is a promise about
/// memory, and `channel::unbounded` is a name nothing declares.
#[test]
fn there_is_no_unbounded_channel() {
    let library = Ledger::parse(STD).expect("std ships a ledger");
    assert!(
        !library.functions.contains_key("channel::unbounded"),
        "a capacity is a promise about memory (ADR-149 D5)"
    );
}

/// **D4: the value type must cross, checked where `tx` moves into a `spawn`.**
///
/// Being a container is the whole of it: the crossing analysis already asks
/// that question at a move, so a channel carrying a lock is refused there and
/// nothing about channels is written anywhere else.
#[test]
fn a_channel_is_answered_by_what_it_carries() {
    use nikaia::contracts::send::{Crossing, Destination, crossing};
    use nikaia::contracts::ty::Ty;
    let own = Ledger::default();
    let library = Ledger::parse(STD).expect("std ships a ledger");
    assert_eq!(
        crossing(
            &Ty::parse("Sender[String]"),
            &own,
            &library,
            Destination::Ours
        ),
        Crossing::May,
        "text may go to another thread, so a channel of it may"
    );
    assert_ne!(
        crossing(
            &Ty::parse("Receiver[Locked[i64]]"),
            &own,
            &library,
            Destination::Foreign
        ),
        Crossing::May,
        "a lock is not handed to code nothing describes, in a channel or out of one"
    );
}

/// **Back-pressure is a pause and the value still arrives.** A channel of one,
/// filled, with a second send waiting for room: the receiver takes the first,
/// the sender wakes, and both values come out in order.
#[test]
fn a_full_channel_waits_for_room() {
    let printed = output(
        "channel-backpressure",
        "use std::channel\n\
         \n\
         fn main() {\n\
         \x20   let (tx, rx) = channel::bounded(1)\n\
         \x20   spawn fn {\n\
         \x20       tx.send(1)\n\
         \x20       tx.send(2)\n\
         \x20       tx.send(3)\n\
         \x20   }\n\
         \x20   let a = rx.recv() ?? 0\n\
         \x20   let b = rx.recv() ?? 0\n\
         \x20   let c = rx.recv() ?? 0\n\
         \x20   println(f\"{a} {b} {c}\")\n\
         }\n",
    );
    assert_eq!(printed, "1 2 3");
}
