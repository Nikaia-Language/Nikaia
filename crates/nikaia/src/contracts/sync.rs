// crates/nikaia/src/contracts/sync.rs
//
// Part II 12.1: a `sync` function may only call `sync` functions.
//
// Two analyses of one rule, running in **opposite directions**, and keeping
// them apart is the whole design (ADR-288).
//
// `check` verifies an assertion. Someone wrote `sync`, and this reports the
// calls that contradict it (`NK2202`). It is conservative in the **permissive**
// direction: with no receiver types, `a.method()` cannot be looked up, so it is
// not an error. What *can* be looked up is a call by name - a function in this
// unit, or a path like `io::read_to_string` into a library's ledger - and that
// is where the rule earns its keep, because `fs::` and `io::` are named rather
// than called on a receiver. The check never rejects a program the rule allows,
// and does not yet catch every program the rule forbids. An unchecked promise
// catches nothing at all, so that is worth having and worth saying.
//
// `infer` makes a claim, and therefore runs the other way. It writes `sync`
// into the ledger for a function nobody annotated, and that entry is **shipped**
// (Part III 13.5): a consumer reads it and puts the function inside `access`.
// So it is conservative in the **restrictive** direction - a call it cannot
// resolve is a call it cannot vouch for, and the function does not get the
// promise. The ledger settled this polarity once already, for provenance: "an
// analysis that fails open is a vulnerability generator". Inferring `sync` from
// a body full of calls one cannot see would be exactly that.
//
// **Why infer at all.** Before this, `sync` was opt-in, so almost nothing was
// `sync`, so `access`, `access_all`, `par_iter` and the panic hook - everything
// Part II 12.2 makes safe by demanding a `sync` lambda - could call almost
// nothing. The restrictive side of the language was the unusable one, and the
// way out was to annotate a chain of pure helpers by hand. Now a body that
// provably cannot pause says so on its own, and `sync` in the source becomes an
// **assertion you write where you want it held**, checked against the body,
// rather than a mode you have to enter.

use std::collections::BTreeMap;

use crate::ast::{Block, Expr, Span, Stmt};
use crate::parser::Parsed;

use crate::check::MethodCalls;

use super::{Ledger, Sync};

/// One call that a `sync` function may not make, declared in Nikaia
/// (`nikaia-std/src/tools/sync.nika`): the statement it is in, the caller and
/// what it promised, the callee as the ledger names it, whether a library
/// answered, whether the callee never pauses and does not promise it
/// ([ADR-288](../../../docs/specification/adr/adr-288.md) D28), and whether it
/// is a construct rather than a function
/// ([ADR-292](../../../docs/specification/adr/adr-292.md) D16).
pub use nikaia_std::tools::sync::Violation;

/// Every call a `sync` function makes that the ledgers say can pause.
pub fn check(parsed: &Parsed, own: &Ledger, library: &Ledger) -> Vec<Violation> {
    // **The walk is Nikaia** (`tools/sync.nika`, #125).
    let mut found = nikaia_std::tools::sync::unit_violations(
        &parsed.program,
        &parsed.interner,
        &|name: &str| parsed.unaliased(name),
        &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
        own,
        library,
    );
    found.sort_by_key(|v| v.span.at());
    found
}

/// Give every function in the ledger the `sync` its body earns.
///
/// Runs after the entries exist, and only ever *adds* [`Sync::Inferred`]: an
/// assertion in the source is what the source said and is left exactly as it
/// was written, so that `NK2202` still has something to contradict.
///
/// The fixpoint is a greatest one - start from "every candidate is `sync`" and
/// take the claim away from anything that reaches a function without it. Two
/// consequences worth naming. Mutual recursion between pure functions keeps the
/// claim, which is correct and is what a least fixpoint would have got wrong.
/// And the iteration walks a `BTreeMap` and repeats until nothing changes, so
/// the answer does not depend on the order the source declared things in -
/// which it must not, because 13.5 makes this file a pure function of (source,
/// toolchain) and `--locked` compares it byte for byte.
///
/// `resolved` is the type checker's answer to the one question this walk cannot
/// ask: what a method call goes to (ADR-288). Handing it in rather than
/// computing it here keeps one type checker in the compiler; the alternative
/// was a second, worse one living in this file.
///
/// **Hands back the functions that pause only where their lambdas do**
/// ([ADR-288](../../../docs/specification/adr/adr-288.md) D29): by key, the code
/// parameters that may pause and that each one calls. Nothing else it reaches
/// can pause, so `sync(those)` is a promise its body would keep. The ledger is
/// not told - the inference never writes `sync(f)`, because a promise is the
/// source's to make (§4) - and the build says so in a note instead.
pub fn infer(
    ledger: &mut Ledger,
    units: &[&Parsed],
    library: &Ledger,
    resolved: &BTreeMap<String, MethodCalls>,
) -> Noted {
    use nikaia_std::tools::sync::{self as nika, SyncGraph};
    use nikaia_std::tools::throws::MethodsCalled;

    // **The walk is Nikaia** (`tools/sync.nika`, #125): every body's calls,
    // the type checker's method calls merged in - one whose receiver is not
    // known, or code handed in that may pause, is enough (D2's polarity) -
    // and a trait's and a grammar's entries as leaves.
    let mut graph = SyncGraph {
        methods: resolved
            .iter()
            .map(|(key, calls)| {
                let called = MethodsCalled {
                    resolved: calls.resolved.clone(),
                    unseen: calls.unresolved || calls.code_pauses,
                };
                (key.clone(), called)
            })
            .collect(),
        nodes: BTreeMap::new(),
        unit: 0,
    };
    for (unit, parsed) in units.iter().copied().enumerate() {
        graph.unit = unit as i64;
        nika::unit_sync(
            &parsed.program,
            &parsed.interner,
            &|name: &str| parsed.unaliased(name),
            &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
            ledger,
            library,
            &mut graph,
        );
    }

    // **The fixpoint is Nikaia** too: start optimistic, take the claim away
    // until nothing changes, and name the functions whose only pausing is
    // their lambdas' (ADR-288 D29).
    let reached: BTreeMap<String, nika::SyncReach> = graph
        .nodes
        .iter()
        .map(|(name, node)| (name.clone(), node.reach.clone()))
        .collect();
    let holds = nika::sync_holds(&reached);
    let by_code = nika::paused_only_by_code(&reached, &holds);

    for (name, holds) in &holds {
        if !holds {
            continue;
        }
        // Only ever *adds* `Inferred`: an assertion in the source is what the
        // source said, so that `NK2202` still has something to contradict.
        if let Some(contract) = ledger.functions.get_mut(name)
            && contract.sync_claim == Sync::No
        {
            contract.sync_claim = Sync::Inferred;
        }
    }

    // **Why each function that does not hold pauses** (ADR-288 D32): the
    // shortest way through the package's calls to a statement that pauses
    // itself. Breadth-first and over the map's order, so the answer is the same
    // on every build.
    let mut why: BTreeMap<String, Pause> = BTreeMap::new();
    for start in graph.nodes.keys().filter(|name| !holds[name.as_str()]) {
        let chain = nika::pause_chain(&reached, &holds, start);
        if let Some(node) = chain.last().and_then(|at| graph.nodes.get(at)) {
            why.insert(
                start.clone(),
                Pause {
                    chain: chain.clone(),
                    unit: node.unit as usize,
                    site: node
                        .site
                        .as_ref()
                        .map(|site| (site.span, site.what.clone())),
                },
            );
        }
    }
    Noted { by_code, why }
}

/// What [`infer`] learns besides the ledger's own column, for the build to say
/// ([ADR-288](../../../docs/specification/adr/adr-288.md) D29, D32).
#[derive(Debug, Default)]
pub struct Noted {
    /// The functions that pause only where their lambdas do, with the code
    /// parameters that decide (D2).
    pub by_code: BTreeMap<String, Vec<String>>,
    /// Why each function that can pause does (D5).
    pub why: BTreeMap<String, Pause>,
}

/// **The way from a function to the statement that makes it pause**.
#[derive(Debug, Clone)]
pub struct Pause {
    /// The function, the ones of its package it calls on the way, and last the
    /// one that pauses itself.
    pub chain: Vec<String>,
    /// The unit the last one is written in, by its place in the list
    /// [`infer`] was handed.
    pub unit: usize,
    /// The statement and what in it pauses; `None` where it is a method call
    /// only the type checker resolved.
    pub site: Option<(Span, String)>,
}

/// What one expression tells either analysis, where it is a call at all.
///
/// One resolution rule, written once. The check and the inference disagree
/// about what to *do* with `Opaque` - the first shrugs, the second refuses -
/// and that disagreement is the design. Having them disagree about what a call
/// even resolves to would just be a bug waiting to happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reached {
    /// A function this package declares, by the name the ledger records it under.
    Own(String),
    /// A function in a library, and what that library's ledger says about it.
    Library { key: String, sync: bool },
    /// A method call. Neither analysis here can resolve one: `stats.add(5)`
    /// names `add` and says nothing about what `stats` is.
    ///
    /// The **type checker** can, and does (ADR-288). So this is not "unknown"
    /// but "asked elsewhere", and the two callers of `reached` take it
    /// differently: the inference merges in the checker's answer per function,
    /// and the check looks the resolved name up the same way it looks up any
    /// other. Collapsing this into `Opaque` was what threw the answer away.
    Method,
    /// A call whose target this compiler cannot name and nobody else can
    /// either: a name no ledger knows.
    ///
    /// Also everything that is not a plain call but still *runs* something -
    /// `spawn`, a `dsl` - because a body containing one is not the pure CPU
    /// task Part II 12.1 describes, whatever the thing it runs turns out to do.
    ///
    /// `Some(name)` where the source wrote one and no ledger knew it, `None`
    /// for a construct that has no callee to name. Both block, and the name is
    /// carried only so the diagnostic can say which call it was about.
    Opaque(Option<String>),
}

/// What a call resolves to, by the same rule for every analysis:
/// `tools/calls.nika`'s `callee_of` (#125). This compiler's walk hands it each
/// expression and reads the answer back as its own enum.
///
/// `None` means the expression is not a call at all - or a variant of a type,
/// which builds a value and runs no body - the one case no analysis has
/// anything to say about.
pub(crate) fn reached(
    parsed: &Parsed,
    expr: &Expr,
    own: &Ledger,
    library: &Ledger,
) -> Option<Reached> {
    use nikaia_std::tools::calls::{Callee, callee_of};
    Some(
        match callee_of(&parsed.interner, expr, own, library, &parsed.program.items)? {
            Callee::Own(name) => Reached::Own(name),
            Callee::Library { key, never_pauses } => Reached::Library {
                key,
                sync: never_pauses,
            },
            Callee::Method => Reached::Method,
            Callee::Opaque(name) => Reached::Opaque(name),
        },
    )
}

/// Every expression a statement holds, without descending into nested blocks -
/// those are walked separately so that each keeps its own statement's span.
pub(crate) fn visit_stmt(parsed: &Parsed, stmt: &Stmt, f: &mut impl FnMut(&Expr)) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Comptime { value, .. } => visit_expr(parsed, value, f),
        Stmt::Assign { target, value, .. } => {
            visit_expr(parsed, target, f);
            visit_expr(parsed, value, f);
        }
        Stmt::For { iter, .. } => visit_expr(parsed, iter, f),
        Stmt::While { cond, .. } => visit_expr(parsed, cond, f),
        Stmt::Return(Some(value)) => visit_expr(parsed, value, f),
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
        Stmt::Expr(expr) => visit_expr(parsed, expr, f),
    }
}

pub(crate) fn visit_stmt_blocks<'a>(stmt: &'a Stmt, f: &mut impl FnMut(&'a Block)) {
    match stmt {
        Stmt::For { body, .. } | Stmt::While { body, .. } => f(body),
        Stmt::Let { value, .. } | Stmt::Comptime { value, .. } | Stmt::Expr(value) => {
            visit_expr_blocks(value, f)
        }
        Stmt::Assign { target, value, .. } => {
            visit_expr_blocks(target, f);
            visit_expr_blocks(value, f);
        }
        Stmt::Return(Some(value)) => visit_expr_blocks(value, f),
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
    }
}

/// Every block an expression holds, including the body of a lambda passed as
/// an argument.
///
/// A trailing lambda runs *during* the call it is given to - `.and_modify fn {
/// … }` is not deferred - so what it calls, the function around it calls. The
/// one shape that is different is `spawn`, whose body runs later and elsewhere;
/// it is a detached context (Part I, 5.4) and is not walked here.
pub(crate) fn visit_expr_blocks<'a>(expr: &'a Expr, f: &mut impl FnMut(&'a Block)) {
    match expr {
        // An `unsafe` block is part of the function that writes it
        // ([ADR-302](../../../docs/specification/adr/adr-302.md) D3): it makes
        // no boundary of its own, so what it calls, the function calls.
        Expr::Block(block)
        | Expr::Unsafe(block)
        | Expr::Overlap(block)
        | Expr::Closure { body: block, .. } => f(block),
        Expr::Call { func, args, config } => {
            visit_expr_blocks(func, f);
            args.iter().for_each(|a| visit_expr_blocks(a, f));
            config.iter().for_each(|c| visit_expr_blocks(&c.value, f));
        }
        Expr::MethodCall {
            receiver,
            args,
            config,
            ..
        }
        | Expr::SafeMethod {
            receiver,
            args,
            config,
            ..
        } => {
            visit_expr_blocks(receiver, f);
            args.iter().for_each(|a| visit_expr_blocks(a, f));
            config.iter().for_each(|c| visit_expr_blocks(&c.value, f));
        }
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => {
            f(then_branch);
            if let Some(block) = else_branch {
                f(block);
            }
        }
        Expr::TryCatch { expr, handler } => {
            visit_expr_blocks(expr, f);
            f(handler);
        }
        Expr::Match { arms, .. } => {
            for arm in arms {
                visit_expr_blocks(&arm.body, f);
            }
        }
        // A `select` arm's body runs in this function once its value won.
        Expr::Select(arms) => arms.iter().for_each(|arm| f(&arm.body)),
        _ => {}
    }
}

/// Every expression inside one, excluding the bodies of nested blocks.
pub(crate) fn visit_expr(parsed: &Parsed, expr: &Expr, f: &mut impl FnMut(&Expr)) {
    f(expr);

    // **A hole is a call like any other.** Its expression is parsed out of the
    // literal on the way to the emitter, so until this walk existed a call
    // inside `"{io::read_to_string()}"` was invisible here - and `sync` is
    // *inferred* from what a body calls (ADR-288), so the function came out of
    // the ledger claiming it cannot pause. ADR-288 D2 and ADR-010 D1 name that
    // direction the dangerous one.
    for hole in crate::emit::literal_expressions(parsed, expr) {
        visit_expr(parsed, &hole, f);
    }

    match expr {
        // What stands after a `;` is an expression too, and one that can
        // pause. `sync` is *inferred* from what a body calls (ADR-288), so a
        // call this walk does not reach is a function claiming it cannot pause
        // - the fail-open direction ADR-288 D2 names as the dangerous one. A
        // DSL's deferred parameters stand there (ADR-296 D5).
        Expr::Call { func, args, config } => {
            visit_expr(parsed, func, f);
            args.iter().for_each(|a| visit_expr(parsed, a, f));
            config.iter().for_each(|c| visit_expr(parsed, &c.value, f));
        }
        Expr::MethodCall {
            receiver,
            args,
            config,
            ..
        }
        | Expr::SafeMethod {
            receiver,
            args,
            config,
            ..
        } => {
            visit_expr(parsed, receiver, f);
            args.iter().for_each(|a| visit_expr(parsed, a, f));
            config.iter().for_each(|c| visit_expr(parsed, &c.value, f));
        }
        Expr::Binary { lhs, rhs, .. } => {
            visit_expr(parsed, lhs, f);
            visit_expr(parsed, rhs, f);
        }
        // **`throw` holds an expression, and it was not walked** — so every
        // derived column was blind to whatever built the error.
        // `throw wrap(io::read())` left its function looking `sync`, and it is
        // this walk that says otherwise. Found by `keeps`
        // ([ADR-094](../../../docs/specification/adr/adr-094.md) D2) reading a
        // parameter as lent because the `throw` that stores it was invisible;
        // the same hole was `sync`'s, `throws`' and `touches`'.
        Expr::Unary { expr, .. }
        | Expr::Try(expr)
        | Expr::Throw(expr)
        | Expr::Cast { expr, .. } => visit_expr(parsed, expr, f),
        Expr::Field { base, .. } | Expr::SafeField { base, .. } => visit_expr(parsed, base, f),
        Expr::Index { base, index } => {
            visit_expr(parsed, base, f);
            visit_expr(parsed, index, f);
        }
        Expr::Range { start, end, .. } => {
            visit_expr(parsed, start, f);
            visit_expr(parsed, end, f);
        }
        Expr::Tuple(parts) => parts.iter().for_each(|p| visit_expr(parsed, p, f)),
        Expr::Coalesce { value, fallback } => {
            visit_expr(parsed, value, f);
            visit_expr(parsed, fallback, f);
        }
        Expr::TryCatch { expr, .. } => visit_expr(parsed, expr, f),
        Expr::If { cond, .. } => visit_expr(parsed, cond, f),
        // **An arm is an expression of this function** (issue #171
        // issue #171): `R::A => slow()` is a call, and only an arm that is a block
        // reached `visit_expr_blocks`. `pick` came out of the ledger `sync`
        // while its lowering awaited `slow()`, which `rustc` refused - the
        // fail-open direction ADR-288 D2 names, for `throws`, `touches` and
        // `keeps` as much as for `sync`. A guard runs too.
        Expr::Match { value, arms } => {
            visit_expr(parsed, value, f);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    visit_expr(parsed, guard, f);
                }
                visit_expr(parsed, &arm.body, f);
            }
        }
        Expr::StructLit { fields, .. } => fields
            .iter()
            .filter_map(|field| field.value.as_ref())
            .for_each(|value| visit_expr(parsed, value, f)),
        // **And the four other places a value is computed** that the walk
        // passed by, for the same reason: a list's items, the copy a `with`
        // is made of and its fields, what a `return` hands back where it is an
        // expression (ADR-276), and what each arm of a `select` starts.
        Expr::ListLit { items, .. } => items.iter().for_each(|item| visit_expr(parsed, item, f)),
        Expr::With { base, fields, .. } => {
            visit_expr(parsed, base, f);
            fields
                .iter()
                .filter_map(|field| field.value.as_ref())
                .for_each(|value| visit_expr(parsed, value, f));
        }
        Expr::Return(value) => {
            if let Some(value) = &**value {
                visit_expr(parsed, value, f);
            }
        }
        Expr::Select(arms) => arms
            .iter()
            .for_each(|arm| visit_expr(parsed, &arm.value, f)),
        _ => {}
    }
}
