//! A `catch` over an expression that cannot fail is `NK1134`
//! ([ADR-308](../../../docs/specification/adr/adr-308.md)).
//!
//! The lowering makes a `catch` a `match` over a `Result`, so a guarded
//! expression that is not one produced `E0308` about the generated file, naming
//! a `match` and an `Ok` arm nobody wrote —
//! [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s class.
//!
//! **Every test here is about the polarity**, because the refusal has a way of
//! being wrong that the defect did not:
//! [Part III C.4](../../../docs/specification/30-nikaia-tooling.md) forbids
//! refusing a program that is right, and a call no contract describes says
//! *nothing* about whether it throws. So the rule is **known not to fail**,
//! never *not known to fail* — and each shape that this compiler cannot answer
//! for has a test saying it stays quiet.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings
}

fn refusals(source: &str) -> Vec<check::Finding> {
    findings(source)
        .into_iter()
        .filter(|f| f.code == "NK1134")
        .collect()
}

/// A function that can fail and one that cannot, so every fixture below says
/// which it means without an undescribed name doing the work.
const DECLARED: &str = r#"
fn risky(text: ref String) -> i32 throws {
    return text.parse()
}

fn safe(n: i32) -> i32 {
    return n + 1
}
"#;

fn program(body: &str) -> String {
    format!(
        "{DECLARED}\nfn guarded(text: ref String) -> i32 {{\n    return {body}\n}}\n\n\
         fn main() {{\n    let it = guarded(\"7\")\n    println(f\"{{it}}\")\n}}\n"
    )
}

/// **The reproduction**, which is the line from `examples/n-body/src/main.nika` that the
/// `catch`-binding round was written against: `text.len()` is in the ledger and
/// carries no `throws`, so there is nothing for the handler to do.
#[test]
fn the_reproduction_is_refused() {
    let found = refusals(&program("text.len() as i32 catch { 1000 }"));
    assert_eq!(found.len(), 1, "one refusal, at the `catch`: {found:#?}");
    let said = &found[0];
    assert!(
        said.message
            .contains("Nothing here can fail, so the `catch` has nothing to handle"),
        "said in this language's words: {said:#?}"
    );
    assert!(
        said.help
            .as_deref()
            .is_some_and(|h| h.contains("Remove the `catch` and its handler")),
        "and the way out is to remove it, because the expression is the value: {said:#?}"
    );
}

/// **The shapes that are refused**, each one a thing this compiler can look up
/// and each one carrying no `throws`.
#[test]
fn what_cannot_fail_is_refused() {
    for body in [
        "safe(1) catch { 2 }",
        "1 catch { 2 }",
        "text.len() as i32 catch { 0 }",
        "safe(safe(1)) catch { 0 }",
    ] {
        assert_eq!(
            refusals(&program(body)).len(),
            1,
            "`{body}` has nothing to catch"
        );
    }
}

/// **The shapes that are not**, which is the half that has to be right.
///
/// `risky` declares `throws`; a failure inside an *argument* is still inside
/// the guarded expression; and a call the ledger does not describe is the C.4
/// case — `insert_str` is `common`'s stand-in for that, so the day somebody
/// writes it down this moves with the rest.
#[test]
fn what_might_fail_is_left_alone() {
    let undescribed = common::undescribed_call("text.to_string()");
    for body in [
        "risky(text) catch { 1 }".to_string(),
        "safe(risky(text)) catch { 3 }".to_string(),
        format!("{undescribed} catch {{ 0 }}"),
    ] {
        assert!(
            refusals(&program(&body)).is_empty(),
            "`{body}` may fail, or may for all this compiler knows"
        );
    }
}

/// **A `catch` inside another one's guarded expression is not the outer one's
/// answer**, and the outer one is told *no answer* rather than *nothing fails*.
///
/// `(text.parse() catch { 1 }) catch { 2 }` really does have nothing left for
/// the second handler — but saying so needs what a handler's own failure does,
/// which is [ADR-292](../../../docs/specification/adr/adr-292.md)'s question
/// and not this refusal's. So it stays quiet, in the direction that cannot
/// refuse a program that is right.
#[test]
fn a_nested_catch_leaves_the_outer_one_alone() {
    assert!(
        refusals(&program("(text.parse() catch { 1 }) catch { 2 }")).is_empty(),
        "the outer `catch` is not refused, on purpose"
    );
}

/// **Running a grammar can fail, and the six examples that write it said so by
/// being refused.**
///
/// [ADR-023](../../../docs/specification/adr/adr-023.md) D9: a parse failure
/// leaves the parser as the Nikaia error it is and lands in the `catch` beside
/// the `dsl`. It is not a call and carries no contract, so the walk had to be
/// told — which is what `examples/access-log/src/main.nika`, `calc`, `config`, `json`,
/// `k-nucleotide` and `report` did, all at once, the first time this ran over
/// the corpus.
#[test]
fn a_grammar_run_over_an_input_may_fail() {
    let source = r#"
grammar Nums {
    entry rule number -> i64 =
        n:dec[i64](digit+) { n }
}

fn main() {
    let text = "7"
    let it = Nums.number(text) catch { 0 }
    println(f"{it}")
}
"#;
    assert!(
        refusals(source).is_empty(),
        "a `dsl … from …` is exactly what a `catch` beside a `dsl` is for"
    );
}

/// **The corpus is the test that matters most**, and it is the one that found
/// the `dsl` case. Kept here as a named assertion rather than a habit: every
/// `.nika` in `examples/` and `tests/samples/` goes past the checker with no
/// `NK1134`.
#[test]
fn nothing_in_the_corpus_is_newly_refused() {
    let mut refused = Vec::new();
    for directory in ["examples", "tests/samples"] {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(directory);
        for path in common::standalone_programs(&root) {
            if path.extension().is_none_or(|e| e != "nika") {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            // A file that does not parse is somebody else's problem; this test
            // is about what the checker does to the ones that do.
            if parse_to_ast(&source).is_err() {
                continue;
            }
            if !refusals(&source).is_empty() {
                refused.push(path.display().to_string());
            }
        }
    }
    assert!(
        refused.is_empty(),
        "these programs are right and this refusal says otherwise: {refused:#?}"
    );
}
