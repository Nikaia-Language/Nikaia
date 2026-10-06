//! **What a grammar matched and what a callee promises are facts of the
//! walk** ([ADR-314](../../../docs/specification/adr/adr-314.md), #421): a
//! grammar binding is what it matched (D1), a `char`'s code is in
//! `0..=0x10FFFF` (D2), and a callee's `ensures` holds of the call (D3), so
//! arithmetic over them that cannot leave its type is proved.

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

/// **D1: a grammar's action starts from what its bindings matched** - a
/// `dec[i32](digit{1,2})` is `0..=99`, a `digit` a `char` whose code is
/// `48..=57` - so 1BRC's `TENTHS` stays inside `i32`. A run of digits with no
/// bound says nothing.
const TENTHS: &str = "\
use text

grammar Temps {
    entry rule tenths -> i32 =
        neg:\"-\"? whole:dec[i32](digit{1,2}) \".\" frac:digit
        {
            let value = whole * 10 + text::digit_value(frac)
            if neg != null { -value } else { value }
        }

    entry rule any -> i64 = n:dec[i64](digit+) { n * 10 }
}

fn flipped(n: i64) -> i64 {
    return -n
}

fn main() throws {
    println(f\"{Temps::tenths(\"-12.3\")} {Temps::tenths(\"4.5\")} {Temps::any(\"7\")} {flipped(3)}\")
}
";

#[test]
fn a_grammar_binding_is_what_it_matched() {
    let out = report(TENTHS);
    assert!(line(&out, "whole * 10 + ").ends_with("proved"), "{out}");
    assert!(line(&out, "whole * 10 ").ends_with("proved"), "{out}");
    assert!(line(&out, "n * 10").contains("checked"), "{out}");
    // D5: the negation too - `value` is never the least `i32`.
    assert!(line(&out, "-value").ends_with("proved"), "{out}");
    // A number nothing bounds may be the least `i64`.
    assert!(line(&out, "-n").contains("checked"), "{out}");
}

/// The same numbers with the proved checks dropped.
#[test]
fn a_grammar_action_runs_the_same_without_its_proved_checks() {
    let parsed = parse_to_ast(TENTHS).expect("the source parses");
    let build = Build {
        overflow: OverflowChecks::Removed,
        ..Build::default()
    };
    let rust = emit_program(&parsed, build).expect("lowers").rust;
    assert!(rust.contains("<i32>::wrapping_neg("), "{rust}");
    assert!(!rust.contains("<i64>::wrapping_neg("), "{rust}");
    let dir = common::scratch_dir("grammar-bindings");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(
        &file,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary).output().expect("runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "-123 45 70 -3");
}

/// **D1, text bindings** (#491): `x:digit{1,2}` is text whose length is
/// `1..=2` and whose characters are digits, so a loop over its characters
/// reads each as `0..=9` and a number of at most two of them fits an `i32`;
/// `digit+` says only that the length is at least one.
const DIGITS: &str = "\
grammar Digits {
    entry rule two -> i32 =
        x:digit{1,2}
        {
            let mut n: i32 = 0
            for d in x.chars() {
                n = n * 10 + (d as i32 - 48)
            }
            let most = x.len() * 2000000000000000000
            n
        }

    entry rule many -> i64 =
        x:digit+
        {
            x.len() * 2000000000000000000
        }
}

fn main() throws {
    println(f\"{Digits::two(\"42\")} {Digits::many(\"7\")}\")
}
";

#[test]
fn a_text_binding_has_its_length_and_its_class() {
    let parsed = parse_to_ast(DIGITS).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let checked = nikaia::check::check(&parsed, &own, &library);
    assert!(
        checked.findings.is_empty(),
        "{:#?}",
        checked
            .findings
            .iter()
            .map(|f| &f.message)
            .collect::<Vec<_>>()
    );
    let out = report(DIGITS);
    // The character's code: `d as i32 - 48` is a digit's value.
    assert!(line(&out, "  (…) - 48").ends_with("proved"), "{out}");
    // `n * 10` grows with every turn, which the walk does not count.
    assert!(line(&out, "n * 10 +").contains("checked"), "{out}");
    // The length: at most two, so two times 2 * 10^18 fits an `i64`; `digit+`
    // has no upper end, and the same product may not.
    let products: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("x.len() * 2000000000000000000"))
        .collect();
    assert_eq!(products.len(), 2, "{out}");
    assert!(products[0].ends_with("proved"), "{out}");
    assert!(products[1].contains("checked"), "{out}");
}
