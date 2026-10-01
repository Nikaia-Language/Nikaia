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

mod common;

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

/// The lowering, compiled and run — the way `borrowed_subject.rs` does it.
fn ran(purpose: &str, source: &str) -> String {
    let rust = lowered(source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering does not compile:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr),
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("the program runs");
    let printed = String::from_utf8_lossy(&ran.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    printed
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
    // A struct of one float is also a copy (ADR-252 D4.1).
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

/// **And the whole of it runs**, which is what says the derives are the right
/// ones rather than merely present.
#[test]
fn the_comparisons_run() {
    let printed = ran(
        "comparison",
        "enum Kind { Get, Post }\n\
         \n\
         struct P { x: i64, name: String }\n\
         \n\
         struct Reading { temp: f64 }\n\
         \n\
         fn main() {\n\
         \x20   let k = Kind::Post\n\
         \x20   if k == Kind::Post { print(\"enum \") }\n\
         \x20   if k != Kind::Get { print(\"not \") }\n\
         \x20   let a = P { x: 1, name: \"a\" }\n\
         \x20   let b = P { x: 1, name: \"a\" }\n\
         \x20   if a == b { print(\"struct \") }\n\
         \x20   let one = Reading { temp: 1.5 }\n\
         \x20   let two = Reading { temp: 1.5 }\n\
         \x20   if one == two { print(\"float\") }\n\
         }\n",
    );
    assert_eq!(printed.trim(), "enum not struct float");
}

/// **A view compared with a value is read, whatever the type**
/// (issue #171, found moving `fold` into Nikaia): `op == Op::Neg`
/// for an `op: ref Op` compared a `&Op` with an `Op` below. And two
/// comparisons of one name in one statement are told apart - only the one
/// against a value reads it.
#[test]
fn a_view_of_a_declared_type_compares_with_a_value() {
    let printed = ran(
        "view-compared",
        "enum Op { Add, Neg }\n\
         \n\
         fn is_neg(op: ref Op) -> bool {\n\
         \x20   return op == Op::Neg\n\
         }\n\
         \n\
         fn both(a: ref Op, b: ref Op) -> bool {\n\
         \x20   return a == b && Op::Add != a\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let o = Op::Neg\n\
         \x20   if is_neg(o) { print(\"neg \") }\n\
         \x20   if both(o, o) { print(\"both\") }\n\
         }\n",
    );
    assert_eq!(printed.trim(), "neg both");
}

/// **A name a `for` lends over a list of text compares with text of its own**:
/// `for one in words` binds a `&String` below, and `one == named` for a
/// `named: String` compared a `&String` with a `String`, which has no impl
/// there - `rustc` about a file nobody wrote (found moving `describe` into
/// Nikaia, #124). Against a literal and against a view it stays as written.
#[test]
fn a_lent_text_compares_with_owned_text() {
    let printed = ran(
        "lent-text-compared",
        "fn has(words: ref Vec[String], wanted: ref String) -> bool {\n\
         \x20   let named = wanted.trim().clone()\n\
         \x20   for one in words {\n\
         \x20       if one == named { return true }\n\
         \x20   }\n\
         \x20   return false\n\
         }\n\
         \n\
         fn literal(words: ref Vec[String]) -> bool {\n\
         \x20   for one in words {\n\
         \x20       if one == \"b\" || one == words[0] { return true }\n\
         \x20   }\n\
         \x20   return false\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let words: Vec[String] = [\"a\", \"b\"]\n\
         \x20   let found = has(words, \" b \")\n\
         \x20   let missing = has(words, \"c\")\n\
         \x20   let lit = literal(words)\n\
         \x20   println(f\"{found} {missing} {lit}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "true false true");
}
