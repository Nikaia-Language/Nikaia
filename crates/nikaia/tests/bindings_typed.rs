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

/// **A grammar binds `dec[f64](…)` as an `f64`** (Part II 10.8: any number
/// type), **and a choice whose alternatives agree as their type**.
#[test]
fn a_grammar_binding_of_a_float_and_of_a_choice_is_typed() {
    let found = errors(
        "enum Shade { Odd, Even }\n\n\
         grammar Pick {\n    \
         rule NUM -> String = n:dec[f64](text(digit+ (\".\" digit+)?)) { n }\n    \
         rule ONE -> Shade = \"odd\" { Shade::Odd }\n    \
         rule TWO -> Shade = \"even\" { Shade::Even }\n    \
         rule SHADE -> i64 = s:(ONE | TWO) { s }\n    \
         entry rule many -> Vec[i64] = shades:SHADE* { shades }\n}\n\n\
         fn main() {\n    let r = Pick::many(\"odd\") catch { [] }\n    println(f\"{r.len()}\")\n}\n",
    );
    let said: Vec<&str> = found
        .iter()
        .filter(|f| f.code == "NK1104")
        .map(|f| f.message.as_str())
        .collect();
    assert!(
        said.contains(&"This action builds `f64`, but its rule returns `String`.")
            && said.contains(&"This action builds `Shade`, but its rule returns `i64`."),
        "{found:#?}"
    );
}

/// **A field reached through `Shared` is the field of what it holds**: `db.host`
/// on a `Shared[C]` is `C`'s `String`.
#[test]
fn a_field_through_shared_is_typed() {
    let found = errors(
        "struct C {\n    host: String,\n}\n\n\
         fn serve(db: Shared[C]) -> i64 {\n    let n: i64 = db.host\n    n\n}\n\n\
         fn main() {\n    println(f\"{serve(Shared(C { host: \"x\" }))}\")\n}\n",
    );
    assert_eq!(refused_as_i64(&found), 1, "{found:#?}");
}

fn refused_as_i64(found: &[nikaia::check::Finding]) -> usize {
    found
        .iter()
        .filter(|f| {
            f.code == "NK1103"
                && f.message == "This value is `String`, but the `let` declares `i64`."
        })
        .count()
}

/// **An `if` of a typed number and a literal is the typed one** (ADR-285 D24):
/// `if at >= 2 { at - 2 } else { 0 }` is an `i64`.
#[test]
fn a_typed_arm_beside_a_literal_types_the_if() {
    let found = errors(
        "fn f(at: i64) -> i64 {\n    let from = if at >= 2 { at - 2 } else { 0 }\n    let s: String = from\n    0\n}\n\n\
         fn main() {\n    println(f\"{f(3)}\")\n}\n",
    );
    assert_eq!(refused_as(&found, "i64"), 1, "{found:#?}");
}
