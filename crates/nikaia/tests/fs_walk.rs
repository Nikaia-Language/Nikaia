//! **`fs::walk`: every file under a directory**
//! ([ADR-290](../../../docs/specification/adr/adr-290.md) D14, under
//! [ADR-108](../../../docs/specification/adr/adr-108.md)'s root).
//!
//! The tests here **run a program** over a tree this file lays out, at both
//! settings of `user_parallelism`: the names come back relative and sorted, the
//! directories are left out, a name that leaves the root is refused, and the
//! refusal is a `catch`.

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
    let dir = common::scratch_dir(&format!("fs-walk-{purpose}"));
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

/// **Files only, relative to the directory asked for, `/` between the parts,
/// sorted** — and each name is one the program can open under the same root.
#[test]
fn a_walk_lists_every_file_under_a_directory() {
    let source = "use std::fs\n\
                  \n\
                  fn main() throws {\n\
                  \x20   let root = fs::Root::Dir(\"tree\")\n\
                  \x20   for name in fs::walk(\".\", root) {\n\
                  \x20       let text = fs::read_to_string(ref name, root)\n\
                  \x20       println(f\"{name}={text}\")\n\
                  \x20   }\n\
                  \x20   for name in fs::walk(\"src\", root) {\n\
                  \x20       println(f\"src: {name}\")\n\
                  \x20   }\n\
                  }\n";
    runs(
        "lists",
        source,
        "b.rs=b\nsrc/a.rs=a\nsrc/deep/c.txt=c\nsrc: a.rs\nsrc: deep/c.txt\n",
    );
}

/// **The root holds for the walk**: a directory above it is refused, and the
/// refusal is caught like any other `io::IoError`.
#[test]
fn a_walk_out_of_its_root_is_refused() {
    let source = "use std::fs\n\
                  \n\
                  fn main() {\n\
                  \x20   let names = fs::walk(\"..\", fs::Root::Dir(\"tree\")) catch {\n\
                  \x20       println(\"refused\")\n\
                  \x20       return\n\
                  \x20   }\n\
                  \x20   println(f\"listed {names.len()}\")\n\
                  }\n";
    runs("outside", source, "refused\n");
}
