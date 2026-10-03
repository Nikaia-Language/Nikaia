//! **A jump may carry its condition after it**
//! ([ADR-276](../../../docs/specification/adr/adr-276.md)).
//!
//! `return 250 if speed > 250` is `if speed > 250 { return 250 }`, and the
//! same for `throw`, `break` and `continue`. Each test runs the program,
//! because what the form promises is where control goes.

mod common;

use nikaia::ast::{Expr, Item, Stmt};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

fn ran(purpose: &str, source: &str) -> String {
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust;
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );
    let out = std::process::Command::new(&binary)
        .output()
        .expect("run it");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The statements of the program's `main`, as the parser built them.
fn main_body(source: &str) -> Vec<Stmt> {
    let parsed = parse_to_ast(source).expect("the source parses");
    parsed
        .program
        .items
        .iter()
        .find_map(|item| match &item.node {
            Item::Fn {
                name: Some(name),
                body,
                ..
            } if parsed.text(*name) == "main" => {
                Some(body.stmts.iter().map(|s| s.node.clone()).collect())
            }
            _ => None,
        })
        .expect("a main")
}

#[test]
fn a_return_leaves_only_where_its_condition_holds() {
    let out = ran(
        "jump-guards-return",
        "fn limit(speed: i64) -> i64 {\n\
         \x20   return 250 if speed > 250\n\
         \x20   return speed\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{limit(300)} {limit(100)}\")\n\
         }\n",
    );
    assert_eq!(out, "250 100\n");
}

#[test]
fn a_bare_return_may_carry_its_condition() {
    let out = ran(
        "jump-guards-bare-return",
        "fn greet(quiet: bool) {\n\
         \x20   return if quiet\n\
         \x20   println(\"hello\")\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   greet(true)\n\
         \x20   greet(false)\n\
         }\n",
    );
    assert_eq!(out, "hello\n");
}

#[test]
fn break_and_continue_carry_theirs() {
    let out = ran(
        "jump-guards-loop",
        "fn main() {\n\
         \x20   let mut seen = 0\n\
         \x20   for n in 0..<10 {\n\
         \x20       continue if n % 2 != 0\n\
         \x20       break if n > 6\n\
         \x20       seen = seen + n\n\
         \x20   }\n\
         \x20   println(f\"{seen}\")\n\
         }\n",
    );
    // 0 + 2 + 4 + 6; 8 breaks.
    assert_eq!(out, "12\n");
}

#[test]
fn a_throw_carries_its_condition() {
    let out = ran(
        "jump-guards-throw",
        "enum SpeedError {\n\
         \x20   TooFast(i64),\n\
         }\n\
         \n\
         impl Error for SpeedError {\n\
         \x20   fn message(ref self) -> String {\n\
         \x20       match self {\n\
         \x20           SpeedError::TooFast(n) => f\"too fast: {n}\"\n\
         \x20       }\n\
         \x20   }\n\
         }\n\
         \n\
         fn check(speed: i64) -> i64 throws {\n\
         \x20   throw SpeedError::TooFast(speed) if speed > 250\n\
         \x20   return speed\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let a = check(100) catch { 0 }\n\
         \x20   let b = check(300) catch { 0 - 1 }\n\
         \x20   println(f\"{a} {b}\")\n\
         }\n",
    );
    assert_eq!(out, "100 -1\n");
}

/// **D2: where a block follows the condition, the `if` is the value.**
#[test]
fn a_return_of_an_if_expression_is_what_it_was() {
    let out = ran(
        "jump-guards-if-value",
        "fn sign(n: i64) -> i64 {\n\
         \x20   return if n < 0 { 0 - 1 } else { 1 }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{sign(0 - 5)} {sign(5)}\")\n\
         }\n",
    );
    assert_eq!(out, "-1 1\n");
}

/// **D3: an `if` that begins a line is a statement of its own**, never the
/// condition of the jump above it.
#[test]
fn an_if_on_the_next_line_is_not_the_jumps_condition() {
    let body = main_body(
        "fn main() {\n\
         \x20   for n in 0..<3 {\n\
         \x20       break\n\
         \x20       if n > 1 { println(\"never\") }\n\
         \x20   }\n\
         }\n",
    );
    let Stmt::For { body, .. } = &body[0] else {
        panic!("a for: {body:#?}");
    };
    assert!(matches!(body.stmts[0].node, Stmt::Break), "{body:#?}");
    assert!(
        matches!(body.stmts[1].node, Stmt::Expr(Expr::If { .. })),
        "{body:#?}"
    );
}

/// **D1: the tree is the `if` it stands for**, so nothing after the parser
/// needed to learn the form.
#[test]
fn the_guard_is_an_if_around_the_jump() {
    let body = main_body("fn main() {\n    for n in 0..<3 {\n        break if n > 1\n    }\n}\n");
    let Stmt::For { body, .. } = &body[0] else {
        panic!("a for: {body:#?}");
    };
    let Stmt::Expr(Expr::If {
        then_branch,
        else_branch: None,
        ..
    }) = &body.stmts[0].node
    else {
        panic!("an if: {body:#?}");
    };
    assert!(matches!(then_branch.stmts[0].node, Stmt::Break));
}
