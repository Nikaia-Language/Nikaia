//! **`par_iter()`: a list walked on every core at once**
//! ([ADR-235](../../../docs/specification/adr/adr-235.md), Part II 12.6). Each
//! program is compiled and **run** at both settings of `user_parallelism` and
//! prints the same at each; what a parallel lambda may not do is refused at
//! both.

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
    let dir = common::scratch_dir(&format!("parallel-walks-{purpose}"));
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

fn refused(source: &str, code: &str) -> nikaia::check::Finding {
    let found = findings(source);
    found
        .iter()
        .find(|f| f.code == code)
        .cloned()
        .unwrap_or_else(|| panic!("no {code}: {found:#?}"))
}

const WALKS: &str = "fn weight(x: i64) -> i64 {\n\
    \x20   return x * x\n\
    }\n\
    \n\
    fn main() {\n\
    \x20   let xs: Vec[i64] = [1, 2, 3, 4, 5]\n\
    \x20   let big = xs.par_iter().map(fn(x) { weight(x) }).filter(fn(y) { y > 4 }).collect()\n\
    \x20   println(f\"{big.len()} {big[0]} {big[2]}\")\n\
    \x20   println(f\"{xs.par_iter().filter(fn(x) { x % 2 == 1 }).count()}\")\n\
    \x20   let total = SharedMut(0)\n\
    \x20   xs.par_iter().for_each(fn(x) { total.update fn(mut t) { t += x } })\n\
    \x20   println(f\"{total.get()}\")\n\
    \x20   let walk = xs.par_iter()\n\
    \x20   walk.for_each(fn(x) { total.update fn(mut t) { t += x } })\n\
    \x20   println(f\"{total.get()}\")\n\
    \x20   for y in xs.par_iter().map(fn(x) { x + 1 }).take(2) {\n\
    \x20       println(f\"{y}\")\n\
    \x20   }\n\
    \x20   let words: Vec[String] = [f\"a\", f\"bb\"]\n\
    \x20   println(words.par_iter().map(fn(w) { w.len() }).join(\",\"))\n\
    }\n";

/// **D1: the walks, the order of a `collect`, a `for` over one, and a
/// `SharedMut` changed through `update`** - at `no` the list's `iter()`, at
/// `yes` every core, and the same lines at both.
#[test]
fn a_parallel_walk_means_the_same_at_both_settings() {
    runs("walks", WALKS, "3 9 25\n3\n15\n30\n2\n3\n1,2");
    let at_no = lowered(WALKS, Build::default());
    assert!(at_no.contains("xs.iter()"), "{at_no}");
    assert!(!at_no.contains("nikaia_std::par"), "{at_no}");
    let at_yes = lowered(WALKS, Build::parallel());
    assert!(at_yes.contains("nikaia_std::par::iter(&xs)"), "{at_yes}");
}

/// **D2: a parallel lambda may not pause** - `NK2209`, Part II 12.6's demand
/// in the code Part III gives it.
#[test]
fn a_pause_in_a_parallel_lambda_is_refused() {
    let finding = refused(
        "use std::time\n\
         \n\
         fn slow(x: i64) -> i64 {\n\
         \x20   time::sleep(1.millis())\n\
         \x20   return x\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let xs: Vec[i64] = [1, 2]\n\
         \x20   let ys = xs.par_iter().map(fn(x) { slow(x) }).collect()\n\
         \x20   println(f\"{ys.len()}\")\n\
         }\n",
        "NK2209",
    );
    assert!(finding.message.contains("several cores"), "{finding:#?}");
}

/// **D2: nor change a name outside it** - `NK2107`, by an assignment or by a
/// method that changes what it is called on. A name of its own is its own.
#[test]
fn a_parallel_lambda_that_changes_a_shared_name_is_refused() {
    for body in [
        "    let mut sum = 0\n    xs.par_iter().for_each(fn(x) { sum += x })\n",
        "    let mut seen: Vec[i64] = []\n    xs.par_iter().for_each(fn(x) { seen.push(x) })\n",
    ] {
        let source = format!(
            "fn main() {{\n    let xs: Vec[i64] = [1, 2]\n{body}    println(\"done\")\n}}\n"
        );
        let finding = refused(&source, "NK2107");
        assert!(
            finding.message.contains("and they all share `"),
            "{finding:#?}"
        );
    }
    runs(
        "own-name",
        "fn main() {\n\
         \x20   let xs: Vec[i64] = [1, 2]\n\
         \x20   let ys = xs.par_iter().map(fn(x) {\n\
         \x20       let mut y = x\n\
         \x20       y += 1\n\
         \x20       return y\n\
         \x20   }).collect()\n\
         \x20   println(f\"{ys[1]}\")\n\
         }\n",
        "3",
    );
}

/// **§3: `join` over a plain walk**, which had no counterpart below: a `map`
/// is not a list, and the language below's `join` is a list's.
#[test]
fn a_plain_walk_joins() {
    runs(
        "join",
        "fn main() {\n\
         \x20   let xs: Vec[i64] = [1, 2, 3]\n\
         \x20   println(xs.iter().map(fn(x) { x * 2 }).join(\"-\"))\n\
         }\n",
        "2-4-6",
    );
}
