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

/// **An open number's type is decided before any widening** (#439 step 6):
/// `let n = 5` used as an `i64` and as a `u32` is two uses that disagree, and
/// stays `NK1200` - widening the `u32` use would be a second solution.
#[test]
fn an_open_number_is_decided_before_any_widening() {
    let found = codes(
        "fn wide(x: i64) -> i64 {\n    x\n}\n\n\
         fn narrow(x: u32) -> u32 {\n    x\n}\n\n\
         fn main() {\n    let n = 5\n    println(f\"{wide(n)} {narrow(n)}\")\n}\n",
    );
    assert!(found.contains(&"NK1200"), "{found:?}");
}

/// **A stated type reaches the operands** (#439 step 7): `x * y` over two
/// `i32`s, returned as an `i64`, is computed in `i64` and does not stop where
/// an `i32` would. A literal among the operands is an `i64` too.
#[test]
fn a_stated_type_reaches_the_operands() {
    let source = "fn area(x: i32, y: i32) -> i64 {\n    x * y\n}\n\n\
                  fn scaled(x: i32) -> i64 {\n    return -(x * 3) + 1\n}\n\n\
                  fn main() {\n    let s: i64 = 50000 \n    let a: i32 = 50000\n    \
                  let t: i64 = a * a\n    \
                  println(f\"{area(50000, 50000)} {scaled(1000000000)} {t} {s}\")\n}\n";
    runs("reach", source, "2500000000 -2999999999 2500000000 50000\n");
}

/// **It does not reach into a name**: `let p = x * y` is computed in `i32`,
/// as written, and stops at the overflow.
#[test]
fn a_name_keeps_the_type_it_was_computed_in() {
    let source = "fn area(x: i32, y: i32) -> i64 {\n    let p = x * y\n    p\n}\n\n\
                  fn main() {\n    println(f\"{area(50000, 50000)}\")\n}\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust;
    let dir = common::scratch_dir("widening-name");
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &[
            "--crate-type",
            "bin",
            "-C",
            "overflow-checks=on",
            "-o",
            &binary.to_string_lossy(),
        ],
    );
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary).output().expect("run it");
    assert!(
        !out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **`NK1215`: a computation whose result depends on the width, narrow, where
/// a wider type is stated** (#439 step 8). The help names both readings; the
/// program that wrote one runs.
#[test]
fn a_width_dependent_computation_says_which_width() {
    let program = |line: &str| {
        format!(
            "fn main() {{\n    let a: u32 = 7\n    let b: u32 = 3\n    {line}\n    println(f\"{{t}}\")\n}}\n"
        )
    };
    for line in [
        "let t: i64 = a << 3",
        "let t: i64 = !a",
        "let t: i64 = a & b",
        "let t: i64 = a.wrapping_mul(3) + 1",
    ] {
        let found = errors(&program(line));
        assert!(
            found.iter().any(|f| f.code == "NK1215"),
            "{line}: {found:#?}"
        );
    }
    let shift = errors(&program("let t: i64 = a << 3"));
    assert_eq!(
        shift[0].help.as_deref(),
        Some(
            "Say which: `(a as i64) << 3` computes in `i64`, `(a << 3) as i64` computes \
             first and widens the result."
        )
    );
    runs("written", &program("let t: i64 = (a as i64) << 3"), "56\n");
}

/// **The help says when a number goes into another type on its own** (#439
/// step 11): where every value fits, and not otherwise.
#[test]
fn the_help_says_when_a_number_widens() {
    let refusals = errors(
        "fn main() {\n    let a: u64 = 7\n    let b: i64 = -3\n    let x: i64 = a\n    \
         let s = a + b\n    println(f\"{x} {s}\")\n}\n",
    );
    let slot = refusals
        .iter()
        .find(|f| f.code == "NK1103")
        .expect("NK1103");
    assert_eq!(
        slot.help.as_deref(),
        Some(
            "Convert it with `as i64`. A number goes into another type on its own only where \
             every value it can hold fits."
        )
    );
    let operator = refusals
        .iter()
        .find(|f| f.code == "NK1199")
        .expect("NK1199");
    assert_eq!(
        operator.notes,
        vec![
            "Two integer types mix where one holds every value of both, and no type holds \
             every `u64` and every `i64`."
                .to_string()
        ]
    );
}

const NOT_NEGATIVE: &str = "fn half(k: i64) -> u64 {\n\
     \x20   if k >= 0 {\n\
     \x20       let n: u64 = k\n\
     \x20       return n / 2\n\
     \x20   }\n\
     \x20   return 0\n\
     }\n\
     \n\
     fn main() {\n\
     \x20   let xs = [1, 2, 3]\n\
     \x20   let n: u64 = xs.len()\n\
     \x20   println(f\"{n} {half(9)} {half(-4)}\")\n\
     }\n";

/// **A value the walk shows is not negative goes into a `u64`** (#439 step
/// 9): a length, and a value after `if k >= 0`. Neither is checked again in
/// the emitted Rust.
#[test]
fn a_value_proved_not_negative_goes_into_a_u64() {
    runs("not-negative", NOT_NEGATIVE, "3 4 0\n");
    let parsed = parse_to_ast(NOT_NEGATIVE).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust;
    assert!(
        rust.contains("let n: u64 = nikaia_std::num::u64_of(k);"),
        "{rust}"
    );
    assert!(!rust.contains("u64::try_from"), "{rust}");
}

/// **Where nothing shows it, the slot is refused** (#439 step 9), and the help
/// names the checked conversion and the `assert`.
#[test]
fn a_value_nothing_shows_not_negative_is_refused() {
    let found = errors(
        "fn f(k: i64) -> u64 {\n    let n: u64 = k\n    n\n}\n\n\
         fn main() {\n    println(f\"{f(3)}\")\n}\n",
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1103")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert_eq!(
        refusal.help.as_deref(),
        Some(
            "Write `k as u64`, which stops the program on a negative value, or `assert k >= 0` before this line."
        )
    );
    // **And the `assert` is a way out**: what it claims holds after it.
    let asserted = errors(
        "fn f(k: i64) -> u64 {\n    assert(k >= 0)\n    let n: u64 = k\n    n\n}\n\n\
         fn main() {\n    println(f\"{f(3)}\")\n}\n",
    );
    assert!(asserted.is_empty(), "{asserted:#?}");
}

/// **The answer is the recorded one** (#439 step 9, ADR-270 D4): a project
/// build writes it into `nikaia.proofs`, and a `--locked` build reads it back.
#[test]
fn the_answer_is_recorded_and_replayed() {
    let dir = common::scratch_dir("widening-proofs");
    std::fs::create_dir_all(dir.join("src")).expect("the package");
    std::fs::write(
        dir.join("nikaia.toml"),
        "[package]\nname = \"wp\"\nversion = \"0.1.0\"\n",
    )
    .expect("a manifest");
    std::fs::write(dir.join("src/main.nika"), NOT_NEGATIVE).expect("a source");
    let build = |locked: bool| {
        let mut args = vec!["build", "--no-cache"];
        if locked {
            args.push("--locked");
        }
        Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .current_dir(&dir)
            .args(args)
            .output()
            .expect("the nikaia binary runs")
    };
    let first = build(false);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let written = std::fs::read_to_string(dir.join("nikaia.proofs")).expect("the file is written");
    let again = build(true);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert!(written.lines().any(|l| l.contains(" proved ")), "{written}");
}

/// **`len() >= 0` for every list** (#439 step 10): a list whose elements take
/// no space has no upper bound on its length, and still none below zero.
#[test]
fn a_length_of_elements_that_take_no_space_is_not_negative() {
    runs(
        "unsized",
        "enum One {\n    It,\n}\n\n\
         fn main() {\n    let units = [One::It, One::It]\n    let n: u64 = units.len()\n    \
         println(f\"{n}\")\n}\n",
        "2\n",
    );
}

/// **`x += a` is `x = x + a`** (D32): over a `u32` into an `i64` it widens and
/// runs; a sum that is wider than `x` does not fit it (`NK1105`); `u64` with
/// `i64` has no common type (`NK1199`).
#[test]
fn a_compound_assignment_computes_in_the_common_type() {
    runs(
        "compound",
        "fn main() {\n\
         \x20   let xs: Vec[u32] = [1, 2, 3]\n\
         \x20   let a: u32 = 4\n\
         \x20   let mut total: i64 = 0\n\
         \x20   total += a\n\
         \x20   for n in xs {\n\
         \x20       total += n * 2\n\
         \x20   }\n\
         \x20   println(f\"{total}\")\n\
         }\n",
        "16\n",
    );
    let narrower = "fn main() {\n    let b: i64 = 4\n    let mut small: u32 = 1\n    small += b\n    println(f\"{small}\")\n}\n";
    assert!(codes(narrower).contains(&"NK1105"), "{:?}", codes(narrower));
    let none = "fn main() {\n    let b: i64 = 4\n    let mut u: u64 = 1\n    u += b\n    println(f\"{u}\")\n}\n";
    assert!(codes(none).contains(&"NK1199"), "{:?}", codes(none));
}

/// **A body's last expression asks the walk too** (D32): `xs.len()` handed
/// back as a `u64` is taken; a parameter nothing shows is refused; and a bit
/// operation narrow into a wider return type is `NK1215` there as well.
#[test]
fn a_last_expression_asks_as_a_return_does() {
    runs(
        "tail-len",
        "fn count(xs: ref Vec[i64]) -> u64 {\n    xs.len()\n}\n\n\
         fn main() {\n    let xs = [1, 2, 3]\n    println(f\"{count(xs)}\")\n}\n",
        "3\n",
    );
    let refused = "fn f(k: i64) -> u64 {\n    k\n}\n\nfn main() {\n    println(f\"{f(3)}\")\n}\n";
    assert!(codes(refused).contains(&"NK1104"), "{:?}", codes(refused));
    let narrow =
        "fn f(a: u32) -> i64 {\n    a << 3\n}\n\nfn main() {\n    println(f\"{f(3)}\")\n}\n";
    assert!(codes(narrow).contains(&"NK1215"), "{:?}", codes(narrow));
}
