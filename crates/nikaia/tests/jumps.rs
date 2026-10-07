//! `break` and `continue` — Part I 3.3, unlabelled.
//!
//! Two halves, and they are the two halves of what the construct is:
//!
//! **What it means** is `tests/language/src/jumps.nika`: both readings of a
//! jump type-check, so what a `break` does is visible only in what a program
//! computes, and those are tests of the language, run by `nikaia test`.
//!
//! **Where it may not stand.** A jump is a machine instruction that moves to a
//! label, and a label in another function is not reachable — so a `break` whose
//! loop is outside a lambda, a task or an `overlap` branch is not a program this
//! compiler may lower. Each of those is a closure or an `async` block below, and
//! without a refusal here the message would be `rustc`'s, about a file nobody
//! wrote (Part III, C.1).

mod common;

use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check(&parsed, &own, &library).findings
}

/// The one finding a source is written to produce, with its code.
fn one(source: &str) -> (String, String) {
    let found = findings(source);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one finding, got {:#?}",
        found.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
    (found[0].code.to_string(), found[0].message.clone())
}

// --- how they are lowered ------------------------------------------------

/// The lowering is name for name (ADR-296 D17), and this is what says so — the
/// one assertion in this file about emitted text rather than about behaviour,
/// because *"it compiles to a jump and not to a flag"* is the claim
/// `docs/history/break-continue-cost.md` rests its numbers on.
#[test]
fn the_lowering_is_the_same_word() {
    let rust = lowered(
        "fn f(n: i64) -> i64 {\n\
         \x20   let mut t = 0\n\
         \x20   for i in 0..<n {\n\
         \x20       if i == 1 {\n\
         \x20           continue\n\
         \x20       }\n\
         \x20       if i == 5 {\n\
         \x20           break\n\
         \x20       }\n\
         \x20       t += i\n\
         \x20   }\n\
         \x20   return t\n\
         }\n",
    );
    assert!(rust.contains("continue;"), "{rust}");
    assert!(rust.contains("break;"), "{rust}");
}

// --- where they may not stand ------------------------------------------------

/// `NK1132`, with no loop at all.
#[test]
fn a_break_outside_a_loop_is_refused() {
    let (code, message) = one("fn f() {\n    break\n}\n");
    assert_eq!(code, "NK1132");
    assert!(
        message.contains("but this isn't inside a loop"),
        "{message}"
    );
}

#[test]
fn a_continue_outside_a_loop_is_refused() {
    let (code, message) = one("fn f() {\n    continue\n}\n");
    assert_eq!(code, "NK1132");
    assert!(message.starts_with("`continue`"), "{message}");
}

/// **A lambda is a closure below**, so the loop outside it is not reachable
/// from inside it — and the message says which of the two facts refused the
/// program.
#[test]
fn a_break_in_a_lambda_cannot_reach_the_loop_outside_it() {
    let (code, message) = one("fn f(xs: Vec[i64]) -> i64 {\n\
         \x20   let mut t = 0\n\
         \x20   for x in xs {\n\
         \x20       let each = fn (n) { break }\n\
         \x20       t += 1\n\
         \x20   }\n\
         \x20   return t\n\
         }\n");
    assert_eq!(code, "NK1132");
    assert!(message.contains("outside this lambda"), "{message}");
}

/// A task is an `async` block below (ADR-055 §6), which is a function too.
#[test]
fn a_continue_in_a_task_cannot_reach_the_loop_outside_it() {
    let (code, message) = one("fn f(n: i64) -> i64 {\n\
         \x20   let mut t = 0\n\
         \x20   while t < n {\n\
         \x20       let h = spawn fn { continue }\n\
         \x20       t += 1\n\
         \x20   }\n\
         \x20   return t\n\
         }\n");
    assert_eq!(code, "NK1132");
    assert!(message.contains("outside this task"), "{message}");
}

/// Each branch of an `overlap` is an `async` block of its own (ADR-292 D2).
#[test]
fn a_break_in_an_overlap_branch_cannot_reach_the_loop_outside_it() {
    let (code, message) = one("use std::fs\n\
         \n\
         fn f(n: i64) -> i64 {\n\
         \x20   let mut t = 0\n\
         \x20   for i in 0..<n {\n\
         \x20       let r = overlap {\n\
         \x20           fs::read_to_string(\"a\", fs::Root::Anywhere) catch { break }\n\
         \x20           fs::read_to_string(\"b\", fs::Root::Anywhere) catch { \"\" }\n\
         \x20       }\n\
         \x20       t += 1\n\
         \x20   }\n\
         \x20   return t\n\
         }\n");
    assert_eq!(code, "NK1132");
    assert!(
        message.contains("outside this `overlap` branch"),
        "{message}"
    );
}

/// **A loop written inside the lambda is the lambda's own**, so this one is a
/// correct program — the boundary is about which loop is reachable, not about
/// lambdas being loop-free.
#[test]
fn a_loop_inside_a_lambda_is_a_loop_a_break_may_leave() {
    assert_eq!(
        findings(
            "fn f(xs: Vec[i64]) -> i64 {\n\
             \x20   let each = fn (n) {\n\
             \x20       for i in 0..<n {\n\
             \x20           break\n\
             \x20       }\n\
             \x20       n\n\
             \x20   }\n\
             \x20   return 0\n\
             }\n"
        )
        .len(),
        0
    );
}

/// `NK1133`: **`break i` is two statements**, and the second is not reached.
/// The shape the refusal exists for — a `break` in Rust carries a value and
/// here it does not, so the value would be dropped in silence.
///
/// **And the help names the two shapes that do carry a value out of a loop**
/// ([ADR-276](../../../docs/specification/adr/adr-276.md) D14). It said *bind it
/// before the `break`* alone, which is one of the two and not the one a reader
/// usually wants: a search loop is written with a `return`.
#[test]
fn a_value_written_after_a_break_is_refused() {
    let (code, message) = one("fn f(n: i64) -> i64 {\n\
         \x20   for i in 0..<n {\n\
         \x20       break i\n\
         \x20   }\n\
         \x20   return 0\n\
         }\n");
    assert_eq!(code, "NK1133");
    assert!(
        message.contains("Nothing after `break` in the same block can run"),
        "{message}"
    );
}

/// The help, on its own, because the message above is the *claim* and this is
/// the way out ([ADR-276](../../../docs/specification/adr/adr-276.md) D14, and
/// [Part III C.2](../../../docs/specification/30-nikaia-tooling.md): a rule a
/// reader cannot act on is an obstacle).
#[test]
fn the_help_names_a_let_before_the_loop_and_a_return() {
    let parsed = parse_to_ast(
        "fn f(n: i64) -> i64 {\n\
         \x20   for i in 0..<n {\n\
         \x20       break i\n\
         \x20   }\n\
         \x20   return 0\n\
         }\n",
    )
    .expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    let found = check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .find(|f| f.code == "NK1133")
        .expect("the refusal");
    let help = found.help.as_deref().expect("a way out");
    assert!(help.contains("a `let` declared before it"), "{help}");
    assert!(help.contains("`return`"), "{help}");
}

/// The same rule, reached by the other door: a line left below a `break`.
#[test]
fn a_statement_after_a_break_is_refused() {
    let (code, _) = one("fn f(n: i64) -> i64 {\n\
         \x20   let mut t = 0\n\
         \x20   for i in 0..<n {\n\
         \x20       break\n\
         \x20       t += i\n\
         \x20   }\n\
         \x20   return t\n\
         }\n");
    assert_eq!(code, "NK1133");
}

/// And a `break` that **is** the last statement of its block is silent, which
/// is what keeps the rule from reaching the shape every program writes.
#[test]
fn a_break_at_the_end_of_its_block_is_silent() {
    assert_eq!(
        findings(
            "fn f(n: i64) -> i64 {\n\
             \x20   let mut t = 0\n\
             \x20   for i in 0..<n {\n\
             \x20       t += i\n\
             \x20       if t > 10 {\n\
             \x20           break\n\
             \x20       }\n\
             \x20   }\n\
             \x20   return t\n\
             }\n"
        )
        .len(),
        0
    );
}

/// **The backstop under `NK1132`.**
///
/// The checker's boundaries are a walk, and a walk can miss a corner. This is
/// the one that was missed: a `spawn` inside a DSL fold's step, which the block
/// walk that reaches a fold's lambdas deliberately does not descend into,
/// because a task's body is a detached context everywhere else it is asked
/// about. The *emitter* has no such walk — every statement goes through one
/// place — so the refusal is there as well, and this is what says it is.
///
/// A program should never meet this message; it exists so that no program meets
/// `rustc`'s.
#[test]
fn a_jump_the_checker_walk_misses_is_still_not_emitted() {
    let source = "grammar Nums {\n\
         \x20   rule N -> i64 = d:i64 { d }\n\
         \x20   entry rule file -> i64 = fold(N, zero, fn(acc, m) { let h = spawn fn { break } })\n\
         }\n\
         \n\
         fn zero() -> i64 { return 0 }\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let refused =
        emit_program(&parsed, Build::default()).expect_err("a jump with no loop is not emitted");
    assert!(
        refused
            .to_string()
            .contains("only works inside a loop, and there's none here"),
        "{refused}"
    );
}

/// And the same guarantee from the other side: a loop written **inside** the
/// task is a loop the jump may leave, so the backstop does not refuse a correct
/// program.
#[test]
fn the_backstop_lets_a_loop_written_inside_the_task_through() {
    let rust = lowered(
        "fn f(n: i64) -> i64 {\n\
         \x20   let h = spawn fn {\n\
         \x20       for i in 0..<n {\n\
         \x20           break\n\
         \x20       }\n\
         \x20       n\n\
         \x20   }\n\
         \x20   return h.join()\n\
         }\n",
    );
    assert!(rust.contains("break;"), "{rust}");
}

/// **`while true` is lowered to `loop`**, which is the form the language below
/// has for what [ADR-276](../../../docs/specification/adr/adr-276.md) D1
/// decided `while true` *is*.
///
/// Two things rest on it and only one of them is cosmetic. `rustc` answers
/// *"denote infinite loops with `loop { … }`"* on the emitted line otherwise —
/// a warning about a file nobody wrote — and, the reason that matters less:
/// `while true { }` is `()` below and `loop { }` is `!`, so the second form is
/// the only one a function that never returns can be built out of.
#[test]
fn an_unconditional_loop_is_lowered_to_loop() {
    let rust = lowered(
        "fn f(n: i64) -> i64 {\n\
         \x20   let mut t = n\n\
         \x20   while true {\n\
         \x20       t -= 1\n\
         \x20       if t <= 0 {\n\
         \x20           break\n\
         \x20       }\n\
         \x20   }\n\
         \x20   return t\n\
         }\n",
    );
    assert!(rust.contains("loop {"), "{rust}");
    assert!(!rust.contains("while true"), "{rust}");
}

/// **The literal only.** A condition that is a name stays a `while`, even where
/// the name is a `let` bound to `true`: the equivalence is about the written
/// form, and claiming it anywhere else would be claiming something this
/// compiler has not established.
#[test]
fn a_condition_that_is_not_the_literal_stays_a_while() {
    let rust = lowered(
        "fn f(n: i64) -> i64 {\n\
         \x20   let mut t = n\n\
         \x20   let running = true\n\
         \x20   while running {\n\
         \x20       t -= 1\n\
         \x20       if t <= 0 {\n\
         \x20           break\n\
         \x20       }\n\
         \x20   }\n\
         \x20   return t\n\
         }\n",
    );
    assert!(rust.contains("while running {"), "{rust}");
}

// --- the head grammar (ADR-301) ----------------------------------------------
//
// These sit here rather than in a file of their own because of how they were
// found: the loop a language *without* `break` has to write is
// `while i < n && running`, and it did not parse. The gap is about heads and not
// about jumps, and the record says so; the tests stay beside the work that
// turned them up.

/// And the restriction the head chain exists for is **unchanged**: a brace-led
/// form still may not stand in a head, because the `{` is the body's.
#[test]
fn a_head_still_refuses_a_brace_led_form() {
    let refused = parse_to_ast(
        "struct P { x: i64 }\n\
         \n\
         fn f(p: P) -> i64 {\n\
         \x20   if p == P { x: 1 } {\n\
         \x20       return 1\n\
         \x20   }\n\
         \x20   return 0\n\
         }\n",
    );
    assert!(
        refused.is_err(),
        "a struct literal in a head would take the body's brace"
    );
}

/// **The claim itself, checked directly**: a head parses what a body parses.
///
/// The same expression in both positions, lowered, and the two lowerings
/// compared — so a level that mirrors its counterpart *badly* is caught as
/// readily as one that is missing.
///
/// `a ?? 0 > 3` used to be the first entry here, and it is the case that found
/// this test's own first draft wrong: it was `a ?? (0 > 3)` in **both**
/// positions, which reads oddly and was not the head's business to differ
/// about. [ADR-279](../../../docs/specification/adr/adr-279.md) then refused the
/// shape outright — in both positions, which is this test's claim holding by a
/// different route — so it moved to
/// [`a_bare_binary_fallback_is_refused_in_a_head_too`] below rather than out.
#[test]
fn a_head_parses_what_a_body_parses() {
    for expression in [
        "(a ?? 0) > 3",
        "b as i64 > 3",
        "a == null",
        "b > 1 && a != null || b < 0",
        "b as i64 * 2 + 1 > 3",
    ] {
        let body = lowered(&format!(
            "fn f(a: i64?, b: i32) -> bool {{\n    let x = {expression}\n    return x\n}}\n"
        ));
        let head = lowered(&format!(
            "fn f(a: i64?, b: i32) -> i64 {{\n    if {expression} {{\n        return 1\n    }}\n    return 0\n}}\n"
        ));
        let body = body
            .lines()
            .find_map(|l| l.trim().strip_prefix("let x = "))
            .map(|l| l.trim_end_matches(';').to_string())
            .unwrap_or_else(|| panic!("no `let` in:\n{body}"));
        let head = head
            .lines()
            .find_map(|l| l.trim().strip_prefix("if "))
            .map(|l| {
                l.split(" { return 1")
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .unwrap_or_else(|| panic!("no `if` in:\n{head}"));
        assert_eq!(body, head, "`{expression}` parses differently in a head");
    }
}

/// And the claim holds for what is **refused**, which is the half a test about
/// parsing would otherwise miss.
///
/// [ADR-279](../../../docs/specification/adr/adr-279.md) D1 narrows a `??`'s
/// fallback, and the grammar has two chains: narrowing one and not the other
/// would let `while a ?? x == y` parse where the same line in a body does not.
/// That is exactly the drift this file exists to catch, and it caught it — the
/// head's rule was missed on the first pass.
#[test]
fn a_bare_binary_fallback_is_refused_in_a_head_too() {
    for shape in [
        "fn f(a: i64?) -> bool {\n    let x = a ?? 0 > 3\n    return x\n}\n",
        "fn f(a: i64?) -> i64 {\n    if a ?? 0 > 3 {\n        return 1\n    }\n    return 0\n}\n",
        "fn f(a: i64?) -> i64 {\n    while a ?? 0 > 3 {\n        return 1\n    }\n    return 0\n}\n",
    ] {
        let refused = nikaia::parser::parse_to_ast(shape)
            .expect_err("a bare binary fallback is refused wherever it stands");
        let finding = nikaia::diagnostics::refused_finding(&refused)
            .expect("a parse error carries its finding");
        let said = nikaia::diagnostics::render_finding(finding, "f.nika", shape);
        assert!(
            said.contains("The fallback after `??` is a single value unless you use brackets"),
            "and says the same thing in every position:\n{said}"
        );
    }
}

/// The restriction itself, unchanged: without the parentheses the `{` is the
/// body's, and the program does not parse.
#[test]
fn a_bare_brace_led_form_is_still_refused_in_a_head() {
    assert!(
        parse_to_ast(
            "struct P { x: i64 }\n\
             \n\
             fn f(p: P) -> i64 {\n\
             \x20   if p == P { x: 1 } {\n\
             \x20       return 1\n\
             \x20   }\n\
             \x20   return 0\n\
             }\n"
        )
        .is_err(),
        "a bare struct literal in a head would take the body's brace"
    );
}
