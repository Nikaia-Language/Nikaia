//! **A view handed back is handed back silently**
//! ([ADR-283](../../../docs/specification/adr/adr-283.md) D23-D24, #442): a
//! view of what a function owns moves it into the caller's keep, and a view
//! of a parameter borrows from the caller's argument - an argument made in the
//! call going into the caller's keep. Each of these reached `rustc` before.

mod common;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

/// Lower it, compile it, run it, and what it printed.
fn output(purpose: &str, source: &str) -> String {
    let found = findings(source);
    assert!(found.is_empty(), "{found:#?}");
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust;
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    String::from_utf8_lossy(&ran.stdout).trim().to_string()
}

const OWNED: &str = "\
fn d() -> ref String {
    let s: String = \"x\".clone()
    return ref s
}

fn main() {
    println(d())
}
";

/// **D23, step 1**: the caller's keep is declared wherever the call stands -
/// here alone in `main`, inside `println`, where it was never declared.
#[test]
fn a_view_of_a_local_lives_in_the_callers_keep() {
    assert_eq!(output("handed-back-local", OWNED), "x");
}

/// …and in an `f"…"` hole, a condition and an argument.
#[test]
fn the_callers_keep_in_every_position() {
    let source = "\
fn d() -> ref String {
    let s: String = \"x\".clone()
    return ref s
}

fn main() {
    let n = d().len()
    if d().len() == 1 {
        println(f\"{d()} {n}\")
    }
}
";
    assert_eq!(output("handed-back-positions", source), "x 1");
}

const PARAMETERS: &str = "\
struct Row {
    name: String,
}

fn e(t: String) -> ref String {
    return ref t
}

fn c(row: Row) -> ref String {
    return ref row.name
}

fn first(a: String, b: String) -> ref String {
    if a.len() > b.len() {
        return ref a
    }
    return ref b
}

fn label(a: String, b: String) -> ref String {
    if a == b {
        return \"same\"
    }
    return \"other\"
}

fn main() {
    let t: String = \"y\".clone()
    let row = Row { name: \"z\".clone() }
    println(f\"{e(t)} {c(row)} {first(\"long\", \"x\")} {label(\"a\", \"b\")}\")
}
";

/// **D24, step 2**: a view handed back through a parameter lowered as a view
/// borrows from the caller's argument - `'static` was *lifetime may not live
/// long enough* - and with two such parameters they share one lifetime.
#[test]
fn a_view_of_a_parameter_borrows_from_the_argument() {
    assert_eq!(
        output("handed-back-parameter", PARAMETERS),
        "y z long other"
    );
}

/// **D24, step 3**: an argument made in the call goes into the caller's keep.
#[test]
fn an_argument_made_in_the_call_is_kept() {
    let source = "\
struct Row {
    name: String,
}

fn e(t: String) -> ref String {
    return ref t
}

fn c(row: Row) -> ref String {
    return ref row.name
}

fn main() {
    let r = e(\"y\".clone())
    let q = c(Row { name: \"z\".clone() })
    let l = e(\"w\")
    println(f\"{r} {q} {l}\")
}
";
    assert_eq!(output("handed-back-made-in-the-call", source), "y z w");
}

/// **Step 4**: `--tethers` names the keep, and what a borrowed result borrows
/// from.
#[test]
fn tethers_names_the_keep() {
    let report = |source: &str| {
        let parsed = parse_to_ast(source).expect("the source parses");
        let ledger = Ledger::infer(&parsed);
        nikaia::contracts::tether::report(&parsed, &ledger)
    };
    let owned = report(OWNED);
    assert!(
        owned.contains("`s` lives in `main`'s keep: handed back by `d`"),
        "{owned}"
    );
    let parameters = report(PARAMETERS);
    assert!(
        parameters.contains("borrowed  `<result>` - from the caller's argument for `row`"),
        "{parameters}"
    );
}

/// **Step 5**: `NK1104`'s help is the way out, and following it runs.
#[test]
fn following_nk1104s_help_runs() {
    let refused = "\
fn e(t: String) -> ref String {
    return t
}

fn main() {
    println(e(\"y\"))
}
";
    let found = findings(refused);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1104");
    assert_eq!(
        found[0].help.as_deref(),
        Some("Write `ref` in front of it to pass a view of it.")
    );
    let followed = refused.replace("return t", "return ref t");
    assert_eq!(output("handed-back-help", &followed), "y");
}
