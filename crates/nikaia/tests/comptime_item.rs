//! A `comptime` binding where an item stands
//! ([ADR-287](../../../docs/specification/adr/adr-287.md)).
//!
//! [ADR-287](../../../docs/specification/adr/adr-287.md) D3 decided **both**
//! places and built one: `comptime MAX = 1000` at the top of a file was a parse
//! error while the same line inside a body parsed, folded and ran. Part I 9.2
//! already lists **Constants** among the items `pub` applies to, so the rule for
//! a constant another package may read was written and the syntax for one was
//! not.
//!
//! **What the item form needed that the body form did not** is a frame under
//! every body, filled before any of them is walked: this checker's scope is a
//! stack pushed per function, and an item is visible in its whole scope — a
//! function declared *above* the constant included.
//!
//! What the constants hold when the program runs is
//! `tests/build-time/src/comptime_item.nika`.
use nikaia::check::CodeOps;

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::Build;
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = common::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    common::checked(&parsed, &own, &library).findings
}

fn lowered(source: &str) -> String {
    let found = findings(source);
    assert!(found.is_empty(), "a correct program: {found:#?}");
    let parsed = parse_to_ast(source).expect("the source parses");
    nikaia::emit::emit_program_reading(&parsed, Build::default(), &common::reads())
        .expect("the source lowers")
        .rust
}

/// **The whole shape, lowered**: the four kinds of initialiser the body form
/// takes, `pub`, and a function that reads a constant declared **below** it.
/// What it prints is `tests/build-time/src/comptime_item.nika`.
#[test]
fn an_item_constant_is_written_folded_and_read_from_anywhere() {
    let source = r#"
fn before() -> i64 {
    return LIMIT
}

comptime MAX = 1000
pub comptime LIMIT: i64 = 4 * 1024
comptime DOUBLE = MAX * 2
comptime YES = true

fn main() {
    let b = before()
    println(f"{MAX} {LIMIT} {DOUBLE} {YES} {b}")
}
"#;
    let rust = lowered(source);
    for written in [
        "const MAX: i32 = 1000;",
        "pub const LIMIT: i64 = 4096;",
        "const DOUBLE: i32 = 2000;",
        "const YES: bool = true;",
    ] {
        assert!(rust.contains(written), "`{written}` is written:\n{rust}");
    }
}

/// **A constant declared after the function that reads it**, on its own,
/// because it is the half the frame exists for and the easiest to lose: a pass
/// that filled the frame as it walked would pass every other test here.
#[test]
fn a_function_above_the_constant_still_sees_it() {
    assert!(
        findings("fn f() -> i64 {\n    return N\n}\n\ncomptime N: i64 = 7\n\nfn main() { }\n")
            .is_empty(),
        "an item is visible in its whole scope, which is what makes it an item"
    );
}

/// **`NK1127` reaches the item form too**, which it has to: the refusal is what
/// the word is for ([ADR-287](../../../docs/specification/adr/adr-287.md) D4 —
/// a `let` may fold, a `comptime` **must**), and a place where it did not fire
/// would be a place where `comptime` quietly means `let`.
///
/// **The example has moved once**, and that is the guard working rather than
/// failing: `"x".len()` stood here because text at build time did not exist,
/// and it **folds** since 0.0.113. What still does not is a method this
/// evaluator has no value to call on — a `struct` declared here has one since
/// 0.0.114, and `std`'s body is Rust either way.
#[test]
fn an_item_that_cannot_fold_is_refused() {
    let found = findings("comptime BAD = doubled(21)\n\nfn main() { }\n");
    let codes: Vec<&str> = found.iter().map(|f| f.code_str()).collect();
    assert!(
        codes.contains(&"NK1127"),
        "the same refusal the body form gets: {found:#?}"
    );
}

/// **One function under both places**, checked by behaviour rather than by
/// reading the source: the same initialiser folds to the same value and the
/// same spelling below, wherever it stands.
#[test]
fn the_two_places_agree_about_what_a_constant_is() {
    let as_item = lowered("comptime N: i64 = 2 * 3\n\nfn main() {\n    println(f\"{N}\")\n}\n");
    let in_a_body = lowered("fn main() {\n    comptime N: i64 = 2 * 3\n    println(f\"{N}\")\n}\n");
    assert!(
        as_item.contains("const N: i64 = 6;") && in_a_body.contains("const N: i64 = 6;"),
        "the same `const`, one level apart:\n--- item ---\n{as_item}\n--- body ---\n{in_a_body}"
    );
}

/// **`pub` is Part I 9.2's existing rule for Constants**, not a new one — and
/// it is the reason the item form carries a flag the statement form does not.
#[test]
fn pub_reaches_the_language_below() {
    let public = lowered("pub comptime N: i64 = 1\n\nfn main() {\n    println(f\"{N}\")\n}\n");
    assert!(public.contains("pub const N: i64 = 1;"), "{public}");
    let private = lowered("comptime N: i64 = 1\n\nfn main() {\n    println(f\"{N}\")\n}\n");
    assert!(
        private.contains("const N: i64 = 1;") && !private.contains("pub const N"),
        "{private}"
    );
}

/// **A function the build calls is used** (#446): its value reaches the Rust
/// as a literal, so `rustc` would call it *never used*. What a `comptime` and
/// an option's default reach, transitively, is not warned; a function nothing
/// calls still is.
#[test]
fn a_function_called_only_at_build_time_is_not_warned_unused() {
    let source = "fn helper() -> i64 {\n\
         \x20   return 3\n\
         }\n\
         \n\
         fn three() -> i64 {\n\
         \x20   return helper()\n\
         }\n\
         \n\
         fn scaled() -> i64 {\n\
         \x20   return 4\n\
         }\n\
         \n\
         fn unused() -> i64 {\n\
         \x20   return 5\n\
         }\n\
         \n\
         comptime A = three()\n\
         \n\
         fn f(t: i64 = scaled()) -> i64 {\n\
         \x20   return t\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{A} {f()}\")\n\
         }\n";
    let rust = lowered(source);
    let dir = common::scratch_dir("comptime-item-unused");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    let _ = std::fs::remove_dir_all(&dir);
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{said}\n{rust}");
    for used in ["helper", "three", "scaled"] {
        assert!(
            !said.contains(&format!("function `{used}` is never used")),
            "`{used}` runs at build time:\n{said}"
        );
    }
    assert!(said.contains("function `unused` is never used"), "{said}");
}
