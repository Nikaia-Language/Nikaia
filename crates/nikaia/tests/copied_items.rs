//! **A value read through a view is the value**
//! ([ADR-231](../../../docs/specification/adr/adr-231.md)): a walk over views
//! of numbers yields the numbers, `keys()` and `values()` yield views, and a
//! `filter` lambda's parameter is the item - each compiled and **run** at both
//! settings of `user_parallelism`. Every program here was `rustc`'s error
//! before: a reference below where the program reads a value.

mod common;

use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

const APP_CONF: &str = "# settings\nhost = example.org\nport = 8080\ninclude = extra.conf\n";
const EXTRA_CONF: &str = "mode = fast\nhost = override.org\n";

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
}

/// Compile the lowering and run it in a directory holding the two files.
fn ran(purpose: &str, source: &str, how: Build) -> String {
    let rust = lowered(source, how);
    let dir = common::scratch_dir(&format!("copied-items-{purpose}"));
    std::fs::write(dir.join("app.conf"), APP_CONF).expect("write app.conf");
    std::fs::write(dir.join("extra.conf"), EXTRA_CONF).expect("write extra.conf");
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
    let out = Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run it");
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

const ITEMS: &str = r##"use std::fs
use std::collections

struct P {
    x: i64,
}

fn main() throws {
    let text = fs::read_to_string("app.conf", fs::Root::Anywhere)
    let xs: Vec[i64] = [1, 2, 3, 4]
    let mut n = 0
    for x in xs.iter() {
        if x > 2 {
            n += 1
        }
    }
    let m = xs.iter().map(fn(x) { x * 10 }).filter(fn(x) { x > 15 }).count()
    let a = "a-b".chars().filter(fn(c) { c != '-' }).count()
    let c = text.lines().filter(fn(l) { l != "port = 8080" }).count()
    let owned: Vec[String] = [f"a", f"bb"]
    let d = owned.iter().filter(fn(s) { s.len() > 1 && s != "x" }).count()
    let ps: Vec[P] = [P { x: 1 }, P { x: 5 }]
    let e = ps.iter().filter(fn(p) { p.x > 2 }).count()
    let mut h: collections::HashMap[String, i64] = collections::HashMap()
    h[f"a"] = 1
    h[f"b"] = 7
    let k = h.keys().filter(fn(k) { k != "b" }).count()
    let v = h.values().filter(fn(v) { v > 3 }).count()
    println(f"{n} {m} {a} {c} {d} {e} {k} {v}")
}
"##;

#[test]
fn a_value_read_through_a_view_is_the_value() {
    runs("items", ITEMS, "2 3 2 3 1 1 1 1");
    let rust = lowered(ITEMS, Build::default());
    assert!(rust.contains("xs.iter().copied()"), "{rust}");
    assert!(rust.contains(".filter(|&c| "), "{rust}");
}
