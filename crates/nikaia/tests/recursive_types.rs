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

/// **A pattern looks inside a boxed part** (D5, item 1): the lowering binds
/// the part to a name, asks the question in the arm's guard - so the next arm
/// is tried where the part has another shape - and takes the part apart in the
/// arm. Over a lent value and an owned one, a part on either side, two parts
/// at once, and a guard of the program's own beside it.
#[test]
fn a_pattern_looks_inside_a_boxed_part() {
    let source = "enum Expr {\n\
                  \x20   Num(i64),\n\
                  \x20   Add(Expr, Expr),\n\
                  \x20   Neg { inner: Expr },\n\
                  }\n\
                  \n\
                  fn simplify(e: ref Expr) -> i64 {\n\
                  \x20   return match e {\n\
                  \x20       Expr::Add(Expr::Num(0), b) => simplify(b),\n\
                  \x20       Expr::Add(a, Expr::Num(0)) => simplify(a),\n\
                  \x20       Expr::Add(Expr::Num(x), Expr::Num(y)) => x + y,\n\
                  \x20       Expr::Add(Expr::Neg { inner }, b) if simplify(b) > 100 => 100 - simplify(inner),\n\
                  \x20       Expr::Add(a, b) => simplify(a) + simplify(b),\n\
                  \x20       Expr::Num(n) => n,\n\
                  \x20       Expr::Neg { inner } => 0 - simplify(inner),\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn first(e: Expr) -> i64 {\n\
                  \x20   return match e {\n\
                  \x20       Expr::Add(Expr::Num(n), _) => n,\n\
                  \x20       else => -1,\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(simplify(Expr::Add(Expr::Num(0), Expr::Num(5))))\n\
                  \x20   println(simplify(Expr::Add(Expr::Num(7), Expr::Num(0))))\n\
                  \x20   println(simplify(Expr::Add(Expr::Num(2), Expr::Num(3))))\n\
                  \x20   println(simplify(Expr::Add(Expr::Neg { inner: Expr::Num(1) }, Expr::Num(500))))\n\
                  \x20   println(simplify(Expr::Add(Expr::Neg { inner: Expr::Num(1) }, Expr::Num(5))))\n\
                  \x20   println(first(Expr::Add(Expr::Num(9), Expr::Num(1))))\n\
                  \x20   println(first(Expr::Num(3)))\n\
                  }\n";
    runs("nested", source, "5\n7\n5\n99\n4\n9\n-1\n");
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
    runs(
        "guard-inside-a-box",
        source,
        "zero\nadd\nnamed x\nnamed long\nadd\n5 -1\n",
    );
}

/// **A guard reads a part bound whole out of a box as the part** (D5 item 2,
/// #97). The arm opens the box at its head, and a guard runs before that, so
/// the guard reads it through the box: a call that is lent it works as before,
/// and a comparison with a value of the part's type - `&Box<Expr>` against an
/// `Expr` below - now reads the part.
#[test]
fn a_guard_reads_a_boxed_part_as_the_part() {
    let source = "enum Expr {\n\
                  \x20   Num(i64),\n\
                  \x20   Add(Expr, Expr),\n\
                  }\n\
                  \n\
                  fn is_zero(e: ref Expr) -> bool {\n\
                  \x20   return match e {\n\
                  \x20       Expr::Num(n) => n == 0,\n\
                  \x20       else => false,\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn simplify(e: Expr) -> String {\n\
                  \x20   return match e {\n\
                  \x20       Expr::Add(a, b) if is_zero(a) => \"left zero\",\n\
                  \x20       Expr::Add(a, b) if a == Expr::Num(1) && b != Expr::Num(1) => \"left one\",\n\
                  \x20       Expr::Add(a, b) => \"add\",\n\
                  \x20       Expr::Num(n) => f\"{n}\",\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(simplify(Expr::Add(Expr::Num(0), Expr::Num(1))))\n\
                  \x20   println(simplify(Expr::Add(Expr::Num(1), Expr::Num(2))))\n\
                  \x20   println(simplify(Expr::Add(Expr::Num(1), Expr::Num(1))))\n\
                  }\n";
    runs("guard-reads-a-box", source, "left zero\nleft one\nadd\n");
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
