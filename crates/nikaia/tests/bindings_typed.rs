//! **A name the checker can type is typed** (#497): places where a binding's
//! type is written down nearby and the checker kept it unknown, so a mistake
//! with it reached `rustc` - a pattern inside a pattern, and a lambda whose
//! type the place it stands in declares.

use nikaia::contracts::{Ledger, LedgerOps, STD};
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

fn refused_as(found: &[nikaia::check::Finding], ty: &str) -> usize {
    let message = format!("This value is `{ty}`, but the `let` declares `String`.");
    found
        .iter()
        .filter(|f| f.code == "NK1103" && f.message == message)
        .count()
}

/// **A pattern inside a pattern binds from its part's type** (ADR-291 D10):
/// `E::Add(E::Num(x), E::Num(y))` and `E::Add(E::Neg { inner }, _b)`.
#[test]
fn a_nested_pattern_binds_typed_names() {
    let found = errors(
        "enum E {\n    Num(i64),\n    Add(E, E),\n    Neg { inner: E },\n}\n\n\
         fn f(e: ref E) -> i64 {\n    return match e {\n        \
         E::Add(E::Num(x), E::Num(y)) => {\n            let s: String = x\n            0\n        }\n        \
         E::Add(E::Neg { inner }, _b) => {\n            let t: String = inner\n            0\n        }\n        \
         else => 0,\n    }\n}\n\n\
         fn main() {\n    println(f\"{f(E::Num(1))}\")\n}\n",
    );
    assert_eq!(refused_as(&found, "i64"), 1, "{found:#?}");
    assert_eq!(refused_as(&found, "ref E"), 1, "{found:#?}");
}

/// **A lambda's parameters are what the place it stands in declares**: a
/// `return` against a declared `fn(i64) -> i64`, a field of that type, and an
/// annotated `let`, as a callee's signature types an argument's.
#[test]
fn a_lambdas_parameters_come_from_the_place() {
    let found = errors(
        "struct Step {\n    apply: fn(i64) -> i64,\n}\n\n\
         fn adder(n: i64) -> fn(i64) -> i64 {\n    return fn(x) {\n        let s: String = x\n        x + n\n    }\n}\n\n\
         fn step(n: i64) -> Step {\n    return Step { apply: fn(y) {\n        let t: String = y\n        y + n\n    } }\n}\n\n\
         fn main() {\n    let f: fn(i64) -> i64 = fn(z) {\n        let u: String = z\n        z\n    }\n    \
         let g = adder(1)\n    println(f\"{g(2)} {f(3)} {step(1).apply(1)}\")\n}\n",
    );
    assert_eq!(refused_as(&found, "i64"), 3, "{found:#?}");
}
