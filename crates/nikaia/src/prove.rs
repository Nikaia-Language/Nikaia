// crates/nikaia/src/prove.rs
//
// Every `assert` outside a `test` block, proved while the program is built
// ([ADR-269](../../docs/specification/adr/adr-269.md)).
//
// **What it reads** (D9): comparisons of whole numbers built from names,
// literals, `+`, `-` and a `*` by a constant, joined with `&&`, `||` and `!`.
// A name is a variable of the proof only where it is an immutable binding of a
// whole-number type; anything else makes the claim one this prover cannot
// read, and that is never a guess: the claim is checked when the program
// runs (D4).
//
// **What it knows** (D9): the branch an `if` is in, what a branch that always
// leaves has ruled out — `return … if c` is exactly that
// ([ADR-276](../../docs/specification/adr/adr-276.md)) — a `for` over a
// range, a `let`'s value, the function's preconditions (D5) and every claim
// proved before.
//
// **How** (D10) is not decided here: each claim is a query to `nikaia-logic`
// ([ADR-270](../../docs/specification/adr/adr-270.md)) - its facts and its
// goal as terms - and the reference solver there answers it. This file is the
// frontend: it knows the program, and what an answer means for it.
//
// **Soundness around bindings.** A fact names variables by their name, so a
// name bound again drops every fact that mentions it, and a block whose
// bindings this walk cannot see — a lambda's body, a `match` arm — starts with
// no facts and no variables. Losing facts only costs proofs.

use std::collections::{BTreeMap, BTreeSet};

use nikaia_logic::{Arena, Query, TermId, verify_model};
use nikaia_std::tools::prover_arena::TermArena;
use nikaia_std::tools::solver_terms::SolverTerm;

use crate::ast::{Expr, Item, Stmt};
use crate::check::Finding;
use crate::contracts::Ledger;
use crate::parser::Parsed;

// **What the prover hands the rest of the compiler** is
// `tools/prover_results.nika` (ADR-294, #436).
pub use nikaia_std::tools::prover_results::{
    CallReach as Reach, ClaimHeld as Held, PreconditionCheck as Check, Published, joined_reach,
};

/// How each call the prover saw reaches its callee, by callee and arguments
/// as written. A call it did not see is not here, and reaches the checked
/// entry.
pub type Reaches = BTreeMap<(String, String), Reach>;

/// What the prover found: its refusals, how each claim it reached is held,
/// keyed as [`crate::check::Checked::claims`] is, the functions that have a
/// checked entry, and how each call reaches them.
#[derive(Debug, Default)]
pub struct Proved {
    pub findings: Vec<Finding>,
    pub held: BTreeMap<(usize, String), Held>,
    /// By the ledger's key (`f`, `Type::m`): what the checked entry checks.
    pub entries: BTreeMap<String, Vec<Check>>,
    pub reaches: Reaches,
    /// By the ledger's key: what the ledger publishes (ADR-269 D18).
    pub published: BTreeMap<String, Published>,
}

/// How a call is named in [`Reaches`].
pub fn call_key(parsed: &Parsed, callee: &str, args: &[Expr]) -> (String, String) {
    let args: Vec<String> = args
        .iter()
        .map(|a| crate::check::written(parsed, a))
        .collect();
    (callee.to_string(), args.join(", "))
}

/// Prove every `assert` of the program. `claims` are the checker's: a call is
/// an `assert` exactly where the checker recognised the prelude's.
pub fn prove(
    parsed: &Parsed,
    own: &Ledger,
    library: &Ledger,
    claims: &BTreeSet<(usize, String)>,
) -> Proved {
    let mut walk = ProverWalk {
        state: ProverState::fresh(),
        arena: TermArena::empty(),
        held: BTreeMap::new(),
        reaches: BTreeMap::new(),
        findings: Vec::new(),
        claims: claims
            .iter()
            .map(|(at, shape)| (*at as i64, shape.clone()))
            .collect(),
    };
    let copy = std::cell::RefCell::new(SolverCopy::default());
    // **The passes, and what they make** (`tools/prover_passes.nika`): the
    // walk repeated until the preconditions, then the postconditions, stop
    // changing (ADR-269 D15, D17); the checked entries (D20), what the ledger
    // publishes (D18), and how a claim no walk reached is held.
    let answers = prover_passes::prover_passes(
        &mut walk,
        &parsed.program,
        &parsed.interner,
        own,
        library,
        &|arena, facts, goal| answer_of(&copy, arena, facts, goal),
        &|arena, facts, goal| model_of(&copy, arena, facts, goal),
        &|name| crate::emit::escaped(name).into_owned(),
        &|e| crate::emit::literal_expressions(parsed, e),
        &|name| parsed.unaliased(name),
        &|e| crate::check::written(parsed, e),
        &|e| crate::check::argument_shape(e),
        &|text, names| condition_nodes(text, names),
    );
    Proved {
        findings: walk
            .findings
            .into_iter()
            .map(crate::traits::from_nikaia)
            .collect(),
        held: walk
            .held
            .into_iter()
            .map(|((at, shape), held)| ((at as usize, shape), held))
            .collect(),
        entries: answers.entries,
        reaches: walk.reaches,
        published: answers.published,
    }
}

// **The walk and its passes** are `tools/prover_walk.nika` and
// `tools/prover_passes.nika` (ADR-294, #436): this file hands them what only
// the compiler can answer.
use nikaia_std::tools::prover_passes;
use nikaia_std::tools::prover_solver::SolverAnswer;
use nikaia_std::tools::prover_state::ProverState;
use nikaia_std::tools::prover_walk::ProverWalk;

use nikaia_std::tools::prove_terms;

// --- A program's numbers as terms -----------------------------------------

/// **What a `pub` function's contract changed in the breaking direction**
/// since the committed ledger (ADR-269 D19), as warnings: a precondition the
/// old one does not imply - a caller that showed the old may now stop where
/// it is checked - and a postcondition the new one does not imply - a caller's
/// proof that leaned on it may no longer hold. The solver decides each
/// direction; where it cannot show the implication, the change is said.
/// `mine` says which entries are this package's own.
pub fn changed_contracts(now: &Ledger, committed: &Ledger, mine: impl Fn(&str) -> bool) -> String {
    nikaia_std::tools::contract_changes::changed_contracts(
        now,
        committed,
        &|key| mine(key),
        &|from, to, names| implies(from, to, names),
    )
}

/// Whether the conditions `from` imply every one of `to`, by the reference
/// solver with a checked certificate; `false` where it cannot show it, or a
/// condition does not read back.
fn implies(from: &[String], to: &[String], names: &BTreeSet<String>) -> bool {
    if to.iter().all(|c| from.contains(c)) {
        return true;
    }
    let mut arena = TermArena::empty();
    let Some(facts) = from
        .iter()
        .map(|c| condition(&mut arena, c, names))
        .collect::<Option<Vec<i64>>>()
    else {
        return false;
    };
    let Some(goals) = to
        .iter()
        .map(|c| condition(&mut arena, c, names))
        .collect::<Option<Vec<i64>>>()
    else {
        return false;
    };
    let goal = arena.and(goals);
    let copy = std::cell::RefCell::new(SolverCopy::default());
    asked_of(&copy, &arena, &facts, goal, crate::proofs::ask) == crate::proofs::Asked::Proved
}

/// **A condition the ledger states, read back** (ADR-269 D18): the text is
/// the language's own syntax, so the compiler's parser reads it, inside an
/// `assert` of a function nobody calls, and the prover's own reading turns it
/// into a term. Only `names` are variables of it.
fn condition(arena: &mut TermArena, text: &str, names: &BTreeSet<String>) -> Option<i64> {
    let source = format!("fn __condition() {{\n    assert({text})\n}}\n");
    let parsed = crate::parser::parse_to_ast(&source).ok()?;
    let Item::Fn { body, .. } = &parsed.program.items.first()?.node else {
        return None;
    };
    let Stmt::Expr(Expr::Call { args, .. }) = &body.stmts.first()?.node else {
        return None;
    };
    prove_terms::claim_term(
        args.first()?,
        &parsed.interner,
        &|name| names.contains(name),
        &mut arena.nodes,
    )
}

/// Whether `text` reads back as a condition over `names`: what a ledger may
/// publish of it (ADR-314 D3).
pub(crate) fn reads_back(text: &str, names: &BTreeSet<String>) -> bool {
    condition(&mut TermArena::empty(), text, names).is_some()
}

/// A condition read back on its own, for the walk to adopt
/// (`TermArena::adopt`): its nodes, the root last; none where it does not
/// read back.
pub(crate) fn condition_nodes(text: &str, names: &BTreeSet<String>) -> Vec<SolverTerm> {
    let mut arena = TermArena::empty();
    match condition(&mut arena, text, names) {
        Some(root) => {
            debug_assert_eq!(root as usize + 1, arena.nodes.len());
            arena.nodes
        }
        None => Vec::new(),
    }
}

/// `nikaia-logic`'s copy of the terms, as far as it has been made.
#[derive(Debug, Clone, Default)]
pub(crate) struct SolverCopy {
    logic: Arena,
    /// The copy's `TermId` of each node of `held`, by place.
    ids: Vec<TermId>,
}

/// The solver's question whether `facts` imply `goal`, handed to `ask`,
/// once `copy` has every node of `held`.
pub(crate) fn asked_of<R>(
    copy: &std::cell::RefCell<SolverCopy>,
    held: &TermArena,
    facts: &[i64],
    goal: i64,
    ask: impl FnOnce(&Query) -> R,
) -> R {
    let mut copy = copy.borrow_mut();
    let SolverCopy { logic, ids } = &mut *copy;
    for node in &held.nodes[ids.len()..] {
        let at = |i: &i64| ids[*i as usize];
        let id = match node {
            SolverTerm::Bool(b) => logic.bool(*b),
            SolverTerm::Int(n) => logic.int(*n),
            SolverTerm::Var(name) => logic.var(name),
            SolverTerm::Add(a, b) => logic.add(at(a), at(b)),
            SolverTerm::Sub(a, b) => logic.sub(at(a), at(b)),
            SolverTerm::Neg(a) => logic.neg(at(a)),
            SolverTerm::Mul(a, b) => logic.mul(at(a), at(b)),
            SolverTerm::Le(a, b) => logic.le(at(a), at(b)),
            SolverTerm::Lt(a, b) => logic.lt(at(a), at(b)),
            SolverTerm::Ge(a, b) => logic.ge(at(a), at(b)),
            SolverTerm::Gt(a, b) => logic.gt(at(a), at(b)),
            SolverTerm::Eq(a, b) => logic.eq(at(a), at(b)),
            SolverTerm::Ne(a, b) => logic.ne(at(a), at(b)),
            SolverTerm::And(parts) => logic.and(parts.iter().map(at).collect()),
            SolverTerm::Or(parts) => logic.or(parts.iter().map(at).collect()),
            SolverTerm::Not(a) => logic.not(at(a)),
            SolverTerm::Other => logic.bool(false),
        };
        ids.push(id);
    }
    let id = |at: &i64| ids[*at as usize];
    let facts: Vec<TermId> = facts.iter().map(id).collect();
    ask(&Query {
        arena: logic,
        facts: &facts,
        goal: id(&goal),
    })
}

/// What the solver says to `facts` and `goal`, as `prover_solver` reads it.
fn answer_of(
    copy: &std::cell::RefCell<SolverCopy>,
    held: &TermArena,
    facts: &[i64],
    goal: i64,
) -> SolverAnswer {
    match asked_of(copy, held, facts, goal, crate::proofs::ask) {
        crate::proofs::Asked::Proved => SolverAnswer::Proved,
        crate::proofs::Asked::Rejected(why) => SolverAnswer::Rejected(why),
        crate::proofs::Asked::Refuted(_) | crate::proofs::Asked::Unknown => SolverAnswer::NotProved,
    }
}

/// A checked model of `facts` and `goal`'s negation: the values it gives.
fn model_of(
    copy: &std::cell::RefCell<SolverCopy>,
    held: &TermArena,
    facts: &[i64],
    goal: i64,
) -> Option<BTreeMap<String, i64>> {
    asked_of(copy, held, facts, goal, refuted).map(|model| model.values)
}

/// A model of a question the solver refutes, checked by evaluating it.
fn refuted(query: &Query) -> Option<nikaia_logic::Model> {
    let crate::proofs::Asked::Refuted(model) = crate::proofs::ask(query) else {
        return None;
    };
    verify_model(query, &model).then_some(model)
}
