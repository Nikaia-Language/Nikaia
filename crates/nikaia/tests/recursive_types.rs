//! **A type that holds itself holds itself through a box the compiler writes**
//! ([ADR-246](../../../docs/specification/adr/adr-246.md)).
//!
//! `enum Expr { Add(Expr, Expr) }` is a program as written. The fields on a
//! ring of types that hold each other inline go behind a `Box` below; a program
//! builds, matches and reads them as the types it declared. That the box is
//! invisible - which a test of the emitted text could not show - is shown by
//! the programs in `tests/language/src/recursive_types.nika`, run by `nikaia
//! test` at both settings; here stay what the lowering writes and what is
//! refused.
//!
//! Two defects found on the way are closed here too, because a syntax tree
//! meets both on its first line: a literal handed to a variant's text part was
//! not built into text of its own (`Stmt::Say("a")`), and a number bound out of
//! a lent value was a reference (`Expr::Num(n) => n` over a `ref Expr`).

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("it lowers").rust
}

/// **A syntax tree, as written** (D1-D3): built with its constructors, walked
/// by a `match` over a lent value and over an owned one, and a variant written
/// as a struct. The number a leaf holds comes out of the lent `match` as the
/// `i64` its type says.
#[test]
fn an_expression_tree_is_built_matched_and_taken_apart() {
    let source = "enum Expr {\n\
                  \x20   Num(i64),\n\
                  \x20   Add(Expr, Expr),\n\
                  \x20   Mul(Expr, Expr),\n\
                  \x20   Neg { inner: Expr },\n\
                  }\n\
                  \n\
                  fn eval(e: ref Expr) -> i64 {\n\
                  \x20   return match e {\n\
                  \x20       Expr::Num(n) => n,\n\
                  \x20       Expr::Add(a, b) => eval(a) + eval(b),\n\
                  \x20       Expr::Mul(a, b) => eval(a) * eval(b),\n\
                  \x20       Expr::Neg { inner } => 0 - eval(inner),\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn show(e: ref Expr) -> String {\n\
                  \x20   return match e {\n\
                  \x20       Expr::Num(n) => f\"{n}\",\n\
                  \x20       Expr::Add(a, b) => f\"({show(a)} + {show(b)})\",\n\
                  \x20       Expr::Mul(a, b) => f\"({show(a)} * {show(b)})\",\n\
                  \x20       Expr::Neg { inner } => f\"-{show(inner)}\",\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn left(e: Expr) -> Expr {\n\
                  \x20   return match e {\n\
                  \x20       Expr::Add(a, b) => a,\n\
                  \x20       else => Expr::Num(0),\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let tree = Expr::Add(Expr::Num(2), Expr::Mul(Expr::Num(3), Expr::Neg { inner: Expr::Num(4) }))\n\
                  \x20   println(f\"{show(tree)} = {eval(tree)}\")\n\
                  \x20   let l = left(tree)\n\
                  \x20   println(f\"left = {show(l)}\")\n\
                  }\n";
    // **The box is below, and only there**: in the declaration, around what
    // is built, and nowhere the program wrote a word about it.
    let rust = lowered(source, Build::default());
    assert!(rust.contains("Add(Box<Expr>, Box<Expr>)"), "{rust}");
    assert!(rust.contains("Neg { inner: Box<Expr> }"), "{rust}");
    assert!(rust.contains("Box::new(Expr::Num(2))"), "{rust}");
    assert!(!rust.contains("Num(Box"), "{rust}");
}

/// **A ring through another type** (D1): `Expr` holds a `Call`, which holds an
/// `Expr`. Both edges are on the ring and both are boxed; a field read off the
/// inner type goes through its box. A list is an indirection already, so a
/// type held through one is left alone.
#[test]
fn a_ring_through_another_type_is_boxed_on_both_edges() {
    let source = "enum Expr {\n\
                  \x20   Lit(i64),\n\
                  \x20   Apply(Call),\n\
                  }\n\
                  \n\
                  struct Call {\n\
                  \x20   name: String,\n\
                  \x20   arg: Expr,\n\
                  }\n\
                  \n\
                  struct Block { stmts: Vec[Stmt] }\n\
                  enum Stmt { Say(String), Nested(Block) }\n\
                  \n\
                  fn depth(e: ref Expr) -> i64 {\n\
                  \x20   return match e {\n\
                  \x20       Expr::Lit(n) => 0,\n\
                  \x20       Expr::Apply(call) => 1 + depth(call.arg),\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn count(b: ref Block) -> i64 {\n\
                  \x20   let mut n = 0\n\
                  \x20   for s in b.stmts {\n\
                  \x20       n += match s {\n\
                  \x20           Stmt::Say(t) => 1,\n\
                  \x20           Stmt::Nested(inner) => count(inner),\n\
                  \x20       }\n\
                  \x20   }\n\
                  \x20   return n\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let e = Expr::Apply(Call { name: \"f\", arg: Expr::Apply(Call { name: \"g\", arg: Expr::Lit(1) }) })\n\
                  \x20   let inner = Block { stmts: [Stmt::Say(\"a\"), Stmt::Say(\"b\")] }\n\
                  \x20   let outer = Block { stmts: [Stmt::Say(\"x\"), Stmt::Nested(inner)] }\n\
                  \x20   println(f\"{depth(e)} {count(outer)}\")\n\
                  }\n";
    let rust = lowered(source, Build::default());
    assert!(rust.contains("Apply(Box<Call>)"), "{rust}");
    assert!(rust.contains("arg: Box<Expr>"), "{rust}");
    assert!(rust.contains("stmts: Vec<Stmt>"), "{rust}");
    assert!(rust.contains("Nested(Block)"), "{rust}");
}

/// **An arm that looks inside a box covers no variant on its own** (D5):
/// `Expr::Add(Expr::Num(n), b)` is not every `Add`, so a `match` whose only
/// `Add` arm is one is missing a case - said here, rather than by `rustc`
/// about the guard the lowering writes.
#[test]
fn an_arm_that_looks_inside_a_box_does_not_cover_its_variant() {
    let source = "enum Expr { Num(i64), Add(Expr, Expr) }\n\
                  \n\
                  fn f(e: ref Expr) -> i64 {\n\
                  \x20   return match e {\n\
                  \x20       Expr::Add(Expr::Num(a), b) => a,\n\
                  \x20       Expr::Num(n) => n,\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn main() { println(f\"{f(Expr::Num(1))}\") }\n";
    let found = findings(source);
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK1151" && f.message.contains("Expr::Add")),
        "{found:#?}"
    );
    assert!(!found.iter().any(|f| f.code == "NK1193"), "{found:#?}");
}

/// **What the lowering does not reach is refused by name** (D4, D5): a
/// pattern that looks inside a box inside a boxed part, and one that looks
/// inside a box in one alternative of an `|`. (A guard that reads a name bound
/// in there is no longer one of them: D5 item 2, below.)
#[test]
fn a_pattern_too_deep_inside_a_box_is_refused_with_the_way_that_works() {
    for (arm, why) in [
        (
            "Expr::Add(Expr::Add(Expr::Num(a), c), b) => a,",
            "inside a part that is itself behind a pointer",
        ),
        (
            "Expr::Add(Expr::Num(a), b) | Expr::Add(b, Expr::Num(a)) => a,",
            "in one alternative of an `|` pattern",
        ),
    ] {
        let source = format!(
            "enum Expr {{ Num(i64), Add(Expr, Expr) }}\n\
             \n\
             fn f(e: ref Expr) -> i64 {{\n\
             \x20   return match e {{\n\
             \x20       {arm}\n\
             \x20       else => 0,\n\
             \x20   }}\n\
             }}\n\
             \n\
             fn main() {{ println(f\"{{f(Expr::Num(1))}}\") }}\n"
        );
        let found: Vec<_> = findings(&source)
            .into_iter()
            .filter(|f| f.code == "NK1193")
            .collect();
        assert!(!found.is_empty(), "{arm}: {:#?}", findings(&source));
        assert!(found[0].message.contains(why), "{found:#?}");
        assert!(
            found[0]
                .help
                .as_deref()
                .is_some_and(|help| help.contains("`match` on")),
            "the way that works is named: {found:#?}"
        );
    }
}

/// **A guard reads a name bound inside a boxed part** (D5 item 2, #97). The
/// guard runs before the arm takes the part apart, so the lowering binds the
/// names a second time inside the guard, through the box: a number is copied
/// out, text stays a view of the part. Where the guard says no, the next arm is
/// tried - and the arm that is taken still takes the part apart. Over a value
/// the function owns, and over one it was lent.
#[test]
fn a_guard_reads_a_name_bound_inside_a_box() {
    let source = "enum Expr {\n\
                  \x20   Num(i64),\n\
                  \x20   Name(String),\n\
                  \x20   Add(Expr, Expr),\n\
                  }\n\
                  \n\
                  fn owned(e: Expr) -> String {\n\
                  \x20   return match e {\n\
                  \x20       Expr::Add(Expr::Num(n), b) if n == 0 => \"zero\",\n\
                  \x20       Expr::Add(Expr::Name(s), b) if s == \"x\" || s.len() > 3 => f\"named {s}\",\n\
                  \x20       Expr::Add(a, b) => \"add\",\n\
                  \x20       else => \"leaf\",\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn lent(e: ref Expr) -> i64 {\n\
                  \x20   return match e {\n\
                  \x20       Expr::Add(Expr::Num(n), b) if n > 1 => n,\n\
                  \x20       else => -1,\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(owned(Expr::Add(Expr::Num(0), Expr::Num(1))))\n\
                  \x20   println(owned(Expr::Add(Expr::Num(2), Expr::Num(1))))\n\
                  \x20   println(owned(Expr::Add(Expr::Name(\"x\"), Expr::Num(1))))\n\
                  \x20   println(owned(Expr::Add(Expr::Name(\"long\"), Expr::Num(1))))\n\
                  \x20   println(owned(Expr::Add(Expr::Name(\"y\"), Expr::Num(1))))\n\
                  \x20   let big = Expr::Add(Expr::Num(5), Expr::Num(1))\n\
                  \x20   let small = Expr::Add(Expr::Num(1), Expr::Num(1))\n\
                  \x20   println(f\"{lent(big)} {lent(small)}\")\n\
                  }\n";
    let found = findings(source);
    assert!(!found.iter().any(|f| f.code == "NK1193"), "{found:#?}");
}
