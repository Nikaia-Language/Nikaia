// crates/nikaia/src/contracts/sharing.rs
//
// Which reference count a **particular** `Shared` value gets
// ([ADR-037](../../../../docs/specification/adr/adr-037.md) D7).
//
// ## One rule, and everything here is downstream of it
//
//     It may only ever take an atomic away.
//
// ADR-037 D6 made the atomic count the **floor**, and ADR-312 D10 lowered that
// floor at `user_parallelism = no`, where nothing a user writes can cross and D1
// closed the last way out of the program. So the floor is the atomic count at
// `yes` and the plain one at `no`, and this file is an **optimisation** on top of
// whichever floor the build has and nothing else. What does *not* move with the
// switch is `contracts::send`'s verdict about the type, which is why a `Shared`
// permitted into a task at `no` is still sound: there the task is interleaved on
// the same thread and the crossing the verdict allows does not happen. It may lower a
// particular value to a plain count where it *proves* that nothing crosses a
// thread with it; where it cannot prove that, the answer stays atomic. It may
// never add an atomic, and it may never make a program unsafe - which are the
// same sentence read from both ends.
//
// **That is why being wrong here costs speed in one direction and correctness in
// the other**, and why the polarity is not negotiable. An atomic count where a
// plain one would have done costs about 9 ns per clone-and-drop pair
// (ADR-037 D7). A plain count on a value that crosses is a data
// race. So every case this cannot decide comes out [`Count::Atomic`], every such
// case carries the reason, and the reasons are a **closed list** - see
// [`Fallback`], which is the enumeration ADR-037 D8 answers "would an override
// help?" for.
//
// **Fail-closed is affordable here in places `NK25xx` could not afford it.**
// `send.rs` may not refuse what it cannot decide, because refusing a correct
// program is the one thing this compiler may never do (Part III C.4) - so it has
// a third answer, `Undecided`. This has two, because the cost of being wrong in
// the safe direction is speed: the program still compiles and still means the
// same thing. ADR-292 D3's polarity, applied where it is cheap.
//
// ## What this file promises `send.rs`
//
// `send.rs` answers `May` for a `Shared` of crossable data, and that answer is
// only true because the count is atomic. So the two files have a contract with
// each other, and it runs one way:
//
//     a value this file lowers to a plain count must not cross a thread.
//
// Every crossing is therefore a **seed**, and a missed seed is unsoundness
// rather than a missed optimisation. The seeds are ADR-005 §5.2's own
// enumeration of the crossings, asked per value rather than per type - including
// the one D6 made live. `task::both`, the crossing the *compiler* chooses for
// statement overlapping, used to be safe to leave out because a `Shared` was
// `MayNot` and was never overlapped; since D6 the result of an overlapped
// operation is a `Shared` that crosses back to the thread that started it, so it
// is a seed now, reached through [`Fallback::UnseenOrigin`] - a handle whose
// allocation this analysis did not watch being made.
//
// ## Why the fixpoint degenerates, which is itself a finding
//
// A count belongs to the **allocation**, not to the handle: two handles on one
// `Shared` share one count, so they cannot disagree about whether it is atomic.
// The relation between handles is therefore an *equivalence* and not an ordering,
// and the analysis is union-find over handles plus one colouring pass - a least
// fixpoint reached in a single step, where `sync` (ADR-288 D1) genuinely needs a
// greatest one because its constraint is conjunctive over a call graph that can
// cycle. The least fixpoint of "is atomic" is the complement of the greatest
// fixpoint of "stays plain", because every clause is a Horn clause and the only
// sources of `Atomic` are the seeds. What carries the weight is not the lattice -
// it is **which crossings are seeds**.
//
// ## What counts as an account, and the one thing it rests on
//
// A handle may stay plain only if every place it goes is a place something
// written down accounts for. Three kinds of place do:
//
//   * **a body this build lowers** - a function of this unit that is not
//     public. The handle joins the callee's parameter slot, one allocation gets
//     one answer, and whichever side of the call the crossing is on decides both;
//   * **a field of a type this unit declares**, through the ledger's `fields`
//     (ADR-024, the walk ADR-288 established) - run in the opposite direction
//     from `send.rs`: there a field that may not cross makes the struct refuse,
//     here a struct that crosses makes the field's count atomic;
//   * **a call the ledger describes**. This is the one that rests on something,
//     and it rests on exactly what ADR-005 §5.2 already rests on: a contract is
//     an account, `std`'s entries are written in the file that ships and
//     reviewed like code (ADR-005 §5.1), and a call nothing describes is the
//     crossing. The residual is the one §5.3 names - "enforceable only as far as
//     a foreign crate is honest". No `std` entry takes or hands back a **handle**
//     on a `Shared`: the one entry that mentions the type at all is
//     `Shared::deref`, which takes `&Shared[$T]` and hands back `&$T` - a borrow
//     duplicates nothing (ADR-312 D1) and what comes out is a view of the value
//     inside, so there is no handle for it to keep.
//     `a_std_entry_that_takes_a_shared_needs_a_second_look` in `tests/sharing.rs`
//     is what makes somebody look on the day one does take one.
//
// Everywhere else is [`Fallback`], and the remedy for every row of it is to
// write the contract down - ADR-292 D3's own closing argument, which is why
// ADR-037 D8 has no keyword in it.

pub use nikaia_std::tools::ty::{Class, Count};
use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Block, Expr, Item, Stmt};
use crate::parser::Parsed;

use super::{Ledger, send, ty::Ty};
use crate::contracts::SignatureOps;
use crate::contracts::ty::TyOps;

/// The name of the slot standing for what a function hands back.
const RESULT: &str = "<result>";

/// Every reason this analysis answers `atomic` **because nothing decided it**
/// ([`Fallback`]), what it chose for one value ([`Decision`]) and for all of
/// them ([`Sharing`]) are records of `tools/sharing.nika` (ADR-294, #125).
pub use nikaia_std::tools::sharing::{
    Decision, Fallback, Sharing, every_fallback, lock_name, rust_name,
};
use nikaia_std::tools::sharing::{
    FIELDS, Slots, by_value_shared, decided, declared_field_type, every_count_plain, handed_on_to,
    held_that_does_not_copy, holds_shared, is_hull, literal_type, published_reason, shared_fields,
    slot, split_slot, unseen,
};
use nikaia_std::tools::views::Asked;

/// What stays Rust of a [`Class`] (ADR-294 step (b)): reading one back.
pub trait ClassOps: Sized {
    fn parse(text: &str) -> Option<Self>;
}

impl ClassOps for Class {
    /// Read one back from the ledger.
    fn parse(text: &str) -> Option<Self> {
        nikaia_std::tools::ledger::class_of(text)
    }
}

/// Every `Shared` value in a program, which count it would get, and the
/// per-function summary.
pub fn analyse_program(
    parsed: &Parsed,
    own: &Ledger,
    library: &Ledger,
    crossings_are_possible: bool,
) -> Sharing {
    // **Where nothing can cross, there is nothing to decide**
    // ([ADR-312](../../../../docs/specification/adr/adr-312.md) D10). At
    // `user_parallelism = no` one thread runs the user's code; the runtime's I/O
    // thread carries none of it, a task interleaves on the same thread, and since
    // D1 a `Shared` may not be handed to code this compiler cannot see - which
    // was the last way out. So every count is plain, and the seven reasons to
    // decline are reasons to decline *a proof about another thread*, which no
    // longer has to be found.
    //
    // The **verdict** is untouched and stays switch-independent
    // ([ADR-312](../../../../docs/specification/adr/adr-312.md) D6): a
    // `Shared[Locked[i32]]` may go into a task of ours at either setting. This is
    // about what is emitted, not about what is permitted.
    let mut analysis = Analysis::new(parsed, own, library);
    for item in &parsed.program.items {
        match &item.node {
            Item::Fn { .. } => analysis.function(&item.node, None),
            Item::Impl {
                target, methods, ..
            } => {
                let target = parsed.text(target.name).to_string();
                for method in methods {
                    analysis.function(&method.node, Some(&target));
                }
            }
            // A `pub` field of a `pub` type is a place code this build never
            // reads can take the value out of, which is the library boundary
            // one level in from a signature (ADR-037 D8).
            Item::Struct {
                name,
                fields,
                is_public: true,
                ..
            } => {
                let struct_name = parsed.text(*name).to_string();
                for field in fields.iter().filter(|f| f.is_public) {
                    let ty = Ty::from_ast(parsed, &field.ty);
                    if !holds_shared(&ty) {
                        continue;
                    }
                    let key = format!("{struct_name}.{}", parsed.text(field.name));
                    analysis.note(FIELDS, &key, ty, true, false);
                    analysis.force(
                        FIELDS,
                        &key,
                        format!(
                            "`{key}` is a public field of a public type, so code this build \
                             cannot see may take the value out of it and cross with it"
                        ),
                        Some(Fallback::PublicField),
                    );
                }
            }
            _ => {}
        }
    }
    let mut sharing = analysis.decide();
    // Every count plain, because nothing can cross (ADR-312 D10): applied to
    // the finished answer, so the duplication sites are kept as they were
    // found and one run of this analysis is one code path at both settings.
    if !crossings_are_possible {
        every_count_plain(&mut sharing);
    }
    sharing
}

/// Every `Shared` value in a program, and which count it would get.
pub fn analyse(parsed: &Parsed, own: &Ledger, library: &Ledger) -> Vec<Decision> {
    analyse_program(parsed, own, library, true).decisions
}

/// Fill in the `sharing` column of a ledger, from the bodies.
///
/// Runs after the other inferences for the same reason `throws::infer` does:
/// it needs every function to have a `signature` to resolve a callee's
/// parameters against, and every type to have its `fields`. It reads nothing any
/// of them wrote.
/// **At the floor, whatever the switch says**, and that is the one place D2 does
/// not reach ([ADR-312](../../../../docs/specification/adr/adr-312.md)). What
/// this writes is a **contract** — a statement about a function's positions that
/// a reader and a later build consult — and a contract that moved with a build
/// switch would be the thing ADR-312 D6 refuses. What is emitted is the
/// emitter's question and it asks it with the setting in hand.
pub fn infer(ledger: &mut Ledger, parsed: &Parsed, library: &Ledger) {
    let summaries = analyse_program(parsed, ledger, library, true).summaries;
    for (key, classes) in summaries {
        if let Some(contract) = ledger.functions.get_mut(&key) {
            contract.sharing = classes;
        }
    }
}

/// What `--sharing` prints: one line per `Shared` value, the reason beside every
/// atomic one, and the enumeration of what the floor caught.
///
/// The shape is `--overlaps`' and `--trust`'s: a heading per function, then one
/// indented line per thing decided, then what it adds up to. It **explains a
/// decision rather than changing one** (ADR-292 D2), which is what a person
/// reads when they want the 9 ns back.
pub fn report(
    parsed: &Parsed,
    own: &Ledger,
    library: &Ledger,
    crossings_are_possible: bool,
) -> String {
    nikaia_std::tools::sharing::sharing_report(&analyse_program(
        parsed,
        own,
        library,
        crossings_are_possible,
    ))
}

/// Everything a file declares that a slot can belong to.
///
/// A slot key is `owner::name`, and the owner is a struct (`<field>` slots), a
/// function, or a `Type::method`. Which of them this file declares is what says
/// whether this run of the analysis can see the whole of that slot.
fn declared_here(parsed: &Parsed) -> BTreeSet<String> {
    nikaia_std::tools::sharing::declared_here(&parsed.interner, &parsed.program.items)
}

/// Handles joined into allocation classes, with a reason recorded against the
/// ones that cross.
struct Analysis<'a> {
    parsed: &'a Parsed,
    own: &'a Ledger,
    library: &'a Ledger,
    /// Every `Shared` handle and the classes it joins, with what forced a
    /// class atomic and where a handle is duplicated or copied out:
    /// `tools/sharing.nika`'s record, which this walk fills.
    slots: Slots,
    /// The names bound to a walk that runs in parallel
    /// ([ADR-235](../../../../docs/specification/adr/adr-235.md) D1), so a walk
    /// chained onto the name is one too. Over-approximate - a name is never
    /// taken out - which is this file's safe direction.
    parallel_names: BTreeSet<String>,
}

impl<'a> Analysis<'a> {
    fn new(parsed: &'a Parsed, own: &'a Ledger, library: &'a Ledger) -> Self {
        Analysis {
            parsed,
            own,
            library,
            slots: Slots::new(declared_here(parsed)),
            parallel_names: BTreeSet::new(),
        }
    }

    // --- collecting, into the classes ------------------------------------

    fn join(&mut self, left: &str, right: &str) {
        self.slots.join(left, right);
    }

    fn note(&mut self, function: &str, value: &str, ty: Ty, internal: bool, position: bool) {
        self.slots.note(function, value, ty, internal, position);
    }

    /// This slot's class must be atomic, and this is what forced it. The slot
    /// is made where it does not exist yet, and no guard may be added
    /// (`docs/history/rc-or-arc.md` §8).
    fn force(&mut self, function: &str, value: &str, why: String, fallback: Option<Fallback>) {
        self.slots.force(function, value, why, fallback);
    }

    /// A second handle on this slot's allocation is made here
    /// ([ADR-312](../../../../docs/specification/adr/adr-312.md) D1), only
    /// where the handle is handed on by value.
    fn duplicates(&mut self, function: &str, value: &str, site: String) {
        self.slots.duplicates(function, value, site);
    }

    fn function(&mut self, item: &Item, target: Option<&str>) {
        let Item::Fn {
            name,
            args,
            body,
            ret_type,
            is_public,
            ..
        } = item
        else {
            return;
        };
        let own_name = match name {
            Some(name) => self.parsed.text(*name).to_string(),
            None => "new".to_string(),
        };
        let key = match target {
            Some(target) => format!("{target}::{own_name}"),
            None => own_name,
        };

        // Parameters, and the result, are the slots a *caller* meets.
        let mut scope: BTreeMap<String, Ty> = BTreeMap::new();
        for arg in args {
            let ty = Ty::from_ast(self.parsed, &arg.ty);
            let name = self.parsed.text(arg.name).to_string();
            if holds_shared(&ty) {
                self.note(&key, &name, ty.clone(), false, true);
                if *is_public {
                    self.force(
                        &key,
                        &name,
                        published_reason(&key, &name),
                        Some(Fallback::PublicSignature),
                    );
                }
            } else if *is_public {
                // A public function's parameter may *hold* a `Shared` without
                // being one, and the field's class is then as exposed as a
                // parameter is. `send::crossing`'s walk through the ledger's
                // `fields`, run the other way round.
                for field in shared_fields(&ty.text(), &self.asked()) {
                    self.force(
                        FIELDS,
                        &field,
                        published_reason(&key, &name),
                        Some(Fallback::PublicSignature),
                    );
                }
            }
            scope.insert(name, ty);
        }
        if let Some(ret) = ret_type {
            let ty = Ty::from_ast(self.parsed, ret);
            if holds_shared(&ty) {
                self.note(&key, RESULT, ty, true, true);
                if *is_public {
                    self.force(
                        &key,
                        RESULT,
                        published_reason(&key, RESULT),
                        Some(Fallback::PublicSignature),
                    );
                }
            } else if *is_public {
                for field in shared_fields(&ty.text(), &self.asked()) {
                    self.force(
                        FIELDS,
                        &field,
                        published_reason(&key, RESULT),
                        Some(Fallback::PublicSignature),
                    );
                }
            }
        }

        self.block(&key, body, &mut scope);
    }

    fn block(&mut self, function: &str, block: &Block, scope: &mut BTreeMap<String, Ty>) {
        for stmt in &block.stmts {
            self.stmt(function, &stmt.node, scope);
        }
    }

    fn stmt(&mut self, function: &str, stmt: &Stmt, scope: &mut BTreeMap<String, Ty>) {
        match stmt {
            Stmt::Let {
                names, ty, value, ..
            } => {
                // **One name only** ([ADR-291](../../../../docs/specification/adr/adr-291.md)).
                // A hull's count is decided per **value**, and what a tuple's
                // parts are is the call's business rather than this walk's - so
                // a destructure records no handle, which leaves each part at the
                // atomic floor [ADR-037](../../../../docs/specification/adr/adr-037.md)
                // D6 sets. Not recording is the safe direction: D7 may only ever
                // take an atomic **away**, and it can only do that for a value it
                // has an answer about.
                let [name] = names.as_slice() else {
                    return;
                };
                let name = self.parsed.text(*name).to_string();
                if self.walks_in_parallel(value) {
                    self.parallel_names.insert(name.clone());
                }
                let declared = ty.as_ref().map(|t| Ty::from_ast(self.parsed, t));
                // An untyped `let` that names a `Shared` is a second handle on
                // the same allocation, and takes its type from the first.
                let aliased = self.names_a_handle(value, scope);
                let ty = declared
                    .clone()
                    .or_else(|| aliased.as_ref().map(|(_, ty)| ty.clone()))
                    // `let k = Counter { … }` is a value of `Counter`, and a
                    // crossing of it reaches the `Shared` its field holds - the
                    // second of the three cases `docs/history/rc-or-arc.md` §5 names.
                    .or_else(|| match value {
                        Expr::StructLit { name, .. } => Some(Ty::named(self.parsed.text(*name))),
                        // **A hull written by a call**
                        // ([ADR-281](../../../../docs/specification/adr/adr-281.md)
                        // D2): `let counter = SharedMut(0)` makes one here and
                        // says so without an annotation. Without this case the
                        // slot is not watched at all, and a position nothing
                        // watched takes the floor - so the *constructor* would
                        // come out atomic while the parameter it is handed to came
                        // out plain, and the two ends of one value would disagree.
                        Expr::Call { func, args, .. } => match func.as_ref() {
                            Expr::Variable(name) => {
                                let text = self.parsed.text(*name);
                                // **And what it holds, where a literal says**
                                // (ADR-281 D8): `SharedMut(0)` holds an `i64`,
                                // which is what decides whether its lock is a
                                // word. Anything else is left unsaid, and an
                                // unsaid type is never a word.
                                is_hull(text).then(|| match args.as_slice() {
                                    [held] => match literal_type(held) {
                                        Some(held) => Ty::Named {
                                            name: text.to_string(),
                                            args: vec![Ty::named(&held)],
                                            view: false,
                                        },
                                        None => Ty::named(text),
                                    },
                                    _ => Ty::named(text),
                                })
                            }
                            _ => None,
                        },
                        _ => None,
                    });
                if let Some(ty) = ty {
                    if holds_shared(&ty) {
                        self.note(function, &name, ty.clone(), false, false);
                        match &aliased {
                            Some((other, _)) => {
                                self.join(&slot(function, &name), &slot(function, other))
                            }
                            // Nothing here watched this allocation being made.
                            // It came out of a call, an index, or a field of a
                            // type whose parts this compiler cannot walk - and a
                            // count belongs to the allocation, so a handle whose
                            // allocation is elsewhere is not one this analysis
                            // may lower.
                            //
                            // **Unless this line is where it is made**
                            // ([ADR-281](../../../../docs/specification/adr/adr-281.md)
                            // D2): `let counter = SharedMut(0)` is the allocation,
                            // and this analysis is watching it. Asked of the
                            // slot's type rather than of a written annotation -
                            // the constructor says which hull, and the line needs
                            // no annotation to say it.
                            None => match self.slot_of(function, value, scope) {
                                Some(source) => {
                                    self.join(&slot(function, &name), &source);
                                }
                                None if by_value_shared(&ty) && self.allocates_here(value) => {}
                                None => self.origin_unseen(function, &name, value),
                            },
                        }
                    }
                    scope.insert(name.clone(), ty);
                }
                self.expr(function, value, scope);
            }
            // **A `const` never holds a handle**
            // ([ADR-287](../../../docs/specification/adr/adr-287.md) D4): its
            // value is what the fold came to, and the fold evaluates literals
            // and arithmetic over them - a hull is made by a call, which D5
            // does not admit into an initialiser yet. So there is no count to
            // record and no second handle to find. The day D5's second stage
            // lands, this arm is where the question is asked again.
            Stmt::Comptime { .. } => {}

            Stmt::Assign { target, value, .. } => {
                // `a = b` makes `a` a handle on `b`'s allocation - and `a` may
                // be a field of a struct as easily as a name.
                if let Some(target_slot) = self.slot_of(function, target, scope) {
                    match self.slot_of(function, value, scope) {
                        Some(source) => self.join(&target_slot, &source),
                        None => {
                            let (at, name) = split_slot(&target_slot);
                            self.origin_unseen(&at, &name, value)
                        }
                    }
                }
                self.expr(function, target, scope);
                self.expr(function, value, scope);
            }
            Stmt::For { iter, body, .. } => {
                self.expr(function, iter, scope);
                self.block(function, body, scope);
            }
            Stmt::While { cond, body } => {
                self.expr(function, cond, scope);
                self.block(function, body, scope);
            }
            Stmt::Return(Some(value)) => {
                self.hands_back(function, value, scope);
                self.expr(function, value, scope);
            }
            // A jump carries no value, so nothing here joins two slots and
            // nothing reaches the result. The classes are about **where a
            // value goes**, and these send none anywhere.
            Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
            Stmt::Expr(value) => {
                // Part I 3.1: the last statement of a value-returning body is
                // the value, and joining it with the result slot costs nothing
                // where the function returns no `Shared`.
                self.hands_back(function, value, scope);
                self.expr(function, value, scope);
            }
        }
    }

    /// What a function hands back is the same allocation as whatever it names.
    fn hands_back(&mut self, function: &str, value: &Expr, scope: &BTreeMap<String, Ty>) {
        if let Some(source) = self.slot_of(function, value, scope) {
            self.join(&slot(function, RESULT), &source);
        }
    }

    /// Whether this `let` is itself the place the first handle is made.
    ///
    /// **The hull is written by a call**
    /// ([ADR-281](../../../../docs/specification/adr/adr-281.md) D2), so
    /// `let db = Shared(connect(…))` allocates the count on this line - and a
    /// count this analysis watched being made is not [`Fallback::UnseenOrigin`],
    /// whatever else it may turn out to be. An annotated `let` beside a plain
    /// value is no longer such a place, because it no longer constructs.
    ///
    /// **The question is only ever answered yes where something written down says
    /// the value is not already a handle**, which keeps the polarity this file
    /// runs on. A literal and a struct literal are plain values by construction.
    /// A call is one where a ledger gives it a result type and that type is known
    /// and holds no `Shared`; a result of `?` is the absence of a claim
    /// (ADR-024 D1) and is answered no, because a call that may hand a handle back
    /// is a handle whose allocation is elsewhere. Everything else is no.
    fn allocates_here(&self, value: &Expr) -> bool {
        nikaia_std::tools::sharing::allocates_here(value, &self.asked())
    }

    /// A handle whose allocation this analysis did not watch being made.
    ///
    /// The floor holds and says so. It is [`Fallback::UnseenOrigin`], and it is
    /// also the seed that step 1 made necessary: since ADR-037 D6 the
    /// overlapping analysis may run a call on a thread of its own and hand the
    /// result back (ADR-005 §5.2's `task::both` row), so a `Shared` that came
    /// out of a call is a `Shared` that crosses.
    fn origin_unseen(&mut self, function: &str, name: &str, value: &Expr) {
        self.force(
            function,
            name,
            nikaia_std::tools::sharing::origin_reason(value, &self.parsed.interner),
            Some(Fallback::UnseenOrigin),
        );
    }

    /// A crossing reached this name: whatever count it owns, or owns through a
    /// field, must be atomic.
    ///
    /// Two ways a crossing lands. The name may *be* a `Shared`, and then its own
    /// class is forced. Or it may be a value of a type that **holds** one, and
    /// then the field's class is - which is `send::crossing`'s transitivity
    /// (ADR-288's walk through the ledger's `fields`) asked the other way round:
    /// there a field that may not cross makes the struct refuse, here a struct
    /// that crosses makes the field's count atomic.
    fn reached(
        &mut self,
        function: &str,
        name: &str,
        scope: &BTreeMap<String, Ty>,
        why: &str,
        fallback: Option<Fallback>,
    ) {
        let Some(ty) = scope.get(name).cloned() else {
            return;
        };
        if holds_shared(&ty) {
            self.force(function, name, why.to_string(), fallback);
            return;
        }
        for field in shared_fields(&ty.text(), &self.asked()) {
            self.force(FIELDS, &field, why.to_string(), fallback);
        }
    }

    /// The handle an expression names, where it names one in this function's own
    /// scope.
    fn names_a_handle(&self, expr: &Expr, scope: &BTreeMap<String, Ty>) -> Option<(String, Ty)> {
        let name = nikaia_std::tools::sharing::named_handle(expr, &self.parsed.interner, scope)?;
        let ty = scope.get(&name)?.clone();
        Some((name, ty))
    }

    /// The slot an expression names, where this analysis has one for it.
    ///
    /// A name in scope, or a **field** of a value whose type the ledger records:
    /// `c.hits` is the `Counter.hits` slot, which is the same slot the function
    /// that built the `Counter` joined its handle to. Without this, reading a
    /// handle back out of a field produced a fresh class nothing forced - a
    /// sibling of the guard `docs/history/rc-or-arc.md` §8 describes, and fail-open in
    /// the same direction.
    fn slot_of(
        &mut self,
        function: &str,
        expr: &Expr,
        scope: &BTreeMap<String, Ty>,
    ) -> Option<String> {
        if let Some((name, _)) = self.names_a_handle(expr, scope) {
            return Some(slot(function, &name));
        }
        match expr {
            Expr::Field { base, name } => {
                let base = self.names_the_type(base, scope)?;
                let field = self.parsed.text(*name).to_string();
                let ty = declared_field_type(&base, &field, &self.asked())?;
                if !holds_shared(&ty) {
                    return None;
                }
                let key = format!("{base}.{field}");
                self.note(FIELDS, &key, ty, true, false);
                Some(slot(FIELDS, &key))
            }
            _ => None,
        }
    }

    /// The name of the type an expression has, where the scope says.
    fn names_the_type(&self, expr: &Expr, scope: &BTreeMap<String, Ty>) -> Option<String> {
        nikaia_std::tools::sharing::named_type(expr, &self.parsed.interner, scope)
    }

    fn expr(&mut self, function: &str, expr: &Expr, scope: &mut BTreeMap<String, Ty>) {
        match expr {
            // Part II 11.2: a task runs on a thread of its own.
            Expr::Spawn { body, .. } => {
                for name in send::names_used(self.parsed, body) {
                    // ADR-312 D1's second half: a task that uses a handle takes one
                    // of its own, so the name outside the task stays usable.
                    if scope.get(&name).is_some_and(by_value_shared) {
                        self.duplicates(
                            function,
                            &name,
                            "used by a `spawn` body, which takes a handle of its own"
                                .to_string(),
                        );
                    }
                    self.reached(
                        function,
                        &name,
                        &scope.clone(),
                        "a `spawn` body uses it, and a task runs on a thread of its own",
                        None,
                    );
                }
                self.expr(function, body, scope);
            }
            Expr::Call { func, args, config } => {
                let callee = self.path_of(func);
                if matches!(callee.as_deref(), Some("access_all" | "update_all")) {
                    for arg in args {
                        if let Some((handle, _)) = self.names_a_handle(arg, scope) {
                            self.slots.held_together.push(slot(function, &handle));
                        }
                    }
                }
                self.arguments(function, callee.as_deref(), false, args, config, scope);
                self.expr(function, func, scope);
            }
            Expr::MethodCall {
                receiver,
                method,
                args,
                config,
            }
            // **A handle handed to a `?.m()` is handed over.** Whether the call
            // happens does not change where the value would go if it did, and
            // this analysis may not miss a place it could (ADR-312 D1).
            | Expr::SafeMethod {
                receiver,
                method,
                args,
                config,
            } => {
                let method = self.parsed.text(*method).to_string();
                // **A `get` of a value that does not copy cheaply** (ADR-281
                // D3): it copies the whole of it out each time, where `access`
                // would read it in place. Named where the type says so, and
                // silent where it does not.
                if method == "get"
                    && args.is_empty()
                    && let Some((handle, ty)) = self.names_a_handle(receiver, scope)
                    && let Some(held) = held_that_does_not_copy(&ty)
                {
                    self.slots.copies.push((
                        slot(function, &handle),
                        format!("`{handle}.get()` copies the whole `{held}` out of the lock"),
                    ));
                }
                // **And every walk chained onto one**
                // ([ADR-235](../../../../docs/specification/adr/adr-235.md) D1):
                // `xs.par_iter().map fn …` hands its lambda to `map`, and the
                // lambda is what runs on every core.
                if nikaia_std::tools::sharing::a_parallel_method(&method) || self.walks_in_parallel(receiver) {
                    let why = format!(
                        "a lambda handed to `{method}` uses it, and that lambda runs on a \
                         thread the program asked for"
                    );
                    for arg in args {
                        for name in send::names_used(self.parsed, arg) {
                            self.reached(function, &name, &scope.clone(), &why, None);
                        }
                    }
                }
                // A method is resolved by name only (ADR-296 D17), so a method
                // nothing describes is a body this compiler cannot see the end
                // of - the same case as an unseen call, and answered the same
                // way.
                self.arguments(function, Some(&method), true, args, config, scope);
                self.expr(function, receiver, scope);
            }
            // A `Shared` put into a struct is the struct's field's allocation,
            // and the field is where a crossing of the *struct* lands.
            Expr::StructLit { name, fields } => {
                let struct_name = self.parsed.text(*name).to_string();
                for field in fields {
                    let field_name = self.parsed.text(field.name).to_string();
                    // `Counter { hits }` is the shorthand: the field takes the
                    // variable of its own name (Part I 4.1).
                    let value = field.value.clone().unwrap_or(Expr::Variable(field.name));
                    let field_slot = format!("{struct_name}.{field_name}");
                    match self.names_a_handle(&value, scope) {
                        Some((handle, ty)) => {
                            self.note(FIELDS, &field_slot, ty, true, false);
                            self.join(&slot(function, &handle), &slot(FIELDS, &field_slot));
                        }
                        None => {
                            // A `Shared`-holding field filled from something
                            // this analysis cannot follow. The floor holds for
                            // the field, and therefore for every handle that
                            // ever joins it.
                            let declared =
                                declared_field_type(&struct_name, &field_name, &self.asked());
                            if let Some(ty) = declared.filter(holds_shared) {
                                self.note(FIELDS, &field_slot, ty, true, false);
                                match self.slot_of(function, &value, scope) {
                                    Some(source) => self.join(&source, &slot(FIELDS, &field_slot)),
                                    None => self.origin_unseen(FIELDS, &field_slot, &value),
                                }
                            }
                        }
                    }
                    self.expr(function, &value, scope);
                }
            }
            // **A hole is Nikaia source and is walked like any other**
            // (ADR-309 D13, which the type checker already follows). Measured:
            // `println(f"{hold(c)}")` handed a handle to a function this analysis
            // never saw, so the value kept the plain count while the callee's
            // parameter was decided atomic in the file declaring it - and the two
            // met in one generated file as `Rc` against `Arc`. Any analysis that
            // stops at a literal is one a hole can be hidden in.
            Expr::LitInterpolated { .. } => {
                for hole in crate::emit::literal_expressions(self.parsed, expr) {
                    self.expr(function, &hole, scope);
                }
            }
            Expr::Closure { body, .. } => self.block(function, body, scope),
            Expr::Block(block) | Expr::Overlap(block) => self.block(function, block, scope),
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(function, cond, scope);
                self.block(function, then_branch, scope);
                if let Some(block) = else_branch {
                    self.block(function, block, scope);
                }
            }
            Expr::Match { value, arms } => {
                self.expr(function, value, scope);
                for arm in arms {
                    self.expr(function, &arm.body, scope);
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(function, lhs, scope);
                self.expr(function, rhs, scope);
            }
            Expr::Unary { expr, .. }
            | Expr::Try(expr)
            | Expr::Throw(expr)
            | Expr::Cast { expr, .. } => self.expr(function, expr, scope),
            Expr::Field { base, .. } | Expr::SafeField { base, .. } => {
                self.expr(function, base, scope)
            }
            Expr::Index { base, index } => {
                self.expr(function, base, scope);
                self.expr(function, index, scope);
            }
            Expr::Coalesce { value, fallback } => {
                self.expr(function, value, scope);
                self.expr(function, fallback, scope);
            }
            Expr::TryCatch { expr, handler } => {
                self.expr(function, expr, scope);
                self.block(function, handler, scope);
            }
            Expr::Tuple(parts) => {
                for part in parts {
                    self.expr(function, part, scope);
                }
            }
            Expr::Range { start, end, .. } => {
                self.expr(function, start, scope);
                self.expr(function, end, scope);
            }
            _ => {}
        }
    }

    /// What happens to a handle handed to a call.
    ///
    /// Two answers and they are the two halves of the finding. A callee the
    /// ledger describes has a parameter slot, and the handle joins it: one
    /// allocation, one count, whichever side of the call decides it. A callee
    /// nothing describes is ADR-303 D7's case: it may start a thread of its own,
    /// so the count is atomic.
    ///
    /// **Every name inside the argument counts, not only a bare variable.** An
    /// argument may be a lambda that *captures* a handle, or a tuple, or an
    /// expression with a handle somewhere in it, and a callee nothing describes
    /// may cross with any of them. `send::names_used` is the same
    /// over-approximate walk `spawn` uses, and over-approximate is the safe
    /// direction here too. Leaving it at `Expr::Variable` was a sibling of
    /// `docs/history/rc-or-arc.md` §8's guard: it read reasonably and it failed open.
    ///
    /// **And the options after the `;` are arguments.** `f(x; opt: handle)` hands
    /// a handle over as surely as `f(x, handle)` does, and no contract covers
    /// where it went, so it is [`Fallback::UncoveredArgument`].
    fn arguments(
        &mut self,
        function: &str,
        callee: Option<&str>,
        is_method: bool,
        args: &[Expr],
        config: &[crate::ast::ConfigArg],
        scope: &mut BTreeMap<String, Ty>,
    ) {
        let described = callee.and_then(|callee| self.parameters(callee));
        let kind = match is_method {
            true => Fallback::UnseenMethod,
            false => Fallback::UnseenCall,
        };
        for (at, arg) in args.iter().enumerate() {
            let Some((handle, _)) = self.names_a_handle(arg, scope) else {
                // Not a handle itself. It may hold one, capture one, or be an
                // expression with one inside it - and a callee nothing describes
                // may put any of those on a thread.
                if described.is_none() {
                    let why = unseen(callee);
                    for name in send::names_used(self.parsed, arg) {
                        self.reached(function, &name, &scope.clone(), &why, Some(kind));
                    }
                }
                // A handle read out of a **field** is a handle handed on by value
                // as surely as a named one is: `keep(pool.db)` gives the callee an
                // owner. The field has a slot, so it joins the parameter's class
                // exactly as a name would - without which the field's class was
                // left unjoined and could disagree with the parameter's about
                // which count it is, which is the fail-open direction
                // `docs/history/rc-or-arc.md` §8 warns about.
                if let Some((key, params)) = &described
                    && let Some((param, param_ty)) = params.get(at)
                    && by_value_shared(param_ty)
                    && let Some(source) = self.slot_of(function, arg, scope)
                {
                    let (key, param) = (key.clone(), param.clone());
                    let (at, name) = split_slot(&source);
                    self.duplicates(&at, &name, handed_on_to(&key, &param));
                    self.join(&source, &slot(&key, &param));
                }
                self.expr(function, arg, scope);
                continue;
            };
            match (&described, callee) {
                (Some((key, params)), _) => match params.get(at) {
                    Some((param, param_ty)) => {
                        let (key, param) = (key.clone(), param.clone());
                        // ADR-312 D1: a handle handed on **by value** is
                        // duplicated; one the callee only borrows is not.
                        if by_value_shared(param_ty) {
                            self.duplicates(function, &handle, handed_on_to(&key, &param));
                        }
                        self.join(&slot(function, &handle), &slot(&key, &param));
                    }
                    // More arguments than the contract has parameters: nothing
                    // written down says where this one goes.
                    None => self.force(
                        function,
                        &handle,
                        format!(
                            "`{}` takes it in a position no contract describes",
                            callee.unwrap_or("the callee")
                        ),
                        Some(Fallback::UncoveredArgument),
                    ),
                },
                (None, callee_name) => {
                    self.force(function, &handle, unseen(callee_name), Some(kind))
                }
            }
        }
        for arg in config {
            for name in send::names_used(self.parsed, &arg.value) {
                self.reached(
                    function,
                    &name,
                    &scope.clone(),
                    &format!(
                        "it is handed to `{}` as the option `{}`, and no contract says what \
                         happens to a `Shared` in an option",
                        callee.unwrap_or("the callee"),
                        self.parsed.text(arg.name)
                    ),
                    Some(Fallback::UncoveredArgument),
                );
            }
            self.expr(function, &arg.value, scope);
        }
    }

    /// A callee's ledger key and its parameters - name and declared type -
    /// where a ledger has them.
    ///
    /// The program's own ledger first, then `std`'s, with the suffix rule every
    /// other resolution in this compiler uses (ADR-296 D17).
    ///
    /// **The type is here for one question only**: whether the position takes
    /// the handle by value or lends the inner value out. A by-value `Shared`
    /// parameter is a second handle (ADR-312 D1); a `&Shared` one is a borrow
    /// and duplicates nothing. Neither changes which count the class gets.
    fn parameters(&self, callee: &str) -> Option<(String, Vec<(String, Ty)>)> {
        let suffix = format!("::{callee}");
        let (key, contract) = [self.own, self.library].into_iter().find_map(|ledger| {
            ledger.functions.get_key_value(callee).or_else(|| {
                ledger
                    .functions
                    .iter()
                    .find(|(key, _)| key.ends_with(&suffix))
            })
        })?;
        let signature = contract.signature.as_ref()?;
        Some((key.clone(), signature.arguments().to_vec()))
    }

    // --- colouring -------------------------------------------------------

    /// One pass over the seeds, then one answer per handle - and the summary.
    /// What every handle got, read off the classes.
    fn decide(mut self) -> Sharing {
        decided(&mut self.slots)
    }
}

impl Analysis<'_> {
    /// The interner and the two ledgers, as the Nikaia half reads them.
    fn asked(&self) -> Asked<'_> {
        Asked {
            names: &self.parsed.interner,
            own: self.own,
            library: self.library,
        }
    }

    /// The qualified name a call names, where it names one.
    /// Whether a receiver is a walk that runs in parallel: a `par_iter()`, a
    /// name bound to one, or a walk chained onto either.
    fn walks_in_parallel(&self, receiver: &Expr) -> bool {
        nikaia_std::tools::sharing::walks_in_parallel(
            receiver,
            &self.parsed.interner,
            &self.parallel_names,
        )
    }

    fn path_of(&self, func: &Expr) -> Option<String> {
        nikaia_std::tools::sharing::call_path(func, &self.parsed.interner)
    }
}
