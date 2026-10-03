// crates/nikaia/src/contracts/locks.rs
//
// Whether a function **touches a lock** ([ADR-281](../../../../docs/specification/adr/adr-281.md)
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
// ([ADR-288](../../../../docs/specification/adr/adr-288.md) D1) and correctly
// gets nothing here, for the mirrored reason — the same shape `keeps` has
// beside it, and for the same reason: a restriction is added on doubt where a
// promise is taken away on doubt.
//
// ## What a base case is, and why the checker answers it
//
// A **door** is `get`, `set`, `access` or `update` on a `Locked` or a
// `SharedMut`, and `access_all` or `update_all` over several
// ([ADR-281](../../../../docs/specification/adr/adr-281.md) D10,
// [ADR-281](../../../../docs/specification/adr/adr-281.md)). Which of those a
// call goes to is a question about the **receiver's type**, and this file has
// none: `kasse.set(42)` names `set`, and so does a `Config::set` somebody
// wrote. So the base case is read from `check::MethodCalls::resolved`, which is
// the type checker's answer ([ADR-288](../../../../docs/specification/adr/adr-288.md))
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
// ## Where it is written
//
// **The walk, the doors and the fixpoint are Nikaia**
// (`nikaia-std/src/tools/locks.nika`, #125). What stays here is handing in
// each unit with its aliases and its literals' holes, the checker's resolved
// calls as plain data, and writing the answer into the ledger.
//
// ## A scope's tasks count and a `spawn` does not
//
// A scope waits for its tasks, so they run **during** the call and their bodies
// belong to the surrounding function — the same reason a trailing lambda's body
// does ([ADR-288](../../../../docs/specification/adr/adr-288.md) D16). A task
// started with `spawn` runs later and elsewhere, so taking a lock in one is the
// ordinary case and its body is not walked.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Block, Expr};
use crate::parser::Parsed;

use super::Ledger;
use nikaia_std::tools::locks::{self as nika, BodyCalls, Reaches};

/// The free calls a lambda's body makes, for the checker to remember with the
/// function field it is stored in (ADR-230 D1).
pub fn free_calls(parsed: &Parsed, body: &Block) -> BTreeSet<String> {
    nika::free_calls(
        body,
        &parsed.interner,
        &|name: &str| parsed.unaliased(name),
        &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
    )
}

/// Give every function of this package its `touches_a_lock`.
///
/// `resolved` is the checker's answer about method calls, keyed by the same
/// function names the ledger uses; what the walk reads of it is what was
/// resolved **outside** a `spawn` (D3) - the checker records which side each
/// call was on, because only it knows.
pub fn infer(
    ledger: &mut Ledger,
    units: &[&Parsed],
    library: &Ledger,
    resolved: &BTreeMap<String, crate::check::MethodCalls>,
    stored: &BTreeMap<String, crate::check::StoredCode>,
) {
    let calls: BTreeMap<String, BodyCalls> = resolved
        .iter()
        .map(|(key, calls)| {
            let body = BodyCalls {
                resolved: calls
                    .resolved
                    .iter()
                    .filter(|to| !calls.in_a_task.resolved.contains(*to))
                    .cloned()
                    .collect(),
                unresolved: calls.unresolved && !calls.in_a_task.unresolved,
                fields_called: calls.fields_called.clone(),
            };
            (key.clone(), body)
        })
        .collect();
    let mut graph: BTreeMap<String, Reaches> = BTreeMap::new();
    for (field, code) in stored {
        nika::stored_reaches(
            field,
            &code.resolved,
            code.unresolved,
            &code.free,
            &mut graph,
        );
    }
    let mut declared: BTreeSet<String> = BTreeSet::new();
    for parsed in units.iter().copied() {
        nika::unit_reaches(
            &parsed.program,
            &parsed.interner,
            &|name: &str| parsed.unaliased(name),
            &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
            &calls,
            &mut graph,
        );
        nika::declared_types(&parsed.program, &parsed.interner, &mut declared);
    }
    declared.extend(ledger.types.keys().cloned());
    declared.extend(library.types.keys().cloned());
    let answer = nika::locking_of(&graph, &declared, library, ledger);
    for (name, holds) in answer.functions {
        if let Some(contract) = ledger.functions.get_mut(&name) {
            contract.touches_a_lock = holds;
        }
    }
    ledger.code_locks.extend(answer.fields);
}
