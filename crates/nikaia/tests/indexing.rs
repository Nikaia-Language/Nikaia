//! A write through the brackets, and why it is not an index
//! ([ADR-080](../../../docs/specification/adr/adr-080.md) D2).
//!
//! **Part I 4.5's own three-line map example did not compile.** `scores[k] = v`
//! lowered to an indexed assignment, and Rust's `Index` for a map is over
//! whatever the key *borrows* as — so indexing a `HashMap<K, V>` with a `&str`
//! leaves `K` unpinned:
//!
//! ```text
//! error[E0282]: type annotations needed for
//!               `HashMap<_, i32, BuildHasherDefault<FxHasher>>`
//! help: consider giving `scores` an explicit type, where the type for type
//!       parameter `K` is specified
//! ```
//!
//! `TrustedMap`, `BuildHasherDefault<FxHasher>` and a type parameter `K`: three
//! spellings the program never wrote, in a message about a file nobody wrote.
//! [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s class at
//! its worst, because it names this compiler's own internal word for a map.
//!
//! These are run rather than read: whether the key type is pinned is a question
//! only the language below can answer, and comparing the emitted string would
//! say no more than that this compiler agrees with itself.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(purpose: &str, source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let found =
        check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings;
    assert!(
        found.is_empty(),
        "{purpose} is a correct program and the checker says otherwise: {found:#?}"
    );
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Compile and run, hand back what it printed.
fn ran(purpose: &str, source: &str) -> String {
    let rust = lowered(purpose, source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering of {purpose} does not compile:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr),
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("the program runs");
    let printed = String::from_utf8_lossy(&ran.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    printed
}

/// Part I 4.5's own example, compiled and run.
///
/// **With the `??` the read now needs** ([ADR-114](../../../docs/specification/adr/adr-114.md)
/// D1, built by [ADR-161](../../../docs/specification/adr/adr-161.md)): a map
/// has a value only where the key is, so the bracket answers a `T?` and a
/// program that knows better says so.
#[test]
fn a_map_written_through_the_brackets_compiles_and_runs() {
    let printed = ran(
        "a map written through the brackets",
        r#"
use std::collections


fn main() {
    let mut scores = collections::HashMap()
    scores["Player1"] = 100
    scores["Player2"] = 7
    let one = scores["Player1"] ?? 0
    let two = scores["Player2"] ?? 0
    println(f"{one} {two}")
}
"#,
    );
    assert_eq!(printed.trim(), "100 7");
}

/// D2: the write is an `insert`, which takes the key **by value** and is what
/// pins it. The **read** is a call of its own now
/// ([ADR-114](../../../docs/specification/adr/adr-114.md) D4): it answers what
/// the container can promise, and the `*` around it is what lets the same three
/// tokens serve a map and a sequence
/// ([ADR-161](../../../docs/specification/adr/adr-161.md) D6).
#[test]
fn the_write_is_a_set_and_the_read_is_a_get() {
    let rust = lowered(
        "a map's two directions",
        r#"
use std::collections


fn main() {
    let mut scores = collections::HashMap()
    scores["Player1"] = 100
    let one = scores["Player1"] ?? 0
    println(f"{one}")
}
"#,
    );
    // **And the key is not a position** (0.0.235): the map's key type is
    // pinned by nothing here, and `index::at` - which turns a number into a
    // `usize` - made `m[1] = 2` a map keyed by `usize`. It was the identity on
    // this text key, so what changed is the spelling and not the program.
    assert!(
        rust.contains("nikaia_std::index::set(&mut scores, \"Player1\", __nikaia_stored)"),
        "the write goes through `set`:\n{rust}"
    );
    assert!(
        rust.contains("*nikaia_std::index::get(&scores, \"Player1\")"),
        "the read goes through `get`:\n{rust}"
    );
    assert!(
        !rust.contains("index::at("),
        "a key is not a position:\n{rust}"
    );
    // **And it is no longer an index.** Rust's `Index` for a map panics on an
    // absent key, which is the abort
    // [ADR-114](../../../docs/specification/adr/adr-114.md) took away.
    assert!(
        !rust.contains("scores[nikaia_std::index::at(\"Player1\")]"),
        "the read is not an index any more:\n{rust}"
    );
}

/// **A sequence is written the same way and means the same thing**, which is
/// what makes this one rule rather than two: `Set` for a `Vec` *is* an indexed
/// assignment, chosen by the language below on the container's type because this
/// emitter does not know it (ADR-011 D2).
#[test]
fn a_sequence_written_through_the_brackets_still_works() {
    let printed = ran(
        "a sequence written through the brackets",
        r#"
fn main() {
    let mut xs = Vec()
    xs.push(10)
    xs.push(20)
    xs[0] = 99
    println(f"{xs[0]} {xs[1]}")
}
"#,
    );
    assert_eq!(printed.trim(), "99 20");
}

/// **A compound write is left alone**, and that is the decision rather than an
/// omission: `xs[0] += 1` reads the slot as well as writing it, so it is an
/// `Index` either way — and saying what reading an absent key means is a
/// question of its own (ADR-080 §4).
#[test]
fn a_compound_write_is_still_an_indexed_assignment() {
    let rust = lowered(
        "a compound write",
        r#"
fn main() {
    let mut xs = Vec()
    xs.push(10)
    xs[0] += 1
    println(f"{xs[0]}")
}
"#,
    );
    assert!(
        !rust.contains("index::set"),
        "a compound write is not a `set`:\n{rust}"
    );
    // The `at` is absent here and that is a different rule: a **constant**
    // index needs no conversion, because the fold already knows it fits
    // ([ADR-048](../../../docs/specification/adr/adr-048.md) D1). What this
    // asserts is the form, not the spelling of the subscript.
    assert!(
        rust.contains("xs[0] += 1"),
        "it keeps the indexed form:\n{rust}"
    );
}

/// And both together, run: the sequence, its compound write, and the map.
#[test]
fn a_sequence_and_a_map_in_one_program() {
    let printed = ran(
        "both containers",
        r#"
use std::collections


fn main() {
    let mut xs = Vec()
    xs.push(10)
    xs.push(20)
    xs[0] = 99
    xs[1] += 1

    let mut scores = collections::HashMap()
    scores["a"] = 1

    let n = scores["a"] ?? 0
    println(f"{xs[0]} {xs[1]} {n}")
}
"#,
    );
    assert_eq!(printed.trim(), "99 21 1");
}

/// **A field read on an element of a sequence**, which did not compile at all
/// (0.0.125).
///
/// `rows[1].a` — for a `Vec` and for an `Array[T, N]` alike — lowered to
/// `(*index::get(&rows, index::at(1))).a` and `rustc` answered *type
/// annotations needed*: `at`'s `I` has nothing to infer itself from, and where
/// the element is only printed an integer literal's late defaulting settles it,
/// while a **field read on the element** needs the type before the defaulting
/// happens.
///
/// The emitter already knew this — the paragraph beside the write branch says
/// `at(0)` has nothing to infer `I` from and takes the literal out of the
/// conversion — and the **read** branch had never had the same exception. So a
/// `let` with an index in it was fine and the field after it was
/// [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s class:
/// `rustc` speaking about the generated file, for an ordinary line.
///
/// **A nested index is the same absence one level out**, and it is here for
/// that reason: `grid[1][0]` has an index where a base stands.
#[test]
fn a_field_of_an_indexed_element_compiles_and_runs() {
    let printed = ran(
        "a field through an index",
        "struct Row { a: i64, b: i64 }\n\
         \n\
         fn main() {\n\
         \x20   let rows: Array[Row, 2] = [Row { a: 1, b: 2 }, Row { a: 3, b: 4 }]\n\
         \x20   println(f\"{rows[1].a}\")\n\
         \x20   let mut held: Vec[Row] = []\n\
         \x20   held.push(Row { a: 5, b: 6 })\n\
         \x20   println(f\"{held[0].b}\")\n\
         \x20   let grid: Array[Array[i64, 2], 2] = [[1, 2], [3, 4]]\n\
         \x20   println(f\"{grid[1][0]}\")\n\
         }\n",
    );
    assert_eq!(printed, "3\n6\n3\n");
}

/// …and **a range the program computes stays in the conversion**, which is
/// where `index::at` earns its place over a slice.
///
/// A range written in **literals** settles itself, because
/// `RangeInclusive<usize>` is the only one of `At`'s candidates that is a
/// `SliceIndex<str>` — and handing a bare `1..=3` to `at` settles *nothing*,
/// since `At` is implemented for a range of every signed type and all of them
/// answer the same `usize`. A range built out of **names** is an `i64` one and
/// has to be converted, which is [ADR-048](../../../docs/specification/adr/adr-048.md)
/// D1's whole trade. `examples/k-nucleotide/src/main.nika` writes the second shape.
#[test]
fn a_slice_of_text_is_converted_where_the_range_is_computed() {
    let printed = ran(
        "a slice of text",
        "fn main() {\n\
         \x20   let text = \"hello\"\n\
         \x20   let part = ref text[1..3]\n\
         \x20   println(f\"{part}\")\n\
         \x20   let at: i64 = 1\n\
         \x20   println(f\"{text[at..<at + 3]}\")\n\
         }\n",
    );
    assert_eq!(printed, "ell\nell\n");

    let rust = lowered(
        "a computed slice of text",
        "fn main() {\n\
         \x20   let text = \"hello\"\n\
         \x20   let at: i64 = 1\n\
         \x20   println(f\"{text[at..<at + 3]}\")\n\
         \x20   println(f\"{text[1..3]}\")\n\
         }\n",
    );
    assert!(
        rust.contains("nikaia_std::index::at(at..at + 3)")
            && rust.contains("nikaia_std::index::get(&text, 1..=3)"),
        "the computed range converts and the written one does not: {rust}"
    );
}

/// **A slice of text read as a value did not lower**
/// ([ADR-182](../../../docs/specification/adr/adr-182.md) D2, which closed
/// `open-work.md`'s entry for it at 0.0.131).
///
/// The read wrapper writes a `*` around every bracket
/// ([ADR-161](../../../docs/specification/adr/adr-161.md) D6), which is what
/// makes `xs[0]` the element rather than a view of it. Over a **range** the
/// read answers a `&str` already, and `*` over one is a `str`: *the size for
/// values of type `str` cannot be known at compilation time*, about a noun
/// nobody wrote ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
///
/// **Off the shape of what is in the brackets** and not off a type: a range is
/// a run and a key is not, in this language and in the one below alike.
#[test]
fn a_slice_of_text_read_as_a_value_runs() {
    let printed = ran(
        "a bare slice of text",
        "fn main() {\n\
         \x20   let text = \"hello world\"\n\
         \x20   println(f\"{text[1..3]}\")\n\
         \x20   let part = ref text[1..3]\n\
         \x20   println(f\"{part}\")\n\
         \x20   println(f\"{text[0..<5]}\")\n\
         }\n",
    );
    assert_eq!(printed, "ell\nell\nhello\n");
}

/// **And a run of a sequence is the same shape**, which is the half that says
/// this is about the brackets rather than about text: `*&[i64]` is unsized for
/// the reason `*&str` is.
#[test]
fn a_slice_of_a_sequence_read_as_a_value_runs() {
    let printed = ran(
        "a bare slice of a sequence",
        "fn main() {\n\
         \x20   let xs: Vec[i64] = [1, 2, 3, 4]\n\
         \x20   println(f\"{xs[1..2].len()}\")\n\
         \x20   let run = ref xs[1..2]\n\
         \x20   println(f\"{run.len()}\")\n\
         }\n",
    );
    assert_eq!(printed, "2\n2\n");
}

/// **A read at a number keeps its `*`**, which is the line the rule is drawn
/// on: `xs[0]` is the element the program asked for and not a view of it.
#[test]
fn a_read_at_a_number_is_still_the_element() {
    let rust = lowered(
        "a read at a number",
        "fn main() {\n\
         \x20   let xs: Vec[i64] = [1, 2, 3]\n\
         \x20   let first: i64 = xs[0]\n\
         \x20   println(f\"{first}\")\n\
         }\n",
    );
    assert!(
        rust.contains("*nikaia_std::index::get(&xs"),
        "a number keeps the `*`: {rust}"
    );
}

/// **A range that counts from the end reaches run time**
/// ([ADR-048](../../../docs/specification/adr/adr-048.md) D1,
/// [ADR-182](../../../docs/specification/adr/adr-182.md) D3).
///
/// `xs[-2..-1]` is an access out of bounds and says so — but only if it gets
/// there. Handed over as written it does not: `-2` against the `usize` a slice
/// wants is *the trait `Neg` is not implemented for `usize`*, about a type the
/// program never named. So a negation goes back through the conversion, widened
/// to the one width this language indexes with.
#[test]
fn a_range_that_counts_from_the_end_is_an_access_out_of_bounds() {
    let rust = lowered(
        "a negative range",
        "fn main() {\n\
         \x20   let xs: Vec[i64] = [1, 2, 3, 4]\n\
         \x20   println(f\"{xs[-2..-1].len()}\")\n\
         }\n",
    );
    assert!(
        rust.contains("nikaia_std::index::at(-2i64..=-1i64)"),
        "widened, or `at` has nothing to read the width off: {rust}"
    );

    let dir = common::scratch_dir("a negative range");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "it has to compile before it can abort:\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("the program runs");
    assert!(!ran.status.success(), "an index out of bounds aborts");
    assert!(
        String::from_utf8_lossy(&ran.stderr).contains("index out of bounds: the index is -2"),
        "D1's own words: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A field of an element is read through the reference the read answers**:
/// `rows[1].a` is `get(&rows, 1).a`, with no `*` the language below does on
/// its own (found moving the ledger's records into Nikaia, ADR-257, where
/// `clippy` refused the deref in `std`).
#[test]
fn a_field_of_an_element_is_read_without_a_deref() {
    let source = r#"struct Row {
    a: i64,
    name: String,
}

fn main() {
    let rows = [Row { a: 1, name: "x" }, Row { a: 2, name: "y" }]
    println(f"{rows[1].a} {rows[0].name}")
}
"#;
    let rust = lowered("element-field", source);
    assert!(!rust.contains("(*nikaia_std::index::get("), "{rust}");
    assert_eq!(ran("element-field", source), "2 x\n");
}
