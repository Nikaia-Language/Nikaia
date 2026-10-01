//! **Generating code from a type's shape** — Part II 10.3,
//! [ADR-088](../../../docs/specification/adr/adr-088.md) D2, D4, D5 and D6,
//! built by [ADR-181](../../../docs/specification/adr/adr-181.md).
//!
//! That section was specified in full and built by halves: the **bound** landed
//! at 0.0.120 and what it reaches was `NK1171` — *this is specified and this
//! compiler does not have it* — until 0.0.129.
//!
//! **These tests run programs.** What an unrolling produces is a question the
//! language below answers: a test that compared the emitted Rust against a
//! string would pass for a copy that read the wrong field, and one that asked
//! the checker would pass for a loop that was never unrolled at all.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings
}

fn lowered(source: &str) -> String {
    let found = findings(source);
    assert!(found.is_empty(), "a correct program: {found:#?}");
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

fn ran(purpose: &str, source: &str) -> String {
    let rust = lowered(source);
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

/// **Part II 10.3's own example, compiled and run** — the first time anything
/// in that section was.
///
/// Two types, so the unrolling is per **instantiation** and not per function:
/// `describe__User` prints two fields and `describe__Point` prints two others,
/// from one `describe` the program wrote once.
#[test]
fn the_specifications_own_example_runs() {
    let source = "struct User { name: ref String, age: i64 }\n\
                  struct Point { x: i64, y: i64 }\n\
                  \n\
                  fn describe[T: Struct](value: T) {\n\
                  \x20   for field in T::fields {\n\
                  \x20       println(f\"{field.name} = {field.of(value)}\")\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   describe(User { name: \"ada\", age: 36 })\n\
                  \x20   describe(Point { x: 1, y: 2 })\n\
                  }\n";
    let rust = lowered(source);
    // **Nothing is left at run time** (ADR-088 D5): no loop, no descriptor, no
    // dispatch — the field reads a program would have written by hand.
    assert!(!rust.contains("T::fields"), "{rust}");
    assert!(
        !rust.contains("fn describe<T>"),
        "the generic original: {rust}"
    );
    assert!(rust.contains("fn describe__User("), "{rust}");
    assert!(rust.contains("fn describe__Point("), "{rust}");
    assert!(rust.contains("\"name\", value.name"), "{rust}");

    assert_eq!(
        ran("the describe example", source),
        "name = ada\nage = 36\nx = 1\ny = 2\n"
    );
}

/// **A body wrong for one field is right for the others, and the message says
/// which turn it came from** ([ADR-088](../../../docs/specification/adr/adr-088.md)
/// D5).
///
/// `total + field.of(value)` is arithmetic for `age` and not for `name`, on one
/// line. Without the note this is the error class C++ templates carried for
/// twenty years: a message about a line that is correct for every other turn.
#[test]
fn a_body_wrong_for_one_field_names_the_turn() {
    let found = findings(
        "struct User { name: ref String, age: i64 }\n\
         \n\
         fn sum[T: Struct](value: T) -> i64 {\n\
         \x20   let mut total: i64 = 0\n\
         \x20   for field in T::fields {\n\
         \x20       total = total + field.of(value)\n\
         \x20   }\n\
         \x20   return total\n\
         }\n\
         \n\
         fn main() { println(f\"{sum(User { name: \"ada\", age: 36 })}\") }\n",
    );
    assert_eq!(
        found.len(),
        1,
        "one turn is wrong and one is right: {found:#?}"
    );
    assert!(
        found[0].notes.iter().any(|n| n
            == "This happened while going through `T::fields` for `User`, at the field `name`."),
        "{:#?}",
        found[0]
    );
}

/// **A reflected field answers two members**
/// ([ADR-088](../../../docs/specification/adr/adr-088.md) D2), and `NK1180`
/// prints the list, which is short enough to be a misspelling rather than
/// something nobody told the compiler about.
///
/// **Said once**, not once per field: the generic walk owns this message
/// ([ADR-181](../../../docs/specification/adr/adr-181.md) D3), because it is
/// about the function rather than about a turn.
#[test]
fn a_member_a_reflected_field_does_not_have_is_said_once() {
    let found = findings(
        "struct User { name: ref String, age: i64 }\n\
         \n\
         fn d[T: Struct](value: T) {\n\
         \x20   for field in T::fields { println(field.label) }\n\
         }\n\
         \n\
         fn main() { d(User { name: \"a\", age: 1 }) }\n",
    );
    let refused: Vec<_> = found.iter().filter(|f| f.code == "NK1180").collect();
    assert_eq!(refused.len(), 1, "{found:#?}");
    assert!(
        refused[0].message.contains("has no `label`"),
        "{:#?}",
        refused[0]
    );
}

/// **A `[T: Struct]` function that never asks for the shape stays generic**
/// ([ADR-181](../../../docs/specification/adr/adr-181.md) D1).
///
/// The bound says a shape *may* be asked for; the body says whether it is. One
/// function below, generic in the language below, and no copies.
#[test]
fn a_bound_that_is_never_asked_is_an_ordinary_generic() {
    let rust = lowered(
        "struct User { name: ref String }\n\
         \n\
         fn tell[T: Struct](value: T) { println(\"told\") }\n\
         \n\
         fn main() { tell(User { name: \"a\" }) }\n",
    );
    assert!(rust.contains("fn tell<T>("), "{rust}");
    assert!(!rust.contains("tell__User"), "{rust}");
}

/// **`T::variants` walks an `enum`'s variants**, in the order they are
/// declared: a variant answers `.name` and `.is(value)`, and the loop is
/// unrolled once per variant, as `T::fields` is once per field.
#[test]
fn variants_are_walked_like_fields() {
    let source = "enum Op { Add, Sub(i64), Mul { by: i64 } }\n\
         \n\
         fn which[T: Enum](value: T) -> String {\n\
         \x20   for v in T::variants {\n\
         \x20       if v.is(value) {\n\
         \x20           return f\"{v.name}\"\n\
         \x20       }\n\
         \x20   }\n\
         \x20   return \"?\"\n\
         }\n\
         \n\
         fn names[T: Enum](value: T) {\n\
         \x20   for v in T::variants { println(v.name) }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(which(Op::Sub(2)))\n\
         \x20   println(which(Op::Mul { by: 3 }))\n\
         \x20   names(Op::Add)\n\
         }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert_eq!(ran("variants", source), "Sub\nMul\nAdd\nSub\nMul\n");
}

/// **A variant answers two members**, as a field does: anything else is
/// `NK1180`, with the `match` named for what a variant carries.
#[test]
fn a_member_a_reflected_variant_does_not_have_is_refused() {
    let found = findings(
        "enum Shade { Odd, Even }\n\
         \n\
         fn tell[T: Enum](value: T) {\n\
         \x20   for v in T::variants { println(v.payload) }\n\
         }\n\
         \n\
         fn main() { tell(Shade::Odd) }\n",
    );
    let refused: Vec<_> = found.iter().filter(|f| f.code == "NK1180").collect();
    assert_eq!(refused.len(), 1, "{found:#?}");
    assert!(
        refused[0].message.contains("`payload`"),
        "{:#?}",
        refused[0]
    );
}

/// **`--comptime` prints what was unrolled**
/// ([ADR-088](../../../docs/specification/adr/adr-088.md) D6).
///
/// The same information [ADR-181](../../../docs/specification/adr/adr-181.md)
/// D3's diagnostic carries, offered on demand instead of on failure — which is
/// the arrangement `--overlaps`, `--sharing`, `--tethers` and `--trust` already
/// have, and the alternative to inventing syntax for it.
///
/// **A shape walk nobody calls has its own line**, and it is the one thing a
/// reader could not otherwise find out: no copy is written for it, so nothing
/// in the generated file says it exists.
#[test]
fn the_report_prints_what_was_unrolled() {
    let source = "struct User { name: ref String, age: i64 }\n\
                  struct Point { x: i64, y: i64 }\n\
                  \n\
                  fn describe[T: Struct](value: T) {\n\
                  \x20   for field in T::fields { println(field.name) }\n\
                  }\n\
                  \n\
                  fn unused[T: Struct](value: T) {\n\
                  \x20   for field in T::fields { println(field.name) }\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   describe(User { name: \"ada\", age: 36 })\n\
                  \x20   describe(Point { x: 1, y: 2 })\n\
                  }\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let report = check::unrolling_report(&[&parsed], &own, &nikaia::assets::Reads::none());

    assert!(
        report.contains("`describe` unrolled over `User` as `describe__User`")
            && report.contains("    name: ref String")
            && report.contains("    age: i64"),
        "{report}"
    );
    assert!(
        report.contains("`describe` unrolled over `Point` as `describe__Point`"),
        "one line per type actually used: {report}"
    );
    assert!(
        report.contains("`unused` walks `T::fields` and nothing calls it"),
        "the one thing only this report can say: {report}"
    );
}

/// **A shape walk declared in one file and called from another**
/// ([ADR-181](../../../docs/specification/adr/adr-181.md) D2, 0.0.130).
///
/// The files of a package share one namespace (Part I 9.1), so both halves of
/// an unrolling cross files: *which functions walk a shape* is read off the
/// items of the file that declares one, and *which types they were used with*
/// off the calls, which may all stand somewhere else. Collected per unit, the
/// declaring unit wrote no copy and no generic original at all.
///
/// Asked from **either side**, because neither file is the one that knows: the
/// unit holding the call learns that the name it wrote is a shape walk, and the
/// unit holding the body learns which copies to write.
#[test]
fn a_shape_walk_is_unrolled_across_the_files_of_a_package() {
    let declares = parse_to_ast(
        "struct User { name: ref String, age: i64 }\n\
         struct Point { x: i64, y: i64 }\n\
         \n\
         fn describe[T: Struct](value: T) {\n\
         \x20   for field in T::fields { println(field.name) }\n\
         }\n",
    )
    .expect("the first file parses");
    let calls = parse_to_ast(
        "fn main() {\n\
         \x20   describe(User { name: \"ada\", age: 36 })\n\
         \x20   describe(Point { x: 1, y: 2 })\n\
         }\n",
    )
    .expect("the second file parses");

    let beside = [&declares, &calls];
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let own = Ledger::infer_package(&beside, &library);
    let reads = nikaia::assets::Reads::none();

    // **The unit that writes the copies** knows both, although neither call
    // stands in it.
    let there = check::propagation_against(&declares, &beside, &own, &Ledger::empty(), &reads);
    assert!(
        there
            .unrolled
            .contains_key(&("describe".into(), "User".into()))
            && there
                .unrolled
                .contains_key(&("describe".into(), "Point".into())),
        "{:#?}",
        there.unrolled
    );

    // **The unit that holds the calls** knows the name is a shape walk, so each
    // call is rewritten to the copy rather than left pointing at an original
    // nobody emits.
    let here = check::propagation_against(&calls, &beside, &own, &Ledger::empty(), &reads);
    assert_eq!(here.unrolled_calls.len(), 2, "{:#?}", here.unrolled_calls);
    assert!(here.walks_fields.contains_key("describe"));

    // And the report is one report for the program, whichever file is first.
    for order in [[&declares, &calls], [&calls, &declares]] {
        let report = check::unrolling_report(&order, &own, &reads);
        assert!(
            report.contains("`describe` unrolled over `User` as `describe__User`")
                && report.contains("`describe` unrolled over `Point` as `describe__Point`"),
            "{report}"
        );
    }
}
