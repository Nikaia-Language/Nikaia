// crates/nikaia/src/contracts/locks.rs
//
// Whether a function **touches a lock** ([ADR-039](../../../../docs/specification/adr/adr-039.md)
// D3), propagated over the same call graph `sync` uses and with the opposite
// lattice.
//
// ## One traversal and two lattices
//
// | | starts from | rule |
// | :--- | :--- | :--- |
// | `sync` | everyone is `sync` | **greatest** fixpoint — take the claim from whoever reaches a non-holder |
// | this | nobody touches one | **least** fixpoint — give the property to whoever reaches a holder |
//
// So mutual recursion between pure functions keeps `sync`
// ([ADR-027](../../../../docs/specification/adr/adr-027.md) D1) and correctly
// gets nothing here, for the mirrored reason — the same shape `keeps` has
// beside it, and for the same reason: a restriction is added on doubt where a
// promise is taken away on doubt.
//
// ## What a base case is, and why the checker answers it
//
// A **door** is `get`, `set`, `access` or `update` on a `Locked` or a
// `SharedMut`, and `access_all` or `update_all` over several
// ([ADR-039](../../../../docs/specification/adr/adr-039.md) D10,
// [ADR-065](../../../../docs/specification/adr/adr-065.md)). Which of those a
// call goes to is a question about the **receiver's type**, and this file has
// none: `kasse.set(42)` names `set`, and so does a `Config::set` somebody
// wrote. So the base case is read from `check::MethodCalls::resolved`, which is
// the type checker's answer ([ADR-028](../../../../docs/specification/adr/adr-028.md))
// — the same arrangement `keeps` uses for its receivers, and for the same
// reason.
//
// **Guessing by name would make the column worthless rather than merely
// coarse.** `set` and `get` are among the most common method names a program
// writes; a column that answered *touches a lock* for every one of them would
// be set almost everywhere, and a refusal reading it would refuse correct
// programs — which is the one thing Part III C.4 forbids.
//
// ## Fail-closed, and what that costs here
//
// An **unresolvable** call sets the property, which is D3's own sentence: it
// takes the `sync` claim away *and* sets this, so one polarity decision serves
// both. That is the safe direction for a refusal about deadlock — the wrong
// answer the other way is a program that hangs.
//
// **It is also why nothing reads this column yet.** The measurement comes
// first: a property that lands on most of the corpus is one whose refusals
// would be noise, and that is a thing to find out before `NK2201` and `NK2203`
// are written against it, not after.
//
// ## A scope's tasks count and a `spawn` does not
//
// A scope waits for its tasks, so they run **during** the call and their bodies
// belong to the surrounding function — the same reason a trailing lambda's body
// does ([ADR-029](../../../../docs/specification/adr/adr-029.md) D4). A task
// started with `spawn` runs later and elsewhere, so taking a lock in one is the
// ordinary case and its body is not walked.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Block, Expr, Item};
use crate::parser::Parsed;

use super::{Ledger, Lock};

/// The `std` entries that open a lock, by their ledger keys.
///
/// A closed list, like `touch::KINDS` and `send`'s table and for the same
/// reason: a name this file does not know is answered *no*, and a name it knew
/// wrongly would be a claim about a program nobody made.
const DOORS: &[&str] = &[
    "Locked::get",
    "Locked::set",
    // The witness door ([ADR-111](../../../../docs/specification/adr/adr-111.md)
    // D5), under the key it is written with. It opens a lock exactly as `set`
    // does — a program that reaches it inside another lock is `NK2203` for the
    // same reason.
    "Locked::set(after)",
    "Locked::access",
    "Locked::update",
    "SharedMut::get",
    "SharedMut::set",
    "SharedMut::set(after)",
    "SharedMut::access",
    "SharedMut::update",
];

/// The doors over **several** locks, which are free calls rather than methods
/// ([ADR-065](../../../../docs/specification/adr/adr-065.md)) and so are named
/// here rather than found among the resolved receivers.
const MULTI: &[&str] = &["access_all", "update_all"];

/// The prefix a function field's node is keyed by in the graph: no function's
/// key starts with it.
const FIELD: &str = "field:";

/// The free calls a lambda's body makes, for the checker to remember with the
/// function field it is stored in (ADR-230 D1).
pub fn free_calls(parsed: &Parsed, body: &Block) -> BTreeSet<String> {
    let mut reaches = Reaches::default();
    walk(parsed, body, &mut reaches);
    reaches.callees
}

/// What one body reaches, before the fixpoint.
#[derive(Clone, Debug)]
struct Reaches {
    /// What the body says on its own: a door it opens, or the doubt an
    /// unresolvable call leaves.
    itself: Lock,
    /// The functions it calls, by the key the ledger records them under.
    callees: BTreeSet<String>,
}

/// Written out because [`Lock`] is Nikaia's, which names its value rather
/// than deriving a `Default` (ADR-252 D4.4).
impl Default for Reaches {
    fn default() -> Self {
        Reaches {
            itself: Lock::No,
            callees: Default::default(),
        }
    }
}

/// Give every function of this package its `touches_a_lock`.
///
/// `resolved` is the checker's answer about method calls, keyed by the same
/// function names the ledger uses.
pub fn infer(
    ledger: &mut Ledger,
    units: &[&Parsed],
    library: &Ledger,
    resolved: &BTreeMap<String, crate::check::MethodCalls>,
    stored: &BTreeMap<String, crate::check::StoredCode>,
) {
    let mut graph: BTreeMap<String, Reaches> = BTreeMap::new();

    // **Code stored in a function field is a node of its own**
    // ([ADR-230](../../../../docs/specification/adr/adr-230.md) D1), keyed
    // `field:Type.field`: what every lambda and function put there reaches, and
    // a call through the field reaches it.
    for (field, code) in stored {
        let mut reaches = Reaches::default();
        if code.unresolved {
            reaches.itself = reaches.itself.or(Lock::Undecided);
        }
        if code.resolved.iter().any(|to| DOORS.contains(&to.as_str()))
            || code.free.iter().any(|to| MULTI.contains(&to.as_str()))
        {
            reaches.itself = reaches.itself.or(Lock::Holds);
        }
        reaches.callees.extend(code.resolved.iter().cloned());
        reaches.callees.extend(code.free.iter().cloned());
        graph.insert(format!("{FIELD}{field}"), reaches);
    }

    for parsed in units.iter().copied() {
        for item in &parsed.program.items {
            match &item.node {
                Item::Fn { .. } => {
                    if let Some((name, reaches)) = reaches_of(parsed, &item.node, None, resolved) {
                        graph.insert(name, reaches);
                    }
                }
                Item::Impl {
                    target, methods, ..
                } => {
                    let target = parsed.text(target.name).to_string();
                    for method in methods {
                        if let Some((name, reaches)) =
                            reaches_of(parsed, &method.node, Some(&target), resolved)
                        {
                            graph.insert(name, reaches);
                        }
                    }
                }
                // **A `pub` rule is an entry, so it gets the walk too**
                // ([ADR-082](../../../docs/specification/adr/adr-082.md) D1,
                // [ADR-186](../../../docs/specification/adr/adr-186.md)). Its
                // body is every **action block**
                // in the grammar, for the reason `touch::infer` gives one file
                // over: a rule's pattern names other rules of the same grammar
                // and their actions run with it, so the grammar is the unit —
                // which is the safe direction here, because a lock this walk
                // does not see is a deadlock the checker does not refuse.
                Item::Grammar(def) => {
                    let named = parsed.text(def.name).to_string();
                    let mut whole = Reaches::default();
                    for rule in &def.rules {
                        for alt in &rule.alts {
                            if let Some(action) = &alt.action {
                                walk(parsed, action, &mut whole);
                            }
                        }
                    }
                    for rule in def.rules.iter().filter(|r| r.is_public) {
                        let key = format!("{named}::{}", parsed.text(rule.name));
                        let mut reaches = whole.clone();
                        if let Some(calls) = resolved.get(&key) {
                            let outside = |name: &String| !calls.in_a_task.resolved.contains(name);
                            if calls.unresolved && !calls.in_a_task.unresolved {
                                reaches.itself = reaches.itself.or(Lock::Undecided);
                            }
                            if calls
                                .resolved
                                .iter()
                                .filter(|to| outside(to))
                                .any(|to| DOORS.contains(&to.as_str()))
                            {
                                reaches.itself = reaches.itself.or(Lock::Holds);
                            }
                            reaches
                                .callees
                                .extend(calls.resolved.iter().filter(|to| outside(to)).cloned());
                        }
                        graph.insert(key, reaches);
                    }
                }
                _ => {}
            }
        }
    }

    // **The fixpoint is Nikaia** (`tools/locks.nika`, #125): it gives the
    // property to whoever reaches a holder until nothing changes, and answers
    // each function's `touches_a_lock` and each function field's.
    let itself: BTreeMap<String, Lock> = graph
        .iter()
        .map(|(name, reaches)| (name.clone(), reaches.itself))
        .collect();
    let callees: BTreeMap<String, BTreeSet<String>> = graph
        .into_iter()
        .map(|(name, reaches)| (name, reaches.callees))
        .collect();

    // **A variant is a value, not a callee** - the rule `sync::reached` states
    // for both analyses. `Json::Array(items)` builds a value and runs no body,
    // so it opens no door; read as a callee nothing describes it made every
    // grammar that builds a tree undecided. The type is everything but the
    // last segment, and a type is one this package or a library declares.
    let declared: BTreeSet<String> = units
        .iter()
        .flat_map(|parsed| {
            parsed
                .program
                .items
                .iter()
                .filter_map(|item| match &item.node {
                    Item::Enum { name, .. } | Item::Struct { name, .. } => {
                        Some(parsed.text(*name).to_string())
                    }
                    _ => None,
                })
        })
        .chain(ledger.types.keys().cloned())
        .chain(library.types.keys().cloned())
        .collect();
    let answer = nikaia_std::tools::locks::locking(&itself, &callees, &declared, library, ledger);
    for (name, holds) in answer.functions {
        if let Some(contract) = ledger.functions.get_mut(&name) {
            contract.touches_a_lock = holds;
        }
    }
    ledger.code_locks.extend(answer.fields);
}

/// What one function's body reaches, and the key the ledger records it under.
fn reaches_of(
    parsed: &Parsed,
    item: &Item,
    target: Option<&str>,
    resolved: &BTreeMap<String, crate::check::MethodCalls>,
) -> Option<(String, Reaches)> {
    let Item::Fn { name, body, .. } = item else {
        return None;
    };
    // The same key the ledger uses, arrived at the same way - the anonymous
    // constructor of Part I 4.2 included, which a caller reaches as `Type::new`.
    let own = match name {
        Some(name) => parsed.text(*name).to_string(),
        None => "new".to_string(),
    };
    let key = match target {
        Some(target) => format!("{target}::{own}"),
        None => own,
    };

    let mut reaches = Reaches::default();
    // **The base case is the checker's answer** (ADR-028): which entry
    // `kasse.set(42)` goes to is a question about the receiver's type.
    if let Some(calls) = resolved.get(&key) {
        // **What a `spawn` body did is not what this body did** (D3): it runs
        // later and elsewhere, so a lock taken in one is the ordinary case. The
        // checker records which side of the `spawn` each call was on, because
        // only it knows - `resolved` is keyed by function and a task's body is
        // written inside one.
        let outside = |name: &String| !calls.in_a_task.resolved.contains(name);
        if calls.unresolved && !calls.in_a_task.unresolved {
            reaches.itself = reaches.itself.or(Lock::Undecided);
        }
        if calls
            .resolved
            .iter()
            .filter(|to| outside(to))
            .any(|to| DOORS.contains(&to.as_str()))
        {
            reaches.itself = reaches.itself.or(Lock::Holds);
        }
        reaches
            .callees
            .extend(calls.resolved.iter().filter(|to| outside(to)).cloned());
        reaches.callees.extend(
            calls
                .fields_called
                .iter()
                .map(|field| format!("{FIELD}{field}")),
        );
    }
    walk(parsed, body, &mut reaches);
    Some((key, reaches))
}

/// Every free call a body makes, the blocks it holds included.
fn walk(parsed: &Parsed, block: &Block, reaches: &mut Reaches) {
    for stmt in &block.stmts {
        super::sync::visit_stmt(parsed, &stmt.node, &mut |expr| {
            // A `spawn`'s body runs later and elsewhere, so a lock taken in one
            // is the ordinary case (D3). `visit_stmt` hands the `spawn` itself
            // here and `visit_stmt_blocks` does not descend into it, so there
            // is nothing to exclude - this arm exists to say so.
            if matches!(expr, Expr::Spawn { .. }) {
                return;
            }
            if let Some(name) = free_call(parsed, expr) {
                // **A door over several locks is a free call** (ADR-065), so it
                // is named here where every other free call is.
                if MULTI.contains(&name.as_str()) {
                    reaches.itself = reaches.itself.or(Lock::Holds);
                }
                reaches.callees.insert(name);
            }
        });
        let mut blocks: Vec<&Block> = Vec::new();
        super::sync::visit_stmt_blocks(&stmt.node, &mut |inner| blocks.push(inner));
        for inner in blocks {
            walk(parsed, inner, reaches);
        }
    }
}

/// The name a free call names, unaliased, or nothing where this is not one.
fn free_call(parsed: &Parsed, expr: &Expr) -> Option<String> {
    let Expr::Call { func, .. } = expr else {
        return None;
    };
    let name = match func.as_ref() {
        Expr::Variable(name) => parsed.text(*name).to_string(),
        Expr::Path(segments) => segments
            .iter()
            .map(|s| parsed.text(*s))
            .collect::<Vec<_>>()
            .join("::"),
        _ => return None,
    };
    Some(parsed.unaliased(&name))
}
