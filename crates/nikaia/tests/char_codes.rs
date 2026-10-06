//! **A `char`'s code is a number of the proof**
//! ([ADR-314](../../../docs/specification/adr/adr-314.md) D2, #421): `c as T`
//! of a `char` into a whole number is in `0..=0x10FFFF`, so arithmetic over it
//! that cannot leave its type is proved.

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

fn line(out: &str, text: &str) -> String {
    out.lines()
        .find(|l| l.contains(text))
        .unwrap_or_else(|| panic!("no line for `{text}`:\n{out}"))
        .to_string()
}

const SOURCE: &str = "\
fn digit_value(c: char) -> i32 {
    return c as i32 - 48
}

fn twice(c: char) -> i64 {
    let code = c as i64
    return code * 2 + 1
}

fn far(c: char) -> i32 {
    return c as i32 * 4096
}

fn number(n: i64) -> i64 {
    return n - 48
}

fn main() {
    println(f\"{digit_value('7')} {twice('a')} {far('b')} {number(3)}\")
}
";

#[test]
fn arithmetic_over_a_code_is_proved_where_it_fits() {
    let out = report(SOURCE);
    assert!(line(&out, "(…) - 48").ends_with("proved"), "{out}");
    // Through a `let`: the name is the code.
    assert!(line(&out, "code * 2 ").ends_with("proved"), "{out}");
    assert!(line(&out, "code * 2 + 1").ends_with("proved"), "{out}");
    // 0x10FFFF * 4096 leaves `i32`: still checked.
    assert!(line(&out, "(…) * 4096").contains("checked"), "{out}");
    // A whole number with no bound is not a code.
    assert!(line(&out, "n - 48").contains("checked"), "{out}");
}

/// The program prints the same with the proved checks dropped.
#[test]
fn a_code_is_the_same_number_without_the_check() {
    let parsed = parse_to_ast(SOURCE).expect("the source parses");
    let build = Build {
        overflow: OverflowChecks::Removed,
        ..Build::default()
    };
    let rust = emit_program(&parsed, build).expect("lowers").rust;
    let dir = common::scratch_dir("char-codes");
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
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "7 195 401408 -45"
    );
}

/// **D3: a callee's `ensures` is a fact at the call** - here `std`'s
/// `text::digit_value`, whose ledger entry ensures `result == c - 48` (an
/// expression function's, inferred from `text.nika`): with D2, a digit's value
/// times ten fits an `i32`.
const ACROSS: &str = "\
use text

fn tens(c: char) -> i32 {
    return text::digit_value(c) * 10
}

fn main() {
    println(f\"{tens('7')}\")
}
";

#[test]
fn a_callees_ensures_is_a_fact_at_the_call() {
    let out = report(ACROSS);
    assert!(line(&out, "* 10").ends_with("proved"), "{out}");
}

/// **An expression function publishes `ensures result == e`** (D3), where it
/// is `sync` and the prover reads `e`.
#[test]
fn an_expression_function_publishes_what_it_returns() {
    let parsed = parse_to_ast(
        "fn code(c: char) -> i64 sync {\n    return c as i64 - 48\n}\n\n\
         fn twice(n: i64) -> i64 sync {\n    return n * 2\n}\n\n\
         fn square(n: i64) -> i64 sync {\n    return n * n\n}\n",
    )
    .expect("parses");
    let ledger = Ledger::infer(&parsed);
    assert_eq!(ledger.functions["code"].ensures, ["result == c - 48"]);
    assert_eq!(ledger.functions["twice"].ensures, ["result == n * 2"]);
    // Not linear: nothing the prover reads, nothing published.
    assert!(ledger.functions["square"].ensures.is_empty());
}
