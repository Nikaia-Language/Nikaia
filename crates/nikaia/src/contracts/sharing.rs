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
// enumeration of the crossings, asked per value rather than per type. An
// `overlap` branch is not one: it is polled on the block's own thread
// (ADR-292 D4). A handle whose allocation this analysis did not watch being
// made - one a call handed back - stays at the floor through
// [`Fallback::UnseenOrigin`], because what is not seen is not proved.
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

use crate::ast::Expr;
use crate::parser::Parsed;

use super::{Ledger, ty::Ty};
use crate::contracts::ty::TyOps;

/// Every reason this analysis answers `atomic` **because nothing decided it**
/// ([`Fallback`]), what it chose for one value ([`Decision`]) and for all of
/// them ([`Sharing`]) are records of `tools/sharing.nika` (ADR-294, #125).
pub use nikaia_std::tools::sharing::{
    Decision, Fallback, Sharing, every_fallback, lock_name, rust_name,
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
    //
    // **The walk is Nikaia** (`tools/sharing.nika`, #125): every handle a unit
    // makes or is handed, joined into allocation classes, with what forces a
    // class atomic. What a written type means is this compiler's to say, and
    // the holes of a literal are its parser's.
    let asked = Asked {
        names: &parsed.interner,
        own,
        library,
    };
    nikaia_std::tools::sharing::sharing_of(
        &parsed.program,
        &asked,
        &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
        &|ty: &crate::ast::Type| Ty::from_ast(parsed, ty),
        crossings_are_possible,
    )
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
