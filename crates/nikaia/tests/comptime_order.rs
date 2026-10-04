//! **Where a `comptime` may be read before it is declared** (ADR-287 D19,
//! #380): at item level in any order, as other items are, and a ring is
//! `NK1168`; inside a body in written order, as `let` is, so a constant further
//! down is `NK1117` - and only that, not also `NK1127`.

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn codes(source: &str) -> Vec<String> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
        .into_iter()
        .map(|f| f.code.to_string())
        .collect()
}

#[test]
fn an_item_level_constant_may_read_one_declared_below_it() {
    let source = "comptime D = C + 1\ncomptime C = 1\n\nfn main() {\n    println(f\"{D}\")\n}\n";
    assert!(codes(source).is_empty(), "{:?}", codes(source));
}

#[test]
fn an_item_level_ring_is_nk1168() {
    let source =
        "comptime A = B + 1\ncomptime B = A + 1\n\nfn main() {\n    println(f\"{A}\")\n}\n";
    assert!(
        codes(source).contains(&"NK1168".to_string()),
        "{:?}",
        codes(source)
    );
}

#[test]
fn a_body_level_forward_reference_is_nk1117_alone() {
    let source =
        "fn main() {\n    comptime D = C + 1\n    comptime C = 1\n    println(f\"{D}\")\n}\n";
    assert_eq!(codes(source), vec!["NK1117".to_string()]);
}

#[test]
fn a_body_constant_may_read_an_item_constant() {
    let source =
        "comptime C = 1\n\nfn main() {\n    comptime D = C + 1\n    println(f\"{D}\")\n}\n";
    assert!(codes(source).is_empty(), "{:?}", codes(source));
}
