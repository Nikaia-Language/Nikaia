//! **A word-sized value that crosses is the word itself**
//! ([ADR-238](../../../docs/specification/adr/adr-238.md), ADR-110 D2 and D3):
//! its `update` runs the block on a copy and compare-and-swaps it in, and no
//! lock is taken. A value a door over several locks holds, a published one and
//! one that is not a word keep the lock; each program prints the same at both
//! settings of `user_parallelism`.

mod common;

use std::process::Command;

use nikaia::contracts::sharing::{self, Count};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
}

fn ran(purpose: &str, source: &str, how: Build) -> String {
    let rust = lowered(source, how);
    let dir = common::scratch_dir(&format!("word-locks-{purpose}"));
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

fn decisions(source: &str) -> Vec<sharing::Decision> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    sharing::analyse_program(&parsed, &own, &library, true).decisions
}

fn count_of(source: &str, function: &str, value: &str) -> sharing::Decision {
    decisions(source)
        .into_iter()
        .find(|d| d.function == function && d.value == value)
        .unwrap_or_else(|| panic!("no decision for `{function}::{value}`"))
}

const PROGRAM: &str = "fn bump(c: SharedMut[i64], by: i64) {\n\
    \x20   c.update fn(mut v) { v += by }\n\
    }\n\
    \n\
    pub fn published(c: SharedMut[i64]) {\n\
    \x20   c.update fn(mut v) { v += 1 }\n\
    }\n\
    \n\
    fn main() {\n\
    \x20   let hits = SharedMut(0)\n\
    \x20   let t = spawn fn { bump(hits, 2) }\n\
    \x20   t.join()\n\
    \x20   bump(hits, 3)\n\
    \x20   let a = SharedMut(10)\n\
    \x20   let b = SharedMut(20)\n\
    \x20   let u = spawn fn { a.update fn(mut v) { v -= 1 } }\n\
    \x20   u.join()\n\
    \x20   update_all(a, b) fn(mut x, mut y) {\n\
    \x20       x -= 5\n\
    \x20       y += 5\n\
    \x20   }\n\
    \x20   let p = SharedMut(0)\n\
    \x20   let w = spawn fn { published(p) }\n\
    \x20   w.join()\n\
    \x20   let big = SharedMut(f\"text\")\n\
    \x20   let z = spawn fn { big.update fn(mut s) { s = f\"{s}!\" } }\n\
    \x20   z.join()\n\
    \x20   let xs: Vec[i64] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]\n\
    \x20   let total = SharedMut(0)\n\
    \x20   xs.par_iter().for_each(fn(x) { total.update fn(mut t) { t += x } })\n\
    \x20   println(f\"{hits.get()} {a.get()} {b.get()} {p.get()} {big.get()} {total.get()}\")\n\
    }\n";

/// **D1: the rows, and the same program at both settings.** At `no` every
/// value is the cheap shape; at `yes` a word that crosses is a word, and a
/// value passed to a function takes the function's parameter with it.
#[test]
fn a_crossing_word_is_a_word_and_the_rest_keep_their_lock() {
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran("rows", PROGRAM, how), "5 4 25 1 text! 55", "at {how:?}");
    }
    let rust = lowered(PROGRAM, Build::parallel());
    for word in [
        "fn bump(c: std::sync::Arc<nikaia_std::lock::Word<i64>>",
        "let hits = std::sync::Arc::new(nikaia_std::lock::Word::new(0))",
        "let total = std::sync::Arc::new(nikaia_std::lock::Word::new(0))",
    ] {
        assert!(rust.contains(word), "{word}\n{rust}");
    }
    for kept in [
        "let a = std::sync::Arc::new(nikaia_std::lock::Crossing::new(10))",
        "pub fn published(c: std::sync::Arc<nikaia_std::lock::Crossing<i64>>",
        "let big = std::sync::Arc::new(nikaia_std::lock::Crossing::new(",
    ] {
        assert!(rust.contains(kept), "{kept}\n{rust}");
    }
    assert!(!lowered(PROGRAM, Build::default()).contains("lock::Word"));
}

/// **D2: `--sharing` names the row, and why a word kept its lock.**
#[test]
fn the_explanation_names_the_row() {
    let hits = count_of(PROGRAM, "main", "hits");
    assert_eq!(hits.count, Count::Word);
    let a = count_of(PROGRAM, "main", "a");
    assert_eq!(a.count, Count::Atomic);
    assert!(
        a.kept_its_lock
            .is_some_and(|why| why.contains("a door over several locks")),
        "{a:#?}"
    );
    let report = {
        let parsed = parse_to_ast(PROGRAM).expect("the source parses");
        let own = Ledger::infer(&parsed);
        let library = Ledger::parse(STD).expect("std ships a ledger");
        sharing::report(&parsed, &own, &library, true)
    };
    assert!(report.contains("word    `hits`"), "{report}");
    assert!(report.contains("compare-and-swap"), "{report}");
}

/// **D3: a `get` that copies a value that does not copy cheaply is named.**
#[test]
fn a_get_of_a_large_value_is_named() {
    let big = count_of(PROGRAM, "main", "big");
    assert_eq!(
        big.copied_out,
        ["`big.get()` copies the whole `String` out of the lock"]
    );
    assert!(count_of(PROGRAM, "main", "hits").copied_out.is_empty());
}
