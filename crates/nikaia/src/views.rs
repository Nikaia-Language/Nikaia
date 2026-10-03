// crates/nikaia/src/views.rs
//
// Where a naked view parameter's view ends up - `NK2302`.
//
// **Written in Nikaia** (`tools/views.nika`, #125): a view parameter a
// function stores - into a field of its subject, into a struct it builds, into
// its result or a task - is refused where nothing names the buffer it points
// into, and written as a view of the subject's (or a struct parameter's)
// buffer where something does (ADR-283 D1). The reasoning, the four places
// the reach stops and why the walk errs the way it does are in that file.
// What stays here is the one question a walk of the tree cannot answer -
// whether a `let`'s value is a buffer of its own (`contracts::tether`,
// ADR-283) - and the map `contracts::keeps` reads.

use std::collections::{HashMap, HashSet};

use winnow_grammar::Symbol as Ident;

use crate::ast::{Expr, Item, Type, VariantFields};
use crate::check::Finding;
use crate::contracts::Ledger;
use crate::parser::Parsed;

pub use nikaia_std::tools::views::{Destination, Stored};

/// Every naked view parameter in this unit whose view is stored.
pub fn analyse(parsed: &Parsed, own: &Ledger, library: &Ledger) -> Vec<Stored> {
    nikaia_std::tools::views::views_stored(
        &asked(parsed, own, library),
        &parsed.program.items,
        &|value: &Expr| own_buffer(parsed, value, own, library),
    )
}

/// The refusals: every stored naked view whose destination names no buffer.
pub fn check(parsed: &Parsed, own: &Ledger, library: &Ledger) -> Vec<Finding> {
    nikaia_std::tools::views::views_checked(
        &asked(parsed, own, library),
        &parsed.program.items,
        &|value: &Expr| own_buffer(parsed, value, own, library),
    )
    .into_iter()
    .map(crate::traits::from_nikaia)
    .collect()
}

/// The parameters the emitter writes as views of the subject's buffer, by the
/// byte the method they belong to starts at.
pub fn carried(parsed: &Parsed, own: &Ledger, library: &Ledger) -> HashMap<usize, HashSet<Ident>> {
    let mut out: HashMap<usize, HashSet<Ident>> = HashMap::new();
    for stored in analyse(parsed, own, library) {
        if stored.carried {
            let at = out.entry(stored.method as usize).or_default();
            at.insert(stored.symbol);
            // The struct parameter that carries it is written over the same
            // buffer.
            at.extend(stored.carrier);
        }
    }
    out
}

fn asked<'a>(
    parsed: &'a Parsed,
    own: &'a Ledger,
    library: &'a Ledger,
) -> nikaia_std::tools::views::Asked<'a> {
    nikaia_std::tools::views::Asked {
        names: &parsed.interner,
        own,
        library,
    }
}

/// **A buffer of its own carries no view of the parameter**
/// ([ADR-283](../../docs/specification/adr/adr-283.md)):
/// `fs::read_to_string(path, …)` mentions `path` and hands back a `String` it
/// read, so a view cut from that text is a view of the text.
fn own_buffer(parsed: &Parsed, value: &Expr, own: &Ledger, library: &Ledger) -> bool {
    matches!(
        crate::contracts::tether::makes_a_buffer(parsed, value, own, library),
        crate::contracts::tether::Buffer::Named(_)
    )
}

/// Every struct's and enum's fields, by the name that declares them.
///
/// An enum's variants are flattened into it: for the one question this map is
/// asked - does this field hold a view - a variant's field is a field of the
/// enum.
pub(crate) fn fields_of(parsed: &Parsed) -> HashMap<Ident, Vec<(String, Type)>> {
    let mut out: HashMap<Ident, Vec<(String, Type)>> = HashMap::new();
    for item in &parsed.program.items {
        match &item.node {
            Item::Struct { name, fields, .. } => {
                out.insert(
                    *name,
                    fields
                        .iter()
                        .map(|f| (parsed.text(f.name).to_string(), f.ty.clone()))
                        .collect(),
                );
            }
            Item::Enum { name, variants, .. } => {
                let mut carried = Vec::new();
                for variant in variants {
                    match &variant.fields {
                        VariantFields::Unit => {}
                        VariantFields::Tuple(types) => {
                            for (i, ty) in types.iter().enumerate() {
                                carried.push((i.to_string(), ty.clone()));
                            }
                        }
                        VariantFields::Named(fields) => {
                            for field in fields {
                                carried
                                    .push((parsed.text(field.name).to_string(), field.ty.clone()));
                            }
                        }
                    }
                }
                out.insert(*name, carried);
            }
            _ => {}
        }
    }
    out
}
