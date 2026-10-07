//! **A lambda that pauses, where `std` takes one**
//! ([ADR-233](../../../docs/specification/adr/adr-233.md)): `map` and `filter`
//! hand back a sequence whose step pauses, a list's `map`, `sort_by_key`,
//! `or_insert_with` and `and_modify` await the lambda where they would have
//! called it, and a door refuses a pause by the language's own rule. Every
//! one of them was refused by the lowering before, as *not something this
//! compiler can build yet*.
//!
//! What the programs compute, at both settings of `user_parallelism`, is
//! `tests/language/src/pausing_lambdas.nika`; this file keeps what the lowering
//! writes and what is refused.

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

const SLOW: &str = "use std::time\n\
    use std::collections\n\
    \n\
    fn slow(x: i64) -> i64 {\n\
    \x20   time::sleep(1.millis())\n\
    \x20   return x * 2\n\
    }\n\
    \n\
    fn slow_len(w: ref String) -> i64 {\n\
    \x20   time::sleep(1.millis())\n\
    \x20   return w.len()\n\
    }\n\
    \n";

fn program(body: &str) -> String {
    format!("{SLOW}fn main() {{\n{body}}}\n")
}

/// **D1: `map` and `filter` hand back a sequence whose step pauses**, lowered
/// onto `seq::then` and `seq::then_filter`, and a second pausing lambda chains
/// onto the first.
#[test]
fn map_and_filter_with_a_pausing_lambda_lower_to_a_pausing_sequence() {
    let source = program(
        "    let xs: Vec[i64] = [1, 2, 3]\n\
         \x20   let a = xs.iter().map(fn(x) { slow(x) }).collect()\n\
         \x20   println(f\"{a[2]}\")\n\
         \x20   let n = xs.iter().filter(fn(x) { slow(x) > 2 }).count()\n\
         \x20   println(f\"{n}\")\n\
         \x20   println(xs.iter().map(fn(x) { slow(x) }).map(fn(y) { slow(y) }).join(\",\"))\n\
         \x20   let b = xs.iter().map(fn(x) { slow(x) }).map(fn(y) { y + 1 }).filter(fn(y) { y > 3 }).collect()\n\
         \x20   println(f\"{b.len()}\")\n\
         \x20   println(f\"{xs.iter().map(fn(x) { slow(x) }).nth(1) ?? 0}\")\n\
         \x20   for y in xs.iter().map(fn(x) { slow(x) }) {\n\
         \x20       println(f\"{y}\")\n\
         \x20   }\n",
    );
    let rust = lowered(&source, Build::default());
    assert!(rust.contains("nikaia_std::seq::then("), "{rust}");
    assert!(rust.contains("nikaia_std::seq::then_filter("), "{rust}");
    assert!(rust.contains("async |x|"), "{rust}");
    assert!(rust.contains(".then(async |y|"), "{rust}");
}

/// **D2: the eager entries await the lambda where they would have called it**,
/// through their counterparts in `seq`.
#[test]
fn the_eager_entries_lower_to_their_pausing_counterparts() {
    let source = program(
        "    let xs: Vec[i64] = [3, 1, 2]\n\
         \x20   let doubled = xs.clone().map(fn(x) { slow(x) })\n\
         \x20   println(f\"{doubled[0]}\")\n\
         \x20   let mut ys: Vec[i64] = [3, 1, 2]\n\
         \x20   ys.sort_by_key(fn(y) { slow(0 - y) })\n\
         \x20   println(f\"{ys[0]}\")\n\
         \x20   let mut m: collections::HashMap[String, i64] = collections::HashMap()\n\
         \x20   m.entry(f\"a\").or_insert_with(fn() { slow(5) })\n\
         \x20   m.entry(f\"a\").or_insert_with(fn() { slow(7) })\n\
         \x20   m.entry(f\"a\").and_modify(fn(mut v) { v += slow(1) })\n\
         \x20   let got = m[\"a\"] ?? 0\n\
         \x20   println(f\"{got}\")\n",
    );
    // A `mut` parameter is a `&mut Vec` already, and is handed on as it is.
    let rust = lowered(
        &format!(
            "{SLOW}fn sorted(mut xs: Vec[i64]) {{\n\
             \x20   xs.sort_by_key(fn(x) {{ slow(x) }})\n\
             }}\n\
             \n\
             fn main() {{\n\
             \x20   let mut xs: Vec[i64] = [3, 1, 2]\n\
             \x20   sorted(xs)\n\
             \x20   println(f\"{{xs[0]}}\")\n\
             }}\n"
        ),
        Build::default(),
    );
    assert!(rust.contains("nikaia_std::seq::sort_by_key(xs,"), "{rust}");
    let rust = lowered(&source, Build::default());
    assert!(rust.contains("nikaia_std::seq::map_list("), "{rust}");
    assert!(
        rust.contains("nikaia_std::seq::sort_by_key(&mut ys"),
        "{rust}"
    );
    assert!(rust.contains("nikaia_std::seq::or_insert_with("), "{rust}");
    assert!(rust.contains("nikaia_std::seq::and_modify("), "{rust}");
}

/// **A lambda that does not pause is lowered as it always was**: the
/// counterparts are for the special case, and the general one pays nothing.
#[test]
fn a_lambda_that_does_not_pause_keeps_the_plain_closure() {
    let rust = lowered(
        &program(
            "    let xs: Vec[i64] = [1, 2, 3]\n\
             \x20   let a = xs.iter().map(fn(x) { x * 2 }).collect()\n\
             \x20   println(f\"{a.len()}\")\n",
        ),
        Build::default(),
    );
    assert!(!rust.contains("nikaia_std::seq::"), "{rust}");
    assert!(rust.contains(".map(|x|"), "{rust}");
}

/// **D3: a pause inside a door is `NK2202`**, which Part II 12.2 has always
/// said and which nothing asked: the lock is held for as long as the wait.
#[test]
fn a_pause_inside_a_door_is_refused_by_the_language() {
    for door in [
        "    let a = SharedMut(0)\n    a.access fn(v) { let w = slow(1) }\n",
        "    let a = SharedMut(0)\n    a.update fn(mut v) { v += slow(1) }\n",
        "    let a = Locked(0)\n    a.access fn(v) { let w = slow(1) }\n",
        "    let a = SharedMut(0)\n    let b = SharedMut(1)\n    access_all(a, b) fn(x, y) { let w = slow(1) }\n",
    ] {
        let finding = refused(&program(door), "NK2202");
        assert!(
            finding.message.contains("holding a lock"),
            "{door}: {finding:#?}"
        );
    }
}

/// **What is still refused is refused by a rule of the language**: a list's
/// `map` takes the list, a pausing sequence has no back end, and it does not
/// go where a sequence that does not pause is wanted.
#[test]
fn the_pausing_sequence_is_refused_where_the_language_refuses_it() {
    let used_again = refused(
        &program(
            "    let xs: Vec[i64] = [1, 2]\n\
             \x20   let ys = xs.map(fn(x) { slow(x) })\n\
             \x20   println(f\"{xs.len()} {ys.len()}\")\n",
        ),
        "NK2105",
    );
    assert!(used_again.message.contains("`map`"), "{used_again:#?}");
    refused(
        &program(
            "    let xs: Vec[i64] = [1, 2]\n\
             \x20   println(f\"{xs.iter().map(fn(x) { slow(x) }).rev().count()}\")\n",
        ),
        "NK2703",
    );
    refused(
        &program(
            "    let xs: Vec[i64] = [1, 2]\n\
             \x20   let r = xs.iter().zip(xs.iter().map(fn(x) { slow(x) })).collect()\n\
             \x20   println(f\"{r.len()}\")\n",
        ),
        "NK1102",
    );
}
