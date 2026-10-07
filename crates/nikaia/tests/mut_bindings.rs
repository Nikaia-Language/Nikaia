//! **What is changed says `mut`**, in both of the two places a binding is
//! written: a parameter ([ADR-094](../../../docs/specification/adr/adr-094.md)
//! D3, the fourth of that record's five steps) and a `let`
//! ([Part I 2.1](../../../docs/specification/10-nikaia-light.md), which had
//! stated it all along and had `rustc` answering for it).
//!
//! `fn fill(mut out: Vec[i64])` is a parameter the callee changes in place, and
//! the **caller's** value is what changes. It lowers to `&mut T`, and the call
//! — `fill(xs)` — shows nothing, exactly as `xs.push(1)` shows nothing. A
//! language that hides mutation through a receiver and shows it through an
//! argument has two rules for one thing.
//!
//! **The third state, and the only one of the three the author writes.** D1's
//! `&` comes off the inferred `keeps` column and D2's *handed over* is what is
//! left; this one is a word in the source, and `keeps::lends` withholds its own
//! claim on such a position so the two never both write a reference.
//!
//! **And it closes a hole older than the record.** A body that changed an owned
//! parameter lowered to a Rust declaration with no `mut` on it, and `rustc`
//! answered *cannot borrow as mutable* about a file nobody wrote
//! ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)). That is
//! `NK1138`, and half the tests here are about it landing only where the change
//! is **certain** — the other half of C.1 is C.4, and a correct program refused
//! is the worse of the two mistakes.
//!
//! **What the programs compute** - a caller's value changed through a `mut`
//! parameter, a `mut` receiver, a `mut let` - is
//! `tests/language/src/mut_bindings.nika`, run by `nikaia test`.

use nikaia::contracts::LedgerOps;

use nikaia::contracts::ty::TyOps;
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

/// The Rust this source lowers to.
fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
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

/// Whether the source is refused with `NK1138`.
fn refused(source: &str) -> bool {
    findings(source).iter().any(|f| f.code == "NK1138")
}

/// **Both halves off one word**, which is the invariant: the declaration gains
/// `&mut` and so does the argument, and either alone is a type error below.
#[test]
fn the_declaration_and_the_call_gain_the_reference_together() {
    let rust = lowered(
        "fn fill(mut out: Vec[i64]) { out.push(1) }\n\
         fn main() { let mut xs = Vec() fill(xs) }\n",
    );
    assert!(rust.contains("fn fill(out: &mut Vec<i64>)"), "{rust}");
    assert!(rust.contains("fill(&mut xs)"), "{rust}");
}

/// **And `lends` withholds its claim there**, so a `mut` parameter never gets
/// both references. Without this the declaration would read `& &mut`, or the
/// call would write `&&mut`, depending which pass answered last.
#[test]
fn a_mut_parameter_is_not_also_lent() {
    let rust = lowered(
        "fn fill(mut out: Vec[i64]) { out.push(1) }\n\
         fn main() { let mut xs = Vec() fill(xs) }\n",
    );
    assert!(!rust.contains("&&"), "{rust}");
    assert!(!rust.contains("& mut"), "{rust}");
}

/// **It rides in the signature**, which is what a caller across a package
/// boundary reads a parameter's kind off — and it survives the round trip,
/// which `--locked` needs.
#[test]
fn the_word_is_in_the_signature_and_parses_back() {
    let parsed =
        parse_to_ast("pub fn fill(mut out: Vec[i64], n: i64) { out.push(n) }").expect("it parses");
    let ledger = nikaia::contracts::Ledger::infer(&parsed);
    let rendered = ledger.render();
    assert!(
        rendered.contains("(mut out: Vec[i64], n: i64)"),
        "{rendered}"
    );

    let read = nikaia::contracts::Ledger::parse(&rendered).expect("its own output parses");
    let signature = read.functions["fill"]
        .signature
        .as_ref()
        .expect("a signature");
    assert_eq!(signature.mutable, ["out"]);
    assert_eq!(
        signature.params[0],
        (
            "out".to_string(),
            nikaia::contracts::ty::Ty::parse("Vec[i64]")
        ),
        "the word is the parameter's and not part of its type"
    );
    assert_eq!(read.render(), rendered);
}

/// **`NK1138`: a parameter a body changes says `mut`.** The hole this closes is
/// older than the record — without the word the parameter lowered to a Rust one
/// with no `mut` on it, and the answer came from `rustc`.
#[test]
fn a_changed_parameter_without_the_word_is_refused() {
    // Through a method that changes its subject.
    assert!(refused(
        "fn fill(out: Vec[i64]) { out.push(1) }\n\
         fn main() { }\n"
    ));

    // And through an assignment into it, which is the other shape.
    assert!(refused(
        "struct Row { total: i64 }\n\
         fn zero(row: Row) { row.total = 0 }\n\
         fn main() { }\n"
    ));

    // The same two with the word are not refused, which is the half that says
    // the rule is about the declaration and not about the body.
    assert!(!refused(
        "fn fill(mut out: Vec[i64]) { out.push(1) }\n\
         fn main() { }\n"
    ));
    assert!(!refused(
        "struct Row { total: i64 }\n\
         fn zero(mut row: Row) { row.total = 0 }\n\
         fn main() { }\n"
    ));
}

/// **A method that only reads is not a change**, which is the line between this
/// refusal and refusing every method call on a parameter.
#[test]
fn reading_a_parameter_is_not_changing_it() {
    assert!(!refused(
        "fn width(xs: Vec[i64]) -> i64 { return xs.len() as i64 }\n\
         fn main() { }\n"
    ));
}

/// **A name a `let` has bound is the local's**, so what happens to it after
/// that line says nothing about the parameter. D3 names this shape itself: *a
/// callee that wants a mutable copy writes `let mut v = x` inside*.
#[test]
fn a_shadowed_parameter_is_no_longer_the_parameter() {
    assert!(!refused(
        "fn count(xs: Vec[i64]) -> i64 {\n\
         \x20   let mut xs = Vec()\n\
         \x20   xs.push(1)\n\
         \x20   return xs.len() as i64\n\
         }\n\
         fn main() { }\n"
    ));
}

/// **A method no ledger describes is not one to refuse on**, and neither is one
/// whose candidates disagree. Answering *it might change* here would refuse a
/// correct program, which is [Part III
/// C.4](../../../docs/specification/30-nikaia-tooling.md) and the worse of the
/// two mistakes — the other is a message `rustc` gives instead.
#[test]
fn a_method_nothing_describes_is_not_refused() {
    assert!(!refused(
        "struct Sink { n: i64 }\n\
         fn hand(sink: Sink) { sink.swallow() }\n\
         fn main() { }\n"
    ));
}

/// **It is said once per parameter.** A body that changes one usually does so
/// several times, and three carets on one declaration is noise rather than
/// information.
#[test]
fn the_refusal_is_said_once() {
    let found = findings(
        "fn fill(out: Vec[i64]) {\n\
         \x20   out.push(1)\n\
         \x20   out.push(2)\n\
         \x20   out.push(3)\n\
         }\n\
         fn main() { }\n",
    );
    assert_eq!(
        found.iter().filter(|f| f.code == "NK1138").count(),
        1,
        "{found:#?}"
    );
}

/// **The caret is on the declaration**, because that is where the change has to
/// be written — not on the line that does it.
#[test]
fn the_caret_is_on_the_parameter() {
    let source = "fn fill(out: Vec[i64]) {\n\
                  \x20   out.push(1)\n\
                  }\n\
                  fn main() { }\n";
    let found = findings(source);
    let at = found
        .iter()
        .find(|f| f.code == "NK1138")
        .expect("it is refused");
    assert_eq!(&source[at.span.at()..at.span.at() + 3], "out");
}

/// Whether the source is refused with `NK1139`.
fn refused_let(source: &str) -> bool {
    findings(source).iter().any(|f| f.code == "NK1139")
}

/// **`NK1139`: a `let` that is changed says `mut`** — the same rule one binding
/// over, and the one [Part I 2.1](../../../docs/specification/10-nikaia-light.md)
/// states outright: it writes `// x = 20  <-- This would cause a Compiler
/// Error`, and this compiler was not the one giving it. The binding lowered
/// without its `mut` and `rustc` answered about a file nobody wrote.
#[test]
fn a_changed_let_without_the_word_is_refused() {
    // An assignment, which is Part I 2.1's own example.
    assert!(refused_let(
        "fn main() {\n\
         \x20   let x = 10\n\
         \x20   x = 20\n\
         }\n"
    ));

    // And a method that changes its subject, which is the same change.
    assert!(refused_let(
        "fn main() {\n\
         \x20   let xs = Vec()\n\
         \x20   xs.push(1)\n\
         }\n"
    ));

    // The same two with the word are programs.
    assert!(!refused_let(
        "fn main() {\n\
         \x20   let mut x = 10\n\
         \x20   x = 20\n\
         }\n"
    ));
    assert!(!refused_let(
        "fn main() {\n\
         \x20   let mut xs = Vec()\n\
         \x20   xs.push(1)\n\
         }\n"
    ));
}

/// **The scope is the one `scope` already keeps**, which is why the answer lives
/// on the binding rather than in a map of its own.
///
/// An inner block's `xs` stops being the answer when the block closes, and the
/// outer `mut xs` is the answer again — a parallel map would have had to be
/// pushed and popped at twenty-eight places, and getting one wrong is a correct
/// program refused ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)).
#[test]
fn an_inner_binding_does_not_answer_for_an_outer_one() {
    assert!(!refused_let(
        "fn main() {\n\
         \x20   let mut xs = Vec()\n\
         \x20   if true {\n\
         \x20       let xs = 1\n\
         \x20       println(f\"{xs}\")\n\
         \x20   }\n\
         \x20   xs.push(1)\n\
         }\n"
    ));
}

/// **And the two codes stay apart.** One rule, two places the word goes, and a
/// reader doing a different thing at each: a parameter's `mut` also decides
/// what the **caller** sees, where a `let`'s is only about this body.
#[test]
fn a_parameter_and_a_let_get_different_codes() {
    let found = findings(
        "fn fill(out: Vec[i64]) {\n\
         \x20   out.push(1)\n\
         }\n\
         fn main() {\n\
         \x20   let xs = Vec()\n\
         \x20   xs.push(1)\n\
         }\n",
    );
    assert!(found.iter().any(|f| f.code == "NK1138"), "{found:#?}");
    assert!(found.iter().any(|f| f.code == "NK1139"), "{found:#?}");
    let help: Vec<_> = found.iter().filter_map(|f| f.help.as_deref()).collect();
    assert!(
        help.iter().any(|h| h.contains("`mut out`")),
        "a parameter's word goes in the declaration: {help:?}"
    );
    assert!(
        help.iter().any(|h| h.contains("`let mut xs`")),
        "a `let`'s goes on the `let`: {help:?}"
    );
}

/// **A `for` binding, a lambda's argument and a `catch`'s `error` are not
/// refused**, because each of those is either not a place a program assigns to
/// or one whose `mut` is a question of its own — and silence is what C.4 asks
/// for where nothing was decided.
#[test]
fn only_the_two_written_bindings_are_asked() {
    assert!(!refused_let(
        "fn main() {\n\
         \x20   let mut xs = Vec()\n\
         \x20   xs.push(1)\n\
         \x20   for x in xs { println(f\"{x}\") }\n\
         }\n"
    ));
}

/// **The message as the reader sees it** (Part III C.2, rule 5): said in a
/// sentence, with both places underlined whole and named, and the way out.
/// Pinned whole, because the alignment of a caret under a name is exactly
/// what a change elsewhere in the renderer would move without anyone looking.
#[test]
fn a_let_changed_without_mut_is_explained_at_both_places() {
    let source = "fn main() {\n    let count = 0\n    for n in 0..<3 {\n        count += n\n    }\n    println(f\"{count}\")\n}\n";
    let found = findings(source);
    let finding = found
        .iter()
        .find(|f| f.code == "NK1139")
        .expect("NK1139 is raised");
    let rendered = nikaia::diagnostics::render_finding(finding, "app.nika", source);
    assert_eq!(
        rendered,
        "error[NK1139]: You're changing `count`, but it wasn't declared as mutable.\n\
         \x20 --> app.nika:4:9\n\
         \x20  |\n\
         \x202 |     let count = 0\n\
         \x20  |         ----- declared here without `mut`\n\
         \x20  ...\n\
         \x204 |         count += n\n\
         \x20  |         ^^^^^ changed here\n\
         \x20  |\n\
         \x20  = help: Add `mut` where it's declared: `let mut count`.\n"
    );
}
