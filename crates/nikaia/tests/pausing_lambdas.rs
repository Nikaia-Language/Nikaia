//! **A lambda that pauses, where `std` takes one**
//! ([ADR-233](../../../docs/specification/adr/adr-233.md)): `map` and `filter`
//! hand back a sequence whose step pauses, a list's `map`, `sort_by_key`,
//! `or_insert_with` and `and_modify` await the lambda where they would have
//! called it, and a door refuses a pause by the language's own rule. Each
//! program is compiled and **run** at both settings of `user_parallelism`;
//! every one of them was refused by the lowering before, as *not something
//! this compiler can build yet*.

mod common;

use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
}

fn ran(purpose: &str, source: &str, how: Build) -> String {
    let rust = lowered(source, how);
    let dir = common::scratch_dir(&format!("pausing-lambdas-{purpose}"));
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

/// **D1: `map` and `filter` hand back a sequence whose step pauses**, and every
/// walk of it is the one a pausing sequence has: `collect`, `count`, `nth` and
/// `join` are awaited, and a `for` gives its thread up at each step. A plain
/// lambda after it, and a second pausing one, chain onto it.
#[test]
fn map_and_filter_with_a_pausing_lambda_make_a_pausing_sequence() {
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
    runs("map-filter", &source, "6\n2\n4,8,12\n2\n4\n2\n4\n6");
    let rust = lowered(&source, Build::default());
    assert!(rust.contains("nikaia_std::seq::then("), "{rust}");
    assert!(rust.contains("nikaia_std::seq::then_filter("), "{rust}");
    assert!(rust.contains("async |x|"), "{rust}");
    assert!(rust.contains(".then(async |y|"), "{rust}");
}

/// **D1: the lazy walks of the sequence**, which it has as adapters of its own,
/// and one kept in a name before it is walked. Text items are handed to the
/// lambda as views.
#[test]
fn the_pausing_sequence_has_the_lazy_walks_and_can_wait_in_a_name() {
    runs(
        "lazy",
        &program(
            "    let xs: Vec[i64] = [1, 2, 3, 4, 5, 6]\n\
             \x20   let r = xs.iter().map(fn(x) { slow(x) }).skip(1).step_by(2).zip(xs.iter()).take(2).collect()\n\
             \x20   println(f\"{r.len()}\")\n\
             \x20   let lazy = xs.iter().filter(fn(x) { slow(x) > 8 })\n\
             \x20   for y in lazy {\n\
             \x20       println(f\"{y}\")\n\
             \x20   }\n\
             \x20   let words: Vec[String] = [f\"ab\", f\"abc\", f\"abcd\"]\n\
             \x20   println(f\"{words.iter().filter(fn(w) { slow_len(w) > 2 }).count()}\")\n\
             \x20   let c = xs.iter().map(fn(x) { xs.iter().map(fn(y) { slow(y + x) }).count() }).collect()\n\
             \x20   println(f\"{c[0]}\")\n",
        ),
        "2\n5\n6\n2\n6",
    );
}

/// **D2: the eager entries await the lambda where they would have called it.**
#[test]
fn the_eager_entries_await_a_pausing_lambda() {
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
    runs("eager", &source, "6\n3\n12");
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

/// **D4: a view of a value that copies goes where the value is wanted**, which
/// is what the lambda `sort_by_key` hands over meets first; and a view of a
/// `String` goes where a view of text is wanted.
#[test]
fn a_view_of_a_copy_and_of_a_string_are_handed_as_arguments() {
    runs(
        "views",
        "fn quick(x: i64) -> i64 {\n\
         \x20   return x\n\
         }\n\
         \n\
         fn quick_len(w: ref String) -> i64 {\n\
         \x20   return w.len()\n\
         }\n\
         \n\
         fn own_len(w: String) -> i64 {\n\
         \x20   return w.len()\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let mut xs: Vec[i64] = [3, 1, 2]\n\
         \x20   xs.sort_by_key(fn(x) { quick(0 - x) })\n\
         \x20   let words: Vec[String] = [f\"ab\", f\"abc\"]\n\
         \x20   for w in words.iter() {\n\
         \x20       println(f\"{quick_len(w)} {own_len(w)}\")\n\
         \x20   }\n\
         \x20   println(f\"{xs[0]}\")\n\
         }\n",
        "2 2\n3 3\n3",
    );
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
