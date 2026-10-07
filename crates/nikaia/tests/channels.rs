//! A channel is `std`'s, and only bounded
//! ([ADR-149](../../../docs/specification/adr/adr-149.md), Part II 12.5).
//!
//! Nothing here is syntax: two values, two methods, and the tuple `let` that
//! binds them was already built
//! ([ADR-291](../../../docs/specification/adr/adr-291.md)). What the record had
//! to decide was where it lives and what it promises.
//!
//! What a program reads from one when it runs is
//! `tests/language/src/channels.nika`; here are the ledger, the lowering and
//! the refusals.

use nikaia::contracts::SignatureOps;
use nikaia::contracts::ty::TyOps;
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

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
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
    let own = Ledger::blank();
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
