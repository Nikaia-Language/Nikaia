//! What a task **binds** and then holds across a pause
//! ([ADR-055](../../../docs/specification/adr/adr-055.md) §2 D6's third sharp
//! edge).
//!
//! `NK2501` has always asked its question of what a task **captures**. A task's
//! body is an `async` block below, so a value bound *inside* it and still live
//! at a suspension point further down is held **inside the future** — and the
//! pool's starter asks for `Send` of that whole future. Until now the only
//! thing that said so was the backend, about the generated file, which
//! [Part III C.1](../../../docs/specification/30-nikaia-tooling.md) calls a bug
//! in this compiler.
//!
//! **The refusal is silent today and that is why it was built.** No type
//! answers `MayNot` at `Destination::Ours` — `Shared` stopped being one when
//! [ADR-037](../../../docs/specification/adr/adr-037.md) D6 gave it one
//! representation at both settings, and a lock is `MayNot` only at a *foreign*
//! destination. A refusal costs nothing before there are programs it would
//! reject, and the same refusal added afterwards breaks them.
//!
//! **So what these tests hold is the liveness half**, which is the half that
//! can be wrong. The verdict is a lookup; *bound before a pause and named after
//! it* is a computation, and a computation nothing can see the answer of is one
//! nothing can check. `Checked::held_across_a_pause` is that answer.

use std::collections::{BTreeMap, BTreeSet};

use nikaia::contracts::send::held_across_a_pause;
use nikaia::contracts::ty::TyOps;
use nikaia::contracts::{Ledger, STD};
use nikaia::parser::parse_to_ast;

/// What the checker found each `spawn` in this source holds across a pause.
fn held(source: &str) -> BTreeSet<String> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &BTreeSet::new())
        .held_across_a_pause
        .into_iter()
        .map(|(_, name)| name)
        .collect()
}

/// The rule itself, in the four arrangements that are the whole of it.
#[test]
fn bound_before_a_pause_and_named_after_it() {
    let named: BTreeMap<String, usize> = [("x".to_string(), 30)].into_iter().collect();

    // Bound at 10, pause at 20, last named at 30: held.
    assert_eq!(
        held_across_a_pause(&[("x".to_string(), 10)], &[20], &named),
        ["x".to_string()].into_iter().collect::<BTreeSet<_>>()
    );

    // **Bound after the pause**: the future was already suspended once without
    // it, and nothing holds it over one.
    assert!(held_across_a_pause(&[("x".to_string(), 25)], &[20], &named).is_empty());

    // **Last named before the pause**: it is done with, so the future does not
    // carry it. This is the half that says the walk reads the *last* mention
    // and not any mention.
    let early: BTreeMap<String, usize> = [("x".to_string(), 15)].into_iter().collect();
    assert!(held_across_a_pause(&[("x".to_string(), 10)], &[20], &early).is_empty());

    // **No pause at all**: a body that never suspends holds nothing over
    // anything, whatever it binds.
    assert!(held_across_a_pause(&[("x".to_string(), 10)], &[], &named).is_empty());

    // **A name nothing mentions again** is not held either - `named` has no
    // entry for it, and an absent last use is not a late one.
    assert!(held_across_a_pause(&[("y".to_string(), 10)], &[20], &named).is_empty());
}

/// **And the walk that feeds it**, on a program: the value is bound, the task
/// pauses, and the value is used afterwards.
#[test]
fn a_value_used_after_a_pause_is_held() {
    assert_eq!(
        held(
            "use std::fs\n\nfn work() -> i64 { return 1 }\n\
             fn main() {\n\
             \x20   spawn fn {\n\
             \x20       let n = work()\n\
             \x20       let text = fs::read_to_string(\"log\", fs::Root::Anywhere) catch { return }\n\
             \x20       println(f\"{n} {text.len()}\")\n\
             \x20   }\n\
             }\n"
        ),
        ["n".to_string()].into_iter().collect::<BTreeSet<_>>(),
        "`n` is bound before the read and printed after it"
    );
}

/// **A value the body is done with before the pause is not held**, which is the
/// measurement that says the walk reads positions rather than answering *yes*.
#[test]
fn a_value_finished_with_before_the_pause_is_not_held() {
    assert!(
        held(
            "use std::fs\n\nfn work() -> i64 { return 1 }\n\
             fn main() {\n\
             \x20   spawn fn {\n\
             \x20       let n = work()\n\
             \x20       println(f\"{n}\")\n\
             \x20       let text = fs::read_to_string(\"log\", fs::Root::Anywhere) catch { return }\n\
             \x20       println(f\"{text.len()}\")\n\
             \x20   }\n\
             }\n"
        )
        .is_empty(),
        "`n` is printed before the read, and `text` is bound after it"
    );
}

/// **A body that never pauses holds nothing.** Every `sync` call in it is a
/// call the future does not suspend at, so there is no point to be held over.
#[test]
fn a_task_that_never_pauses_holds_nothing() {
    assert!(
        held(
            "fn work(n: i64) -> i64 sync { return n + 1 }\n\
         fn main() {\n\
         \x20   spawn fn {\n\
         \x20       let a = work(1)\n\
         \x20       let b = work(a)\n\
         \x20       println(f\"{a} {b}\")\n\
         \x20   }\n\
         }\n"
        )
        .is_empty()
    );
}

/// **What the task *captures* is not this question.** A name bound outside is
/// `NK2501`'s original half and is asked before the body is walked; this one is
/// about what the body binds for itself, and the two must not be confused —
/// answering both about one name would say the same thing twice.
#[test]
fn a_captured_name_is_the_other_halfs_question() {
    assert!(
        held(
            "use std::fs\n\nfn main() {\n\
         \x20   let message = \"hello\"\n\
         \x20   spawn fn {\n\
         \x20       let text = fs::read_to_string(\"log\", fs::Root::Anywhere) catch { return }\n\
         \x20       println(f\"{message} {text.len()}\")\n\
         \x20   }\n\
         }\n"
        )
        .is_empty()
    );
}

/// **A call this compiler cannot name counts as a pause**, which is the
/// fail-closed direction: the wrong answer here is a value silently crossing a
/// thread, and [ADR-010](../../../docs/specification/adr/adr-010.md) D1 says an
/// analysis that fails open is a vulnerability generator.
#[test]
fn a_call_nothing_describes_counts_as_a_pause() {
    assert_eq!(
        held(
            "fn work() -> i64 { return 1 }\n\
             fn main() {\n\
             \x20   spawn fn {\n\
             \x20       let n = work()\n\
             \x20       whatever::thing()\n\
             \x20       println(f\"{n}\")\n\
             \x20   }\n\
             }\n"
        ),
        ["n".to_string()].into_iter().collect::<BTreeSet<_>>()
    );
}

/// **No type answers `MayNot` into our own code**, which is the whole reason
/// the refusal beside this is silent — and a thing worth a test of its own,
/// because the day it stops being true is the day these programs start being
/// refused.
#[test]
fn nothing_may_not_cross_into_our_own_code_today() {
    use nikaia::contracts::send::{Crossing, Destination, crossing};
    use nikaia::contracts::ty::Ty;

    let own = Ledger::default();
    let library = Ledger::parse(STD).expect("std ships a ledger");
    for text in [
        "i64",
        "String",
        "ref String",
        "Vec[i64]",
        "Shared[i64]",
        "SharedMut[i64]",
        "Locked[i64]",
    ] {
        let ty = Ty::parse(text);
        assert!(
            !matches!(
                crossing(&ty, &own, &library, Destination::Ours),
                Crossing::MayNot { .. }
            ),
            "`{text}` answers `MayNot` into our own code, and the refusals that read \
             that verdict are written on the understanding that nothing does"
        );
    }

    // And the one that does, at the other destination - which is what says the
    // arm is reachable at all and the verdict is not simply stuck on `May`.
    assert!(matches!(
        crossing(
            &Ty::parse("Locked[i64]"),
            &own,
            &library,
            Destination::Foreign
        ),
        Crossing::MayNot { .. }
    ));
}
