// crates/nikaia/src/prove.rs
//
// Every `assert` outside a `test` block, proved while the program is built
// ([ADR-256](../../docs/specification/adr/adr-256.md)).
//
// **What it reads** (D5): comparisons of whole numbers built from names,
// literals, `+`, `-` and a `*` by a constant, joined with `&&`, `||` and `!`.
// A name is a variable of the proof only where it is an immutable binding of a
// whole-number type; anything else makes the claim one this prover cannot
// read, and that is a refusal, never a guess.
//
// **What it knows** (D5): the branch an `if` is in, what a branch that always
// leaves has ruled out — `return … if c` is exactly that
// ([ADR-255](../../docs/specification/adr/adr-255.md)) — a `for` over a
// range, a `let`'s value, the function's preconditions (D3) and every claim
// proved before.
//
// **How** (D6): a claim is proved when every case of its negation, joined with
// the facts, has no integer solution. Fourier-Motzkin elimination over the
// rationals decides that from one side: where it finds a contradiction there is
// none over the rationals, so none over the integers; each derived bound is
// tightened to the integers on the way, which is what lets `x < 5` and
// `x > 4` contradict. It may fail to find a contradiction that exists — then
// the claim is refused — and never finds one that does not.
//
// **Soundness around bindings.** A fact names variables by their name, so a
// name bound again drops every fact that mentions it, and a block whose
// bindings this walk cannot see — a lambda's body, a `match` arm — starts with
// no facts and no variables. Losing facts only costs proofs.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{BinaryOp, Block, Expr, Item, Span, Spanned, Stmt, UnaryOp};
use crate::check::{Finding, Severity};
use crate::contracts::{Ledger, Provenance};
use crate::parser::Parsed;

/// How the compiler holds one `assert` (ADR-256 D1-D3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    /// In a `test` block: the test's verdict, checked when it runs (D2).
    ByTheTest,
    /// Proved from what precedes it; no check is emitted.
    Proved,
    /// The precondition of the named function: its callers prove it (D3).
    Precondition(String),
    /// Neither; the program is refused (`NK1202`).
    Refused,
}

/// What the prover found: its refusals, and how each claim it reached is held,
/// keyed as [`crate::check::Checked::claims`] is.
#[derive(Debug, Default)]
pub struct Proved {
    pub findings: Vec<Finding>,
    pub held: BTreeMap<(usize, String), Held>,
}

/// Prove every `assert` of the program. `claims` are the checker's: a call is
/// an `assert` exactly where the checker recognised the prelude's.
pub fn prove(parsed: &Parsed, library: &Ledger, claims: &BTreeSet<(usize, String)>) -> Proved {
    let mut prover = Prover {
        parsed,
        library,
        claims,
        preconditions: BTreeMap::new(),
        collecting: true,
        out: Proved::default(),
        in_test: false,
    };
    // Pass 1: which parameter claims are preconditions (D3). A function's own
    // preconditions depend only on its own body.
    prover.every_body();
    let preconditions = std::mem::take(&mut prover.preconditions);
    prover.out = Proved::default();
    prover.collecting = false;
    prover.preconditions = preconditions;
    // Pass 2: every claim and every call.
    prover.every_body();

    // An `assert` this walk did not reach is not one it may leave to run time.
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
        prover.out.held.insert(key.clone(), Held::Refused);
        prover.out.findings.push(refusal(
            Span::new(key.0, key.0 + 1),
            "This `assert` is somewhere the prover doesn't look yet.".to_string(),
            vec![
                "An `assert` outside a test is proved while the program is built, or the \
                 program doesn't build."
                    .to_string(),
            ],
            "Move the claim out of the expression it's in, onto a line of its own in the \
             function's body.",
        ));
    }
    prover.out
}

/// A function's precondition: the claims its callers prove about its
/// parameters (D3).
#[derive(Debug, Clone)]
struct Precondition {
    claims: Vec<Expr>,
}

struct Prover<'a> {
    parsed: &'a Parsed,
    library: &'a Ledger,
    claims: &'a BTreeSet<(usize, String)>,
    preconditions: BTreeMap<String, Precondition>,
    /// Pass 1 records preconditions and says nothing.
    collecting: bool,
    out: Proved,
    in_test: bool,
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
    facts: Vec<Formula>,
}

impl Scope {
    /// `name` is bound again: nothing known about the old binding holds.
    fn rebind(&mut self, name: &str) {
        let length = length_of(name);
        self.ints.remove(name);
        self.ints.remove(&length);
        self.tainted.remove(name);
        self.locals.insert(name.to_string());
        self.facts
            .retain(|fact| !fact.mentions(name) && !fact.mentions(&length));
    }

    /// `name` is a list or text that does not change, so `name.len()` is a
    /// variable of the proof: a length is never negative, and a literal's is
    /// known.
    fn has_length(&mut self, name: &str, known: Option<i128>) {
        let length = length_of(name);
        self.ints.insert(length.clone());
        self.facts.push(Formula::le(Lin::var(&length).neg()));
        if let Some(n) = known {
            self.facts
                .push(Formula::eq(Lin::var(&length).add_const(-n)));
        }
    }

    /// A block whose bindings this walk cannot see: no facts, no variables.
    fn blind(&self) -> Scope {
        Scope {
            ints: BTreeSet::new(),
            tainted: self.tainted.clone(),
            locals: self.locals.clone(),
            facts: Vec::new(),
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
        // still a test (D2).
        let was_a_test = own
            .as_deref()
            .is_some_and(crate::modules::is_a_test_function);
        let mut scope = Scope::default();
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
                if ty.starts_with('u') {
                    scope.facts.push(Formula::le(Lin::var(&arg_name).neg()));
                }
            }
            if !arg.mutable && has_a_length(ty) {
                scope.has_length(&arg_name, None);
                params.insert(arg_name.clone());
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
                let value_lin = lin(self.parsed, value, scope);
                for name in names {
                    let name = self.parsed.text(*name).to_string();
                    scope.rebind(&name);
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
                        let difference = Lin::var(&name).sub(&value_lin);
                        if let Some(difference) = difference {
                            scope.facts.push(Formula::eq(difference));
                        }
                    } else if typed_whole {
                        scope.ints.insert(name.clone());
                    }
                    if let Some(t) = ty
                        && self.parsed.text(t.name).starts_with('u')
                        && scope.ints.contains(&name)
                    {
                        scope.facts.push(Formula::le(Lin::var(&name).neg()));
                    }
                    match value {
                        Expr::ListLit { items, .. } => {
                            scope.has_length(&name, i128::try_from(items.len()).ok());
                        }
                        Expr::LitStr { .. } => scope.has_length(&name, None),
                        _ if ty
                            .as_ref()
                            .is_some_and(|t| has_a_length(self.parsed.text(t.name))) =>
                        {
                            scope.has_length(&name, None)
                        }
                        _ => {}
                    }
                }
                false
            }
            Stmt::Comptime { name, value, .. } => {
                self.expr(value, span, scope, &nested);
                scope.rebind(self.parsed.text(*name));
                false
            }
            Stmt::Assign { target, value, .. } => {
                self.expr(target, span, scope, &nested);
                self.expr(value, span, scope, &nested);
                if let Expr::Variable(name) = target {
                    let name = self.parsed.text(*name).to_string();
                    let tainted = self.tainted(value, scope);
                    scope.rebind(&name);
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
                    inner.rebind(&name);
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
                    && let (Some(low), Some(high)) =
                        (lin(self.parsed, start, scope), lin(self.parsed, end, scope))
                {
                    let name = self.parsed.text(*only).to_string();
                    inner.ints.insert(name.clone());
                    let n = Lin::var(&name);
                    // low <= n, and n <= high (inclusive) or n <= high - 1.
                    if let Some(below) = low.sub(&n) {
                        inner.facts.push(Formula::le(below));
                    }
                    let last = if *inclusive { high } else { high.add_const(-1) };
                    if let Some(above) = n.sub(&last) {
                        inner.facts.push(Formula::le(above));
                    }
                }
                self.block(body, &mut inner, &nested);
                false
            }
            Stmt::While { cond, body } => {
                self.expr(cond, span, scope, &nested);
                let mut inner = scope.clone();
                if let Some(holds) = formula(self.parsed, cond, &inner, true) {
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
                let holds = formula(self.parsed, cond, scope, true);
                let fails = formula(self.parsed, cond, scope, false);
                let mut then_scope = scope.clone();
                if let Some(holds) = &holds {
                    then_scope.facts.push(holds.clone());
                }
                let then_leaves = self.block(then_branch, &mut then_scope, &nested);
                let else_leaves = match else_branch {
                    Some(block) => {
                        let mut else_scope = scope.clone();
                        if let Some(fails) = &fails {
                            else_scope.facts.push(fails.clone());
                        }
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
                    }
                    (false, true) => {
                        if let Some(holds) = holds {
                            scope.facts.push(holds);
                        }
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
                if let Some(value) = value {
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

    /// An `assert` (D1-D4).
    fn an_assert(&mut self, cond: &Expr, span: Span, scope: &mut Scope, at: &Where) {
        let key = (span.at(), crate::check::argument_shape(cond));
        if self.in_test {
            self.out.held.insert(key, Held::ByTheTest);
            return;
        }
        let claim = formula(self.parsed, cond, scope, true);
        let negation = formula(self.parsed, cond, scope, false);
        if let (Some(claim), Some(negation)) = (&claim, &negation)
            && proves(&scope.facts, negation)
        {
            scope.facts.push(claim.clone());
            self.out.held.insert(key, Held::Proved);
            return;
        }

        // **A claim about parameters the body cannot prove is its callers'**
        // (D3), where it can be: at the top of a free function, naming only
        // parameters.
        let names = names_in(self.parsed, cond);
        let only_params = !names.is_empty() && names.iter().all(|n| at.params.contains(n));
        if let (Some(claim), true, true, None, Some(function)) = (
            &claim,
            only_params,
            at.top,
            at.no_precondition,
            at.function.as_ref(),
        ) {
            if self.collecting {
                let entry = self
                    .preconditions
                    .entry(function.clone())
                    .or_insert_with(|| Precondition { claims: Vec::new() });
                entry.claims.push(cond.clone());
            }
            scope.facts.push(claim.clone());
            self.out
                .held
                .insert(key, Held::Precondition(function.clone()));
            return;
        }

        self.out.held.insert(key, Held::Refused);
        if self.collecting {
            return;
        }
        let written = crate::check::written(self.parsed, cond);
        let tainted: Vec<&String> = names
            .iter()
            .filter(|n| scope.tainted.contains(*n))
            .collect();
        let mut notes = Vec::new();
        let help;
        if let Some(first) = tainted.first() {
            notes.push(format!(
                "`{first}` comes from outside the program, so nothing the compiler can see \
                 says what it holds."
            ));
            help = format!(
                "Check it where it arrives, with a guard the program handles: \
                 `throw BadInput({first}) if !({written})`, or `return … if !({written})`. \
                 After that line, this `assert` is proved."
            );
        } else if claim.is_none() {
            notes.push(
                "The prover reads comparisons of whole numbers built with `+`, `-` and a `*` \
                 by a constant, joined with `&&`, `||` and `!`, over names that don't change."
                    .to_string(),
            );
            help = "Compute the numbers you're claiming something about with `let` first, \
                    or check it with a guard the program handles."
                .to_string();
        } else {
            notes.push("Nothing before this line shows it.".to_string());
            if only_params && let Some(why) = at.no_precondition {
                notes.push(why.to_string());
            } else if only_params && !at.top {
                notes.push(
                    "A claim about parameters is a precondition only at the top of the \
                     function's body."
                        .to_string(),
                );
            }
            help = format!(
                "Add a guard before it, like `return … if !({written})`, so the compiler \
                 can see why it holds."
            );
        }
        notes.push(
            "An `assert` outside a test is proved while the program is built; it is never \
             checked while it runs."
                .to_string(),
        );
        self.out.findings.push(refusal(
            span,
            format!("The compiler can't prove `{written}`."),
            notes,
            &help,
        ));
    }

    /// Walk an expression for what it calls: a call to a function with a
    /// precondition proves it here (D3), a function with one is not handed on
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
        if !self.collecting {
            for value in values {
                if !scope.locals.contains(&value) && self.preconditions.contains_key(&value) {
                    self.out.findings.push(Finding {
                        severity: Severity::Error,
                        span,
                        code: "NK1204",
                        message: format!(
                            "`{value}` has a precondition, so it can't be handed on as a value."
                        ),
                        notes: vec![
                            "Whoever calls it later has to prove the precondition, and the \
                             compiler can't see that call."
                                .to_string(),
                        ],
                        help: Some(format!("Call `{value}` here, or wrap the call in a lambda that checks the arguments first.")),
                        labels: Vec::new(),
                    });
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
                        inner.rebind(param);
                    }
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

    /// A call to a function with a precondition (D3).
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
        for claim in &pre.claims {
            let mut with: BTreeMap<String, Option<Lin>> = BTreeMap::new();
            for (param, arg) in params.iter().zip(args) {
                with.insert(param.clone(), lin(self.parsed, arg, scope));
                // A list handed over carries its length: `p.len()` in the
                // callee's claim is the argument's.
                if let Expr::Variable(name) = arg {
                    let length = length_of(self.parsed.text(*name));
                    with.insert(
                        length_of(param),
                        scope.ints.contains(&length).then(|| Lin::var(&length)),
                    );
                }
            }
            let negation = substituted(self.parsed, claim, &with, false);
            let proved = negation.as_ref().is_some_and(|n| proves(&scope.facts, n));
            if proved {
                continue;
            }
            let written = crate::check::written(self.parsed, claim);
            self.out.findings.push(Finding {
                severity: Severity::Error,
                span,
                code: "NK1203",
                message: format!("This call to `{callee}` doesn't show `{written}`."),
                notes: vec![
                    format!("`{callee}` asserts `{written}` about its parameters, so every call has to prove it."),
                    match negation {
                        None => "An argument here isn't a whole number the prover can read."
                            .to_string(),
                        Some(_) => "Nothing before this call shows it.".to_string(),
                    },
                ],
                help: Some(format!(
                    "Check the arguments before the call, like `return … if !(…)`, so \
                     `{written}` holds for them."
                )),
                labels: Vec::new(),
            });
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

// --- The theory: linear integer arithmetic ---------------------------------

/// `Σ coefficient·name + constant`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lin {
    terms: BTreeMap<String, i128>,
    constant: i128,
}

impl Lin {
    fn constant(c: i128) -> Lin {
        Lin {
            terms: BTreeMap::new(),
            constant: c,
        }
    }

    fn var(name: &str) -> Lin {
        Lin {
            terms: BTreeMap::from([(name.to_string(), 1)]),
            constant: 0,
        }
    }

    fn add(&self, other: &Lin) -> Option<Lin> {
        let mut out = self.clone();
        for (name, k) in &other.terms {
            let entry = out.terms.entry(name.clone()).or_insert(0);
            *entry = entry.checked_add(*k)?;
            if *entry == 0 {
                out.terms.remove(name);
            }
        }
        out.constant = out.constant.checked_add(other.constant)?;
        Some(out)
    }

    fn scale(&self, k: i128) -> Option<Lin> {
        if k == 0 {
            return Some(Lin::constant(0));
        }
        let mut terms = BTreeMap::new();
        for (name, c) in &self.terms {
            terms.insert(name.clone(), c.checked_mul(k)?);
        }
        Some(Lin {
            terms,
            constant: self.constant.checked_mul(k)?,
        })
    }

    fn neg(&self) -> Lin {
        self.scale(-1).unwrap_or_else(|| self.clone())
    }

    fn sub(&self, other: &Lin) -> Option<Lin> {
        self.add(&other.scale(-1)?)
    }

    fn add_const(&self, c: i128) -> Lin {
        let mut out = self.clone();
        out.constant = out.constant.saturating_add(c);
        out
    }

    /// Divide through by the coefficients' common factor and round the bound
    /// towards the integers: `2x + 3 <= 0` is `x + 2 <= 0` over the integers.
    fn tightened(mut self) -> Lin {
        let g = self.terms.values().fold(0i128, |g, k| gcd(g, k.abs()));
        if g > 1 {
            for k in self.terms.values_mut() {
                *k /= g;
            }
            // ceil(constant / g)
            self.constant = self.constant.div_euclid(g)
                + if self.constant.rem_euclid(g) == 0 {
                    0
                } else {
                    1
                };
        }
        self
    }
}

fn gcd(a: i128, b: i128) -> i128 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// A formula over `lin <= 0` atoms, with negation already pushed inward.
#[derive(Debug, Clone)]
enum Formula {
    True,
    False,
    /// `lin <= 0`.
    Le(Lin),
    And(Vec<Formula>),
    Or(Vec<Formula>),
}

impl Formula {
    fn le(lin: Lin) -> Formula {
        Formula::Le(lin)
    }

    fn eq(lin: Lin) -> Formula {
        Formula::And(vec![Formula::Le(lin.clone()), Formula::Le(lin.neg())])
    }

    fn mentions(&self, name: &str) -> bool {
        match self {
            Formula::True | Formula::False => false,
            Formula::Le(lin) => lin.terms.contains_key(name),
            Formula::And(parts) | Formula::Or(parts) => parts.iter().any(|p| p.mentions(name)),
        }
    }
}

/// A whole-number expression, where it is one this prover reads.
fn lin(parsed: &Parsed, expr: &Expr, scope: &Scope) -> Option<Lin> {
    lin_with(parsed, expr, &|name| {
        scope.ints.contains(name).then(|| Lin::var(name))
    })
}

fn lin_with(parsed: &Parsed, expr: &Expr, var: &dyn Fn(&str) -> Option<Lin>) -> Option<Lin> {
    match expr {
        Expr::LitInt { value, negative } => {
            Some(Lin::constant(crate::ast::int_value(*value, *negative)))
        }
        Expr::Variable(name) => var(parsed.text(*name)),
        // `xs.len()` of a list that does not change is a variable of its own.
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } if args.is_empty() && parsed.text(*method) == "len" => match &**receiver {
            Expr::Variable(name) => var(&length_of(parsed.text(*name))),
            _ => None,
        },
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
        } => Some(lin_with(parsed, expr, var)?.neg()),
        Expr::Binary { op, lhs, rhs, .. } => {
            let l = lin_with(parsed, lhs, var)?;
            let r = lin_with(parsed, rhs, var)?;
            match op {
                BinaryOp::Add => l.add(&r),
                BinaryOp::Sub => l.sub(&r),
                BinaryOp::Mul if l.terms.is_empty() => r.scale(l.constant),
                BinaryOp::Mul if r.terms.is_empty() => l.scale(r.constant),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The claim `expr` as a formula, or its negation where `holds` is false.
fn formula(parsed: &Parsed, expr: &Expr, scope: &Scope, holds: bool) -> Option<Formula> {
    formula_with(
        parsed,
        expr,
        &|name| scope.ints.contains(name).then(|| Lin::var(name)),
        holds,
    )
}

/// A callee's claim with the call's arguments in place of its parameters.
fn substituted(
    parsed: &Parsed,
    expr: &Expr,
    with: &BTreeMap<String, Option<Lin>>,
    holds: bool,
) -> Option<Formula> {
    formula_with(
        parsed,
        expr,
        &|name| with.get(name).cloned().flatten(),
        holds,
    )
}

fn formula_with(
    parsed: &Parsed,
    expr: &Expr,
    var: &dyn Fn(&str) -> Option<Lin>,
    holds: bool,
) -> Option<Formula> {
    match expr {
        Expr::LitBool(b) => Some(if *b == holds {
            Formula::True
        } else {
            Formula::False
        }),
        Expr::Unary {
            op: UnaryOp::Not,
            expr,
        } => formula_with(parsed, expr, var, !holds),
        Expr::Binary { op, lhs, rhs, .. } => match op {
            BinaryOp::And | BinaryOp::Or => {
                let l = formula_with(parsed, lhs, var, holds)?;
                let r = formula_with(parsed, rhs, var, holds)?;
                let conjunction = matches!(op, BinaryOp::And) == holds;
                Some(if conjunction {
                    Formula::And(vec![l, r])
                } else {
                    Formula::Or(vec![l, r])
                })
            }
            BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge
            | BinaryOp::Eq
            | BinaryOp::Ne => {
                let a = lin_with(parsed, lhs, var)?;
                let b = lin_with(parsed, rhs, var)?;
                let op = if holds { *op } else { negated(*op) };
                let a_minus_b = a.sub(&b)?;
                let b_minus_a = b.sub(&a)?;
                Some(match op {
                    // a < b  is  a - b + 1 <= 0 over the integers.
                    BinaryOp::Lt => Formula::Le(a_minus_b.add_const(1)),
                    BinaryOp::Le => Formula::Le(a_minus_b),
                    BinaryOp::Gt => Formula::Le(b_minus_a.add_const(1)),
                    BinaryOp::Ge => Formula::Le(b_minus_a),
                    BinaryOp::Eq => Formula::eq(a_minus_b),
                    _ => Formula::Or(vec![
                        Formula::Le(a_minus_b.add_const(1)),
                        Formula::Le(b_minus_a.add_const(1)),
                    ]),
                })
            }
            _ => None,
        },
        _ => None,
    }
}

fn negated(op: BinaryOp) -> BinaryOp {
    match op {
        BinaryOp::Lt => BinaryOp::Ge,
        BinaryOp::Le => BinaryOp::Gt,
        BinaryOp::Gt => BinaryOp::Le,
        BinaryOp::Ge => BinaryOp::Lt,
        BinaryOp::Eq => BinaryOp::Ne,
        _ => BinaryOp::Eq,
    }
}

/// How many cases a proof may split into before it gives up (D6).
const CASES: usize = 256;
/// How many bounds an elimination may hold before it gives up (D6).
const BOUNDS: usize = 4096;

/// Whether `facts` rule out `negation`: every case of the two together is
/// contradictory.
fn proves(facts: &[Formula], negation: &Formula) -> bool {
    let mut all = facts.to_vec();
    all.push(negation.clone());
    let Some(cases) = cases(&Formula::And(all)) else {
        return false;
    };
    cases.into_iter().all(contradictory)
}

/// The formula as a disjunction of conjunctions of bounds, or `None` past
/// [`CASES`].
fn cases(formula: &Formula) -> Option<Vec<Vec<Lin>>> {
    match formula {
        Formula::True => Some(vec![Vec::new()]),
        Formula::False => Some(Vec::new()),
        Formula::Le(lin) => Some(vec![vec![lin.clone()]]),
        Formula::Or(parts) => {
            let mut out = Vec::new();
            for part in parts {
                out.extend(cases(part)?);
                if out.len() > CASES {
                    return None;
                }
            }
            Some(out)
        }
        Formula::And(parts) => {
            let mut out: Vec<Vec<Lin>> = vec![Vec::new()];
            for part in parts {
                let each = cases(part)?;
                let mut next = Vec::new();
                for left in &out {
                    for right in &each {
                        let mut both = left.clone();
                        both.extend(right.iter().cloned());
                        next.push(both);
                        if next.len() > CASES {
                            return None;
                        }
                    }
                }
                out = next;
            }
            Some(out)
        }
    }
}

/// Fourier-Motzkin with integer tightening: whether the bounds, all `<= 0`,
/// have no integer solution. `false` where it cannot tell.
fn contradictory(bounds: Vec<Lin>) -> bool {
    let mut bounds: Vec<Lin> = bounds.into_iter().map(Lin::tightened).collect();
    loop {
        // A bound with no variables left is a verdict: `c <= 0`.
        if bounds.iter().any(|b| b.terms.is_empty() && b.constant > 0) {
            return true;
        }
        bounds.retain(|b| !b.terms.is_empty());
        bounds.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        bounds.dedup();
        let names: BTreeSet<String> = bounds
            .iter()
            .flat_map(|b| b.terms.keys().cloned())
            .collect();
        // Eliminate the name whose elimination makes the fewest new bounds.
        let Some(name) = names.into_iter().min_by_key(|name| {
            let up = bounds
                .iter()
                .filter(|b| b.terms.get(name).is_some_and(|k| *k > 0))
                .count();
            let down = bounds
                .iter()
                .filter(|b| b.terms.get(name).is_some_and(|k| *k < 0))
                .count();
            up * down
        }) else {
            return false;
        };
        let (with, without): (Vec<Lin>, Vec<Lin>) = bounds
            .into_iter()
            .partition(|b| b.terms.contains_key(&name));
        let (up, down): (Vec<Lin>, Vec<Lin>) = with.into_iter().partition(|b| b.terms[&name] > 0);
        let mut next = without;
        for u in &up {
            for d in &down {
                let a = u.terms[&name];
                let b = -d.terms[&name];
                // b·u + a·d cancels `name`; both are `<= 0`, so is the sum.
                let (Some(left), Some(right)) = (u.scale(b), d.scale(a)) else {
                    return false;
                };
                let Some(sum) = left.add(&right) else {
                    return false;
                };
                next.push(sum.tightened());
                if next.len() > BOUNDS {
                    return false;
                }
            }
        }
        bounds = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le(terms: &[(&str, i128)], constant: i128) -> Lin {
        Lin {
            terms: terms.iter().map(|(n, k)| (n.to_string(), *k)).collect(),
            constant,
        }
    }

    #[test]
    fn a_bound_and_its_opposite_contradict() {
        // x - 5 <= 0 and 6 - x <= 0: x <= 5 and x >= 6.
        assert!(contradictory(vec![
            le(&[("x", 1)], -5),
            le(&[("x", -1)], 6)
        ]));
        // x <= 5 and x >= 5 is x = 5: no contradiction.
        assert!(!contradictory(vec![
            le(&[("x", 1)], -5),
            le(&[("x", -1)], 5)
        ]));
    }

    #[test]
    fn integer_tightening_finds_what_the_rationals_miss() {
        // 2x <= 1 and 2x >= 1 has x = 1/2 over the rationals and nothing over
        // the integers.
        assert!(contradictory(vec![
            le(&[("x", 2)], -1),
            le(&[("x", -2)], 1)
        ]));
    }

    #[test]
    fn a_chain_of_bounds_is_followed() {
        // a <= b, b <= c, c < a.
        assert!(contradictory(vec![
            le(&[("a", 1), ("b", -1)], 0),
            le(&[("b", 1), ("c", -1)], 0),
            le(&[("c", 1), ("a", -1)], 1),
        ]));
    }
}
