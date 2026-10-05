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

use crate::contracts::LedgerOps;
use std::collections::{BTreeMap, BTreeSet};

use nikaia_logic::{Arena, Query, TermId, verify_model};
use nikaia_std::tools::prover_arena::TermArena;
use nikaia_std::tools::solver_terms::SolverTerm;

use crate::ast::{Block, Expr, Item, Span, Spanned, Stmt};
use crate::check::{Finding, Severity};
use crate::contracts::{Ledger, Provenance};
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
    let mut prover = Prover {
        parsed,
        own,
        library,
        claims,
        state: ProverState::fresh(),
        out: Proved::default(),
        arena: Terms::default(),
    };
    // Pass 1: which parameter claims are preconditions (D5) - a function's
    // own `assert`s, and what its calls ask that it cannot show (D15). The
    // second depends on the callees' preconditions, so the pass repeats with
    // the last one's until nothing changes. A call inside a cycle is never
    // carried back, so every chain of carrying is at most as long as the
    // functions are many.
    let bound = parsed.program.items.len() + 2;
    let mut preconditions = BTreeMap::new();
    for _ in 0..bound {
        prover.state.known = preconditions;
        prover.state.preconditions = BTreeMap::new();
        prover.state.candidates = BTreeMap::new();
        prover.every_body();
        preconditions = std::mem::take(&mut prover.state.preconditions);
        if same_preconditions(&prover.arena, &preconditions, &prover.state.known) {
            break;
        }
    }
    prover.state.known = BTreeMap::new();
    prover.state.collecting = false;
    prover.state.preconditions = preconditions;
    // **Pass 2, until nothing changes: every claim and every call**, with
    // the postconditions still standing (ADR-269 D17). Each one starts as a
    // candidate and is struck where an exit does not show it; a proof at one
    // exit may lean on another function's postcondition, so the walk repeats
    // until no candidate falls. The last walk is the answer: it ran with
    // exactly the postconditions that hold.
    prover.state.postconditions = std::mem::take(&mut prover.state.candidates);
    loop {
        prover.out = Proved::default();
        prover.state.broken.clear();
        prover.every_body();
        if prover.state.broken.is_empty() {
            break;
        }
        for (function, index) in std::mem::take(&mut prover.state.broken).into_iter().rev() {
            if let Some(posts) = prover.state.postconditions.get_mut(&function) {
                posts.remove(index as usize);
            }
        }
    }

    // **Every function with a precondition has a checked entry** (ADR-269
    // D20), whatever its calls do: a caller the compiler doesn't see - a
    // function value, another package, the language below - reaches it.
    let entries: BTreeMap<String, Vec<Check>> = prover
        .state
        .preconditions
        .iter()
        .map(|(function, pre)| {
            let checks = pre
                .claims
                .iter()
                .map(|claim| {
                    let mut read = BTreeSet::new();
                    prover.arena.variables(claim.term, &mut read);
                    let operands = read
                        .into_iter()
                        .map(|name| {
                            let value = rust_of_name(&name);
                            (name, value)
                        })
                        .collect();
                    Check {
                        rust: rust_of(&prover.arena, claim.term),
                        written: claim.failure(function),
                        message: claim.message.clone(),
                        operands,
                    }
                })
                .collect();
            (function.clone(), checks)
        })
        .collect();
    prover.out.entries = entries;
    // **What the ledger publishes** (ADR-269 D18): every precondition and
    // every postcondition still standing, and the `assert` each came from.
    let mut published: BTreeMap<String, Published> = BTreeMap::new();
    for (function, pre) in &prover.state.preconditions {
        let entry = published
            .entry(function.clone())
            .or_insert_with(Published::nothing);
        for claim in &pre.claims {
            entry.requires.push(ledger_text(&prover.arena, claim.term));
            entry.from.push(format!("assert({})", claim.written));
        }
    }
    for (function, posts) in &prover.state.postconditions {
        if posts.is_empty() {
            continue;
        }
        let entry = published
            .entry(function.clone())
            .or_insert_with(Published::nothing);
        for post in posts {
            entry.ensures.push(ledger_text(&prover.arena, post.term));
        }
    }
    for (function, entry) in published.iter_mut() {
        if let Some(posts) = prover.state.postconditions.get(function) {
            entry
                .from
                .extend(posts.iter().map(|p| format!("assert({})", p.written)));
        }
    }
    prover.out.published = published;
    let mut checked: BTreeMap<String, i64> = BTreeMap::new();
    for ((callee, _), reach) in &prover.out.reaches {
        if matches!(reach, Reach::Checked(_)) {
            *checked.entry(callee.clone()).or_insert(0) += 1;
        }
    }
    for held in prover.out.held.values_mut() {
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
        if prover.out.held.contains_key(key) {
            continue;
        }
        if tests.iter().any(|range| range.contains(&key.0)) {
            prover.out.held.insert(key.clone(), Held::ByTheTest);
            continue;
        }
        prover.out.held.insert(
            key.clone(),
            Held::AtRunTime("it stands somewhere the prover doesn't look yet".to_string()),
        );
    }
    prover.out
}

// **The claims carried between functions** are `tools/prover_claims.nika`
// (ADR-294, #436).
use nikaia_std::tools::prover_claims::{
    self, CarriedFrom, Foreign, PostClaim, PreClaim, Precondition,
};
use nikaia_std::tools::prover_solver::{self, SolverAnswer};
use nikaia_std::tools::prover_state::{self, ProverState};

struct Prover<'a> {
    parsed: &'a Parsed,
    /// The program's ledger: another package's functions, with their
    /// `requires` and `ensures` (ADR-269 D18).
    own: &'a Ledger,
    library: &'a Ledger,
    claims: &'a BTreeSet<(usize, String)>,
    /// What the walk keeps across functions and passes
    /// (`tools/prover_state.nika`).
    state: ProverState,
    out: Proved,
    /// Every term the walk builds, facts and claims alike (ADR-270 D2).
    arena: Terms,
}

/// What a body may know at one point (`tools/prover_scope.nika`, #436).
use nikaia_std::tools::prover_scope::{ProverScope as Scope, length_of};

/// Where a function body is, for what an `assert` there may become.
#[derive(Clone)]
struct Where {
    /// The free function this body is, where a precondition may belong to it.
    function: Option<String>,
    /// Its whole-number parameters.
    params: BTreeSet<String>,
    /// Why a parameter claim here cannot be a precondition, if it cannot.
    no_precondition: Option<&'static str>,
    /// The function body's own statements, not a nested block.
    top: bool,
    /// A free function's body, called by name: what has a postcondition.
    free: bool,
    /// Inside a lambda, whose `return` is not the function's.
    lambda: bool,
    /// The function `throws`, so a call in it may leave it.
    throws: bool,
}

impl<'a> Prover<'a> {
    fn every_body(&mut self) {
        let parsed = self.parsed;
        for item in &parsed.program.items {
            match &item.node {
                Item::Fn { .. } => self.function(item, None),
                Item::Impl {
                    methods,
                    target,
                    trait_name,
                } => {
                    let owner = self.parsed.text(target.name).to_string();
                    for method in methods {
                        self.function(method, Some((&owner, trait_name.is_some())));
                    }
                }
                Item::Test { body, .. } | Item::Bench { body, .. } => {
                    self.state.in_test = true;
                    let at = Where {
                        function: None,
                        params: BTreeSet::new(),
                        no_precondition: Some("a test has no callers"),
                        top: true,
                        free: false,
                        lambda: false,
                        throws: true,
                    };
                    self.block(body, &mut Scope::unknown(), &at);
                    self.state.in_test = false;
                }
                _ => {}
            }
        }
    }

    /// One function, or a method of `owner` - in a trait's implementation
    /// where the flag says so.
    fn function(&mut self, item: &Spanned<Item>, owner: Option<(&str, bool)>) {
        let Item::Fn {
            name,
            receiver,
            args,
            body,
            is_public,
            can_throw,
            ..
        } = &item.node
        else {
            return;
        };
        // The ledger's key: `f`, or `Type::m` for a method.
        let own = name.map(|n| {
            let name = self.parsed.text(n);
            match owner {
                Some((target, _)) => format!("{target}::{name}"),
                None => name.to_string(),
            }
        });
        // A `test` block `nikaia test` has already turned into a function is
        // still a test (D11).
        let was_a_test = own
            .as_deref()
            .is_some_and(crate::modules::is_a_test_function);
        let mut scope = Scope::at_the_entry();
        let mut params = BTreeSet::new();
        if receiver.is_some() {
            scope.locals.insert("self".to_string());
        }
        for arg in args {
            let arg_name = self.parsed.text(arg.name).to_string();
            scope.locals.insert(arg_name.clone());
            let ty = self.parsed.text(arg.ty.name);
            if !arg.mutable && arg.ty.generics.is_empty() && is_whole_number(ty) {
                scope.ints.insert(arg_name.clone());
                scope.loose.insert(arg_name.clone());
                params.insert(arg_name.clone());
                let at_entry = self.arena.var(&arg_name);
                scope.entry.insert(arg_name.clone(), at_entry);
                if ty.starts_with('u') {
                    scope.not_negative(&mut self.arena.held, &arg_name);
                }
            }
            if !arg.mutable && has_a_length(ty) {
                scope.has_length(&mut self.arena.held, &arg_name, None);
                params.insert(arg_name.clone());
                let length = length_of(&arg_name);
                let at_entry = self.arena.var(&length);
                scope.entry.insert(length, at_entry);
            }
        }
        let _ = (receiver, is_public);
        let no_precondition = if owner.is_some_and(|(_, of_a_trait)| of_a_trait) {
            Some(
                "A method of a trait's implementation can't have a precondition: a call \
                 through the trait doesn't know it, and there is no second entry to check it.",
            )
        } else if own.as_deref() == Some("main") {
            Some("`main` has no callers.")
        } else {
            None
        };
        let free = owner.is_none() && !was_a_test;
        let at = Where {
            function: own.clone(),
            params,
            no_precondition,
            top: true,
            free,
            lambda: false,
            throws: *can_throw,
        };
        self.state.in_test = was_a_test;
        let leaves = self.block(body, &mut scope, &at);
        self.state.in_test = false;
        // A body that can end without a `return` has an exit no candidate was
        // shown at.
        if !leaves
            && !self.state.collecting
            && let Some(own) = own
        {
            for index in 0..self.state.postconditions.get(&own).map_or(0, Vec::len) {
                self.state.broken.insert((own.clone(), index as i64));
            }
        }
    }

    /// Walk a block; whether it always leaves.
    fn block(&mut self, block: &Block, scope: &mut Scope, at: &Where) -> bool {
        for (index, stmt) in block.stmts.iter().enumerate() {
            if self.state.collecting
                && let Some(next) = block.stmts.get(index + 1)
                && let Stmt::Return(Some(Expr::Variable(name))) = &next.node
            {
                self.state
                    .before_return
                    .insert(stmt.span.at() as i64, self.parsed.text(*name).to_string());
            }
            if self.stmt(stmt, scope, at) {
                return true;
            }
            // A statement that may leave for some states and not for others
            // narrows what goes on past it, and only an `if` the prover reads
            // says how (`expr_stmt`).
            if !matches!(stmt.node, Stmt::Expr(Expr::If { .. }))
                && leaves_stmt(self.parsed, &stmt.node, &|e| self.throwing(e, at), true)
            {
                scope.exact = false;
            }
        }
        false
    }

    /// Walk one statement; whether control never goes past it.
    fn stmt(&mut self, stmt: &Spanned<Stmt>, scope: &mut Scope, at: &Where) -> bool {
        let span = stmt.span;
        let nested = Where {
            top: false,
            ..at.clone()
        };
        match &stmt.node {
            Stmt::Let {
                names,
                mutable,
                ty,
                value,
            } => {
                self.expr(value, span, scope, &nested);
                let tainted = self.tainted(value, scope);
                let value_lin = lin(&mut self.arena, self.parsed, value, scope);
                let value_at_entry = value_lin.and_then(|v| self.at_entry(v, scope));
                // **A call's postconditions are facts about its result**
                // (ADR-269 D17): `let r = f(x)` knows what `f` ensures.
                let called = match (names.as_slice(), value) {
                    ([only], Expr::Call { func, args, .. }) if !mutable => {
                        let callee = match &**func {
                            Expr::Variable(callee)
                                if !scope.locals.contains(self.parsed.text(*callee)) =>
                            {
                                Some(self.parsed.text(*callee).to_string())
                            }
                            Expr::Path(_) => qualified(self.parsed, func),
                            _ => None,
                        };
                        callee.map(|callee| {
                            let bound = self.parsed.text(*only).to_string();
                            let facts = self.postconditions_of(&callee, args, &bound, scope);
                            (self.returns_whole(&callee), facts)
                        })
                    }
                    _ => None,
                };
                for name in names {
                    let name = self.parsed.text(*name).to_string();
                    scope.rebind(&self.arena.held, &name);
                    if tainted {
                        scope.tainted.insert(name.clone());
                        scope.loose.insert(name.clone());
                    }
                }
                if let [only] = names.as_slice()
                    && !mutable
                {
                    let name = self.parsed.text(*only).to_string();
                    let typed_whole = ty.as_ref().is_some_and(|t| {
                        t.generics.is_empty() && is_whole_number(self.parsed.text(t.name))
                    });
                    if let Some(value_lin) = value_lin {
                        scope.ints.insert(name.clone());
                        // **A value that reads the name it shadows** -
                        // `let x = x + 1` - is about the old binding, which a
                        // fact names the same way: `x = x + 1` would be a
                        // contradiction, and from it everything follows. The
                        // name is a whole number; nothing more is known.
                        if !self.arena.mentions(value_lin, &name) {
                            let named = self.arena.var(&name);
                            scope.facts.push(self.arena.eq(named, value_lin));
                        } else {
                            scope.loose.insert(name.clone());
                        }
                        // At the entry the value is what it read there, so
                        // a shadowing `let` is no trouble here.
                        if let Some(at_entry) = value_at_entry {
                            scope.entry.insert(name.clone(), at_entry);
                        }
                    } else if typed_whole || called.as_ref().is_some_and(|(whole, _)| *whole) {
                        scope.ints.insert(name.clone());
                        scope.loose.insert(name.clone());
                    }
                    if let Some((true, facts)) = &called {
                        scope.facts.extend(facts.iter().copied());
                    }
                    if let Some(t) = ty
                        && self.parsed.text(t.name).starts_with('u')
                        && scope.ints.contains(&name)
                    {
                        scope.not_negative(&mut self.arena.held, &name);
                    }
                    match value {
                        Expr::ListLit { items, .. } => {
                            scope.has_length(
                                &mut self.arena.held,
                                &name,
                                i64::try_from(items.len()).ok(),
                            );
                        }
                        Expr::LitStr { .. } => scope.has_length(&mut self.arena.held, &name, None),
                        _ if ty
                            .as_ref()
                            .is_some_and(|t| has_a_length(self.parsed.text(t.name))) =>
                        {
                            scope.has_length(&mut self.arena.held, &name, None)
                        }
                        _ => {}
                    }
                }
                false
            }
            Stmt::Comptime { name, value, .. } => {
                self.expr(value, span, scope, &nested);
                scope.rebind(&self.arena.held, self.parsed.text(*name));
                false
            }
            Stmt::Assign { target, value, .. } => {
                self.expr(target, span, scope, &nested);
                self.expr(value, span, scope, &nested);
                if let Expr::Variable(name) = target {
                    let name = self.parsed.text(*name).to_string();
                    let tainted = self.tainted(value, scope);
                    scope.rebind(&self.arena.held, &name);
                    if tainted {
                        scope.tainted.insert(name);
                    }
                }
                false
            }
            Stmt::For {
                bindings,
                iter,
                body,
            } => {
                self.expr(iter, span, scope, &nested);
                let tainted = self.tainted(iter, scope);
                let mut inner = scope.clone();
                for binding in bindings {
                    let name = self.parsed.text(*binding).to_string();
                    inner.rebind(&self.arena.held, &name);
                    if tainted {
                        inner.tainted.insert(name);
                    }
                }
                if let (
                    [only],
                    Expr::Range {
                        start,
                        end,
                        inclusive,
                    },
                ) = (bindings.as_slice(), iter)
                    && let (Some(low), Some(high)) = (
                        lin(&mut self.arena, self.parsed, start, scope),
                        lin(&mut self.arena, self.parsed, end, scope),
                    )
                {
                    let name = self.parsed.text(*only).to_string();
                    inner.ints.insert(name.clone());
                    // low <= n, and n <= high (inclusive) or n < high - unless
                    // a bound reads the name it shadows, as `let` above.
                    if !self.arena.mentions(low, &name) && !self.arena.mentions(high, &name) {
                        let n = self.arena.var(&name);
                        inner.facts.push(self.arena.le(low, n));
                        inner.facts.push(if *inclusive {
                            self.arena.le(n, high)
                        } else {
                            self.arena.lt(n, high)
                        });
                    } else {
                        inner.loose.insert(name.clone());
                    }
                    // **Every value of the range reaches the body** only where
                    // no turn can end the loop early: after a `break` the
                    // values left are never reached.
                    if leaves_block(self.parsed, body, &|e| self.throwing(e, at), false) {
                        inner.loose.insert(name);
                    }
                } else {
                    for binding in bindings {
                        inner.loose.insert(self.parsed.text(*binding).to_string());
                    }
                }
                // A claim in a loop, or after one, is not carried back to
                // the entry (ADR-269 D16): that needs an invariant.
                inner.path = None;
                self.block(body, &mut inner, &nested);
                scope.path = None;
                if leaves_block(self.parsed, body, &|e| self.throwing(e, at), false) {
                    scope.exact = false;
                }
                false
            }
            Stmt::While { cond, body } => {
                self.expr(cond, span, scope, &nested);
                let mut inner = scope.clone();
                inner.path = None;
                // Which turns there are depends on what the loop changes.
                inner.exact = false;
                scope.path = None;
                if let Some(holds) = claim(&mut self.arena, self.parsed, cond, &inner) {
                    inner.facts.push(holds);
                }
                self.block(body, &mut inner, &nested);
                if leaves_block(self.parsed, body, &|e| self.throwing(e, at), false) {
                    scope.exact = false;
                }
                false
            }
            Stmt::Return(value) => {
                if let Some(value) = value {
                    self.expr(value, span, scope, &nested);
                    self.exit(value, scope, at);
                }
                true
            }
            Stmt::Break | Stmt::Continue => true,
            Stmt::Expr(expr) => self.expr_stmt(expr, span, scope, at),
        }
    }

    /// An expression standing as a statement: an `assert`, an `if` whose
    /// branches teach the rest of the block something, or anything else.
    fn expr_stmt(&mut self, expr: &Expr, span: Span, scope: &mut Scope, at: &Where) -> bool {
        let nested = Where {
            top: false,
            ..at.clone()
        };
        match expr {
            Expr::Call { func, args, config }
                if matches!(&**func, Expr::Variable(_))
                    && args.len() == 1
                    && self
                        .claims
                        .contains(&(span.at(), crate::check::argument_shape(&args[0]))) =>
            {
                self.expr(&args[0], span, scope, &nested);
                // The `message:` a failure says, where it is text as written:
                // a precondition checked elsewhere says it too.
                let message = config.iter().find_map(|c| match &c.value {
                    Expr::LitStr { text, .. } if self.parsed.text(c.name) == "message" => {
                        Some(text.clone())
                    }
                    _ => None,
                });
                self.an_assert(&args[0], message, span, scope, at);
                false
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(cond, span, scope, &nested);
                let holds = claim(&mut self.arena, self.parsed, cond, scope);
                let fails = holds.map(|h| self.arena.not(h));
                let mut then_scope = scope.clone();
                if let Some(holds) = &holds {
                    then_scope.facts.push(*holds);
                }
                // A branch on a condition the prover cannot read is reached
                // by some states and not others, and no fact says which.
                then_scope.exact &= holds.is_some();
                let cond_leaves = leaves_expr(self.parsed, cond, &|e| self.throwing(e, at), true);
                self.on_the_path(&mut then_scope, holds);
                let then_leaves = self.block(then_branch, &mut then_scope, &nested);
                let else_leaves = match else_branch {
                    Some(block) => {
                        let mut else_scope = scope.clone();
                        if let Some(fails) = &fails {
                            else_scope.facts.push(*fails);
                        }
                        else_scope.exact &= fails.is_some();
                        self.on_the_path(&mut else_scope, fails);
                        self.block(block, &mut else_scope, &nested)
                    }
                    None => false,
                };
                // **What a branch that always leaves rules out** holds after
                // it: `return 250 if speed > 250` leaves `speed <= 250`.
                match (then_leaves, else_leaves) {
                    (true, true) => return true,
                    (true, false) => {
                        if let Some(fails) = fails {
                            scope.facts.push(fails);
                        }
                        self.on_the_path(scope, fails);
                        scope.exact &= fails.is_some()
                            && !else_branch.as_ref().is_some_and(|b| {
                                leaves_block(self.parsed, b, &|e| self.throwing(e, at), true)
                            });
                    }
                    (false, true) => {
                        if let Some(holds) = holds {
                            scope.facts.push(holds);
                        }
                        self.on_the_path(scope, holds);
                        scope.exact &= holds.is_some()
                            && !leaves_block(
                                self.parsed,
                                then_branch,
                                &|e| self.throwing(e, at),
                                true,
                            );
                    }
                    (false, false) => {
                        if leaves_block(self.parsed, then_branch, &|e| self.throwing(e, at), true)
                            || else_branch.as_ref().is_some_and(|b| {
                                leaves_block(self.parsed, b, &|e| self.throwing(e, at), true)
                            })
                        {
                            scope.exact = false;
                        }
                    }
                }
                if cond_leaves {
                    scope.exact = false;
                }
                false
            }
            Expr::Throw(value) => {
                self.expr(value, span, scope, &nested);
                true
            }
            Expr::Return(value) => {
                if let Some(value) = &**value {
                    self.expr(value, span, scope, &nested);
                    self.exit(value, scope, at);
                }
                true
            }
            Expr::Break | Expr::Continue => true,
            other => {
                self.expr(other, span, scope, &nested);
                false
            }
        }
    }

    /// An `assert` (D4-D6, D11).
    fn an_assert(
        &mut self,
        cond: &Expr,
        message: Option<String>,
        span: Span,
        scope: &mut Scope,
        at: &Where,
    ) {
        let key = (span.at(), crate::check::argument_shape(cond));
        if self.state.in_test {
            self.out.held.insert(key, Held::ByTheTest);
            return;
        }
        let claim = claim(&mut self.arena, self.parsed, cond, scope);
        // A claim about the value the next statement returns is a candidate
        // postcondition (ADR-269 D17).
        if self.state.collecting
            && at.free
            && !at.lambda
            && let (Some(claim), Some(function)) = (claim, at.function.clone())
            && let Some(returned) = self.state.before_return.get(&(span.at() as i64)).cloned()
            && let Some(term) = self.postcondition_at(claim, &returned, scope, at)
        {
            self.state
                .candidates
                .entry(function)
                .or_default()
                .push(PostClaim {
                    term,
                    written: crate::check::written(self.parsed, cond),
                });
        }
        let mut rejected = None;
        if let Some(claim) = claim {
            match self.proves(&scope.facts, claim) {
                Ok(()) => {
                    scope.facts.push(claim);
                    self.out.held.insert(key, Held::Proved);
                    return;
                }
                Err(why) => rejected = why,
            }
        }

        // **A claim the body cannot prove is its callers'** where it can be
        // carried back to the entry (ADR-269 D15): what it says about the
        // parameters, under the branches and guards on the way. At the top of
        // the body, over parameters only, that is the claim as written
        // (ADR-269 D5).
        let names = names_in(self.parsed, cond);
        let only_params = !names.is_empty() && names.iter().all(|n| at.params.contains(n));
        // **A claim what is known rules out is not a precondition**: every
        // caller that reaches it breaks it, so it would only say *never come
        // here*. It is warned about where it stands (ADR-269 D8). Decided in
        // both passes alike, so the first pass's preconditions are the
        // second's.
        let tainted_claim = names.iter().any(|n| scope.tainted.contains(n));
        let refuted = match claim {
            Some(claim) if !tainted_claim => self.refutes(&scope.facts, claim),
            _ => None,
        };
        if let (Some(claim), None, Some(function), None) =
            (claim, at.no_precondition, at.function.clone(), &refuted)
            && let Some(term) = self.precondition_at(claim, scope, at)
        {
            if self.state.collecting {
                let as_written = only_params && at.top;
                let pre = PreClaim {
                    term,
                    computed: (!as_written).then(|| term_text(&self.arena, term)),
                    written: crate::check::written(self.parsed, cond),
                    message,
                    origin: None,
                    site: None,
                };
                self.state
                    .preconditions
                    .entry(function.clone())
                    .or_insert_with(Precondition::none)
                    .claims
                    .push(pre);
            }
            scope.facts.push(claim);
            self.out
                .held
                .insert(key, Held::Precondition(function.clone(), 0));
            return;
        }

        let tainted: Vec<&String> = names
            .iter()
            .filter(|n| scope.tainted.contains(*n))
            .collect();
        // **A claim about data from outside is a guard's job** (D6): the one
        // claim that is refused rather than checked.
        if let Some(first) = tainted.first() {
            self.out.held.insert(key, Held::Refused);
            if self.state.collecting {
                return;
            }
            let written = crate::check::written(self.parsed, cond);
            self.out.findings.push(refusal(
                span,
                format!("`{written}` is a claim about data from outside the program."),
                vec![
                    format!(
                        "`{first}` comes from outside the program, so nothing the compiler \
                         can see says what it holds."
                    ),
                    "That it is wrong is a case the program has to handle, not a defect an \
                     `assert` catches."
                        .to_string(),
                ],
                &format!(
                    "Check it where it arrives, with a guard the program handles: \
                     `throw BadInput({first}) if !({written})`, or `return … if !({written})`. \
                     After that line, this `assert` is proved."
                ),
            ));
            return;
        }

        // **Neither proved nor refused: checked where it is reached** (D4) -
        // and where what is known before it rules the claim out, the author
        // is told, with values (ADR-269 D8).
        if let Some(values) = refuted.as_ref().filter(|_| !self.state.collecting) {
            let written = crate::check::written(self.parsed, cond);
            self.out.findings.push(Finding {
                severity: Severity::Error,
                span,
                code: "NK1207",
                message: format!("`{written}` is false every time it is reached."),
                notes: vec![format!(
                    "What is known before it rules the claim out: {}.",
                    shown(values)
                )],
                help: Some(
                    "If the claim is right, the code before it is wrong; if the code is right, \
                     the claim is."
                        .to_string(),
                ),
                labels: Vec::new(),
            });
        }
        // **False for some of the values that reach it** (D8): the same
        // warning, with one such state.
        let sometimes = match (claim, &refuted) {
            (Some(claim), None) if !tainted_claim => self.breaks_when(scope, claim),
            _ => None,
        };
        if let Some(values) = sometimes.as_ref().filter(|_| !self.state.collecting) {
            let written = crate::check::written(self.parsed, cond);
            self.out.findings.push(Finding {
                severity: Severity::Error,
                span,
                code: "NK1207",
                message: format!("`{written}` is false when {}.", shown(values)),
                notes: vec![
                    "That state reaches it: what is known here pins every name the claim \
                     depends on."
                        .to_string(),
                ],
                help: Some(
                    "If the claim is right, the code before it is wrong; if the code is right, \
                     the claim is."
                        .to_string(),
                ),
                labels: Vec::new(),
            });
        }
        let why = if let Some(rejected) = rejected {
            rejected
        } else if let Some(values) = &refuted {
            format!("it is false every time it is reached ({})", shown(values))
        } else if let Some(values) = &sometimes {
            format!("it is false when {}", shown(values))
        } else if claim.is_none() {
            "it is not a comparison of whole numbers the prover reads".to_string()
        } else if only_params && let Some(no) = at.no_precondition {
            format!(
                "nothing before it shows it; {}",
                lowered_first(no.trim_end_matches('.'))
            )
        } else if only_params && !at.top {
            "nothing before it shows it, and a claim about parameters is a precondition only \
             at the top of the function's body"
                .to_string()
        } else {
            "nothing before it shows it".to_string()
        };
        self.out.held.insert(key, Held::AtRunTime(why));
        // Past a check the claim holds: the program stops where it does not.
        // Not a claim ruled out, which would make what follows vacuous.
        if let Some(claim) = claim
            && refuted.is_none()
        {
            scope.facts.push(claim);
        }
    }

    /// Walk an expression for what it calls: a call to a function with a
    /// precondition proves it here (D5), a function with one is not handed on
    /// as a value, and a nested block is walked with what it may know.
    fn expr(&mut self, expr: &Expr, span: Span, scope: &Scope, at: &Where) {
        let parsed = self.parsed;
        let mut calls: Vec<(String, Vec<Expr>)> = Vec::new();
        let mut values: Vec<String> = Vec::new();
        let mut callees: BTreeSet<*const Expr> = BTreeSet::new();
        crate::contracts::sync::visit_expr(parsed, expr, &mut |e| match e {
            Expr::Call { func, args, .. } => match &**func {
                Expr::Variable(name) => {
                    callees.insert(&**func as *const Expr);
                    calls.push((parsed.text(*name).to_string(), args.clone()));
                }
                Expr::Path(_) => {
                    if let Some(qualified) = qualified(parsed, func) {
                        calls.push((qualified, args.clone()));
                    }
                }
                _ => {}
            },
            Expr::Variable(name) if !callees.contains(&(e as *const Expr)) => {
                values.push(parsed.text(*name).to_string());
            }
            _ => {}
        });
        for (callee, args) in calls {
            if scope.locals.contains(&callee) {
                continue;
            }
            if let Some(caller) = &at.function {
                self.state.edges.insert((caller.clone(), callee.clone()));
            }
            // The first pass reads the walk before's preconditions: its own
            // are still being found.
            let pre = match self.state.collecting {
                true => self.state.known.get(&callee).cloned(),
                false => self.state.preconditions.get(&callee).cloned(),
            };
            if let Some(pre) = pre {
                let Some(Item::Fn { args: params, .. }) = self.function_named(&callee) else {
                    continue;
                };
                let params: Vec<String> = params
                    .iter()
                    .map(|p| self.parsed.text(p.name).to_string())
                    .collect();
                self.a_call(&callee, &params, &pre.claims, &args, span, scope, at);
            } else if let Some(foreign) = self.foreign_contract(&callee)
                && !foreign.requires.is_empty()
            {
                self.a_call(
                    &callee,
                    &foreign.params,
                    &foreign.requires,
                    &args,
                    span,
                    scope,
                    at,
                );
            }
        }
        // Nested blocks: their own bindings are not visible here, so they
        // start blind (the soundness note at the top).
        // A lambda's body is the exception: its own names are its parameters,
        // and what holds of the bindings around it holds inside it, because a
        // binding the prover reads never changes.
        let mut lambdas: BTreeMap<*const Block, Vec<String>> = BTreeMap::new();
        crate::contracts::sync::visit_expr(parsed, expr, &mut |e| {
            if let Expr::Closure {
                params,
                mutable,
                body,
            } = e
            {
                let names = params
                    .iter()
                    .chain(mutable)
                    .map(|p| parsed.text(*p).to_string())
                    .collect();
                lambdas.insert(body as *const Block, names);
            }
        });
        let mut blocks: Vec<&Block> = Vec::new();
        crate::contracts::sync::visit_expr_blocks(expr, &mut |b| blocks.push(b));
        for block in blocks {
            let lambda = at.lambda || lambdas.contains_key(&(block as *const Block));
            let mut inner = match lambdas.get(&(block as *const Block)) {
                Some(params) => {
                    let mut inner = scope.clone();
                    for param in params {
                        inner.rebind(&self.arena.held, param);
                        inner.loose.insert(param.clone());
                    }
                    inner.path = None;
                    // Called as often as the callee likes, or never.
                    inner.exact = false;
                    inner
                }
                None => scope.blind(),
            };
            self.block(
                block,
                &mut inner,
                &Where {
                    top: false,
                    lambda,
                    ..at.clone()
                },
            );
        }
    }

    /// A call to a function with a precondition (D5).
    #[allow(clippy::too_many_arguments)]
    fn a_call(
        &mut self,
        callee: &str,
        params: &[String],
        claims: &[PreClaim],
        args: &[Expr],
        span: Span,
        scope: &Scope,
        at: &Where,
    ) {
        let mut reach = Reach::Proved;
        for (index, pre) in claims.iter().enumerate() {
            // Each parameter's term as this call gives it, and a list's
            // length as the argument's: `p.len()` in the precondition is the
            // argument's.
            let mut with: BTreeMap<String, Option<i64>> = BTreeMap::new();
            for (param, arg) in params.iter().zip(args) {
                with.insert(param.clone(), lin(&mut self.arena, self.parsed, arg, scope));
                if let Expr::Variable(name) = arg {
                    let length = length_of(self.parsed.text(*name));
                    let known = scope
                        .ints
                        .contains(&length)
                        .then(|| self.arena.var(&length));
                    with.insert(length_of(param), known);
                }
            }
            let mut read = BTreeSet::new();
            self.arena.variables(pre.term, &mut read);
            let given: Option<BTreeMap<String, i64>> = read
                .iter()
                .map(|name| Some((name.clone(), with.get(name).copied().flatten()?)))
                .collect();
            let goal = given.map(|given| self.arena.substitute(pre.term, &given));
            let proved = goal.is_some_and(|g| self.proves(&scope.facts, g).is_ok());
            if proved {
                continue;
            }
            let site = CarriedFrom {
                at: span.at() as i64,
                callee: callee.to_string(),
                place: index as i64,
            };
            if self.state.collecting {
                self.carry_back(goal, pre, callee, site, scope, at);
                continue;
            }
            // **Carried back to this function's entry** in the first pass:
            // its callers prove it, so the call needs no check of its own.
            if let Some(function) = &at.function
                && self
                    .state
                    .preconditions
                    .get(function)
                    .is_some_and(|p| p.claims.iter().any(|c| c.carried_from(&site)))
            {
                continue;
            }
            // **A call that breaks the precondition every time** (ADR-269
            // D8): what is known at the call rules it out. The values shown
            // are the parameters', as this call gives them.
            let always = match goal {
                Some(goal) => self.refutes(&scope.facts, goal),
                None => None,
            };
            if let Some(values) = &always {
                // The precondition's own names: the parameters it reads, and
                // the lengths of them.
                let given: BTreeMap<String, i64> = with
                    .iter()
                    .filter(|(name, _)| read.contains(*name))
                    .filter_map(|(name, term)| {
                        Some((name.clone(), self.arena.int_value((*term)?, values)?))
                    })
                    .collect();
                let (written, from) = match (&pre.computed, &pre.origin) {
                    (Some(computed), Some(origin)) => (
                        computed.clone(),
                        Some(format!(
                            "The precondition is `assert({})` in `{origin}`, carried back to \
                             `{callee}`'s entry.",
                            pre.written
                        )),
                    ),
                    (Some(computed), None) => (
                        computed.clone(),
                        Some(format!(
                            "The precondition is `assert({})` in `{callee}`, carried back to its \
                             entry.",
                            pre.written
                        )),
                    ),
                    (None, _) => (pre.written.clone(), None),
                };
                self.out.findings.push(Finding {
                    severity: Severity::Error,
                    span,
                    code: "NK1207",
                    message: format!(
                        "This call breaks `{callee}`'s precondition `{written}` every time it is \
                         reached."
                    ),
                    notes: [Some(format!("Here {}.", shown(&given))), from]
                        .into_iter()
                        .flatten()
                        .collect(),
                    help: Some(format!(
                        "Pass `{callee}` arguments for which `{written}` holds, or check them \
                         with a guard before the call."
                    )),
                    labels: Vec::new(),
                });
            }
            // **A call that breaks it for some of the values that reach it**
            // (D8): one such state, and what the parameters are in it.
            if always.is_none()
                && let Some(goal) = goal
                && let Some(values) = self.breaks_when(scope, goal)
            {
                let given: BTreeMap<String, i64> = with
                    .iter()
                    .filter(|(name, _)| read.contains(*name))
                    .filter_map(|(name, term)| {
                        Some((name.clone(), self.arena.int_value((*term)?, &values)?))
                    })
                    .collect();
                let written = pre.computed.clone().unwrap_or_else(|| pre.written.clone());
                let from = pre.computed.as_ref().map(|_| {
                    let origin = pre.origin.as_deref().unwrap_or(callee);
                    format!(
                        "The precondition is `assert({})` in `{origin}`.",
                        pre.written
                    )
                });
                self.out.findings.push(Finding {
                    severity: Severity::Error,
                    span,
                    code: "NK1207",
                    message: format!(
                        "This call breaks `{callee}`'s precondition `{written}` when {}.",
                        shown(&values)
                    ),
                    notes: [Some(format!("Then {}.", shown(&given))), from]
                        .into_iter()
                        .flatten()
                        .collect(),
                    help: Some(format!(
                        "Pass `{callee}` arguments for which `{written}` holds, or check them \
                         with a guard before the call."
                    )),
                    labels: Vec::new(),
                });
            }
            // **Not proved: the call carries the check** (D5), with the
            // arguments in place of the parameters - where every argument is
            // one the prover reads, which also makes it pure, so evaluating it
            // once more for the check changes nothing. Where one is not, the
            // call reaches the checked entry (ADR-269 D20).
            reach = joined_reach(
                reach,
                match goal {
                    Some(goal) => Reach::Checked(vec![Check {
                        rust: rust_of(&self.arena, goal),
                        written: pre.failure(callee),
                        message: pre.message.clone(),
                        operands: read
                            .iter()
                            .filter_map(|name| {
                                let value = with.get(name).copied().flatten()?;
                                Some((name.clone(), rust_of(&self.arena, value)))
                            })
                            .collect(),
                    }]),
                    None => Reach::Through,
                },
            );
        }
        let key = call_key(self.parsed, callee, args);
        let joined = match self.out.reaches.remove(&key) {
            Some(before) => joined_reach(before, reach),
            None => reach,
        };
        self.out.reaches.insert(key, joined);
    }

    /// **A callee's precondition this function cannot show is its callers'**
    /// (`ProverState::carry_back`).
    fn carry_back(
        &mut self,
        goal: Option<i64>,
        pre: &PreClaim,
        callee: &str,
        site: CarriedFrom,
        scope: &Scope,
        at: &Where,
    ) {
        let function = at
            .function
            .as_deref()
            .filter(|_| at.no_precondition.is_none());
        let Terms { held, copy } = &mut self.arena;
        self.state.carry_back(
            held,
            goal,
            pre,
            callee,
            site,
            scope,
            function,
            &at.params,
            |arena, facts, goal| answer_of(copy, arena, facts, goal),
            |arena, facts, goal| model_of(copy, arena, facts, goal),
        )
    }

    /// Whether the facts prove the goal: one query to the reference solver
    /// ([ADR-270](../../docs/specification/adr/adr-270.md) D3, D4), whose
    /// certificate is checked before a check is left out (D5). The reference
    /// solver's word would be enough; checking it costs a replay of a few
    /// steps, and a solver fault becomes a check at run time instead of a
    /// claim nobody holds. `Err(Some(_))` says the certificate was rejected.
    fn proves(&self, facts: &[i64], goal: i64) -> Result<(), Option<String>> {
        match self.arena.query(facts, goal, crate::proofs::ask) {
            crate::proofs::Asked::Proved => Ok(()),
            crate::proofs::Asked::Rejected(why) => Err(Some(why)),
            crate::proofs::Asked::Refuted(_) | crate::proofs::Asked::Unknown => Err(None),
        }
    }

    /// `term` at the function's entry (`ProverScope::at_entry`).
    fn at_entry(&mut self, term: i64, scope: &Scope) -> Option<i64> {
        scope.at_entry(&mut self.arena.held, term)
    }

    /// `claim` as a postcondition (`ProverScope::postcondition_at`).
    fn postcondition_at(
        &mut self,
        claim: i64,
        returned: &str,
        scope: &Scope,
        at: &Where,
    ) -> Option<i64> {
        scope.postcondition_at(&mut self.arena.held, claim, returned, &at.params)
    }

    /// **An exit of a free function** (`ProverState::exit`).
    fn exit(&mut self, value: &Expr, scope: &Scope, at: &Where) {
        let Terms { held, copy } = &mut self.arena;
        self.state.exit(
            held,
            value,
            &self.parsed.interner,
            scope,
            at.function.as_deref(),
            !at.lambda && at.free,
            &at.params,
            |arena, facts, goal| answer_of(copy, arena, facts, goal),
        )
    }

    /// What a call's postconditions say of the name its result is bound to
    /// (`prover_state::postconditions_for`), from the callee's own
    /// postconditions or another package's ledger.
    fn postconditions_of(
        &mut self,
        callee: &str,
        args: &[Expr],
        bound: &str,
        scope: &Scope,
    ) -> Vec<i64> {
        let (posts, params): (Vec<PostClaim>, Vec<String>) =
            match self.state.postconditions.get(callee).cloned() {
                Some(posts) => {
                    let Some(Item::Fn { args: params, .. }) = self.function_named(callee) else {
                        return Vec::new();
                    };
                    let params = params
                        .iter()
                        .map(|p| self.parsed.text(p.name).to_string())
                        .collect();
                    (posts, params)
                }
                None => match self.foreign_contract(callee) {
                    Some(foreign) => (foreign.ensures, foreign.params),
                    None => return Vec::new(),
                },
            };
        prover_state::postconditions_for(
            &mut self.arena.held,
            &posts,
            &params,
            args,
            bound,
            scope,
            &self.parsed.interner,
        )
    }

    /// Whether a free function hands back a whole number.
    fn returns_whole(&mut self, callee: &str) -> bool {
        match self.function_named(callee) {
            Some(Item::Fn { ret_type, .. }) => ret_type.as_ref().is_some_and(|t| {
                t.generics.is_empty() && is_whole_number(self.parsed.text(t.name))
            }),
            _ => self
                .foreign_contract(callee)
                .is_some_and(|foreign| foreign.whole_result),
        }
    }

    /// **Another package's contract, read back into terms** (ADR-269 D18):
    /// each `requires` and `ensures` the ledger states, parsed as the
    /// language's own syntax over the parameters' names. `None` where the
    /// ledger has no such function; a condition that does not read back is
    /// left out, which only ever proves less.
    fn foreign_contract(&mut self, key: &str) -> Option<Foreign> {
        if let Some(known) = self.state.foreign.get(key) {
            return known.clone();
        }
        let contract = self.own.functions.get(key)?.clone();
        let shape = prover_claims::foreign_shape(&contract);
        let mut read_back = |texts: &[String]| -> Vec<i64> {
            texts
                .iter()
                .map(|text| condition(&mut self.arena, text, &shape.names).unwrap_or(-1))
                .collect()
        };
        let requires = read_back(&contract.requires);
        let ensures = read_back(&contract.ensures);
        let foreign = prover_claims::foreign_from(&contract, shape, &requires, &ensures);
        self.state
            .foreign
            .insert(key.to_string(), Some(foreign.clone()));
        Some(foreign)
    }

    /// A condition joins the path (`ProverScope::on_the_path`).
    fn on_the_path(&mut self, scope: &mut Scope, condition: Option<i64>) {
        scope.on_the_path(&mut self.arena.held, condition)
    }

    /// **The precondition a claim makes** (`ProverScope::precondition_at`).
    fn precondition_at(&mut self, claim: i64, scope: &Scope, at: &Where) -> Option<i64> {
        scope.precondition_at(&mut self.arena.held, claim, &at.params)
    }

    /// The values that show a claim false every time it is reached
    /// (`prover_solver::refuting_values`).
    fn refutes(&mut self, facts: &[i64], claim: i64) -> Option<BTreeMap<String, i64>> {
        let Terms { held, copy } = &mut self.arena;
        prover_solver::refuting_values(
            held,
            facts,
            claim,
            &|arena, facts, goal| answer_of(copy, arena, facts, goal),
            &|arena, facts, goal| model_of(copy, arena, facts, goal),
        )
    }

    /// **Values that reach a claim and make it false**
    /// (`prover_solver::breaking_values`).
    fn breaks_when(&mut self, scope: &Scope, claim: i64) -> Option<BTreeMap<String, i64>> {
        let Terms { held, copy } = &mut self.arena;
        prover_solver::breaking_values(held, scope, claim, &|arena, facts, goal| {
            model_of(copy, arena, facts, goal)
        })
    }

    /// Whether a call may throw out of the function `at` is, which `throws`:
    /// what the callee declares, by its `throws` or its ledger's; a call
    /// nothing describes may.
    fn throwing(&self, call: &Expr, at: &Where) -> bool {
        if !at.throws {
            return false;
        }
        let name = match call {
            Expr::Call { func, .. } => match &**func {
                Expr::Variable(name) => Some(self.parsed.text(*name).to_string()),
                Expr::Path(_) => qualified(self.parsed, func),
                _ => None,
            },
            _ => None,
        };
        let Some(name) = name else {
            return true;
        };
        if let Some(Item::Fn { can_throw, .. }) = self.function_named(&name) {
            return *can_throw;
        }
        match self
            .own
            .functions
            .get(&name)
            .or_else(|| self.library.functions.get(&name))
        {
            Some(contract) => !contract.fails_with.is_empty(),
            None => true,
        }
    }

    fn function_named(&self, name: &str) -> Option<&'a Item> {
        let parsed: &'a Parsed = self.parsed;
        parsed.program.items.iter().map(|i| &i.node).find(|item| {
            matches!(item, Item::Fn { name: Some(n), receiver: None, .. } if parsed.text(*n) == name)
        })
    }

    /// Whether a value came from outside the program (ADR-010 D2): it names a
    /// tainted binding, or calls a source `std`'s ledger calls untrusted.
    fn tainted(&self, expr: &Expr, scope: &Scope) -> bool {
        let parsed = self.parsed;
        let library = self.library;
        let mut tainted = false;
        crate::contracts::sync::visit_expr(parsed, expr, &mut |e| match e {
            Expr::Variable(name) if scope.tainted.contains(parsed.text(*name)) => tainted = true,
            Expr::Call { func, .. } => {
                let name = match &**func {
                    Expr::Variable(n) => Some(parsed.text(*n).to_string()),
                    Expr::Path(segments) => Some(
                        segments
                            .iter()
                            .map(|s| parsed.text(*s))
                            .collect::<Vec<_>>()
                            .join("::"),
                    ),
                    _ => None,
                };
                if let Some(name) = name
                    && let Some((_, contract)) = library.lookup(&name)
                    && contract.provenance == Some(Provenance::Untrusted)
                {
                    tainted = true;
                }
            }
            _ => {}
        });
        tainted
    }
}

/// **Whether a statement may leave the block it stands in** for some states:
/// a `return`, a `break`, a `throw`, a `?`, a `continue` where `with_continue`
/// says it counts, and a call `throwing` says may throw. Inside nested
/// blocks too, lambdas included: it only ever costs a warning. The walk is
/// `tools/leaves.nika` (ADR-294, #436); this hands it the holes a literal
/// holds, which only the compiler can parse.
fn leaves_stmt(
    parsed: &Parsed,
    stmt: &Stmt,
    throwing: &dyn Fn(&Expr) -> bool,
    with_continue: bool,
) -> bool {
    nikaia_std::tools::leaves::leaves_stmt(
        stmt,
        &|e| throwing(e),
        &|e| crate::emit::literal_expressions(parsed, e),
        with_continue,
    )
}

fn leaves_block(
    parsed: &Parsed,
    block: &Block,
    throwing: &dyn Fn(&Expr) -> bool,
    with_continue: bool,
) -> bool {
    nikaia_std::tools::leaves::leaves_block(
        block,
        &|e| throwing(e),
        &|e| crate::emit::literal_expressions(parsed, e),
        with_continue,
    )
}

fn leaves_expr(
    parsed: &Parsed,
    expr: &Expr,
    throwing: &dyn Fn(&Expr) -> bool,
    with_continue: bool,
) -> bool {
    nikaia_std::tools::leaves::leaves_expr(
        expr,
        &|e| throwing(e),
        &|e| crate::emit::literal_expressions(parsed, e),
        with_continue,
    )
}

/// Whether two walks found the same preconditions, claim by claim.
fn same_preconditions(
    arena: &Terms,
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

fn refusal(span: Span, message: String, notes: Vec<String>, help: &str) -> Finding {
    Finding {
        severity: Severity::Error,
        span,
        code: "NK1202",
        message,
        notes,
        help: Some(help.to_string()),
        labels: Vec::new(),
    }
}

// **How a term and a value are written** (ADR-269 D7, D18) is
// `tools/prove_text.nika` (ADR-294, #436): these hand it the arena one node
// at a time and the emitter's escaping of a name.

fn rust_of(arena: &Terms, id: i64) -> String {
    prove_text::rust_of(id, &|at| arena.held.at(at), &|name| {
        crate::emit::escaped(name).into_owned()
    })
}

fn rust_of_name(name: &str) -> String {
    prove_text::rust_of_name(name, &|name| crate::emit::escaped(name).into_owned())
}

/// A term as a reader writes it, with `→` for the implication a branch makes
/// of a precondition.
fn term_text(arena: &Terms, id: i64) -> String {
    prove_text::term_text(id, &|at| arena.held.at(at))
}

/// A term in the language's own syntax, as the ledger writes it and reads it
/// back ([ADR-251](../../docs/specification/adr/adr-251.md) D4).
fn ledger_text(arena: &Terms, id: i64) -> String {
    prove_text::term_ledger_text(id, &|at| arena.held.at(at))
}

use nikaia_std::tools::prove_terms;
use nikaia_std::tools::prove_text::{self, has_a_length, lowered_first, shown};

fn is_whole_number(ty: &str) -> bool {
    nikaia_std::tools::bounds_shape::is_whole_number(ty)
}

/// Every name a claim reads.
fn names_in(parsed: &Parsed, expr: &Expr) -> BTreeSet<String> {
    nikaia_std::tools::claim_names::claim_names_in(expr, &parsed.interner, &|e| {
        crate::emit::literal_expressions(parsed, e)
    })
}

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
    let mut arena = Terms::default();
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
    arena.query(&facts, goal, crate::proofs::ask) == crate::proofs::Asked::Proved
}

/// **A condition the ledger states, read back** (ADR-269 D18): the text is
/// the language's own syntax, so the compiler's parser reads it, inside an
/// `assert` of a function nobody calls, and the prover's own reading turns it
/// into a term. Only `names` are variables of it.
fn condition(arena: &mut Terms, text: &str, names: &BTreeSet<String>) -> Option<i64> {
    let source = format!("fn __condition() {{\n    assert({text})\n}}\n");
    let parsed = crate::parser::parse_to_ast(&source).ok()?;
    let Item::Fn { body, .. } = &parsed.program.items.first()?.node else {
        return None;
    };
    let Stmt::Expr(Expr::Call { args, .. }) = &body.stmts.first()?.node else {
        return None;
    };
    claim_with(arena, &parsed, args.first()?, &|name| names.contains(name))
}

/// A path callee as the ledger keys it: `mathx::percent`, unaliased.
fn qualified(parsed: &Parsed, func: &Expr) -> Option<String> {
    let Expr::Path(segments) = func else {
        return None;
    };
    Some(
        parsed.unaliased(
            &segments
                .iter()
                .map(|s| parsed.text(*s))
                .collect::<Vec<_>>()
                .join("::"),
        ),
    )
}

/// A whole-number expression, where it is one this prover reads (ADR-269 D9).
fn lin(arena: &mut Terms, parsed: &Parsed, expr: &Expr, scope: &Scope) -> Option<i64> {
    lin_with(arena, parsed, expr, &|name| scope.ints.contains(name))
}

/// `expr` as a linear term over the names `var` admits
/// (`tools/prove_terms.nika`, ADR-294, #436), its nodes put into `arena`.
fn lin_with(
    arena: &mut Terms,
    parsed: &Parsed,
    expr: &Expr,
    var: &dyn Fn(&str) -> bool,
) -> Option<i64> {
    arena.built(|nodes| prove_terms::lin_term(expr, &parsed.interner, &|name| var(name), nodes))
}

/// The claim `expr` as a term, where it is one this prover reads.
fn claim(arena: &mut Terms, parsed: &Parsed, expr: &Expr, scope: &Scope) -> Option<i64> {
    claim_with(arena, parsed, expr, &|name| scope.ints.contains(name))
}

fn claim_with(
    arena: &mut Terms,
    parsed: &Parsed,
    expr: &Expr,
    var: &dyn Fn(&str) -> bool,
) -> Option<i64> {
    arena.built(|nodes| prove_terms::claim_term(expr, &parsed.interner, &|name| var(name), nodes))
}

/// **The walk's terms, held in Nikaia** (`tools/prover_arena.nika`, #436):
/// every term is built and read there. The solver reads `nikaia-logic`'s
/// arena, a copy made node for node when a question is asked, so that a
/// term's place is the same in both.
#[derive(Debug, Clone)]
struct Terms {
    held: TermArena,
    copy: std::cell::RefCell<SolverCopy>,
}

/// `nikaia-logic`'s copy of the terms, as far as it has been made.
#[derive(Debug, Clone, Default)]
struct SolverCopy {
    logic: Arena,
    /// The copy's `TermId` of each node of `held`, by place.
    ids: Vec<TermId>,
}

impl Default for Terms {
    fn default() -> Terms {
        Terms {
            held: TermArena::empty(),
            copy: Default::default(),
        }
    }
}

impl Terms {
    /// The solver's question whether `facts` imply `goal`, handed to `ask`.
    fn query<R>(&self, facts: &[i64], goal: i64, ask: impl FnOnce(&Query) -> R) -> R {
        asked_of(&self.copy, &self.held, facts, goal, ask)
    }

    fn var(&mut self, name: &str) -> i64 {
        self.held.var(name)
    }

    fn le(&mut self, a: i64, b: i64) -> i64 {
        self.held.le(a, b)
    }

    fn lt(&mut self, a: i64, b: i64) -> i64 {
        self.held.lt(a, b)
    }

    fn eq(&mut self, a: i64, b: i64) -> i64 {
        self.held.equal(a, b)
    }

    fn and(&mut self, parts: Vec<i64>) -> i64 {
        self.held.and(parts)
    }

    fn not(&mut self, a: i64) -> i64 {
        self.held.not(a)
    }

    fn mentions(&self, id: i64, name: &str) -> bool {
        self.held.mentions(id, name)
    }

    fn variables(&self, id: i64, names: &mut BTreeSet<String>) {
        self.held.variables(id, names)
    }

    fn int_value(&self, id: i64, values: &BTreeMap<String, i64>) -> Option<i64> {
        self.held.int_value(id, values)
    }

    fn substitute(&mut self, id: i64, with: &BTreeMap<String, i64>) -> i64 {
        let given = with
            .iter()
            .map(|(name, term)| (name.clone(), *term))
            .collect();
        self.held.substitute(id, &given)
    }

    /// A term `prove_terms` builds, its nodes appended to `held`.
    fn built(&mut self, build: impl FnOnce(&mut Vec<SolverTerm>) -> Option<i64>) -> Option<i64> {
        build(&mut self.held.nodes)
    }
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
