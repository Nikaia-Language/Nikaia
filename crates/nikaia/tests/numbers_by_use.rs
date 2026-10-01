//! **A number takes the type its uses ask for, and this compiler decides it**
//! ([ADR-249](../../../docs/specification/adr/adr-249.md)): an unannotated
//! `let` of a number is typed by what is done with it - the parameter it is
//! handed to, the value it is put beside, the index it is, the numbers it is
//! given later - and the type is written into the generated `let`, so nothing
//! is left to the language below's inference. And a constant operation that
//! overflows is `NK1116` wherever it stands (issue #176, closed).

mod common;

use std::process::Command;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

/// Runs the program at both settings of `user_parallelism` and hands back the
/// Rust it was lowered to, for the line a test wants to see.
fn runs(purpose: &str, source: &str, expected: &str) -> String {
    let found = findings(source);
    assert!(found.is_empty(), "{purpose}: {found:#?}");
    let mut lowered = String::new();
    for how in [Build::default(), Build::parallel()] {
        let parsed = parse_to_ast(source).expect("the source parses");
        let rust = emit_program(&parsed, how).expect("it lowers").rust;
        let dir = common::scratch_dir(&format!("numbers-{purpose}"));
        let path = dir.join("program.rs");
        std::fs::write(&path, &rust).expect("write the Rust");
        let binary = dir.join("program");
        let compiled = common::compile(
            &path,
            &["--crate-type", "bin", "-o", &binary.to_string_lossy()],
        );
        assert!(
            compiled.status.success(),
            "{purpose} did not compile at {how:?}:\n{}\n--- emitted ---\n{rust}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let out = Command::new(&binary).output().expect("run it");
        assert!(
            out.status.success(),
            "{purpose} failed at {how:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            expected,
            "{purpose} at {how:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
        lowered = rust;
    }
    lowered
}

/// **An index is a use** (D2): `v[i].push(x)` over a bare `let i = 0` was
/// `rustc`'s *type annotations needed* (issue #177), because
/// `index::at` takes any integer and nothing else said which.
#[test]
fn an_index_makes_a_bare_number_an_i64() {
    let rust = runs(
        "index",
        "fn main() {\n\
         \x20   let mut rows: Vec[Vec[i64]] = [[], []]\n\
         \x20   let i = 1\n\
         \x20   rows[i].push(7)\n\
         \x20   println(f\"{rows[1].len()} {rows[1][0]}\")\n\
         }\n",
        "1 7\n",
    );
    assert!(rust.contains("let i: i64 = 1;"), "{rust}");
}

/// **A parameter, an annotation and a comparison are uses** (D2), and each
/// types the number it is given - `u32` and `u64` included, which a number
/// above an `i32` could not be before (ADR-060 D3 is answered).
#[test]
fn the_use_decides_among_all_the_integer_types() {
    runs(
        "uses",
        "fn wide(n: u64) -> u64 {\n\
         \x20   return n * 2\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let big = 3000000000\n\
         \x20   let small = 5\n\
         \x20   let mask: u32 = small\n\
         \x20   let v: Vec[i64] = [1, 2, 3]\n\
         \x20   let mut i = 0\n\
         \x20   let mut total = 0\n\
         \x20   while i < v.len() {\n\
         \x20       total += v[i]\n\
         \x20       i += 1\n\
         \x20   }\n\
         \x20   println(f\"{wide(big)} {mask} {i} {total}\")\n\
         }\n",
        "6000000000 5 3 6\n",
    );
}

/// **What is given later is held to the type too** (D3), and a sum through
/// names is wide where a use asks for it to be: `a + a` over a bare
/// `2000000000` is an `i64` handed to an `i64`, and still refused where
/// nothing asks - a name is where the widening stops (ADR-063 D2).
#[test]
fn what_a_number_is_given_decides_where_no_use_does() {
    runs(
        "given",
        "fn wide(n: i64) -> i64 {\n\
         \x20   return n\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let mut n = 1\n\
         \x20   n = 3000000000\n\
         \x20   let a = 2000000000\n\
         \x20   let c = a + a\n\
         \x20   println(f\"{n} {wide(c)}\")\n\
         }\n",
        "3000000000 4000000000\n",
    );
    let unasked = findings(
        "fn main() {\n\
         \x20   let a = 2000000000\n\
         \x20   let c = a + a\n\
         }\n",
    );
    assert!(
        unasked.iter().any(|f| f.code == "NK1116"
            && f.message == "This comes to 4000000000, which doesn't fit in an `i32`."),
        "{unasked:#?}"
    );
}

/// **Two uses that ask two types** are `NK1200` (D2), naming both, where the
/// language below said *mismatched types* about the generated file.
#[test]
fn two_uses_that_disagree_are_refused_in_this_languages_words() {
    let found = findings(
        "fn narrow(n: i32) -> i32 {\n\
         \x20   return n\n\
         }\n\
         \n\
         fn wide(n: i64) -> i64 {\n\
         \x20   return n\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let k = 5\n\
         \x20   println(f\"{narrow(k)} {wide(k)}\")\n\
         }\n",
    );
    assert!(
        found.iter().any(|f| f.code == "NK1200"
            && f.message == "You're using `k` as an `i32` and here as an `i64`."
            && f.help.as_deref()
                == Some(
                    "Declare its type, `let k: i32 = …`, and convert with `as` where the \
                 other one is needed."
                )),
        "{found:#?}"
    );
}

/// **An operation that overflows is refused where it stands** (issue #176
/// issue #176): in an `f"…"` hole, a condition, a list and a receiver, each once,
/// where only a `let`, a `return` and an argument were asked before.
#[test]
fn an_overflow_is_refused_wherever_the_operation_stands() {
    for (place, line) in [
        ("hole", "println(f\"{a + b}\")"),
        (
            "condition",
            "if a + b + 1 > 0 {\n        println(\"x\")\n    }",
        ),
        ("list", "let v = [a + b]"),
        ("receiver", "let d = (a + b).abs()"),
    ] {
        let found = findings(&format!(
            "fn main() {{\n    let a: i32 = 2000000000\n    let b: i32 = 2000000000\n    {line}\n}}\n"
        ));
        assert_eq!(found.len(), 1, "{place}: {found:#?}");
        assert!(
            found[0].code == "NK1116"
                && found[0].message == "This comes to 4000000000, which doesn't fit in an `i32`.",
            "{place}: {found:#?}"
        );
    }
    runs(
        "fits",
        "fn main() {\n\
         \x20   let a: i32 = 2000000000\n\
         \x20   let b: i32 = 100\n\
         \x20   println(f\"{a + b} {a - b}\")\n\
         }\n",
        "2000000100 1999999900\n",
    );
}
