//! Kap 7.1: which errors can leave a function.
//!
//! `throws` in the source says *that* a function can fail. Which errors is a
//! question about its body and about everything its body reaches, so it is
//! derived here rather than written down — the argument of
//! [ADR-005](../../../../docs/specification/adr/adr-005.md) D3, applied to the
//! second kind of contract in
//! [ADR-023](../../../../docs/specification/adr/adr-023.md) D1.
//!
//! This is [`super::sync`]'s walk with the lattice turned around. `sync` is a
//! **greatest** fixpoint over a boolean: everyone starts pure and loses the
//! claim on contact with something that pauses. An error set is a **least**
//! fixpoint: everyone starts with what they throw themselves, and grows by what
//! their callees throw, until nothing changes. Mutual recursion terminates for
//! the same reason it does there — the sets only grow, and there are finitely
//! many names to grow by.
//!
//! The call resolution is `sync`'s, not a second one. ADR-288 recorded what the
//! alternative costs: two analyses that have to agree about what `stats.add(5)`
//! goes to, and eventually do not. That includes the **method** half of it: the
//! type checker resolves a receiver and both walks read its answer, so
//! `a.add(v)` in ADR-288's `HashMap[&str, Stats]` chain reaches `Stats::add`
//! here for the same reason it reaches it in [`super::sync`]. Before that this
//! file answered every method call with `"?"` - fail-closed and therefore not a
//! bug, but a set that said "something I cannot name" about a call the compiler
//! had already named.

use std::collections::{BTreeMap, BTreeSet};

use super::sync::{Reached, reached};
use super::{Ledger, UNNAMED_ERROR};
use crate::ast::Expr;
use crate::check::MethodCalls;
use crate::parser::Parsed;
use nikaia_std::tools::throws::{self as nika, MethodsCalled};

/// Give every `throws` function of this package its `fails_with`.
///
/// **The walk, the graph and the fixpoint are Nikaia** (`tools/throws.nika`,
/// #125). What stays here is handing in each unit, what one expression reaches
/// - `sync`'s single answer (ADR-288) - and the type checker's method calls as
/// plain data, and writing the answer into the ledger. A callee the graph does
/// not hold still has the set the ledger gives it (ADR-296 D40).
pub fn infer(
    ledger: &mut Ledger,
    units: &[&Parsed],
    library: &Ledger,
    resolved: &BTreeMap<String, MethodCalls>,
) {
    let methods: BTreeMap<String, MethodsCalled> = resolved
        .iter()
        .map(|(key, calls)| {
            let called = MethodsCalled {
                resolved: calls.resolved.clone(),
                unseen: calls.unresolved || calls.code_fails,
            };
            (key.clone(), called)
        })
        .collect();
    let mut direct: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut calls: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for parsed in units.iter().copied() {
        let own: &Ledger = ledger;
        nika::unit_throws(
            &parsed.program,
            &parsed.interner,
            &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
            &|expr: &Expr| contribution(parsed, expr, own, library),
            &methods,
            own,
            library,
            &mut direct,
            &mut calls,
        );
    }
    for (name, errors) in nika::error_sets(&direct, &calls, ledger) {
        if let Some(contract) = ledger.functions.get_mut(&name) {
            contract.fails_with = errors;
        }
    }
}

fn contribution(
    parsed: &Parsed,
    expr: &Expr,
    own: &Ledger,
    library: &Ledger,
) -> nikaia_std::tools::throws::Contribution {
    let (call, errors) = match reached(parsed, expr, own, library) {
        Some(Reached::Own(name)) => (Some(name), Vec::new()),
        Some(Reached::Library { key, .. }) => (
            None,
            library
                .functions
                .get(&key)
                .map(|contract| contract.fails_with.clone())
                .unwrap_or_default(),
        ),
        Some(Reached::Opaque(_)) => (None, vec![UNNAMED_ERROR.to_string()]),
        Some(Reached::Method) | None => (None, Vec::new()),
    };
    nikaia_std::tools::throws::Contribution { call, errors }
}

/// The type a `throw` raises: `tools/throws.nika` answers it (#125).
pub(crate) fn error_type(parsed: &Parsed, thrown: &Expr) -> Option<String> {
    nikaia_std::tools::throws::error_type(&parsed.interner, thrown)
}
