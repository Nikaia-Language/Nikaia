//! **An option's default is a build-time value** (ADR-318, #374): any
//! expression a `comptime` may hold, evaluated once where the function is
//! declared, with the ledger recording its value; refused with a `comptime`'s
//! codes where it cannot be computed.

mod common;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn codes(source: &str) -> Vec<String> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
        .into_iter()
        .map(|f| f.code.to_string())
        .collect()
}

fn ran(source: &str) -> String {
    assert!(codes(source).is_empty(), "{:?}", codes(source));
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust;
    let dir = common::scratch_dir("option-defaults");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8_lossy(&run.stdout).to_string()
}

/// D1: arithmetic, a call to a function of the program reading an item-level
/// `comptime`, and text joined - each computed once, and a call that leaves
/// the option out receives the value.
#[test]
fn a_computed_default_is_evaluated_once_and_handed_to_the_call() {
    let source = "comptime BASE = 7\n\
         \n\
         fn twice(n: i64) -> i64 {\n\
         \x20   return n * 2\n\
         }\n\
         \n\
         fn wait(label: ref String; timeout: i64 = 30 * 1000, tries: i64 = twice(BASE), name: ref String = \"a\" + \"b\") -> String {\n\
         \x20   return f\"{label} {timeout} {tries} {name}\"\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(wait(\"x\"))\n\
         \x20   println(wait(\"y\"; tries: 1))\n\
         }\n";
    assert_eq!(ran(source), "x 30000 14 ab\ny 30000 1 ab\n");
    let parsed = parse_to_ast(source).expect("the source parses");
    let ledger = Ledger::infer(&parsed).render();
    assert!(ledger.contains("timeout: i64 = 30000"), "{ledger}");
    assert!(ledger.contains("tries: i64 = 14"), "{ledger}");
}

/// D7: a default whose callee touches the world is `NK1152`, as a `comptime`'s.
#[test]
fn a_default_that_cannot_run_at_build_time_is_refused() {
    let source = "fn loud() -> i64 {\n\
         \x20   println(\"side effect\")\n\
         \x20   return 1\n\
         }\n\
         \n\
         fn a(t: i64 = loud()) -> i64 {\n\
         \x20   return t\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{a()}\")\n\
         }\n";
    assert_eq!(codes(source), vec!["NK1152".to_string()]);
}

/// A literal default is what it always was: nothing evaluated, nothing refused.
#[test]
fn a_literal_default_is_unchanged() {
    let source = "fn a(t: i64 = -3, s: ref String = \"x\") -> String {\n\
         \x20   return f\"{t}{s}\"\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(a())\n\
         }\n";
    assert_eq!(ran(source), "-3x\n");
}

/// **A function with options is called at build time** (#374 step 2): what
/// the caller names is used, and what it leaves out takes the default.
#[test]
fn a_function_with_options_runs_at_build_time() {
    let source = "fn base(scale: i64 = 10) -> i64 {\n\
         \x20   return scale * 3\n\
         }\n\
         \n\
         comptime A = base()\n\
         comptime B = base(scale: 2)\n\
         \n\
         fn f(x: i64; t: i64 = base()) -> i64 {\n\
         \x20   return x + t\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{A} {B} {f(1)}\")\n\
         }\n";
    assert_eq!(ran(source).trim(), "30 6 31");
}

/// **A default that needs itself is a ring** (ADR-318, `NK1168`), said once,
/// and not `NK1127`'s *it comes from a package* about a function of the
/// program.
#[test]
fn a_default_that_needs_itself_is_a_ring() {
    let source = "comptime X = f()\n\
         \n\
         fn f(t: i64 = X) -> i64 {\n\
         \x20   return t + 1\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{f()}\")\n\
         }\n";
    assert_eq!(codes(source), vec!["NK1168".to_string()]);
}
