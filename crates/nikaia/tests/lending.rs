//! A `for` lends, and a `let` over a place is a view of it
//! ([ADR-094](../../../docs/specification/adr/adr-094.md) D4) — the second of
//! that record's five steps.
//!
//! **This one changes what existing programs mean**, which the first step
//! deliberately did not. `for e in entries { … }` used to take `entries` away;
//! it leaves it where it was now, so `entries.len()` on the next line is a
//! program rather than `rustc`'s *use of moved value* about a file nobody
//! wrote. Iteration that takes the elements away is **written** —
//! `for x in xs.drain()` — because removing a name from scope is the rare case
//! and the one worth a word.
//!
//! **Off the shape of the expression and not off a column**, which is why this
//! step needs no ledger: a *place* is lent, and a call, a range or a literal
//! owns what it made. The one question the shape cannot answer is whether a
//! place's value would **move**, and that one the checker answers — `let mi =
//! self.bodies[i].mass` over an `f64` is a copy, and a `&` there is a borrow
//! held across the loop that writes the same field.

mod common;

use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

/// The Rust this source lowers to.
fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Compile the lowering and run it, which is the only proof that the borrow it
/// now writes is one the language below accepts.
fn ran(purpose: &str, source: &str) -> String {
    let rust = lowered(source);
    let dir = common::scratch_dir(&format!("lending-{purpose}"));
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

/// Every finding the checker has about a source.
fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

/// **The line the record was written for.** `entries.len()` after the loop used
/// to be `rustc`'s *use of moved value*; it is a program.
#[test]
fn a_for_leaves_the_collection_where_it_was() {
    let printed = ran(
        "for-lends",
        "fn main() {\n\
         \x20   let mut xs = Vec()\n\
         \x20   xs.push(1)\n\
         \x20   xs.push(2)\n\
         \x20   let mut sum = 0\n\
         \x20   for x in xs { sum += x }\n\
         \x20   println(f\"{sum} of {xs.len()}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "3 of 2");
}

/// **And a call, a range or a method call owns what it made.** There is nothing
/// to lend: the value did not exist before the expression that names it.
#[test]
fn a_for_over_something_that_is_not_a_place_owns_it() {
    let rust = lowered(
        "fn main() {\n\
         \x20   let mut sum = 0\n\
         \x20   for i in 0..<3 { sum += i }\n\
         \x20   let text = \"abc\"\n\
         \x20   for c in text.chars() { sum += 1 }\n\
         \x20   println(f\"{sum}\")\n\
         }\n",
    );
    assert!(rust.contains("for i in 0..3"), "{rust}");
    assert!(!rust.contains("0..3.iter()"), "{rust}");
    assert!(rust.contains(".chars()"), "{rust}");
    assert!(!rust.contains(".chars().iter()"), "{rust}");
}

/// **`.iter()` and not `&`**, which is a measurement rather than a preference.
///
/// The iterated name may already *be* a view — a parameter declared
/// `&Vec[Entry]` — and `&entries` is then a `&&Vec<Entry>`, which Rust does not
/// iterate. `.iter()` reads the same through any number of references, and this
/// emitter has no types to tell the two apart with
/// ([ADR-028](../../../docs/specification/adr/adr-028.md)). Found by
/// `examples/inventory/stock.nika`, whose `total` takes exactly that parameter.
#[test]
fn a_for_over_a_parameter_that_is_already_a_view_still_iterates() {
    let printed = ran(
        "for-over-a-view",
        "fn total(xs: ref Vec[i64]) -> i64 {\n\
         \x20   let mut sum = 0\n\
         \x20   for x in xs { sum += x }\n\
         \x20   return sum\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let mut xs = Vec()\n\
         \x20   xs.push(4)\n\
         \x20   xs.push(5)\n\
         \x20   println(f\"{total(ref xs)}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "9");
}

/// **Taking the elements away is written**, and below it is `into_iter`: D4's
/// words are *"removing a name from scope"*, which is consuming the container
/// rather than emptying one somebody still holds.
///
/// It is what a body that hands an element to a callee which **keeps** it has
/// to say — `all.push(v)` in `examples/json/src/main.nika`, and `or_insert(counts)` in
/// `access-log.nika`, both of which this step rewrote.
#[test]
fn a_drain_takes_the_elements_away() {
    let printed = ran(
        "for-drains",
        "fn main() {\n\
         \x20   let mut xs = Vec()\n\
         \x20   xs.push(\"a\")\n\
         \x20   xs.push(\"b\")\n\
         \x20   let mut all = Vec()\n\
         \x20   for x in xs.drain() { all.push(x) }\n\
         \x20   println(f\"{all.len()}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "2");

    let rust = lowered(
        "fn main() {\n\
         \x20   let mut xs = Vec()\n\
         \x20   xs.push(1)\n\
         \x20   for x in xs.drain() { println(f\"{x}\") }\n\
         }\n",
    );
    assert!(rust.contains(".into_iter()"), "{rust}");
    assert!(!rust.contains(".drain()"), "{rust}");
}

/// **`NK1137`: the `&` is the compiler's to write.** A `for` lends whatever
/// place it is given, so one written in front says the line twice — and left
/// alone it is a reference to a reference, which Rust does not iterate.
#[test]
fn a_written_ampersand_in_a_for_head_is_refused() {
    let found = findings(
        "fn main() {\n\
         \x20   let mut xs = Vec()\n\
         \x20   xs.push(1)\n\
         \x20   for x in ref xs { println(f\"{x}\") }\n\
         }\n",
    );
    assert!(found.iter().any(|f| f.code == "NK1137"), "{found:#?}");

    // And the same loop without it is not refused, which is the half that says
    // the rule is about the `&` and not about the loop.
    let clean = findings(
        "fn main() {\n\
         \x20   let mut xs = Vec()\n\
         \x20   xs.push(1)\n\
         \x20   for x in xs { println(f\"{x}\") }\n\
         }\n",
    );
    assert!(!clean.iter().any(|f| f.code == "NK1137"), "{clean:#?}");
}

/// **A `let` over a place is a view of it**: the language below refuses to move
/// a value out of a container, so a move there was never what the line meant.
#[test]
fn a_let_over_a_place_is_a_view() {
    let rust = lowered(
        "struct Row { name: String }\n\
         struct Store { rows: Vec[Row] }\n\
         fn first(store: ref Store) -> i64 {\n\
         \x20   let row = store.rows[0]\n\
         \x20   return row.name.len() as i64\n\
         }\n\
         fn main() { }\n",
    );
    // The read is `index::get`, which hands back a **view** of the element —
    // which is what this test is about, one spelling on
    // ([ADR-161](../../../docs/specification/adr/adr-161.md) D6).
    assert!(
        rust.contains("let row = &*nikaia_std::index::get(&store.rows,"),
        "{rust}"
    );
}

/// **And over a value that copies, it is not.**
///
/// `let mi = self.bodies[i].mass` over an `f64` is a copy, and a `&` there is a
/// borrow held across the loop that writes the same field — `E0502` about a
/// file nobody wrote. `examples/n-body/src/main.nika` is where that was met, and
/// [`moves_away`] is the same answer `NK2101` reads.
#[test]
fn a_let_over_a_place_that_copies_is_not_a_view() {
    let rust = lowered(
        "struct Body { mass: f64 }\n\
         struct World { bodies: Vec[Body] }\n\
         fn heavy(world: ref World) -> f64 {\n\
         \x20   let m = world.bodies[0].mass\n\
         \x20   return m\n\
         }\n\
         fn main() { }\n",
    );
    assert!(!rust.contains("let m = &"), "{rust}");
}

/// **`let y = x` over a whole variable stays a move** — it is a rename, and the
/// one shape that separates this rule from `for`'s.
#[test]
fn a_let_over_a_whole_name_stays_a_move() {
    let rust = lowered(
        "fn main() {\n\
         \x20   let a = \"one\"\n\
         \x20   let b = a\n\
         \x20   println(f\"{b}\")\n\
         }\n",
    );
    assert!(rust.contains("let b = a;"), "{rust}");
}

/// **A cast over a `for` binding is a cast over a view**
/// ([ADR-182](../../../docs/specification/adr/adr-182.md) D1, which closed
/// `open-work.md`'s entry for it at 0.0.131).
///
/// The loop binds a view of each element, which is what D4 is for and is what
/// lets the loop read without copying — and Rust's `as` does not see through
/// one. What came back was *casting `&i32` as `i64` is invalid*, relayed onto
/// the `.nika` line, with a way out that reads *dereference the expression*: a
/// noun and an instruction about a file nobody wrote
/// ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
///
/// **This runs**, because what a cast comes to is a question only the language
/// below answers, and the arithmetic has to be right as well as compile.
#[test]
fn a_cast_over_a_for_binding_reaches_the_number() {
    let printed = ran(
        "a cast over a for binding",
        "comptime NS: Array[i32, 3] = [1, 2, 3]\n\
         \n\
         fn main() {\n\
         \x20   let mut sum: i64 = 0\n\
         \x20   for n in NS { sum = sum + (n as i64) }\n\
         \x20   println(f\"{sum}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "6");
}

/// **A name that shadows the binding is not a view**, and the same statement
/// may cast over both.
///
/// This is why the answer is the checker's and keyed by the name: a `*` would
/// be right for one of these two and wrong for the other, on one line.
#[test]
fn a_name_that_shadows_a_for_binding_is_cast_as_itself() {
    let printed = ran(
        "a shadowed for binding",
        "comptime NS: Array[i32, 3] = [1, 2, 3]\n\
         \n\
         fn main() {\n\
         \x20   let mut sum: i64 = 0\n\
         \x20   for n in NS {\n\
         \x20       let m: i32 = 10\n\
         \x20       sum = sum + (n as i64) + (m as i64)\n\
         \x20   }\n\
         \x20   for n in NS {\n\
         \x20       let n: i32 = 100\n\
         \x20       sum = sum + (n as i64)\n\
         \x20   }\n\
         \x20   println(f\"{sum}\")\n\
         }\n",
    );
    // 6 + 30 from the first loop, 300 from the second.
    assert_eq!(printed.trim(), "336");
}

/// **A narrowing cast over a binding is still narrowing**, which is the half a
/// fix written around the conversion rather than inside it would have lost:
/// `as` truncates by definition, and [ADR-043](../../../docs/specification/adr/adr-043.md)
/// D4 puts an abort there rather than a silent wrong number.
#[test]
fn a_narrowing_cast_over_a_for_binding_still_aborts() {
    let rust = lowered(
        "comptime NS: Array[i64, 2] = [1, 300]\n\
         \n\
         fn main() {\n\
         \x20   for n in NS { println(f\"{n as u8}\") }\n\
         }\n",
    );
    assert!(
        rust.contains("u8::try_from(nikaia_std::num::value("),
        "the check goes around the value and not the view: {rust}"
    );
}

/// **A `let` that declares the element's type over a `for` binding is refused**
/// ([ADR-185](../../../docs/specification/adr/adr-185.md) D1, closing
/// `open-work.md`'s entry for it at 0.0.136).
///
/// A `for` lends (D4), so the binding is a **view** and an annotation naming
/// the element is a type the value does not have. `rustc` said *mismatched
/// types* with *consider using clone here* — an instruction to insert exactly
/// the copy [ADR-008](../../../docs/specification/adr/adr-008.md) D5 says is
/// written and never inserted, about a file nobody wrote.
///
/// **A diagnostic and not a lowering**, which is the whole of why it is this
/// shape: the way out exists and the program can take it, so the compiler knew
/// both that the annotation was wrong and what to write instead, and said
/// neither.
#[test]
fn a_let_that_declares_a_type_over_a_for_binding_is_refused() {
    let found = findings(
        "struct Row { a: i64 }\n\
         \n\
         fn main() {\n\
         \x20   let rows: Vec[Row] = [Row { a: 1 }]\n\
         \x20   for r in rows {\n\
         \x20       let copy: Row = r\n\
         \x20       println(f\"{copy.a}\")\n\
         \x20   }\n\
         }\n",
    );
    let refused = found
        .iter()
        .find(|f| f.code == "NK1183")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert!(
        refused.message.contains("`r` is a view of a `Row`"),
        "{refused:#?}"
    );
    // **A way out that can be taken** (Part III C.2), and the other answer
    // named beside it, because which was meant is not this compiler's to know.
    let help = refused.help.as_deref().unwrap_or("");
    assert!(
        help.contains("take the annotation off") && help.contains(".clone()"),
        "{refused:#?}"
    );
}

/// **And the way out works**, which is what makes the refusal one: a view reads
/// the same, and the numeric half still reads the number through it
/// ([ADR-182](../../../docs/specification/adr/adr-182.md) D5).
#[test]
fn the_way_out_of_that_refusal_runs() {
    let printed = ran(
        "the way out of a declared type over a binding",
        "struct Row { a: i64 }\n\
         \n\
         fn main() {\n\
         \x20   let rows: Vec[Row] = [Row { a: 1 }, Row { a: 2 }]\n\
         \x20   for r in rows {\n\
         \x20       let copy = r\n\
         \x20       println(f\"{copy.a}\")\n\
         \x20   }\n\
         \x20   let ns: Vec[i64] = [7]\n\
         \x20   for n in ns {\n\
         \x20       let q: i64 = n\n\
         \x20       println(f\"{q}\")\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(printed, "1\n2\n7\n");
}
