//! `select { … }` keeps the first arm to finish, and a task handle has
//! `cancel()` ([ADR-292](../../../docs/specification/adr/adr-292.md)).
//!
//! Part II 12.4 wrote the block and
//! [ADR-141](../../../docs/specification/adr/adr-141.md) D2 marked it
//! *unspecified*: the **semantics** were built since
//! [ADR-297](../../../docs/specification/adr/adr-297.md) D5 — the loser stops at
//! its pause point, its `cleanup` is adopted, the deadline bounds it — and what
//! was missing was the construct.
//!
//! [ADR-292](../../../docs/specification/adr/adr-292.md)'s `overlap` is the
//! other half of the pair (D4): it runs its branches at once and keeps
//! **every** result, this one keeps the **first**.
//!
//! What a `select` computes is `tests/language/src/racing.nika`; here stay what
//! it lowers to, its refusals, and what depends on how long a task sleeps.

mod common;

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

/// Build the Rust, run it, and hand back what it printed.
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

/// **D1: an arm is `pattern = expr => { … }`**, and what it lowers to is a
/// `match` over which arm won — because an arm binding a name and then running
/// a block *is* a match arm in the language below.
#[test]
fn an_arm_binds_and_then_runs_a_block() {
    let rust = lowered(
        "use std::time\n\
         \n\
         fn slow() -> i64 { return 7 }\n\
         \n\
         fn main() {\n\
         \x20   select {\n\
         \x20       result = slow() => { println(f\"{result}\") }\n\
         \x20       _ = time::sleep(50.millis()) => { println(\"too slow\") }\n\
         \x20   }\n\
         }\n",
    );
    assert!(rust.contains("match nikaia_std::task::race2("), "{rust}");
    assert!(
        rust.contains("nikaia_std::task::Race2::First(result) =>"),
        "{rust}"
    );
    assert!(
        rust.contains("nikaia_std::task::Race2::Second(_) =>"),
        "{rust}"
    );
}

/// **Every arm is started**, which is the `async` block per arm — the same
/// shape an `overlap` branch has, and for the same reason: an arm may pause.
#[test]
fn every_arm_is_a_block_of_its_own() {
    let rust = lowered(
        "use std::time\n\
         \n\
         fn a() -> i64 { return 1 }\n\
         fn b() -> i64 { return 2 }\n\
         fn c() -> i64 { return 3 }\n\
         \n\
         fn main() {\n\
         \x20   select {\n\
         \x20       x = a() => { println(f\"{x}\") }\n\
         \x20       y = b() => { println(f\"{y}\") }\n\
         \x20       z = c() => { println(f\"{z}\") }\n\
         \x20   }\n\
         }\n",
    );
    assert!(rust.contains("race3("), "{rust}");
    for branch in ["async { a() }", "async { b() }", "async { c() }"] {
        assert!(rust.contains(branch), "{branch}\n{rust}");
    }
    assert!(rust.contains("Race3::Third(z)"), "{rust}");
}

/// **D1: `_` is the ignore pattern and not a catch-all arm**
/// ([ADR-291](../../../docs/specification/adr/adr-291.md) D6). A value
/// *arrives* at that arm and is not wanted — which is exactly what `_` means in
/// the language below, so nothing has to be translated.
#[test]
fn an_underscore_arm_binds_nothing() {
    let source = "use std::time\n\
         \n\
         fn a() -> i64 { return 1 }\n\
                  fn b() -> i64 { return 2 }\n\
                  \n\
                  fn main() {\n\
                  \x20   select {\n\
                  \x20       _ = a() => { println(\"first\") }\n\
                  \x20       _ = b() => { println(\"second\") }\n\
                  \x20   }\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("Race2::First(_) =>"), "{rust}");
    assert!(rust.contains("Race2::Second(_) =>"), "{rust}");
}

/// **A `select` of one arm has nothing to race against**, which is the
/// `overlap` refusal one construct over.
#[test]
fn one_arm_is_refused_with_its_reason() {
    let parsed = parse_to_ast(
        "use std::time\n\
         \n\
         fn a() -> i64 { return 1 }\n\
         \n\
         fn main() {\n\
         \x20   select {\n\
         \x20       x = a() => { println(f\"{x}\") }\n\
         \x20   }\n\
         }\n",
    )
    .expect("the source parses");
    let refused = emit_program(&parsed, Build::default()).expect_err("one arm races nothing");
    assert!(
        refused.to_string().contains("at least two arms"),
        "{refused}"
    );
}

/// **D1: `select` is a keyword now**, which is the cost
/// [ADR-276](../../../docs/specification/adr/adr-276.md) calls the most
/// expensive thing a language adds and this record spends knowingly.
#[test]
fn select_is_a_reserved_word() {
    assert!(
        nikaia::parser::RESERVED_WORDS.contains(&"select"),
        "`select` is reserved (ADR-292 D12)"
    );
    assert!(
        parse_to_ast("use std::time\n\nfn main() { let select = 3 }\n").is_err(),
        "a reserved word is not a name"
    );
}

/// **And the other way round**: an arm that pauses loses to one that does not
/// have to, whatever the written order.
#[test]
fn a_sleeping_arm_loses_to_a_ready_one() {
    let printed = output(
        "select-sleeping",
        "use std::time\n\
         \n\
         fn main() {\n\
         \x20   select {\n\
         \x20       _ = time::sleep(5.seconds()) => { println(\"too slow\") }\n\
         \x20       _ = time::sleep(1.millis()) => { println(\"soon enough\") }\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(printed, "soon enough");
}

/// **An arm that can fail propagates from the arm that won**, and not from the
/// `match`: only the winner has a value.
#[test]
fn a_failing_arm_propagates_from_its_own_body() {
    let source = "use std::time\n\
         \n\
         fn risky() -> i64 throws { return 3 }\n\
                  \n\
                  fn main() throws {\n\
                  \x20   select {\n\
                  \x20       n = risky() => { println(f\"{n}\") }\n\
                  \x20       _ = time::sleep(5.seconds()) => { println(\"too slow\") }\n\
                  \x20   }\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(
        rust.contains("Ok::<_, Box<dyn std::error::Error>>("),
        "every arm is wrapped so the vehicle sees one shape\n{rust}"
    );
    assert!(rust.contains("let n = __nikaia_won?;"), "{rust}");
}

/// **D3: a handle has `cancel()`**, and it takes the handle — so §4's open
/// question, *can a cancelled task be observed to have been cancelled?*, is one
/// no program can ask.
#[test]
fn a_handle_can_be_cancelled() {
    let source = "use std::time\n\
         \n\
         fn main() {\n\
                  \x20   let handle = spawn fn { 1 + 1 }\n\
                  \x20   handle.cancel()\n\
                  \x20   println(\"stopped\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("handle.cancel();"),
        "{}",
        lowered(source)
    );
}

/// **A cancelled task does not run to its end**, which is what makes `cancel`
/// a mechanism rather than a name: the task pauses once, and by the time it
/// would come back it has been asked to stop.
#[test]
fn a_cancelled_task_stops_at_its_pause_point() {
    let printed = output(
        "select-cancel-stops",
        "use std::time\n\
         \n\
         fn main() {\n\
         \x20   let handle = spawn fn {\n\
         \x20       time::sleep(50.millis())\n\
         \x20       println(\"the task finished\")\n\
         \x20   }\n\
         \x20   handle.cancel()\n\
         \x20   time::sleep(150.millis())\n\
         \x20   println(\"main finished\")\n\
         }\n",
    );
    assert_eq!(printed, "main finished");
}

/// **A task nobody cancels still runs** (ADR-055 D5), which is the other half
/// of the sentence above: `cancel` is what stops a task, and nothing else here
/// changed.
#[test]
fn a_task_that_is_not_cancelled_still_runs() {
    let printed = output(
        "select-uncancelled",
        "use std::time\n\
         \n\
         fn main() {\n\
         \x20   let handle = spawn fn {\n\
         \x20       time::sleep(10.millis())\n\
         \x20       println(\"the task finished\")\n\
         \x20   }\n\
         \x20   time::sleep(100.millis())\n\
         \x20   println(\"main finished\")\n\
         }\n",
    );
    assert_eq!(printed, "the task finished\nmain finished");
}

/// **A task that sleeps after `main` is done wakes when its clock says so**,
/// and not at the drain's deadline: once `main`'s value was in, the drain went
/// round without ever reaching the park where the clock is read, and a task
/// nobody joined waited out the whole 30 seconds (found by ADR-297 D5).
#[test]
fn a_task_that_sleeps_after_main_is_done_is_not_held_to_the_deadline() {
    let started = std::time::Instant::now();
    let printed = output(
        "select-sleeps-after-main",
        "use std::time\n\
         \n\
         fn main() {\n\
         \x20   let handle = spawn fn {\n\
         \x20       time::sleep(50.millis())\n\
         \x20       println(\"the task finished\")\n\
         \x20   }\n\
         \x20   println(\"main finished\")\n\
         }\n",
    );
    assert_eq!(printed, "main finished\nthe task finished");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
}
