//! **`par_iter()`: a list walked on every core at once**
//! ([ADR-235](../../../docs/specification/adr/adr-235.md), Part II 12.6). That
//! each walk means the same at both settings of `user_parallelism` is
//! `tests/language/src/parallel_walks.nika`, run by `nikaia test`; here stay
//! what each setting lowers to and what a parallel lambda may not do.

use nikaia::contracts::LedgerOps;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
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
}
