//! **A view handed back is handed back silently**
//! ([ADR-283](../../../docs/specification/adr/adr-283.md) D23-D24, #442): a
//! view of what a function owns moves it into the caller's keep, and a view
//! of a parameter borrows from the caller's argument - an argument made in the
//! call going into the caller's keep. Each of these reached `rustc` before.
//!
//! What the programs compute is `tests/language/src/views_handed_back.nika`;
//! this file keeps the refusal and what `--tethers` reports.

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

const OWNED: &str = "\
fn d() -> ref String {
    let s: String = \"x\".clone()
    return ref s
}

fn main() {
    println(d())
}
";

const PARAMETERS: &str = "\
struct Row {
    name: String,
}

fn e(t: String) -> ref String {
    return ref t
}

fn c(row: Row) -> ref String {
    return ref row.name
}

fn first(a: String, b: String) -> ref String {
    if a.len() > b.len() {
        return ref a
    }
    return ref b
}

fn label(a: String, b: String) -> ref String {
    if a == b {
        return \"same\"
    }
    return \"other\"
}

fn main() {
    let t: String = \"y\".clone()
    let row = Row { name: \"z\".clone() }
    println(f\"{e(t)} {c(row)} {first(\"long\", \"x\")} {label(\"a\", \"b\")}\")
}
";

/// **Step 4**: `--tethers` names the keep, and what a borrowed result borrows
/// from.
#[test]
fn tethers_names_the_keep() {
    let report = |source: &str| {
        let parsed = parse_to_ast(source).expect("the source parses");
        let ledger = Ledger::infer(&parsed);
        nikaia::contracts::tether::report(&parsed, &ledger)
    };
    let owned = report(OWNED);
    assert!(
        owned.contains("`s` lives in `main`'s keep: handed back by `d`"),
        "{owned}"
    );
    let parameters = report(PARAMETERS);
    assert!(
        parameters.contains("borrowed  `<result>` - from the caller's argument for `row`"),
        "{parameters}"
    );
}

/// **Step 5**: `NK1104`'s help is the way out, and following it is a program
/// (that it runs is `tests/language/src/views_handed_back.nika`).
#[test]
fn following_nk1104s_help_is_a_program() {
    let refused = "\
fn e(t: String) -> ref String {
    return t
}

fn main() {
    println(e(\"y\"))
}
";
    let found = findings(refused);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1104");
    assert_eq!(
        found[0].help.as_deref(),
        Some("Write `ref` in front of it to pass a view of it.")
    );
    let followed = refused.replace("return t", "return ref t");
    assert!(findings(&followed).is_empty(), "{followed}");
}

/// **An argument made in the call whose view is handed on** (#493): the view
/// leaves `g` through its result, so the argument goes into the keep `g` is
/// given and `main` declares it beside the call - it was *cannot return value
/// referencing local variable*.
#[test]
fn an_argument_made_in_the_call_handed_on_lives_in_the_callers_keep() {
    let source = "\
fn e(t: String) -> ref String {
    return ref t
}

fn g() -> ref String {
    return e(\"y\".clone())
}

fn main() {
    println(g())
}
";
    let parsed = parse_to_ast(source).expect("the source parses");
    let report = nikaia::contracts::tether::report(&parsed, &Ledger::infer(&parsed));
    assert!(
        report.contains(
            "argument 1 made in the call to `e` lives in the caller's keep, because views of it leave this function"
        ),
        "{report}"
    );
    assert!(
        report.contains(
            "argument 1 made in the call to `e` lives in `main`'s keep: handed back by `g`"
        ),
        "{report}"
    );
}
