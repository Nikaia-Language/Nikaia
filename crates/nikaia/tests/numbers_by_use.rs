//! **A number takes the type its uses ask for, and this compiler decides it**
//! ([ADR-285](../../../docs/specification/adr/adr-285.md)): an unannotated
//! `let` of a number is typed by what is done with it - the parameter it is
//! handed to, the value it is put beside, the index it is, the numbers it is
//! given later - and the type is written into the generated `let`, so nothing
//! is left to the language below's inference. And a constant operation that
//! overflows is `NK1116` wherever it stands (issue #176, closed).
//!
//! What the programs compute is `tests/language/src/numbers_by_use.nika`; this
//! file keeps the generated `let` and the refusals.

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

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust
}

/// **An index is a use** (D2): `v[i].push(x)` over a bare `let i = 0` was
/// `rustc`'s *type annotations needed* (issue #177), because
/// `index::at` takes any integer and nothing else said which. The type is
/// written into the generated `let`.
#[test]
fn an_index_makes_a_bare_number_an_i64() {
    let rust = lowered(
        "fn main() {\n\
         \x20   let mut rows: Vec[Vec[i64]] = [[], []]\n\
         \x20   let i = 1\n\
         \x20   rows[i].push(7)\n\
         \x20   println(f\"{rows[1].len()} {rows[1][0]}\")\n\
         }\n",
    );
    assert!(rust.contains("let i: i64 = 1;"), "{rust}");
}

/// **A sum through names is refused where nothing asks it to be wide** - a
/// name is where the widening stops (ADR-285 D26). Where a use asks, it is wide:
/// `tests/language/src/numbers_by_use.nika`.
#[test]
fn a_sum_nothing_asks_to_be_wide_is_refused() {
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
}

/// **A use that asks for a type no number can be is refused at that use**
/// (#554): `let n = 12` and then `let s: String = n` passed the checker, the
/// number being unknown to it until a use decides, and reached `rustc` as
/// *mismatched types*. Each typed place refuses it with its own code.
#[test]
fn a_number_where_no_number_fits_is_refused_at_the_use() {
    for (place, code, message, source) in [
        (
            "let",
            "NK1103",
            "This value is a number, but the `let` declares `String`.",
            "fn main() {\n    let n = 12\n    let s: String = n\n}\n",
        ),
        (
            "argument",
            "NK1102",
            "`f` expects `s` to be `String`, but you're passing a number.",
            "fn f(s: String) {\n    println(s)\n}\n\nfn main() {\n    let n = 12\n    f(n)\n}\n",
        ),
        (
            "field",
            "NK1106",
            "`P.name` holds `String`, but you're giving it a number.",
            "struct P {\n    name: String,\n}\n\nfn main() {\n    let n = 12\n    let p = P { name: n }\n}\n",
        ),
        (
            "return",
            "NK1104",
            "You're returning a number, but the function is declared to return `String`.",
            "fn g() -> String {\n    let n = 3\n    return n + 1\n}\n\nfn main() {\n    println(g())\n}\n",
        ),
        (
            "bool",
            "NK1103",
            "This value is a number, but the `let` declares `bool`.",
            "fn main() {\n    let n = 12\n    let b: bool = n\n}\n",
        ),
    ] {
        let found = findings(source);
        assert!(
            found.len() == 1 && found[0].code == code && found[0].message == message,
            "{place}: {found:#?}"
        );
    }
}

/// **A number type still decides it** (#554): `let x: u8 = n` types `n` as a
/// `u8` and passes.
#[test]
fn a_number_type_still_decides_the_number() {
    let source = "fn main() {\n    let n = 12\n    let x: u8 = n\n    println(f\"{x}\")\n}\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("let n: u8 = 12"), "{rust}");
}

/// **A list written in numbers has one open number for its elements**
/// (ADR-285 D34, #513): a read of an element is a use of it, as a read of a
/// name is. `let s: String = readings[0]` reached `rustc` as *mismatched
/// types*; two `2000000000`s summed through the list stopped at run time where
/// the same sum through names was refused while building.
#[test]
fn a_lists_elements_are_refused_as_a_names_number_is() {
    for (place, code, message, source) in [
        (
            "no number fits",
            "NK1103",
            "This value is a number, but the `let` declares `String`.",
            "fn main() {\n    let readings = [12, 14, 13]\n    let s: String = readings[0]\n}\n",
        ),
        (
            "an overflow",
            "NK1116",
            "This comes to 4000000000, which doesn't fit in an `i32`.",
            "fn main() {\n    let xs = [2000000000, 2000000000]\n    println(f\"{xs[0] + xs[1]}\")\n}\n",
        ),
        (
            "two uses",
            "NK1200",
            "You're using the elements of `xs` as `u8`s and here as `i32`s.",
            "fn a(xs: Vec[u8]) -> i64 {\n    return xs.len()\n}\n\n\
             fn b(xs: Vec[i32]) -> i64 {\n    return xs.len()\n}\n\n\
             fn main() {\n    let xs = [1, 2]\n    println(f\"{a(xs)} {b(xs)}\")\n}\n",
        ),
    ] {
        let found = findings(source);
        assert!(
            found.len() == 1 && found[0].code == code && found[0].message == message,
            "{place}: {found:#?}"
        );
    }
}

/// **The type the uses decide is written into the generated list** (D34, D27),
/// and one no use asked for is the first that holds every element.
#[test]
fn a_lists_decided_type_is_written() {
    let rust = lowered(
        "fn total(xs: Vec[u8]) -> i64 {\n    return xs.len()\n}\n\n\
         fn main() {\n    let ys = [1, 2, 3]\n    println(f\"{total(ys)}\")\n}\n",
    );
    assert!(rust.contains("let ys: Vec<u8> = vec![1, 2, 3];"), "{rust}");
    let rust = lowered("fn main() {\n    let xs = [1, 3000000000]\n    println(f\"{xs[1]}\")\n}\n");
    assert!(rust.contains("vec![1, 3000000000i64]"), "{rust}");
}
