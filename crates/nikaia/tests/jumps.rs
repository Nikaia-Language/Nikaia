//! `break` and `continue` — Part I 3.3, unlabelled.
//!
//! Two halves, and they are the two halves of what the construct is:
//!
//! **What it means.** Lowered, compiled as Rust and *run*, for the reason
//! `returns.rs` gives in its own header: both readings of a jump type-check, so
//! a test that only compiled the output would be asserting that the emitter
//! agrees with itself. What a `break` does is visible in what the program
//! prints and nowhere else.
//!
//! **Where it may not stand.** A jump is a machine instruction that moves to a
//! label, and a label in another function is not reachable — so a `break` whose
//! loop is outside a lambda, a task or an `overlap` branch is not a program this
//! compiler may lower. Each of those is a closure or an `async` block below, and
//! without a refusal here the message would be `rustc`'s, about a file nobody
//! wrote (Part III, C.1).

mod common;

use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, STD};
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

/// Lower, compile, run, and hand back what it printed.
fn output_of(purpose: &str, source: &str) -> String {
    assert_eq!(
        findings(source)
            .iter()
            .map(|f| f.message.as_str())
            .collect::<Vec<_>>(),
        Vec::<&str>::new(),
        "{purpose} is meant to be a correct program"
    );

    let dir = common::scratch_dir(purpose);
    let rust = dir.join(format!("{purpose}.rs"));
    std::fs::write(&rust, lowered(source)).expect("write the Rust");

    let binary = dir.join(purpose);
    let built = common::compile(&rust, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "the lowering of {purpose} does not compile:\n{}\n--- the Rust ---\n{}",
        String::from_utf8_lossy(&built.stderr),
        lowered(source)
    );

    let run = std::process::Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run it");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let printed = String::from_utf8_lossy(&run.stdout).to_string();
    let _ = std::fs::remove_dir_all(&dir);
    printed
}

// --- what they mean ----------------------------------------------------------

/// A `break` leaves the loop and not the function: the line after the loop runs.
#[test]
fn a_break_leaves_the_loop_and_the_function_goes_on() {
    let source = "fn main() {\n\
         \x20   let mut total = 0\n\
         \x20   for i in 0..<10 {\n\
         \x20       if i == 4 {\n\
         \x20           break\n\
         \x20       }\n\
         \x20       total += i\n\
         \x20   }\n\
         \x20   println(f\"{total}\")\n\
         \x20   println(\"after\")\n\
         }\n";
    assert_eq!(output_of("break-leaves-the-loop", source), "6\nafter\n");
}

/// A `continue` skips the rest of the turn and not the turns after it.
#[test]
fn a_continue_skips_one_turn() {
    let source = "fn main() {\n\
         \x20   let mut total = 0\n\
         \x20   for i in 0..<10 {\n\
         \x20       if i % 2 == 0 {\n\
         \x20           continue\n\
         \x20       }\n\
         \x20       total += i\n\
         \x20   }\n\
         \x20   println(f\"{total}\")\n\
         }\n";
    assert_eq!(output_of("continue-skips-a-turn", source), "25\n");
}

/// **The innermost loop, and nothing further out** — which is the whole of what
/// an unlabelled jump promises, and the one thing a label would change.
#[test]
fn a_break_leaves_the_innermost_loop() {
    let source = "fn main() {\n\
         \x20   let mut total = 0\n\
         \x20   for i in 0..<3 {\n\
         \x20       for j in 0..<10 {\n\
         \x20           if j == 2 {\n\
         \x20               break\n\
         \x20           }\n\
         \x20           total += 1\n\
         \x20       }\n\
         \x20       total += 100\n\
         \x20   }\n\
         \x20   println(f\"{total}\")\n\
         }\n";
    // Two turns of the inner loop per outer turn, three outer turns that all
    // reach the `+= 100`: the outer loop is untouched by the inner `break`.
    assert_eq!(output_of("break-innermost", source), "306\n");
}

/// `while true { … break … }` — the unconditional loop
/// ([ADR-070](../../../docs/specification/adr/adr-070.md) D1) with the exit
/// D2 said it did not have.
#[test]
fn a_break_is_how_an_unconditional_loop_ends() {
    let source = "fn main() {\n\
         \x20   let mut n = 0\n\
         \x20   while true {\n\
         \x20       n += 1\n\
         \x20       if n == 5 {\n\
         \x20           break\n\
         \x20       }\n\
         \x20   }\n\
         \x20   println(f\"{n}\")\n\
         }\n";
    assert_eq!(output_of("break-while-true", source), "5\n");
}

/// **A `catch` handler is not a function**, so a jump in one reaches the loop
/// around it. The handler lowers to a `match` arm (Kap 7.1), which is why this
/// works where a lambda's does not — and why it is worth a test of its own
/// rather than an assumption.
#[test]
fn a_break_in_a_catch_handler_leaves_the_loop() {
    let source = "use std::fs\n\
         \n\
         fn read_them(paths: Vec[ref String]) -> i64 {\n\
         \x20   let mut seen = 0\n\
         \x20   for p in paths {\n\
         \x20       let text = fs::read_to_string(p, fs::Root::Anywhere) catch {\n\
         \x20           break\n\
         \x20       }\n\
         \x20       seen += text.len()\n\
         \x20   }\n\
         \x20   return seen\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let mut paths = Vec()\n\
         \x20   paths.push(\"nothing-here.txt\")\n\
         \x20   println(f\"{read_them(paths)}\")\n\
         }\n";
    assert_eq!(output_of("break-in-a-catch", source), "0\n");
}

/// A jump in **value position**: the language below reads a block that ends in
/// one as the `!` it is, so the other half of the `if` decides the type and
/// nothing has to be said about it here.
#[test]
fn a_break_may_stand_where_a_value_is_expected() {
    let source = "fn main() {\n\
         \x20   let mut total = 0\n\
         \x20   for i in 0..<10 {\n\
         \x20       let step = if i > 3 { break } else { i }\n\
         \x20       total += step\n\
         \x20   }\n\
         \x20   println(f\"{total}\")\n\
         }\n";
    assert_eq!(output_of("break-as-a-value", source), "6\n");
}

/// The lowering is name for name (ADR-011 D2), and this is what says so — the
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

/// Each branch of an `overlap` is an `async` block of its own (ADR-050 D2).
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
/// ([ADR-151](../../../docs/specification/adr/adr-151.md) D2). It said *bind it
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
/// the way out ([ADR-151](../../../docs/specification/adr/adr-151.md) D2, and
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
         \x20   pub rule file -> i64 = fold(N, zero, fn(acc, m) { let h = spawn fn { break } })\n\
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
/// has for what [ADR-070](../../../docs/specification/adr/adr-070.md) D1
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

/// And it is the same program: the loop is left by its `break`, and what comes
/// after the loop still runs.
#[test]
fn an_unconditional_loop_still_means_what_it_meant() {
    let source = "fn main() {\n\
         \x20   let mut n = 0\n\
         \x20   while true {\n\
         \x20       n += 1\n\
         \x20       if n == 5 {\n\
         \x20           break\n\
         \x20       }\n\
         \x20   }\n\
         \x20   println(f\"{n}\")\n\
         \x20   println(\"after\")\n\
         }\n";
    assert_eq!(output_of("loop-lowering", source), "5\nafter\n");
}

// --- the head grammar (ADR-086) ----------------------------------------------
//
// These sit here rather than in a file of their own because of how they were
// found: the loop a language *without* `break` has to write is
// `while i < n && running`, and it did not parse. The gap is about heads and not
// about jumps, and the record says so; the tests stay beside the work that
// turned them up.

/// `&&` and `||` in a `while` head — run, because *"it parses"* is not the
/// claim. The claim is that it means what it says.
#[test]
fn a_while_head_takes_both_connectives() {
    let source = "fn main() {\n\
         \x20   let mut i = 0\n\
         \x20   let mut total = 0\n\
         \x20   let mut running = true\n\
         \x20   while i < 10 && running {\n\
         \x20       total += i\n\
         \x20       if total > 20 || i > 8 {\n\
         \x20           running = false\n\
         \x20       }\n\
         \x20       i += 1\n\
         \x20   }\n\
         \x20   println(f\"{total}\")\n\
         }\n";
    // 0+1+…+6 = 21, which is the first total over 20; the turn that reaches it
    // still finishes, so `i` becomes 7 and the head then stops the loop.
    assert_eq!(output_of("head-and-or", source), "21\n");
}

/// **The precedence is the ordinary one** ([ADR-086](../../../docs/specification/adr/adr-086.md)
/// D1): `a || b && c` is `a || (b && c)`, not `(a || b) && c`. The two disagree
/// exactly where the first operand is false and the last one is, which is what
/// the second call checks.
#[test]
fn a_head_binds_and_tighter_than_or() {
    let source = "fn pick(a: bool, b: bool, c: bool) -> i64 {\n\
         \x20   if a || b && c {\n\
         \x20       return 1\n\
         \x20   }\n\
         \x20   return 0\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{pick(false, true, true)}{pick(false, true, false)}\")\n\
         }\n";
    // `(false || true) && false` would be 0 and 0; `false || (true && false)`
    // is 1 and 0.
    assert_eq!(output_of("head-precedence", source), "10\n");
}

/// A range in a head still binds looser than everything in it — `head_range_tail`
/// moved down to the new top of the chain, and this is what says the move was
/// harmless.
#[test]
fn a_range_head_still_binds_loosest() {
    let source = "fn main() {\n\
         \x20   let mut t = 0\n\
         \x20   for i in 0..<5 - 1 {\n\
         \x20       t += i\n\
         \x20   }\n\
         \x20   println(f\"{t}\")\n\
         }\n";
    // `0..(5 - 1)` is 0,1,2,3 and sums to 6; `(0..5) - 1` is not a program.
    assert_eq!(output_of("head-range", source), "6\n");
}

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

/// **`??`, `as`, `null` and a tuple in a head** — the four forms
/// [ADR-087](../../../docs/specification/adr/adr-087.md) added, in one program,
/// run rather than read.
#[test]
fn a_head_holds_every_expression_that_is_not_brace_led() {
    let source = "fn pick(a: i64?, b: i32) -> i64 {\n\
         \x20   if (a ?? 0) > 3 {\n\
         \x20       return 1\n\
         \x20   }\n\
         \x20   if b as i64 > 3 {\n\
         \x20       return 2\n\
         \x20   }\n\
         \x20   if a == null {\n\
         \x20       return 3\n\
         \x20   }\n\
         \x20   return 4\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{pick(9, 0)}\")\n\
         \x20   println(f\"{pick(null, 9)}\")\n\
         \x20   println(f\"{pick(null, 0)}\")\n\
         \x20   println(f\"{pick(1, 0)}\")\n\
         }\n";
    // One hole per `println`: several in one `f"…"` walks into a defect that
    // has nothing to do with heads — the second `null` comes out `Some(None)`
    // — and it is on the issue tracker with its own reproduction.
    assert_eq!(output_of("head-complete", source), "1\n2\n3\n4\n");
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
/// about. [ADR-089](../../../docs/specification/adr/adr-089.md) then refused the
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
/// [ADR-089](../../../docs/specification/adr/adr-089.md) D1 narrows a `??`'s
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

/// A tuple in a head, which needed `tuple_expr` *before* `paren_expr` for the
/// same reason `primary_expr` needs it: a tuple is a parenthesised expression
/// until the comma.
#[test]
fn a_head_holds_a_tuple() {
    let source = "fn main() {\n\
         \x20   let a = 1\n\
         \x20   let b = 2\n\
         \x20   if (a, b) == (1, 2) {\n\
         \x20       println(\"pair\")\n\
         \x20   }\n\
         }\n";
    assert_eq!(output_of("head-tuple", source), "pair\n");
}

/// **And the one restriction that remains is reachable through parentheses**
/// ([ADR-087](../../../docs/specification/adr/adr-087.md) D2), which is what
/// makes it a restriction on *spelling* rather than on meaning.
#[test]
fn a_brace_led_form_reaches_a_head_through_parentheses() {
    let source = "fn main() {\n\
         \x20   let n = 1\n\
         \x20   if (match n { 1 => 10, else => 20 }) > 15 {\n\
         \x20       println(\"high\")\n\
         \x20   } else {\n\
         \x20       println(\"low\")\n\
         \x20   }\n\
         }\n";
    assert_eq!(output_of("head-parenthesised", source), "low\n");
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
