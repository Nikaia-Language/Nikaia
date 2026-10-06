//! **A narrower integer goes into a wider slot where no value can be lost**
//! ([ADR-285](../../../docs/specification/adr/adr-285.md) D32, #439): `u32`
//! into `i64` at a `let`, an argument (`v.push(a)` too), a `return`, a last expression, a field
//! and an assignment, and a list element of a stated type. `u64` into `i64` can
//! lose a value and stays refused at each of them.

mod common;

use std::process::Command;

use nikaia::check::common_integer;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn errors(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

fn runs(purpose: &str, source: &str, expected: &str) {
    let found = errors(source);
    assert!(found.is_empty(), "{purpose}: {found:#?}");
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust;
    let dir = common::scratch_dir(&format!("widening-{purpose}"));
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
    let out = Command::new(&binary).output().expect("run it");
    assert!(out.status.success(), "{purpose} failed");
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "{purpose}");
    std::fs::remove_dir_all(&dir).ok();
}

/// One program per slot: `{from}` written where `{into}` is stated.
fn slots(from: &str, into: &str) -> Vec<(&'static str, String)> {
    let value = format!("    let a: {from} = 7\n");
    vec![
        (
            "let",
            format!("fn main() {{\n{value}    let x: {into} = a\n    println(f\"{{x}}\")\n}}\n"),
        ),
        (
            "argument",
            format!(
                "fn take(x: {into}) -> {into} {{\n    return x\n}}\n\n\
                 fn main() {{\n{value}    println(f\"{{take(a)}}\")\n}}\n"
            ),
        ),
        (
            "return",
            format!(
                "fn give(a: {from}) -> {into} {{\n    return a\n}}\n\n\
                 fn main() {{\n{value}    println(f\"{{give(a)}}\")\n}}\n"
            ),
        ),
        (
            "last expression",
            format!(
                "fn give(a: {from}) -> {into} {{\n    a\n}}\n\n\
                 fn main() {{\n{value}    println(f\"{{give(a)}}\")\n}}\n"
            ),
        ),
        (
            "field",
            format!(
                "struct S {{\n    v: {into},\n}}\n\n\
                 fn main() {{\n{value}    let s = S {{ v: a }}\n    println(f\"{{s.v}}\")\n}}\n"
            ),
        ),
        (
            "assignment",
            format!(
                "fn main() {{\n{value}    let mut x: {into} = 0\n    x = a\n    println(f\"{{x}}\")\n}}\n"
            ),
        ),
        (
            "method argument",
            format!(
                "fn main() {{\n{value}    let mut v: Vec[{into}] = []\n    v.push(a)\n    println(f\"{{v[0]}}\")\n}}\n"
            ),
        ),
        (
            "list element",
            format!(
                "fn main() {{\n{value}    let xs: Vec[{into}] = [a, a]\n    println(f\"{{xs.len()}} {{xs[0]}}\")\n}}\n"
            ),
        ),
    ]
}

/// **Part I 2.2's table**, all 25 pairs: the type that holds every value of
/// both, the same either way round, and none for `u64` with a signed type.
#[test]
fn the_common_type_of_every_pair() {
    let rows = [
        ("u8", ["u8", "u32", "u64", "i32", "i64"].map(Some)),
        (
            "u32",
            [
                Some("u32"),
                Some("u32"),
                Some("u64"),
                Some("i64"),
                Some("i64"),
            ],
        ),
        ("u64", [Some("u64"), Some("u64"), Some("u64"), None, None]),
        (
            "i32",
            [Some("i32"), Some("i64"), None, Some("i32"), Some("i64")],
        ),
        (
            "i64",
            [Some("i64"), Some("i64"), None, Some("i64"), Some("i64")],
        ),
    ];
    let columns = ["u8", "u32", "u64", "i32", "i64"];
    for (row, expected) in rows {
        for (column, want) in columns.iter().zip(expected) {
            assert_eq!(common_integer(row, column), want, "{row} with {column}");
        }
    }
    assert_eq!(common_integer("u32", "f64"), None);
}

/// **`u32` into `i64`, in every slot that states a type** (D32): it is
/// accepted, lowered as `i64::from(..)`, and runs.
#[test]
fn a_u32_goes_into_an_i64_in_every_slot() {
    for (slot, source) in slots("u32", "i64") {
        let expected = if slot == "list element" {
            "2 7\n"
        } else {
            "7\n"
        };
        runs(slot, &source, expected);
    }
}

/// **The other widenings the table allows**: `u8` into `u32`, `u32` into
/// `u64`, `i32` into `i64`.
#[test]
fn every_lossless_widening_runs() {
    for (from, into) in [("u8", "u32"), ("u32", "u64"), ("i32", "i64"), ("u8", "i64")] {
        let (_, source) = slots(from, into).swap_remove(0);
        runs(&format!("{from}-{into}"), &source, "7\n");
    }
}

/// **`u64` into `i64` can lose a value, and stays refused in each slot** (D32):
/// the common type of the two does not exist.
#[test]
fn a_u64_into_an_i64_stays_refused_in_every_slot() {
    for (slot, source) in slots("u64", "i64") {
        let found = errors(&source);
        assert!(!found.is_empty(), "{slot} was accepted:\n{source}");
    }
}

/// **Narrowing is not widening**: `i64` into `u32` stays refused.
#[test]
fn a_wider_integer_into_a_narrower_stays_refused() {
    for (slot, source) in slots("i64", "u32") {
        let found = errors(&source);
        assert!(!found.is_empty(), "{slot} was accepted:\n{source}");
    }
}

/// **A list that already exists is not converted** (D32): a `Vec[u32]` is not
/// a `Vec[i64]`, only a list literal's elements widen.
#[test]
fn a_list_that_exists_is_not_converted() {
    let found = errors(
        "fn main() {\n    let a: u32 = 7\n    let ys = [a]\n    let xs: Vec[i64] = ys\n    println(f\"{xs.len()}\")\n}\n",
    );
    assert!(!found.is_empty(), "a Vec[u32] was taken as a Vec[i64]");
}

fn codes(source: &str) -> Vec<&'static str> {
    errors(source).into_iter().map(|f| f.code).collect()
}

/// **A mixed operator computes in the common type** (#439 step 3): `u32 +
/// i64` is an `i64` and prints the right sum, a bit operator too.
#[test]
fn a_mixed_operator_computes_in_the_common_type() {
    runs(
        "operators",
        "fn main() {\n\
         \x20   let a: u32 = 4000000000\n\
         \x20   let b: i64 = -3\n\
         \x20   let c: u8 = 2\n\
         \x20   let s = a + b\n\
         \x20   let t: i64 = b * a - c\n\
         \x20   let m = a ^ c\n\
         \x20   println(f\"{s} {t} {m}\")\n\
         }\n",
        "3999999997 -12000000002 4000000002\n",
    );
}

/// **A list literal of two integer types holds their common type**, whatever
/// comes first (#439 step 3).
#[test]
fn a_list_of_two_integer_types_holds_their_common_type() {
    runs(
        "lists",
        "fn main() {\n\
         \x20   let a: u32 = 7\n\
         \x20   let b: i64 = -3\n\
         \x20   let xs = [a, b]\n\
         \x20   let ys = [b, a]\n\
         \x20   println(f\"{xs[0] + ys[0]}\")\n\
         \x20   let zs: Vec[i64] = xs\n\
         \x20   println(f\"{zs.len()}\")\n\
         }\n",
        "4\n2\n",
    );
}

/// **`u64` with a signed type has no common type** (D32): the operator stays
/// `NK1199` and the list `NK1154`.
#[test]
fn a_u64_with_a_signed_type_stays_refused() {
    let both = "    let a: u64 = 7\n    let b: i64 = -3\n";
    assert!(
        codes(&format!(
            "fn main() {{\n{both}    let s = a + b\n    println(f\"{{s}}\")\n}}\n"
        ))
        .contains(&"NK1199")
    );
    assert!(
        codes(&format!(
            "fn main() {{\n{both}    let xs = [a, b]\n    println(f\"{{xs.len()}}\")\n}}\n"
        ))
        .contains(&"NK1154")
    );
}

/// **Every pair of integer types compares as numbers** (#439 step 4): with a
/// common type the narrower side is widened, and `u64` against a signed type is
/// a sign test and a compare - so a negative number is below every `u64`.
#[test]
fn every_pair_of_integer_types_compares_as_numbers() {
    runs(
        "comparisons",
        "fn main() {\n\
         \x20   let a: u32 = 7\n\
         \x20   let b: i64 = -3\n\
         \x20   let u: u64 = 18446744073709551615\n\
         \x20   let i: i32 = -1\n\
         \x20   println(f\"{(-1 as i64) < (0 as u64)} {u > 9223372036854775807 as i64}\")\n\
         \x20   println(f\"{a > b} {b < a} {a != b}\")\n\
         \x20   println(f\"{u == b} {i < u} {u >= i} {b <= u}\")\n\
         }\n",
        "true true\ntrue true true\nfalse true true true\n",
    );
}

/// **Widening takes no part in inference** (#439 step 5): one type variable
/// handed an `i32` and an `i64` is refused by the checker, with the
/// conversion as the help, where `rustc` refused it before.
#[test]
fn a_type_variable_is_not_widened_to_agree() {
    let source = "fn pick[T](a: T, b: T) -> T {\n    a\n}\n\n\
                  fn main() {\n    let a: i32 = 1\n    let b: i64 = 2\n    \
                  println(f\"{pick(a, b)}\")\n}\n";
    let found = errors(source);
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1102")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert_eq!(
        refusal.help.as_deref(),
        Some("Convert one of them: `a as i64`.")
    );
    // **One type is one type**: the same call with two `i64`s runs.
    runs(
        "one-type",
        &source.replace("let a: i32", "let a: i64"),
        "1\n",
    );
}
