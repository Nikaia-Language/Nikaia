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
//! The programs are run rather than read: whether the key type is pinned is a
//! question only the language below can answer, and comparing the emitted
//! string would say no more than that this compiler agrees with itself. They
//! are `tests/language/src/indexing.nika`; this file keeps what the brackets
//! lower to and the range that aborts.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
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

/// D2: the write is an `insert`, which takes the key **by value** and is what
/// pins it. The **read** is a call of its own now
/// ([ADR-293](../../../docs/specification/adr/adr-293.md) D4): it answers what
/// the container can promise, and the `*` around it is what lets the same three
/// tokens serve a map and a sequence
/// ([ADR-293](../../../docs/specification/adr/adr-293.md) D9).
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
    // [ADR-293](../../../docs/specification/adr/adr-293.md) took away.
    assert!(
        !rust.contains("scores[nikaia_std::index::at(\"Player1\")]"),
        "the read is not an index any more:\n{rust}"
    );
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
    // ([ADR-285](../../../docs/specification/adr/adr-285.md) D1). What this
    // asserts is the form, not the spelling of the subscript.
    assert!(
        rust.contains("xs[0] += 1"),
        "it keeps the indexed form:\n{rust}"
    );
}

/// …and **a range the program computes stays in the conversion**, which is
/// where `index::at` earns its place over a slice.
///
/// A range written in **literals** settles itself, because
/// `RangeInclusive<usize>` is the only one of `At`'s candidates that is a
/// `SliceIndex<str>` — and handing a bare `1..=3` to `at` settles *nothing*,
/// since `At` is implemented for a range of every signed type and all of them
/// answer the same `usize`. A range built out of **names** is an `i64` one and
/// has to be converted, which is [ADR-285](../../../docs/specification/adr/adr-285.md)
/// D1's whole trade. `examples/k-nucleotide/src/main.nika` writes the second shape.
#[test]
fn a_slice_of_text_is_converted_where_the_range_is_computed() {
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
/// ([ADR-285](../../../docs/specification/adr/adr-285.md) D1,
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
/// its own (found moving the ledger's records into Nikaia, ADR-294, where
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
}
