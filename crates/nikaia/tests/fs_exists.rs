//! **`fs::exists`: whether a name is there** (Part III 17.1, under
//! [ADR-108](../../../docs/specification/adr/adr-108.md)'s root, #454).
//!
//! The program runs over a tree this file lays out: a name inside the root is
//! there or not, a name that leaves the root is refused and not `false`, a
//! call with no root is `NK1101`, and `--trust` lists an `Anywhere`.

mod common;

use nikaia::contracts::LedgerOps;
use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

/// Lowered at `how`, compiled, and run in a directory holding `tree/`.
fn ran(purpose: &str, source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, how).expect("it lowers").rust;
    let dir = common::scratch_dir(&format!("fs-exists-{purpose}"));
    for (name, text) in [
        ("tree/b.rs", "b"),
        ("tree/src/a.rs", "a"),
        ("tree/src/deep/c.txt", "c"),
    ] {
        let at = dir.join(name);
        std::fs::create_dir_all(at.parent().expect("a parent")).expect("scratch");
        std::fs::write(at, text).expect("write");
    }
    std::fs::create_dir_all(dir.join("tree/empty")).expect("scratch");
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &["--crate-type", "bin", "-o", &binary.to_string_lossy()],
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
    std::fs::remove_dir_all(&dir).ok();
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn runs(purpose: &str, source: &str, expected: &str) {
    let found = findings(source);
    assert!(found.is_empty(), "{purpose}: {found:#?}");
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran(purpose, source, how), expected, "{purpose} at {how:?}");
    }
}

/// **There, and not there**, inside the root and anywhere.
#[test]
fn exists_says_whether_a_name_is_there() {
    let source = "use std::fs\n\
                  \n\
                  fn main() throws {\n\
                  \x20   let root = fs::Root::Dir(\"tree\")\n\
                  \x20   println(f\"{fs::exists(\"b.rs\", root)} {fs::exists(\"src/deep\", root)} {fs::exists(\"nope.rs\", root)}\")\n\
                  \x20   if !fs::exists(\"tree/b.rs\", fs::Root::Anywhere) {\n\
                  \x20       println(\"lost\")\n\
                  \x20   }\n\
                  }\n";
    runs("there", source, "true true false\n");
}

/// **A name that leaves the root is refused, not absent** (ADR-108 D1): the
/// call throws `Outside`, and the refusal is caught like any `io::IoError`.
#[test]
fn exists_out_of_its_root_is_refused() {
    let source = "use std::fs\n\
                  \n\
                  fn main() {\n\
                  \x20   let there = fs::exists(\"../program.rs\", fs::Root::Dir(\"tree\")) catch {\n\
                  \x20       println(\"refused\")\n\
                  \x20       return\n\
                  \x20   }\n\
                  \x20   println(f\"there {there}\")\n\
                  }\n";
    runs("outside", source, "refused\n");
}

/// **The root is a subject, with no default** (ADR-108 D1): a call that
/// leaves it out is `NK1101`, and the help names both forms.
#[test]
fn exists_with_no_root_is_refused() {
    // Deliberately short of its root, which is the whole of the test.
    let found = findings("use std::fs\nfn main() throws { let there = fs::exists(\"x\") }\n");
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1101")
        .unwrap_or_else(|| panic!("an `exists` with no root is refused: {found:#?}"));
    let help = refusal.help.as_deref().expect("every refusal has one");
    assert!(help.contains("fs::Root::Dir(store)"), "{help}");
    assert!(help.contains("fs::Root::Anywhere"), "{help}");
}

/// **`--trust` lists an `exists` written with `Anywhere`** (ADR-108 D4).
#[test]
fn the_trust_report_lists_an_exists_anywhere() {
    let source = "use std::fs\n\
                  fn main() throws {\n\
                  \x20   let there = fs::exists(\"a\", fs::Root::Anywhere)\n\
                  }\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    let t = nikaia::contracts::trust::analyse(&parsed, &library);
    assert_eq!(t.roots.len(), 1, "{:#?}", t.roots);
    assert_eq!(t.roots[0].entry, "fs::exists");
    assert_eq!(t.roots[0].wrote, nikaia::contracts::trust::Wrote::Anywhere);
}
