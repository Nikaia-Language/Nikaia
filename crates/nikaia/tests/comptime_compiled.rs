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
    // Earlier bundles' runs of the same code stay beside it, under keys of
    // their own.
    assert!(!before.is_empty(), "the run was kept");
    assert!(lowered(source).contains("const T: i64 = 8642;"));
    assert_eq!(runs(&kept), before);
}

// --- ADR-321 D7, D10: what a build-time run may spend ------------------------

/// Findings under smaller bounds than a build's, so that a run past them ends
/// in a moment.
fn findings_bounded(source: &str, steps: u64, bytes: u64) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let reads = reads().bounded(nikaia::comptime_run::Bounds { steps, bytes });
    check::check_against(
        &parsed,
        &[],
        &own,
        &library,
        &BTreeSet::new(),
        &check::Newly::new(),
        &reads,
    )
    .findings
}

fn the_one(found: Vec<Finding>) -> Finding {
    let mut stopped: Vec<Finding> = found.into_iter().filter(|f| f.code == "NK1152").collect();
    assert_eq!(stopped.len(), 1, "{stopped:#?}");
    stopped.remove(0)
}

/// **A loop that does not end is refused, not waited for** (D7), naming the
/// `comptime` and the path it was in.
#[test]
fn a_loop_that_does_not_end_is_stopped_by_the_budget() {
    let source = "fn spin() -> i64 {\n\
                  \x20   let mut n = 0\n\
                  \x20   while true {\n\
                  \x20       n += 1\n\
                  \x20   }\n\
                  \x20   return n\n\
                  }\n\
                  \n\
                  comptime SPUN = spin()\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{SPUN}\")\n\
                  }\n";
    let stopped = the_one(findings_bounded(source, 1_000_000, 4 << 30));
    assert_eq!(
        stopped.message,
        "`SPUN` took more than 1000000 steps while the program was built."
    );
    assert!(
        stopped
            .notes
            .iter()
            .any(|n| n == "It was in `SPUN` > `spin`."),
        "{:#?}",
        stopped.notes
    );
}

/// **`fib(50)` is refused rather than hanging the build; `fib(20)` passes**
/// (D7): each call is a step.
#[test]
fn a_recursion_past_the_budget_is_refused_and_one_inside_it_passes() {
    let fib = "fn fib(n: i64) -> i64 {\n\
               \x20   if n < 2 {\n\
               \x20       return n\n\
               \x20   }\n\
               \x20   return fib(n - 1) + fib(n - 2)\n\
               }\n\
               \n";
    let within =
        format!("{fib}comptime F = fib(20)\n\nfn main() {{\n    println(f\"{{F}}\")\n}}\n");
    assert!(
        findings_bounded(&within, 1_000_000, 4 << 30).is_empty(),
        "fib(20) takes some 22 000 steps"
    );
    let past = format!("{fib}comptime F = fib(50)\n\nfn main() {{\n    println(f\"{{F}}\")\n}}\n");
    let stopped = the_one(findings_bounded(&past, 1_000_000, 4 << 30));
    assert_eq!(
        stopped.message,
        "`F` took more than 1000000 steps while the program was built."
    );
}

/// **A recursion with no base case meets the call depth** (D7) before the
/// budget, and is named as that.
#[test]
fn a_recursion_without_a_base_case_meets_the_call_depth() {
    let source = "fn down(n: i64) -> i64 {\n\
                  \x20   return down(n + 1)\n\
                  }\n\
                  \n\
                  comptime D = down(0)\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{D}\")\n\
                  }\n";
    let stopped = the_one(findings(source));
    assert_eq!(
        stopped.message,
        "`D` went more than 10000 calls deep while the program was built."
    );
}

/// **A list that grows without end is stopped by the memory bound** (D10),
/// counted as the run asks for it.
#[test]
fn a_list_that_grows_without_end_is_stopped_by_the_memory_bound() {
    let source = "fn grow() -> i64 {\n\
                  \x20   let mut xs: Vec[i64] = []\n\
                  \x20   while true {\n\
                  \x20       xs.push(1)\n\
                  \x20   }\n\
                  \x20   return xs.len()\n\
                  }\n\
                  \n\
                  comptime G = grow()\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{G}\")\n\
                  }\n";
    let stopped = the_one(findings_bounded(source, 10_000_000_000, 100_000_000));
    assert_eq!(
        stopped.message,
        "`G` held more than 100000000 bytes at once while the program was built."
    );
}

/// **A method is called as a function is** (ADR-321 D2): one of `std`'s on
/// text, which the interpreter could not run because its body is Rust, and
/// one of the program's own.
#[test]
fn a_method_runs_at_build_time_whoever_declares_it() {
    let source = "struct Point {\n\
                  \x20   x: i64,\n\
                  \x20   y: i64,\n\
                  }\n\
                  \n\
                  impl Point {\n\
                  \x20   fn sum(self) -> i64 {\n\
                  \x20       return self.x + self.y\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  comptime LOUD = \"abc\".to_uppercase()\n\
                  comptime S = Point { x: 3, y: 4 }.sum()\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{LOUD} {S}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("const LOUD: &str = \"ABC\";"), "{rust}");
    assert!(rust.contains("const S: i64 = 7;"), "{rust}");
}

/// **A program of several files** (Part I 9.1): the callee is in another
/// file, which is lowered with the one the `comptime` stands in; a stop there
/// is named by the function it is in.
#[test]
fn a_callee_in_another_file_runs_and_a_stop_there_is_named() {
    let main = parse_to_ast(
        "comptime T = triple(14)\n\
         comptime C = sq(100000)\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{T} {C}\")\n\
         }\n",
    )
    .expect("the source parses");
    let helpers = parse_to_ast(
        "fn triple(n: i64) -> i64 {\n\
         \x20   return n * 3\n\
         }\n\
         \n\
         fn sq(x: i32) -> i32 {\n\
         \x20   return x * x\n\
         }\n",
    )
    .expect("the source parses");
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let own = Ledger::infer_package(&[&main, &helpers], &library);
    let found = check::check_against(
        &main,
        &[&main, &helpers],
        &own,
        &library,
        &BTreeSet::new(),
        &check::Newly::new(),
        &reads(),
    )
    .findings;
    let stopped = the_one(found);
    assert_eq!(
        stopped.message,
        "`C` stopped while the program was built: attempt to multiply with overflow."
    );
    assert!(
        stopped
            .notes
            .iter()
            .any(|n| n == "It stopped in `sq`, in another file of the program."),
        "{:#?}",
        stopped.notes
    );
}
