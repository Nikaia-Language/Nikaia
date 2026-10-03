// crates/nikaia/src/contracts/tether.rs
//
// Which of Part I 6.6's three states each view in a signature is in
// ([ADR-283](../../../../docs/specification/adr/adr-283.md) D2, D5).
//
// ## What this is, and what it deliberately is not
//
// D2 gives every view one of three states and says the compiler picks the least
// one that makes the program valid:
//
//     Borrowed ⊑ Tethered ⊑ Owned
//     Transient = borrow. Escaping = tether. Copying = yours to ask for.
//
// **Only the first of those is built as a representation.** Borrowed is the
// language below's own lifetime and costs nothing; Owned is `.to_owned()`,
// written by the program; and **Tethered is not built at all** — where a value
// would tether, the program is refused today (`NK2302`, or the backend on the
// Nikaia line for a view of a local that escapes). So this file computes the
// state and **changes no lowering**. It writes a column, which is D7, and the
// column is what a later change reads when the representation exists.
//
// That order is deliberate: an analysis whose answers nothing depends on can be
// held against the whole corpus and read, and being wrong costs a wrong line in
// a file rather than a wrong program.
//
// ## Owned is never this file's word
//
// D5: *promotion to `Owned` is never automatic*. A `.to_owned()` hands back a
// `String`, which is not a view at all — so no **view** position is ever Owned,
// and the lattice this file solves over is the two states below it. The top of
// the lattice exists for the program to reach, not for the analysis to assign.
//
// ## Which way it errs
//
// **Towards Tethered**, which is D7's own polarity for the case it names: a
// state barrier *widens* to Tethered, *"the widest representation, never to
// Owned"*. A position this file cannot decide is therefore Tethered, and the
// cost of being wrong that way is a representation wider than it had to be —
// never a program that does the wrong thing. The other direction would tell a
// later reader that a value borrows when it escapes, which is the use-after-free
// the state exists to prevent.
//
// ## What is not here, and why
//
// **D7's state barrier is not applied.** *Crossing a `dyn` boundary or a
// published non-generic API is a state barrier* is a consequence of there being
// three representations and one ABI to choose; with one representation there is
// no barrier to cross. It belongs with the layouts rather than with the
// analysis, and marking every `pub` signature Tethered today would fill the
// column with a state nothing has.
//
// **The buffer table is not here either** (D4). Its shape is a fact about a
// container's representation, and this file writes no representation.

// **The analysis is written in Nikaia** (`tools/tether.nika`, #125): the
// states one function's views solve to, whether a parse's result may point
// into its input, and whether an expression makes a buffer of its own. What
// stays here is writing the states into the ledger, the `--tethers` report
// and the refusal, both of which read the keep plan.

use crate::contracts::LedgerOps;
pub use nikaia_std::tools::tether::Buffer;
pub use nikaia_std::tools::tether::PackageTypes as Declared;
pub use nikaia_std::tools::ty::{Held, State};
use std::collections::BTreeMap;

use crate::ast::{Item, Type};
use crate::parser::Parsed;

use super::{INPUT, Ledger};

/// The position a function's result occupies in the column.
///
/// The same spelling `sharing` uses for the same thing, because they are the
/// same position read by two analyses.
pub const RESULT: &str = "<result>";

/// What stays Rust of a [`Held`] (ADR-294 step (b)): reading one back.
pub trait HeldOps: Sized {
    fn parse(text: &str) -> Option<Self>;
}

impl HeldOps for Held {
    fn parse(text: &str) -> Option<Held> {
        nikaia_std::tools::ledger::held_of(text)
    }
}

/// Give every entry of this package the states its views solved to (D7):
/// each function's (`function_views`), and a `pub` rule's whose result may
/// point into the text it parsed - the entry holds the input and the result
/// borrows it ([ADR-296](../../../../docs/specification/adr/adr-296.md) D24).
/// Then which positions really leave, over the whole package, to a fixpoint
/// (`keep::infer`, [ADR-283](../../../../docs/specification/adr/adr-283.md)
/// D6).
pub fn infer(ledger: &mut Ledger, units: &[&Parsed], library: &Ledger) {
    let mut solved: BTreeMap<String, Vec<Held>> = BTreeMap::new();
    let package = declared_in(units);
    for parsed in units.iter().copied() {
        // By **name**: a `Symbol` belongs to the parse that interned it.
        let borrowing = crate::emit::borrowing_structs(parsed)
            .into_iter()
            .map(|s| parsed.text(s).to_string())
            .collect();
        let mut keep = |key: String, held: Vec<Held>| {
            if !held.is_empty() {
                solved.insert(key, held);
            }
        };
        for item in &parsed.program.items {
            match &item.node {
                Item::Fn { name, .. } => keep(
                    function_key(parsed, *name, None),
                    nikaia_std::tools::tether::function_views(
                        &item.node,
                        None,
                        &parsed.interner,
                        &borrowing,
                    ),
                ),
                Item::Impl {
                    target, methods, ..
                } => {
                    let target = parsed.text(target.name).to_string();
                    for method in methods {
                        let Item::Fn { name, .. } = &method.node else {
                            continue;
                        };
                        keep(
                            function_key(parsed, *name, Some(&target)),
                            nikaia_std::tools::tether::function_views(
                                &method.node,
                                Some(&target),
                                &parsed.interner,
                                &borrowing,
                            ),
                        );
                    }
                }
                Item::Grammar(def) => {
                    let named = parsed.text(def.name).to_string();
                    for rule in def.rules.iter().filter(|r| r.is_public) {
                        if a_parse_that_views(parsed, rule.ret_type.as_ref(), &package) {
                            keep(
                                format!("{named}::{}", parsed.text(rule.name)),
                                vec![
                                    Held {
                                        position: INPUT.to_string(),
                                        state: State::Borrowed,
                                    },
                                    Held {
                                        position: RESULT.to_string(),
                                        state: State::Borrowed,
                                    },
                                ],
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
    for (key, held) in solved {
        if let Some(contract) = ledger.functions.get_mut(&key) {
            contract.views = held;
        }
    }
    super::keep::infer(ledger, units, library);
}

/// `Type::method`, or the function's own name; `new` for Part I 4.2's
/// anonymous constructor.
fn function_key(
    parsed: &Parsed,
    name: Option<winnow_grammar::Symbol>,
    target: Option<&str>,
) -> String {
    let own = match name {
        Some(name) => parsed.text(name).to_string(),
        None => "new".to_string(),
    };
    match target {
        Some(target) => format!("{target}::{own}"),
        None => own,
    }
}

/// **A name declared `String` holds text of this frame's own**
/// ([ADR-282](../../../docs/specification/adr/adr-282.md) D4).
pub(crate) fn declares_text(parsed: &Parsed, ty: Option<&crate::ast::Type>) -> bool {
    nikaia_std::tools::tether::declares_text(&parsed.interner, ty)
}

/// Whether this expression hands back a buffer of its own
/// (`tools/tether.nika`).
pub(crate) fn makes_a_buffer(
    parsed: &Parsed,
    expr: &crate::ast::Expr,
    own: &Ledger,
    library: &Ledger,
) -> Buffer {
    nikaia_std::tools::tether::makes_a_buffer(
        expr,
        &nikaia_std::tools::views::Asked {
            names: &parsed.interner,
            own,
            library,
        },
        &|name: &str| parsed.unaliased(name),
    )
}

/// What a **package's** declarations say about the types a parse can hand
/// back, read off the units in one pass.
pub(super) fn declared_in(units: &[&Parsed]) -> Declared {
    let mut package = Declared::empty();
    for parsed in units.iter().copied() {
        nikaia_std::tools::tether::declare_unit(
            &parsed.interner,
            &parsed.program.items,
            &mut package,
        );
    }
    package
}

/// Whether a grammar entry's result **may point into the text it parsed**
/// (`tools/tether.nika`); `keeps` asks it too.
pub(super) fn a_parse_that_views(parsed: &Parsed, ret: Option<&Type>, package: &Declared) -> bool {
    nikaia_std::tools::tether::a_parse_that_views(&parsed.interner, ret, package)
}

/// `--tethers`: what the analysis solved, per function
/// ([ADR-283](../../../../docs/specification/adr/adr-283.md) D4).
///
/// *The inverse tool is inspection, not assertion*, and it is the half of D6
/// that survived: the assertion beside it is gone
/// ([ADR-283](../../../../docs/specification/adr/adr-283.md) D4) and this shows
/// what was chosen without being asked. Read off the ledger the build produced
/// rather than computed again here, so that what a person reads is what the file
/// records.
pub fn report(parsed: &Parsed, ledger: &Ledger) -> String {
    // **This file's own entries**, because a report is about one file and the
    // ledger is the package's. A key names a function of this unit exactly when
    // this unit's own inference produced it.
    let here = Ledger::infer(parsed);
    let mut lines: Vec<String> = Vec::new();
    // **What a `String` field or result is below** (ADR-282 D26), where it is
    // not text of its own.
    if !parsed.text_tiers.is_empty() {
        lines.push("text (declared `String`):\n".to_string());
        for line in &parsed.text_tiers {
            lines.push(format!("    {line}\n"));
        }
    }
    for (key, contract) in &ledger.functions {
        if contract.views.is_empty() || !here.functions.contains_key(key) {
            continue;
        }
        lines.push(format!("{key}:\n"));
        for held in &contract.views {
            lines.push(format!(
                "    {:<9} `{}`\n",
                held.state.as_str(),
                held.position
            ));
        }
    }
    // **And where each buffer lives** (ADR-283 D13): the keep plan, per
    // function, in the words a reader asks the question in.
    let library = crate::contracts::std_ledger();
    for plan in super::keep::plans(parsed, ledger, library) {
        let mut said = Vec::new();
        for (at, keep) in &plan.puts {
            // By the name the buffer is bound to, which is what a reader looks
            // for in the source.
            let name = plan
                .escapes
                .iter()
                .find_map(|(source, _)| match source {
                    super::keep::Source::Buffer { at: a, name, .. } if a == at => {
                        Some(format!("`{name}`"))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| "a buffer".to_string());
            said.push(format!(
                "    {name} lives {}\n",
                super::keep::describe(*keep)
            ));
        }
        for ((_, callee), keep) in &plan.calls {
            said.push(format!(
                "    what `{callee}` reads lives {}\n",
                super::keep::describe(*keep)
            ));
        }
        for keeper in &plan.element_keepers {
            // ADR-283: a struct of views carries a handle on each buffer it
            // points into, and the report says how many that is.
            let line = match plan.struct_keepers.contains(keeper) {
                true => {
                    let width = plan.widths.get(keeper).copied().unwrap_or(1);
                    let handles = match width {
                        1 => "a handle on the buffer it points into".to_string(),
                        n => format!("a handle on each of the {n} buffers it points into"),
                    };
                    format!("    `{keeper}` holds each struct with {handles}\n")
                }
                false => format!("    `{keeper}` holds each view with its own handle\n"),
            };
            said.push(line);
        }
        if said.is_empty() {
            continue;
        }
        lines.push(format!("{} (keeps):\n", plan.key));
        lines.extend(said);
    }
    match lines.is_empty() {
        // "here" and not "in this program", for the reason `sharing`'s report
        // gives: a report is about one file.
        true => "no view in a signature here, so there is no state to solve.\n".to_string(),
        false => lines.concat(),
    }
}

// ---------------------------------------------------------------------------
// The refusal: where the tether would be needed and is not built
// ---------------------------------------------------------------------------

/// A view handed back that points into a buffer the body **owns** (`NK2303`).
///
/// This is the one shape Part I 6.6's `Tethered` exists for, and
/// [ADR-283](../../../../docs/specification/adr/adr-283.md) D9 is what to do
/// about it while the state is not built: refuse on the Nikaia line, naming the
/// buffer and the mechanism, rather than lower a function whose result outlives
/// the buffer it points into and let `rustc` speak about the generated file
/// ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
///
/// **It stands on a named buffer and never on a doubt.** [`Buffer::Unknown`] is
/// a call no ledger describes; the *column* errs towards Tethered for it,
/// because a wide state costs a wide representation. A refusal cannot err that
/// way — refusing a correct program is the worse of the two mistakes
/// ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)) — so this
/// walk asks for the buffer **by name** and for the returned expression to be
/// derived from it. Where either is missing the program is lowered exactly as
/// it was before this check existed.
pub fn check(parsed: &Parsed, own: &Ledger, library: &Ledger) -> Vec<crate::check::Finding> {
    // **Where a view outlives its buffer is the keep plan's answer**
    // ([ADR-283](../../../../docs/specification/adr/adr-283.md)): it follows
    // views through `push`, `for`, fields and calls, which the walk that stood
    // here did not, and it refuses only what it cannot lower or what no
    // declaration permits.
    super::keep::plans(parsed, own, library)
        .into_iter()
        .flat_map(|plan| plan.refusals)
        .collect()
}
