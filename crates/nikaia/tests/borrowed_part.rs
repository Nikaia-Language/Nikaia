//! A field moved out of a value that is only lent here, handed back
//! ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md), #576).
//!
//! `return self.username` out of a `ref self` method is `NK1131`
//! ([ADR-083](../../../docs/specification/adr/adr-083.md)); the same line out
//! of a `ref` parameter, a `for` binding over a list or a `let` over a place
//! passed the check and reached `rustc`, whose *cannot move out of `l.ty`
//! which is behind a shared reference* is about a file nobody wrote.

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
struct Ty {
    name: String,
}

struct L {
    ty: Ty,
    n: i64,
}

struct Wide {
    inner: L,
}

fn main() {
}

";

fn with(function: &str) -> Vec<Finding> {
    errors(&format!("{HEAD}{function}"))
}

fn refused(function: &str, field: &str) {
    let found = with(function);
    let codes: Vec<_> = found.iter().map(|f| f.code).collect();
    assert_eq!(codes, ["NK1131"], "{function}\n{found:#?}");
    assert!(
        found[0].message.contains(&format!("`{field}`")),
        "{}",
        found[0].message
    );
    let help = found[0].help.clone().unwrap_or_default();
    assert!(help.contains(&format!("{field}.clone()")), "{help}");
}

fn accepted(function: &str) {
    let found = with(function);
    assert!(found.is_empty(), "{function}\n{found:#?}");
}

#[test]
fn a_field_of_a_ref_parameter_handed_back_is_refused() {
    refused("fn a(l: ref L) -> Ty {\n    return l.ty\n}\n", "l.ty");
}

#[test]
fn a_field_of_a_ref_parameter_as_the_tail_is_refused() {
    refused("fn a(l: ref L) -> Ty {\n    l.ty\n}\n", "l.ty");
}

#[test]
fn a_field_of_a_for_binding_handed_back_is_refused() {
    refused(
        "fn b(ls: ref Vec[L]) -> Ty {\n    for l in ls {\n        if l.n == 1 {\n            return l.ty\n        }\n    }\n    return Ty { name: \"z\" }\n}\n",
        "l.ty",
    );
}

#[test]
fn a_field_of_a_let_over_a_place_handed_back_is_refused() {
    refused(
        "fn e(ls: ref Vec[L]) -> Ty {\n    let l = ls[0]\n    return l.ty\n}\n",
        "l.ty",
    );
}

#[test]
fn a_field_handed_back_into_a_maybe_is_refused() {
    refused(
        "fn j(ls: ref Vec[L]) -> Ty? {\n    for l in ls {\n        return l.ty\n    }\n    return null\n}\n",
        "l.ty",
    );
}

#[test]
fn a_field_of_a_field_is_refused_by_its_whole_path() {
    refused(
        "fn w(x: ref Wide) -> Ty {\n    return x.inner.ty\n}\n",
        "x.inner.ty",
    );
}

#[test]
fn the_tail_of_a_ref_self_method_is_refused_as_its_return_is() {
    let found = errors(&format!(
        "{HEAD}impl L {{\n    fn t(ref self) -> Ty {{\n        self.ty\n    }}\n}}\n"
    ));
    let codes: Vec<_> = found.iter().map(|f| f.code).collect();
    assert_eq!(codes, ["NK1131"], "{found:#?}");
}

#[test]
fn what_copies_is_not_a_move() {
    accepted("fn n(l: ref L) -> i64 {\n    return l.n\n}\n");
}

#[test]
fn a_copy_made_with_clone_is_the_way_out() {
    accepted("fn c(l: ref L) -> Ty {\n    return l.ty.clone()\n}\n");
}

#[test]
fn a_value_the_function_owns_gives_its_field_away() {
    accepted("fn o(l: L) -> Ty {\n    return l.ty\n}\n");
}

#[test]
fn a_binding_over_what_a_drain_hands_over_is_owned() {
    accepted(
        "fn d(mut ls: Vec[L]) -> Ty {\n    for l in ls.drain() {\n        return l.ty\n    }\n    return Ty { name: \"z\" }\n}\n",
    );
}

#[test]
fn a_result_declared_a_view_is_a_view_of_the_field() {
    accepted("fn v(l: ref L) -> ref Ty {\n    return l.ty\n}\n");
}
