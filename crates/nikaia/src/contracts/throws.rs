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
//! The call resolution is `sync`'s, not a second one. ADR-028 recorded what the
//! alternative costs: two analyses that have to agree about what `stats.add(5)`
//! goes to, and eventually do not. That includes the **method** half of it: the
//! type checker resolves a receiver and both walks read its answer, so
//! `a.add(v)` in ADR-031's `HashMap[&str, Stats]` chain reaches `Stats::add`
//! here for the same reason it reaches it in [`super::sync`]. Before that this
//! file answered every method call with `"?"` - fail-closed and therefore not a
//! bug, but a set that said "something I cannot name" about a call the compiler
//! had already named.

use std::collections::{BTreeMap, BTreeSet};

use super::sync::{Reached, reached};
use super::{FnContract, Ledger, UNNAMED_ERROR};
use crate::ast::{Expr, Item};
use crate::check::MethodCalls;
use crate::parser::Parsed;

/// What one function contributes to its own error set before its callees are
/// taken into account.
#[derive(Debug, Default)]
struct Contrib {
    /// Named here: `throw ConfigError::NotFound(p)` puts `ConfigError` in.
    /// `"?"` where something can fail and this compiler cannot name what with.
    direct: BTreeSet<String>,
    /// Functions in this **package** it calls — every unit of it, since
    /// [ADR-100](../../../docs/specification/adr/adr-100.md) D2. Their errors
    /// reach it, because nothing marks a failing call (ADR-023 D8) —
    /// propagation is what a call does.
    calls: BTreeSet<String>,
}

/// Give every `throws` function in the ledger the error set its body earns.
///
/// Only a function the source declared `throws` gets one: a function that
/// cannot fail cannot propagate, and a declared `throws` whose body names
/// nothing keeps `["?"]` rather than losing its entry. That last case is the
/// same shape as an asserted `sync` — the declaration is a promise a caller
/// already relies on, and inference is here to say more than it, never less.
///
/// `resolved` is the type checker's answer to what a method call goes to
/// (ADR-028), the same map [`super::sync::infer`] is handed. Having one of the
/// two read it and the other not is what recorded `throws = ["?"]` for a
/// function whose only failing call was `a.add(v)` - a chain
/// `docs/history/from-for-throws-and-touches.md` §3 walks link by link, and which
/// `sync` had been following all along.
pub fn infer(
    ledger: &mut Ledger,
    units: &[&Parsed],
    library: &Ledger,
    resolved: &BTreeMap<String, MethodCalls>,
) {
    let mut graph: BTreeMap<String, Contrib> = BTreeMap::new();

    for parsed in units.iter().copied() {
        for item in &parsed.program.items {
            match &item.node {
                Item::Fn { .. } => {
                    if let Some((name, contrib)) =
                        contrib_of(parsed, &item.node, None, ledger, library, resolved)
                    {
                        graph.insert(name, contrib);
                    }
                }
                Item::Impl {
                    target, methods, ..
                } => {
                    let target = parsed.text(target.name).to_string();
                    for method in methods {
                        if let Some((name, contrib)) = contrib_of(
                            parsed,
                            &method.node,
                            Some(&target),
                            ledger,
                            library,
                            resolved,
                        ) {
                            graph.insert(name, contrib);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // **The fixpoint is Nikaia** (`tools/throws.nika`, #125): it starts with
    // what each one throws itself, grows by what it reaches - a callee the
    // graph does not hold still has the set the ledger gives it (ADR-173 D3) -
    // and answers what every `throws` function fails with.
    let direct: BTreeMap<String, BTreeSet<String>> = graph
        .iter()
        .map(|(name, c)| (name.clone(), c.direct.clone()))
        .collect();
    let calls: BTreeMap<String, BTreeSet<String>> =
        graph.into_iter().map(|(name, c)| (name, c.calls)).collect();
    for (name, errors) in nikaia_std::tools::throws::error_sets(&direct, &calls, ledger) {
        if let Some(contract) = ledger.functions.get_mut(&name) {
            contract.fails_with = errors;
        }
    }
}

/// What one function throws directly, and whom it calls. `None` where the item
/// is not a `throws` function - one that cannot fail has no set to grow.
fn contrib_of(
    parsed: &Parsed,
    item: &Item,
    target: Option<&str>,
    own: &Ledger,
    library: &Ledger,
    resolved: &BTreeMap<String, MethodCalls>,
) -> Option<(String, Contrib)> {
    let Item::Fn {
        name,
        body,
        can_throw: throws,
        ..
    } = item
    else {
        return None;
    };
    if !*throws {
        return None;
    }

    let own_name = match name {
        Some(name) => parsed.text(*name).to_string(),
        None => "new".to_string(),
    };
    let key = match target {
        Some(target) => format!("{target}::{own_name}"),
        None => own_name,
    };

    // **The walk is Nikaia** (`tools/throws.nika`, #125); what a call
    // resolves to is `sync`'s single answer (ADR-028), handed in.
    let thrown = nikaia_std::tools::throws::thrown_by(
        body,
        &parsed.interner,
        &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
        &|expr: &Expr| contribution(parsed, expr, own, library),
    );
    let mut contrib = Contrib {
        direct: thrown.direct,
        calls: thrown.calls,
    };

    // What the walk above left to somebody else: this function's method calls,
    // as the type checker resolved them (ADR-028). Merged rather than
    // reconciled, exactly as `sync::reach_of` merges them - the walk skips
    // method calls entirely and this covers those and nothing else.
    //
    // The polarity is the one the module header states. `unresolved` is the
    // absence of an answer and contributes `"?"`; a resolved callee
    // contributes what *it* throws; and a callee that resolved to a name no
    // ledger carries after all contributes `"?"` too, because "I cannot see
    // it" may never be read as "it does not fail" (ADR-010 D1).
    if let Some(methods) = resolved.get(&key) {
        if methods.unresolved || methods.code_fails {
            contrib.direct.insert(UNNAMED_ERROR.to_string());
        }
        for callee in &methods.resolved {
            if own.functions.contains_key(callee) {
                contrib.calls.insert(callee.clone());
            } else {
                match library.functions.get(callee) {
                    Some(FnContract {
                        fails_with: throws, ..
                    }) => {
                        contrib.direct.extend(throws.iter().cloned());
                    }
                    None => {
                        contrib.direct.insert(UNNAMED_ERROR.to_string());
                    }
                }
            }
        }
    }

    Some((key, contrib))
}

/// What one expression contributes to the function it stands in, as `sync`'s
/// resolution answers it: a function of this package it calls, the errors a
/// library's ledger names for its callee, or `?` for a call nobody can name.
///
/// A library names its errors in its own ledger, or does not; `std` does not,
/// and ADR-020 D5 has its Rust failures written by hand. A call nobody can
/// name can fail, and reading *I cannot see it* as *it does not fail* is the
/// one direction ADR-010 D1 calls a vulnerability generator. A method call is
/// answered per function by the type checker, and merged in by `contrib_of` -
/// `sync`'s own arrangement, and the point of ADR-028's single resolution.
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
