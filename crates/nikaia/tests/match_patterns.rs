//! The six pattern shapes
//! ([ADR-137](../../../docs/specification/adr/adr-137.md) D1, D2, D3).
//!
//! `match` matched a literal, a path with bindings and the catch-all, and
//! nothing else. The absence was visible in the corpus: `examples/calc/src/main.nika`
//! matched `step.0` and then read `step.1` in every arm, because it could not
//! match `step`.
//!
//! Five of the six are Rust's and lower verbatim. The sixth — the **range** —
//! needed the spelling decided first, because a pattern's range includes both
//! ends and `..` meant the other thing until D4 was built.

use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust
}

const OP: &str = "enum Op { Plus, Minus, Times }\n";

/// **A tuple pattern**, which is the shape `calc.nika` wanted.
#[test]
fn a_tuple_pattern_reads_the_pair() {
    let source = "fn describe(point: (i64, i64)) -> ref String {\n\
                  \x20   return match point {\n\
                  \x20       (0, 0) => \"origin\",\n\
                  \x20       else => \"elsewhere\",\n\
                  \x20   }\n\
                  }\n\
                  fn main() { println(f\"{describe((0, 0))}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("(0, 0) => \"origin\""),
        "{}",
        lowered(source)
    );
}

/// **An or-pattern**, and every alternative binds the same names.
#[test]
fn an_or_pattern_is_one_arm() {
    let source = "fn describe(point: (i64, i64)) -> ref String {\n\
                  \x20   return match point {\n\
                  \x20       (0, y) | (y, 0) => \"on an axis\",\n\
                  \x20       else => \"elsewhere\",\n\
                  \x20   }\n\
                  }\n\
                  fn main() { println(f\"{describe((0, 1))}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("(0, y) | (y, 0) =>"),
        "{}",
        lowered(source)
    );
}

/// **`NK1155`: the alternatives bind different names.** The rule is what keeps
/// the arm's body answerable — a name the body reads has to be bound whichever
/// alternative matched.
#[test]
fn alternatives_that_bind_different_names_are_refused() {
    let found: Vec<_> = findings(
        "fn f(point: (i64, i64)) -> i64 {\n\
         \x20   return match point {\n\
         \x20       (0, y) | (x, 0) => y,\n\
         \x20       else => 0,\n\
         \x20   }\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1155")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].message.contains('x') && found[0].message.contains('y'),
        "{found:#?}"
    );
}

/// **A guard is `if`** (D2), and it stands on the arm.
#[test]
fn a_guard_is_written_if() {
    let source = "fn describe(point: (i64, i64)) -> ref String {\n\
                  \x20   return match point {\n\
                  \x20       (x, y) if x == y => \"diagonal\",\n\
                  \x20       else => \"elsewhere\",\n\
                  \x20   }\n\
                  }\n\
                  fn main() { println(f\"{describe((1, 1))}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("(x, y) if x == y =>"),
        "{}",
        lowered(source)
    );
}

/// **A guarded arm covers nothing**, which is Rust's reading and has to be this
/// one too: the pattern says which values reach the arm and the guard says
/// which of those it takes, so the rest reach the arms below.
#[test]
fn a_guarded_arm_does_not_cover() {
    let found: Vec<_> = findings(&format!(
        "{OP}fn word(o: Op) -> ref String {{\n\
         \x20   return match o {{\n\
         \x20       rest if true => \"anything\",\n\
         \x20   }}\n\
         }}\n"
    ))
    .into_iter()
    .filter(|f| f.code == "NK1151")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
}

/// **A range, inclusive at both ends** (D3), and `..=` below because that is
/// how the language below spells the same set.
#[test]
fn a_range_pattern_includes_both_ends() {
    let source = "fn band(code: i64) -> ref String {\n\
                  \x20   return match code {\n\
                  \x20       200..299 => \"ok\",\n\
                  \x20       else => \"other\",\n\
                  \x20   }\n\
                  }\n\
                  fn main() { println(f\"{band(204)}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("200..=299 =>"),
        "{}",
        lowered(source)
    );
}

/// **`..<` is never written in a pattern** (D4): a pattern is one shape, and an
/// exclusive range is written by moving the end.
#[test]
fn an_exclusive_range_is_refused_in_a_pattern() {
    let refused = parse_to_ast(
        "fn band(code: i64) -> ref String {\n\
         \x20   return match code {\n\
         \x20       200..<300 => \"ok\",\n\
         \x20       else => \"other\",\n\
         \x20   }\n\
         }\n",
    )
    .expect_err("`..<` is not a pattern");
    let message = refused.to_string();
    assert!(message.contains("not written in a pattern"), "{message}");
    assert!(message.contains("200..298"), "{message}");
}

/// **A pattern inside a pattern**, and `..` for the fields this one does not
/// name.
#[test]
fn a_pattern_nests_and_a_struct_may_leave_the_rest() {
    let source = "struct Point { x: i64, y: i64 }\n\
                  enum Event { Click(Point), Quit }\n\
                  fn at(e: Event) -> i64 {\n\
                  \x20   return match e {\n\
                  \x20       Event::Click(Point { x, .. }) => x,\n\
                  \x20       Event::Quit => 0,\n\
                  \x20   }\n\
                  }\n\
                  fn main() { println(f\"{at(Event::Quit)}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("Event::Click(Point { x, .. }) => x"),
        "{}",
        lowered(source)
    );
}

/// **A struct pattern that names nothing** is every field and none of them.
#[test]
fn a_struct_pattern_may_name_no_field_at_all() {
    let source = "struct Point { x: i64, y: i64 }\n\
                  enum Event { Click(Point), Quit }\n\
                  fn any(e: Event) -> i64 {\n\
                  \x20   return match e {\n\
                  \x20       Event::Click(Point { .. }) => 1,\n\
                  \x20       Event::Quit => 0,\n\
                  \x20   }\n\
                  }\n\
                  fn main() { println(f\"{any(Event::Quit)}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("Event::Click(Point { .. }) => 1"),
        "{}",
        lowered(source)
    );
}

/// **An or-pattern covers every variant in it**, which is
/// [ADR-146](../../../docs/specification/adr/adr-146.md) §4's question answered
/// by building the form it waited on: `Op::Plus | Op::Minus` is two cases and
/// one arm.
#[test]
fn an_or_pattern_covers_each_variant_it_names() {
    let source = format!(
        "{OP}fn word(o: Op) -> ref String {{\n\
         \x20   return match o {{\n\
         \x20       Op::Plus | Op::Minus => \"additive\",\n\
         \x20       Op::Times => \"times\",\n\
         \x20   }}\n\
         }}\n\
         fn main() {{ println(f\"{{word(Op::Plus)}}\") }}\n"
    );
    assert!(findings(&source).is_empty(), "{:#?}", findings(&source));

    // And one left out is still a case missing.
    let short = format!(
        "{OP}fn word(o: Op) -> ref String {{\n\
         \x20   return match o {{\n\
         \x20       Op::Plus | Op::Minus => \"additive\",\n\
         \x20   }}\n\
         }}\n"
    );
    let found: Vec<_> = findings(&short)
        .into_iter()
        .filter(|f| f.code == "NK1151")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("Op::Times"), "{found:#?}");
}

/// **`calc.nika` matches the pair**, which is the corpus line this record
/// opened with.
#[test]
fn the_calculator_matches_the_pair() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/calc/src/main.nika");
    let source = std::fs::read_to_string(path).expect("calc.nika");
    assert!(source.contains("match step {"), "it still matches `step.0`");
    // The comment beside it *names* the old shape, which is the point of a
    // comment; what must be gone is the code.
    let code: Vec<&str> = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    assert!(
        !code.join("\n").contains("step.0"),
        "it still reads `step.0`"
    );
}
