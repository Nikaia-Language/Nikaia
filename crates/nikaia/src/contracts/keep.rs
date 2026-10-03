// crates/nikaia/src/contracts/keep.rs
//
// Where a buffer lives once views of it outlive the scope that made it
// ([ADR-209](../../../../docs/specification/adr/adr-209.md), Part I 6.6).
//
// ## The question, and the one answer to it
//
// A body reads a file into `text`, cuts it into views, and something keeps the
// views longer than `text` would live: the result, a `mut` parameter, a list
// declared outside the loop, a task. [ADR-008](../../../../docs/specification/adr/adr-008.md)
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

use crate::contracts::LedgerOps;
use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Block, Expr, Item, Span, Stmt, Type};
use crate::parser::Parsed;

use super::Ledger;
use super::tether::{Buffer, RESULT, State};
/// What a whole unit says about views, which types hold one and what the
/// structs among them are made of, with the questions asked of a type, a
/// method's name and a ledger: `tools/keep.nika` (ADR-250, #125).
use nikaia_std::tools::keep::KeepContext as Context;
pub use nikaia_std::tools::keep::takes_a_keep;
use nikaia_std::tools::keep::{
    a_ledger_type_with_a_view, capitalised, drops_entries, hands_back_its_own,
    keeps_what_it_is_given, names_a_struct_of_views, struct_in, taken_from,
};

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

    fn span(&self) -> &Span {
        match self {
            Source::Buffer { span, .. } | Source::Call { span, .. } => span,
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
    /// ([ADR-221](../../../../docs/specification/adr/adr-221.md) D1).
    pub element_keepers: BTreeSet<String>,
    /// The element keepers whose elements are **structs** of views, read
    /// through the handle rather than through `Deref` (ADR-221 D4).
    pub struct_keepers: BTreeSet<String>,
    /// How many keeps each element of a keeper carries: the most buffers any
    /// one value put into it points into (ADR-221 D2).
    pub widths: BTreeMap<String, usize>,
    /// The struct each such keeper holds, by the keeper's name: what a write
    /// through its handle asks about a field (ADR-221 D4).
    pub keeper_structs: BTreeMap<String, String>,
    /// Locals bound to an element **taken out** of such a keeper -
    /// `let old = kept.remove(0)` - which is still held and read through its
    /// handle (ADR-221 D3).
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
pub fn plans(parsed: &Parsed, ledger: &Ledger, library: &Ledger) -> Vec<Plan> {
    let context = Context::of(&parsed.interner, &parsed.program.items);
    let mut out = Vec::new();
    for item in &parsed.program.items {
        match &item.node {
            Item::Fn { .. } => {
                if let Some(plan) = plan(parsed, item, None, ledger, library, &context) {
                    out.push(plan);
                }
            }
            Item::Impl {
                target, methods, ..
            } => {
                let target = parsed.text(target.name).to_string();
                for method in methods {
                    if let Some(plan) =
                        plan(parsed, method, Some(&target), ledger, library, &context)
                    {
                        out.push(plan);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// One local, as far as this walk needs it.
#[derive(Debug, Clone)]
struct Local {
    /// The statement that binds it, which orders it against a buffer's.
    at: usize,
    /// The blocks it is declared inside, outermost first, each named by the
    /// statement that opens it; a loop's is marked.
    path: Vec<(usize, bool)>,
    origins: BTreeSet<Id>,
    /// Its written type, where the `let` wrote one.
    ty: Option<Type>,
    /// A parameter, or the subject: a destination with a name of its own.
    parameter: bool,
    /// A `mut` parameter or a `ref mut self`: a place views may be kept in.
    keeps: bool,
    /// **The buffer itself**, not a view of it: `let text = fs::read(…)`, or
    /// a second name for the same value. Handed on whole it is a move, which
    /// takes the buffer along and leaves nothing to keep alive.
    buffer: bool,
    /// **Text, where the `let` wrote no type**: `let last = current.trim()`.
    /// Its `.clone()` is a copy holding nothing of `current`, as the clone of
    /// a `String` is - read without this, it carried `current` into whatever
    /// the copy went into, and `current` into the frame's keep (#293).
    text: bool,
}

struct Walk<'a> {
    parsed: &'a Parsed,
    ledger: &'a Ledger,
    library: &'a Ledger,
    target: Option<&'a str>,
    /// Scopes of locals, innermost last.
    scopes: Vec<BTreeMap<String, Local>>,
    /// The blocks the walk is inside, outermost first.
    path: Vec<(usize, bool)>,
    sources: BTreeMap<Id, (Source, Vec<(usize, bool)>)>,
    escapes: BTreeSet<(Id, Escape)>,
    /// Locals something removes entries from, or assigns again, with the
    /// blocks each removal stands in: a removal only matters to a buffer read
    /// in a loop the removal is inside of (D4).
    sheds: BTreeMap<String, Vec<Vec<(usize, bool)>>>,
    /// Statements that put a value with origins into a local:
    /// `(statement, local) -> argument -> origins`.
    puts_into: BTreeMap<(usize, String), BTreeMap<usize, BTreeSet<Id>>>,
    /// Locals something puts a **struct** of views into, by its name - read
    /// off the value where the local's type is not written.
    struct_puts: BTreeMap<String, String>,
    /// Locals bound to what a removal hands back, by the local it was taken
    /// from.
    taken: BTreeMap<String, String>,
    /// A buffer local assigned again whole, by the assignment's statement:
    /// the statement of the `let` that bound it, which is its own buffer's.
    /// Only that one: what flowed into it since may have a keep of its own,
    /// and the binding is a place in a keep only where its `let` was put.
    reassigned: BTreeMap<usize, usize>,
    /// The structs that hold a view, from the unit.
    borrowing: &'a BTreeSet<String>,
    /// Task captures: `(binding, its let statement)`.
    captured: BTreeMap<String, (usize, BTreeSet<Id>)>,
    /// The statement being walked.
    statement: usize,
    /// Whether the declared result can hold a view at all.
    result_carries: bool,
    /// Every local as it was last bound, for questions asked after its scope
    /// has closed.
    last_seen: BTreeMap<String, Local>,
}

/// One function's plan, or `None` for something that is not a function.
fn plan(
    parsed: &Parsed,
    item: &crate::ast::Spanned<Item>,
    target: Option<&str>,
    ledger: &Ledger,
    library: &Ledger,
    context: &Context,
) -> Option<Plan> {
    let Item::Fn {
        name,
        receiver,
        args,
        ret_type,
        body,
        ..
    } = &item.node
    else {
        return None;
    };
    let own = match name {
        Some(name) => parsed.text(*name).to_string(),
        None => "new".to_string(),
    };
    let key = match target {
        Some(target) => format!("{target}::{own}"),
        None => own.clone(),
    };

    let mut frame = BTreeMap::new();
    for arg in args {
        frame.insert(
            parsed.text(arg.name).to_string(),
            Local {
                at: 0,
                path: Vec::new(),
                origins: BTreeSet::new(),
                ty: Some(arg.ty.clone()),
                parameter: true,
                keeps: arg.mutable && context.carries(&parsed.interner, &arg.ty),
                buffer: false,
                text: false,
            },
        );
    }
    if let Some(receiver) = receiver {
        frame.insert(
            "self".to_string(),
            Local {
                at: 0,
                path: Vec::new(),
                origins: BTreeSet::new(),
                ty: None,
                parameter: true,
                keeps: receiver.is_mut && target.is_some_and(|t| context.borrowing.contains(t)),
                buffer: false,
                text: false,
            },
        );
    }

    let mut walk = Walk {
        parsed,
        ledger,
        library,
        target,
        scopes: vec![frame],
        path: Vec::new(),
        sources: BTreeMap::new(),
        escapes: BTreeSet::new(),
        sheds: BTreeMap::new(),
        puts_into: BTreeMap::new(),
        struct_puts: BTreeMap::new(),
        taken: BTreeMap::new(),
        reassigned: BTreeMap::new(),
        borrowing: &context.borrowing,
        captured: BTreeMap::new(),
        statement: 0,
        last_seen: BTreeMap::new(),
        result_carries: ret_type
            .as_ref()
            .is_some_and(|t| context.carries(&parsed.interner, t)),
    };
    let result_carries = walk.result_carries;
    // Twice, so that an origin a loop's later statement gives a local reaches
    // the loop's earlier ones: origins only grow, and the second pass sees
    // everything the first one found.
    for _ in 0..2 {
        walk.scopes.truncate(1);
        walk.path.clear();
        // The body's own scope stays open for its last expression, which is
        // what the function hands back and reads the body's locals.
        walk.scopes.push(BTreeMap::new());
        for stmt in &body.stmts {
            walk.statement = stmt.span.at();
            walk.stmt(&stmt.node, &stmt.span);
        }
        if result_carries
            && let Some(last) = body.stmts.last()
            && let Stmt::Expr(value) = &last.node
        {
            walk.statement = last.span.at();
            let origins = walk.origins(value);
            if !walk.is_the_buffer(value) {
                walk.escape_all(&origins, Escape::Result);
            }
        }
        walk.scopes.pop();
    }
    let _ = result_carries;

    Some(decide(parsed, item, key, walk, context))
}

impl Walk<'_> {
    fn block(&mut self, block: &Block) {
        self.scopes.push(BTreeMap::new());
        for stmt in &block.stmts {
            self.statement = stmt.span.at();
            self.stmt(&stmt.node, &stmt.span);
        }
        self.scopes.pop();
    }

    fn nested(&mut self, at: usize, looping: bool, block: &Block) {
        self.path.push((at, looping));
        self.block(block);
        self.path.pop();
    }

    /// **Whether a `clone` here is a copy of text**
    /// ([ADR-216](../../../docs/specification/adr/adr-216.md) D2), which is
    /// text of its own and points into nothing - where a copy of a list of
    /// views still points where the views did. Read off what this walk can see
    /// without types: a literal, or a name whose type was written as text.
    /// Anything else is the list's answer, which is the one that keeps more.
    fn copies_text(&self, receiver: &Expr) -> bool {
        match receiver {
            Expr::LitStr { .. } | Expr::LitInterpolated { .. } => true,
            Expr::Variable(name) => self.local(self.parsed.text(*name)).is_some_and(|local| {
                local.text
                    || local.ty.as_ref().is_some_and(|ty| {
                        matches!(self.parsed.text(ty.name), "String" | "str")
                            && ty.generics.is_empty()
                    })
            }),
            // **A trimmed text is text** (0.0.252): `expr.trim().clone()` is a
            // copy of a piece of `expr`, and holds nothing of it. So is a copy
            // of text, which is what `let path = whole.clone()` binds (#293).
            Expr::MethodCall {
                receiver, method, ..
            } if matches!(
                self.parsed.text(*method),
                "trim" | "trim_start" | "trim_end" | "clone"
            ) =>
            {
                self.copies_text(receiver)
            }
            _ => false,
        }
    }

    /// Whether `to_string` of this makes text rather than handing back the
    /// text it is: a name whose written type is something other than text.
    fn formats(&self, receiver: &Expr) -> bool {
        match receiver {
            Expr::Variable(name) => self
                .local(self.parsed.text(*name))
                .and_then(|local| local.ty.as_ref())
                .is_some_and(|ty| !matches!(self.parsed.text(ty.name), "String" | "str")),
            _ => false,
        }
    }

    fn local(&self, name: &str) -> Option<&Local> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    fn local_mut(&mut self, name: &str) -> Option<&mut Local> {
        self.scopes
            .iter_mut()
            .rev()
            .find_map(|scope| scope.get_mut(name))
    }

    fn bind(
        &mut self,
        name: String,
        at: usize,
        origins: BTreeSet<Id>,
        ty: Option<Type>,
        buffer: bool,
        text: bool,
    ) {
        let local = Local {
            at,
            path: self.path.clone(),
            origins,
            ty,
            parameter: false,
            keeps: false,
            buffer,
            text,
        };
        self.last_seen.insert(name.clone(), local.clone());
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, local);
        }
    }

    fn stmt(&mut self, stmt: &Stmt, span: &Span) {
        let at = span.at();
        match stmt {
            Stmt::Let {
                names, value, ty, ..
            } => {
                self.spawns_in(value);
                self.nested_blocks(value, at);
                let made = match names.as_slice() {
                    [_] => match value {
                        // A copy of text is a buffer of its own (ADR-216 D2).
                        Expr::MethodCall {
                            receiver, method, ..
                        } if self.parsed.text(*method) == "clone" && self.copies_text(receiver) => {
                            Buffer::Named("String".to_string())
                        }
                        // And so is the text form of something that is not
                        // text - `seed.to_string()` for `seed: i64` - where
                        // text's own is the text itself (ADR-216 D4).
                        Expr::MethodCall {
                            receiver, method, ..
                        } if self.parsed.text(*method) == "to_string" && self.formats(receiver) => {
                            Buffer::Named("String".to_string())
                        }
                        _ => match super::tether::makes_a_buffer(
                            self.parsed,
                            value,
                            self.ledger,
                            self.library,
                        ) {
                            // A name declared `String` is text of this
                            // frame's own, built or moved here.
                            Buffer::Named(name) => Buffer::Named(name),
                            _ if !self.is_the_buffer(value)
                                && super::tether::declares_text(self.parsed, ty.as_ref()) =>
                            {
                                Buffer::Named("String".to_string())
                            }
                            other => other,
                        },
                    },
                    _ => Buffer::None,
                };
                // **An element taken out of a keeper** stays what it was in
                // there (ADR-221 D3).
                if let [name] = names.as_slice()
                    && let Some(from) = taken_from(&self.parsed.interner, value)
                {
                    self.taken.insert(self.parsed.text(*name).to_string(), from);
                }
                // Text with no type written: what a `.clone()` of it copies.
                let text = names.len() == 1 && ty.is_none() && self.copies_text(value);
                // A second name for the buffer is the buffer.
                let renames_a_buffer = self.is_the_buffer(value);
                let is_buffer = matches!(made, Buffer::Named(_)) || renames_a_buffer;
                let origins = match made {
                    Buffer::Named(buffer) => {
                        let name = self.parsed.text(names[0]).to_string();
                        self.sources.entry((at, String::new())).or_insert((
                            Source::Buffer {
                                at,
                                name,
                                ty: buffer,
                                span: *span,
                            },
                            self.path.clone(),
                        ));
                        BTreeSet::from([(at, String::new())])
                    }
                    _ => self.origins(value),
                };
                for name in names {
                    let name = self.parsed.text(*name).to_string();
                    self.bind(name, at, origins.clone(), ty.clone(), is_buffer, text);
                }
            }
            Stmt::Comptime { .. } => {}
            Stmt::Assign { target, op, value } => {
                self.spawns_in(value);
                self.nested_blocks(value, at);
                let origins = self.origins(value);
                if let Some(root) = root_of(self.parsed, target) {
                    let whole = op.is_none() && matches!(target, Expr::Variable(_));
                    if whole
                        && let Some(local) = self.local(&root)
                        && local.buffer
                    {
                        let bound = local.at;
                        self.reassigned.insert(at, bound);
                    }
                    if whole {
                        self.sheds
                            .entry(root.clone())
                            .or_default()
                            .push(self.path.clone());
                    }
                    // **A buffer handed over whole is a move**, the rule a
                    // `return` and a `let` already have: `inner = args` takes
                    // the buffer along, so `inner` is the buffer now and no
                    // view of it is left behind for a keep to hold. Flowed as
                    // views, an `inner` declared around the buffer outlived
                    // it, `args` went into the frame's keep as a view, and the
                    // assignment put a view where a `String` goes - `rustc`
                    // about a file nobody wrote (found moving `Ty::parse` into
                    // Nikaia, ADR-257).
                    if whole && self.is_the_buffer(value) {
                        if let Some(local) = self.local_mut(&root) {
                            local.buffer = true;
                            local.origins.extend(origins);
                        }
                    } else {
                        self.flow_into(&root, origins);
                    }
                }
            }
            Stmt::For {
                bindings,
                iter,
                body,
            } => {
                let origins = self.origins(iter);
                self.path.push((at, true));
                self.scopes.push(BTreeMap::new());
                for name in bindings {
                    let name = self.parsed.text(*name).to_string();
                    self.bind(name, at, origins.clone(), None, false, false);
                }
                for inner in &body.stmts {
                    self.statement = inner.span.at();
                    self.stmt(&inner.node, &inner.span);
                }
                self.scopes.pop();
                self.path.pop();
            }
            Stmt::While { cond, body } => {
                let _ = self.origins(cond);
                self.nested(at, true, body);
            }
            Stmt::Return(Some(value)) => {
                self.spawns_in(value);
                self.nested_blocks(value, at);
                let origins = self.origins(value);
                // Only a result that can hold a view hands one back, and a
                // buffer handed back whole is a move.
                if self.result_carries && !self.is_the_buffer(value) {
                    self.escape_all(&origins, Escape::Result);
                }
            }
            Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
            Stmt::Expr(expr) => {
                self.spawns_in(expr);
                self.nested_blocks(expr, at);
                self.effects(expr);
            }
        }
    }

    /// The blocks inside an expression that are not a value's arms: a lambda
    /// run during a call, an `if` or `match` in statement position. Walked for
    /// what their statements do.
    fn nested_blocks(&mut self, expr: &Expr, at: usize) {
        let mut blocks: Vec<&Block> = Vec::new();
        super::sync::visit_expr_blocks(expr, &mut |b| blocks.push(b));
        for block in blocks {
            let statement = self.statement;
            self.nested(at, false, block);
            self.statement = statement;
        }
    }

    /// What an expression in statement position does to the places views may
    /// be kept in: `xs.push(v)`, `m.insert(k, v)`, a call that keeps into one
    /// of its arguments.
    fn effects(&mut self, expr: &Expr) {
        match expr {
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                let method_name = self.parsed.text(*method).to_string();
                if let Some(root) = root_of(self.parsed, receiver) {
                    if drops_entries(&method_name) {
                        self.sheds
                            .entry(root.clone())
                            .or_default()
                            .push(self.path.clone());
                    }
                    if keeps_what_it_is_given(&method_name) {
                        let mut origins = BTreeSet::new();
                        for (at, arg) in args.iter().enumerate() {
                            let of = match self.is_the_buffer(arg) {
                                // The buffer itself goes in: a move.
                                true => BTreeSet::new(),
                                false => self.origins(arg),
                            };
                            self.puts_into
                                .entry((self.statement, root.clone()))
                                .or_default()
                                .entry(at)
                                .or_default()
                                .extend(of.iter().cloned());
                            if let Some(name) = self.struct_of(arg) {
                                self.struct_puts.insert(root.clone(), name);
                            }
                            origins.extend(of);
                        }
                        self.flow_into(&root, origins);
                    }
                }
                let _ = self.origins(expr);
            }
            Expr::TryCatch { expr, .. } | Expr::Try(expr) => self.effects(expr),
            _ => {
                let _ = self.origins(expr);
            }
        }
    }

    /// The struct of views an expression is a value of, where this walk can
    /// see it without types: a literal of one, or a name whose type was
    /// written as one.
    fn struct_of(&self, expr: &Expr) -> Option<String> {
        let name = match expr {
            Expr::StructLit { name, .. } => self.parsed.text(*name).to_string(),
            Expr::Variable(name) => self
                .local(self.parsed.text(*name))
                .and_then(|l| l.ty.as_ref())
                .map(|ty| self.parsed.text(ty.name).to_string())?,
            _ => return None,
        };
        self.borrowing.contains(&name).then_some(name)
    }

    /// Whether `Type::Variant` names a variant of an `enum` this file
    /// declares: a value built, not a function called.
    fn a_variant(&self, callee: &str) -> bool {
        let Some((owner, variant)) = callee.rsplit_once("::") else {
            return false;
        };
        self.parsed
            .program
            .items
            .iter()
            .any(|item| match &item.node {
                Item::Enum { name, variants, .. } => {
                    self.parsed.text(*name) == owner
                        && variants.iter().any(|v| self.parsed.text(v.name) == variant)
                }
                _ => false,
            })
    }

    /// Whether an expression **is** a buffer rather than a view of one: a
    /// name bound to one, or its copy.
    fn is_the_buffer(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Variable(name) => self
                .local(self.parsed.text(*name))
                .is_some_and(|l| l.buffer),
            Expr::MethodCall {
                receiver, method, ..
            } if matches!(
                self.parsed.text(*method),
                "clone" | "to_owned" | "to_string"
            ) =>
            {
                self.is_the_buffer(receiver)
            }
            _ => false,
        }
    }

    /// Views with these origins now live in `root` as well.
    fn flow_into(&mut self, root: &str, origins: BTreeSet<Id>) {
        if origins.is_empty() {
            return;
        }
        let Some(local) = self.local(root).cloned() else {
            return;
        };
        if local.parameter {
            if local.keeps {
                self.escape_all(&origins, Escape::Param(root.to_string()));
            }
            return;
        }
        // **A local declared before the buffer, in a scope around it, outlives
        // it**: the loop's list, or a list declared first in the same block -
        // which the language below drops *after* a buffer declared later, and
        // refuses for it.
        for origin in &origins {
            let Some((source, path)) = self.sources.get(origin) else {
                continue;
            };
            let _ = source;
            let around =
                local.path.len() <= path.len() && path[..local.path.len()] == local.path[..];
            if around && local.at < origin.0 {
                self.escapes
                    .insert((origin.clone(), Escape::Outer(root.to_string())));
            }
        }
        if let Some(local) = self.local_mut(root) {
            local.origins.extend(origins);
        }
    }

    fn escape_all(&mut self, origins: &BTreeSet<Id>, escape: Escape) {
        for origin in origins {
            if self.sources.contains_key(origin) {
                self.escapes.insert((origin.clone(), escape.clone()));
            }
        }
    }

    /// A `spawn` anywhere in this expression: every local its body names that
    /// carries origins leaves with the task (D3).
    fn spawns_in(&mut self, expr: &Expr) {
        let parsed = self.parsed;
        // Cloned, because a hole's expressions are parsed out of the literal
        // for the walk and do not outlive it.
        let mut bodies: Vec<Expr> = Vec::new();
        super::sync::visit_expr(parsed, expr, &mut |e| {
            if let Expr::Spawn { body, .. } = e {
                bodies.push((**body).clone());
            }
        });
        for body in &bodies {
            let mut named = BTreeSet::new();
            names_in(parsed, body, &mut named);
            for name in named {
                let Some(local) = self.local(&name).cloned() else {
                    continue;
                };
                // A buffer a task takes is moved into it, whole.
                if local.origins.is_empty() || local.buffer {
                    continue;
                }
                self.escape_all(&local.origins, Escape::Task);
                self.captured
                    .entry(name)
                    .or_insert((local.at, BTreeSet::new()))
                    .1
                    .extend(local.origins.iter().cloned());
            }
        }
    }

    /// The origins of an expression's value: where its views may point.
    fn origins(&mut self, expr: &Expr) -> BTreeSet<Id> {
        match expr {
            Expr::Variable(name) => self
                .local(self.parsed.text(*name))
                .map(|l| l.origins.clone())
                .unwrap_or_default(),
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            }
            | Expr::SafeMethod {
                receiver,
                method,
                args,
                ..
            } => {
                let name = self.parsed.text(*method).to_string();
                // **A removal is a removal wherever it stands** - `let old =
                // kept.remove(0)` drops the entry as surely as a statement does.
                if drops_entries(&name)
                    && let Some(root) = root_of(self.parsed, receiver)
                {
                    self.sheds.entry(root).or_default().push(self.path.clone());
                }
                let mut out = BTreeSet::new();
                if let Some(callee) = self.keeping_method(receiver, &name) {
                    out.extend(self.call_source(&callee));
                }
                if hands_back_its_own(&name) || (name == "clone" && self.copies_text(receiver)) {
                    for arg in args {
                        let _ = self.origins(arg);
                    }
                    let _ = self.origins(receiver);
                    return out;
                }
                out.extend(self.origins(receiver));
                for arg in args {
                    out.extend(self.origins(arg));
                }
                out
            }
            Expr::Call { func, args, .. } => {
                let callee = callee_name(self.parsed, func);
                let mut out = BTreeSet::new();
                let mut arg_origins = Vec::new();
                // **And to a variant whole**, for the same reason:
                // `Seg::Text(text)` is the text moved into the value.
                let variant = callee.as_deref().is_some_and(|c| self.a_variant(c));
                for arg in args {
                    let origins = self.origins(arg);
                    if !(variant && self.is_the_buffer(arg)) {
                        arg_origins.push(origins);
                    }
                }
                let contract = callee
                    .as_deref()
                    .and_then(|c| self.ledger.lookup(c).or_else(|| self.library.lookup(c)));
                if let Some((key, contract)) = &contract {
                    if takes_a_keep(contract) {
                        let source = self.call_source(key);
                        // A call that keeps into one of its arguments: the
                        // argument's root now keeps this call's buffers.
                        let params: Vec<String> = contract
                            .signature
                            .as_ref()
                            .map(|s| s.params.iter().map(|(n, _)| n.clone()).collect())
                            .unwrap_or_default();
                        for held in &contract.views {
                            if held.state != State::Tethered || held.position == RESULT {
                                continue;
                            }
                            if let Some(at) = params.iter().position(|p| *p == held.position)
                                && let Some(root) =
                                    args.get(at).and_then(|a| root_of(self.parsed, a))
                            {
                                self.flow_into(&root, source.clone());
                            }
                        }
                        let hands_back = contract
                            .views
                            .iter()
                            .any(|h| h.position == RESULT && h.state == State::Tethered);
                        if hands_back {
                            out.extend(source);
                        }
                    }
                    // **A result that holds no view is the call's own**, and
                    // that is where a chain of views stops: `fs::read_to_string
                    // (path)` hands back a `String`, not a piece of `path`.
                    let owned = contract
                        .signature
                        .as_ref()
                        .and_then(|s| s.result.as_ref())
                        .is_some_and(|ty| !a_ledger_type_with_a_view(ty));
                    if owned {
                        return out;
                    }
                }
                for origins in arg_origins {
                    out.extend(origins);
                }
                out
            }
            Expr::Field { base, .. } | Expr::SafeField { base, .. } => self.origins(base),
            Expr::Index { base, index } => {
                let _ = self.origins(index);
                self.origins(base)
            }
            // **A buffer handed to a field whole is moved there** (0.0.252),
            // and takes nothing of this frame's along: `Seg { text }` holds
            // the text, not a view of it. Counted as one, the `let` went into a
            // keep and became a reference the field's `String` refused - found
            // moving the template splitter into Nikaia.
            Expr::StructLit { fields, .. } => {
                let mut out = BTreeSet::new();
                for field in fields {
                    match &field.value {
                        Some(value) if self.is_the_buffer(value) => {
                            let _ = self.origins(value);
                        }
                        Some(value) => out.extend(self.origins(value)),
                        None => {
                            if let Some(l) = self.local(self.parsed.text(field.name))
                                && !l.buffer
                            {
                                out.extend(l.origins.iter().cloned());
                            }
                        }
                    }
                }
                out
            }
            Expr::With { base, fields, .. } => {
                let mut out = self.origins(base);
                for field in fields {
                    if let Some(value) = &field.value {
                        out.extend(self.origins(value));
                    }
                }
                out
            }
            Expr::ListLit { items, .. } | Expr::Tuple(items) => {
                let mut out = BTreeSet::new();
                for item in items {
                    out.extend(self.origins(item));
                }
                out
            }
            Expr::If {
                then_branch,
                else_branch,
                cond,
            } => {
                let _ = self.origins(cond);
                let mut out = self.tail_origins(then_branch);
                if let Some(block) = else_branch {
                    out.extend(self.tail_origins(block));
                }
                out
            }
            Expr::Match { value, arms } => {
                let _ = self.origins(value);
                let mut out = BTreeSet::new();
                for arm in arms {
                    out.extend(self.origins(&arm.body));
                }
                out
            }
            Expr::Block(block) | Expr::Unsafe(block) => self.tail_origins(block),
            Expr::Unary { expr, .. } | Expr::Try(expr) | Expr::Cast { expr, .. } => {
                self.origins(expr)
            }
            Expr::TryCatch { expr, handler } => {
                let mut out = self.origins(expr);
                out.extend(self.tail_origins(handler));
                out
            }
            Expr::Coalesce { value, fallback } => {
                let mut out = self.origins(value);
                out.extend(self.origins(fallback));
                out
            }
            // **A hole is code** (ADR-032): a call in `f"{load(p).len()}"` is
            // a call of this statement, and the text built around it is text
            // of its own.
            Expr::LitInterpolated { .. } => {
                for hole in crate::emit::literal_expressions(self.parsed, expr) {
                    let _ = self.origins(&hole);
                }
                BTreeSet::new()
            }
            // A comparison or arithmetic is a value of its own; text joined
            // with `+` is text of its own.
            Expr::Binary { lhs, rhs, .. } => {
                let _ = self.origins(lhs);
                let _ = self.origins(rhs);
                BTreeSet::new()
            }
            _ => BTreeSet::new(),
        }
    }

    /// The origins of what a block ends in, without walking its statements a
    /// second time for their effects.
    ///
    /// **As the statement it is**, because a call there is written by the
    /// emitter as part of that statement and not of the `if` around it: the
    /// key a call's keep is found by has to be the one the emitter has in hand.
    fn tail_origins(&mut self, block: &Block) -> BTreeSet<Id> {
        match block.stmts.last() {
            Some(last) => match &last.node {
                Stmt::Expr(value) => {
                    let outer = std::mem::replace(&mut self.statement, last.span.at());
                    let origins = self.origins(value);
                    self.statement = outer;
                    origins
                }
                _ => BTreeSet::new(),
            },
            None => BTreeSet::new(),
        }
    }

    /// The source a call to a keeping function stands for, made on first sight.
    ///
    /// Keyed by statement and callee, which is what the emitter has in hand
    /// where it writes the call: one keep per callee per statement.
    fn call_source(&mut self, callee: &str) -> BTreeSet<Id> {
        let id: Id = (self.statement, callee.to_string());
        self.sources.entry(id.clone()).or_insert((
            Source::Call {
                at: self.statement,
                callee: callee.to_string(),
                span: Span::new(self.statement, self.statement),
            },
            self.path.clone(),
        ));
        BTreeSet::from([id])
    }

    /// A method call to a function of this package that takes a keep.
    fn keeping_method(&self, receiver: &Expr, method: &str) -> Option<String> {
        let on_self = matches!(receiver, Expr::Variable(name) if self.parsed.text(*name) == "self");
        keeping_method_key(self.ledger, self.target, on_self, method)
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

/// Every name an expression mentions, lambdas and blocks included - and a
/// task's body, which this reader asks about where the shared walk does not
/// descend. By the walk in Nikaia (`tools/foreign.nika`, #125).
fn names_in(parsed: &Parsed, expr: &Expr, out: &mut BTreeSet<String>) {
    out.extend(crate::foreign::names_in_expression(parsed, expr).0);
    if let Expr::Spawn { body, .. } = expr {
        names_in(parsed, body, out);
    }
}

/// From what the walk found to what the lowering writes (D2-D5), and the
/// refusals where it cannot.
fn decide(
    parsed: &Parsed,
    item: &crate::ast::Spanned<Item>,
    key: String,
    walk: Walk<'_>,
    context: &Context,
) -> Plan {
    let Item::Fn { body, .. } = &item.node else {
        return Plan::default();
    };
    let mut plan = Plan {
        key: key.clone(),
        first: body.stmts.first().map(|s| s.span.at()),
        ..Plan::default()
    };
    let mut by_source: BTreeMap<Id, BTreeSet<Escape>> = BTreeMap::new();
    for (source, escape) in &walk.escapes {
        by_source
            .entry(source.clone())
            .or_default()
            .insert(escape.clone());
    }

    for (id, (source, path)) in &walk.sources {
        let escapes = by_source.get(id).cloned().unwrap_or_default();
        for escape in &escapes {
            plan.escapes.push((source.clone(), escape.clone()));
        }
        let leaves = escapes
            .iter()
            .any(|e| matches!(e, Escape::Result | Escape::Param(_)));
        let task = escapes.contains(&Escape::Task);
        let outer: Vec<&String> = escapes
            .iter()
            .filter_map(|e| match e {
                Escape::Outer(r) => Some(r),
                _ => None,
            })
            .collect();

        // **A keeper that drops entries across a loop** (D4): the buffer is
        // read inside a loop the keeper is declared outside of, and the entries
        // are dropped **inside that same loop** - while it goes on reading.
        // Dropped only after the loop, or before it, nothing is freed early by
        // a handle per view: the frame's keep holds the buffers exactly as
        // long, and costs nothing per view.
        let shedding: Vec<&String> = outer
            .iter()
            .copied()
            .filter(|r| {
                let declared = walk_local_path(&walk, r);
                let Some(sheds) = walk.sheds.get(*r) else {
                    return false;
                };
                path.iter()
                    .filter(|(at, looping)| *looping && !declared.contains(&(*at, true)))
                    .any(|around| sheds.iter().any(|shed| shed.contains(around)))
            })
            .collect();

        let keep = if task && leaves {
            plan.refusals.push(refusal(
                source,
                "is passed to a task and also returned from this function.",
                "The task may outlive the caller, or the caller the task, so neither can be \
                 the one that keeps the data alive.",
                "Give the task a copy with `.clone()`, or pass the task only what it needs \
                 and return the rest.",
            ));
            continue;
        } else if task {
            KeepAt::Task
        } else if leaves {
            KeepAt::Param
        } else if !shedding.is_empty() {
            KeepAt::Element(source.at())
        } else if !outer.is_empty() {
            KeepAt::Frame
        } else {
            match source {
                // A buffer nothing keeps past its scope stays what it was.
                Source::Buffer { .. } => continue,
                // A call's buffers need a keep all the same; one declared
                // just before the statement lives exactly as long as the
                // value the call hands back.
                Source::Call { at, .. } => KeepAt::Local(*at),
            }
        };

        match keep {
            KeepAt::Param => plan.takes_keep = true,
            KeepAt::Frame => plan.frame_keep = true,
            KeepAt::Task => plan.task_keep = true,
            KeepAt::Local(at) => {
                plan.local_keeps.insert(at);
            }
            KeepAt::Element(_) => {
                for keeper in &shedding {
                    plan.element_keepers.insert((*keeper).clone());
                }
                for ((statement, into), by_arg) in &walk.puts_into {
                    if !shedding.contains(&into) {
                        continue;
                    }
                    for (arg, origins) in by_arg {
                        if origins.contains(id) {
                            plan.holds
                                .entry((*statement, into.clone()))
                                .or_default()
                                .entry(*arg)
                                .or_default()
                                .insert(source.at());
                        }
                    }
                }
            }
        }
        match source {
            Source::Buffer { at, .. } => {
                plan.puts.insert(*at, keep);
            }
            Source::Call { at, callee, .. } => {
                plan.calls.insert((*at, callee.clone()), keep);
            }
        }
        if keep == KeepAt::Task {
            for (name, (at, origins)) in &walk.captured {
                if origins.contains(id) {
                    plan.tethered.insert(name.clone(), *at);
                }
            }
        }
    }

    // **A kept buffer assigned again goes into the same keep** (#293).
    for (statement, bound) in &walk.reassigned {
        if let Some(keep) = plan.puts.get(bound).copied() {
            plan.reputs.insert(*statement, keep);
        }
    }

    // **One keep per buffer, as many as the widest value needs** (ADR-221
    // D2): a value whose views point into two buffers carries a handle on
    // each, and every element of one keeper carries the same number, since
    // they are one type.
    for ((_, keeper), by_arg) in &plan.holds {
        let widest = by_arg.values().map(BTreeSet::len).max().unwrap_or(1);
        let width = plan.widths.entry(keeper.clone()).or_insert(1);
        *width = (*width).max(widest);
    }
    // **Which keepers hold structs** (ADR-221 D4): read through the handle.
    for keeper in &plan.element_keepers {
        let written = walk_local_type(&walk, keeper)
            .is_some_and(|ty| names_a_struct_of_views(&parsed.interner, &ty, &context.borrowing));
        if written || walk.struct_puts.contains_key(keeper) {
            plan.struct_keepers.insert(keeper.clone());
            let named = walk.struct_puts.get(keeper).cloned().or_else(|| {
                walk_local_type(&walk, keeper)
                    .and_then(|ty| struct_in(&parsed.interner, &ty, &context.borrowing))
            });
            if let Some(name) = named {
                plan.keeper_structs.insert(keeper.clone(), name);
            }
        }
    }
    for (local, from) in &walk.taken {
        if plan.struct_keepers.contains(from) {
            plan.held_locals.insert(local.clone());
        }
    }

    // **No permission is asked for** ([ADR-209](../../../../docs/specification/adr/adr-209.md)
    // D5, withdrawing [ADR-201](../../../../docs/specification/adr/adr-201.md)
    // D2): which buffer lives where is the compiler's decision, like which
    // count a `Shared` gets, and it is shown by `--tethers` and in the ledger
    // rather than demanded of the source. A word the program had to write for
    // something the compiler already knows is the bookkeeping this language
    // exists to take away.
    for refusal in walk_refusals(&plan, &walk, parsed, context) {
        plan.refusals.push(refusal);
    }
    plan
}

/// The refusals that are about the lowering rather than the permission.
fn walk_refusals(
    plan: &Plan,
    walk: &Walk<'_>,
    parsed: &Parsed,
    context: &Context,
) -> Vec<crate::check::Finding> {
    let mut out = Vec::new();
    // D4 holds text one view at a time, and ADR-221 holds a **struct** of
    // views one handle per buffer. What is left is a value `tether::Rebase`
    // has no shape for, and a struct in a container whose reads are not a
    // sequence's - each said by name rather than handed to `rustc`.
    for keeper in &plan.element_keepers {
        let written = walk_local_type(walk, keeper)
            .and_then(|ty| context.not_held(&parsed.interner, &ty, &mut Vec::new()));
        let put = walk
            .struct_puts
            .get(keeper)
            .and_then(|name| context.struct_not_held(&parsed.interner, name, &mut Vec::new()));
        let sequence = walk_local_type(walk, keeper)
            .is_none_or(|ty| matches!(parsed.text(ty.name), "Vec" | "List" | "VecDeque" | "Deque"));
        // **One message for one container**: where it is not a list, that is
        // the thing to change, and what it holds is said by the one way out.
        let not_a_list = plan.struct_keepers.contains(keeper) && !sequence;
        for why in written.into_iter().chain(put).filter(|_| !not_a_list) {
            {
                out.push(element_refusal(
                    keeper,
                    &format!("it holds {why}, which isn't supported there yet"),
                    "Keep text, or structs of text views, in it, or store copies made with \
                     `.clone()`.",
                    plan,
                    walk,
                ));
            }
        }
        if not_a_list {
            out.push(element_refusal(
                keeper,
                "it isn't a list, and only a list can hold structs of views here yet",
                "Keep the structs in a list, or store copies made with `.clone()`.",
                plan,
                walk,
            ));
        }
    }
    out
}

fn element_refusal(
    keeper: &str,
    why: &str,
    help: &str,
    plan: &Plan,
    _walk: &Walk<'_>,
) -> crate::check::Finding {
    let span = plan
        .holds
        .keys()
        .find(|(_, k)| k == keeper)
        .map(|(at, _)| Span::new(*at, *at))
        .unwrap_or(Span::nowhere());
    crate::check::Finding {
        severity: crate::check::Severity::Error,
        span,
        code: "NK2304",
        message: format!(
            "`{keeper}` keeps views of data read inside a loop and removes entries as it \
             goes, but {why}."
        ),
        notes: vec![
            "Each view keeps its own data alive here, so that data read for entries that \
             were removed can be freed during the loop."
                .to_string(),
        ],
        help: Some(help.to_string()),
        labels: Vec::new(),
    }
}

fn refusal(source: &Source, what: &str, why: &str, help: &str) -> crate::check::Finding {
    crate::check::Finding {
        severity: crate::check::Severity::Error,
        span: *source.span(),
        code: "NK2304",
        message: format!("{} {what}", capitalised(&source.named())),
        notes: vec![why.to_string()],
        help: Some(help.to_string()),
        labels: Vec::new(),
    }
}

fn walk_local_path(walk: &Walk<'_>, name: &str) -> Vec<(usize, bool)> {
    walk.last_seen
        .get(name)
        .map(|l| l.path.clone())
        .unwrap_or_default()
}

fn walk_local_type(walk: &Walk<'_>, name: &str) -> Option<Type> {
    walk.last_seen.get(name).and_then(|l| l.ty.clone())
}
