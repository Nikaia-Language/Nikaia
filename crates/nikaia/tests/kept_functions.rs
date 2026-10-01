//! **A function value that is kept** — in a field, a result, a `let`, or a
//! parameter the callee stores (Part I 5.3, Part II 12.3).

mod common;

use nikaia::contracts::LedgerOps;
use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
}

fn ran(purpose: &str, source: &str, how: Build) -> String {
    let rust = lowered(source, how);
    let dir = common::scratch_dir(&format!("kept-{purpose}"));
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
    let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    std::fs::remove_dir_all(&dir).ok();
    printed
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

/// Both settings, the same output, and no refusal on the way.
fn runs(purpose: &str, source: &str, expected: &str) {
    assert!(
        findings(source).is_empty(),
        "{purpose}: {:#?}",
        findings(source)
    );
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran(purpose, source, how), expected, "{purpose} at {how:?}");
    }
}

/// **A function value kept in a field, a result and a `let`**: one shared
/// closure below, called through the field and by name.
#[test]
fn a_function_value_is_kept_in_a_field_a_result_and_a_let() {
    runs(
        "sync",
        "struct Button { label: String, on_click: fn(i64) -> i64 sync }\n\n\
         fn adder(n: i64) -> fn(i64) -> i64 sync {\n\
         \x20   return fn(x) { x + n }\n\
         }\n\n\
         fn main() {\n\
         \x20   let b = Button { label: \"ok\", on_click: fn(x) { x * 2 } }\n\
         \x20   let add: fn(i64) -> i64 sync = adder(3)\n\
         \x20   let twice: fn(i64) -> i64 sync = fn(x) { x + x }\n\
         \x20   println(f\"{b.label} {b.on_click(4)} {add(1)} {twice(5)}\")\n\
         }\n",
        "ok 8 4 10",
    );
}

/// **A kept function that may pause** hands back a boxed future, and a call
/// through the field waits for it.
#[test]
fn a_kept_function_that_may_pause_is_awaited() {
    runs(
        "pausing",
        "use std::time\n\n\
         struct Job { name: String, step: fn(i64) -> i64 }\n\n\
         fn main() {\n\
         \x20   let job = Job { name: \"tick\", step: fn(n) {\n\
         \x20       time::sleep(time::Duration::from_millis(1))\n\
         \x20       n + 1\n\
         \x20   } }\n\
         \x20   println(f\"{job.name} {job.step(41)}\")\n\
         }\n",
        "tick 42",
    );
}

/// **A parameter the callee keeps** is the same kept value: a handler handed
/// in and stored in a field, then called from there.
#[test]
fn a_kept_parameter_is_stored_in_a_field() {
    runs(
        "stored",
        "struct Router { handler: fn(i64) -> i64 sync }\n\n\
         fn route(handler: fn(i64) -> i64 sync) -> Router {\n\
         \x20   return Router { handler: handler }\n\
         }\n\n\
         fn main() {\n\
         \x20   let r = route(fn(x) { x * 10 })\n\
         \x20   println(f\"{r.handler(7)}\")\n\
         }\n",
        "70",
    );
}

/// **A pausing function held in a `let`** is awaited where it is called.
#[test]
fn a_pausing_function_in_a_let_is_awaited() {
    runs(
        "let-pausing",
        "use std::time\n\n\
         fn main() {\n\
         \x20   let step: fn(i64) -> i64 = fn(n) {\n\
         \x20       time::sleep(time::Duration::from_millis(1))\n\
         \x20       n * 3\n\
         \x20   }\n\
         \x20   println(f\"{step(5)}\")\n\
         }\n",
        "15",
    );
}

/// **A kept value handed to a parameter that only runs it** is lent to it.
#[test]
fn a_kept_value_is_handed_to_a_run_parameter() {
    runs(
        "handed-on",
        "fn apply(f: fn(i64) -> i64 sync, x: i64) -> i64 {\n\
         \x20   return f(x)\n\
         }\n\n\
         fn main() {\n\
         \x20   let add: fn(i64) -> i64 sync = fn(x) { x + 1 }\n\
         \x20   println(f\"{apply(add, 1)} {apply(add, 2)}\")\n\
         }\n",
        "2 3",
    );
}

/// **A field whose function may fail** fails the function that calls it.
#[test]
fn a_kept_function_that_may_fail_is_propagated() {
    runs(
        "fails",
        "struct Parser { parse: fn(ref String) -> i64 sync throws }\n\n\
         fn run(p: ref Parser) -> i64 throws {\n\
         \x20   return p.parse(\"12\") + 1\n\
         }\n\n\
         fn main() {\n\
         \x20   let p = Parser { parse: fn(t) { t.len() as i64 } }\n\
         \x20   let n = run(p) catch { 0 }\n\
         \x20   println(f\"{n}\")\n\
         }\n",
        "3",
    );
}

/// **A kept function handed to a task** travels with it, at both settings.
#[test]
fn a_kept_function_travels_into_a_task() {
    runs(
        "task",
        "fn holds(f: fn() -> String) {\n\
         \x20   let t = spawn fn { f() }\n\
         \x20   println(t.join())\n\
         }\n\n\
         fn main() {\n\
         \x20   holds(fn() { \"from the task\" })\n\
         }\n",
        "from the task",
    );
}

/// **A named function where a function value is kept** is the closure that
/// calls it, lent its arguments as a written call is.
#[test]
fn a_named_function_is_kept_as_the_closure_that_calls_it() {
    runs(
        "named",
        "struct Tools { double: fn(i64) -> i64 sync, measure: fn(String) -> i64 sync }\n\n\
         fn double(x: i64) -> i64 { return x * 2 }\n\
         fn measure(text: String) -> i64 { return text.len() as i64 }\n\n\
         fn main() {\n\
         \x20   let t = Tools { double: double, measure: measure }\n\
         \x20   println(f\"{t.double(21)} {t.measure(\"four\")}\")\n\
         }\n",
        "42 4",
    );
}
