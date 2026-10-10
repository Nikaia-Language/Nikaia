//! A part taken out of a `mut` parameter is given back before the function is
//! left, and no call that can fail stands in between
//! ([ADR-094](../../../docs/specification/adr/adr-094.md) D7, Part I 6.5,
//! `NK2108`, #576).
//!
//! The caller's value is whole whenever the caller can see it. Before this the
//! checker accepted a function that took a part and kept it, and `rustc` refused
//! the result in its own words.

use nikaia::check::{Finding, Severity};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn errors(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
        .into_iter()
        .filter(|f| f.severity == Severity::Error)
        .collect()
}

const HEAD: &str = "\
enum E {
    Num(i64),
    Add(E, E),
    Wrap(E),
}

enum Oops {
    Bad,
}

impl Error for Oops {
    fn message(ref self) -> String {
        return \"bad\".clone()
    }
}

fn risky() throws {
    throw Oops::Bad
}

fn keep(mut out: Vec[E], x: E) {
    out.push(x)
}

fn main() {
}

";

fn with(function: &str) -> Vec<Finding> {
    errors(&format!("{HEAD}{function}"))
}

fn refused(function: &str, words: &[&str]) {
    let found = with(function);
    let codes: Vec<_> = found.iter().map(|f| f.code).collect();
    assert_eq!(codes, ["NK2108"], "{found:#?}");
    let text = format!(
        "{} {} {}",
        found[0].message,
        found[0].notes.join(" "),
        found[0].help.clone().unwrap_or_default()
    );
    for word in words {
        assert!(text.contains(word), "no `{word}` in: {text}");
    }
}

fn accepted(function: &str) {
    let found = with(function);
    assert!(found.is_empty(), "{found:#?}");
}

/// The case the issue opens with: the arm hands `a` to something that keeps
/// it, and the function ends with `e` still missing it.
#[test]
fn a_part_kept_and_never_given_back_is_refused_at_the_end() {
    refused(
        "fn steal(mut e: E, mut out: Vec[E]) {
    match e {
        E::Num(n) => {}
        E::Add(a, b) => { keep(out, a) }
        E::Wrap(x) => {}
    }
}
",
        &["`e`", "`a`", "Assign"],
    );
}

#[test]
fn the_arm_that_takes_the_parts_and_assigns_the_whole_is_accepted() {
    accepted(
        "fn wrap_nums(mut e: E) {
    match e {
        E::Num(n) => { e = E::Wrap(E::Num(n)) }
        E::Add(a, b) => {
            let mut l = a
            let mut r = b
            wrap_nums(l)
            wrap_nums(r)
            e = E::Add(l, r)
        }
        E::Wrap(x) => {}
    }
}
",
    );
}

#[test]
fn a_part_handed_to_a_constructor_and_assigned_back_is_accepted() {
    accepted(
        "fn swap(mut e: E) {
    match e {
        E::Num(n) => {}
        E::Add(a, b) => { e = E::Add(b, a) }
        E::Wrap(x) => { e = x }
    }
}
",
    );
}

#[test]
fn a_part_taken_from_another_value_is_nobodys_business() {
    accepted(
        "fn pull(e: E, mut out: Vec[E]) {
    match e {
        E::Num(n) => {}
        E::Add(a, b) => { keep(out, a) }
        E::Wrap(x) => {}
    }
}
",
    );
}

#[test]
fn an_early_return_before_the_whole_is_assigned_is_refused() {
    refused(
        "fn early(mut e: E, stop: bool) {
    match e {
        E::Num(n) => {}
        E::Add(a, b) => {
            let mut l = a
            let mut r = b
            if stop {
                return
            }
            e = E::Add(l, r)
        }
        E::Wrap(x) => {}
    }
}
",
        &["`return`", "`a`"],
    );
}

#[test]
fn a_return_after_the_whole_is_assigned_is_accepted() {
    accepted(
        "fn early(mut e: E, stop: bool) {
    match e {
        E::Num(n) => {}
        E::Add(a, b) => {
            let mut l = a
            let mut r = b
            e = E::Add(l, r)
            if stop {
                return
            }
        }
        E::Wrap(x) => {}
    }
}
",
    );
}

#[test]
fn a_throw_while_a_part_is_out_is_refused() {
    refused(
        "fn thrown(mut e: E, stop: bool) throws {
    match e {
        E::Num(n) => {}
        E::Add(a, b) => {
            let mut l = a
            let mut r = b
            if stop {
                throw Oops::Bad
            }
            e = E::Add(l, r)
        }
        E::Wrap(x) => {}
    }
}
",
        &["`throw`", "`a`"],
    );
}

#[test]
fn a_call_that_can_fail_between_the_taking_and_the_giving_back_is_refused() {
    refused(
        "fn between(mut e: E) throws {
    match e {
        E::Num(n) => {}
        E::Add(a, b) => {
            let mut l = a
            let mut r = b
            risky()
            e = E::Add(l, r)
        }
        E::Wrap(x) => {}
    }
}
",
        &["can fail", "`a`"],
    );
}

#[test]
fn a_failing_call_inside_the_value_that_is_assigned_back_is_between() {
    refused(
        "fn inside(mut e: E) throws {
    match e {
        E::Num(n) => {}
        E::Add(a, b) => {
            e = E::Add(rebuilt(a), b)
        }
        E::Wrap(x) => {}
    }
}

fn rebuilt(x: E) -> E throws {
    risky()
    return x
}
",
        &["can fail"],
    );
}

#[test]
fn the_same_call_before_the_match_or_after_the_assignment_is_accepted() {
    accepted(
        "fn around(mut e: E) throws {
    risky()
    match e {
        E::Num(n) => {}
        E::Add(a, b) => {
            let mut l = a
            let mut r = b
            e = E::Add(l, r)
            risky()
        }
        E::Wrap(x) => {}
    }
    risky()
}
",
    );
}

#[test]
fn a_give_back_in_one_branch_only_leaves_the_other_way_out_incomplete() {
    refused(
        "fn some(mut e: E, c: bool) {
    match e {
        E::Num(n) => {}
        E::Add(a, b) => {
            let mut l = a
            let mut r = b
            if c {
                e = E::Add(l, r)
            }
        }
        E::Wrap(x) => {}
    }
}
",
        &["`a`"],
    );
}

#[test]
fn a_give_back_after_the_match_covers_every_arm() {
    accepted(
        "fn after(mut e: E, mut out: Vec[E]) {
    match e {
        E::Num(n) => {}
        E::Add(a, b) => { keep(out, a) }
        E::Wrap(x) => {}
    }
    e = E::Num(0)
}
",
    );
}

#[test]
fn a_part_changed_in_place_is_not_taken() {
    accepted(
        "fn deeper(mut e: E) throws {
    match e {
        E::Num(n) => {}
        E::Add(a, b) => {
            deeper(a)
            deeper(b)
        }
        E::Wrap(x) => { deeper(x) }
    }
}
",
    );
}
