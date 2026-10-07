//! `==` on a declared type
//! ([ADR-204](../../../docs/specification/adr/adr-204.md)).
//!
//! It had no lowering at all. `a == b` over a `struct` reached `rustc` as *binary
//! operation `==` cannot be applied to type `P`*, with *consider annotating `P`
//! with `#[derive(PartialEq)]`* as the help — the backend's words about a file
//! nobody wrote, and a way out the source cannot take: [Part III C.1 and
//! C.2](../../../docs/specification/30-nikaia-tooling.md) at once.
//!
//! **Both directions are the point.** A type whose parts compare must compare, or
//! a correct program is refused (C.4); and a type whose parts do not must be
//! refused **here**, in this language's words, or C.1 stays open one type over.
//!
//! What the comparisons answer when they run is
//! `tests/language/src/comparison.nika`; this file keeps the derives and the
//! refusals.

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn refusals(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.code == "NK1188")
        .collect()
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// **A `struct` of parts that compare compares**, and it says both words.
#[test]
fn a_struct_of_comparable_parts_derives_both() {
    let source = "struct P { x: i64, name: String }\n\
                  fn main() {\n\
                  \x20   let a = P { x: 1, name: \"a\" }\n\
                  \x20   let b = P { x: 1, name: \"a\" }\n\
                  \x20   if a == b { print(\"equal\") }\n\
                  }\n";
    assert!(refusals(source).is_empty(), "{:#?}", refusals(source));
    let rust = lowered(source);
    assert!(
        rust.contains("#[derive(Debug, Clone, PartialEq, Eq)]"),
        "{rust}"
    );
}

/// **A float compares and is not an equivalence**, because `NaN != NaN` — so the
/// type gets `PartialEq` and not `Eq`, which is Rust's own rule and the one the
/// derive would have failed on.
#[test]
fn a_float_anywhere_takes_the_second_word_away() {
    let rust = lowered(
        "struct Reading { temp: f64 }\n\
         fn main() { print(\"x\") }\n",
    );
    // A struct of one float is also a copy (ADR-294 D9.1).
    assert!(
        rust.contains("#[derive(Debug, Clone, Copy, PartialEq)]"),
        "{rust}"
    );
    assert!(!rust.contains("PartialEq, Eq"), "{rust}");
}

/// **And it is a walk and not a look at the syntax**: a `struct` holding a type
/// that holds a float is the same answer, however far down.
///
/// `examples/json/src/main.nika` is what found this: `Json::Number(f64)` made `Json`
/// `PartialEq`, and `Document { value: Json }` was derived `Eq` beside it —
/// *the trait bound `Json<'a>: Eq is not satisfied*, about a generated file.
#[test]
fn the_float_travels_through_a_declaration() {
    let rust = lowered(
        "enum Held { Nothing, Number(f64) }\n\
         \n\
         struct Document { value: Held }\n\
         \n\
         fn main() { print(\"x\") }\n",
    );
    for line in rust.lines() {
        assert!(
            !line.contains("PartialEq, Eq"),
            "neither of the two may be an equivalence:\n{rust}"
        );
    }
    assert_eq!(
        rust.matches("#[derive(Debug, Clone, Copy, PartialEq)]")
            .count(),
        2,
        "{rust}"
    );
}

/// **A positional payload is a part too**, which is the shape nothing recorded:
/// a `Named` variant's fields were in the checker's map and a `Tuple` variant's
/// were in no map at all, so `Number(f64)` looked like a variant holding nothing.
#[test]
fn a_positional_payload_is_read() {
    let rust = lowered(
        "use std::net\n\
         \n\
         enum Held { Nothing, Wire(net::Connection) }\n\
         \n\
         fn main() { print(\"x\") }\n",
    );
    assert!(
        !rust.contains("PartialEq"),
        "a socket does not compare, so nothing holding one does:\n{rust}"
    );
}

/// **A type whose parts do not compare is refused here**, in this language's
/// words and on the author's line.
#[test]
fn a_type_that_does_not_compare_is_refused_by_name() {
    let found = refusals(
        "use std::net\n\
         \n\
         struct Shut { conn: net::Connection }\n\
         \n\
         fn main() throws {\n\
         \x20   let a = Shut { conn: net::connect(\"127.0.0.1:1\") }\n\
         \x20   let b = Shut { conn: net::connect(\"127.0.0.1:2\") }\n\
         \x20   if a == b { print(\"equal\") }\n\
         }\n",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("`Shut`"), "{}", found[0].message);
    // The help is one a program can take: there is no `#[derive]` to write.
    let help = found[0].help.as_deref().expect("every refusal has one");
    assert!(!help.contains("derive"), "{help}");
}

/// **A `std` type compares where `std` says so, and not otherwise.**
///
/// `compares = true` is a claim reviewed like code, for the reason `crosses` is:
/// such a type's parts are Rust, and nothing in the compiler can walk them. Its
/// absence is *no*, which is [ADR-010](../../../docs/specification/adr/adr-010.md)
/// D1's polarity.
#[test]
fn a_library_type_compares_where_the_ledger_says_so() {
    let said = refusals(
        "use std::io\n\
         fn f(a: io::IoError, b: io::IoError) -> bool { return a == b }\n",
    );
    assert!(said.is_empty(), "{said:#?}");

    let refused = refusals(
        "use std::net\n\
         fn f(a: net::Connection, b: net::Connection) -> bool { return a == b }\n",
    );
    assert_eq!(refused.len(), 1, "{refused:#?}");
}

/// **A type this compiler could not work out compares**, which is the answer an
/// absent claim gets everywhere here ([Part III
/// C.4](../../../docs/specification/30-nikaia-tooling.md)): a refusal on a guess
/// is a correct program refused.
#[test]
fn an_unknown_type_is_left_alone() {
    let found = refusals("fn f(xs: Vec[i64]) -> bool { return xs.first() == xs.last() }\n");
    assert!(found.is_empty(), "{found:#?}");
}

/// **A name nothing here knows is not asked whether it compares** (#379,
/// ADR-204 D1): no declaration, no ledger entry, and a module no ledger knows.
/// It is another refusal's business, and `NK1188` beside it would name the
/// wrong cause - also for a declared type that holds one.
#[test]
fn a_name_nobody_knows_is_left_to_its_own_refusal() {
    let found = refusals(
        "struct P { h: nowhere::Handle }\n\
         fn same(a: ref P, b: ref P) -> bool { return a == b }\n\
         fn other(a: ref nowhere::Handle, b: ref nowhere::Handle) -> bool { return a == b }\n",
    );
    assert!(found.is_empty(), "{found:#?}");
}

/// **A name a ledger knows answers from its column**, whose absence is *no*:
/// `std`'s lock has no entry of its own and is still known, and a module `std`
/// has refuses a type it does not offer rather than taking it for unknown.
#[test]
fn a_known_name_without_the_column_is_still_refused() {
    let lock = refusals(
        "struct Q { h: Locked[i64] }\n\
         fn same(a: ref Q, b: ref Q) -> bool { return a == b }\n",
    );
    assert_eq!(lock.len(), 1, "{lock:#?}");

    let mapped = refusals(
        "use std::fs\n\
         fn same(a: ref fs::Mapped, b: ref fs::Mapped) -> bool { return a == b }\n",
    );
    assert_eq!(mapped.len(), 1, "{mapped:#?}");
}
