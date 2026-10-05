//! **A call to an expression function is its expression**
//! ([ADR-314](../../../docs/specification/adr/adr-314.md) D3, #421): the
//! bounds walk reads `f(a)`, where `f`'s body is one `return e`, as `e` with
//! `a` for the parameter - and only that, so a helper that says something
//! else proves nothing.

mod common;

use nikaia::bounds::{BoundsChecks, OverflowChecks};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn report(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::bounds_report::report(
        &parsed,
        source,
        "p.nika",
        &own,
        &library,
        BoundsChecks::Removed,
        OverflowChecks::Removed,
    )
}

fn proved(out: &str, site: &str) -> bool {
    out.lines()
        .find(|l| l.contains(&format!("  {site}  ")))
        .unwrap_or_else(|| panic!("no line for `{site}`:\n{out}"))
        .ends_with("proved")
}

fn source(helper: &str) -> String {
    format!(
        "fn before(n: i64) -> i64 {{\n\
         \x20   return {helper}\n\
         }}\n\
         \n\
         fn twice(n: i64) -> i64 {{\n\
         \x20   return before(n) - before(n) + n - 1\n\
         }}\n\
         \n\
         fn last(xs: ref Vec[i64]) -> i64 {{\n\
         \x20   if xs.len() == 0 {{\n\
         \x20       return 0\n\
         \x20   }}\n\
         \x20   return xs[before(xs.len())] + xs[twice(xs.len())]\n\
         }}\n\
         \n\
         fn main() {{\n\
         \x20   let xs = [1, 2, 3]\n\
         \x20   println(f\"{{last(xs)}}\")\n\
         }}\n"
    )
}

/// `before(len)` is `len - 1`, inside a non-empty list; through a second
/// helper too.
#[test]
fn an_expression_function_proves_the_index_it_computes() {
    let out = report(&source("n - 1"));
    assert!(proved(&out, "xs[before(xs.len())]"), "{out}");
    assert!(proved(&out, "xs[twice(xs.len())]"), "{out}");
}

/// A helper that hands back the length itself proves nothing - the walk
/// reads what the body says, not what the name suggests.
#[test]
fn a_helper_that_says_otherwise_proves_nothing() {
    let out = report(&source("n"));
    assert!(!proved(&out, "xs[before(xs.len())]"), "{out}");
}

/// With the checks the proof removes left out, the program prints what it
/// prints with them.
#[test]
fn the_proved_index_runs_without_its_check() {
    let text = source("n - 1");
    let parsed = parse_to_ast(&text).expect("the source parses");
    let build = Build {
        bounds: BoundsChecks::Removed,
        ..Build::default()
    };
    let rust = emit_program(&parsed, build).expect("lowers").rust;
    let dir = common::scratch_dir("expression-functions");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary).output().expect("runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "6");
}
