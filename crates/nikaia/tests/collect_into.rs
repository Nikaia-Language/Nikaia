//! **`collect()` builds what the place it goes into declares**
//! ([ADR-227](../../../docs/specification/adr/adr-227.md)): a list where
//! nothing says otherwise, and a map, a set or text where a `let`, a
//! field or a result declares one. Each program is compiled and **run** at both
//! settings of `user_parallelism`.

mod common;

use nikaia::contracts::LedgerOps;
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
    let dir = common::scratch_dir(&format!("collect-into-{purpose}"));
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

/// **A map from pairs, keyed by views**, a set, an ordered set and text - each
/// declared by a `let` - and a list where nothing declares anything.
const DECLARED: &str = r##"use std::fs
use std::collections

fn main() throws {
    let text = fs::read_to_string("app.conf", fs::Root::Anywhere)
    let m: collections::HashMap[String, i64] = text.lines().map(fn(l) { (l, l.len()) }).collect()
    let s: collections::HashSet[String] = text.split(" ").collect()
    let d: collections::BTreeSet[String] = text.lines().collect()
    let t: String = "ab".chars().collect()
    let xs = text.lines().collect()
    println(f"{m.len()} {m["port = 8080"] ?? 0} {s.len()} {d.len()} {t} {xs.len()}")
}
"##;

#[test]
fn collect_builds_the_map_the_set_and_the_text_a_let_declares() {
    runs("declared", DECLARED, "4 11 6 4 ab 4");
    let rust = lowered(DECLARED, Build::default());
    assert_eq!(rust.matches(".collect::<Vec<_>>()").count(), 1, "{rust}");
}

/// **Both kinds into a map built by `collect`**: the key is mixed, so each key
/// goes in as it is (ADR-227 D2), and a result declares the map.
const MIXED_KEYS: &str = r##"use std::fs
use std::collections

fn counted(text: ref String) -> collections::HashMap[String, i64] {
    let mut m: collections::HashMap[String, i64] = text.lines().map(fn(l) { (l, 1) }).collect()
    m[f"total"] = m.len()
    return m
}

fn main() throws {
    let text = fs::read_to_string("app.conf", fs::Root::Anywhere)
    let m = counted(text)
    println(f"{m.len()} {m["total"] ?? 0}")
}
"##;

#[test]
fn a_map_both_kinds_are_collected_into_hands_each_key_over_as_it_is() {
    runs("mixed-keys", MIXED_KEYS, "5 4");
    let rust = lowered(MIXED_KEYS, Build::default());
    assert!(rust.contains(".either_keys().collect()"), "{rust}");
}

/// **What does not fit is refused, in this language's words**: pairs are not
/// a set of text.
#[test]
fn items_that_do_not_fit_the_declared_container_are_refused() {
    let found = findings(
        "use std::fs\nuse std::collections\n\n\
         fn main() throws {\n\
         \x20   let text = fs::read_to_string(\"app.conf\", fs::Root::Anywhere)\n\
         \x20   let s: collections::HashSet[String] = text.lines().map(fn(l) { (l, 1) }).collect()\n\
         \x20   println(f\"{s.len()}\")\n\
         }\n",
    );
    assert!(found.iter().any(|f| f.code == "NK1103"), "{found:#?}");
}
