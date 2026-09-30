//! A grammar fold's `init`, `step` and `merge` are walked by the whole checker
//! ([ADR-092](../../../docs/specification/adr/adr-092.md)).
//!
//! They are expressions inside a **pattern**, and the grammar walk took a
//! rule's *action block* and nothing else — so `fn(acc, m) { undeclared }`
//! lowered without a word and `|acc, m| { undeclared }` reached `rustc`, which
//! answered about a file nobody wrote
//! ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
//!
//! [ADR-084](../../../docs/specification/adr/adr-084.md) D4 had closed the half
//! a **jump** can reach, with a walk of its own over these same three
//! expressions and deliberately no more. That walk is gone: the ordinary
//! `Expr::Closure` arm already crosses a boundary and counts loops from zero,
//! so one walk now answers both — and gives the jump the better of the two
//! messages.

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

/// Everything below is one rule in this grammar, with a `zero` to be the `init`
/// that the real examples write as `Summary::new`.
fn program(rule: &str) -> String {
    // **The entry is named at the call** now
    // ([ADR-082](../../../docs/specification/adr/adr-082.md) D1), so this
    // fixture has to name it too — and which rule it is varies per test, so it
    // is read off the rule being planted rather than written twice.
    let entry = rule
        .split_whitespace()
        .nth(2)
        .expect("`pub rule <name> -> …`");
    format!(
        "grammar Nums {{\n\
         \x20   rule N -> i64 = d:dec[i64](digit+) {{ d }}\n\
         \x20   {rule}\n\
         }}\n\
         \n\
         fn zero() -> i64 {{ return 0 }}\n\
         \n\
         fn main() {{\n\
         \x20   let text = \"7\"\n\
         \x20   let it = Nums::{entry}(text) catch {{ 0 }}\n\
         \x20   println(f\"{{it}}\")\n\
         }}\n"
    )
}

fn codes(rule: &str) -> Vec<String> {
    findings(&program(rule))
        .into_iter()
        .map(|f| f.code.to_string())
        .collect()
}

/// **The entry's own reproduction.**
#[test]
fn an_undeclared_name_in_a_step_is_refused() {
    let found = findings(&program(
        "pub rule file -> i64 = fold(N, zero, fn(acc, m) { nothing_declares_this })",
    ));
    assert_eq!(found.len(), 1, "one refusal: {found:#?}");
    assert_eq!(found[0].code, "NK1117");
    assert!(
        found[0].message.contains("nothing_declares_this"),
        "naming what is undeclared: {:#?}",
        found[0]
    );
}

/// **All three expressions, not just the one the reproduction used.** `init`
/// and `merge` are the two the entry did not name, and they are the two the
/// real programs write a `Type::method` in.
#[test]
fn init_and_merge_are_walked_too() {
    assert_eq!(
        codes("pub rule a -> i64 = fold(N, undeclared_init, fn(acc, m) { acc + m })"),
        ["NK1117"],
        "the `init`"
    );
    assert_eq!(
        codes("pub rule a -> i64 = par_fold(N, zero, fn(acc, m) { acc + m }, undeclared_merge)"),
        ["NK1117"],
        "the `merge` of a `par_fold`"
    );
}

/// **The shapes the real programs write are untouched**, which is the half that
/// decides whether this refusal costs anything.
///
/// `examples/1brc.nika` and `examples/access-log/src/main.nika` both write
/// `par_fold(RULE, Type::new, fn(acc, m) { acc.record(m) }, Type::merge)` — a
/// path as a value in two of the three positions, and a method on a binding
/// whose type is `?` in the third.
#[test]
fn what_the_examples_write_is_left_alone() {
    for rule in [
        "pub rule a -> i64 = fold(N, zero, fn(acc, m) { acc + m })",
        "pub rule a -> i64 = par_fold(N, zero, fn(acc, m) { acc + m }, zero)",
    ] {
        assert!(
            codes(rule).is_empty(),
            "`{rule}` is a program and this says otherwise: {:#?}",
            findings(&program(rule))
        );
    }
}

/// **A binding beside the fold is in scope**, which is why the walk runs inside
/// the frame the rule's pattern makes rather than beside it. Without that
/// frame, `head` is `NK1117` — measured, and it is the reason the two lines
/// exist.
#[test]
fn a_sibling_binding_reaches_into_the_lambda() {
    let rule = "pub rule mixed -> i64 = \
                head:N rest:fold(N, zero, fn(acc, m) { acc + m + head }) { rest }";
    assert!(
        codes(rule).is_empty(),
        "`head` is bound by this rule's own pattern: {:#?}",
        findings(&program(rule))
    );
}

/// **A jump in a step still meets `NK1132`**, which is
/// [ADR-084](../../../docs/specification/adr/adr-084.md) D4's promise — now
/// kept by the ordinary lambda path rather than by a walk of its own.
///
/// The message is the better of the two: the old walk ran with no barrier set
/// and said *"this is not in a loop"*, offering `return` as the way out —
/// which in a grammar action is the wrong advice.
#[test]
fn a_jump_in_a_step_names_the_lambda() {
    let found = findings(&program(
        "pub rule a -> i64 = fold(N, zero, fn(acc, m) { break })",
    ));
    assert_eq!(found.len(), 1, "one refusal: {found:#?}");
    assert_eq!(found[0].code, "NK1132");
    assert!(
        found[0].message.contains("outside this lambda"),
        "and it names the boundary: {:#?}",
        found[0]
    );
}

/// And the other side of that: a loop **written inside** the step is a loop the
/// jump may leave, so nothing is refused.
#[test]
fn a_loop_written_inside_the_step_takes_its_own_jump() {
    let rule = "pub rule a -> i64 = fold(N, zero, fn(acc, m) { \
                let mut t = acc\n\
                \x20       for i in 0..<m {\n\
                \x20           if i > 3 { break }\n\
                \x20           t = t + i\n\
                \x20       }\n\
                \x20       t })";
    assert!(
        codes(rule).is_empty(),
        "the `break` has a loop: {:#?}",
        findings(&program(rule))
    );
}

/// **The corpus**, which is what the entry asked for by name: *running the
/// corpus to see what it newly refuses*. The answer is nothing.
#[test]
fn nothing_in_the_corpus_is_newly_refused() {
    let mut refused = Vec::new();
    let mut looked_at = 0;
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
            let Ok(parsed) = parse_to_ast(&source) else {
                continue;
            };
            if !source.contains("fold(") {
                continue;
            }
            looked_at += 1;
            let own = Ledger::infer(&parsed);
            let library = Ledger::parse(STD).expect("std's shipped ledger parses");
            let found =
                check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
                    .findings;
            if !found.is_empty() {
                refused.push(format!("{}: {found:#?}", path.display()));
            }
        }
    }
    assert!(
        refused.is_empty(),
        "these programs write a fold and are right: {refused:#?}"
    );
    // Not vacuous: `1brc` and `access-log` both write a `par_fold`, and a
    // sweep that quietly found none would pass while checking nothing.
    assert!(
        looked_at >= 2,
        "the corpus still holds programs that write a fold, and this found {looked_at}"
    );
}
