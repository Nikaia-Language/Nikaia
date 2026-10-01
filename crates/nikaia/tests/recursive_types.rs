//! **A type that holds itself holds itself through a box the compiler writes**
//! ([ADR-246](../../../docs/specification/adr/adr-246.md)).
//!
//! `enum Expr { Add(Expr, Expr) }` is a program as written. The fields on a
//! ring of types that hold each other inline go behind a `Box` below; a program
//! builds, matches and reads them as the types it declared. Every test here
//! **runs a program**, at both settings of `user_parallelism`, because what is
//! claimed is that the box is invisible - which a test of the emitted text
//! could not show.
//!
//! Two defects found on the way are closed here too, because a syntax tree
//! meets both on its first line: a literal handed to a variant's text part was
//! not built into text of its own (`Stmt::Say("a")`), and a number bound out of
//! a lent value was a reference (`Expr::Num(n) => n` over a `ref Expr`).

mod common;

use std::process::Command;

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

fn ran(purpose: &str, source: &str, how: Build) -> String {
    let rust = lowered(source, how);
    let dir = common::scratch_dir(&format!("recursive-{purpose}"));
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
    assert!(
        out.status.success(),
        "{purpose} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn runs(purpose: &str, source: &str, expected: &str) {
    let found = findings(source);
    assert!(found.is_empty(), "{purpose}: {found:#?}");
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran(purpose, source, how), expected, "{purpose} at {how:?}");
    }
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
    runs("tree", source, "(2 + (3 * -4)) = -10\nleft = 2\n");
    // **The box is below, and only there**: in the declaration, around what
    // is built, and nowhere the program wrote a word about it.
    let rust = lowered(source, Build::default());
    assert!(rust.contains("Add(Box<Expr>, Box<Expr>)"), "{rust}");
    assert!(rust.contains("Neg { inner: Box<Expr> }"), "{rust}");
    assert!(rust.contains("Box::new(Expr::Num(2))"), "{rust}");
    assert!(!rust.contains("Num(Box"), "{rust}");
}

/// **A list linked by a nullable field** (D1): `next: Node?` holds a `Node`
/// inline when it is there, so it is boxed too. Built with `null` and with a
/// node, read through `?.`, assigned, and copied with `with`.
#[test]
fn a_linked_list_is_built_read_and_changed() {
    let source = "struct Node {\n\
                  \x20   value: i64,\n\
                  \x20   next: Node?,\n\
                  }\n\
                  \n\
                  fn two(n: ref Node) -> i64 {\n\
                  \x20   return n.value + (n.next?.value ?? 0)\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let tail = Node { value: 3, next: null }\n\
                  \x20   let mid = Node { value: 2, next: tail }\n\
                  \x20   let mut head = Node { value: 1, next: mid }\n\
                  \x20   println(f\"{two(head)}\")\n\
                  \x20   head.next = Node { value: 10, next: null }\n\
                  \x20   println(f\"{two(head)}\")\n\
                  \x20   let other = head with { value: 5 }\n\
                  \x20   let last = other with { next: null }\n\
                  \x20   println(f\"{two(other)} {two(last)}\")\n\
                  }\n";
    runs("list", source, "3\n11\n15 5\n");
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
    runs("ring", source, "2 3\n");
    let rust = lowered(source, Build::default());
    assert!(rust.contains("Apply(Box<Call>)"), "{rust}");
    assert!(rust.contains("arg: Box<Expr>"), "{rust}");
    assert!(rust.contains("stmts: Vec<Stmt>"), "{rust}");
    assert!(rust.contains("Nested(Block)"), "{rust}");
}

/// **A pattern that looks inside a boxed part is refused by name** (D4), with
/// the way that works today; the lowering that rewrites it is the next step.
#[test]
fn a_pattern_inside_a_box_is_refused_with_the_way_that_works() {
    let source = "enum Expr { Num(i64), Add(Expr, Expr) }\n\
                  \n\
                  fn f(e: ref Expr) -> i64 {\n\
                  \x20   return match e {\n\
                  \x20       Expr::Add(Expr::Num(a), b) => a,\n\
                  \x20       else => 0,\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn main() { println(f\"{f(Expr::Num(1))}\") }\n";
    let found: Vec<_> = findings(source)
        .into_iter()
        .filter(|f| f.code == "NK1193")
        .collect();
    assert_eq!(found.len(), 1, "{:#?}", findings(source));
    assert!(
        found[0].message.contains("part 0 of `Expr::Add`"),
        "{found:#?}"
    );
    assert_eq!(
        found[0].help.as_deref(),
        Some("Bind the part to a name here, and `match` on that name inside the arm.")
    );
}

/// **The two defects a syntax tree meets first**, in a type that holds nothing
/// of itself: a literal handed to a variant's text part, and a number bound
/// out of a lent `match`.
#[test]
fn a_variants_text_is_built_and_a_lent_number_comes_out_as_a_number() {
    let source = "enum Token { Word(String), Count(i64), End }\n\
                  \n\
                  fn weight(t: ref Token) -> i64 {\n\
                  \x20   return match t {\n\
                  \x20       Token::Word(w) => w.len() as i64,\n\
                  \x20       Token::Count(n) => n,\n\
                  \x20       Token::End => 0,\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let owned: String = \"xyz\"\n\
                  \x20   let tokens = [Token::Word(\"ab\"), Token::Count(5), Token::Word(owned), Token::End]\n\
                  \x20   let mut total = 0\n\
                  \x20   for t in tokens { total += weight(t) }\n\
                  \x20   println(f\"{total}\")\n\
                  }\n";
    runs("token", source, "10\n");
}
