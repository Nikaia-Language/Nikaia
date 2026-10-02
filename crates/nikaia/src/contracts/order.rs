// crates/nikaia/src/contracts/order.rs
//
// Whether two statements have to keep the order they were written in
// (ADR-033, Part I 8.1.1).
//
// The rule is one sentence - *two operations whose touch sets are disjoint have
// no order between them* - and this file is the part of it that looks at a
// program rather than at a contract. It answers one question:
//
//     may these adjacent statements overlap?
//
// **It says "no" for every reason it can think of, and for every reason it
// cannot.** That polarity is the decision (ADR-033 D4): a statement whose
// effects the compiler cannot enumerate keeps its position, so a program built
// against libraries that describe nothing behaves exactly as it does today.
// Every `false` below is either a real dependency or an admission of ignorance,
// and the two are deliberately worth the same.
//
// What this is *not*: a scheduler, a cost model, or an answer for a whole
// block. It answers about a **run** of adjacent statements - a pair, or a group
// of three or more where every one of them meets every other on nothing
// ([`group_of`]) - and everything it refuses today it refuses for a reason
// written down here rather than for lack of a case.
//
// It also knows nothing about the build. Whether two operations *may* overlap
// is a question about the program; whether a vehicle exists to overlap them
// with, and whether the program wrote `seq` around them, are the emitter's
// (ADR-033 §8.2b). No switch reaches this file, and none may.
//
// [`vehicle`] is not an exception to that, and the distinction is worth stating
// because it looks like one. *Which* vehicle a pair would need - `std` putting
// two operations in flight, or two closures carrying the program's own code -
// is read off the statements and is therefore this file's (ADR-033 D10).
// Whether this build *has* that vehicle is two methods on `emit::Build`, and
// the answers differ: one of them is `user_parallelism`'s and the other is not.
//
// **The shapes it sees** (ADR-033 §8.3's first item). The first increment read
// one shape: a `let` bound to exactly one call. Measuring it found that 101 of
// 127 refused pairs in `examples/` fell out on that alone, before any `touches`
// set was consulted - a zero that measured the analysis rather than the corpus.
// So a statement is now reduced where it is
//
//   * a **`let`**, as before, or a **bare expression statement**: `println(x)`,
//     `fs::write(p, fs::Root::Anywhere, d)` - an operation the ledger can account for that nothing
//     binds;
//   * built out of **literals and calls**, at any depth, rather than being one
//     call: `f("a") + g("b")` performs two operations and its touch set is
//     their union.
//
// Everything else is still refused, and the refusals are now *about the
// program* rather than about the analysis not looking: a method call whose
// ledger key needs the type checker, an argument that is not a literal, a
// callee nobody described.

use crate::contracts::LedgerOps;
use std::collections::BTreeSet;

use crate::ast::{BinaryOp, Expr, Item, Spanned, Stmt};
use crate::parser::Parsed;

use super::Ledger;
use super::touch::Reached;
use crate::contracts::SignatureOps;
use crate::contracts::touch::TouchOps;

/// One statement, reduced to what deciding an order needs.
#[derive(Debug, Clone)]
pub struct Operation {
    /// The name it binds, where it binds one.
    pub binds: Option<String>,
    /// Every name its value mentions. A statement that mentions what the one
    /// before it bound is a data dependency, and nothing else has to be
    /// consulted.
    pub mentions: BTreeSet<String>,
    /// What the call it performs reaches, as the ledger describes it with the
    /// call's own arguments filled in.
    pub reaches: Vec<Reached>,
    /// The ledger key of the call, for a diagnostic that wants to name it.
    pub callee: String,
}

/// Reduce a statement to an [`Operation`], where it is one this can reason about.
///
/// `None` for everything else: assignments, loops, returns, and any value this
/// analysis will not take apart. An assignment is deliberately among them and
/// will stay there - `x = 1` changes a name without binding one, so the data
/// dependency that [`verdict`] finds by comparing `binds` against `mentions`
/// would not be found at all.
pub fn operation(
    parsed: &Parsed,
    stmt: &Stmt,
    own: &Ledger,
    library: &Ledger,
) -> Option<Operation> {
    match accounted(parsed, stmt, own, library) {
        Accounted::Operation(operation) => Some(operation),
        _ => None,
    }
}

/// A statement, reduced - or the reason it could not be.
///
/// The reason is the whole point (ADR-033 D9). Nikaia has no `allow_parallel`,
/// so the only thing standing between a refusal and a mystery is the compiler
/// being able to say which refusal it was.
#[derive(Debug, Clone)]
pub enum Accounted {
    Operation(Operation),
    /// It performs nothing at all: a loop, an `if`, an assignment, a `return`,
    /// a `let` of a value that calls nothing.
    ///
    /// The one refusal that is about the **statement** rather than about what
    /// the compiler knows. `let n = 0` has no touch set because it reaches
    /// nothing, and pairing it with the operation next to it would ask a
    /// thread to carry a constant.
    NotAnOperation,
    /// It performs something this analysis will not take apart, named so the
    /// report says *which* thing.
    Opaque(&'static str),
    /// Its `catch` handler can leave the function, so the statement after it is
    /// conditional on this one having succeeded (ADR-034).
    DivertingHandler,
    /// It can fail and nothing catches the failure, so the failure leaves the
    /// function - and the statement after it is conditional on this one having
    /// succeeded, exactly as a diverting handler makes it (ADR-034 D2).
    ///
    /// The same rule as [`Accounted::DivertingHandler`] reached from the other
    /// side, and it has to be here: an uncaught `fs::write(…, fs::Root::Anywhere)` is precisely the
    /// shape a bare expression statement makes common.
    UncaughtFailure(String),
    /// Nothing describes what the call reaches, so it reaches everything (D4).
    NoTouches(String),
    /// Something describes what the call reaches, in a word this compiler does
    /// not know - so it reaches everything, for D4's reason and not for want of
    /// a contract.
    ///
    /// The two are worth telling apart in a report: `NoTouches` is a gap
    /// somebody can close by writing a line, and this one is a ledger written
    /// against a newer vocabulary than this compiler has. It is refused rather
    /// than ignored because an unknown kind is *different* from every kind
    /// there is, and therefore disjoint from all of them - a typo that bought
    /// an overlap would be the worst shape a mistake in this analysis can take
    /// (`contracts::touch::KINDS`).
    UnknownResource {
        callee: String,
        kind: String,
    },
    /// What it hands back may not cross a thread, or nothing written down says
    /// it may (ADR-005 §1 Group B).
    ///
    /// An overlapped operation runs in a closure somewhere else and hands its
    /// value back, so the result crosses a thread. `contracts::send` answers
    /// whether it may, and anything but yes keeps the statement where it was
    /// written - the crossing question answered with D4's polarity. The way to
    /// buy the overlap back is the same as for `touches`: write the type down.
    MayNotCross {
        callee: String,
        crossing: super::send::Crossing,
    },
    /// A value that is not a literal, which this increment will not send to
    /// another thread.
    ///
    /// Each operation is lowered into a closure that runs somewhere else, and a
    /// closure that captures nothing cannot capture something that must not
    /// cross a thread. What may cross one is a decision of its own and not an
    /// implicit answer here - D9's table calls it "an implementation limit, not
    /// a language question", and it is the one refusal in that table a wider
    /// analysis alone cannot lift.
    ///
    /// **D10's vehicle has no closure in it**, so that argument does not reach
    /// a completion pair - and the refusal stays anyway. A path this compiler
    /// cannot read is a file it cannot *name*, which is what D4 answers with
    /// "it reaches everything": the pair would be refused one line further down
    /// for a reason that has nothing to do with threads. Lifting this is still
    /// the decision it always was.
    NonLiteralArgument(String),
}

impl Accounted {
    /// One line, for a report a person reads - and, where there is one, the way
    /// out. A refusal a reader can act on is worth several they cannot.
    pub fn why(&self) -> String {
        match self {
            Accounted::Operation(_) => "it is an operation".to_string(),
            Accounted::NotAnOperation => "one of them performs no operation at all".to_string(),
            Accounted::Opaque(what) => format!("one of them holds {what}"),
            Accounted::DivertingHandler => {
                "its `catch` can leave the function, so the next statement might never run. \
                 Make the handler return a value instead, and check it afterwards"
                    .to_string()
            }
            Accounted::UncaughtFailure(name) => {
                format!(
                    "`{name}` can fail and nothing catches it, so the next statement might never \
                     run. `catch` it into a value, and check it afterwards"
                )
            }
            Accounted::NoTouches(name) => {
                format!("nothing says what `{name}` touches, so it might touch anything")
            }
            Accounted::UnknownResource { callee, kind } => {
                format!(
                    "`{callee}` says it touches {} `{kind}`, which this version of Nikaia \
                     doesn't know, so it might touch anything",
                    crate::check::an_or_a(kind)
                )
            }
            Accounted::MayNotCross { callee, crossing } => {
                format!(
                    "what `{callee}` returns would have to move to another thread. {}",
                    crossing.note().unwrap_or_else(|| "It can't.".to_string())
                )
            }
            Accounted::NonLiteralArgument(name) => {
                format!("`{name}` isn't a literal, and only literals can be sent to another thread")
            }
        }
    }
}

pub fn accounted(parsed: &Parsed, stmt: &Stmt, own: &Ledger, library: &Ledger) -> Accounted {
    // Two shapes, and the second is ADR-033 §8.3's first item: an operation
    // the ledger can account for is not always bound to a name. `println(x)`,
    // `out.push(y)` and `fs::write(p, fs::Root::Anywhere, d)` are statements a program is mostly
    // made of, and reading only `let` is what produced §8.1's 101.
    //
    // An **assignment** is not among them and must not be: `x = 1` changes a
    // name without binding one, so [`verdict`]'s data-dependency test - does
    // the later statement mention what the earlier one bound - would miss it
    // entirely. A shape whose dependencies this cannot see is a shape it may
    // not read.
    let (binds, value) = match stmt {
        Stmt::Let {
            names, value, ty, ..
        } => {
            // A written type would have to be carried onto one element of a
            // tuple pattern. Nothing needs it yet.
            if ty.is_some() {
                return Accounted::Opaque(
                    "a written type, which would have to be carried onto one half of a pattern",
                );
            }
            // **A tuple of names is opaque here**
            // ([ADR-098](../../../../docs/specification/adr/adr-098.md)), the
            // same answer a written type already gets and for the same reason:
            // this analysis carries **one** bound name and reordering a
            // statement that binds several would need all of them. Opaque is
            // the safe direction - it declines to reorder rather than
            // reordering wrongly.
            let [name] = names.as_slice() else {
                return Accounted::Opaque("several names bound at once");
            };
            (Some(parsed.text(*name).to_string()), value)
        }
        Stmt::Expr(value) => (None, value),
        _ => return Accounted::NotAnOperation,
    };

    // A real program writes `fs::read_to_string(p, fs::Root::Anywhere) catch { … }`, so the call is
    // usually wrapped. Looking through the wrapper is what makes this apply to
    // code anyone actually writes.
    //
    // **But only where the handler cannot divert.** A handler that `return`s
    // makes the *next* statement conditional on this one having succeeded, and
    // ADR-033 D5 forbids running a conditional operation early - overlapping
    // them would perform a read the sequential program would never have
    // performed. That case was not in D5 when it was written; it is the first
    // thing building this found, and it is recorded in ADR-034.
    let (value, handler, caught) = match value {
        Expr::TryCatch { handler, .. } if diverts(&handler.stmts) => {
            return Accounted::DivertingHandler;
        }
        Expr::TryCatch { expr, handler } => (&**expr, Some(handler), true),
        other => (other, None, false),
    };

    // What the statement is made of. Literals and calls, at any depth - a `let`
    // whose initialiser is not a bare call is the other half of §8.3's first
    // item, and `f("a") + g("b")` performs two operations whose touch sets
    // union.
    let mut walked = Walked::default();
    walk(parsed, value, &mut walked);
    // **And the handler's own effects, not only the names it mentions.** The
    // analysis looks *past* a `catch` at the call it guards, so without this a
    // handler that writes a file the next statement reads would be invisible
    // and the pair would overlap - the emitted program producing a different
    // result from the sequential one. A handler is part of the statement, so
    // what it reaches is part of what the statement touches; where that cannot
    // be read, D4 says the statement touches everything and stays put.
    if let Some(handler) = handler {
        walk_block(parsed, handler, &mut walked);
    }
    if walked.calls.is_empty() && !walked.performs {
        // Nothing here reaches the world at all. Not an admission of ignorance:
        // the statement genuinely is not an operation, and saying so keeps a
        // constant out of a thread.
        return Accounted::NotAnOperation;
    }

    // The ledger is consulted **before** the walk's own refusal is reported,
    // and the order is the point: "nothing says what `x` reaches" is a gap
    // somebody can close by writing a contract, where "it holds a method call"
    // is this compiler's own limit. Of two true answers, the one a reader can
    // act on is the one worth printing (D9).
    let mut reaches = Vec::new();
    let mut named_by = Vec::new();
    for call in &walked.calls {
        let Expr::Call { args, .. } = call else {
            unreachable!("`walk` collects only calls");
        };
        let Some(callee) = callee_of(parsed, call) else {
            unreachable!("`walk` refuses a call through anything but a name");
        };

        // The contract has to be found *and* has to describe its effects. An
        // entry without `touches` is the absence of an answer (ADR-033 D4).
        let Some((key, contract)) = own
            .lookup(&callee)
            .or_else(|| library.lookup(&callee))
            .filter(|(_, contract)| contract.touches_known)
        else {
            return Accounted::NoTouches(callee);
        };

        // A failure nobody catches leaves the function, which makes the *next*
        // statement conditional on this one having succeeded - the same reason
        // a diverting handler is refused, and ADR-034 D2 in general (D5's "no
        // speculation" had this case too). Starting the next operation early
        // would perform work the program as written might never have performed.
        if !caught && !contract.fails_with.is_empty() {
            return Accounted::UncaughtFailure(key);
        }

        // A resource this compiler has no name for is one it cannot compare,
        // and the failure mode is the dangerous direction: an unknown kind
        // differs from every kind there is, so it would be disjoint from all of
        // them and the pair would overlap. D4's answer instead - it reaches
        // everything - and the whole entry goes with it, because an entry is
        // only as readable as its least readable line.
        if let Some(touch) = contract.touches.iter().find(|t| !t.kind_is_known()) {
            return Accounted::UnknownResource {
                callee: key,
                kind: touch.kind.clone(),
            };
        }

        // Which argument goes with which parameter, so that `file(path)` can be
        // turned into "the file named by this call's first argument".
        let Some(signature) = contract.signature.as_ref() else {
            return Accounted::NoTouches(key);
        };

        // **And what it hands back has to be able to cross a thread**
        // (ADR-005 §1 Group B). Overlapping puts this operation inside a closure
        // that runs elsewhere and hands its value back to the thread that
        // started it, so the result crosses whether or not anything binds it.
        // An answer that is not "it may" keeps the statement where it was
        // written: this is a step the compiler was never obliged to take, so
        // fail-closed costs only speed here - which is D4's polarity, applied to
        // the crossing question instead of to a touch set.
        //
        // The *captures* need the same question asked of them, and today they
        // cannot: `Accounted::NonLiteralArgument` means the closure captures
        // nothing at all. This is the check that has to be asked about each
        // captured name on the day that limit is lifted.
        // `Destination::Ours` (ADR-045 D1): the closure overlapping builds is
        // this compiler's own, on this compiler's own thread, so it is the same
        // destination a `spawn`ed task is - not a library we cannot read. A lock
        // may therefore be handed back out of an overlapped statement, which is
        // D2's answer and not a relaxation of it.
        let crossing = super::send::crossing(
            &signature.result_or_unit(),
            own,
            library,
            super::send::Destination::Ours,
        );
        if !crossing.may() {
            return Accounted::MayNotCross {
                callee: key,
                crossing,
            };
        }
        let parameters: Vec<&str> = signature
            .arguments()
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();

        reaches.extend(contract.touches.iter().map(|touch| {
            let named = touch.parameter.as_ref().and_then(|parameter| {
                let at = parameters.iter().position(|p| p == parameter)?;
                literal_text(args.get(at)?)
            });
            Reached {
                kind: touch.kind.clone(),
                // A resource with a parameter whose argument could not be read
                // is one this compiler cannot name, and `unknown` is what says
                // so: it then conflicts with its whole kind.
                unknown: touch.parameter.is_some() && named.is_none(),
                named,
                write: touch.write,
            }
        }));
        named_by.push(key);
    }

    // A method call is accounted for only where **every** entry of that name
    // reaches nothing. One that may reach the world leaves this analysis unable
    // to say which resource, and D4's answer to "which" being unanswerable is
    // that it is all of them - so the statement stays where it was written.
    for method in &walked.methods {
        let candidates = {
            let mine = own.candidates(method);
            if mine.is_empty() {
                library.candidates(method)
            } else {
                mine
            }
        };
        if candidates.is_empty()
            || candidates
                .iter()
                .any(|(_, contract)| !contract.touches_known || !contract.touches.is_empty())
        {
            return Accounted::NoTouches(method.clone());
        }
    }

    if let Some(refusal) = walked.refused {
        return refusal;
    }

    let mut mentions = BTreeSet::new();
    names_in(parsed, value, &mut mentions);
    // **The handler counts too.** `f("a") catch { w }` is one expression, and
    // `w` is a name it mentions - so if the statement before it bound `w`,
    // that is a data dependency like any other. Looking only past the `catch`
    // would miss it, and the lowering puts the handler in the same closure.
    if let Some(handler) = handler {
        names_in_block(parsed, handler, &mut mentions);
    }

    Accounted::Operation(Operation {
        binds,
        mentions,
        reaches,
        callee: named_by.join(" + "),
    })
}

/// What walking a statement's value found (ADR-033 §8.3, first item).
///
/// Three answers rather than one, because "this performs nothing" and "this
/// performs something I will not look inside" are different facts and the
/// report is only useful if it can tell them apart. A `let n = 0` is the first;
/// a `let n = xs.len()` is the second.
#[derive(Default)]
struct Walked<'a> {
    /// Every [`Expr::Call`] it performs, at whatever depth.
    calls: Vec<&'a Expr>,
    /// It performs *something* - a method call, a block, a grammar - even where
    /// this could not say what. Set wherever the walk stops descending, so that
    /// an unreadable operation is never mistaken for no operation.
    performs: bool,
    /// Every method called, by name. Which ledger entry each *is* needs the
    /// type checker (ADR-028); whether they all reach nothing does not, and
    /// that weaker question is answered in [`accounted`].
    methods: Vec<String>,
    /// Why it could not be taken apart, where it could not. The first reason
    /// only: a refusal is a refusal, and a list of them would be noise.
    refused: Option<Accounted>,
}

impl Walked<'_> {
    /// Record a reason, and say nothing about whether anything was performed.
    /// For a node whose children the walk still descends into.
    fn note(&mut self, why: Accounted) {
        if self.refused.is_none() {
            self.refused = Some(why);
        }
    }

    /// Record a reason for a node the walk stops at. Stopping is exactly when
    /// "performs something" has to be assumed: what is inside was not read.
    fn refuse(&mut self, why: Accounted) {
        self.performs = true;
        self.note(why);
    }
}

/// The same, for the statements of a `catch` handler.
///
/// A handler is code that runs, so its calls belong in the statement's touch
/// set. Only the two shapes whose value `walk` can take apart are read; every
/// other statement in a handler stops the walk, which is what makes the
/// handler's effects unknown rather than empty (D4).
fn walk_block<'a>(parsed: &Parsed, block: &'a crate::ast::Block, out: &mut Walked<'a>) {
    for stmt in &block.stmts {
        match &stmt.node {
            Stmt::Let { value, .. } | Stmt::Expr(value) => walk(parsed, value, out),
            _ => out.refuse(Accounted::Opaque(
                "a `catch` handler doing more than handing back a value",
            )),
        }
    }
}

/// Take a statement's value apart into the calls it performs.
///
/// **The whole expression, not only the arguments.** `f("a") + n` would be
/// lowered into a closure that captures `n`, so a name anywhere in it is the
/// same question the literal-argument rule asks about an argument, and gets the
/// same answer.
///
/// What is walked is the fail-closed half of the design: literals, calls, and
/// operators over them. Everything else records a reason and stops, which makes
/// adding a shape a deliberate act rather than the consequence of a `_` arm.
fn walk<'a>(parsed: &Parsed, expr: &'a Expr, out: &mut Walked<'a>) {
    match expr {
        // A value with no name in it and nothing to evaluate.
        Expr::LitInt { .. }
        | Expr::LitFloat(_)
        | Expr::LitBool(_)
        | Expr::LitNull
        | Expr::LitChar(_)
        | Expr::LitStr { .. } => {}

        Expr::Call { args, config, .. } => {
            // Kap 5.1's options are values like any other and would have to be
            // matched against the callee's declaration order before a `touches`
            // parameter could be read off them. They are not, so a call that
            // uses them is refused.
            if !config.is_empty() {
                out.refuse(Accounted::Opaque(
                    "a call with options, which nothing matches against the callee's order yet",
                ));
                return;
            }
            if callee_of(parsed, expr).is_none() {
                out.refuse(Accounted::Opaque(
                    "a call through something that is not a name",
                ));
                return;
            }
            for arg in args {
                walk(parsed, arg, out);
            }
            out.calls.push(expr);
        }

        // Pure structure over the values above. The arithmetic itself reaches
        // nothing, so what the statement touches is what its calls touch.
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => walk(parsed, expr, out),
        Expr::Binary { op, lhs, rhs, .. } => {
            // `&&` and `||` evaluate their right side only sometimes, and an
            // operation that only sometimes runs may not be started early
            // (ADR-033 D5).
            if matches!(op, BinaryOp::And | BinaryOp::Or) {
                out.refuse(Accounted::Opaque(
                    "a `&&` or `||`, whose right side runs only sometimes",
                ));
                return;
            }
            walk(parsed, lhs, out);
            walk(parsed, rhs, out);
        }
        Expr::Tuple(parts) | Expr::ListLit { items: parts, .. } => {
            parts.iter().for_each(|part| walk(parsed, part, out))
        }
        // A jump is not an operation to reorder and it ends the sequence
        // ([ADR-138](../../../docs/specification/adr/adr-138.md) D1): what
        // follows it in a `seq` does not run, so this refuses rather than
        // accounting for it.
        Expr::Return(_) | Expr::Break | Expr::Continue => {
            out.refuse(Accounted::Opaque("a `return`, a `break` or a `continue`"))
        }

        // A name that is not a callee's is a value from somewhere else, and the
        // closure this is lowered into would have to capture it. It performs
        // nothing on its own, so `let b = a` still reports as no operation
        // rather than as a capture this increment will not make.
        Expr::Variable(name) => {
            out.note(Accounted::NonLiteralArgument(
                parsed.text(*name).to_string(),
            ));
        }
        // A value read from somewhere else. Reaching it is not the problem -
        // carrying it to another thread is, and that is the same decision the
        // literal rule defers. The walk goes on through it, so that
        // `f("a").field` is still known to perform `f`.
        Expr::Field { base, .. } | Expr::SafeField { base, .. } => {
            out.note(Accounted::Opaque("a value read from somewhere else"));
            walk(parsed, base, out);
        }
        Expr::Index { base, index } => {
            out.note(Accounted::Opaque("a value read from somewhere else"));
            walk(parsed, base, out);
            walk(parsed, index, out);
        }
        // **A path names an item, and an item is not a place.** A unit variant
        // (`fs::Root::Anywhere`), a constructor handed over as a value
        // ([ADR-140](../../../docs/specification/adr/adr-140.md) D2) and an
        // associated constant are all *static*: there is nothing to read, so
        // there is nothing for a closure to capture and nothing to carry to
        // another thread. It performs no operation either, so this notes
        // nothing at all — the same as a literal.
        //
        // It was `Opaque("a value read from somewhere else")`, beside the field
        // and the index reads below, where the sentence is true. Nothing wrote a
        // path in an argument until [ADR-108](../../../docs/specification/adr/adr-108.md)
        // gave every path call an `fs::Root`, and then the over-approximation
        // took the *control pair* of the ADR-038 D7 experiment with it: two reads
        // of two files, which have to overlap or that test proves nothing.
        Expr::Path(_) => {}
        Expr::StructLit { fields, .. } => {
            out.note(Accounted::Opaque("a value read from somewhere else"));
            for field in fields {
                if let Some(value) = &field.value {
                    walk(parsed, value, out);
                }
            }
        }
        Expr::Range { start, end, .. } => {
            out.note(Accounted::Opaque("a value read from somewhere else"));
            walk(parsed, start, out);
            walk(parsed, end, out);
        }
        // `a ?? b` runs `b` only when `a` had nothing, which is D5's case again.
        Expr::Coalesce { value, fallback } => {
            out.note(Accounted::Opaque(
                "a `??`, whose fallback runs only sometimes",
            ));
            walk(parsed, value, out);
            walk(parsed, fallback, out);
        }

        // --- and the nodes the walk stops at -------------------------------

        // Text with code in it (ADR-035). What it says depends on what its holes
        // hold, and the holes are Nikaia this has not parsed - so what they call
        // is unknown, which is the reason this counts as performing something.
        Expr::LitInterpolated { .. } => out.refuse(Accounted::Opaque(
            "text with code in it, whose holes this has not parsed",
        )),
        // *Which* ledger entry `xs.len()` is depends on what `xs` is, and that
        // is the type checker's answer rather than this one's (ADR-028). The
        // weaker question is answerable here: if every `::len` in the ledger
        // reaches nothing, this reaches nothing whatever the receiver is. So
        // the name is recorded and `accounted` decides; anything that may reach
        // the world is refused there (D4).
        Expr::MethodCall {
            receiver,
            method,
            args,
            config,
        }
        // **A `?.m()` is accounted for exactly as a `.m()` is.** What this walk
        // answers is what an operation *may* touch, and a call that may not
        // happen may touch everything one that does would - an
        // over-approximation is the direction D4 requires.
        | Expr::SafeMethod {
            receiver,
            method,
            args,
            config,
        } => {
            walk(parsed, receiver, out);
            // Kap 5.1's options are values like any other, and a call this
            // walk did not reach into is a call whose names it does not know.
            for value in args.iter().chain(config.iter().map(|option| &option.value)) {
                walk(parsed, value, out);
            }
            out.methods.push(parsed.text(*method).to_string());
        }

        // A lambda. It would fall to the arm below anyway; it is named here
        // because this is the one shape where the fail-closed answer differs
        // from what the rest of the ledger does with the same lambda, and
        // somebody will stand here wondering why.
        //
        // `sync` and `throws` walk a trailing lambda's body **as part of the
        // function that writes it** (`contracts::sync`'s `visit_expr_blocks`),
        // which is what lets ADR-029 D3 read `sync = "from(f)"` as "this call
        // adds no pausing of its own": whatever the lambda does is already
        // counted at the call site. Those two analyses ask about a *function*,
        // so counting an effect anywhere in its body is enough.
        //
        // This one asks about a **statement**, and needs a touch set that is
        // complete for that statement. So the same argument does not carry: the
        // lambda's body is in the function and not in the value this walk
        // reduced, and `names_in` below descends into it for a data dependency
        // while nothing descends into it for an effect. D4's answer is the only
        // one available - the statement reaches everything and keeps its place.
        //
        // Which is also why a `touches` key shaped like ADR-029's `from(f)` may
        // not be *read* like it. "Adds nothing" would buy the overlap on a touch
        // set missing everything the lambda reaches, and `println` inside the
        // lambda would interleave with the one after it: ADR-033 D1 broken by an
        // effect nobody counted, which is §8.3's `catch`-handler hole again. What
        // a `touches` entry for a higher-order function would have to say is
        // *add* what the lambda reaches, and everything where the lambda cannot
        // be read - the polarity `walk_block` already gives a handler.
        Expr::Closure { .. } => out.refuse(Accounted::Opaque(
            "a lambda, whose body this analysis does not read",
        )),

        // Everything with its own control flow: what runs inside it is decided
        // while it runs, and D5 allows only operations that certainly run.
        _ => out.refuse(Accounted::Opaque("something with its own control flow")),
    }
}

/// Why two statements keep the order they were written in - or that they need
/// not (ADR-033 D9).
///
/// The analysis knew every one of these and threw all but the boolean away. It
/// is kept because the decision *not* to give the language a word for "run
/// these together anyway" is only defensible if the compiler can say what it
/// refused and why: a silent refusal with no way to ask is the trap the
/// keyword would have been an escape from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// They meet on nothing and neither waits for the other.
    Overlap,
    /// `later` uses what `earlier` bound.
    DataDependency(String),
    /// Both bind the same name, so the order decides which value survives.
    Shadowed(String),
    /// Their touch sets meet on something one of them writes.
    SameResource {
        kind: String,
        named: Option<String>,
        /// The resource has no name because this compiler could not read one,
        /// rather than because the kind has only one of it.
        ///
        /// Two answers that both say "no name" and mean opposite things: there
        /// is exactly one `stdout`, and `fs::write(pfad, fs::Root::Anywhere, …)` reaches a file
        /// nobody here can identify. A report that called the first one
        /// unnameable would be telling a reader to go and name something that
        /// has no name to give.
        unnameable: bool,
    },
    /// Their touch sets meet on two resources that are named differently and
    /// may be the same thing - `stdout` and `stderr` under `2>&1`
    /// (`contracts::touch::FAMILIES`).
    SameDestination { one: String, other: String },
    /// One of them is not a statement this analysis can account for at all -
    /// a call it cannot resolve, a handler that can leave the function, an
    /// argument that is not a literal. An admission of ignorance, and it keeps
    /// the order for the same reason a real dependency does (D4).
    NotAccountedFor,
}

impl Verdict {
    pub fn is_overlap(&self) -> bool {
        matches!(self, Verdict::Overlap)
    }

    /// One line, for a report a person reads.
    pub fn why(&self) -> String {
        match self {
            Verdict::Overlap => "they meet on nothing".to_string(),
            Verdict::DataDependency(name) => format!("the second uses `{name}`"),
            Verdict::Shadowed(name) => format!("both bind `{name}`"),
            Verdict::SameResource {
                kind,
                named: Some(named),
                ..
            } => {
                format!("both reach {kind} `{named}`, and one writes it")
            }
            Verdict::SameResource {
                kind,
                unnameable: true,
                ..
            } => {
                format!("both touch a {kind} the compiler can't name, and one writes to it")
            }
            Verdict::SameResource { kind, .. } => {
                format!("both touch {kind}, and one writes to it")
            }
            Verdict::SameDestination { one, other } => {
                format!(
                    "one touches {one} and the other {other}, which might be the same \
                     place. Use `seq` to run them in order"
                )
            }
            Verdict::NotAccountedFor => "the compiler can't tell what one of them does".to_string(),
        }
    }
}

/// Whether `later` may run at the same time as `earlier`, and why not.
///
/// Both halves of the rule, in the order they are cheapest to refuse:
/// a **data** dependency - `later` uses what `earlier` bound - and an **effect**
/// dependency, where their touch sets meet on something one of them writes.
pub fn verdict(earlier: &Operation, later: &Operation) -> Verdict {
    if let Some(bound) = &earlier.binds
        && later.mentions.contains(bound)
    {
        return Verdict::DataDependency(bound.clone());
    }
    // Two `let`s of the same name would make the order decide which value
    // survives. The parser allows shadowing, so this is reachable.
    if earlier.binds.is_some() && earlier.binds == later.binds {
        return Verdict::Shadowed(earlier.binds.clone().unwrap_or_default());
    }

    for a in &earlier.reaches {
        for b in &later.reaches {
            if a.conflicts_with(b) {
                // Two kinds that conflict at all are two names for what may be
                // one thing, and saying which two is the whole of the answer.
                if a.kind != b.kind {
                    return Verdict::SameDestination {
                        one: a.kind.clone(),
                        other: b.kind.clone(),
                    };
                }
                // The one that is named is the more useful half to print; where
                // neither is, saying so is the point.
                let named = a
                    .named
                    .clone()
                    .filter(|_| !a.unknown)
                    .or_else(|| b.named.clone().filter(|_| !b.unknown));
                return Verdict::SameResource {
                    kind: a.kind.clone(),
                    unnameable: named.is_none() && (a.unknown || b.unknown),
                    named,
                };
            }
        }
    }

    Verdict::Overlap
}

/// Whether a whole **run** of operations may run as one group, and why not
/// (ADR-033 §6, "more than two statements at a time").
///
/// **Every pair, not every adjacent pair.** Disjointness is not transitive, and
/// this is the one place where believing it would be a soundness hole rather
/// than a missed optimisation:
///
/// ```text
/// let a = fs::read_to_string("eins.txt", fs::Root::Anywhere)   // reads eins.txt
/// let b = fs::read_to_string("zwei.txt", fs::Root::Anywhere)   // reads zwei.txt
/// fs::write("eins.txt", fs::Root::Anywhere, "x")               // writes eins.txt
/// ```
///
/// The two adjacent pairs are each disjoint. The run is not: the third meets
/// the first, and in a group they all run at once, so a chain of adjacent
/// answers would have overlapped a read of a file with the write of it. So this
/// asks about `(i, j)` for every `i < j`, and the pair it names in a refusal is
/// the *earliest* such pair - which is also the one a reader is looking at.
///
/// D6 needs nothing extra from a group. Members that meet on nothing cannot
/// observe one another's effects, and the operations this analysis accounts for
/// either cannot fail or catch their failure into a value
/// ([`Accounted::UncaughtFailure`]), so there is no error left whose order
/// could be decided by a race.
pub fn group_verdict(run: &[Operation]) -> Verdict {
    for (at, earlier) in run.iter().enumerate() {
        for later in &run[at + 1..] {
            let verdict = verdict(earlier, later);
            if !verdict.is_overlap() {
                return verdict;
            }
        }
    }
    Verdict::Overlap
}

/// How many operations from the front of `run` may go together.
///
/// A **prefix** and not the largest subset, deliberately: a group is a run of
/// statements the emitter replaces in place, so leaving one out of the middle
/// would reorder the ones after it past the one left behind. The answer is
/// therefore the longest prefix every member of which meets every other on
/// nothing, and `0` where that is fewer than two.
pub fn group_of(run: &[Operation]) -> usize {
    let mut taken = 1.min(run.len());
    while taken < run.len() {
        if !group_verdict(&run[..taken + 1]).is_overlap() {
            break;
        }
        taken += 1;
    }
    if taken < 2 { 0 } else { taken }
}

/// The ledger key of the call an expression performs, where it performs one.
///
/// One place, so a refusal names the same call the happy path would have.
fn callee_of(parsed: &Parsed, expr: &Expr) -> Option<String> {
    let Expr::Call { func, .. } = expr else {
        return None;
    };
    match &**func {
        Expr::Variable(name) => Some(parsed.text(*name).to_string()),
        Expr::Path(segments) => Some(
            segments
                .iter()
                .map(|s| parsed.text(*s))
                .collect::<Vec<_>>()
                .join("::"),
        ),
        _ => None,
    }
}

/// Whether a block can leave **by a door other than its own last statement**.
///
/// Named for the `return` and the `throw` it was written for, and it counts a
/// `break` and a `continue` too. What the caller needs is not *"does this leave
/// the function"* but the consequence ADR-034 draws from that: the statement
/// after this one is **conditional on this one having succeeded**, so the two
/// may not be overlapped. A `break` in a handler makes the next statement
/// conditional exactly as a `return` does; that the jump lands inside the same
/// function rather than outside it changes nothing about the question.
///
/// Conservative and shallow otherwise: any `return` or `throw` anywhere in it,
/// however deeply nested, counts. A handler that merely supplies a fallback
/// value does not, and that is the case this exists to let through.
///
/// **A jump bound to a loop inside the block does not count**, which is the one
/// place this is exact rather than conservative, and it is exact because being
/// so is three lines: a `break` inside a `while` written in the handler leaves
/// that `while` and lands in the handler, so the statement after the handler is
/// reached either way.
fn diverts(stmts: &[crate::ast::Spanned<Stmt>]) -> bool {
    diverts_within(stmts, false)
}

/// `bound` says a loop written *inside* the block being asked about stands
/// between here and the block's end, so a jump here is that loop's and not a
/// door out of the block.
fn diverts_within(stmts: &[crate::ast::Spanned<Stmt>], bound: bool) -> bool {
    stmts.iter().any(|stmt| match &stmt.node {
        Stmt::Return(_) => true,
        Stmt::Break | Stmt::Continue => !bound,
        Stmt::Expr(expr) | Stmt::Let { value: expr, .. } | Stmt::Comptime { value: expr, .. } => {
            holds_throw(expr, bound)
        }
        Stmt::Assign { value, .. } => holds_throw(value, bound),
        Stmt::For { body, .. } | Stmt::While { body, .. } => diverts_within(&body.stmts, true),
    })
}

/// Whether an expression can leave the block it is in.
fn holds_throw(expr: &Expr, bound: bool) -> bool {
    match expr {
        Expr::Throw(_) => true,
        Expr::Block(block) | Expr::Overlap(block) => diverts_within(&block.stmts, bound),
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => {
            diverts_within(&then_branch.stmts, bound)
                || else_branch
                    .as_ref()
                    .is_some_and(|block| diverts_within(&block.stmts, bound))
        }
        Expr::Match { arms, .. } => arms.iter().any(|arm| holds_throw(&arm.body, bound)),
        _ => false,
    }
}

/// The text of a literal argument, where the argument is one.
///
/// Only a literal. `fs::read(pfad, root)` names a file this compiler cannot identify,
/// and ADR-033 D4 says what happens then - it is not that the compiler guesses.
fn literal_text(expr: &Expr) -> Option<String> {
    match expr {
        Expr::LitStr { text, .. } => Some(text.clone()),
        // An `f"…"` is not a constant: what it says depends on what its holes
        // hold, and this wants the text of a file name written down.
        _ => None,
    }
}

/// Whether a block mentions a name anywhere inside it.
///
/// **Asked by the emitter about a `catch` handler and the name `error`**
/// ([ADR-090](../../../docs/specification/adr/adr-090.md)): Kap 7.1 says the
/// handler sees the failure under that name, so the lowering binds it whether
/// or not the handler reads one — and a handler that supplies a constant
/// fallback, which is the shape Part I 7.1 teaches first, got
/// *"unused variable: `error`"* about a binding that exists nowhere in the
/// program.
///
/// [`names_in_block`]'s over-approximation is what makes this safe, and the
/// direction matters: a false **yes** keeps today's binding and today's
/// warning, which is where we already are; a false **no** would write `_error`
/// under a handler that uses `error`, and that is a program which does not
/// compile. Since the walk counts every word of an interpolated string's raw
/// text, an `f"{error}"` is a yes.
pub fn block_mentions(parsed: &Parsed, block: &crate::ast::Block, name: &str) -> bool {
    let mut found = BTreeSet::new();
    names_in_block(parsed, block, &mut found);
    found.contains(name)
}

/// Every name an expression mentions.
///
/// Deliberately over-approximate: a field access `a.b` contributes `a`, and a
/// name that happens to be a function's rather than a variable's is counted
/// too. Both make the answer "keep the order", which is the safe direction.
///
/// **Total, with no `_` arm**, and that is load-bearing rather than tidy. This
/// is asked about a `catch` handler as well as about a value, and a handler
/// holds whatever anyone writes - a block, an `f"…"`, a grammar. A variant this
/// walked past silently would be a data dependency it could not see, which is
/// the one direction the analysis may never fail in (D9's first row: "no, and
/// must not"). Where the names cannot be found structurally - inside the holes
/// of an interpolated string, inside a `dsl` body - every word of the raw text
/// counts as one. **Written in Nikaia** (`tools/names.nika`, #125).
pub(super) fn names_in(parsed: &Parsed, expr: &Expr, out: &mut BTreeSet<String>) {
    nikaia_std::tools::names::names_in(expr, &parsed.interner, out);
}

/// Every name the statements of a block mention, the names a statement binds
/// left out (`tools/names.nika`, #125).
pub(super) fn names_in_block(
    parsed: &Parsed,
    block: &crate::ast::Block,
    out: &mut BTreeSet<String>,
) {
    nikaia_std::tools::names::names_in_block(block, &parsed.interner, out);
}

/// **Every `overlap { … }` block in a program, and what it was allowed**
/// ([ADR-050](../../../docs/specification/adr/adr-050.md) D3).
///
/// The answer to *"did these branches actually run together"*, which is the
/// question a language without an `allow_parallel` owes its user. Nothing prints
/// it on its own: it is asked for (`--overlaps`), because a compiler that
/// volunteered a paragraph per block would be noise in exactly the programs that
/// are fine.
///
/// **It used to be a report about pairs the compiler chose**, and D1 withdrew
/// that choice — statements run in the order they are written. So the report is
/// about the blocks the *programmer* wrote, and what it has to say is the thing
/// the source does not show: **which branches are started first** (D6). A block
/// whose branches meet on anything never reaches here, because `NK2104` refused
/// the program.
pub fn overlap_report(
    parsed: &Parsed,
    own: &Ledger,
    library: &Ledger,
    pauses: Pauses<'_>,
) -> String {
    let mut out = String::new();
    for item in &parsed.program.items {
        match &item.node {
            Item::Fn { .. } => {
                block_report(parsed, &item.node, None, own, library, pauses, &mut out)
            }
            Item::Impl {
                target, methods, ..
            } => {
                let target = parsed.text(target.name).to_string();
                for method in methods {
                    block_report(
                        parsed,
                        &method.node,
                        Some(&target),
                        own,
                        library,
                        pauses,
                        &mut out,
                    );
                }
            }
            _ => {}
        }
    }
    if out.is_empty() {
        out.push_str("this program writes no `overlap { … }` block.\n");
    }
    out
}

/// Whether a branch can pause, which is what decides the starting order (D6).
///
/// A function and not a lookup here, because no switch and no build setting may
/// reach this file (ADR-033 §8.2b): the answer is the *emitter's*, read off the
/// ledger's `sync` column and the checker's answers about method calls, and no
/// analysis in this file may reach for either.
pub type Pauses<'a> = &'a dyn Fn(&Spanned<Stmt>) -> bool;

fn block_report(
    parsed: &Parsed,
    item: &Item,
    target: Option<&str>,
    own: &Ledger,
    library: &Ledger,
    pauses: Pauses<'_>,
    out: &mut String,
) {
    let Item::Fn { name, body, .. } = item else {
        return;
    };
    let own_name = match name {
        Some(name) => parsed.text(*name).to_string(),
        None => "new".to_string(),
    };
    let key = match target {
        Some(target) => format!("{target}::{own_name}"),
        None => own_name,
    };

    let mut blocks = Vec::new();
    crate::emit::visit_block(body, &mut |expr| {
        if let Expr::Overlap(block) = expr {
            blocks.push(block.clone());
        }
    });

    for block in &blocks {
        // **The header is read off the pairs rather than asserted.** It said
        // *"which meet on nothing"* about every block, including one the checker
        // refuses on the next line - and this report exists because *"the
        // refusals are the compiler's own … that is only fair if the refusals
        // can be asked about"*. A report that answers the question wrongly is
        // worse than one that is not there.
        //
        // The first refused pair is the one named, for the reason `check` stops
        // after one finding per branch: a block that meets on two things has one
        // thing wrong with it.
        let operations: Vec<Option<Operation>> = block
            .stmts
            .iter()
            .map(|stmt| operation(parsed, &stmt.node, own, library))
            .collect();
        let mut refused: Option<(usize, usize, Verdict)> = None;
        'pairs: for (i, earlier) in operations.iter().enumerate() {
            for (j, later) in operations.iter().enumerate().skip(i + 1) {
                let (Some(earlier), Some(later)) = (earlier, later) else {
                    continue;
                };
                let seen = verdict(earlier, later);
                if !seen.is_overlap() {
                    refused = Some((i, j, seen));
                    break 'pairs;
                }
            }
        }

        let mut lines = Vec::new();
        for stmt in &block.stmts {
            let named = match accounted(parsed, &stmt.node, own, library) {
                Accounted::Operation(operation) => operation.callee,
                _ => "…".to_string(),
            };
            // D6's two halves, said as they happen: a branch that can pause is
            // started first and gives the thread up at its first suspension
            // point; one that cannot runs while the others are in flight.
            let when = match pauses(stmt) {
                true => "started first",
                false => "runs while they wait",
            };
            lines.push(format!("    {when:22} {named}"));
        }
        if lines.is_empty() {
            continue;
        }
        match &refused {
            None => out.push_str(&format!(
                "{key}: an `overlap` of {} branches, which meet on nothing\n",
                block.stmts.len()
            )),
            Some((i, j, seen)) => out.push_str(&format!(
                "{key}: an `overlap` of {} branches, and branches {} and {} may not \
                 run together - {}\n",
                block.stmts.len(),
                i + 1,
                j + 1,
                seen.why()
            )),
        }
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
    }
}
