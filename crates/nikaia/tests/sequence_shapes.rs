//! What a sequence is **as a whole**
//! ([ADR-293](../../../docs/specification/adr/adr-293.md)).
//!
//! Three walls a program met in `rustc` rather than here, each a Part III C.1
//! defect, and the pipeline that was not there:
//!
//! * `let r = 0..<3` and a `for` over `r` - *no method named `iter`* (D3);
//! * a sequence taken by `let t = s`, inside a loop or inside a lambda and then
//!   walked again - *use of moved value* (D4);
//! * a named `keys()` or `drain()` in a `for` - *no method named `iter`* (D4);
//! * `map` whose elements were `?`, and no `rev`, `zip`, `take`, `skip`,
//!   `step_by`, `chunks` or `windows` at all (D5).
//!
//! What the programs compute is in `tests/language/src/sequence_shapes.nika`,
//! run at both settings of `user_parallelism`; every refusal is asked of the
//! checker here.
use nikaia::check::CodeOps;

use nikaia::contracts::LedgerOps;

use nikaia::contracts::ty::Ty;
use nikaia::contracts::ty::TyOps;
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

fn codes(source: &str) -> Vec<&'static str> {
    findings(source).iter().map(|f| f.code_str()).collect()
}

// --- D1: the words ------------------------------------------------------------

/// **The three words parse, render and round-trip**, among the step's.
#[test]
fn the_shape_words_round_trip() {
    for written in [
        "Seq[$T] ends",
        "Seq[$T] sync ends sized",
        "Seq[i64] sync ends sized replays",
        "Seq[String] pauses throws",
    ] {
        let parsed = Ty::parse(written);
        assert!(matches!(parsed, Ty::Seq { .. }), "`{written}`: {parsed:?}");
        assert_eq!(parsed.text(), written, "it writes itself back");
    }
    // In any order among the step's words, and written back in one.
    assert_eq!(
        Ty::parse("Seq[$T] sized sync ends").text(),
        "Seq[$T] sync ends sized"
    );
}

/// **In a position that asks for a word, a sequence without it does not fit.**
#[test]
fn a_demand_is_met_only_by_a_sequence_that_has_the_word() {
    let wants = Ty::parse("Seq[$T] ends");
    assert!(Ty::parse("Seq[$T] sync ends sized").fits(&wants));
    assert!(!Ty::parse("Seq[$T] sync sized").fits(&wants));
    assert!(Ty::parse("Seq[$T] sync").fits(&Ty::parse("Seq[$T]")));
}

// --- D3: a range is a value ------------------------------------------------

/// **A range written into a `for` or into brackets stays Rust's own**: it is
/// walked once where it stands, or it is a slice.
#[test]
fn a_range_written_in_place_stays_the_language_belows() {
    let rust = lowered(
        "fn main() {\n\
         \x20   let t = \"abcdef\"\n\
         \x20   for i in 0..<2 { println(ref t[i..<i + 2]) }\n\
         \x20   let kept = 0..<2\n\
         }\n",
        Build::default(),
    );
    assert!(rust.contains("for i in 0..2 "), "{rust}");
    assert!(rust.contains("nikaia_std::range::span(0, 2)"), "{rust}");
    assert!(
        !rust.contains("span(i"),
        "a slice's range is not a value: {rust}"
    );
}

// --- D4: every place a sequence is taken ------------------------------------

/// **A `let` that names a sequence again takes it.**
#[test]
fn a_let_takes_a_sequence() {
    assert_eq!(
        codes(
            "fn main() {\n\
             \x20   let xs = [1, 2, 3]\n\
             \x20   let s = xs.iter()\n\
             \x20   let t = s\n\
             \x20   println(f\"{s.count()} {t.count()}\")\n\
             }\n"
        ),
        ["NK2702"]
    );
}

/// **Taken inside a loop it was declared outside of**: the next turn finds it
/// gone. Refused where it is taken.
#[test]
fn a_loop_takes_it_again_on_the_next_turn() {
    let found = findings(
        "fn main() {\n\
         \x20   let xs = [1, 2, 3]\n\
         \x20   let s = xs.iter()\n\
         \x20   for x in xs {\n\
         \x20       println(f\"{s.count()}\")\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK2702");
    assert!(
        found[0].message.contains("inside a loop"),
        "{}",
        found[0].message
    );
}

/// **Unless the loop gives it a new one, or may leave on that turn.**
#[test]
fn a_loop_that_revives_or_leaves_is_a_program() {
    assert_eq!(
        codes(
            "fn main() {\n\
             \x20   let xs = [1, 2, 3]\n\
             \x20   let mut s = xs.iter()\n\
             \x20   for x in xs {\n\
             \x20       println(f\"{s.count()}\")\n\
             \x20       s = xs.iter()\n\
             \x20   }\n\
             \x20   let q = xs.iter()\n\
             \x20   for x in xs {\n\
             \x20       println(f\"{q.count()}\")\n\
             \x20       break\n\
             \x20   }\n\
             }\n"
        ),
        Vec::<&str>::new()
    );
}

/// **A lambda may run more than once**, so one that takes a sequence from
/// outside is refused.
#[test]
fn a_lambda_takes_it_again_on_the_next_call() {
    let found = findings(
        "fn main() {\n\
         \x20   let xs = [1, 2, 3]\n\
         \x20   let s = xs.iter()\n\
         \x20   let n = xs.iter().map(fn(x) { s.count() })\n\
         \x20   println(f\"{n.count()}\")\n\
         }\n",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].message.contains("inside a lambda"),
        "{}",
        found[0].message
    );
}

/// **A range is never taken** (D3): it replays.
#[test]
fn a_range_is_never_taken() {
    assert_eq!(
        codes(
            "fn main() {\n\
             \x20   let r = 0..<3\n\
             \x20   let t = r\n\
             \x20   for i in 0..<2 { println(f\"{r.count()} {t.count()}\") }\n\
             }\n"
        ),
        Vec::<&str>::new()
    );
}

// --- D2: a demand, and words that pass through ------------------------------

/// **`NK2703`**: `io::lines()` has no back end, and saying so is this
/// compiler's job and not a trait bound in the generated Rust.
#[test]
fn a_sequence_with_no_back_end_is_not_walked_backwards() {
    let found =
        findings("use std::io\n\nfn main() throws {\n    let back = io::lines().rev()\n}\n");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK2703");
    assert!(
        found[0]
            .message
            .contains("can only be walked from the front"),
        "{}",
        found[0].message
    );
    // The ledger's words are not in it: a program cannot write them.
    assert!(!found[0].message.contains("`ends`"), "{}", found[0].message);
}

/// **A `filter` keeps the back end and loses the length**, so `rev` after it is
/// a program and `zip(…).rev()` after it is not.
#[test]
fn a_filter_keeps_the_back_end_and_loses_the_length() {
    assert_eq!(
        codes(
            "fn main() {\n\
             \x20   let xs = [1, 2, 3]\n\
             \x20   let back: Vec[ref i64] = xs.iter().filter(fn(x) { x > 1 }).rev().collect()\n\
             }\n"
        ),
        Vec::<&str>::new()
    );
    assert_eq!(
        codes(
            "fn main() {\n\
             \x20   let xs = [1, 2, 3]\n\
             \x20   let z = xs.iter().filter(fn(x) { x > 1 }).zip(xs.iter()).rev()\n\
             }\n"
        ),
        ["NK2703"]
    );
}

// --- D5: the pipeline ---------------------------------------------------------

/// **`map` knows its elements**: `$U` is what the lambda comes to, so the
/// collected list is a `Vec[i64]` and an annotation that says otherwise is
/// refused here rather than in `rustc`.
#[test]
fn a_map_knows_what_its_elements_are() {
    assert_eq!(
        codes(
            "fn main() {\n\
             \x20   let xs = [1, 2, 3]\n\
             \x20   let names: Vec[String] = xs.iter().map(fn(x) { x > 1 }).collect()\n\
             }\n"
        ),
        ["NK1103"]
    );
}

/// **Two arms of one choice are not one after the other**: a sequence taken in
/// the `then` and read in the `else` was taken on neither's way to the other.
/// Ordered by statement alone, `NK2702` refused this correct program - and
/// with `iter()` a sequence, that would have been every such program.
#[test]
fn the_arms_of_one_choice_do_not_take_from_each_other() {
    assert_eq!(
        codes(
            "fn main() {\n\
             \x20   let xs = [1, 2, 3]\n\
             \x20   let s = xs.iter()\n\
             \x20   if xs.len() > 2 {\n\
             \x20       println(f\"{s.count()}\")\n\
             \x20   } else {\n\
             \x20       println(f\"{s.count()}\")\n\
             \x20   }\n\
             \x20   let m = xs.iter()\n\
             \x20   match xs.len() {\n\
             \x20       1 => println(f\"{m.count()}\"),\n\
             \x20       else => println(f\"{m.count()}\"),\n\
             \x20   }\n\
             }\n"
        ),
        Vec::<&str>::new()
    );
    // …and a read **after** the choice is after whichever arm took it.
    assert_eq!(
        codes(
            "fn main() {\n\
             \x20   let xs = [1, 2, 3]\n\
             \x20   let s = xs.iter()\n\
             \x20   if xs.len() > 2 {\n\
             \x20       println(f\"{s.count()}\")\n\
             \x20   }\n\
             \x20   println(f\"{s.count()}\")\n\
             }\n"
        ),
        ["NK2702"]
    );
}

/// **Two walks in one statement** are one after the other (ADR-293 D31).
#[test]
fn two_walks_in_one_statement_are_refused() {
    assert_eq!(
        codes(
            "fn main() {\n\
             \x20   let xs = [1, 2]\n\
             \x20   let s = xs.iter()\n\
             \x20   println(f\"{s.count() + s.count()}\")\n\
             }\n"
        ),
        ["NK2702"]
    );
}

/// **A slice handed to a list is refused here, naming the type that takes
/// both** (ADR-293 D23): before, it was `rustc`'s *expected `&Vec<i64>`, found
/// `&&[_]`*.
#[test]
fn a_slice_handed_to_a_list_says_what_to_declare() {
    let found = findings(
        "fn total(xs: ref Vec[i64]) -> i64 {\n\
         \x20   return xs.len()\n\
         }\n\n\
         fn main() {\n\
         \x20   let xs = [1, 2, 3]\n\
         \x20   let part = ref xs[0..<2]\n\
         \x20   println(f\"{total(part)}\")\n\
         }\n",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1102");
    assert!(
        found[0]
            .help
            .as_deref()
            .is_some_and(|h| h.contains("ref Array[i64]")),
        "{:?}",
        found[0].help
    );
}
