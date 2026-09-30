// crates/nikaia/src/contracts/trust.rs
//
// Where a program's bytes came from (ADR-010).
//
// **The adapter of `nikaia-std/src/tools/trust.nika`** (ADR-250 D1, D2): the
// module's decisions - which written root is a way around the root check, and
// what `--trust` says - are Nikaia. What stays here is what it reads, which is
// the compiler's: the call walk and the ledger, handed over as names, `bool`s
// and line numbers.
//
// `Trusted ⊑ Untrusted`, joined in the safe direction: one untrusted input
// makes the result untrusted. The sources are `std`'s, stated in its ledger
// (ADR-020) - a file the operator named, a pipe they connected, the arguments
// they typed - and a program's own trust is the join over the ones it calls.
//
// **One buffer, because Stage 0 has one input lifetime.** ADR-008's model gives
// a compilation unit a single lifetime, so it has a single input buffer: every
// view a parser cuts, every struct tied to it, and every key of a map keyed by
// a view point into the same thing. The join over a program's sources is
// therefore not a coarsening of a per-buffer analysis - it *is* the per-buffer
// analysis, for the one buffer this representation can express. When the
// representation grows more than one, this becomes the join over each.
//
// **A barrier widens to untrusted, never the other way** (ADR-010 D1). A call
// this compiler cannot resolve could be anything, so a program that reaches one
// whose result it then treats as input has no proof of who chose those bytes.
// What Stage 0 can reach is `std`, which states all of its sources, so today
// there is no such call - and `Reason::Unresolved` is what will carry it when
// there is.

use crate::ast::{Expr, Item, Span};
use crate::parser::Parsed;

use super::{Ledger, Provenance};
use crate::contracts::SignatureOps;

/// What the analysis concluded, and what it concluded it from.
#[derive(Debug, Clone)]
pub struct Trust {
    pub provenance: Provenance,
    /// Every source the program calls, in the order they were found, with what
    /// each contributed. This is what `--trust` prints: the choice is visible,
    /// never a mystery (ADR-010 D7).
    pub reasons: Vec<Reason>,
    /// Every path call whose root is a **word** rather than a directory
    /// ([ADR-108](../../../docs/specification/adr/adr-108.md) D4).
    ///
    /// `Anywhere` is the one way around the root check, and the security review
    /// of a program's file access is this list. A `Dir` whose root is the literal
    /// `"/"` is in it too, because that is the same thing in another spelling.
    pub roots: Vec<Root>,
}

#[derive(Debug, Clone)]
pub struct Reason {
    /// The source, as the ledger names it.
    pub source: String,
    pub provenance: Provenance,
}

/// One call that wrote a root the review wants to see
/// ([ADR-108](../../../docs/specification/adr/adr-108.md) D4).
#[derive(Debug, Clone)]
pub struct Root {
    /// The entry, as the ledger names it.
    pub entry: String,
    pub wrote: Wrote,
    /// The statement the call stands in, which is the granularity every other
    /// report here uses.
    pub span: Span,
}

/// Which of the two spellings D4 lists a root is, declared in Nikaia
/// (`nikaia-std/src/tools/trust.nika`).
pub use nikaia_std::tools::trust::Wrote;

/// The provenance of this program's input.
pub fn analyse(parsed: &Parsed, library: &Ledger) -> Trust {
    let mut reasons: Vec<Reason> = Vec::new();
    let mut roots: Vec<Root> = Vec::new();

    for item in &parsed.program.items {
        let bodies = match &item.node {
            Item::Fn { body, .. } => vec![body],
            Item::Impl { methods, .. } => methods
                .iter()
                .filter_map(|m| match &m.node {
                    Item::Fn { body, .. } => Some(body),
                    _ => None,
                })
                .collect(),
            _ => continue,
        };

        for body in bodies {
            super::sync::walk_calls(parsed, body, &mut |name| {
                let Some((key, contract)) = library.lookup(name) else {
                    return;
                };
                let Some(provenance) = contract.provenance else {
                    return;
                };
                if !reasons.iter().any(|r| r.source == key) {
                    reasons.push(Reason {
                        source: key,
                        provenance,
                    });
                }
            });
            roots_of(parsed, body, library, &mut roots);
        }
    }

    reasons.sort_by(|a, b| a.source.cmp(&b.source));
    roots.sort_by_key(|r| r.span.at());

    // A program that reads nothing has no input to distrust. Its maps are keyed
    // by what it wrote itself, which is the compiled-in case ADR-010 D2 calls
    // trusted.
    let provenance = reasons
        .iter()
        .map(|r| r.provenance)
        .fold(Provenance::Trusted, Provenance::join);

    Trust {
        provenance,
        reasons,
        roots,
    }
}

/// Every call in this block that wrote one of [`Wrote`]'s two spellings for its
/// root ([ADR-108](../../../docs/specification/adr/adr-108.md) D4).
///
/// **Which argument is the root comes from the ledger**, not from a list of
/// function names kept here: an entry's signature names a parameter `root`, and
/// the argument in that position is what this reads. So the day `fs::open` or
/// `http::File` is written, its sites are listed without a line changing here.
///
/// **Free calls only.** Every path-taking entry is a free function, and a method
/// call's receiver would have to be typed to find its entry — which is the thing
/// [ADR-028](../../../docs/specification/adr/adr-028.md) says a walk beside the
/// checker does not do.
fn roots_of(parsed: &Parsed, block: &crate::ast::Block, library: &Ledger, out: &mut Vec<Root>) {
    for stmt in &block.stmts {
        super::sync::visit_stmt(parsed, &stmt.node, &mut |expr| {
            let Expr::Call { func, args, .. } = expr else {
                return;
            };
            let Some(name) = call_name(parsed, expr) else {
                return;
            };
            let Some((key, contract)) = library.lookup(&name) else {
                return;
            };
            let _ = func;
            let Some(at) = contract
                .signature
                .as_ref()
                .and_then(|s| s.arguments().iter().position(|(param, _)| param == "root"))
            else {
                return;
            };
            let Some(wrote) = args.get(at).and_then(|arg| written_root(parsed, arg)) else {
                return;
            };
            out.push(Root {
                entry: key,
                wrote,
                span: stmt.span,
            });
        });
        super::sync::visit_stmt_blocks(&stmt.node, &mut |inner| {
            roots_of(parsed, inner, library, out)
        });
    }
}

/// Which of D4's two spellings this argument is, or nothing where it is a
/// directory the program worked out.
///
/// A `Dir` whose root is a **name** is the case this says nothing about, and
/// deliberately: what the name holds is a run-time value, and a report that
/// guessed would be a report a reviewer has to check.
fn written_root(parsed: &Parsed, arg: &Expr) -> Option<Wrote> {
    nikaia_std::tools::trust::written_root(&parsed.interner, arg)
}

/// What `nikaia --explain --trust` prints
/// ([ADR-010](../../../docs/specification/adr/adr-010.md) D7, and
/// [ADR-108](../../../docs/specification/adr/adr-108.md) D4's listing).
///
/// **The file and its text, because a site is a line.** Provenance is a
/// property of the program and wanted neither; a root is written somewhere, and
/// *the security review of a program's file access is that list* — which a list
/// without line numbers is not.
pub fn render(trust: &Trust, path: &str, source: &str) -> String {
    use nikaia_std::tools::trust as nika;
    let reasons: Vec<nika::Reason> = trust
        .reasons
        .iter()
        .map(|r| nika::Reason {
            source: r.source.clone(),
            untrusted: r.provenance == Provenance::Untrusted,
        })
        .collect();
    // **A site is a line and a column**, counted here where the source is:
    // what crosses into Nikaia is two numbers.
    let places: Vec<nika::Place> = trust
        .roots
        .iter()
        .map(|root| {
            let (line, column) = winnow_grammar::span::line_column(source, root.span.at());
            nika::Place {
                line: line as i64,
                column: column as i64,
                entry: root.entry.clone(),
                wrote: root.wrote,
            }
        })
        .collect();
    nika::render(
        trust.provenance == Provenance::Untrusted,
        &reasons,
        &places,
        path,
    )
}

/// Whether a call names something, for the walker below.
pub(super) fn call_name(parsed: &Parsed, expr: &Expr) -> Option<String> {
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
