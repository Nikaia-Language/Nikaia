//! **`--bounds`: every check the optimizations may drop, and what became of
//! it** (#389). Proved sites say so; a checked one names the shape that
//! stopped the walk, with the issue that would supply the fact.

use nikaia::bounds::{BoundsChecks, OverflowChecks};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn report(source: &str, bounds: BoundsChecks, overflow: OverflowChecks) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::bounds_report::report(&parsed, source, "p.nika", &own, &library, bounds, overflow)
}

const SOURCE: &str = "\
struct Grid {
    width: i64,
}

fn scaled(p: i64, q: i64) -> i64 {
    return p * q
}

fn main() {
    let xs: Vec[i64] = [1, 2, 3]
    let mut total: i64 = 0
    for i in 0..<xs.len() {
        total = total + xs[i]
    }
    let g = Grid { width: 4 }
    let a = xs[0] * g.width
    println(f\"{total} {a} {scaled(2, 3)}\")
}
";

#[test]
fn every_site_is_listed_with_what_became_of_it() {
    let out = report(SOURCE, BoundsChecks::Removed, OverflowChecks::Removed);
    let line = |text: &str| {
        out.lines()
            .find(|l| l.contains(text))
            .unwrap_or_else(|| panic!("no line for `{text}`:\n{out}"))
            .to_string()
    };
    assert!(out.starts_with("bounds in p.nika: "), "{out}");
    assert!(
        out.lines()
            .any(|l| l.split_whitespace().nth(1) == Some("xs[i]") && l.ends_with("proved")),
        "{out}"
    );
    assert!(line("total + xs[i]").contains("checked"), "{out}");
    assert!(line("xs[0] * g.width").contains("#384"), "{out}");
    assert!(line("p * q").contains("#386"), "{out}");
    assert!(
        out.lines()
            .next()
            .is_some_and(|l| l.ends_with("indexes on, operations on")),
        "{out}"
    );
}

/// **What is proved does not depend on the build** (ADR-306 D14): a build
/// that removes no check is reported with the same proofs, and the first line
/// says it writes every check.
#[test]
fn a_build_that_removes_nothing_proves_the_same_and_says_so() {
    let kept = report(SOURCE, BoundsChecks::Kept, OverflowChecks::Kept);
    let removed = report(SOURCE, BoundsChecks::Removed, OverflowChecks::Removed);
    assert!(
        kept.lines()
            .next()
            .is_some_and(|l| l.ends_with("indexes off, operations off")),
        "{kept}"
    );
    assert_eq!(
        kept.lines().skip(1).collect::<Vec<_>>(),
        removed.lines().skip(1).collect::<Vec<_>>()
    );
}
