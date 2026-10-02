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

use std::collections::BTreeSet;

use crate::ast::{Expr, Item, Spanned, Stmt};
use crate::parser::Parsed;

use super::Ledger;

/// **The verdict half is written in Nikaia** (`tools/order.nika`, #125): a
/// statement reduced (`Operation`) or the reason it could not be
/// (`Accounted`), and whether two of them, or a run, keep their order
/// (`Verdict`, `verdict`, `group_verdict`, `group_of`). What stays here is the
/// walk that does the reducing.
pub use nikaia_std::tools::order::{
    Accounted, Operation, Verdict, group_of, group_verdict, verdict,
};

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
/// **The walk is written in Nikaia** (`tools/order.nika`, #125): a `let` of
/// one name or a bare expression statement, looked through a `catch` whose
/// handler cannot divert (ADR-034), taken apart into the calls it performs -
/// literals, calls and operators over them, and a reason everywhere else -
/// and each call answered from the ledgers: what it touches, whether a
/// failure leaves uncaught, whether its result may cross back from the closure
/// overlapping builds (ADR-005 §1 Group B, `Destination::Ours`).
pub fn accounted(parsed: &Parsed, stmt: &Stmt, own: &Ledger, library: &Ledger) -> Accounted {
    nikaia_std::tools::order::accounted(
        stmt,
        &parsed.interner,
        &nikaia_std::tools::threads::Walking {
            own,
            library,
            into: super::send::Destination::Ours,
        },
    )
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
