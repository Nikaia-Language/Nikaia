//! **A `comptime` that calls a function is compiled and run**
//! ([ADR-321](../../../docs/specification/adr/adr-321.md) D1, #468).
//!
//! The initialiser and what it calls are lowered as the program is, compiled
//! for the machine that builds and run there, so the build-time answer is the
//! run-time one: an `i32` that overflows stops the build where it would stop
//! the program, and a function of `std`'s Rust half runs as it does in the
//! program.
//!
//! **One workshop for the whole file**, under Cargo's own scratch directory:
//! the library build-time code links against is built once and kept, as a
//! build keeps it beside its cache.

mod common;

use nikaia::assets::Reads;
use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn workshop() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("comptime-compiled")
}

fn reads() -> Reads {
    Reads::at(workshop()).building_in(workshop())
}

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_against(
        &parsed,
        &[],
        &own,
        &library,
        &BTreeSet::new(),
        &check::Newly::new(),
        &reads(),
    )
    .findings
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    nikaia::emit::emit_program_reading(&parsed, Default::default(), &reads())
        .expect("it lowers")
        .rust
}

/// **A call is compiled and run**, and the value is what the program computes.
#[test]
fn a_call_is_compiled_and_run_while_the_program_is_built() {
    let source = "fn fib(n: i64) -> i64 {\n\
                  \x20   if n < 2 {\n\
                  \x20       return n\n\
                  \x20   }\n\
                  \x20   return fib(n - 1) + fib(n - 2)\n\
                  }\n\
                  \n\
                  comptime FIB_10: i64 = fib(10)\n\
                  comptime FIB_20 = fib(20)\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{FIB_10} {FIB_20}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("const FIB_10: i64 = 55;"), "{rust}");
    assert!(rust.contains("const FIB_20: i64 = 6765;"), "{rust}");
}

/// **The program's own types hold at build time** (ADR-321 D1): an `i32`
/// that overflows stops the build at the multiplication, in the program's
/// words, where the interpreter worked the number out in a wider one.
#[test]
fn an_overflow_stops_the_build_where_it_would_stop_the_program() {
    let source = "fn sq(x: i32) -> i32 {\n\
                  \x20   return x * x\n\
                  }\n\
                  \n\
                  comptime C = sq(100000)\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{C}\")\n\
                  }\n";
    let found = findings(source);
    let stopped = found
        .iter()
        .find(|f| f.code == "NK1152")
        .unwrap_or_else(|| panic!("the overflow stops the build: {found:#?}"));
    assert_eq!(
        stopped.message,
        "`C` stopped while the program was built: attempt to multiply with overflow."
    );
    let at = stopped
        .labels
        .iter()
        .find(|label| label.main)
        .expect("the place it stopped is shown");
    assert_eq!(&source[at.span.start as usize..][..12], "return x * x");
}

/// **`std`'s Rust half runs at build time** (ADR-321 D2): a callee that asks
/// `text::digit_value` was `NK1127` while an interpreter could only run code
/// written in the program.
#[test]
fn a_function_of_std_runs_inside_a_callee() {
    let source = "use std::text\n\
                  \n\
                  fn seven() -> i32 {\n\
                  \x20   return text::digit_value('7')\n\
                  }\n\
                  \n\
                  comptime S = seven()\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{S}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(lowered(source).contains("const S: i32 = 7;"));
}

/// **The rule is asked before anything is compiled** (D2): a callee that
/// prints may not run while the program is built.
#[test]
fn a_callee_that_prints_is_refused_before_it_is_compiled() {
    let source = "fn loud() -> i64 {\n\
                  \x20   println(\"hi\")\n\
                  \x20   return 1\n\
                  }\n\
                  \n\
                  comptime L = loud()\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{L}\")\n\
                  }\n";
    let found = findings(source);
    assert!(
        found.iter().any(|f| f.code == "NK1152"
            && f.message == "You can't call `loud` while the program is built."),
        "{found:#?}"
    );
}

/// **An unchanged `comptime` is neither compiled nor run again** (D4): its
/// answer is kept under a key of the code that produced it.
#[test]
fn an_unchanged_comptime_is_answered_from_what_was_kept() {
    let source = "fn twice(n: i64) -> i64 {\n\
                  \x20   return n * 2\n\
                  }\n\
                  \n\
                  comptime T = twice(4321)\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{T}\")\n\
                  }\n";
    assert!(lowered(source).contains("const T: i64 = 8642;"));
    let kept = workshop().join("comptime");
    let runs = |dir: &PathBuf| -> Vec<(PathBuf, std::time::SystemTime)> {
        let mut out: Vec<_> = std::fs::read_dir(dir)
            .expect("the kept runs")
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let answer = entry.path().join("answer");
                let read = std::fs::read_to_string(&answer).ok()?;
                (read == "(i 8642)").then(|| {
                    let modified = std::fs::metadata(entry.path().join("run"))
                        .and_then(|m| m.modified())
                        .expect("the run's binary");
                    (entry.path(), modified)
                })
            })
            .collect();
        out.sort();
        out
    };
    let before = runs(&kept);
    assert_eq!(before.len(), 1, "{before:?}");
    assert!(lowered(source).contains("const T: i64 = 8642;"));
    assert_eq!(runs(&kept), before);
}
