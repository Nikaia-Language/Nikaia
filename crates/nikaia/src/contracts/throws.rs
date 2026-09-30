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

use super::sync::{Reached, reached, visit_stmt, visit_stmt_blocks};
use super::{FnContract, Ledger, UNNAMED_ERROR};
use crate::ast::{Block, Expr, Item};
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

    // Start with what each one throws itself, then grow by what it reaches.
    let mut sets: BTreeMap<String, BTreeSet<String>> = graph
        .iter()
        .map(|(name, c)| (name.clone(), c.direct.clone()))
        .collect();

    loop {
        let mut changed = false;
        for (name, contrib) in &graph {
            let mut grown = sets[name].clone();
            for callee in &contrib.calls {
                // **A callee the graph does not hold still has a set**
                // ([ADR-173](../../../docs/specification/adr/adr-173.md) D3).
                // The graph is built from `Item::Fn`, and a **grammar's entry
                // rule** is not one: its contract is written straight into the
                // ledger ([ADR-082](../../../docs/specification/adr/adr-082.md)
                // D1), so the lookup found nothing and what a parse throws
                // reached no caller at all.
                //
                // *That was a miscompilation and not a missing note.* The
                // caller's channel was inferred without the parse's member in
                // it — `Result<i64, io::IoError>` for a body whose `?` yields a
                // parse failure — and `rustc` refused the generated file, which
                // is the one thing [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)
                // says may not happen. It was invisible while the entry threw
                // `"?"`, because a set of one `"?"` is the boxed channel and a
                // box takes anything.
                let theirs = sets
                    .get(callee)
                    .cloned()
                    .or_else(|| {
                        ledger
                            .functions
                            .get(callee)
                            .map(|c| c.throws.iter().cloned().collect())
                    })
                    .unwrap_or_default();
                for error in theirs {
                    grown.insert(error);
                }
            }
            if grown != sets[name] {
                sets.insert(name.clone(), grown);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for (name, set) in sets {
        let Some(contract) = ledger.functions.get_mut(&name) else {
            continue;
        };
        if contract.throws.is_empty() {
            continue;
        }
        contract.throws = if set.is_empty() {
            vec![UNNAMED_ERROR.to_string()]
        } else {
            set.into_iter().collect()
        };
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

    let mut contrib = Contrib::default();
    collect(parsed, body, own, library, false, &mut contrib);

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
                    Some(FnContract { throws, .. }) => {
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

fn collect(
    parsed: &Parsed,
    block: &Block,
    own: &Ledger,
    library: &Ledger,
    in_a_handler: bool,
    into: &mut Contrib,
) {
    for stmt in &block.stmts {
        // **The handlers written in this statement**, so the walk into them
        // knows where it is.
        let mut handlers: Vec<*const Block> = Vec::new();
        visit_stmt(parsed, &stmt.node, &mut |expr| {
            if let Expr::TryCatch { handler, .. } = expr {
                handlers.push(handler as *const Block);
            }
        });
        visit_stmt(parsed, &stmt.node, &mut |expr| {
            // **`throw error` in a handler passes the caught error on**
            // (ADR-157 D2): its types are the guarded expression's, which the
            // call there already contributed. Read as a value this compiler
            // cannot name, it added `"?"` and the function's sum fell back to
            // the unnamed error - found the day a `match` arm that is not a
            // block was walked at all (`open-work.md` §1.31).
            if in_a_handler
                && let Expr::Throw(thrown) = expr
                && matches!(&**thrown, Expr::Variable(name) if parsed.text(*name) == "error")
            {
                return;
            }
            if let Expr::Throw(thrown) = expr {
                match error_type(parsed, thrown) {
                    Some(name) => into.direct.insert(name),
                    // `throw` reached something this compiler cannot name -
                    // a value from a call, say. It fails; with what is the
                    // absence of a claim rather than an answer.
                    None => into.direct.insert(UNNAMED_ERROR.to_string()),
                };
                return;
            }
            match reached(parsed, expr, own, library) {
                Some(Reached::Own(name)) => {
                    into.calls.insert(name);
                }
                Some(Reached::Library { key, .. }) => {
                    // A library names its errors in its own ledger, or does not.
                    // `std` does not: its failures are the Rust ones below, and
                    // ADR-020 D5 has those entries written by hand.
                    if let Some(FnContract { throws, .. }) = library.functions.get(&key) {
                        for error in throws {
                            into.direct.insert(error.clone());
                        }
                    }
                }
                // A call nobody can name. It can fail, and treating "I cannot
                // see it" as "it does not fail" is the one direction ADR-010 D1
                // calls a vulnerability generator.
                Some(Reached::Opaque(_)) => {
                    into.direct.insert(UNNAMED_ERROR.to_string());
                }
                // Answered per function by the type checker, and merged in by
                // `contrib_of` once this walk is done - which is `sync`'s own
                // arrangement, and the point of ADR-028's single resolution.
                // An answer it could not find still arrives as `"?"`, so this
                // is no less pessimistic than the `Opaque` line above; it is
                // only less pessimistic about calls the compiler *can* name.
                Some(Reached::Method) => {}
                None => {}
            }
        });
        visit_stmt_blocks(&stmt.node, &mut |inner| {
            let handler = in_a_handler || handlers.contains(&(inner as *const Block));
            collect(parsed, inner, own, library, handler, into)
        });
    }
}

/// The type a `throw` raises. `ConfigError::NotFound(p)` and
/// `ConfigError::NotFound` are both `ConfigError` - a variant is written under
/// the enum that declares it (Part I, 3.4), so the first segment is the type.
pub(crate) fn error_type(parsed: &Parsed, thrown: &Expr) -> Option<String> {
    match thrown {
        // **Everything but the last segment**, which is the variant.
        // `ConfigError::NotFound` is `ConfigError` and
        // `io::IoError::NotFound` is `io::IoError` — a type keyed with its
        // module ([ADR-154](../../../../docs/specification/adr/adr-154.md) D3)
        // is still one type. Taking the **first** segment was right for exactly
        // as long as no error type lived in a module, and it recorded `io` —
        // the module — the day one did
        // ([ADR-158](../../../../docs/specification/adr/adr-158.md)).
        Expr::Path(segments) if segments.len() > 1 => Some(
            segments[..segments.len() - 1]
                .iter()
                .map(|s| parsed.text(*s))
                .collect::<Vec<_>>()
                .join("::"),
        ),
        Expr::Call { func, .. } => error_type(parsed, func),
        // **The type and not the variant.**
        // [ADR-023](../../../../docs/specification/adr/adr-023.md) D1 records a
        // set of error **types**, and D4 puts the variants on the other axis:
        // the set of types arriving at a `catch` is open, the variants within
        // one type are closed. A variant with **named** fields is written as a
        // struct literal — `ConfigError::BadSyntax { line, expected }` — and its
        // name carries the path, so the column read `ConfigError::BadSyntax`
        // where the tuple form one line up read `ConfigError`. One error type,
        // two entries, and a set of two is a set nothing can be named after
        // ([ADR-157](../../../../docs/specification/adr/adr-157.md) D1).
        Expr::StructLit { name, .. } => {
            let written = parsed.text(*name);
            Some(match written.rsplit_once("::") {
                Some((ty, _variant)) => ty.to_string(),
                None => written.to_string(),
            })
        }
        _ => None,
    }
}
