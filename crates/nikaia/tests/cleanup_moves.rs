//! **A value's cleanup moved with a contract, and the call is told** — `NK2403`
//! ([ADR-297](../../../docs/specification/adr/adr-297.md), ADR-094 D5).
//!
//! A callee that keeps what it is given runs its cleanup when it is done with
//! it; one that only reads it lends it, and the cleanup runs at the end of the
//! caller's block. When a change to the callee moves that point, on a type
//! whose teardown does something, each call that hands it a name is told once.

mod common;

use std::collections::BTreeSet;
use std::process::Command;

use nikaia::assets::Reads;
use nikaia::check::{self, Finding, Kept, Newly};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str, moved: &[(&str, &str, bool)]) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let mut newly = Newly::new();
    for (callee, param, now) in moved {
        newly
            .keeps
            .entry(callee.to_string())
            .or_default()
            .push(Kept {
                param: param.to_string(),
                now: *now,
                ty: "Handle".to_string(),
            });
    }
    check::check_against(
        &parsed,
        &[],
        &own,
        &library,
        &BTreeSet::new(),
        &newly,
        &Reads::none(),
    )
    .findings
}

fn told(found: &[Finding]) -> Vec<&Finding> {
    found.iter().filter(|f| f.code == "NK2403").collect()
}

const PROGRAM: &str = "struct Handle {\n\
    \x20   name: String,\n\
    }\n\
    \n\
    impl Drop for Handle {\n\
    \x20   fn drop(ref mut self) {\n\
    \x20       println(f\"closing {self.name}\")\n\
    \x20   }\n\
    }\n\
    \n\
    struct Holder {\n\
    \x20   inner: Handle,\n\
    }\n\
    \n\
    fn look(h: Handle) -> i64 {\n\
    \x20   let n = h.name.len()\n\
    \x20   let held: Vec[Handle] = [h]\n\
    \x20   return n + held.len() - 1\n\
    }\n\
    \n\
    fn weigh(w: Holder) -> i64 {\n\
    \x20   return w.inner.name.len()\n\
    }\n\
    \n\
    fn count(n: i64) -> i64 {\n\
    \x20   return n\n\
    }\n\
    \n\
    fn main() {\n\
    \x20   let h = Handle { name: f\"a\" }\n\
    \x20   let n = look(h)\n\
    \x20   let w = Holder { inner: Handle { name: f\"b\" } }\n\
    \x20   let m = weigh(w)\n\
    \x20   println(f\"end of main {n} {m} {count(n)}\")\n\
    }\n";

/// **Kept now, and the call is told where the cleanup went** - a warning, as
/// `NK2402` is, because the program is correct either way.
#[test]
fn a_callee_that_starts_keeping_moves_the_cleanup_into_it() {
    let found = findings(PROGRAM, &[("look", "h", true)]);
    let told = told(&found);
    assert_eq!(told.len(), 1, "{found:#?}");
    assert_eq!(told[0].severity, check::Severity::Warning);
    assert!(
        told[0].message.contains("when `look` is done with it"),
        "{:#?}",
        told[0]
    );
    assert!(
        told[0].notes[0].contains("`impl Drop for Handle`"),
        "{:#?}",
        told[0]
    );
}

/// **And the other way round**, and through a field: a `Holder` tears down a
/// `Handle`, so its teardown does something too.
#[test]
fn a_callee_that_stops_keeping_moves_it_back_and_a_field_counts() {
    let found = findings(PROGRAM, &[("weigh", "w", false)]);
    let told = told(&found);
    assert_eq!(told.len(), 1, "{found:#?}");
    assert!(
        told[0].message.contains("at the end of this block"),
        "{:#?}",
        told[0]
    );
    assert!(
        told[0].notes[0].contains("through its `Handle` field"),
        "{:#?}",
        told[0]
    );
}

/// **Silence where nothing observable moved**: a type with no teardown, and a
/// build with nothing to compare.
#[test]
fn nothing_is_said_where_no_teardown_moved() {
    assert!(told(&findings(PROGRAM, &[("count", "n", true)])).is_empty());
    assert!(told(&findings(PROGRAM, &[])).is_empty());
}

/// **D2: a list literal keeps its elements.** `look` puts its `Handle` into a
/// list, so it keeps it - it was lent, and `rustc` met a `&Handle` where the
/// list wanted a `Handle`. Run, the cleanup happens inside `look`.
#[test]
fn a_parameter_put_into_a_list_literal_is_kept() {
    let parsed = parse_to_ast(PROGRAM).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let look = own.functions.get("look").expect("look is in the ledger");
    assert!(look.keeps.contains(&"h".to_string()), "{look:#?}");

    for how in [Build::default(), Build::parallel()] {
        let rust = emit_program(&parsed, how).expect("the source lowers").rust;
        let dir = common::scratch_dir("cleanup-moves");
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
            "{}\n--- emitted ---\n{rust}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let out = Command::new(&binary).output().expect("run it");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "closing a\nend of main 1 1 1\nclosing b\n",
            "at {how:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
