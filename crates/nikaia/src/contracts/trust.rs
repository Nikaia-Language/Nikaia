// crates/nikaia/src/contracts/trust.rs
//
// Where a program's bytes came from (ADR-010).
//
// **The adapter of `nikaia-std/src/tools/trust.nika`** (ADR-294 D1, D2): the
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
// **One buffer, because Stage 0 has one input lifetime.** ADR-283's model gives
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

use crate::ast::Span;
use crate::contracts::LedgerOps;
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

/// What `--trust` names where a method call could not be resolved, so the
/// program has no proof of who chose the bytes it read.
pub const UNRESOLVED: &str = "a method call this compiler could not resolve";

/// The provenance of this program's input.
pub fn analyse(parsed: &Parsed, library: &Ledger) -> Trust {
    let mut reasons: Vec<Reason> = Vec::new();
    let mut roots: Vec<Root> = Vec::new();

    // **The walk is Nikaia** (`tools/foreign.nika`, #125): every call by name
    // a body makes, with what it hands a parameter called `root` where the
    // callee has one.
    let root_at = |name: &str| -> i64 {
        library
            .lookup(name)
            .and_then(|(_, contract)| contract.signature.as_ref())
            .and_then(|s| s.arguments().iter().position(|(param, _)| param == "root"))
            .map_or(-1, |at| at as i64)
    };
    for seen in crate::foreign::seen(parsed, &root_at) {
        let crate::foreign::Seen::Call { name, wrote, span } = seen else {
            continue;
        };
        let Some((key, contract)) = library.lookup(&name) else {
            continue;
        };
        if let Some(provenance) = contract.provenance
            && !reasons.iter().any(|r| r.source == key)
        {
            reasons.push(Reason {
                source: key.clone(),
                provenance,
            });
        }
        if let Some(wrote) = wrote {
            roots.push(Root {
                entry: key,
                wrote,
                span,
            });
        }
    }

    // **A source read through a method** (#472, ADR-288): `c.read()` is
    // `net::Connection::read` once the receiver's type is known, which only
    // the checker knows. The ledger's own pass asks it and records each
    // function's resolved keys.
    //
    // **And one it could not resolve counts as untrusted** (ADR-010 D1: a
    // barrier widens, never the other way). A map that gets the keyed hash
    // needlessly is slower; the opposite is an attack.
    let (_, checked) = Ledger::infer_package_checked(&[parsed], library);
    let mut unresolved = false;
    for calls in checked.iter().flat_map(|c| c.methods.values()) {
        unresolved |= calls.unresolved;
        for key in &calls.resolved {
            let Some((key, contract)) = library.lookup(key) else {
                continue;
            };
            if let Some(provenance) = contract.provenance
                && !reasons.iter().any(|r| r.source == key)
            {
                reasons.push(Reason {
                    source: key,
                    provenance,
                });
            }
        }
    }
    if unresolved {
        reasons.push(Reason {
            source: UNRESOLVED.to_string(),
            provenance: Provenance::Untrusted,
        });
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
