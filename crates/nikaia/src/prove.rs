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
    /// The precondition of the named function (D5): the body assumes it; a
    /// call proves it or carries the check, and a caller the compiler can't
    /// see reaches the entry that checks it (ADR-266 D7). The number is how
    /// many distinct calls carry a check.
    Precondition(String, usize),
    /// Not proved: the condition is checked where it is reached (D4), for
    /// the reason given.
    AtRunTime(String),
    /// A claim about data from outside the program (D6): `NK1202`.
    Refused,
}

/// A precondition checked before a function's body runs (ADR-264 D5,
/// ADR-266 D7): the condition as the language below writes it, over names in
/// scope where it stands, and what it says when it fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// A `bool` expression of the language below, in `i128` so that the
    /// check itself cannot overflow.
    pub rust: String,
    /// `precondition of `f`: `x > 2``, and where it came from.
    pub written: String,
    /// The `assert`'s `message:`, where it wrote one.
    pub message: Option<String>,
    /// The values a failure shows (ADR-264 D2): each parameter the condition
    /// reads, by name, and the expression of the language below that is its
    /// value where the check stands.
    pub operands: Vec<(String, String)>,
}

/// **How a call reaches a function with a precondition** (ADR-266 D7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// Every precondition is proved here: the unchecked entry.
    Proved,
    /// The call checks what it does not prove, with its arguments in place
    /// of the parameters, then takes the unchecked entry.
    Checked(Vec<Check>),
    /// The call cannot check it - an argument the prover doesn't read - and
    /// reaches the checked entry.
    Through,
}

impl Reach {
    /// Two calls that read the same are one key: the more careful answer
    /// holds for both.
    fn joined(self, other: Reach) -> Reach {
        match (self, other) {
            (Reach::Through, _) | (_, Reach::Through) => Reach::Through,
            (Reach::Checked(mut a), Reach::Checked(b)) => {
                for check in b {
                    if !a.contains(&check) {
                        a.push(check);
                    }
                }
                Reach::Checked(a)
            }
            (Reach::Checked(a), Reach::Proved) | (Reach::Proved, Reach::Checked(a)) => {
                Reach::Checked(a)
            }
            (Reach::Proved, Reach::Proved) => Reach::Proved,
        }
    }
}

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
    /// By the ledger's key: what the ledger publishes (ADR-266 D5).
    pub published: BTreeMap<String, Published>,
}

/// **A function's contract as the ledger writes it** (ADR-266 D5), in the
/// language's syntax.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Published {
    /// Over the parameters.
    pub requires: Vec<String>,
    /// Over the parameters and `result`.
    pub ensures: Vec<String>,
    /// For each of `requires`, then of `ensures`: the `assert` it came from.
    pub from: Vec<String>,
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
        foreign: BTreeMap::new(),
        claims,
        preconditions: BTreeMap::new(),
        candidates: BTreeMap::new(),
        postconditions: BTreeMap::new(),
        broken: BTreeSet::new(),
        before_return: BTreeMap::new(),
        collecting: true,
        out: Proved::default(),
        in_test: false,
        arena: Arena::new(),
    };
    // Pass 1: which parameter claims are preconditions (D5). A function's own
    // preconditions depend only on its own body.
    prover.every_body();
    let preconditions = std::mem::take(&mut prover.preconditions);
    prover.collecting = false;
    prover.preconditions = preconditions;
    // **Pass 2, until nothing changes: every claim and every call**, with
    // the postconditions still standing (ADR-266 D4). Each one starts as a
    // candidate and is struck where an exit does not show it; a proof at one
    // exit may lean on another function's postcondition, so the walk repeats
    // until no candidate falls. The last walk is the answer: it ran with
    // exactly the postconditions that hold.
    prover.postconditions = std::mem::take(&mut prover.candidates);
    loop {
        prover.out = Proved::default();
        prover.broken.clear();
        prover.every_body();
        if prover.broken.is_empty() {
            break;
        }
        for (function, index) in std::mem::take(&mut prover.broken).into_iter().rev() {
            if let Some(posts) = prover.postconditions.get_mut(&function) {
                posts.remove(index);
            }
        }
    }

    // **Every function with a precondition has a checked entry** (ADR-266
    // D7), whatever its calls do: a caller the compiler doesn't see - a
    // function value, another package, the language below - reaches it.
    let entries: BTreeMap<String, Vec<Check>> = prover
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
                        written: claim.message(function),
                        message: claim.message.clone(),
                        operands,
                    }
                })
                .collect();
            (function.clone(), checks)
        })
        .collect();
    prover.out.entries = entries;
    // **What the ledger publishes** (ADR-266 D5): every precondition and
    // every postcondition still standing, and the `assert` each came from.
    let mut published: BTreeMap<String, Published> = BTreeMap::new();
    for (function, pre) in &prover.preconditions {
        let entry = published.entry(function.clone()).or_default();
        for claim in &pre.claims {
            entry.requires.push(ledger_text(&prover.arena, claim.term));
            entry.from.push(format!("assert({})", claim.written));
        }
    }
    for (function, posts) in &prover.postconditions {
        if posts.is_empty() {
            continue;
        }
        let entry = published.entry(function.clone()).or_default();
        for post in posts {
            entry.ensures.push(ledger_text(&prover.arena, post.term));
        }
    }
    for (function, entry) in published.iter_mut() {
        if let Some(posts) = prover.postconditions.get(function) {
            entry
                .from
                .extend(posts.iter().map(|p| format!("assert({})", p.written)));
        }
    }
    prover.out.published = published;
    let mut checked: BTreeMap<String, usize> = BTreeMap::new();
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

/// A function's precondition: the claims its callers prove about its
/// parameters (D5).
#[derive(Debug, Clone)]
struct Precondition {
    claims: Vec<PreClaim>,
}

impl PreClaim {
    /// What a failed check of it says: the condition, and where a computed
    /// one came from (ADR-266 D8).
    fn message(&self, function: &str) -> String {
        match &self.computed {
            Some(computed) => format!(
                "precondition of `{function}`: `{computed}`, from `assert({})`",
                self.written
            ),
            None => format!("precondition of `{function}`: `{}`", self.written),
        }
    }
}

/// **Another package's function, as its ledger states it** (ADR-266 D5):
/// its parameters, whether it hands back a whole number, and its contract
/// read back into terms.
#[derive(Debug, Clone)]
struct Foreign {
    params: Vec<String>,
    whole_result: bool,
    requires: Vec<PreClaim>,
    ensures: Vec<PostClaim>,
}

/// One claim of a postcondition (ADR-266 D4).
#[derive(Debug, Clone)]
struct PostClaim {
    /// Over `result` and the function's parameters.
    term: TermId,
    /// The `assert`'s condition, written.
    written: String,
}

/// The name a postcondition calls the value a function hands back.
const RESULT: &str = "result";

/// One claim of a precondition (ADR-266 D2).
#[derive(Debug, Clone)]
struct PreClaim {
    /// The condition at the function's entry, over its parameters: the
    /// `assert`'s claim carried back through the body - `mode == 1 → x > 1`
    /// for `assert(y > 0)` after `let y = x - 1` inside `if mode == 1`.
    term: TermId,
    /// The `assert`'s condition, written, for messages.
    written: String,
    /// The condition at the entry as a reader writes it, where it is not the
    /// claim as written: `mode == 1 → x - 1 > 0` (ADR-266 D8).
    computed: Option<String>,
    /// The `assert`'s `message:`, where it is text as written.
    message: Option<String>,
}

struct Prover<'a> {
    parsed: &'a Parsed,
    /// The program's ledger: another package's functions, with their
    /// `requires` and `ensures` (ADR-266 D5).
    own: &'a Ledger,
    library: &'a Ledger,
    /// Another package's contracts as terms, read once per function.
    foreign: BTreeMap<String, Option<Foreign>>,
    claims: &'a BTreeSet<(usize, String)>,
    preconditions: BTreeMap<String, Precondition>,
    /// Pass 1's postconditions, before any exit is asked (ADR-266 D4).
    candidates: BTreeMap<String, Vec<PostClaim>>,
    /// The postconditions standing in this walk: callers read them.
    postconditions: BTreeMap<String, Vec<PostClaim>>,
    /// The postconditions an exit of this walk did not show, by function and
    /// place in its list.
    broken: BTreeSet<(String, usize)>,
    /// The `assert`s that stand directly before a `return name`, by the
    /// statement's start: the claims a postcondition is made from.
    before_return: BTreeMap<usize, String>,
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
        if let Some(n) = known.and_then(|n| i64::try_from(n).ok()) {
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
    /// A free function's body, called by name: what has a postcondition.
    free: bool,
    /// Inside a lambda, whose `return` is not the function's.
    lambda: bool,
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
                    self.in_test = true;
                    let at = Where {
                        function: None,
                        params: BTreeSet::new(),
                        no_precondition: Some("a test has no callers"),
                        top: true,
                        free: false,
                        lambda: false,
                    };
                    self.block(body, &mut Scope::default(), &at);
                    self.in_test = false;
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
        };
        self.in_test = was_a_test;
        let leaves = self.block(body, &mut scope, &at);
        self.in_test = false;
        // A body that can end without a `return` has an exit no candidate was
        // shown at.
        if !leaves
            && !self.collecting
            && let Some(own) = own
        {
            for index in 0..self.postconditions.get(&own).map_or(0, Vec::len) {
                self.broken.insert((own.clone(), index));
            }
        }
    }

    /// Walk a block; whether it always leaves.
    fn block(&mut self, block: &Block, scope: &mut Scope, at: &Where) -> bool {
        for (index, stmt) in block.stmts.iter().enumerate() {
            if self.collecting
                && let Some(next) = block.stmts.get(index + 1)
                && let Stmt::Return(Some(Expr::Variable(name))) = &next.node
            {
                self.before_return
                    .insert(stmt.span.at(), self.parsed.text(*name).to_string());
            }
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
                // **A call's postconditions are facts about its result**
                // (ADR-266 D4): `let r = f(x)` knows what `f` ensures.
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
                    } else if typed_whole || called.as_ref().is_some_and(|(whole, _)| *whole) {
                        scope.ints.insert(name.clone());
                    }
                    if let Some((true, facts)) = &called {
                        scope.facts.extend(facts.iter().copied());
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
        if self.in_test {
            self.out.held.insert(key, Held::ByTheTest);
            return;
        }
        let claim = claim(&mut self.arena, self.parsed, cond, scope);
        // A claim about the value the next statement returns is a candidate
        // postcondition (ADR-266 D4).
        if self.collecting
            && at.free
            && !at.lambda
            && let (Some(claim), Some(function)) = (claim, at.function.clone())
            && let Some(returned) = self.before_return.get(&span.at()).cloned()
            && let Some(term) = self.postcondition_at(claim, &returned, scope, at)
        {
            self.candidates
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
                let as_written = only_params && at.top;
                let pre = PreClaim {
                    term,
                    computed: (!as_written).then(|| term_text(&self.arena, term)),
                    written: crate::check::written(self.parsed, cond),
                    message,
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
            if let Some(pre) = self.preconditions.get(&callee).cloned() {
                let Some(Item::Fn { args: params, .. }) = self.function_named(&callee) else {
                    continue;
                };
                let params: Vec<String> = params
                    .iter()
                    .map(|p| self.parsed.text(p.name).to_string())
                    .collect();
                self.a_call(&callee, &params, &pre.claims, &args, span, scope);
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
                    lambda,
                    ..at.clone()
                },
            );
        }
    }

    /// A call to a function with a precondition (D5).
    fn a_call(
        &mut self,
        callee: &str,
        params: &[String],
        claims: &[PreClaim],
        args: &[Expr],
        span: Span,
        scope: &Scope,
    ) {
        if self.collecting {
            return;
        }
        let mut reach = Reach::Proved;
        for pre in claims {
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
                let given: BTreeMap<String, i64> = with
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
            // once more for the check changes nothing. Where one is not, the
            // call reaches the checked entry (ADR-266 D7).
            reach = reach.joined(match goal {
                Some(goal) => Reach::Checked(vec![Check {
                    rust: rust_of(&self.arena, goal),
                    written: pre.message(callee),
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
            });
        }
        let key = call_key(self.parsed, callee, args);
        let joined = match self.out.reaches.remove(&key) {
            Some(before) => before.joined(reach),
            None => reach,
        };
        self.out.reaches.insert(key, joined);
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

    /// `claim` as a postcondition: `returned` is `result`, every other name its
    /// value at the entry; `None` where that leaves a name that is not a
    /// parameter or the length of one.
    fn postcondition_at(
        &mut self,
        claim: TermId,
        returned: &str,
        scope: &Scope,
        at: &Where,
    ) -> Option<TermId> {
        let mut read = BTreeSet::new();
        self.arena.variables(claim, &mut read);
        let result = self.arena.var(RESULT);
        let values: Option<BTreeMap<String, TermId>> = read
            .into_iter()
            .map(|name| {
                let value = if name == returned {
                    result
                } else {
                    *scope.entry.get(&name)?
                };
                Some((name, value))
            })
            .collect();
        let term = self.arena.substitute(claim, &values?);
        let mut read = BTreeSet::new();
        self.arena.variables(term, &mut read);
        read.iter()
            .all(|name| name == RESULT || is_a_parameter(name, at))
            .then_some(term)
    }

    /// **An exit of a free function**: each postcondition standing has to be
    /// shown here, with the value handed back as `result`, or it falls
    /// (ADR-266 D4). A parameter bound again before the exit reads something
    /// else than at the entry, and that is a fall too.
    fn exit(&mut self, value: &Expr, scope: &Scope, at: &Where) {
        if self.collecting || at.lambda || !at.free {
            return;
        }
        let Some(function) = at.function.clone() else {
            return;
        };
        let Some(posts) = self.postconditions.get(&function).cloned() else {
            return;
        };
        let returned = lin(&mut self.arena, self.parsed, value, scope);
        let unshadowed = at.params.iter().all(|p| {
            let key = if scope.entry.contains_key(p) {
                p.clone()
            } else {
                length_of(p)
            };
            scope
                .entry
                .get(&key)
                .is_some_and(|e| *self.arena.get(*e) == nikaia_logic::Term::Var(key.clone()))
        });
        for (index, post) in posts.iter().enumerate() {
            let shown = unshadowed
                && returned.is_some_and(|returned| {
                    let given = BTreeMap::from([(RESULT.to_string(), returned)]);
                    let goal = self.arena.substitute(post.term, &given);
                    self.proves(&scope.facts, goal).is_ok()
                });
            if !shown {
                self.broken.insert((function.clone(), index));
            }
        }
    }

    /// What a call's postconditions say of the name its result is bound to:
    /// each with the call's arguments in place of the parameters and the name
    /// in place of `result`. Computed before the name is bound, so an argument
    /// that reads the name it shadows makes none.
    fn postconditions_of(
        &mut self,
        callee: &str,
        args: &[Expr],
        bound: &str,
        scope: &Scope,
    ) -> Vec<TermId> {
        let (posts, params): (Vec<PostClaim>, Vec<String>) =
            match self.postconditions.get(callee).cloned() {
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
        let mut with: BTreeMap<String, TermId> = BTreeMap::new();
        for (param, arg) in params.iter().zip(args) {
            if let Some(term) = lin(&mut self.arena, self.parsed, arg, scope) {
                with.insert(param.clone(), term);
            }
            if let Expr::Variable(name) = arg {
                let length = length_of(self.parsed.text(*name));
                if scope.ints.contains(&length) {
                    let known = self.arena.var(&length);
                    with.insert(length_of(param), known);
                }
            }
        }
        if with.values().any(|term| {
            self.arena.mentions(*term, bound) || self.arena.mentions(*term, &length_of(bound))
        }) {
            return Vec::new();
        }
        let result = self.arena.var(bound);
        with.insert(RESULT.to_string(), result);
        posts
            .iter()
            .filter_map(|post| {
                let mut read = BTreeSet::new();
                self.arena.variables(post.term, &mut read);
                read.iter()
                    .all(|name| with.contains_key(name))
                    .then(|| self.arena.substitute(post.term, &with))
            })
            .collect()
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

    /// **Another package's contract, read back into terms** (ADR-266 D5):
    /// each `requires` and `ensures` the ledger states, parsed as the
    /// language's own syntax over the parameters' names. `None` where the
    /// ledger has no such function; a condition that does not read back is
    /// left out, which only ever proves less.
    fn foreign_contract(&mut self, key: &str) -> Option<Foreign> {
        if let Some(known) = self.foreign.get(key) {
            return known.clone();
        }
        let contract = self.own.functions.get(key)?.clone();
        let signature = contract.signature.clone();
        let mut params = Vec::new();
        let mut names = BTreeSet::from([RESULT.to_string()]);
        for (name, ty) in signature
            .as_ref()
            .map(|s| s.params.clone())
            .unwrap_or_default()
        {
            if let crate::contracts::ty::Ty::Named {
                name: type_name,
                args,
                view,
            } = &ty
            {
                if args.is_empty() && !view && is_whole_number(type_name) {
                    names.insert(name.clone());
                }
                if has_a_length(type_name) {
                    names.insert(length_of(&name));
                }
            }
            params.push(name);
        }
        let whole_result = matches!(
            signature.as_ref().and_then(|s| s.result.clone()),
            Some(crate::contracts::ty::Ty::Named { name, args, view })
                if args.is_empty() && !view && is_whole_number(&name)
        );
        let assert_of = |at: usize| {
            contract
                .from
                .get(at)
                .and_then(|from| from.strip_prefix("assert("))
                .and_then(|from| from.strip_suffix(')'))
                .unwrap_or_default()
                .to_string()
        };
        let mut requires = Vec::new();
        for (at, text) in contract.requires.iter().enumerate() {
            if let Some(term) = condition(&mut self.arena, text, &names) {
                let written = assert_of(at);
                requires.push(PreClaim {
                    term,
                    computed: (written != *text).then(|| text.clone()),
                    written,
                    message: None,
                });
            }
        }
        let mut ensures = Vec::new();
        for (at, text) in contract.ensures.iter().enumerate() {
            if let Some(term) = condition(&mut self.arena, text, &names) {
                ensures.push(PostClaim {
                    term,
                    written: assert_of(contract.requires.len() + at),
                });
            }
        }
        let foreign = Foreign {
            params,
            whole_result,
            requires,
            ensures,
        };
        self.foreign.insert(key.to_string(), Some(foreign.clone()));
        Some(foreign)
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
        let parameters_only = read.iter().all(|name| is_a_parameter(name, at));
        (parameters_only && self.arena.size(term) <= PRECONDITION_TERMS).then_some(term)
    }

    /// The values that show a claim false every time it is reached
    /// (ADR-264 D8): the solver proves, with a checked certificate, that the
    /// facts rule the claim out - so it is false in every state the program
    /// reaches it in - and a model of the facts, checked by evaluating it,
    /// gives values for the claim's names. A model alone would not do: the
    /// facts are true but not all that is true, so a value they allow need not
    /// be one the program reaches. `None` where either is missing.
    fn refutes(&mut self, facts: &[TermId], claim: TermId) -> Option<BTreeMap<String, i64>> {
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
        let values: BTreeMap<String, i64> = names
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

/// **A term as the language below writes it**, for a check that is not the
/// `assert` as written: every number in `i128`, so that the check cannot
/// overflow where the claim's own arithmetic would have stopped the program
/// (Part I 2.2) - the prover reads numbers without a limit, and so does this.
/// A name is the escaped name the emitter writes; `xs.len()` the length.
fn rust_of(arena: &Arena, id: TermId) -> String {
    use nikaia_logic::Term;
    let r = |id: &TermId| rust_of(arena, *id);
    let joined = |op: &str, parts: &[TermId], empty: &str| match parts {
        [] => empty.to_string(),
        _ => format!("({})", parts.iter().map(r).collect::<Vec<_>>().join(op)),
    };
    match arena.get(id) {
        Term::Bool(b) => b.to_string(),
        Term::Int(n) => format!("({n}i128)"),
        Term::Var(name) => rust_of_name(name),
        Term::Neg(a) => format!("(-{})", r(a)),
        Term::Not(a) => format!("(!{})", r(a)),
        Term::Add(a, b) => format!("({} + {})", r(a), r(b)),
        Term::Sub(a, b) => format!("({} - {})", r(a), r(b)),
        Term::Mul(a, b) => format!("({} * {})", r(a), r(b)),
        Term::Le(a, b) => format!("({} <= {})", r(a), r(b)),
        Term::Lt(a, b) => format!("({} < {})", r(a), r(b)),
        Term::Ge(a, b) => format!("({} >= {})", r(a), r(b)),
        Term::Gt(a, b) => format!("({} > {})", r(a), r(b)),
        Term::Eq(a, b) => format!("({} == {})", r(a), r(b)),
        Term::Ne(a, b) => format!("({} != {})", r(a), r(b)),
        Term::And(parts) => joined(" && ", parts, "true"),
        Term::Or(parts) => joined(" || ", parts, "false"),
    }
}

/// A name of the proof as the language below reads it, in `i128`.
fn rust_of_name(name: &str) -> String {
    match name.strip_suffix(".len()") {
        Some(base) => format!("({}.len() as i128)", crate::emit::escaped(base)),
        None => format!("({}.clone() as i128)", crate::emit::escaped(name)),
    }
}

/// A term as a reader writes it, in the language's operators, with `→` for
/// the implication a branch makes of a precondition: `mode == 1 → x - 1 > 0`.
fn term_text(arena: &Arena, id: TermId) -> String {
    term_text_as(arena, id, true)
}

/// A term in the language's own syntax, as the ledger writes it and reads it
/// back ([ADR-251](../../docs/specification/adr/adr-251.md) D4): the
/// implication is `!(taken) || claim`.
fn ledger_text(arena: &Arena, id: TermId) -> String {
    term_text_as(arena, id, false)
}

fn term_text_as(arena: &Arena, id: TermId, arrow: bool) -> String {
    use nikaia_logic::Term;
    // How tightly each form binds, for parentheses: higher binds tighter.
    fn rank(term: &Term, arrow: bool) -> u8 {
        match term {
            Term::Or(parts) if arrow && is_implication(parts) => 0,
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
    fn inner(arena: &Arena, id: TermId, at_least: u8, arrow: bool) -> String {
        let term = arena.get(id);
        let text = whole(arena, id, arrow);
        if rank(term, arrow) < at_least {
            format!("({text})")
        } else {
            text
        }
    }
    fn whole(arena: &Arena, id: TermId, arrow: bool) -> String {
        let term = arena.get(id);
        let r = rank(term, arrow);
        let binary = |op: &str, a: &TermId, b: &TermId| {
            format!(
                "{} {op} {}",
                inner(arena, *a, r, arrow),
                inner(arena, *b, r + 1, arrow)
            )
        };
        let joined = |op: &str, parts: &[TermId]| {
            parts
                .iter()
                .map(|p| inner(arena, *p, r + 1, arrow))
                .collect::<Vec<_>>()
                .join(op)
        };
        match term {
            Term::Bool(b) => b.to_string(),
            Term::Int(n) => n.to_string(),
            Term::Var(name) => name.clone(),
            Term::Neg(a) => format!("-{}", inner(arena, *a, r, arrow)),
            Term::Not(a) => format!("!{}", inner(arena, *a, r, arrow)),
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
            Term::Or(parts) if arrow && is_implication(parts) => match arena.get(parts[0]) {
                Term::Not(taken) => format!(
                    "{} → {}",
                    inner(arena, *taken, 1, arrow),
                    inner(arena, parts[1], 1, arrow)
                ),
                _ => joined(" || ", parts),
            },
            Term::Or(parts) => joined(" || ", parts),
            Term::And(parts) => joined(" && ", parts),
        }
    }
    whole(arena, id, arrow)
}

/// Values as a reader writes them: `` `x` is 5, `xs.len()` is 0 ``.
fn shown(values: &BTreeMap<String, i64>) -> String {
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

/// **What a `pub` function's contract changed in the breaking direction**
/// since the committed ledger (ADR-266 D6), as warnings: a precondition the
/// old one does not imply - a caller that showed the old may now stop where
/// it is checked - and a postcondition the new one does not imply - a caller's
/// proof that leaned on it may no longer hold. The solver decides each
/// direction; where it cannot show the implication, the change is said.
/// `mine` says which entries are this package's own.
pub fn changed_contracts(now: &Ledger, committed: &Ledger, mine: impl Fn(&str) -> bool) -> String {
    let mut out = String::new();
    for (key, new) in &now.functions {
        let Some(old) = committed.functions.get(key) else {
            continue;
        };
        if !new.public || !mine(key) {
            continue;
        }
        let mut names = BTreeSet::from([RESULT.to_string()]);
        for (name, ty) in new
            .signature
            .as_ref()
            .map(|s| s.params.clone())
            .unwrap_or_default()
        {
            if let crate::contracts::ty::Ty::Named {
                name: type_name, ..
            } = &ty
                && has_a_length(type_name)
            {
                names.insert(length_of(&name));
            }
            names.insert(name);
        }
        let shown = |list: &[String]| match list {
            [] => "nothing".to_string(),
            _ => list
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(" and "),
        };
        if !implies(&old.requires, &new.requires, &names) {
            out.push_str(&format!(
                "warning[NK1208]: `{key}` asks more of its callers than the committed ledger says.\n   \
                 = note: it required {}, and now requires {}.\n   \
                 = note: a caller that showed what it required before may now stop where it is \
                 checked.\n   \
                 = help: if that is meant, commit `nikaia.contracts`, and this is not said again.\n",
                shown(&old.requires),
                shown(&new.requires),
            ));
        }
        if !implies(&new.ensures, &old.ensures, &names) {
            out.push_str(&format!(
                "warning[NK1208]: `{key}` promises less than the committed ledger says.\n   \
                 = note: it ensured {}, and now ensures {}.\n   \
                 = note: a caller's proof that relied on what it ensured may no longer hold.\n   \
                 = help: if that is meant, commit `nikaia.contracts`, and this is not said again.\n",
                shown(&old.ensures),
                shown(&new.ensures),
            ));
        }
    }
    out
}

/// Whether the conditions `from` imply every one of `to`, by the reference
/// solver with a checked certificate; `false` where it cannot show it, or a
/// condition does not read back.
fn implies(from: &[String], to: &[String], names: &BTreeSet<String>) -> bool {
    if to.iter().all(|c| from.contains(c)) {
        return true;
    }
    let mut arena = Arena::new();
    let Some(facts) = from
        .iter()
        .map(|c| condition(&mut arena, c, names))
        .collect::<Option<Vec<TermId>>>()
    else {
        return false;
    };
    let Some(goals) = to
        .iter()
        .map(|c| condition(&mut arena, c, names))
        .collect::<Option<Vec<TermId>>>()
    else {
        return false;
    };
    let goal = arena.and(goals);
    let query = Query {
        arena: &arena,
        facts: &facts,
        goal,
    };
    match FourierMotzkin.check(&query, &Budget::default()) {
        Answer::Proved { certificate } => verify(&query, &certificate).is_ok(),
        _ => false,
    }
}

/// **A condition the ledger states, read back** (ADR-266 D5): the text is
/// the language's own syntax, so the compiler's parser reads it, inside an
/// `assert` of a function nobody calls, and the prover's own reading turns it
/// into a term. Only `names` are variables of it.
fn condition(arena: &mut Arena, text: &str, names: &BTreeSet<String>) -> Option<TermId> {
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

/// Whether a name of the proof is one of the function's parameters, or the
/// length of one.
fn is_a_parameter(name: &str, at: &Where) -> bool {
    at.params.contains(name)
        || name
            .strip_suffix(".len()")
            .is_some_and(|base| at.params.contains(base))
}

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
            // A literal no `i64` holds is outside what the solver reads.
            Some(arena.int(i64::try_from(crate::ast::int_value(*value, *negative)).ok()?))
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
