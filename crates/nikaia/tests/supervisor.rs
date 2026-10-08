//! **A supervisor's child starts fresh** ([ADR-328](../../../docs/specification/adr/adr-328.md),
//! Part II 12.8): what the children do when they run is
//! `tests/language/src/supervision.nika`; what is refused is here.

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn codes(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
        .into_iter()
        .filter(|f| f.code == "NK1229")
        .collect()
}

const WORK: &str = "use std::supervisor\n\nfn work(n: i64) throws {\n    println(f\"{n}\")\n}\n\n";

/// **`NK1229`**: a captured `mut` binding would carry one attempt's changes
/// into the next, and the help says how to start fresh.
#[test]
fn a_child_capturing_a_mut_binding_is_refused() {
    let found = codes(&format!(
        "{WORK}fn main() throws {{\n    let mut count = 0\n    count = count + 1\n    \
         supervisor::run([supervisor::child(fn {{ work(count) }})])\n}}\n"
    ));
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("`count`"), "{}", found[0].message);
    assert!(
        found[0]
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("count.clone()"),
        "{found:#?}"
    );
}

/// A binding without `mut` is lent to every attempt, and is no refusal.
#[test]
fn a_child_capturing_an_immutable_binding_is_accepted() {
    let found = codes(&format!(
        "{WORK}fn main() throws {{\n    let fixed = 7\n    \
         supervisor::run([supervisor::child(fn {{ work(fixed) }})])\n}}\n"
    ));
    assert!(found.is_empty(), "{found:#?}");
}
