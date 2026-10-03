//! `throw`, `return`, `break` and `continue` where an expression stands
//! ([ADR-276](../../../docs/specification/adr/adr-276.md)).
//!
//! `=> throw NotFound` was a parse error. So was `?? throw Missing` and an arm
//! that is one `return`. Each had to be written with braces that said nothing:
//! there was no second statement they held together and no value they
//! produced.
//!
//! The type side was already decided — [ADR-276](../../../docs/specification/adr/adr-276.md)
//! gives the never type — so what this needed was the grammar and one ruling.

use nikaia::contracts::{Ledger, LedgerOps, STD};
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

const BAD: &str = "enum Bad { OutOfRange(i64), Missing }\n";

/// **A `match` arm that throws**, and the `match` is the *other* arm's type:
/// the arm that throws hands back nothing at all.
#[test]
fn an_arm_may_throw_and_the_match_keeps_its_type() {
    let source = format!(
        "{BAD}fn grade(score: i64) -> ref String throws {{\n\
         \x20   return match score {{\n\
         \x20       90..100 => \"A\",\n\
         \x20       else => throw Bad::OutOfRange(score),\n\
         \x20   }}\n\
         }}\n"
    );
    assert!(findings(&source).is_empty(), "{:#?}", findings(&source));
    let rust = lowered(&source);
    assert!(rust.contains("_ => return Err("), "{rust}");
}

/// **A `??` may end in a `throw`**, which is the shape *a value or a failure*
/// is written in everywhere.
#[test]
fn a_coalesce_may_end_in_a_throw() {
    let source = format!(
        "{BAD}fn find(id: i64) -> i64? {{ return null }}\n\
         fn user(id: i64) -> i64 throws {{\n\
         \x20   let u = find(id) ?? throw Bad::Missing\n\
         \x20   return u\n\
         }}\n"
    );
    assert!(findings(&source).is_empty(), "{:#?}", findings(&source));
    assert!(
        lowered(&source).contains("None => return Err("),
        "{}",
        lowered(&source)
    );
}

/// **And the fallback is not put in a closure**, which is the defect this
/// would otherwise have been: `unwrap_or_else` takes one, a jump may not cross
/// a function boundary ([ADR-276](../../../docs/specification/adr/adr-276.md)
/// D4), and the `return` would have returned from the closure while the
/// program carried on.
#[test]
fn a_jumping_fallback_is_a_match_and_not_a_closure() {
    let source = "fn find(id: i64) -> i64? { return null }\n\
                  fn pick(id: i64) -> i64 {\n\
                  \x20   let value = find(id) ?? return 0\n\
                  \x20   return value\n\
                  }\n";
    let rust = lowered(source);
    assert!(rust.contains("None => return 0"), "{rust}");
    assert!(!rust.contains("unwrap_or_else(|| return"), "{rust}");

    // A fallback that is an ordinary value keeps the closure it always had.
    let plain = lowered(
        "fn find(id: i64) -> i64? { return null }\n\
         fn pick(id: i64) -> i64 { return find(id) ?? 0 }\n",
    );
    assert!(plain.contains("nikaia_std::index::or("), "{plain}");
    assert!(plain.contains("|| 0"), "{plain}");
}

/// **A `continue` as a fallback**, inside a loop, which is the same rule with
/// the other word.
#[test]
fn a_fallback_may_continue() {
    let source = "fn find(id: i64) -> i64? { return null }\n\
                  fn total(n: i64) -> i64 {\n\
                  \x20   let mut sum = 0\n\
                  \x20   for i in 0..<n {\n\
                  \x20       let v = find(i) ?? continue\n\
                  \x20       sum += v\n\
                  \x20   }\n\
                  \x20   return sum\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("None => continue"),
        "{}",
        lowered(source)
    );
}

/// **Nothing about what they do changes** (D2). A `break` outside a loop is
/// still `NK1132`, wherever it is written.
#[test]
fn a_jump_expression_still_needs_its_loop() {
    let found: Vec<_> = findings(
        "fn find(id: i64) -> i64? { return null }\n\
         fn f() -> i64 { let v = find(1) ?? break  return v }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1132")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
}

/// **And a statement whose expression is one of these *is* that statement**
/// (D2), which is what keeps `NK1133` — D3's safety net — answering.
///
/// The parser reaches the expression form first, because `break_stmt` and
/// `continue_stmt` are last in `stmt` for a measured reason
/// ([ADR-276](../../../docs/specification/adr/adr-276.md) D19). A bare `break`
/// is normalised back to the statement it was.
#[test]
fn a_bare_break_is_still_the_statement_it_was() {
    let found: Vec<_> = findings(
        "fn f(n: i64) -> i64 {\n\
         \x20   let mut t = 0\n\
         \x20   for i in 0..<n {\n\
         \x20       break\n\
         \x20       t += i\n\
         \x20   }\n\
         \x20   return t\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1133")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
}

/// **A `return` inside a loop, as an expression**, which is D1's own third
/// example: `for x in xs { return x }` needs no braces round the `return`.
#[test]
fn a_loop_body_may_be_one_return() {
    let source = "fn first(n: i64) -> i64 {\n\
                  \x20   for i in 0..<n { return i }\n\
                  \x20   return 0\n\
                  }\n\
                  fn main() { println(f\"{first(3)}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(lowered(source).contains("return i;"), "{}", lowered(source));
}

/// **The braces still work** (D2): the statement forms are kept, so every
/// program written against the old grammar means what it meant.
#[test]
fn the_braced_forms_are_unchanged() {
    let source = format!(
        "{BAD}fn grade(score: i64) -> ref String throws {{\n\
         \x20   return match score {{\n\
         \x20       90..100 => \"A\",\n\
         \x20       else => {{ throw Bad::OutOfRange(score) }},\n\
         \x20   }}\n\
         }}\n"
    );
    assert!(findings(&source).is_empty(), "{:#?}", findings(&source));
}
