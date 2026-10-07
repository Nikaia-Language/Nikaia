//! A text literal where a `String` is wanted is a `String`
//! ([ADR-282](../../../docs/specification/adr/adr-282.md)).
//!
//! `"x"` is a view of static text, and until this record a view put where a
//! `String` was declared was refused until the program wrote `.to_string()` -
//! the refusal every newcomer met first, protecting nothing: the text is a
//! constant, so making a `String` of it is constructing a value, as `[1, 2]`
//! constructs the `Vec` it stands in.
//!
//! **Three halves, and the tests are split the same way.** Where the use keeps
//! the text (a field, an annotated `let`, a `return`, an argument the callee
//! keeps) the literal is constructed there (D2). Where the callee only reads it,
//! the parameter is a `&str` and the literal is handed over as it is, with no
//! allocation at all (D3). And a **view** of text the program has is still
//! refused - that is a copy, and a copy is written (ADR-282 D7).
//!
//! What the programs compute is `tests/language/src/text_literals.nika`; here
//! stay what the literal is lowered to, the refusals, and the callees that
//! print what they read.

mod common;

use nikaia::contracts::LedgerOps;
use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Compile the lowering and run it: the checker's answer and the emitter's
/// writing agree only if the language below accepts both at once.
fn ran(purpose: &str, source: &str) -> String {
    let rust = lowered(source);
    let dir = common::scratch_dir(&format!("text-literals-{purpose}"));
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
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
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    std::fs::remove_dir_all(&dir).ok();
    printed
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

/// Every position that keeps what it is given, in one program.
#[test]
fn a_literal_is_a_string_wherever_one_is_kept() {
    let source = "struct Person {\n\
                      name: String,\n\
                      nick: String?,\n\
                  }\n\
                  fn keep(s: String) -> String { return s }\n\
                  fn label() -> String { return \"label\" }\n\
                  fn main() {\n\
                      let p = Person { name: \"Ada\", nick: \"A\" }\n\
                      let s: String = \"let\"\n\
                      let names: Vec[String] = [\"a\", \"b\", \"c\"]\n\
                      println(f\"{p.name} {p.nick ?? \"-\"} {s} {names.len()} {keep(\"kept\")} {label()}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("name: String::from(\"Ada\")"), "{rust}");
    assert!(rust.contains("Some(String::from(\"A\"))"), "{rust}");
    assert!(rust.contains("keep(String::from(\"kept\"))"), "{rust}");
}

/// **A callee that only reads costs nothing** (D3): the parameter is a `&str`,
/// the literal goes in as it is, and a caller's `String` is lent to the same
/// declaration by the `&` the call already had.
#[test]
fn a_literal_handed_to_a_reader_is_not_allocated() {
    let source = "fn show(s: String) { println(s) }\n\
                  fn main() {\n\
                      let owned = f\"owned\"\n\
                      show(\"lent\")\n\
                      show(owned)\n\
                  }\n";
    let rust = lowered(source);
    assert!(rust.contains("fn show(s: &str)"), "{rust}");
    assert!(rust.contains("show(\"lent\")"), "{rust}");
    assert!(rust.contains("show(&owned)"), "{rust}");
    assert!(!rust.contains("String::from(\"lent\")"), "{rust}");
    assert_eq!(ran("reader", source).trim(), "lent\nowned");
}

/// A literal nobody asked to own stays what it was: a `let` with no annotation,
/// an argument to `println`, a comparison.
#[test]
fn a_literal_nobody_keeps_stays_a_view() {
    let source = "fn main() {\n\
                      let s = \"view\"\n\
                      if s == \"view\" { println(\"same\") }\n\
                  }\n";
    let rust = lowered(source);
    assert!(!rust.contains("String::from"), "{rust}");
}

/// **A view of text the program has is still refused where the field is
/// published and text of its own flows into it too** (ADR-282 D22), and the
/// help says how: that is a copy, and a copy is written where it happens
/// (ADR-282 D7). Anywhere else the field is a view, or both per value.
#[test]
fn a_view_where_a_string_is_kept_is_refused_with_the_way_out() {
    let found = findings(
        "pub struct Person { pub name: String }\n\
         pub fn make(n: ref String) -> Person { return Person { name: n } }\n\
         pub fn built() -> Person { return Person { name: f\"x\" } }\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "NK1106");
    let help = found[0].help.clone().unwrap_or_default();
    assert!(help.contains(".clone()"), "{help}");
}

/// **A list a view goes into is a list of views** (ADR-282): the literal
/// beside it is a view of static text, so nothing is constructed and nothing
/// copied - the refusal this used to be is gone.
#[test]
fn a_list_with_a_view_in_it_is_a_list_of_views() {
    let source = "fn names(n: ref String) -> Vec[String] {\n\
                      let all: Vec[String] = [\"a\", n]\n\
                      return all\n\
                  }\n\
                  fn main() { println(names(\"b\").len()) }\n";
    assert!(findings(source).is_empty(), "{:?}", findings(source));
    let rust = lowered(source);
    assert!(!rust.contains("String::from"), "{rust}");
}

/// **A literal takes its neighbours' text**: in a list that already holds text
/// of its own, and in an `if` or a `match` whose other arms are `String`. A list
/// holds one type and a choice hands back one, so the literal has no other
/// answer - and nothing here is copied, because a literal is not text the
/// program had.
#[test]
fn a_literal_beside_text_of_its_own_becomes_it() {
    let source = "fn name_or(c: bool, name: String) -> String {\n\
                      if c { name } else { \"anonymous\" }\n\
                  }\n\
                  fn pick(n: i64, name: String) -> String {\n\
                      let s = match n {\n\
                          0 => \"zero\"\n\
                          1 => name\n\
                          else => f\"many {n}\"\n\
                      }\n\
                      return s\n\
                  }\n\
                  fn main() {\n\
                      let n = 3\n\
                      let names = [\"a\", f\"c{n}\"]\n\
                      println(f\"{names.len()} {name_or(false, f\"ada\")} {pick(0, f\"ada\")} {pick(1, f\"ada\")} {pick(5, f\"ada\")}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("vec![String::from(\"a\")"), "{rust}");
}

/// **A view handed to a `String` the callee only reads is lent as it is**
/// ([ADR-282](../../../docs/specification/adr/adr-282.md) D11). The parameter is
/// a `&str` below, so `.clone()` there asked for a copy nothing would keep.
#[test]
fn a_view_handed_to_a_reader_needs_no_copy() {
    let source = "fn show(s: String) { println(s) }\n\
                  fn relay(city: ref String) { show(city) }\n\
                  fn main() { relay(\"Hamburg\") }\n";
    assert!(findings(source).is_empty(), "{:?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("show(city)"), "{rust}");
    assert_eq!(ran("relay", source).trim(), "Hamburg");
}

/// The refusal that stays says **why**, for the case it is (ADR-282 D25):
/// whose text it is, what keeps it, and what a copy the compiler made on its
/// own would cost.
#[test]
fn a_kept_view_is_explained_for_the_case_it_is() {
    let notes = |source: &str| {
        let found = findings(source);
        assert_eq!(found.len(), 1, "{found:?}");
        (
            found[0].notes.join("\n"),
            found[0].help.clone().unwrap_or_default(),
        )
    };

    // A parameter: the caller's text, and the answer that copies nothing.
    // Published, with text of its own flowing in too, so the field stays text
    // of its own (ADR-282 D22).
    let (why, help) = notes(
        "pub struct Person { pub name: String }\n\
         pub fn make(n: ref String) -> Person { return Person { name: n } }\n\
         pub fn built() -> Person { return Person { name: f\"x\" } }\n",
    );
    assert!(why.contains("the text belongs to the caller"), "{why}");
    assert!(why.contains("`Person` keeps its `name`"), "{why}");
    // And what a copy made on the compiler's own would cost.
    assert!(
        why.contains("a copy costs as much as the text is long"),
        "{why}"
    );
    assert!(help.contains("Declare `n: String`"), "{help}");
    assert!(help.contains("n.clone()"), "{help}");

    // (A name bound to a literal is no longer a case of its own: wherever it
    // is kept as text of its own, the binding is declared `String` for it -
    // ADR-282 D17, ADR-282.)

    // Any other view: it points into something that stays.
    let (why, help) = notes(
        "pub struct Person { pub name: String }\n\
         pub fn make(n: ref String) -> Person { return Person { name: n.trim() } }\n\
         pub fn built() -> Person { return Person { name: f\"x\" } }\n",
    );
    assert!(
        why.contains("it points into text that something else owns"),
        "{why}"
    );
    assert!(help.contains(".clone()"), "{help}");
}

/// **An `if` or a `match` of literals is static text too** (#441, ADR-282
/// D17), and the rule holds in a program that writes `String` nowhere but in a
/// list's type: kept, each is declared `String`; only looked at, it stays a
/// view.
#[test]
fn a_branch_of_literals_kept_is_declared_string_and_one_looked_at_is_not() {
    let source = "fn look(s: ref String) -> i64 {\n\
         \x20   return s.len()\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let k = 3\n\
         \x20   let mut xs: Vec[String] = Vec()\n\
         \x20   let n = \"ada\"\n\
         \x20   xs.push(n)\n\
         \x20   let q = if k > 1 { \"x\" } else { \"y\" }\n\
         \x20   xs.push(q)\n\
         \x20   let r = match k {\n\
         \x20       1 => \"one\",\n\
         \x20       else => \"many\",\n\
         \x20   }\n\
         \x20   xs.push(r)\n\
         \x20   let v = if k > 1 { \"only\" } else { \"viewed\" }\n\
         \x20   println(f\"{xs.len()} {xs[1]} {xs[2]} {look(v)}\")\n\
         }\n";
    let found = findings(source);
    assert!(found.is_empty(), "{found:#?}");
    let rust = lowered(source);
    assert!(rust.contains("let q: String = if"), "{rust}");
    assert!(rust.contains("let r: String = match"), "{rust}");
    assert!(rust.contains("let v = if"), "{rust}");
}
