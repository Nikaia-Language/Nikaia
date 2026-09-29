// crates/nikaia/src/contracts/sharing.rs
//
// Which reference count a **particular** `Shared` value gets
// ([ADR-037](../../../../docs/specification/adr/adr-037.md) D7).
//
// ## One rule, and everything here is downstream of it
//
//     It may only ever take an atomic away.
//
// ADR-037 D6 made the atomic count the **floor**, and ADR-061 D2 lowered that
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
// same thing. ADR-033 D4's polarity, applied where it is cheap.
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
// fixpoint reached in a single step, where `sync` (ADR-027 D1) genuinely needs a
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
//     (ADR-024, the walk ADR-029 established) - run in the opposite direction
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
//     duplicates nothing (ADR-040 D1) and what comes out is a view of the value
//     inside, so there is no handle for it to keep.
//     `a_std_entry_that_takes_a_shared_needs_a_second_look` in `tests/sharing.rs`
//     is what makes somebody look on the day one does take one.
//
// Everywhere else is [`Fallback`], and the remedy for every row of it is to
// write the contract down - ADR-033 D4's own closing argument, which is why
// ADR-037 D8 has no keyword in it.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Block, Expr, Item, Stmt};
use crate::parser::Parsed;

use super::{Ledger, send, ty::Ty};

/// The type whose count this is about.
const SHARED: &str = "Shared";

/// The shared mutable type, which is a **count around a lock**
/// ([ADR-064](../../../../docs/specification/adr/adr-064.md) D1) - so everything
/// this file decides about a count, it decides about one of these too. It is one
/// name up to the emitter, which is why it is matched by name here rather than
/// being seen as the `Shared` it expands to.
const SHARED_MUT: &str = "SharedMut";

/// The name of the slot standing for what a function hands back.
const RESULT: &str = "<result>";

/// The pseudo-function every `<struct>.<field>` slot is filed under.
const FIELDS: &str = "<field>";

/// Methods that run their lambda on a thread the program asked for.
///
/// A closed list for the same reason `send::PLAIN` is one: a name this file does
/// not know contributes nothing, and the walk that finds the names inside the
/// lambda is over-approximate, which is the safe direction.
const PARALLEL: &[&str] = &["par_iter", "par_fold", "par_map"];

/// Which kind of count a `Shared` value needs.
///
/// `Plain ⊑ Atomic`, joined upwards: one crossing anywhere in a handle's
/// equivalence class makes the whole class atomic. [`Count::Atomic`] is the
/// default because it is the floor (ADR-037 D6) - a value nothing is known about
/// gets the safe answer without anybody remembering to ask for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Count {
    /// A count only the thread that built it ever touches. What this analysis
    /// may lower a value to, where it proves nothing crosses.
    Plain,
    /// **A `SharedMut` of a value that fits a machine word, which crosses**
    /// ([ADR-238](../../../../docs/specification/adr/adr-238.md)): the atomic
    /// count, and the lock inside it the word itself - `update` is a
    /// compare-and-swap (ADR-110 D2). Only for a crossing this analysis
    /// **proved** and a value no door over several locks takes: a class the
    /// floor answered for, or one an `update_all` holds, keeps the lock.
    Word,
    /// A count any thread may touch. The floor, and the answer wherever this
    /// analysis cannot prove the other one.
    #[default]
    Atomic,
}

impl Count {
    /// The more cautious of two.
    pub fn join(self, other: Self) -> Self {
        self.max(other)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Count::Plain => "plain",
            Count::Word => "word",
            Count::Atomic => "atomic",
        }
    }

    /// Read one back from the ledger.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "plain" => Some(Count::Plain),
            "word" => Some(Count::Word),
            "atomic" => Some(Count::Atomic),
            _ => None,
        }
    }
}

/// Every reason this analysis answers `atomic` **because nothing decided it**,
/// as a closed list.
///
/// The list is the deliverable and not a detail. ADR-010 D1 asks a fail-closed
/// analysis to name what it could not see, ADR-037 D8 asks "would an override
/// help?" of every row of it, and the answer is no for every row - the first
/// five want a **contract** and the last one is a promise about somebody else's
/// code. A row added here is a row that has to be answered there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fallback {
    /// The value is a parameter or the result of a **public** function, so its
    /// callers are in a unit this build never reads.
    PublicSignature,
    /// The value is held by a **public field of a public type**, which code this
    /// build never reads may take out of it.
    PublicField,
    /// The value was handed to a **call** nothing written down describes.
    UnseenCall,
    /// The value was handed to a **method** nothing written down describes.
    UnseenMethod,
    /// The value was handed over in an **argument position** no contract covers.
    UncoveredArgument,
    /// The handle exists and this analysis did not watch its allocation being
    /// made - it came out of a call, an index, a field of a type whose `fields`
    /// are unrecorded, or some other expression nothing here accounts for.
    UnseenOrigin,
    /// The slot belongs to **another file of this package**, whose own run of
    /// this analysis decides its count.
    ///
    /// This analysis runs once per file (`analyse_program` takes one `Parsed`),
    /// so neither run sees the whole of such a slot: the declaring file sees the
    /// slot and not this value, this one the value and not what the slot was
    /// decided to be. Measured, before this row existed - a public **field**
    /// declared in `pool.nika` and filled in `main.nika`:
    ///
    /// ```text
    /// pub db: std::sync::Arc<Conn>,      // decided in pool.nika
    /// let c: std::rc::Rc<Conn> = …       // decided in main.nika
    /// ```
    ///
    /// and, in exactly the same way, a public **parameter**: `hold(c)` where
    /// `hold` is declared in another file expects an `Arc` and is handed an `Rc`.
    /// `rustc` refused both, about a file nobody wrote (Part III, C.1). The
    /// answer is the polarity this analysis already runs on: where it cannot
    /// prove that nothing crosses, it does not lower.
    ForeignFile,
}

impl Fallback {
    /// Every row, for a report that enumerates rather than summarises.
    pub const ALL: &'static [Fallback] = &[
        Fallback::PublicSignature,
        Fallback::PublicField,
        Fallback::UnseenCall,
        Fallback::UnseenMethod,
        Fallback::UncoveredArgument,
        Fallback::UnseenOrigin,
        Fallback::ForeignFile,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Fallback::PublicSignature => "a public signature",
            Fallback::PublicField => "a public field",
            Fallback::UnseenCall => "a call nothing describes",
            Fallback::UnseenMethod => "a method nothing describes",
            Fallback::UncoveredArgument => "an argument position no contract covers",
            Fallback::UnseenOrigin => "an origin this analysis cannot see",
            Fallback::ForeignFile => "a slot another file owns",
        }
    }

    /// What a person would do about it, which is ADR-037 D8's whole answer: for
    /// five of the seven, write the contract; for the other two, nothing, because
    /// the answer belongs to code this run does not read.
    pub fn remedy(self) -> &'static str {
        match self {
            Fallback::PublicSignature => {
                "nothing here - whether it crosses is decided by callers this build does not \
                 read, so an answer would be a promise about somebody else's code"
            }
            Fallback::PublicField => {
                "keep the field out of the published surface, or accept the atomic - code this \
                 build does not read can take the value out of it"
            }
            Fallback::UnseenCall | Fallback::UnseenMethod | Fallback::UncoveredArgument => {
                "write the callee down in a ledger, which is how every other derived fact in \
                 this project gets sharper (ADR-033 D4)"
            }
            Fallback::UnseenOrigin => {
                "write down the call the value came out of, so this analysis can follow it to \
                 the allocation"
            }
            Fallback::ForeignFile => {
                "nothing here - the file that declares the field or the function decides its \
                 count, and this run reads only one of the two"
            }
        }
    }
}

/// What this analysis chose for one `Shared` value, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The function the value is written in, as a ledger key
    /// (`Counter::record`).
    pub function: String,
    /// The name the source gives it, or `<result>` for what a function hands
    /// back.
    pub value: String,
    /// Its type, as written.
    pub ty: String,
    pub count: Count,
    /// What kept the count atomic, in the words a user would be told. `None`
    /// for a plain one, which needs no excuse.
    pub why: Option<String>,
    /// Which row of [`Fallback`] this is, where it is one. `None` where the
    /// analysis *found* the crossing rather than failing to rule one out - a
    /// `spawn` is a crossing, not a mystery.
    pub fallback: Option<Fallback>,
    /// Every place a second handle on this allocation is made, in the words a
    /// user would be told ([ADR-040](../../../../docs/specification/adr/adr-040.md)
    /// D5).
    ///
    /// D1 makes a handle duplicated where it is handed on **by value**, and there
    /// is no method to call - so the place one step of the count is paid is
    /// unwritten in the source. D5 asks for it to be readable somewhere, and this
    /// is that somewhere: the count is already printed here, so what an extra
    /// handle costs is printed beside it.
    ///
    /// **A borrow contributes nothing**, which is D1's own correction: lending the
    /// inner value out hands no handle on, so there is nothing to duplicate and no
    /// instruction to pay.
    pub duplications: Vec<String>,
    /// **Where a `get` copies the whole value out of the lock**
    /// ([ADR-238](../../../../docs/specification/adr/adr-238.md) D3): the
    /// value is one that does not copy cheaply, a text or a list, and `access`
    /// would have read it in place.
    pub copied_out: Vec<String>,
    /// Why a word-sized value that crosses kept its lock, where it did
    /// (ADR-238 D2).
    pub kept_its_lock: Option<&'static str>,
}

impl Decision {
    /// Whether the atomic count is the floor holding rather than a crossing
    /// this analysis found. The list ADR-010 D1 asks to be named.
    pub fn undecided(&self) -> bool {
        self.fallback.is_some()
    }
}

/// One allocation class of a function's caller-visible positions, as the ledger
/// records it.
///
/// This is the summary ADR-037 D7 names as the thing that
/// composes: **which parameters and result are one class, and which count that
/// class gets**. A caller reads it to know what it is handing over; a later
/// build that can read a dependency's source reads it to continue the analysis
/// across the boundary without a representation axis.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Class {
    /// The positions that are one allocation: parameter names, and `<result>`
    /// for what the function hands back. Sorted, because 13.5 makes the file a
    /// pure function of (source, toolchain).
    pub members: Vec<String>,
    pub count: Count,
}

impl Class {
    /// `counts | <result>: atomic`, which is the `borrows(a | b)` spelling
    /// ADR-005 D3 already gave a list of positions.
    pub fn text(&self) -> String {
        format!("{}: {}", self.members.join(" | "), self.count.as_str())
    }

    /// Read one back from the ledger.
    pub fn parse(text: &str) -> Option<Self> {
        let (members, count) = text.rsplit_once(':')?;
        let members: Vec<String> = members
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        match members.is_empty() {
            true => None,
            false => Some(Class {
                members,
                count: Count::parse(count)?,
            }),
        }
    }
}

/// Everything one program's `Shared` values got, and the summary the ledger
/// keeps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sharing {
    /// One per `Shared` value a person wrote, sorted by function and then by
    /// name, because a report is read and compared.
    pub decisions: Vec<Decision>,
    /// Per function key, the allocation classes of its caller-visible
    /// positions. What [`super::FnContract::sharing`] records.
    pub summaries: BTreeMap<String, Vec<Class>>,
    /// The count every slot's class got, by the slot key `function::value`.
    ///
    /// [`Sharing::decisions`] is the **report** - one line per value a person
    /// wrote - and it leaves the internal slots out: a function's `<result>`,
    /// and a `<struct>.<field>`. The **emitter** needs all of them, because
    /// `Shared[T]` is written at those positions too and every one of them has
    /// to be lowered to the type its class was given. So this is the same
    /// answer, keyed for a lookup rather than shaped for a reader.
    ///
    /// It is handed to the emitter the way `check::Checked::fallible_loops` and
    /// `fallible_methods` are: computed where the knowledge is, looked up where
    /// the code is written. A slot this map has no entry for gets
    /// [`Count::Atomic`] from [`Sharing::count_of`], because that is the floor
    /// (ADR-037 D6) and a site nothing is known about may not be lowered.
    pub counts: BTreeMap<String, Count>,
}

impl Sharing {
    /// Every count plain, because nothing can cross
    /// ([ADR-061](../../../../docs/specification/adr/adr-061.md) D2).
    ///
    /// Applied to the finished answer rather than instead of computing it, so
    /// that the **duplication** sites - which are about handing a handle on and
    /// not about threads - are kept exactly as they were found, and so that one
    /// run of this analysis is one code path at both settings.
    fn all_plain(mut self) -> Sharing {
        for decision in &mut self.decisions {
            decision.count = Count::Plain;
            decision.why = None;
            decision.fallback = None;
        }
        for count in self.counts.values_mut() {
            *count = Count::Plain;
        }
        for classes in self.summaries.values_mut() {
            for class in classes.iter_mut() {
                class.count = Count::Plain;
            }
        }
        self
    }

    /// The count one slot's class got, with the floor where nothing is known.
    ///
    /// `value` is the name the source gives it, `<result>` for what a function
    /// hands back, or `<struct>.<field>` under `<field>` for a field.
    pub fn count_of(&self, function: &str, value: &str) -> Count {
        self.counts
            .get(&slot(function, value))
            .copied()
            .unwrap_or_default()
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
    // ([ADR-061](../../../../docs/specification/adr/adr-061.md) D2). At
    // `user_parallelism = no` one thread runs the user's code; the runtime's I/O
    // thread carries none of it, a task interleaves on the same thread, and since
    // D1 a `Shared` may not be handed to code this compiler cannot see - which
    // was the last way out. So every count is plain, and the seven reasons to
    // decline are reasons to decline *a proof about another thread*, which no
    // longer has to be found.
    //
    // The **verdict** is untouched and stays switch-independent
    // ([ADR-045](../../../../docs/specification/adr/adr-045.md) D1): a
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
    let sharing = analysis.decide();
    match crossings_are_possible {
        true => sharing,
        false => sharing.all_plain(),
    }
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
/// not reach ([ADR-061](../../../../docs/specification/adr/adr-061.md)). What
/// this writes is a **contract** — a statement about a function's positions that
/// a reader and a later build consult — and a contract that moved with a build
/// switch would be the thing ADR-045 D1 refuses. What is emitted is the
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
/// decision rather than changing one** (ADR-033 D9), which is what a person
/// reads when they want the 9 ns back.
pub fn report(
    parsed: &Parsed,
    own: &Ledger,
    library: &Ledger,
    crossings_are_possible: bool,
) -> String {
    let sharing = analyse_program(parsed, own, library, crossings_are_possible);
    if sharing.decisions.is_empty() {
        // "here" and not "in this program": a report is about one file, and a
        // package of several files is several of them (`project::explain`).
        return "no `Shared` value here, so there is nothing to choose.\n".to_string();
    }

    let mut out = String::new();
    let mut at: Option<&str> = None;
    for decision in &sharing.decisions {
        if at != Some(decision.function.as_str()) {
            out.push_str(&format!("{}:\n", decision.function));
            at = Some(&decision.function);
        }
        out.push_str(&format!(
            "    {:<7} `{}` ({})\n",
            decision.count.as_str(),
            decision.value,
            decision.ty,
        ));
        match (&decision.why, decision.fallback) {
            (Some(why), Some(_)) => {
                out.push_str(&format!("             could not decide: {why}\n"))
            }
            (Some(why), None) if decision.count == Count::Word => {
                out.push_str(&format!("             crosses: {why}\n"));
                out.push_str(
                    "             a word, and no door over several locks takes it, so its \
                     `update` is a compare-and-swap and there is no lock (ADR-110 D2)\n",
                );
            }
            (Some(why), None) => out.push_str(&format!("             crosses: {why}\n")),
            (None, _) => out.push_str(
                "             nothing crosses a thread with it, so the count is lowered\n",
            ),
        }
        if let Some(kept) = decision.kept_its_lock {
            out.push_str(&format!("             {kept}\n"));
        }
        for site in &decision.copied_out {
            out.push_str(&format!(
                "             copied out: {site} each time - `access` reads it in place\n"
            ));
        }
        // ADR-040 D5: a handle is duplicated where it is handed on by value, and
        // the source does not say so - one step of the count is paid there. So the
        // places are named beside the count, which is the output D5 asks for.
        match decision.duplications.as_slice() {
            [] => out
                .push_str("             one handle, so the count is never stepped (ADR-040 D1)\n"),
            sites => {
                for site in sites {
                    out.push_str(&format!("             duplicated: {site}\n"));
                }
            }
        }
    }

    let atomic = sharing
        .decisions
        .iter()
        .filter(|d| d.count != Count::Plain)
        .count();
    let undecided: BTreeSet<Fallback> = sharing
        .decisions
        .iter()
        .filter_map(|d| d.fallback)
        .collect();
    out.push_str(&format!(
        "\n{} `Shared` value(s): {} plain, {} atomic, of which {} because this analysis \
         could not decide.\n",
        sharing.decisions.len(),
        sharing.decisions.len() - atomic,
        atomic,
        sharing.decisions.iter().filter(|d| d.undecided()).count(),
    ));
    out.push_str(
        "atomic is the floor and not a verdict (ADR-037 D6). A plain count is an optimisation, \
         and where it is not taken the reason is one of these:\n",
    );
    for fallback in Fallback::ALL {
        out.push_str(&format!(
            "  {} {} - {}\n",
            match undecided.contains(fallback) {
                true => "*",
                false => " ",
            },
            fallback.as_str(),
            fallback.remedy(),
        ));
    }
    out.push_str(
        "The five that want a contract are what ADR-037 D8 answers with `std.contracts` \
         growing rather than with a keyword.\n",
    );
    out
}

/// One handle, named by the function it is written in.
fn slot(function: &str, name: &str) -> String {
    format!("{function}::{name}")
}

/// Everything a file declares that a slot can belong to.
///
/// A slot key is `owner::name`, and the owner is a struct (`<field>` slots), a
/// function, or a `Type::method`. Which of them this file declares is what says
/// whether this run of the analysis can see the whole of that slot.
fn declared_here(parsed: &Parsed) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for item in &parsed.program.items {
        match &item.node {
            Item::Struct { name, .. } | Item::Enum { name, .. } => {
                names.insert(parsed.text(*name).to_string());
            }
            Item::Fn {
                name: Some(name), ..
            } => {
                names.insert(parsed.text(*name).to_string());
            }
            Item::Impl {
                target, methods, ..
            } => {
                let target = parsed.text(target.name).to_string();
                for method in methods {
                    if let Item::Fn { name, .. } = &method.node {
                        let own = match name {
                            Some(name) => parsed.text(*name).to_string(),
                            None => "new".to_string(),
                        };
                        names.insert(format!("{target}::{own}"));
                    }
                }
                names.insert(target);
            }
            _ => {}
        }
    }
    names
}

/// Handles joined into allocation classes, with a reason recorded against the
/// ones that cross.
struct Analysis<'a> {
    parsed: &'a Parsed,
    own: &'a Ledger,
    library: &'a Ledger,
    /// Everything **this file** declares that a slot can belong to: its structs,
    /// its functions, and its methods under `Type::method`.
    ///
    /// What it is for is the other side of it: a slot whose owner is not in here
    /// belongs to another file, and this run cannot see what that file decided
    /// about it ([`Fallback::ForeignFile`]).
    declared_here: BTreeSet<String>,
    /// Every `Shared` handle: its slot key, and what was written about it.
    handles: BTreeMap<String, Handle>,
    /// Union-find over slot keys, by index into `parent`.
    index: BTreeMap<String, usize>,
    parent: Vec<usize>,
    /// What forced a slot's class to be atomic.
    forced: Vec<(String, String, Option<Fallback>)>,
    /// Where a second handle on a slot's allocation is made
    /// ([ADR-040](../../../../docs/specification/adr/adr-040.md) D5): the slot
    /// key, and what a reader is told about the place.
    duplicated: Vec<(String, String)>,
    /// The names bound to a walk that runs in parallel
    /// ([ADR-235](../../../../docs/specification/adr/adr-235.md) D1), so a walk
    /// chained onto the name is one too. Over-approximate - a name is never
    /// taken out - which is this file's safe direction.
    parallel_names: BTreeSet<String>,
    /// The slots a door over several locks names
    /// ([ADR-238](../../../../docs/specification/adr/adr-238.md) D2): an
    /// `access_all` or an `update_all` holds each lock across the block, and a
    /// compare-and-swap cannot be held, so their classes keep the lock.
    held_together: Vec<String>,
    /// Where a `get` copies a value that does not copy cheaply out of its
    /// lock, by slot (ADR-238 D3).
    copies: Vec<(String, String)>,
}

struct Handle {
    function: String,
    value: String,
    ty: Ty,
    /// A slot that stands for a struct's field or a function's result rather
    /// than for something a person wrote. Not reported, but classes join
    /// through it.
    internal: bool,
    /// A position a **caller** meets - a parameter, or the result. What the
    /// ledger's summary is about.
    position: bool,
}

impl<'a> Analysis<'a> {
    fn new(parsed: &'a Parsed, own: &'a Ledger, library: &'a Ledger) -> Self {
        Analysis {
            parsed,
            own,
            library,
            declared_here: declared_here(parsed),
            handles: BTreeMap::new(),
            index: BTreeMap::new(),
            parent: Vec::new(),
            forced: Vec::new(),
            duplicated: Vec::new(),
            parallel_names: BTreeSet::new(),
            held_together: Vec::new(),
            copies: Vec::new(),
        }
    }

    /// **A slot another file owns keeps the atomic floor**
    /// ([`Fallback::ForeignFile`]).
    ///
    /// This analysis runs once per **file**, and a package of several files is
    /// several runs of it. A slot whose owner another file declares is therefore
    /// one neither run sees the whole of: the declaring file sees the slot and
    /// not this value, this one the value and not what the slot was decided to
    /// be. Measured, both halves in one generated file - a public field:
    ///
    /// ```text
    /// pub db: std::sync::Arc<Conn>,      // decided where `Pool` is declared
    /// let c: std::rc::Rc<Conn> = …       // decided here
    /// ```
    ///
    /// and, in exactly the same way, a **public parameter**: `hold(c)` where
    /// `hold` is declared in another file expects an `Arc` and is handed an `Rc`.
    /// `rustc` refused both, about a file nobody wrote (Part III, C.1).
    ///
    /// One place rather than one per site, because a slot is created wherever a
    /// struct is built, a field is read or assigned, a call is made and a result
    /// is bound - and a rule that has to be remembered at five sites is one that
    /// will be forgotten at the sixth. Here every slot that exists is asked once,
    /// after the walk and before the classes are read off, so a slot no future
    /// site thought about is covered by having been created at all.
    ///
    /// The declaring file forces the same slot itself wherever it is published -
    /// a public field, a public signature - so for the shapes this is about the
    /// two runs agree by both refusing to lower. Where it does not, this file
    /// cannot name the owner either, so the caution costs nothing.
    fn foreign_slots_hold_the_floor(&mut self) {
        // Over the **union-find** and not over `handles`: a slot another file owns
        // has no handle in this run - nothing here declared it - and joining to
        // it is the only trace of it there is. `main::c` joined to `hold::c` was
        // exactly that case, and reading `handles` missed it.
        let foreign: Vec<(String, String)> = self
            .index
            .keys()
            .filter_map(|key| {
                let (owner, slot) = match key.strip_prefix(&format!("{FIELDS}::")) {
                    // `<field>::Pool.db` - the owner is the struct.
                    Some(field) => (
                        field.rsplit_once('.').map_or(field, |(owner, _)| owner),
                        field,
                    ),
                    // `hold::c`, `Counter::record::hits` - the owner is
                    // everything before the last `::`.
                    None => match key.rsplit_once("::") {
                        Some((owner, _)) => (owner, key.as_str()),
                        None => return None,
                    },
                };
                match self.declared_here.contains(owner) {
                    true => None,
                    false => Some((key.clone(), slot.to_string())),
                }
            })
            .collect();

        for (key, slot) in foreign {
            self.forced.push((
                key,
                format!(
                    "`{slot}` belongs to another file of this package, and that file's own pass \
                     decides its count - this one reads only one of the two"
                ),
                Some(Fallback::ForeignFile),
            ));
        }
    }

    // --- union-find ------------------------------------------------------

    fn id(&mut self, key: &str) -> usize {
        if let Some(found) = self.index.get(key) {
            return *found;
        }
        let fresh = self.parent.len();
        self.parent.push(fresh);
        self.index.insert(key.to_string(), fresh);
        fresh
    }

    fn root(&mut self, mut at: usize) -> usize {
        while self.parent[at] != at {
            let up = self.parent[at];
            self.parent[at] = self.parent[up];
            at = self.parent[at];
        }
        at
    }

    /// Two handles on one allocation, which therefore share one count.
    fn join(&mut self, left: &str, right: &str) {
        let (left, right) = (self.id(left), self.id(right));
        let (left, right) = (self.root(left), self.root(right));
        if left != right {
            self.parent[left] = right;
        }
    }

    // --- collecting ------------------------------------------------------

    fn note(&mut self, function: &str, value: &str, ty: Ty, internal: bool, position: bool) {
        let key = slot(function, value);
        self.id(&key);
        self.handles.entry(key).or_insert(Handle {
            function: function.to_string(),
            value: value.to_string(),
            ty,
            internal,
            position,
        });
    }

    /// This slot's class must be atomic, and this is what forced it.
    ///
    /// **The slot is created if it does not exist yet, and that is not a
    /// detail.** A `<struct>.<field>` slot is forced by the function that
    /// crosses with the struct and joined by the function that builds one, and
    /// nothing says which of the two the walk reaches first. Guarding this on
    /// "the slot is already known" made the answer depend on the order the
    /// functions are written in - which is a fail-*open* bug, the one direction
    /// this analysis may never fail in. Union-find does not care about the
    /// order; only a guard here could.
    ///
    /// **So there is no guard here, and none may be added.** The same applies to
    /// every other spelling of "only if we have seen it": a fail-closed analysis
    /// fails open through a guard added for tidiness, not through a missing
    /// crossing (`docs/history/rc-or-arc.md` §8).
    fn force(&mut self, function: &str, value: &str, why: String, fallback: Option<Fallback>) {
        let key = slot(function, value);
        self.id(&key);
        self.forced.push((key, why, fallback));
    }

    /// A second handle on this slot's allocation is made here
    /// ([ADR-040](../../../../docs/specification/adr/adr-040.md) D1).
    ///
    /// **Only where the handle is handed on by value.** A borrow duplicates
    /// nothing - that is D1's own correction, and what it decides is whether a
    /// function that only *uses* a shared value touches the count. So a
    /// `&Shared[T]` parameter never reaches here.
    ///
    /// Recorded rather than acted on: which count a class gets is decided by where
    /// the value **crosses** and not by how many handles there are, so this
    /// changes no verdict. It is what `--sharing` prints beside the count, because
    /// D1 leaves the place the atomic instruction is paid unwritten in the source
    /// (D5).
    fn duplicates(&mut self, function: &str, value: &str, site: String) {
        let key = slot(function, value);
        self.id(&key);
        self.duplicated.push((key, site));
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
                        published(&key, &name),
                        Some(Fallback::PublicSignature),
                    );
                }
            } else if *is_public {
                // A public function's parameter may *hold* a `Shared` without
                // being one, and the field's class is then as exposed as a
                // parameter is. `send::crossing`'s walk through the ledger's
                // `fields`, run the other way round.
                for field in shared_fields(&ty.text(), self.own, self.library) {
                    self.force(
                        FIELDS,
                        &field,
                        published(&key, &name),
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
                        published(&key, RESULT),
                        Some(Fallback::PublicSignature),
                    );
                }
            } else if *is_public {
                for field in shared_fields(&ty.text(), self.own, self.library) {
                    self.force(
                        FIELDS,
                        &field,
                        published(&key, RESULT),
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
                // **One name only** ([ADR-098](../../../../docs/specification/adr/adr-098.md)).
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
                        // ([ADR-064](../../../../docs/specification/adr/adr-064.md)
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
                                // (ADR-238 D1): `SharedMut(0)` holds an `i64`,
                                // which is what decides whether its lock is a
                                // word. Anything else is left unsaid, and an
                                // unsaid type is never a word.
                                is_hull(text).then(|| match args.as_slice() {
                                    [held] => match literal_type(held) {
                                        Some(held) => Ty::Named {
                                            name: text.to_string(),
                                            args: vec![Ty::named(held)],
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
                            // ([ADR-064](../../../../docs/specification/adr/adr-064.md)
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
            // ([ADR-073](../../../docs/specification/adr/adr-073.md) D3): its
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
    /// ([ADR-064](../../../../docs/specification/adr/adr-064.md) D2), so
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
        match value {
            Expr::LitInt { .. }
            | Expr::LitFloat(_)
            | Expr::LitStr { .. }
            | Expr::LitInterpolated(_)
            | Expr::LitChar(_)
            | Expr::LitBool(_)
            | Expr::LitNull
            | Expr::StructLit { .. } => true,
            // **A hull's own constructor is the allocation**, and the one case
            // where a call handing a hull back is *not* a reason to decline
            // ([ADR-064](../../../../docs/specification/adr/adr-064.md) D2). It is
            // the line the hull is made on, which is the whole question here.
            Expr::Call { func, .. } => match self.path_of(func).as_deref() {
                Some(name) if is_hull(name) => true,
                other => self.hands_back_a_plain_value(other),
            },
            Expr::MethodCall { method, .. } | Expr::SafeMethod { method, .. } => {
                self.hands_back_a_plain_value(Some(self.parsed.text(*method)))
            }
            _ => false,
        }
    }

    /// Whether a ledger says this callee's result is a value and not a handle.
    fn hands_back_a_plain_value(&self, callee: Option<&str>) -> bool {
        let Some(callee) = callee else {
            return false;
        };
        let suffix = format!("::{callee}");
        let contract = [self.own, self.library].into_iter().find_map(|ledger| {
            ledger.functions.get(callee).or_else(|| {
                ledger
                    .functions
                    .iter()
                    .find(|(key, _)| key.ends_with(&suffix))
                    .map(|(_, contract)| contract)
            })
        });
        contract
            .and_then(|contract| contract.signature.as_ref())
            .and_then(|signature| signature.result.as_ref())
            .is_some_and(|result| !result.is_unknown() && !holds_shared(result))
    }

    /// A handle whose allocation this analysis did not watch being made.
    ///
    /// The floor holds and says so. It is [`Fallback::UnseenOrigin`], and it is
    /// also the seed that step 1 made necessary: since ADR-037 D6 the
    /// overlapping analysis may run a call on a thread of its own and hand the
    /// result back (ADR-005 §5.2's `task::both` row), so a `Shared` that came
    /// out of a call is a `Shared` that crosses.
    fn origin_unseen(&mut self, function: &str, name: &str, value: &Expr) {
        let from = match value {
            Expr::Call { func, .. } => match self.path_of(func) {
                Some(path) => format!("`{path}` hands it back"),
                None => "a call hands it back".to_string(),
            },
            Expr::MethodCall { method, .. } | Expr::SafeMethod { method, .. } => {
                format!("`{}` hands it back", self.parsed.text(*method))
            }
            Expr::Field { name, .. } => {
                format!("it is read out of `{}`", self.parsed.text(*name))
            }
            Expr::Index { .. } => "it is read out of a collection".to_string(),
            _ => "it comes from an expression this analysis does not follow".to_string(),
        };
        self.force(
            function,
            name,
            format!(
                "{from}, so this analysis did not watch the allocation being made - and the \
                 count belongs to the allocation. A call handed to another thread hands its \
                 result back across one (ADR-033)"
            ),
            Some(Fallback::UnseenOrigin),
        );
    }

    /// A crossing reached this name: whatever count it owns, or owns through a
    /// field, must be atomic.
    ///
    /// Two ways a crossing lands. The name may *be* a `Shared`, and then its own
    /// class is forced. Or it may be a value of a type that **holds** one, and
    /// then the field's class is - which is `send::crossing`'s transitivity
    /// (ADR-029's walk through the ledger's `fields`) asked the other way round:
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
        for field in shared_fields(&ty.text(), self.own, self.library) {
            self.force(FIELDS, &field, why.to_string(), fallback);
        }
    }

    /// The handle an expression names, where it names one in this function's own
    /// scope.
    fn names_a_handle(&self, expr: &Expr, scope: &BTreeMap<String, Ty>) -> Option<(String, Ty)> {
        match expr {
            Expr::Variable(name) => {
                let name = self.parsed.text(*name).to_string();
                let ty = scope.get(&name)?;
                holds_shared(ty).then(|| (name, ty.clone()))
            }
            Expr::Block(block) => match block.stmts.last().map(|s| &s.node) {
                Some(Stmt::Expr(tail)) => self.names_a_handle(tail, scope),
                _ => None,
            },
            _ => None,
        }
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
                let ty = field_type(&base, &field, self.own, self.library)?;
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
        match expr {
            Expr::Variable(name) => {
                let name = self.parsed.text(*name).to_string();
                Some(scope.get(&name)?.text().trim_start_matches('&').to_string())
            }
            _ => None,
        }
    }

    fn expr(&mut self, function: &str, expr: &Expr, scope: &mut BTreeMap<String, Ty>) {
        match expr {
            // Part II 11.2: a task runs on a thread of its own.
            Expr::Spawn { body, .. } => {
                for name in send::names_used(self.parsed, body) {
                    // ADR-040 D1's second half: a task that uses a handle takes one
                    // of its own, so the name outside the task stays usable.
                    if scope.get(&name).is_some_and(by_value_shared) {
                        self.duplicates(
                            function,
                            &name,
                            "used by a `spawn` body, which takes a handle of its own \
                             (Part II 11.2)"
                                .to_string(),
                        );
                    }
                    self.reached(
                        function,
                        &name,
                        &scope.clone(),
                        "a `spawn` body uses it, and a task runs on a thread of its own \
                         (Part II 11.2)",
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
                            self.held_together.push(slot(function, &handle));
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
            // this analysis may not miss a place it could (ADR-040 D1).
            | Expr::SafeMethod {
                receiver,
                method,
                args,
                config,
            } => {
                let method = self.parsed.text(*method).to_string();
                // **A `get` of a value that does not copy cheaply** (ADR-238
                // D3): it copies the whole of it out each time, where `access`
                // would read it in place. Named where the type says so, and
                // silent where it does not.
                if method == "get"
                    && args.is_empty()
                    && let Some((handle, ty)) = self.names_a_handle(receiver, scope)
                    && let Some(held) = held_that_does_not_copy(&ty)
                {
                    self.copies.push((
                        slot(function, &handle),
                        format!("`{handle}.get()` copies the whole `{held}` out of the lock"),
                    ));
                }
                // **And every walk chained onto one**
                // ([ADR-235](../../../../docs/specification/adr/adr-235.md) D1):
                // `xs.par_iter().map fn …` hands its lambda to `map`, and the
                // lambda is what runs on every core.
                if PARALLEL.contains(&method.as_str()) || self.walks_in_parallel(receiver) {
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
                // A method is resolved by name only (ADR-011 D2), so a method
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
                                field_type(&struct_name, &field_name, self.own, self.library);
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
            // (ADR-032 D3, which the type checker already follows). Measured:
            // `println(f"{hold(c)}")` handed a handle to a function this analysis
            // never saw, so the value kept the plain count while the callee's
            // parameter was decided atomic in the file declaring it - and the two
            // met in one generated file as `Rc` against `Arc`. Any analysis that
            // stops at a literal is one a hole can be hidden in.
            Expr::LitInterpolated(_) => {
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
    /// nothing describes is ADR-038 D7's case: it may start a thread of its own,
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
                    self.duplicates(&at, &name, handed_to(&key, &param));
                    self.join(&source, &slot(&key, &param));
                }
                self.expr(function, arg, scope);
                continue;
            };
            match (&described, callee) {
                (Some((key, params)), _) => match params.get(at) {
                    Some((param, param_ty)) => {
                        let (key, param) = (key.clone(), param.clone());
                        // ADR-040 D1: a handle handed on **by value** is
                        // duplicated; one the callee only borrows is not.
                        if by_value_shared(param_ty) {
                            self.duplicates(function, &handle, handed_to(&key, &param));
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
    /// other resolution in this compiler uses (ADR-011 D2).
    ///
    /// **The type is here for one question only**: whether the position takes
    /// the handle by value or lends the inner value out. A by-value `Shared`
    /// parameter is a second handle (ADR-040 D1); a `&Shared` one is a borrow
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
    fn decide(mut self) -> Sharing {
        self.foreign_slots_hold_the_floor();
        let mut atomic: BTreeMap<usize, (String, Option<Fallback>)> = BTreeMap::new();
        let forced_seen = self.forced.clone();
        for (key, why, fallback) in std::mem::take(&mut self.forced) {
            let id = self.id(&key);
            let root = self.root(id);
            let entry = atomic.entry(root).or_insert((why.clone(), fallback));
            // Of two reasons, print the one that is a crossing rather than an
            // admission of ignorance: ADR-033 D9's rule, that of two true
            // answers the useful one is the one somebody can act on.
            if entry.1.is_some() && fallback.is_none() {
                *entry = (why, fallback);
            }
        }

        // Where the second handles are made, gathered per slot before the answer is
        // read off. A duplication is a fact about a **slot** and not about a class:
        // two handles on one allocation are handed on in different places, and a
        // reader wants the place beside the name they wrote.
        let mut duplicated: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (key, site) in std::mem::take(&mut self.duplicated) {
            let sites = duplicated.entry(key).or_default();
            if !sites.contains(&site) {
                sites.push(site);
            }
        }

        // **Which crossing classes are a word** (ADR-238 D2): every handle a
        // `SharedMut` of a word-sized value, the crossing a proven one and not
        // the floor's, and no door over several locks among them.
        let mut floor: BTreeSet<usize> = BTreeSet::new();
        for (key, _, fallback) in &forced_seen {
            if fallback.is_some() {
                let id = self.id(key);
                floor.insert(self.root(id));
            }
        }
        let mut held: BTreeSet<usize> = BTreeSet::new();
        for key in std::mem::take(&mut self.held_together) {
            let id = self.id(&key);
            held.insert(self.root(id));
        }
        let mut words: BTreeMap<usize, bool> = BTreeMap::new();
        for key in self.handles.keys().cloned().collect::<Vec<_>>() {
            let id = self.id(&key);
            let root = self.root(id);
            let word = holds_a_word(&self.handles[&key].ty);
            let entry = words.entry(root).or_insert(true);
            *entry = *entry && word;
        }

        let mut copied_out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (key, site) in std::mem::take(&mut self.copies) {
            let sites = copied_out.entry(key).or_default();
            if !sites.contains(&site) {
                sites.push(site);
            }
        }

        let keys: Vec<String> = self.handles.keys().cloned().collect();
        let mut decisions = Vec::new();
        let mut counts: BTreeMap<String, Count> = BTreeMap::new();
        let mut classes: BTreeMap<String, BTreeMap<usize, Class>> = BTreeMap::new();
        for key in keys {
            let id = self.id(&key);
            let root = self.root(id);
            let answer = atomic.get(&root).cloned();
            let handle = &self.handles[&key];
            let a_word = words.get(&root) == Some(&true);
            let (count, why, fallback) = match answer {
                Some((why, None)) if a_word && !floor.contains(&root) && !held.contains(&root) => {
                    (Count::Word, Some(why), None)
                }
                Some((why, fallback)) => (Count::Atomic, Some(why), fallback),
                None => (Count::Plain, None, None),
            };
            if handle.position {
                let entry = classes
                    .entry(handle.function.clone())
                    .or_default()
                    .entry(root)
                    .or_insert(Class {
                        members: Vec::new(),
                        count,
                    });
                entry.members.push(handle.value.clone());
                entry.count = entry.count.join(count);
            }
            counts.insert(key.clone(), count);
            if !handle.internal {
                decisions.push(Decision {
                    function: handle.function.clone(),
                    value: handle.value.clone(),
                    ty: handle.ty.text(),
                    count,
                    why,
                    fallback,
                    duplications: duplicated.get(&key).cloned().unwrap_or_default(),
                    copied_out: copied_out.get(&key).cloned().unwrap_or_default(),
                    kept_its_lock: match (count, a_word) {
                        (Count::Atomic, true) if held.contains(&root) => Some(
                            "a word, but a door over several locks takes it, and a \
                             compare-and-swap cannot be held, so it keeps its lock",
                        ),
                        (Count::Atomic, true) => Some(
                            "a word, but the floor answered for it rather than a crossing, \
                             so it keeps its lock",
                        ),
                        _ => None,
                    },
                });
            }
        }

        let summaries = classes
            .into_iter()
            .map(|(function, roots)| {
                let mut found: Vec<Class> = roots
                    .into_values()
                    .map(|mut class| {
                        class.members.sort();
                        class.members.dedup();
                        class
                    })
                    .collect();
                found.sort();
                (function, found)
            })
            .collect();

        Sharing {
            decisions,
            summaries,
            counts,
        }
    }
}

impl Analysis<'_> {
    /// The qualified name a call names, where it names one.
    /// Whether a receiver is a walk that runs in parallel: a `par_iter()`, a
    /// name bound to one, or a walk chained onto either.
    fn walks_in_parallel(&self, receiver: &Expr) -> bool {
        match receiver {
            Expr::Variable(name) => self.parallel_names.contains(self.parsed.text(*name)),
            Expr::MethodCall {
                receiver, method, ..
            } => PARALLEL.contains(&self.parsed.text(*method)) || self.walks_in_parallel(receiver),
            _ => false,
        }
    }

    fn path_of(&self, func: &Expr) -> Option<String> {
        match func {
            Expr::Variable(name) => Some(self.parsed.text(*name).to_string()),
            Expr::Path(segments) => Some(
                segments
                    .iter()
                    .map(|s| self.parsed.text(*s))
                    .collect::<Vec<_>>()
                    .join("::"),
            ),
            _ => None,
        }
    }
}

/// What a `SharedMut` holds, where it is a value that does not copy cheaply:
/// anything that is not a word (ADR-238 D3). A hull whose content nobody named
/// says nothing.
fn held_that_does_not_copy(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Named { name, args, .. } if name == SHARED_MUT => match args.as_slice() {
            [held] if !holds_a_word(ty) && !held.is_unknown() => Some(held.text()),
            _ => None,
        },
        _ => None,
    }
}

/// The type a literal is: a word-sized one, or text.
fn literal_type(expr: &Expr) -> Option<&'static str> {
    match expr {
        Expr::LitStr { .. } | Expr::LitInterpolated(_) => Some("String"),
        Expr::LitInt { .. } => Some("i64"),
        Expr::LitFloat(_) => Some("f64"),
        Expr::LitBool(_) => Some("bool"),
        Expr::LitChar(_) => Some("char"),
        Expr::Unary { expr, .. } => literal_type(expr),
        _ => None,
    }
}

/// **A `SharedMut` of a value that fits a machine word** (ADR-238 D1): a
/// number, a truth value or a character.
fn holds_a_word(ty: &Ty) -> bool {
    matches!(ty, Ty::Named { name, args, view: false }
        if name == SHARED_MUT
            && matches!(args.as_slice(), [Ty::Named { name: held, args: none, view: false }]
                if none.is_empty()
                    && matches!(held.as_str(), "i8" | "i16" | "i32" | "i64" | "u8" | "u16"
                        | "u32" | "u64" | "f32" | "f64" | "bool" | "char")))
}

/// The sentence a published position gets.
fn published(key: &str, at: &str) -> String {
    match at == RESULT {
        true => format!(
            "`{key}` is public and hands a `Shared` back, so what the caller does with it is in \
             a unit this build cannot see"
        ),
        false => format!(
            "`{key}` is public, so its callers are in a unit this build cannot see, and one of \
             them may cross a thread with it"
        ),
    }
}

/// The sentence a duplication at a call gets.
///
/// No line number: an `Expr` carries no span in this AST, which is the same limit
/// `check::Checked::fallible_methods` records about a method call. The callee and
/// the position it takes the handle in are what there is to say, and they are
/// enough to find the line.
fn handed_to(key: &str, param: &str) -> String {
    format!("handed to `{key}` as `{param}`, which takes a handle of its own")
}

/// The sentence a call nothing describes gets.
fn unseen(callee: Option<&str>) -> String {
    format!(
        "nothing written down describes `{}`, so this compiler cannot see the end of it - and \
         starting a thread of its own is among the things it may do (ADR-038 D7)",
        callee.unwrap_or("the callee")
    )
}

/// `"Counter::record"` back into its two halves.
fn split_slot(key: &str) -> (String, String) {
    match key.rsplit_once("::") {
        Some((function, name)) => (function.to_string(), name.to_string()),
        None => (String::new(), key.to_string()),
    }
}

/// Whether a position takes a handle on a shared value **by value**.
///
/// `Shared[T]` does; `&Shared[T]` does not, and the difference is
/// [ADR-040](../../../../docs/specification/adr/adr-040.md) D1's correction:
/// lending the inner value out hands no handle on, so nothing is duplicated and
/// no atomic instruction is paid. A type that merely *holds* a `Shared` does not
/// either - a struct is carried by whatever holds it, which is D1's own scope
/// note.
fn by_value_shared(ty: &Ty) -> bool {
    matches!(ty, Ty::Named { name, view: false, .. } if name == SHARED || name == SHARED_MUT)
}

/// The three types a program makes a hull of by calling their name
/// ([ADR-064](../../../../docs/specification/adr/adr-064.md) D2).
fn is_hull(name: &str) -> bool {
    matches!(name, SHARED | SHARED_MUT | "Locked")
}

/// Whether a type is, or holds, a `Shared`.
fn holds_shared(ty: &Ty) -> bool {
    match ty {
        Ty::Named { name, args, .. } => {
            name == SHARED || name == SHARED_MUT || args.iter().any(holds_shared)
        }
        Ty::Tuple(parts) => parts.iter().any(holds_shared),
        _ => false,
    }
}

/// What the ledgers say about a type, by the name a program writes.
fn described<'a>(
    ty: &str,
    own: &'a Ledger,
    library: &'a Ledger,
) -> Option<&'a super::TypeContract> {
    let name = ty.trim_start_matches('&');
    let suffix = format!("::{name}");
    [own, library].into_iter().find_map(|ledger| {
        ledger.types.get(name).or_else(|| {
            ledger
                .types
                .iter()
                .find(|(key, _)| key.ends_with(&suffix))
                .map(|(_, contract)| contract)
        })
    })
}

/// The `<struct>.<field>` slots a type reaches that hold a `Shared`, through the
/// ledger's `fields` (ADR-024, the walk ADR-029 established).
fn shared_fields(ty: &str, own: &Ledger, library: &Ledger) -> Vec<String> {
    let name = ty.trim_start_matches('&');
    let Some(contract) = described(ty, own, library) else {
        return Vec::new();
    };
    contract
        .fields
        .iter()
        .filter(|field| holds_shared(&field.ty))
        .map(|field| format!("{name}.{}", field.name))
        .collect()
}

/// One field's declared type, through the same lookup.
fn field_type(ty: &str, field: &str, own: &Ledger, library: &Ledger) -> Option<Ty> {
    described(ty, own, library)?
        .fields
        .iter()
        .find(|declared| declared.name == field)
        .map(|declared| declared.ty.clone())
}

/// What `Shared[T]` lowers to, which since ADR-037 D6 is one type with one
/// optimisation on top of it.
///
/// D6 decides that the floor is `std::sync::Arc`; D7 decides that a value this
/// analysis proves never crosses may be `std::rc::Rc` instead.
///
/// Named here rather than in `emit` because the answer is **per value**: the
/// emitter asks it of the count this file gave a particular position, which is
/// why `emit::Emitter::map_name` could not have answered it. The full path and no
/// `use`, because the emitted file writes every `std` type that way and two names
/// as common as these are two a program may have of its own.
pub fn rust_name(count: Count) -> &'static str {
    match count {
        Count::Plain => "std::rc::Rc",
        Count::Word | Count::Atomic => "std::sync::Arc",
    }
}

/// What `Locked[T]` lowers to, which is the **same answer to the same question**
/// ([ADR-057](../../../../docs/specification/adr/adr-057.md) D3).
///
/// A lock is only reachable from two places through a shared handle, so the count
/// this file gave that handle decides the lock inside it: one analysis, two
/// decisions, no second pass. Every one of [`Fallback`]'s reasons to decline
/// carries over unchanged, and so does the polarity — where nothing is proved,
/// the shape that is safe.
///
/// The two shapes report the same thing when a program re-enters one lock (D4),
/// which is what makes choosing between them invisible to a reader. That is
/// `nikaia_std::lock`'s business and the reason the crossing one carries an owner
/// check.
pub fn lock_name(count: Count) -> &'static str {
    match count {
        Count::Plain => "nikaia_std::lock::Local",
        Count::Word => "nikaia_std::lock::Word",
        Count::Atomic => "nikaia_std::lock::Crossing",
    }
}
