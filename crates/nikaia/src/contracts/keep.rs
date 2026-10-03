// crates/nikaia/src/contracts/keep.rs
//
// Where a buffer lives once views of it outlive the scope that made it
// ([ADR-283](../../../../docs/specification/adr/adr-283.md), Part I 6.6).
//
// ## The question, and the one answer to it
//
// A body reads a file into `text`, cuts it into views, and something keeps the
// views longer than `text` would live: the result, a `mut` parameter, a list
// declared outside the loop, a task. [ADR-283](../../../../docs/specification/adr/adr-283.md)
// calls that value **tethered** and says the buffer has to live as long as the
// views do. This file decides **where** it lives, and the answer is always the
// same rule (D1):
//
// > **A buffer lives in the keep of whatever keeps its views.**
//
// What differs is who owns that keep, and that is read off facts this compiler
// already has rather than chosen once for every program (D2-D4):
//
// * **a frame** - the nearest caller, or this function's own frame for a list
//   declared outside the loop. Costs nothing: no count, no handle, and the body
//   may pause (D2);
// * **a handle that travels with the value**, where no frame outlives it - a
//   task (D3);
// * **a handle per element**, where a container keeps views across a loop and
//   drops entries as it goes - one keep for the loop would keep every buffer it
//   ever read (D4).
//
// ## How a view is followed
//
// By **origin**: every local carries the set of places its views may point
// into. A `let` whose initialiser makes a buffer (`makes_a_buffer`, the same
// question the column above asks) is an origin; so is a call to a function that
// itself hands back tethered views, because its buffer needs a keep from here.
// Origins flow through methods, fields, indexing, `for` bindings, struct and
// list literals, `push` and `insert`, and they stop at anything that hands back
// something of its own: `to_owned`, `len`, a call whose declared result holds no
// view. That last stop is the one `views::check` was missing, and why it blamed
// `path` for a view of the text `fs::read_to_string(path)` read.
//
// ## Which way it errs
//
// **Towards a keep, never towards a refusal.** A keep nobody needed costs a
// buffer living until the end of a scope instead of the end of a statement. A
// refusal nobody needed is a correct program refused
// ([Part III C.4](../../../../docs/specification/30-nikaia-tooling.md)). So a
// destination whose type this walk cannot read is taken to hold a view, and a
// refusal is only ever raised where the type says so.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Expr, Span};
use crate::parser::Parsed;

use super::Ledger;
use super::tether::{RESULT, State};
/// What a whole unit says about views, which types hold one and what the
/// structs among them are made of, with the questions asked of a type, a
/// method's name and a ledger: `tools/keep.nika` (ADR-294, #125).
use nikaia_std::tools::keep::KeepContext as Context;
pub use nikaia_std::tools::keep::takes_a_keep;

/// A source, by the statement it stands in and - for a call - the callee:
/// what the emitter has in hand where it writes either.
pub type Id = (usize, String);

/// Who owns the keep a buffer, or a call's buffers, go into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeepAt {
    /// The keep this function was **given**: its views leave through the
    /// result, a `mut` parameter or the subject (D2).
    Param,
    /// A keep declared first in this function's body, for views kept by a
    /// local that outlives the scope the buffer was read in (D2).
    Frame,
    /// A keep declared just before the statement that needs it: a call whose
    /// views go nowhere further than this scope.
    Local(usize),
    /// The shared keep of this function's tasks (D3).
    Task,
    /// A keep of its own, one per buffer, for a container that drops entries
    /// (D4). The number is the statement that reads the buffer.
    Element(usize),
}

/// The words `--tethers` uses for a keep.
pub fn describe(keep: KeepAt) -> &'static str {
    match keep {
        KeepAt::Param => "in the caller's keep, because views of it leave this function",
        KeepAt::Frame => {
            "in a keep of this function until it returns, because something outside the \
             loop or block keeps views of it"
        }
        KeepAt::Local(_) => "in a keep beside the call, as long as what the call hands back",
        KeepAt::Task => "in a keep the task carries with it (one count per task, not per view)",
        KeepAt::Element(_) => {
            "in a keep of its own, held by each view kept of it, because what keeps the \
             views drops entries while the loop goes on"
        }
    }
}

/// Where a view leaves the scope that made it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Escape {
    /// Handed back to the caller.
    Result,
    /// Kept by a `mut` parameter, or by the subject.
    Param(String),
    /// Kept by a local that outlives the buffer's scope.
    Outer(String),
    /// Captured by a task.
    Task,
}

/// Something views may point into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `let text = fs::read_to_string(…)`: a buffer this body makes.
    Buffer {
        at: usize,
        name: String,
        ty: String,
        span: Span,
    },
    /// `load(path)`, where `load` hands back tethered views: its buffers need a
    /// keep from here.
    Call {
        at: usize,
        callee: String,
        span: Span,
    },
}

impl Source {
    pub fn at(&self) -> usize {
        match self {
            Source::Buffer { at, .. } | Source::Call { at, .. } => *at,
        }
    }

    /// How a message names it.
    pub fn named(&self) -> String {
        match self {
            Source::Buffer { name, ty, .. } => format!("`{name}` (a `{ty}` this body reads)"),
            Source::Call { callee, .. } => format!("what `{callee}` returns"),
        }
    }
}

/// One function's answer: where each of its buffers lives, and what the
/// lowering writes for it.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// The ledger key.
    pub key: String,
    /// The body's first statement, before which the function's own keeps are
    /// declared.
    pub first: Option<usize>,
    /// It takes a keep from its caller (D2), and the ledger says so.
    pub takes_keep: bool,
    /// It declares a keep first in its body.
    pub frame_keep: bool,
    /// It declares the shared keep of its tasks first in its body (D3).
    pub task_keep: bool,
    /// Statements before which a keep of their own is declared.
    pub local_keeps: BTreeSet<usize>,
    /// A buffer `let`, by its statement, and the keep it goes into.
    pub puts: BTreeMap<usize, KeepAt>,
    /// A call to a function that takes a keep, by its statement and callee.
    pub calls: BTreeMap<(usize, String), KeepAt>,
    /// Bindings a task captures whose views need the task's keep (D3): the
    /// name, and the statement that binds it.
    pub tethered: BTreeMap<String, usize>,
    /// Locals whose views are held one handle per element (D4): a view of
    /// text as a `Held`, a struct of views as a `Holding`
    /// ([ADR-283](../../../../docs/specification/adr/adr-283.md) D14).
    pub element_keepers: BTreeSet<String>,
    /// The element keepers whose elements are **structs** of views, read
    /// through the handle rather than through `Deref` (ADR-283 D17).
    pub struct_keepers: BTreeSet<String>,
    /// How many keeps each element of a keeper carries: the most buffers any
    /// one value put into it points into (ADR-283 D15).
    pub widths: BTreeMap<String, usize>,
    /// The struct each such keeper holds, by the keeper's name: what a write
    /// through its handle asks about a field (ADR-283 D17).
    pub keeper_structs: BTreeMap<String, String>,
    /// Locals bound to an element **taken out** of such a keeper -
    /// `let old = kept.remove(0)` - which is still held and read through its
    /// handle (ADR-283 D16).
    pub held_locals: BTreeSet<String>,
    /// Statements that put a view into such a local: by statement and local,
    /// which argument of the call carries it and the buffer statements whose
    /// keeps it is held with.
    pub holds: BTreeMap<(usize, String), BTreeMap<usize, BTreeSet<usize>>>,
    /// Every place a view leaves its scope, for the report and the refusals.
    pub escapes: Vec<(Source, Escape)>,
    /// What cannot be lowered, said in this language's words.
    pub refusals: Vec<crate::check::Finding>,
    /// **A kept buffer assigned again**, by the assignment's statement, and
    /// the keep its new value goes into as well: the views of the old value
    /// still point into the old one, which the keep goes on holding, and the
    /// binding is a place in the keep, so the new value has to be one too
    /// (#293).
    pub reputs: BTreeMap<usize, KeepAt>,
}

impl Plan {
    /// Whether anything in this function moved.
    pub fn is_empty(&self) -> bool {
        !self.takes_keep
            && !self.frame_keep
            && !self.task_keep
            && self.local_keeps.is_empty()
            && self.puts.is_empty()
            && self.calls.is_empty()
            && self.refusals.is_empty()
    }
}

/// Every function of a package, planned, with the `views` column of each
/// function that takes a keep set to `tethered` (D6).
///
/// **A fixpoint**, because a function takes a keep exactly when one of its
/// sources leaves through its result or a parameter, and a source may be a call
/// to another function that takes one. Monotone - a function never stops taking
/// a keep once it does - so it ends.
pub fn infer(ledger: &mut Ledger, units: &[&Parsed], library: &Ledger) {
    for _ in 0..32 {
        let mut changed = false;
        for parsed in units.iter().copied() {
            for plan in plans(parsed, ledger, library) {
                let Some(contract) = ledger.functions.get_mut(&plan.key) else {
                    continue;
                };
                for position in keeping_positions(&plan) {
                    match contract.views.iter_mut().find(|h| h.position == position) {
                        Some(held) if held.state == State::Tethered => {}
                        Some(held) => {
                            held.state = State::Tethered;
                            changed = true;
                        }
                        None => {
                            contract.views.push(super::tether::Held {
                                position,
                                state: State::Tethered,
                            });
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// The positions through which a plan's views leave.
fn keeping_positions(plan: &Plan) -> BTreeSet<String> {
    plan.escapes
        .iter()
        .filter_map(|(_, escape)| match escape {
            Escape::Result => Some(RESULT.to_string()),
            Escape::Param(name) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Every function of one unit, planned against the ledger as it stands.
///
/// **The walk and the plan are Nikaia** (`tools/buffers.nika`, ADR-294,
/// #125): which locals point into which buffers, where each view leaves, and
/// the keep each buffer goes into. What is left here is the plan in the types
/// the emitter reads.
pub fn plans(parsed: &Parsed, ledger: &Ledger, library: &Ledger) -> Vec<Plan> {
    use nikaia_std::tools::buffers::{self as nika, BufferAsk};
    let context = Context::of(&parsed.interner, &parsed.program.items);
    let ask = BufferAsk {
        names: &parsed.interner,
        own: ledger,
        library,
        items: &parsed.program.items,
        context: &context,
    };
    nika::buffer_plans(
        &parsed.program,
        &ask,
        &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
        &|path: &str| parsed.unaliased(path),
    )
    .into_iter()
    .map(from_nikaia)
    .collect()
}

/// A statement's place, as the walk counts it.
fn place(at: i64) -> usize {
    at as usize
}

fn keep_from(keep: nikaia_std::tools::buffers::KeepAt) -> KeepAt {
    use nikaia_std::tools::buffers::KeepAt as Nika;
    match keep {
        Nika::Param => KeepAt::Param,
        Nika::Frame => KeepAt::Frame,
        Nika::Local(at) => KeepAt::Local(place(at)),
        Nika::Task => KeepAt::Task,
        Nika::Element(at) => KeepAt::Element(place(at)),
    }
}

fn source_from(source: nikaia_std::tools::buffers::Source) -> Source {
    use nikaia_std::tools::buffers::Source as Nika;
    match source {
        Nika::Buffer { at, name, ty, span } => Source::Buffer {
            at: place(at),
            name,
            ty,
            span,
        },
        Nika::Call { at, callee, span } => Source::Call {
            at: place(at),
            callee,
            span,
        },
    }
}

fn escape_from(escape: nikaia_std::tools::buffers::Escape) -> Escape {
    use nikaia_std::tools::buffers::Escape as Nika;
    match escape {
        Nika::Result => Escape::Result,
        Nika::Param(name) => Escape::Param(name),
        Nika::Outer(name) => Escape::Outer(name),
        Nika::Task => Escape::Task,
    }
}

fn from_nikaia(plan: nikaia_std::tools::buffers::BufferPlan) -> Plan {
    Plan {
        key: plan.key,
        first: (plan.first >= 0).then(|| place(plan.first)),
        takes_keep: plan.takes_keep,
        frame_keep: plan.frame_keep,
        task_keep: plan.task_keep,
        local_keeps: plan.local_keeps.into_iter().map(place).collect(),
        puts: plan
            .puts
            .into_iter()
            .map(|(at, keep)| (place(at), keep_from(keep)))
            .collect(),
        calls: plan
            .calls
            .into_iter()
            .map(|((at, callee), keep)| ((place(at), callee), keep_from(keep)))
            .collect(),
        tethered: plan
            .tethered
            .into_iter()
            .map(|(name, at)| (name, place(at)))
            .collect(),
        element_keepers: plan.element_keepers,
        struct_keepers: plan.struct_keepers,
        widths: plan
            .widths
            .into_iter()
            .map(|(name, width)| (name, width as usize))
            .collect(),
        keeper_structs: plan.keeper_structs,
        held_locals: plan.held_locals,
        holds: plan
            .holds
            .into_iter()
            .map(|((at, local), by_arg)| {
                let by_arg = by_arg
                    .into_iter()
                    .map(|(arg, statements)| {
                        (place(arg), statements.into_iter().map(place).collect())
                    })
                    .collect();
                ((place(at), local), by_arg)
            })
            .collect(),
        escapes: plan
            .escapes
            .into_iter()
            .map(|(source, escape)| (source_from(source), escape_from(escape)))
            .collect(),
        refusals: plan
            .refusals
            .into_iter()
            .map(crate::traits::from_nikaia)
            .collect(),
        reputs: plan
            .reputs
            .into_iter()
            .map(|(at, keep)| (place(at), keep_from(keep)))
            .collect(),
    }
}

/// The ledger key of a method that takes a keep: `self.load(…)` inside its
/// `impl`, or a name only one keeping function in the ledger has. One answer
/// for the plan and for the emitter, so the call and its keep cannot part.
pub fn keeping_method_key(
    ledger: &Ledger,
    target: Option<&str>,
    on_self: bool,
    method: &str,
) -> Option<String> {
    nikaia_std::tools::keep::keeping_method_key(ledger, target, on_self, method)
}

/// The name a call's callee is written as.
pub fn callee_name(parsed: &Parsed, func: &Expr) -> Option<String> {
    nikaia_std::tools::keep::written_callee(&parsed.interner, func, &|path: &str| {
        parsed.unaliased(path)
    })
}

/// The local an expression is rooted in: `xs`, `self.items`, `m[k]`.
pub fn root_of(parsed: &Parsed, expr: &Expr) -> Option<String> {
    nikaia_std::tools::keep::root_of(&parsed.interner, expr)
}
