//! **A block under any expression is the function's** (#494): one under `+`,
//! a cast, `??`, a field or an `if`'s head is walked like one under a call, so
//! what it calls reaches `sync`, `touches` and the build-time rule.

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn ledger(source: &str) -> Ledger {
    let parsed = parse_to_ast(source).expect("the source parses");
    Ledger::infer(&parsed)
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

const FOREIGN_UNDER_A_CAST: &str = "\
extern {
    fn abs(n: i32) -> i32
}

fn doubled(n: i64) -> i64 {
    return n * 2 + unsafe { abs(-1) } as i64
}

comptime N: i64 = doubled(21)

fn main() {
    println(f\"{N}\")
}
";

/// The C call under `+` and a cast reaches `doubled`'s touch set, which is
/// therefore unknown - and C does not run while the program is built
/// (ADR-321 D15): it ran, and `N` was 43.
#[test]
fn a_foreign_call_under_a_cast_is_seen() {
    let own = ledger(FOREIGN_UNDER_A_CAST);
    let doubled = &own.functions["doubled"];
    assert!(!doubled.touches_known, "{doubled:?}");
    let found = findings(FOREIGN_UNDER_A_CAST);
    assert!(found.iter().any(|f| f.code == "NK1152"), "{found:#?}");
}

/// A pausing call in an `if` that is an operand: the function is not `sync`.
#[test]
fn a_pausing_call_in_an_operand_is_seen() {
    let own = ledger(
        "use std::time\n\n\
         fn wait(c: bool) -> i64 {\n    \
         return 1 + if c { time::sleep(1.millis())\n 1 } else { 0 }\n}\n\n\
         fn main() {\n    println(f\"{wait(true)}\")\n}\n",
    );
    assert!(
        !own.functions["wait"].sync_claim.is_sync(),
        "{:?}",
        own.functions["wait"]
    );
}
