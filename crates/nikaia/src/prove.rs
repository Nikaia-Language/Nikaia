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
    // Pass 1: which parameter claims are preconditions (D5) - a function's
    // own `assert`s, and what its calls ask that it cannot show (D15). The
    // second depends on the callees' preconditions, so the pass repeats with
    // the last one's until nothing changes. A call inside a cycle is never
    // carried back, so every chain of carrying is at most as long as the
    // functions are many.
    let bound = parsed.program.items.len() + 2;
    let mut preconditions = BTreeMap::new();
    for _ in 0..bound {
        walk.state.known = preconditions;
        walk.state.preconditions = BTreeMap::new();
        walk.state.candidates = BTreeMap::new();
        every_body(&mut walk, &copy, parsed, own, library);
        preconditions = std::mem::take(&mut walk.state.preconditions);
        if same_preconditions(&walk.arena, &preconditions, &walk.state.known) {
            break;
        }
    }
    walk.state.known = BTreeMap::new();
    walk.state.collecting = false;
    walk.state.preconditions = preconditions;
    // **Pass 2, until nothing changes: every claim and every call**, with
    // the postconditions still standing (ADR-269 D17). Each one starts as a
    // candidate and is struck where an exit does not show it; a proof at one
    // exit may lean on another function's postcondition, so the walk repeats
    // until no candidate falls. The last walk is the answer: it ran with
    // exactly the postconditions that hold.
    walk.state.postconditions = std::mem::take(&mut walk.state.candidates);
    loop {
        walk.held.clear();
        walk.reaches.clear();
        walk.findings.clear();
        walk.state.broken.clear();
        every_body(&mut walk, &copy, parsed, own, library);
        if walk.state.broken.is_empty() {
            break;
        }
        for (function, index) in std::mem::take(&mut walk.state.broken).into_iter().rev() {
            if let Some(posts) = walk.state.postconditions.get_mut(&function) {
                posts.remove(index as usize);
            }
        }
    }

    // **Every function with a precondition has a checked entry** (ADR-269
    // D20), whatever its calls do: a caller the compiler doesn't see - a
    // function value, another package, the language below - reaches it.
    let mut out = Proved {
        findings: walk
            .findings
            .drain(..)
            .map(crate::traits::from_nikaia)
            .collect(),
        held: std::mem::take(&mut walk.held)
            .into_iter()
            .map(|((at, shape), held)| ((at as usize, shape), held))
            .collect(),
        reaches: std::mem::take(&mut walk.reaches),
        ..Proved::default()
    };
    let entries: BTreeMap<String, Vec<Check>> = walk
        .state
        .preconditions
        .iter()
        .map(|(function, pre)| {
            let checks = pre
                .claims
                .iter()
                .map(|claim| {
                    let mut read = BTreeSet::new();
                    walk.arena.variables(claim.term, &mut read);
                    let operands = read
                        .into_iter()
                        .map(|name| {
                            let value = rust_of_name(&name);
                            (name, value)
                        })
                        .collect();
                    Check {
                        rust: rust_of(&walk.arena, claim.term),
                        written: claim.failure(function),
                        message: claim.message.clone(),
                        operands,
                    }
                })
                .collect();
            (function.clone(), checks)
        })
        .collect();
    out.entries = entries;
    // **What the ledger publishes** (ADR-269 D18): every precondition and
    // every postcondition still standing, and the `assert` each came from.
    let mut published: BTreeMap<String, Published> = BTreeMap::new();
    for (function, pre) in &walk.state.preconditions {
        let entry = published
            .entry(function.clone())
            .or_insert_with(Published::nothing);
        for claim in &pre.claims {
            entry.requires.push(ledger_text(&walk.arena, claim.term));
            entry.from.push(format!("assert({})", claim.written));
        }
    }
    for (function, posts) in &walk.state.postconditions {
        if posts.is_empty() {
            continue;
        }
        let entry = published
            .entry(function.clone())
            .or_insert_with(Published::nothing);
        for post in posts {
            entry.ensures.push(ledger_text(&walk.arena, post.term));
        }
    }
    for (function, entry) in published.iter_mut() {
        if let Some(posts) = walk.state.postconditions.get(function) {
            entry
                .from
                .extend(posts.iter().map(|p| format!("assert({})", p.written)));
        }
    }
    out.published = published;
    let mut checked: BTreeMap<String, i64> = BTreeMap::new();
    for ((callee, _), reach) in &out.reaches {
        if matches!(reach, Reach::Checked(_)) {
            *checked.entry(callee.clone()).or_insert(0) += 1;
        }
    }
    for held in out.held.values_mut() {
        if let Held::Precondition(function, calls) = held {
            *calls = checked.get(function).copied().unwrap_or(0);
        }
    }

    // An `assert` this walk did not reach is checked where it stands.
    let tests: Vec<std::ops::Range<usize>> = parsed
        .program
        .items
        .iter()
        .filter(|item| matches!(item.node, Item::Test { .. } | Item::Bench { .. }))
        .map(|item| item.span.bytes())
        .collect();
    for key in claims {
        if out.held.contains_key(key) {
            continue;
        }
        if tests.iter().any(|range| range.contains(&key.0)) {
            out.held.insert(key.clone(), Held::ByTheTest);
            continue;
        }
        out.held.insert(
            key.clone(),
            Held::AtRunTime("it stands somewhere the prover doesn't look yet".to_string()),
        );
    }
    out
}

// **The walk itself** is `tools/prover_walk.nika` (ADR-294, #436): every
// body, with what only the compiler can answer handed in.
use nikaia_std::tools::prover_claims::Precondition;
use nikaia_std::tools::prover_solver::SolverAnswer;
use nikaia_std::tools::prover_state::ProverState;
use nikaia_std::tools::prover_walk::{self, ProverWalk};

/// **One walk over every body**: the solver through `copy`, the language
/// below's spelling of a name, the holes of a literal, a package's alias, an
/// expression as written and as the checker keys it, and another package's
/// condition read back.
fn every_body(
    walk: &mut ProverWalk,
    copy: &std::cell::RefCell<SolverCopy>,
    parsed: &Parsed,
    own: &Ledger,
    library: &Ledger,
) {
    prover_walk::prover_bodies(
        walk,
        &parsed.program,
        &parsed.interner,
        own,
        library,
        &|arena, facts, goal| answer_of(copy, arena, facts, goal),
        &|arena, facts, goal| model_of(copy, arena, facts, goal),
        &|name| crate::emit::escaped(name).into_owned(),
        &|e| crate::emit::literal_expressions(parsed, e),
        &|name| parsed.unaliased(name),
        &|e| crate::check::written(parsed, e),
        &|e| crate::check::argument_shape(e),
        &|text, names| condition_nodes(text, names),
    )
}

/// Whether two walks found the same preconditions, claim by claim.
fn same_preconditions(
    arena: &TermArena,
    a: &BTreeMap<String, Precondition>,
    b: &BTreeMap<String, Precondition>,
) -> bool {
    let texts = |m: &BTreeMap<String, Precondition>| -> Vec<(String, Vec<String>)> {
        m.iter()
            .map(|(f, p)| {
                let claims = p.claims.iter().map(|c| term_text(arena, c.term)).collect();
                (f.clone(), claims)
            })
            .collect()
    };
    texts(a) == texts(b)
}

// **How a term and a value are written** (ADR-269 D7, D18) is
// `tools/prove_text.nika` (ADR-294, #436): these hand it the arena one node
// at a time and the emitter's escaping of a name.

fn rust_of(arena: &TermArena, id: i64) -> String {
    prove_text::rust_of(id, &|at| arena.at(at), &|name| {
        crate::emit::escaped(name).into_owned()
    })
}

fn rust_of_name(name: &str) -> String {
    prove_text::rust_of_name(name, &|name| crate::emit::escaped(name).into_owned())
}

/// A term as a reader writes it, with `→` for the implication a branch makes
/// of a precondition.
fn term_text(arena: &TermArena, id: i64) -> String {
    prove_text::term_text(id, &|at| arena.at(at))
}

/// A term in the language's own syntax, as the ledger writes it and reads it
/// back ([ADR-251](../../docs/specification/adr/adr-251.md) D4).
fn ledger_text(arena: &TermArena, id: i64) -> String {
    prove_text::term_ledger_text(id, &|at| arena.at(at))
}

use nikaia_std::tools::prove_terms;
use nikaia_std::tools::prove_text;

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

/// A condition read back on its own, for the walk to adopt
/// (`TermArena::adopt`): its nodes, the root last; none where it does not
/// read back.
fn condition_nodes(text: &str, names: &BTreeSet<String>) -> Vec<SolverTerm> {
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
struct SolverCopy {
    logic: Arena,
    /// The copy's `TermId` of each node of `held`, by place.
    ids: Vec<TermId>,
}

/// The solver's question whether `facts` imply `goal`, handed to `ask`,
/// once `copy` has every node of `held`.
fn asked_of<R>(
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
