//! `1_000_000`, `0xFF`, `0b1010` and `0o17`
//! ([ADR-285](../../../docs/specification/adr/adr-285.md)).
//!
//! The grammar is scannerless, so none of these was a syntax error before this:
//! `1_000` was the number `1` beside a name `_000` that nothing declares, and
//! `0xFF` was `0` beside `xFF`. That is a **misparse** — [Part III
//! C.1](../../../docs/specification/30-nikaia-tooling.md)'s class, and the one
//! `NK1117` was built to report rather than hand to `rustc`.

use nikaia::contracts::LedgerOps;
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(body: &str) -> String {
    let source = format!("fn main() {{\n{body}\n}}\n");
    let parsed = parse_to_ast(&source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust
}

fn refused(body: &str) -> String {
    let source = format!("fn main() {{\n{body}\n}}\n");
    match parse_to_ast(&source) {
        Ok(_) => panic!("this parses, and should not: {source}"),
        Err(e) => e.to_string(),
    }
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    use nikaia::contracts::{Ledger, STD};
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

/// **The separator, and it is not in the value** (D1, D3).
#[test]
fn an_underscore_separates_digits_and_is_not_one() {
    let rust = lowered("    let n = 1_000_000\n    println(f\"{n}\")");
    assert!(rust.contains("let n = 1000000;"), "{rust}");
}

/// **The three prefixes, and the radix is a spelling** (D1, D2). `0xFF` is
/// `255` — a value, taking the first type that holds it exactly as `255` does.
#[test]
fn the_three_radix_prefixes_are_values() {
    let rust = lowered(
        "    let mask = 0xFF\n\
         \x20   let bits = 0b1010\n\
         \x20   let perm = 0o17\n\
         \x20   println(f\"{mask} {bits} {perm}\")",
    );
    assert!(rust.contains("let mask = 255;"), "{rust}");
    assert!(rust.contains("let bits = 10;"), "{rust}");
    assert!(rust.contains("let perm = 15;"), "{rust}");
}

/// **The digits after `0x` may be either case, and the prefix may not** (D1).
/// `0X10` is not a second spelling — it is the number `0` beside a name, which
/// is what it always was, and `NK1117` says so.
#[test]
fn the_hex_digits_take_either_case_and_the_prefix_does_not() {
    let rust = lowered("    let a = 0xff\n    let b = 0xFF\n    println(f\"{a} {b}\")");
    assert!(rust.contains("let a = 255;"), "{rust}");
    assert!(rust.contains("let b = 255;"), "{rust}");

    let found: Vec<_> = findings("fn main() {\n    let n = 0X10\n    println(f\"{n}\")\n}\n")
        .into_iter()
        .filter(|f| f.code == "NK1117")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("X10"), "{found:#?}");
}

/// **A float takes the separator and no prefix** (D1). It has to: without it
/// `1_000.5` is `1_000` beside `.5`, and `.5` on a number is a *tuple part*.
#[test]
fn a_float_takes_the_separator() {
    let rust = lowered("    let f = 1_000.5\n    let e = 1_000e2\n    println(f\"{f} {e}\")");
    assert!(rust.contains("let f = 1000.5;"), "{rust}");
    assert!(rust.contains("let e = 1000e2;"), "{rust}");
}

/// **An underscore stands between digits and nowhere else** (D1).
#[test]
fn an_underscore_out_of_place_is_refused() {
    for (body, said) in [
        ("    let n = 1_", "A number can't end in an underscore."),
        (
            "    let n = 1__0",
            "An underscore in a number goes between two digits, like `1_000_000`.",
        ),
        (
            "    let n = 0x_FF",
            "An underscore in a number goes between two digits, like `1_000_000`.",
        ),
    ] {
        let message = refused(body);
        assert!(message.contains(said), "{body}: {message}");
    }
}

/// **A digit the radix does not have** is the misparse one prefix along:
/// without this, `0b1210` is `0b1` beside the number `210`.
#[test]
fn a_digit_the_radix_does_not_have_is_refused() {
    assert!(
        refused("    let n = 0b1210").contains("A `0b` number only has the digits `0` and `1`.")
    );
    assert!(refused("    let n = 0o19").contains("A `0o` number only has the digits `0` to `7`."));
    assert!(
        refused("    let n = 0x").contains("A number needs at least one digit after its prefix.")
    );
}

/// **A number that does not fit is refused**, where it used to take this
/// compiler down: the action read the digits with `parse().unwrap()`.
#[test]
fn a_number_too_wide_for_an_i64_is_refused_and_does_not_panic() {
    let message = refused("    let n = 99999999999999999999");
    assert!(
        message.contains("This number is too big: the largest integer types are `u64` and `i64`."),
        "{message}"
    );
}

/// **The separator does not survive into a diagnostic** (D3). The number is
/// what the reader needs named, and reading their own spelling back says
/// nothing.
#[test]
fn a_diagnostic_names_the_number_and_not_the_spelling() {
    let found: Vec<_> =
        findings("fn main() {\n    let n: i32 = 3_000_000_000\n    println(f\"{n}\")\n}\n")
            .into_iter()
            .filter(|f| f.code == "NK1116")
            .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("3000000000"), "{found:#?}");
    assert!(!found[0].message.contains('_'), "{found:#?}");
}

/// **`NK1117`'s help stopped explaining `1_000`**, which is the clause the
/// record says loses its meaning: the form is a number now.
#[test]
fn the_undeclared_name_help_no_longer_explains_the_separator() {
    let found: Vec<_> = findings("fn main() {\n    nothing_here\n}\n")
        .into_iter()
        .filter(|f| f.code == "NK1117")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        !found[0]
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("1_000"),
        "{found:#?}"
    );
}

/// **A tuple's part is not a number in this sense** — `t.0` is a field name
/// that happens to be digits, so nothing about the four forms reaches it.
#[test]
fn a_tuple_part_is_untouched() {
    let rust = lowered("    let pair = (\"*\", 3)\n    println(f\"{pair.0}\")");
    assert!(rust.contains("pair.0"), "{rust}");
}

/// **The most negative `i64` has a spelling** — the completeness item
/// issue #135 carried.
///
/// `-9223372036854775808` is `i64::MIN` and is in the type. As a **negation of
/// a positive literal** its digits are `9223372036854775808`, which no `i64`
/// holds, so the parser refused a number that belongs to the language. Every
/// other number in the range was writable either way, which is what made this
/// one number missing rather than a hole.
#[test]
fn the_most_negative_i64_parses() {
    let rust = lowered("    let n: i64 = -9223372036854775808\n    println(f\"{n}\")");
    assert!(rust.contains("-9223372036854775808i64"), "{rust}");
}

/// **And the digits alone are still refused as an `i64`**, which is the other
/// half: what the sign buys is one number, not a wider type. Since 0.0.250 they
/// are a `u64`'s (ADR-285 D19), so it is the checker that says so, pointing at
/// the type - and past a `u64` the parser still refuses the number.
#[test]
fn the_same_digits_without_the_sign_are_still_refused() {
    let found = findings(
        "fn main() {
    let n = 9223372036854775808
}
",
    );
    assert!(
        found.iter().any(|f| f.code == "NK1116"
            && f.message.contains("doesn't fit in an `i64`")
            && f.help.as_deref().is_some_and(|h| h.contains("`u64`"))),
        "{found:#?}"
    );
    assert!(
        refused("    let n = 18446744073709551616")
            .contains("This number is too big: the largest integer types are `u64` and `i64`.")
    );
}

/// **Only where the `-` sits directly in front of the digits.** `- 5` and `-x`
/// are the unary operator they always were, and a binary `-` is matched by the
/// rule that wrote it rather than by its operand's.
#[test]
fn a_minus_that_is_not_against_the_digits_is_the_operator() {
    let rust = lowered(
        "    let a = 10\n\
         \x20   let b = a - 5\n\
         \x20   let c = a -5\n\
         \x20   let d = - a\n\
         \x20   println(f\"{b} {c} {d}\")",
    );
    assert!(rust.contains("let b = a - 5;"), "{rust}");
    assert!(rust.contains("let c = a - 5;"), "{rust}");
    assert!(rust.contains("let d = -a;"), "{rust}");
}

/// **A float keeps the sign it always had**, and that is what
/// `examples/n-body/src/main.nika` said the first time this rule ran: `-1.16e+00` begins
/// with digits and is not an integer, so a signed match would take the `-1` and
/// leave the rest stranded.
#[test]
fn a_negative_float_is_still_a_float() {
    let rust = lowered(
        "    let y = -1.16032004402742839e+00\n\
         \x20   let z = -0.5\n\
         \x20   println(f\"{y} {z}\")",
    );
    assert!(rust.contains("-1.16032004402742839e+00"), "{rust}");
    assert!(rust.contains("-0.5"), "{rust}");
}

/// **A range keeps its own reading too**, for the same reason: the `.` after
/// the digits says this is not an integer literal on its own.
#[test]
fn a_range_that_starts_below_zero_still_parses() {
    let rust = lowered("    for i in -2..<2 { println(f\"{i}\") }");
    assert!(rust.contains("-2..2"), "{rust}");
}

/// **The sign rides through a radix too**, because it goes into the text the
/// radix parser reads rather than being applied to what came out.
#[test]
fn a_negative_number_may_be_written_in_any_radix() {
    let rust = lowered("    let n = -0xFF\n    println(f\"{n}\")");
    assert!(rust.contains("let n = -255;"), "{rust}");
}

/// **A literal in a branch is of the type the `let` wrote** (issue #171
/// issue #171, found moving `fold` into Nikaia): `2147483648` in an `if` inside a
/// `match` arm was written `2147483648i64` under a `let l: u64`, because the
/// type stopped at the `match`.
#[test]
fn a_literal_in_a_branch_takes_the_type_the_let_wrote() {
    let rust = lowered(
        "    let negative = true\n\
         \x20   let l: u64 = match negative {\n\
         \x20       true => if negative { 2147483648 } else { 1 },\n\
         \x20       false => 4294967295,\n\
         \x20   }\n\
         \x20   println(f\"{l}\")",
    );
    assert!(rust.contains("2147483648u64"), "{rust}");
    assert!(rust.contains("4294967295u64"), "{rust}");
    assert!(!rust.contains("i64"), "{rust}");
}

/// **A scale spells the value** ([ADR-322](../../../docs/specification/adr/adr-322.md)):
/// `K M G T P` are 10³ to 10¹⁵, `Ki Mi Gi Ti Pi` 2¹⁰ to 2⁵⁰, after the digits
/// and their separators, and what is lowered is the number.
#[test]
fn a_scale_is_a_spelling_of_the_value() {
    let rust = lowered(
        "    let a = 1K\n\
         \x20   let b = 1Ki\n\
         \x20   let c = 4Gi\n\
         \x20   let d = 1_500M\n\
         \x20   let e = 16P\n\
         \x20   let f = -2K\n\
         \x20   println(f\"{a} {b} {c} {d} {e} {f}\")",
    );
    assert!(rust.contains("let a = 1000;"), "{rust}");
    assert!(rust.contains("let b = 1024;"), "{rust}");
    assert!(rust.contains("let c = 4294967296i64;"), "{rust}");
    assert!(rust.contains("let d = 1500000000;"), "{rust}");
    assert!(rust.contains("let e = 16000000000000000i64;"), "{rust}");
    assert!(rust.contains("let f = -2000;"), "{rust}");
}

/// **The use gives the type**, as for any literal (ADR-285 D20): `1G` is an
/// `i32`, `3G` an `i64`, and `1K` in a `u8` is `NK1116` about `1000`.
#[test]
fn a_scaled_number_is_typed_by_its_use() {
    let rust = lowered("    let a = 1G\n    let b = 3G\n    println(f\"{a} {b}\")");
    assert!(rust.contains("let a = 1000000000;"), "{rust}");
    assert!(rust.contains("let b = 3000000000i64;"), "{rust}");

    let found = findings("fn main() {\n    let c: u8 = 1K\n    println(f\"{c}\")\n}\n");
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK1116" && f.message.contains("`1000`")),
        "{found:#?}"
    );
}

/// **A scale a name goes on from is not one**: `1Gx` is the number `1` beside
/// a name, as `0X10` is.
#[test]
fn a_scale_followed_by_a_name_is_a_name() {
    let found: Vec<_> = findings("fn main() {\n    let n = 1Gx\n    println(f\"{n}\")\n}\n")
        .into_iter()
        .filter(|f| f.code == "NK1117")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("Gx"), "{found:#?}");
}

/// **Past a `u64`, refused by name**: `20000P` is 2·10¹⁹.
#[test]
fn a_scaled_number_past_a_u64_is_refused_by_name() {
    let said = refused("    let n = 20000P\n    println(f\"{n}\")");
    assert!(
        said.contains("`20000P` is 20000000000000000000, which is too big"),
        "{said}"
    );
}

/// **No scale where it does not belong**: a radix-prefixed number has no
/// decimal scale, and a float takes none.
#[test]
fn a_scale_on_a_radix_or_a_float_is_refused() {
    let said = refused("    let n = 0xFFK\n    println(f\"{n}\")");
    assert!(said.contains("A `0x` number only has the digits"), "{said}");
    let said = refused("    let n = 1.5G\n    println(f\"{n}\")");
    assert!(
        said.contains("A scale like `G` stands on a whole number, not on a float"),
        "{said}"
    );
}

/// **An exponent is written `e`** (ADR-322): `1E5` is refused with the
/// spelling it means, and `2e3` is the float it was.
#[test]
fn an_upper_case_exponent_is_refused_with_its_spelling() {
    let said = refused("    let n = 1E5\n    println(f\"{n}\")");
    assert!(
        said.contains("written with a lower-case `e`: `1e5`"),
        "{said}"
    );
    let said = refused("    let n = 1.5E-4\n    println(f\"{n}\")");
    assert!(
        said.contains("written with a lower-case `e`: `e-4`"),
        "{said}"
    );
    let rust = lowered("    let x = 2e3\n    println(f\"{x}\")");
    assert!(rust.contains("2e3"), "{rust}");
}

/// **A number where no number is declared is a mismatch** (#471): a literal
/// takes its type from its use, and a use that is not a number gives it none.
/// It used to fit anything and reach `rustc`.
#[test]
fn a_number_literal_where_text_is_declared_is_refused() {
    let codes =
        |source: &str| -> Vec<&'static str> { findings(source).iter().map(|f| f.code).collect() };
    assert_eq!(
        codes("fn main() {\n    let s: String = 3\n    println(s)\n}\n"),
        ["NK1103"]
    );
    assert_eq!(
        codes("fn f() -> String {\n    return -2\n}\nfn main() {\n    println(f())\n}\n"),
        ["NK1104"]
    );
    assert_eq!(
        codes(
            "struct P { name: String }\nfn main() {\n    let p = P { name: 1.5 }\n    println(p.name)\n}\n"
        ),
        ["NK1106"]
    );
    // A number declared, or a number that may be absent, still takes it.
    assert!(
        codes("fn main() {\n    let n: i32 = 4\n    let x: f64 = 2\n    let y: i64? = 3\n    println(f\"{n} {x} {y ?? 0}\")\n}\n")
            .is_empty()
    );
}
