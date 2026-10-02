// crates/nikaia/src/prove.rs
//
// Every `assert` outside a `test` block, proved while the program is built
// ([ADR-264](../../docs/specification/adr/adr-264.md)).
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
// ([ADR-255](../../docs/specification/adr/adr-255.md)) — a `for` over a
// range, a `let`'s value, the function's preconditions (D5) and every claim
// proved before.
//
// **How** (D10) is not decided here: each claim is a query to `nikaia-logic`
// ([ADR-265](../../docs/specification/adr/adr-265.md)) - its facts and its
// goal as terms - and the reference solver there answers it. This file is the
// frontend: it knows the program, and what an answer means for it.
//
// **Soundness around bindings.** A fact names variables by their name, so a
// name bound again drops every fact that mentions it, and a block whose
// bindings this walk cannot see — a lambda's body, a `match` arm — starts with
// no facts and no variables. Losing facts only costs proofs.

use crate::contracts::LedgerOps;
use std::collections::{BTreeMap, BTreeSet};

use nikaia_logic::{
    Answer, Arena, Budget, FourierMotzkin, Query, Solver, TermId, verify, verify_model,
};

use crate::ast::{BinaryOp, Block, Expr, Item, Span, Spanned, Stmt, UnaryOp};
use crate::check::{Finding, Severity};
use crate::contracts::{Ledger, Provenance};
use crate::parser::Parsed;

/// How the compiler holds one `assert` (ADR-264 D4-D6, D11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    /// In a `test` block: the test's verdict, checked when it runs (D11).
    ByTheTest,
    /// Proved from what precedes it; no check is emitted.
    Proved,
    /// The precondition of the named function (D5): the body assumes it, and
    /// each call proves it or carries the check. The number is how many
    /// distinct calls carry one.
    Precondition(String, usize),
    /// Not proved: the condition is checked where it is reached (D4), for
    /// the reason given.
    AtRunTime(String),
    /// A claim about data from outside the program (D6): `NK1202`.
    Refused,
}

/// A precondition a call does not prove, checked at the call (D5): the
/// callee's claim with the call's arguments in place of its parameters.
#[derive(Debug, Clone)]
pub struct CallCheck {
    pub condition: Expr,
    /// The claim as the callee wrote it, and whose it is.
    pub written: String,
}

/// The calls that carry a check, by callee and their arguments as written:
/// two calls that read the same carry the same check.
pub type CallChecks = BTreeMap<(String, String), Vec<CallCheck>>;

/// What the prover found: its refusals, how each claim it reached is held,
/// keyed as [`crate::check::Checked::claims`] is, and the calls that carry a
/// precondition's check.
#[derive(Debug, Default)]
pub struct Proved {
    pub findings: Vec<Finding>,
    pub held: BTreeMap<(usize, String), Held>,
    pub call_checks: CallChecks,
}

/// How a call is named in [`CallChecks`].
pub fn call_key(parsed: &Parsed, callee: &str, args: &[Expr]) -> (String, String) {
    let args: Vec<String> = args
        .iter()
        .map(|a| crate::check::written(parsed, a))
        .collect();
    (callee.to_string(), args.join(", "))
}

/// Prove every `assert` of the program. `claims` are the checker's: a call is
/// an `assert` exactly where the checker recognised the prelude's.
pub fn prove(parsed: &Parsed, library: &Ledger, claims: &BTreeSet<(usize, String)>) -> Proved {
    let mut prover = Prover {
        parsed,
        library,
        claims,
        preconditions: BTreeMap::new(),
        in_the_body: BTreeMap::new(),
        collecting: true,
        out: Proved::default(),
        in_test: false,
        arena: Arena::new(),
    };
    // Pass 1: which parameter claims are preconditions (D5). A function's own
    // preconditions depend only on its own body.
    prover.every_body();
    let preconditions = std::mem::take(&mut prover.preconditions);
    prover.out = Proved::default();
    prover.collecting = false;
    prover.preconditions = preconditions;
    // Pass 2: every claim and every call.
    prover.every_body();

    // **A precondition some call cannot carry is checked in the body** (D5):
    // a call whose arguments the prover cannot read, or the function handed
    // on as a value. Its calls then carry nothing.
    let in_the_body = std::mem::take(&mut prover.in_the_body);
    prover
        .out
        .call_checks
        .retain(|(callee, _), _| !in_the_body.contains_key(callee));
    let mut carried: BTreeMap<String, usize> = BTreeMap::new();
    for (callee, _) in prover.out.call_checks.keys() {
        *carried.entry(callee.clone()).or_insert(0) += 1;
    }
    for held in prover.out.held.values_mut() {
        if let Held::Precondition(function, calls) = held {
            if let Some(why) = in_the_body.get(function) {
                *held = Held::AtRunTime(format!(
                    "a precondition of `{function}`, checked in its body: {why}"
                ));
            } else {
                *calls = carried.get(function).copied().unwrap_or(0);
            }
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

/// A function's precondition: the claims its callers prove about its
/// parameters (D5).
#[derive(Debug, Clone)]
struct Precondition {
    claims: Vec<PreClaim>,
}

/// One claim of a precondition (ADR-266 D2).
#[derive(Debug, Clone)]
struct PreClaim {
    /// The condition at the function's entry, over its parameters: the
    /// `assert`'s claim carried back through the body - `mode == 1 → x > 1`
    /// for `assert(y > 0)` after `let y = x - 1` inside `if mode == 1`.
    term: TermId,
    /// The `assert`'s condition as written, where it is the precondition as
    /// it stands - at the top of the body, over parameters only - so that a
    /// call can check it with its arguments in place (ADR-264 D5). A
    /// computed one is checked in the body until the ledger carries it
    /// (ADR-266 D7).
    as_written: Option<Expr>,
    /// The `assert`'s condition, written, for messages.
    written: String,
    /// The condition at the entry as a reader writes it, where it is not the
    /// claim as written: `mode == 1 → x - 1 > 0` (ADR-266 D8).
    computed: Option<String>,
}

struct Prover<'a> {
    parsed: &'a Parsed,
    library: &'a Ledger,
    claims: &'a BTreeSet<(usize, String)>,
    preconditions: BTreeMap<String, Precondition>,
    /// Functions whose precondition is checked in their body, and why (D5).
    in_the_body: BTreeMap<String, String>,
    /// Pass 1 records preconditions and says nothing.
    collecting: bool,
    out: Proved,
    in_test: bool,
    /// Every term the walk builds, facts and claims alike (ADR-265 D2).
    arena: Arena,
}

/// What a body may know at one point.
#[derive(Debug, Clone, Default)]
struct Scope {
    /// Names that are variables of the proof: immutable whole numbers.
    ints: BTreeSet<String>,
    /// Names whose value came from outside the program (ADR-010).
    tainted: BTreeSet<String>,
    /// Names bound anywhere in this body, so a callee's name that a local
    /// shadows is not the callee.
    locals: BTreeSet<String>,
    facts: Vec<TermId>,
    /// **Each whole number's value at the function's entry**, as a term over
    /// its parameters (ADR-266 D2): after `let y = x - 1`, `y` is `x - 1`,
    /// and after `let x = x + 1`, `x` is the parameter plus one.
    entry: BTreeMap<String, TermId>,
    /// **What holds of the parameters on the way here**: the branches taken
    /// and the guards passed, at the entry. `None` once a loop, a lambda or a
    /// block this walk cannot see stands between the entry and here (D3).
    path: Option<Vec<TermId>>,
}

impl Scope {
    /// `name` is bound again: nothing known about the old binding holds.
    fn rebind(&mut self, arena: &Arena, name: &str) {
        let length = length_of(name);
        self.ints.remove(name);
        self.ints.remove(&length);
        self.tainted.remove(name);
        self.entry.remove(name);
        self.entry.remove(&length);
        self.locals.insert(name.to_string());
        self.facts
            .retain(|fact| !arena.mentions(*fact, name) && !arena.mentions(*fact, &length));
    }

    /// `name` is a list or text that does not change, so `name.len()` is a
    /// variable of the proof: a length is never negative, and a literal's is
    /// known.
    fn has_length(&mut self, arena: &mut Arena, name: &str, known: Option<i128>) {
        let length = length_of(name);
        self.ints.insert(length.clone());
        let (len, zero) = (arena.var(&length), arena.int(0));
        self.facts.push(arena.ge(len, zero));
        if let Some(n) = known {
            let n = arena.int(n);
            self.facts.push(arena.eq(len, n));
            self.entry.insert(length, n);
        }
    }

    /// `name` is a whole number that is never negative.
    fn not_negative(&mut self, arena: &mut Arena, name: &str) {
        let (n, zero) = (arena.var(name), arena.int(0));
        self.facts.push(arena.ge(n, zero));
    }

    /// A block whose bindings this walk cannot see: no facts, no variables.
    fn blind(&self) -> Scope {
        Scope {
            ints: BTreeSet::new(),
            tainted: self.tainted.clone(),
            locals: self.locals.clone(),
            facts: Vec::new(),
            entry: BTreeMap::new(),
            path: None,
        }
    }
}

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
}

impl<'a> Prover<'a> {
    fn every_body(&mut self) {
        let parsed = self.parsed;
        for item in &parsed.program.items {
            match &item.node {
                Item::Fn { .. } => self.function(item, false),
                Item::Impl { methods, .. } => {
                    for method in methods {
                        self.function(method, true);
                    }
                }
                Item::Test { body, .. } | Item::Bench { body, .. } => {
                    self.in_test = true;
                    let at = Where {
                        function: None,
                        params: BTreeSet::new(),
                        no_precondition: Some("a test has no callers"),
                        top: true,
                    };
                    self.block(body, &mut Scope::default(), &at);
                    self.in_test = false;
                }
                _ => {}
            }
        }
    }

    fn function(&mut self, item: &Spanned<Item>, method: bool) {
        let Item::Fn {
            name,
            receiver,
            args,
            body,
            is_public,
            ..
        } = &item.node
        else {
            return;
        };
        let own = name.map(|n| self.parsed.text(n).to_string());
        // A `test` block `nikaia test` has already turned into a function is
        // still a test (D11).
        let was_a_test = own
            .as_deref()
            .is_some_and(crate::modules::is_a_test_function);
        let mut scope = Scope {
            path: Some(Vec::new()),
            ..Scope::default()
        };
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
                params.insert(arg_name.clone());
                let at_entry = self.arena.var(&arg_name);
                scope.entry.insert(arg_name.clone(), at_entry);
                if ty.starts_with('u') {
                    scope.not_negative(&mut self.arena, &arg_name);
                }
            }
            if !arg.mutable && has_a_length(ty) {
                scope.has_length(&mut self.arena, &arg_name, None);
                params.insert(arg_name.clone());
                let length = length_of(&arg_name);
                let at_entry = self.arena.var(&length);
                scope.entry.insert(length, at_entry);
            }
        }
        let no_precondition = if method || receiver.is_some() {
            Some("A method can't have a precondition yet: its callers aren't found by name.")
        } else if *is_public {
            Some(
                "A `pub fn` can't have a precondition yet: the ledger doesn't carry one, so \
                 another package would call it unchecked.",
            )
        } else if own.as_deref() == Some("main") {
            Some("`main` has no callers.")
        } else {
            None
        };
        let at = Where {
            function: own,
            params,
            no_precondition,
            top: true,
        };
        self.in_test = was_a_test;
        self.block(body, &mut scope, &at);
        self.in_test = false;
    }

    /// Walk a block; whether it always leaves.
    fn block(&mut self, block: &Block, scope: &mut Scope, at: &Where) -> bool {
        for stmt in &block.stmts {
            if self.stmt(stmt, scope, at) {
                return true;
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
                for name in names {
                    let name = self.parsed.text(*name).to_string();
                    scope.rebind(&self.arena, &name);
                    if tainted {
                        scope.tainted.insert(name.clone());
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
                        }
                        // At the entry the value is what it read there, so
                        // a shadowing `let` is no trouble here.
                        if let Some(at_entry) = value_at_entry {
                            scope.entry.insert(name.clone(), at_entry);
                        }
                    } else if typed_whole {
                        scope.ints.insert(name.clone());
                    }
                    if let Some(t) = ty
                        && self.parsed.text(t.name).starts_with('u')
                        && scope.ints.contains(&name)
                    {
                        scope.not_negative(&mut self.arena, &name);
                    }
                    match value {
                        Expr::ListLit { items, .. } => {
                            scope.has_length(
                                &mut self.arena,
                                &name,
                                i128::try_from(items.len()).ok(),
                            );
                        }
                        Expr::LitStr { .. } => scope.has_length(&mut self.arena, &name, None),
                        _ if ty
                            .as_ref()
                            .is_some_and(|t| has_a_length(self.parsed.text(t.name))) =>
                        {
                            scope.has_length(&mut self.arena, &name, None)
                        }
                        _ => {}
                    }
                }
                false
            }
            Stmt::Comptime { name, value, .. } => {
                self.expr(value, span, scope, &nested);
                scope.rebind(&self.arena, self.parsed.text(*name));
                false
            }
            Stmt::Assign { target, value, .. } => {
                self.expr(target, span, scope, &nested);
                self.expr(value, span, scope, &nested);
                if let Expr::Variable(name) = target {
                    let name = self.parsed.text(*name).to_string();
                    let tainted = self.tainted(value, scope);
                    scope.rebind(&self.arena, &name);
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
                    inner.rebind(&self.arena, &name);
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
                    }
                }
                // A claim in a loop, or after one, is not carried back to
                // the entry (ADR-266 D3): that needs an invariant.
                inner.path = None;
                self.block(body, &mut inner, &nested);
                scope.path = None;
                false
            }
            Stmt::While { cond, body } => {
                self.expr(cond, span, scope, &nested);
                let mut inner = scope.clone();
                inner.path = None;
                scope.path = None;
                if let Some(holds) = claim(&mut self.arena, self.parsed, cond, &inner) {
                    inner.facts.push(holds);
                }
                self.block(body, &mut inner, &nested);
                false
            }
            Stmt::Return(value) => {
                if let Some(value) = value {
                    self.expr(value, span, scope, &nested);
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
            Expr::Call { func, args, .. }
                if matches!(&**func, Expr::Variable(_))
                    && args.len() == 1
                    && self
                        .claims
                        .contains(&(span.at(), crate::check::argument_shape(&args[0]))) =>
            {
                self.expr(&args[0], span, scope, &nested);
                self.an_assert(&args[0], span, scope, at);
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
                self.on_the_path(&mut then_scope, holds);
                let then_leaves = self.block(then_branch, &mut then_scope, &nested);
                let else_leaves = match else_branch {
                    Some(block) => {
                        let mut else_scope = scope.clone();
                        if let Some(fails) = &fails {
                            else_scope.facts.push(*fails);
                        }
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
                    }
                    (false, true) => {
                        if let Some(holds) = holds {
                            scope.facts.push(holds);
                        }
                        self.on_the_path(scope, holds);
                    }
                    (false, false) => {}
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
    fn an_assert(&mut self, cond: &Expr, span: Span, scope: &mut Scope, at: &Where) {
        let key = (span.at(), crate::check::argument_shape(cond));
        if self.in_test {
            self.out.held.insert(key, Held::ByTheTest);
            return;
        }
        let claim = claim(&mut self.arena, self.parsed, cond, scope);
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
        // carried back to the entry (ADR-266 D2): what it says about the
        // parameters, under the branches and guards on the way. At the top of
        // the body, over parameters only, that is the claim as written
        // (ADR-264 D5).
        let names = names_in(self.parsed, cond);
        let only_params = !names.is_empty() && names.iter().all(|n| at.params.contains(n));
        // **A claim what is known rules out is not a precondition**: every
        // caller that reaches it breaks it, so it would only say *never come
        // here*. It is warned about where it stands (ADR-264 D8). Decided in
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
            if self.collecting {
                let as_written = (only_params && at.top).then(|| cond.clone());
                let pre = PreClaim {
                    term,
                    computed: as_written.is_none().then(|| term_text(&self.arena, term)),
                    as_written,
                    written: crate::check::written(self.parsed, cond),
                };
                self.preconditions
                    .entry(function.clone())
                    .or_insert_with(|| Precondition { claims: Vec::new() })
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
            if self.collecting {
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
        // is told, with values (ADR-264 D8).
        if let Some(values) = refuted.as_ref().filter(|_| !self.collecting) {
            let written = crate::check::written(self.parsed, cond);
            self.out.findings.push(Finding {
                severity: Severity::Warning,
                span,
                code: "NK1207",
                message: format!("`{written}` is false every time it is reached."),
                notes: vec![
                    format!(
                        "What is known before it rules the claim out: {}.",
                        shown(values)
                    ),
                    "It is checked when the program runs, and stops it there.".to_string(),
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
            Expr::Call { func, args, .. } => {
                if let Expr::Variable(name) = &**func {
                    callees.insert(&**func as *const Expr);
                    calls.push((parsed.text(*name).to_string(), args.clone()));
                }
            }
            Expr::Variable(name) if !callees.contains(&(e as *const Expr)) => {
                values.push(parsed.text(*name).to_string());
            }
            _ => {}
        });
        for (callee, args) in calls {
            if scope.locals.contains(&callee) {
                continue;
            }
            if let Some(pre) = self.preconditions.get(&callee).cloned() {
                self.a_call(&callee, &pre, &args, span, scope);
            }
        }
        // **A function with a precondition handed on as a value** is called
        // where the compiler can't see the call, so its body checks it (D5).
        if !self.collecting {
            for value in values {
                if !scope.locals.contains(&value) && self.preconditions.contains_key(&value) {
                    self.in_the_body
                        .entry(value)
                        .or_insert_with(|| "it is handed on as a value".to_string());
                }
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
            let mut inner = match lambdas.get(&(block as *const Block)) {
                Some(params) => {
                    let mut inner = scope.clone();
                    for param in params {
                        inner.rebind(&self.arena, param);
                    }
                    inner.path = None;
                    inner
                }
                None => scope.blind(),
            };
            self.block(
                block,
                &mut inner,
                &Where {
                    top: false,
                    ..at.clone()
                },
            );
        }
    }

    /// A call to a function with a precondition (D5).
    fn a_call(
        &mut self,
        callee: &str,
        pre: &Precondition,
        args: &[Expr],
        span: Span,
        scope: &Scope,
    ) {
        if self.collecting {
            return;
        }
        let Some(Item::Fn { args: params, .. }) = self.function_named(callee) else {
            return;
        };
        let params: Vec<String> = params
            .iter()
            .map(|p| self.parsed.text(p.name).to_string())
            .collect();
        for pre in &pre.claims {
            // Each parameter's term as this call gives it, and a list's
            // length as the argument's: `p.len()` in the precondition is the
            // argument's.
            let mut with: BTreeMap<String, Option<TermId>> = BTreeMap::new();
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
            let given: Option<BTreeMap<String, TermId>> = read
                .iter()
                .map(|name| Some((name.clone(), with.get(name).copied().flatten()?)))
                .collect();
            let goal = given.map(|given| self.arena.substitute(pre.term, &given));
            let proved = goal.is_some_and(|g| self.proves(&scope.facts, g).is_ok());
            if proved {
                continue;
            }
            // **A call that breaks the precondition every time** (ADR-264
            // D8): what is known at the call rules it out. The values shown
            // are the parameters', as this call gives them.
            if let Some(goal) = goal
                && let Some(values) = self.refutes(&scope.facts, goal)
            {
                // The precondition's own names: the parameters it reads, and
                // the lengths of them.
                let given: BTreeMap<String, i128> = with
                    .iter()
                    .filter(|(name, _)| read.contains(*name))
                    .filter_map(|(name, term)| {
                        Some((name.clone(), self.arena.int_value((*term)?, &values)?))
                    })
                    .collect();
                let (written, from) = match &pre.computed {
                    Some(computed) => (
                        computed.clone(),
                        Some(format!(
                            "The precondition is `assert({})` in `{callee}`, carried back to its \
                             entry.",
                            pre.written
                        )),
                    ),
                    None => (pre.written.clone(), None),
                };
                self.out.findings.push(Finding {
                    severity: Severity::Warning,
                    span,
                    code: "NK1207",
                    message: format!(
                        "This call breaks `{callee}`'s precondition `{written}` every time it is \
                         reached."
                    ),
                    notes: [Some(format!("Here {}.", shown(&given))), from]
                        .into_iter()
                        .flatten()
                        .chain([
                            "When the program runs, it stops where the precondition is checked."
                                .to_string(),
                        ])
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
            // once more for the check changes nothing.
            let arguments: BTreeMap<String, Expr> =
                params.iter().cloned().zip(args.iter().cloned()).collect();
            let condition = match (&pre.as_written, goal) {
                (Some(claim), Some(_)) => with_arguments(self.parsed, claim, &arguments),
                _ => None,
            };
            let Some(condition) = condition else {
                let why = if goal.is_none() {
                    "a call's argument isn't a whole number the prover reads"
                } else {
                    "it is computed through the body, and a call can't check it before the \
                     ledger carries it"
                };
                self.in_the_body
                    .entry(callee.to_string())
                    .or_insert_with(|| why.to_string());
                continue;
            };
            let written = pre.written.clone();
            let checks = self
                .out
                .call_checks
                .entry(call_key(self.parsed, callee, args))
                .or_default();
            if !checks
                .iter()
                .any(|c| c.written.ends_with(&format!("`{written}`")))
            {
                checks.push(CallCheck {
                    condition,
                    written: format!("precondition of `{callee}`: `{written}`"),
                });
            }
        }
    }

    /// Whether the facts prove the goal: one query to the reference solver
    /// ([ADR-265](../../docs/specification/adr/adr-265.md) D3, D4), whose
    /// certificate is checked before a check is left out (D5). The reference
    /// solver's word would be enough; checking it costs a replay of a few
    /// steps, and a solver fault becomes a check at run time instead of a
    /// claim nobody holds. `Err(Some(_))` says the certificate was rejected.
    fn proves(&self, facts: &[TermId], goal: TermId) -> Result<(), Option<String>> {
        let query = Query {
            arena: &self.arena,
            facts,
            goal,
        };
        match FourierMotzkin.check(&query, &Budget::default()) {
            Answer::Proved { certificate } => verify(&query, &certificate).map_err(|why| {
                Some(format!(
                    "the solver's proof did not check ({why:?}), which is a fault of the compiler"
                ))
            }),
            Answer::Refuted { .. } | Answer::Unknown(_) => Err(None),
        }
    }

    /// `term` at the function's entry: each name it reads replaced by its
    /// value there. `None` where it reads a name whose value at the entry is
    /// not known - the result of a call, a mutable binding.
    fn at_entry(&mut self, term: TermId, scope: &Scope) -> Option<TermId> {
        let mut read = BTreeSet::new();
        self.arena.variables(term, &mut read);
        let values: Option<BTreeMap<String, TermId>> = read
            .into_iter()
            .map(|name| Some((name.clone(), *scope.entry.get(&name)?)))
            .collect();
        Some(self.arena.substitute(term, &values?))
    }

    /// A branch taken or a guard passed: what it says at the entry joins the
    /// path, or the path is no longer known.
    fn on_the_path(&mut self, scope: &mut Scope, condition: Option<TermId>) {
        let at_entry = condition.and_then(|c| self.at_entry(c, scope));
        match (&mut scope.path, at_entry) {
            (Some(path), Some(c)) => path.push(c),
            (path, None) => *path = None,
            (None, Some(_)) => {}
        }
    }

    /// **The precondition a claim makes** (ADR-266 D2, D3): `path → claim`
    /// at the entry, where both read only the function's parameters and the
    /// lengths of its lists, and the result is no larger than a fixed number
    /// of terms - counted, so that whether a claim is a precondition does not
    /// depend on the machine.
    fn precondition_at(&mut self, claim: TermId, scope: &Scope, at: &Where) -> Option<TermId> {
        let path = scope.path.clone()?;
        let at_entry = self.at_entry(claim, scope)?;
        let term = match path.as_slice() {
            [] => at_entry,
            _ => {
                let taken = self.arena.and(path);
                let not_taken = self.arena.not(taken);
                self.arena.or(vec![not_taken, at_entry])
            }
        };
        let mut read = BTreeSet::new();
        self.arena.variables(term, &mut read);
        let parameters_only = read.iter().all(|name| {
            at.params.contains(name)
                || name
                    .strip_suffix(".len()")
                    .is_some_and(|base| at.params.contains(base))
        });
        (parameters_only && self.arena.size(term) <= PRECONDITION_TERMS).then_some(term)
    }

    /// The values that show a claim false every time it is reached
    /// (ADR-264 D8): the solver proves, with a checked certificate, that the
    /// facts rule the claim out - so it is false in every state the program
    /// reaches it in - and a model of the facts, checked by evaluating it,
    /// gives values for the claim's names. A model alone would not do: the
    /// facts are true but not all that is true, so a value they allow need not
    /// be one the program reaches. `None` where either is missing.
    fn refutes(&mut self, facts: &[TermId], claim: TermId) -> Option<BTreeMap<String, i128>> {
        let negation = self.arena.not(claim);
        self.proves(facts, negation).ok()?;
        let falsum = self.arena.bool(false);
        let query = Query {
            arena: &self.arena,
            facts,
            goal: falsum,
        };
        let Answer::Refuted { model } = FourierMotzkin.check(&query, &Budget::default()) else {
            return None;
        };
        if !verify_model(&query, &model) {
            return None;
        }
        let mut names = BTreeSet::new();
        self.arena.variables(claim, &mut names);
        let values: BTreeMap<String, i128> = names
            .into_iter()
            .filter_map(|n| Some((n.clone(), *model.values.get(&n)?)))
            .collect();
        Some(values)
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

/// `claim` with each parameter replaced by the argument the call gives it,
/// where the claim is one the prover reads (D9) - which is all a precondition
/// can be.
fn with_arguments(
    parsed: &Parsed,
    claim: &Expr,
    arguments: &BTreeMap<String, Expr>,
) -> Option<Expr> {
    fn replace(parsed: &Parsed, expr: &mut Expr, arguments: &BTreeMap<String, Expr>) -> bool {
        match expr {
            Expr::Variable(name) => {
                if let Some(argument) = arguments.get(parsed.text(*name)) {
                    *expr = argument.clone();
                }
                true
            }
            Expr::LitInt { .. } | Expr::LitBool(_) => true,
            Expr::MethodCall { receiver, args, .. } if args.is_empty() => {
                replace(parsed, receiver, arguments)
            }
            Expr::Unary { expr, .. } => replace(parsed, expr, arguments),
            Expr::Binary { lhs, rhs, .. } => {
                replace(parsed, lhs, arguments) && replace(parsed, rhs, arguments)
            }
            _ => false,
        }
    }
    let mut condition = claim.clone();
    replace(parsed, &mut condition, arguments).then_some(condition)
}

/// A term as a reader writes it, in the language's operators, with `→` for
/// the implication a branch makes of a precondition: `mode == 1 → x - 1 > 0`.
fn term_text(arena: &Arena, id: TermId) -> String {
    use nikaia_logic::Term;
    // How tightly each form binds, for parentheses: higher binds tighter.
    fn rank(term: &Term) -> u8 {
        match term {
            Term::Or(parts) if is_implication(parts) => 0,
            Term::Or(_) => 1,
            Term::And(_) => 2,
            Term::Le(..)
            | Term::Lt(..)
            | Term::Ge(..)
            | Term::Gt(..)
            | Term::Eq(..)
            | Term::Ne(..) => 3,
            Term::Add(..) | Term::Sub(..) => 4,
            Term::Mul(..) => 5,
            Term::Neg(_) | Term::Not(_) => 6,
            Term::Bool(_) | Term::Int(_) | Term::Var(_) => 7,
        }
    }
    fn is_implication(parts: &[TermId]) -> bool {
        parts.len() == 2
    }
    fn inner(arena: &Arena, id: TermId, at_least: u8) -> String {
        let term = arena.get(id);
        let text = whole(arena, id);
        if rank(term) < at_least {
            format!("({text})")
        } else {
            text
        }
    }
    fn whole(arena: &Arena, id: TermId) -> String {
        let term = arena.get(id);
        let r = rank(term);
        let binary = |op: &str, a: &TermId, b: &TermId| {
            format!("{} {op} {}", inner(arena, *a, r), inner(arena, *b, r + 1))
        };
        let joined = |op: &str, parts: &[TermId]| {
            parts
                .iter()
                .map(|p| inner(arena, *p, r + 1))
                .collect::<Vec<_>>()
                .join(op)
        };
        match term {
            Term::Bool(b) => b.to_string(),
            Term::Int(n) => n.to_string(),
            Term::Var(name) => name.clone(),
            Term::Neg(a) => format!("-{}", inner(arena, *a, r)),
            Term::Not(a) => format!("!{}", inner(arena, *a, r)),
            Term::Add(a, b) => binary("+", a, b),
            Term::Sub(a, b) => binary("-", a, b),
            Term::Mul(a, b) => binary("*", a, b),
            Term::Le(a, b) => binary("<=", a, b),
            Term::Lt(a, b) => binary("<", a, b),
            Term::Ge(a, b) => binary(">=", a, b),
            Term::Gt(a, b) => binary(">", a, b),
            Term::Eq(a, b) => binary("==", a, b),
            Term::Ne(a, b) => binary("!=", a, b),
            // `!taken || claim` is how a precondition under a path is built.
            Term::Or(parts) if is_implication(parts) => match arena.get(parts[0]) {
                Term::Not(taken) => format!(
                    "{} → {}",
                    inner(arena, *taken, 1),
                    inner(arena, parts[1], 1)
                ),
                _ => joined(" || ", parts),
            },
            Term::Or(parts) => joined(" || ", parts),
            Term::And(parts) => joined(" && ", parts),
        }
    }
    whole(arena, id)
}

/// Values as a reader writes them: `` `x` is 5, `xs.len()` is 0 ``.
fn shown(values: &BTreeMap<String, i128>) -> String {
    if values.is_empty() {
        return "no value of its own; the claim is false as written".to_string();
    }
    values
        .iter()
        .map(|(name, value)| format!("`{name}` is {value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `"A method can't …"` as the rest of a sentence.
fn lowered_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The variable `name.len()` stands for.
fn length_of(name: &str) -> String {
    format!("{name}.len()")
}

/// A type whose `len()` the prover reads: a list, text, an array.
fn has_a_length(ty: &str) -> bool {
    matches!(ty, "Vec" | "String" | "str" | "Array")
}

fn is_whole_number(ty: &str) -> bool {
    matches!(
        ty,
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize" | "isize"
    )
}

/// Every name a claim reads.
fn names_in(parsed: &Parsed, expr: &Expr) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    crate::contracts::sync::visit_expr(parsed, expr, &mut |e| {
        if let Expr::Variable(name) = e {
            names.insert(parsed.text(*name).to_string());
        }
    });
    names
}

// --- A program's numbers as terms -----------------------------------------

/// How many terms a precondition carried back to the entry may have
/// (ADR-266 D3).
const PRECONDITION_TERMS: usize = 64;

/// A whole-number expression, where it is one this prover reads (ADR-264 D9).
fn lin(arena: &mut Arena, parsed: &Parsed, expr: &Expr, scope: &Scope) -> Option<TermId> {
    lin_with(arena, parsed, expr, &|name| scope.ints.contains(name))
}

fn lin_with(
    arena: &mut Arena,
    parsed: &Parsed,
    expr: &Expr,
    var: &dyn Fn(&str) -> bool,
) -> Option<TermId> {
    match expr {
        Expr::LitInt { value, negative } => {
            Some(arena.int(crate::ast::int_value(*value, *negative)))
        }
        Expr::Variable(name) => {
            let name = parsed.text(*name);
            var(name).then(|| arena.var(name))
        }
        // `xs.len()` of a list that does not change is a variable of its own.
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } if args.is_empty() && parsed.text(*method) == "len" => match &**receiver {
            Expr::Variable(name) => {
                let length = length_of(parsed.text(*name));
                var(&length).then(|| arena.var(&length))
            }
            _ => None,
        },
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
        } => {
            let a = lin_with(arena, parsed, expr, var)?;
            Some(arena.neg(a))
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let l = lin_with(arena, parsed, lhs, var)?;
            let r = lin_with(arena, parsed, rhs, var)?;
            match op {
                BinaryOp::Add => Some(arena.add(l, r)),
                BinaryOp::Sub => Some(arena.sub(l, r)),
                // Linear: one side of a product is a constant.
                BinaryOp::Mul if arena.constant(l).is_some() || arena.constant(r).is_some() => {
                    Some(arena.mul(l, r))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The claim `expr` as a term, where it is one this prover reads.
fn claim(arena: &mut Arena, parsed: &Parsed, expr: &Expr, scope: &Scope) -> Option<TermId> {
    claim_with(arena, parsed, expr, &|name| scope.ints.contains(name))
}

fn claim_with(
    arena: &mut Arena,
    parsed: &Parsed,
    expr: &Expr,
    var: &dyn Fn(&str) -> bool,
) -> Option<TermId> {
    match expr {
        Expr::LitBool(b) => Some(arena.bool(*b)),
        Expr::Unary {
            op: UnaryOp::Not,
            expr,
        } => {
            let a = claim_with(arena, parsed, expr, var)?;
            Some(arena.not(a))
        }
        Expr::Binary { op, lhs, rhs, .. } => match op {
            BinaryOp::And | BinaryOp::Or => {
                let l = claim_with(arena, parsed, lhs, var)?;
                let r = claim_with(arena, parsed, rhs, var)?;
                Some(match op {
                    BinaryOp::And => arena.and(vec![l, r]),
                    _ => arena.or(vec![l, r]),
                })
            }
            BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge
            | BinaryOp::Eq
            | BinaryOp::Ne => {
                let a = lin_with(arena, parsed, lhs, var)?;
                let b = lin_with(arena, parsed, rhs, var)?;
                Some(match op {
                    BinaryOp::Lt => arena.lt(a, b),
                    BinaryOp::Le => arena.le(a, b),
                    BinaryOp::Gt => arena.gt(a, b),
                    BinaryOp::Ge => arena.ge(a, b),
                    BinaryOp::Eq => arena.eq(a, b),
                    _ => arena.ne(a, b),
                })
            }
            _ => None,
        },
        _ => None,
    }
}
