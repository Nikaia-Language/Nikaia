// crates/nikaia/src/contracts/send.rs
//
// Whether a value may cross a thread (ADR-005 §1 Group B, `NK25xx`).
//
// One question, asked of a **type and a destination** (ADR-045 D1):
//
//     may a value of this type go to *this* destination?
//
// ADR-038 D7 states the first half as "a value may only cross into a foreign
// thread if it may cross any thread", and the reason the answer is a property of
// the type rather than of the particular crossing is ADR-005 Group B: it has to
// be **the same at both settings of `user_parallelism`**, so that a library
// written at one setting cannot turn out to be un-compilable where it is used. A
// check that consulted the switch would answer `yes` for a type whose expansion
// is safe at one setting and not at the other, and `no` for the same type at the
// other - which is exactly the asymmetry Group B, `NK25xx` and ADR-037 §3 were
// all written to prevent. So **no switch reaches this file, and none may** - the
// same sentence `order.rs` opens with, for the same reason.
//
// ## Why the destination is not the switch coming back in
//
// [`Destination`] has two values and **each gets an answer that is the same at
// both settings**, which is the whole of what Group B asks - its own note says
// "'At every setting' is a claim about the verdict, not about the severity". The
// justification may differ per setting; the answer may not. Before ADR-045 the
// lock had to take the worse of the two settings and answer `MayNot`
// everywhere, which made Part II 12.2's counter - the program
// `user_parallelism = yes` exists to serve - impossible to write: at `yes` a real
// operating-system lock stands in the emitted program, under which that counter
// is entirely safe, and it was refused on account of an implementation that does
// not occur in it.
//
// So the destination is the axis the verdict was missing, and not a reading of
// the switch. Nothing below asks what `user_parallelism` is.
//
// ## What ADR-037 D6 changed here, and what it did not
//
// `Shared` was that type, and it was the file's only `MayNot`. D6 removed the
// cause rather than the symptom: `Shared` expands to an atomic count at **both**
// settings, so its expansion no longer moves with the switch and there is
// nothing for the verdict to take the worse of. `Shared` therefore joins
// `CONTAINERS` and is answered by what it holds.
//
// **The rule above is untouched.** The verdict still never reads
// `user_parallelism`, and it is still a property of the type. What changed is
// one row of a table.
//
// **And one row came back** ([ADR-061](../../../../docs/specification/adr/adr-061.md)
// D1): ADR-037 D7's per-value inference made the count's shape a property of
// the *value* again, so there is no one representation for a foreign signature
// to name, and a `Shared` may not go into code nothing describes after all.
// That row was written into [`CHOSEN`] and never reached, because `Shared` is
// in [`CONTAINERS`] too and the container row is answered first. It is reached
// now, after the contents have had their say - see the comment at that fork,
// which is where the order is argued.
//
// ## Three answers and not two
//
// [`Crossing`] has a third arm, and it is the whole design. `Undecided` is not
// `May`: ADR-010 D1 says an analysis that fails open is a vulnerability
// generator, and "nothing is written down about this type" is the absence of an
// answer rather than permission. But it is not a refusal either, because the
// one thing this compiler may never do is reject a program that is correct
// (Part III C.4), and Stage 0 knows the type of rather less than half of what a
// program writes.
//
// What the two non-`May` answers cost therefore depends on **who chose the
// crossing**, and the split is the reason the polarity and the no-false-refusal
// promise can both be kept:
//
//   * A crossing **the compiler chose** - the `task::both` that statement
//     overlapping emits (ADR-033) - is a step the compiler was never obliged to
//     take. Anything but `May` means it does not take it: the statements keep
//     the order they were written in. Nothing is refused and only speed is
//     spent, which is `order.rs`'s own polarity ("every `false` is either a real
//     dependency or an admission of ignorance, and the two are deliberately
//     worth the same").
//   * A crossing **the program wrote** - `spawn`, a value handed to a call this
//     compiler cannot see the end of - is refused on `MayNot` and left to the
//     backend on `Undecided`. A refusal on `Undecided` would reject correct
//     programs; silence is not what it buys instead, because `rustc` still
//     type-checks the emitted crate (ADR-002 §3) and ADR-005 D7's `E0277`
//     translation now reports that refusal against the `.nika` line. So an
//     undecided crossing is never *accepted* here - it is handed on.
//
// ## What the table knows
//
// A closed list, like `touch::KINDS` and for the same reason: a name this file
// does not know is answered `Undecided`, and a name it knows wrongly would be a
// typo that bought a crossing. The list grows when a type needs it, never
// speculatively (ADR-028 D5).
//
// Every name in [`PLAIN`] and [`CONTAINERS`] is safe both to **move** to
// another thread and to be **looked at** from one, which is why a view (`&T`)
// asks the same question as the value. The day a name belongs in one column and
// not the other, this needs a second column; stating that here is cheaper than
// discovering it.
//
// **[`CHOSEN`] is that day, and the column it needed turned out to be the
// destination** (ADR-045 §4 predicted this file would want two columns here). A
// lock at `user_parallelism = no` may be moved to another thread and may not be
// looked at from one, so on the move/look axis it does belong in one column and
// not the other. It never has to be asked that way, because both destinations
// answer before the distinction matters: into our own code the lock may go
// whichever of the two it is doing, and into code nothing describes it may do
// neither. So the two columns stay one, and the table splits by destination
// instead.

use std::collections::BTreeSet;

use crate::ast::Expr;
use crate::parser::Parsed;

use super::{Ledger, ty::Ty};

/// Plain data: a value of one of these carries nothing that a second thread
/// could be wrong about.
const PLAIN: &[&str] = &[
    "bool", "char", "str", "String", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16",
    "u32", "u64", "u128", "usize", "f32", "f64",
];

/// A container is whatever it holds: it may cross exactly when every one of its
/// arguments may.
///
/// **`Shared` is in this list since
/// [ADR-037](../../../../docs/specification/adr/adr-037.md) D6**, and that is the
/// whole of step 1. It used to be the one type the records made `MayNot`,
/// because D3 expanded it from `user_parallelism` - a plain count at `no`, an
/// atomic one at `yes` - and a verdict that may not consult the switch has to
/// take the worse of the two settings for a type whose expansion moves with it.
/// D6 stopped the expansion moving, which is what let it into this list. ADR-061
/// D2 lets the expansion move again at `no` - every count is plain there - and
/// the row stays, because what it moved to is the setting where **nothing
/// crosses**: one thread of the user's, the runtime's own threads carrying no
/// code they wrote, and D1 refusing a `Shared` to code nothing describes. So the
/// verdict is answered by what the `Shared` holds at both settings, and at each
/// of them the count underneath is one that suffices for what can actually
/// happen there.
///
/// Which is also why the single-column invariant in the module header matters
/// here rather than being a note for later. An atomic count may cross only
/// where the value under it may both **move** to another thread and be **looked
/// at** from one - `Arc<T>` in the language below - and that is exactly the
/// property every name in [`PLAIN`] and this list is required to have.
/// **The two ends of a channel are in this list since
/// [ADR-149](../../../../docs/specification/adr/adr-149.md) D4**, and that is
/// where that decision is built: *the value type must cross, checked where `tx`
/// moves into a `spawn`*. Being a container is the whole of it — the crossing
/// analysis already asks the question at a move, so nothing about channels had
/// to be written anywhere else.
///
/// They meet the invariant above: what is under either end is an `Arc` over a
/// lock, which may both move to another thread and be looked at from one
/// exactly when the value it carries may.
const CONTAINERS: &[&str] = &[
    "BTreeMap", "BTreeSet", "HashMap", "HashSet", "List", "Option", "Receiver", "Result", "Sender",
    "Shared", "Vec",
];

/// A lock: the one family whose answer depends on where the value is going
/// ([ADR-045](../../../../docs/specification/adr/adr-045.md) D2, D3).
///
/// Both names, because the question is about the lock in either: `SharedMut[T]`
/// is a count around a lock and `Locked[T]` is the lock on its own, for a field.
/// They are **not two spellings of one type** - that was refused by
/// [ADR-064](../../../../docs/specification/adr/adr-064.md) D3 - but they answer
/// alike here, because what this asks about is what they have in common. The
/// count plays no part: which one a value gets follows this answer rather than
/// making it ([ADR-037](../../../../docs/specification/adr/adr-037.md) D7).
///
/// Into our own code a lock is answered **by what it holds**, like a container:
/// a lock does not make its contents crossable, and Group B's transitivity is
/// the whole rule. Into code nothing describes it is refused.
///
/// **Specified ahead of the compiler**, like everything else about these two
/// types: neither is a type the backend can lower
/// ([ADR-039](../../../../docs/specification/adr/adr-039.md) §4), so a program
/// that writes one as an annotation reaches this verdict and then fails to emit.
/// The verdict is still the thing worth having early - it is what a library's
/// signature is written against.
/// **And `Shared` joined them**
/// ([ADR-061](../../../../docs/specification/adr/adr-061.md) D1), for the
/// sentence that was already the lock's reason: *a Rust library has one
/// signature*. When [ADR-037](../../../../docs/specification/adr/adr-037.md) D6
/// made the count atomic at both settings there was one representation to write
/// down and nothing to refuse; D7's per-value inference brought the second one
/// back, so a `Shared[Conn]` is an `Rc` for one value and an `Arc` for another
/// **in the same program** - and no foreign signature can name both.
const CHOSEN: &[&str] = &["Locked", "SharedMut", "Shared"];

/// Where a value is going. The second half of the question this file answers
/// ([ADR-045](../../../../docs/specification/adr/adr-045.md) D1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// Our own code on a thread of our own: the body of a `spawn`ed task
    /// (Part II, 11.2), and the closure statement overlapping builds
    /// ([ADR-033](../../../../docs/specification/adr/adr-033.md)).
    ///
    /// One answer, two reasons, which is what makes it switch-independent
    /// (D2): at `user_parallelism = yes` a real operating-system lock is
    /// underneath and several threads are what it is for; at `no` the task is
    /// interleaved on the same thread, so nothing crosses at all.
    Ours,
    /// Code nothing written down describes, which may do anything with what it
    /// is given - including putting it on a thread of its own
    /// ([ADR-038](../../../../docs/specification/adr/adr-038.md) D7).
    Foreign,
}

/// How far into a type this walks before giving up. A struct that holds itself
/// through a container is a shape the ledger can hold, and this must terminate
/// on one.
const DEPTH: usize = 16;

/// **Why** a value may not cross — the word [`Crossing::MayNot`]'s own note
/// asked for the day the arm had a second producer.
///
/// That note said it plainly: *"the day there is a second the arm has to carry
/// **which** — a refusal whose sentence is about a lock would be wrong about
/// anything else"*. There are three producers now, they print three different
/// sentences, and one of them —
/// [`Refusal::Lock`](Refusal::Lock) — is the one
/// [ADR-039](../../../../docs/specification/adr/adr-039.md) D6 gives its own
/// code to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A lock: `SharedMut[T]` or `Locked[T]` at a foreign destination
    /// ([ADR-045](../../../../docs/specification/adr/adr-045.md) D3). The one
    /// `NK2503` is about, and the only one it is about.
    Lock,
    /// A shared count: a `Shared[T]` at a foreign destination holding nothing a
    /// lock refuses first
    /// ([ADR-061](../../../../docs/specification/adr/adr-061.md) D1).
    ///
    /// The lock's own reason and not a lock: which count a value gets is chosen
    /// per value ([ADR-037](../../../../docs/specification/adr/adr-037.md) D7),
    /// so a `Shared[Conn]` is one representation for one value and another for
    /// the next **in the same program**, and no signature outside this language
    /// can name both.
    Count,
    /// A described type that says outright that it may not cross
    /// ([ADR-123](../../../../docs/specification/adr/adr-123.md) D1). Nothing
    /// about locks or counts is claimed: a ledger said so in one line, and the
    /// sentence a reader gets has to say that rather than invent a reason.
    Said,
}

/// What crossing a thread would mean for a value of one type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Crossing {
    /// Every part of it may cross, and this compiler can see every part.
    May,
    /// A part of it may not, and the records say which.
    ///
    /// **Its one producer is a lock at a foreign destination**
    /// ([ADR-045](../../../../docs/specification/adr/adr-045.md) D3). `Shared`
    /// used to be the producer and stopped being one when
    /// [ADR-037](../../../../docs/specification/adr/adr-037.md) D6 gave it one
    /// representation at both settings; the arm then stood empty, and the lock is
    /// the occupant ADR-037 D6's own coda named as the candidate.
    ///
    /// So `NK2502` fires on a program this language can write and `NK2501` no
    /// longer does: the two codes stopped sharing one verdict the day the
    /// destination entered it.
    ///
    /// **And the day there was a second producer arrived**, which is what
    /// [`Refusal`] is: the arm carries *which*, because
    /// [`Crossing::note`] and [`Crossing::way_out`] were written for a lock and
    /// would be wrong about anything else. Three producers, three sentences,
    /// and `NK2503` reads the word rather than guessing from the type's name.
    MayNot {
        /// The part the answer is about - the whole type where the type itself
        /// is the problem, and the field's type where a field is.
        part: String,
        /// The **field** it was found in, where it was found in one: `None` for
        /// the value itself and `Some("counts")` for a field.
        ///
        /// The bare name and not a sentence, because two readers want it: the
        /// note prints prose around it, and `NK2503`'s way out writes a path a
        /// program can paste (`state.counts.get()`).
        at: Option<String>,
        /// Why - the word that decides the sentence and the code.
        why: Refusal,
    },
    /// Nothing written down decides it. **Not permission** (ADR-010 D1).
    Undecided {
        /// The part nothing is written down about.
        part: String,
    },
}

impl Crossing {
    /// Whether a value of the type may cross - which is true of [`Crossing::May`]
    /// and of nothing else.
    pub fn may(&self) -> bool {
        matches!(self, Crossing::May)
    }

    /// The refusal, where the answer is one.
    pub fn refused(&self) -> Option<(&str, Option<&str>)> {
        match self {
            Crossing::MayNot { part, at, .. } => Some((part, at.as_deref())),
            _ => None,
        }
    }

    /// **Why** it is refused, where it is - the word `NK2503` reads to know
    /// whether the refusal is its own ([ADR-039](../../../../docs/specification/adr/adr-039.md)
    /// D6).
    pub fn why(&self) -> Option<Refusal> {
        match self {
            Crossing::MayNot { why, .. } => Some(*why),
            _ => None,
        }
    }

    /// One concrete way out, as Part III C.2 requires of every diagnostic.
    /// `None` where there is nothing to get out of.
    ///
    /// The remedy is the one ADR-045 D3 names, and it is not "don't do that": the
    /// caller opens the lock and hands the **value inside it** over, so the called
    /// code sees an ordinary number or connection and no lock at all. That is the
    /// shape already settled for an ordinary function
    /// ([ADR-042](../../../../docs/specification/adr/adr-042.md) D1, D2,
    /// Part I 6.2), so nothing new has to be learned to follow it.
    pub fn way_out(&self) -> Option<String> {
        Some(match self.why()? {
            Refusal::Lock => {
                "Open the lock here and pass the value inside it, so the called code gets an \
                 ordinary value and no lock."
                    .to_string()
            }
            // ADR-061 D1's own words: *the way out is to pass what is inside*,
            // a view or a copy. A library that means to keep what it is given
            // puts it in a hull of its own anyway.
            Refusal::Count => {
                "Pass what the shared value holds, a view or a copy, instead of the shared \
                 value itself."
                    .to_string()
            }
            // Nothing is known about *why* the ledger said no, so the way out
            // may not pretend to be a repair: what a reader can always do is
            // keep the value here and hand over what the call actually needs.
            Refusal::Said => {
                "Keep it here, and pass the call only what it needs from it.".to_string()
            }
        })
    }

    /// One line saying why a value of this type may not cross, for a
    /// diagnostic's note. `None` where it may.
    ///
    /// Part III C.2: no Rust vocabulary, and the sentence has to mean something
    /// to a reader who has never heard of a reference count either.
    ///
    /// **The `MayNot` sentence says that the answer was chosen**, because ADR-045
    /// D3 asks it to: at `user_parallelism = yes` the crossing would in fact be
    /// safe, and the cautious answer is taken anyway so that a library written at
    /// one setting stays usable at the other. Anyone who trips over this in two
    /// years should be able to see from the message that it was a decision and
    /// not an oversight.
    pub fn note(&self) -> Option<String> {
        match self {
            Crossing::May => None,
            Crossing::MayNot {
                part,
                at,
                why: Refusal::Lock,
            } => Some(format!(
                "`{part}`{} holds a lock, and a lock can't be passed to code the compiler knows \
                 nothing about.",
                held(at)
            )),
            // ADR-061 D1. The sentence is the lock's reason without the lock:
            // one representation cannot be named from outside, because there
            // is not one.
            Crossing::MayNot {
                part,
                at,
                why: Refusal::Count,
            } => Some(format!(
                "`{part}`{} is shared, and how a shared value is counted is decided per value, \
                 so code outside Nikaia can't be written to handle it.",
                held(at)
            )),
            // ADR-123 D1: a line in a ledger, repeated rather than interpreted.
            Crossing::MayNot {
                part,
                at,
                why: Refusal::Said,
            } => Some(format!(
                "`{part}`{} is described as a value that can't move to another thread.",
                held(at)
            )),
            Crossing::Undecided { part } => Some(format!(
                "Nothing says whether `{part}` can move to another thread, so the compiler \
                 assumes it can't."
            )),
        }
    }
}

/// Every name a task's body mentions.
///
/// The ordering analysis's walk, asked a second question: it is **total and
/// over-approximate on purpose** (a field access `a.b` contributes `a`, a
/// method name counts as a name, the words of a `dsl` body count), and both
/// properties are what this needs. Over-approximate is the safe direction here
/// too - a name that is not a variable is not in scope, so it is looked up,
/// not found, and says nothing.
pub fn names_used(parsed: &Parsed, body: &Expr) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    super::order::names_in(parsed, body, &mut out);
    out
}

/// The same, for a **statement** — every name any expression in it mentions.
///
/// `names_in` walks an expression; a statement holds several, and the blocks a
/// statement holds are walked too (`tools/names.nika`, #125). Over-approximate in the direction that costs
/// nothing, exactly as [`names_used`] is: a function name and a field name are
/// in the answer, and a caller that looks a name up finds nothing for them.
pub fn names_used_in_stmt(parsed: &Parsed, stmt: &crate::ast::Stmt) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    nikaia_std::tools::names::names_in_statement(stmt, &parsed.interner, &mut out);
    out
}

/// Which of a task body's **own** bindings are held across a pause
/// ([ADR-055](../../../../docs/specification/adr/adr-055.md) §2 D6).
///
/// A task's body is an `async` block below, and a value bound inside it and
/// still live at a suspension point further down is held **inside the future**.
/// The pool's starter asks for `Send` of that whole future, so such a value
/// crosses a thread exactly as a captured one does — which is why the same
/// verdict is asked of it.
///
/// Everything is a byte position and the rule is one line: **bound before a
/// pause, and named after it.** `named` is the *last* place each name was
/// mentioned, so a name read once, before the pause, is not held.
///
/// **Live is over-approximated** and Rust's own answer is narrower — a value
/// whose last use is inside a branch the pause is not in, say. That would be a
/// *correct program refused* if a refusal stood on this alone. It does not:
/// what refuses is [`crossing`]'s `MayNot`, which is a claim about a **type**
/// and never `Undecided`'s silence, so a name this is too generous about is
/// refused only where a value of its type could not have crossed from
/// anywhere.
pub fn held_across_a_pause(
    bound: &[(String, usize)],
    pauses: &[usize],
    named: &std::collections::BTreeMap<String, usize>,
) -> BTreeSet<String> {
    bound
        .iter()
        .filter(|(name, at)| {
            pauses
                .iter()
                .any(|pause| pause > at && named.get(name).is_some_and(|last| last > pause))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Whether a value of `ty` may go to `into`.
///
/// `own` is the program's ledger and `library` is `std`'s: between them they
/// hold the fields of every type that is written down (ADR-024), which is what
/// makes this **structural and transitive** as Group B requires - a struct with
/// one lock field is no more crossable than the lock itself.
///
/// `into` is the destination (ADR-045 D1) and reaches exactly one row of the
/// table, [`CHOSEN`]. Every other answer is the same wherever the value is going,
/// which is why the parameter is threaded through the walk rather than asked
/// before it: a lock four fields deep is still a lock.
pub fn crossing(ty: &Ty, own: &Ledger, library: &Ledger, into: Destination) -> Crossing {
    let mut seen = BTreeSet::new();
    walk(ty, own, library, into, &mut seen, DEPTH)
}

fn walk(
    ty: &Ty,
    own: &Ledger,
    library: &Ledger,
    into: Destination,
    seen: &mut BTreeSet<String>,
    depth: usize,
) -> Crossing {
    if depth == 0 {
        return Crossing::Undecided { part: ty.text() };
    }
    match ty {
        // The absence of a claim, and a claim is what this would need.
        Ty::Unknown => Crossing::Undecided {
            part: "?".to_string(),
        },
        // **A count crosses** ([ADR-152](../../../../docs/specification/adr/adr-152.md)
        // D1): it is a number the compiler knows, it is part of the *type* and
        // not part of the value, and there is nothing of it at run time to go
        // anywhere. An `Array[T, N]` therefore crosses exactly when its `T`
        // does, which is D2's *it copies as its elements do* read one layer up.
        Ty::Count(_) => Crossing::May,
        // **What the C boundary lends does not cross**
        // ([ADR-147](../../../../docs/specification/adr/adr-147.md) D1): it
        // lives for the call, and a thread outlives one. Undecided rather than
        // refused, which is this file's polarity: what it names is the part it
        // cannot answer for, and a boundary type is the caller's own for
        // exactly one call.
        Ty::Pointed { .. } => Crossing::Undecided { part: ty.text() },
        // A library signature's variable that nothing bound. `substitute` turns
        // a bound one into the type it bound to before this is ever asked, so
        // one that arrives here is the absence of an answer (ADR-031).
        Ty::Var { .. } => Crossing::Undecided { part: ty.text() },
        // A lambda is its captures, and nothing writes those down.
        Ty::Fn { .. } => Crossing::Undecided { part: ty.text() },
        // **A produced sequence is the thing it is walking**
        // ([ADR-105](../../../../docs/specification/adr/adr-105.md) D1), and
        // nothing says what that is: `keys()` holds a borrow of the map,
        // `chars()` one of the string, and a `map` holds the lambda's captures
        // beside whichever of those it was built on. So the answer is the
        // absence of one, which is not permission (ADR-010 D1) - the same
        // silence a lambda gets one line up, and for the same reason.
        //
        // `Par[T]` is not louder: what `par_iter` hands back crosses by
        // construction, but the type says nothing about it and a claim nobody
        // wrote is not one to read.
        Ty::Seq { .. } => Crossing::Undecided { part: ty.text() },
        // **A `T?` crosses exactly as its `T` does**, because that is what it
        // lowers to: an `Option<T>` is `Send` when `T` is, and it holds one `T`
        // or nothing. Neither answer is changed by the emptiness.
        Ty::Nullable(inner) => walk(inner, own, library, into, seen, depth - 1),
        // `()` holds nothing, so there is nothing to be wrong about; a pair is
        // its parts.
        Ty::Tuple(parts) => join(
            parts
                .iter()
                .map(|p| walk(p, own, library, into, seen, depth - 1)),
        ),
        Ty::Named { name, args, .. } => {
            if PLAIN.contains(&super::ty::base(name)) {
                // A plain type with arguments is not the plain type: `String[T]`
                // is a name this compiler does not know.
                return match args.is_empty() {
                    true => Crossing::May,
                    false => Crossing::Undecided { part: ty.text() },
                };
            }
            if CONTAINERS.contains(&super::ty::base(name)) {
                // A container with no arguments said nothing about what it
                // holds, and what it holds is the whole question.
                let inside = match args.is_empty() {
                    true => Crossing::Undecided { part: ty.text() },
                    false => join(
                        args.iter()
                            .map(|a| walk(a, own, library, into, seen, depth - 1)),
                    ),
                };
                // **`Shared` is in both lists, and the container row answered
                // first** ([ADR-061](../../../../docs/specification/adr/adr-061.md)
                // D1). Its row in [`CHOSEN`] was therefore unreachable: a
                // `Shared[String]` into code nothing describes came back `May`,
                // which is the opposite of what D1 decided. Measured, not read:
                // `only_a_lock_is_refused_at_either_destination` asserted the
                // wrong half of it and passed.
                //
                // The contents answer **first** all the same, because where
                // there is a lock inside, the lock is the better sentence and
                // the one with a code of its own. D1 is what is left when
                // nothing inside refuses.
                if matches!(into, Destination::Foreign)
                    && CHOSEN.contains(&name.as_str())
                    && inside.refused().is_none()
                {
                    return Crossing::MayNot {
                        part: ty.text(),
                        at: None,
                        why: Refusal::Count,
                    };
                }
                return inside;
            }
            // **The one row the destination reaches** (ADR-045 D2, D3). Into our
            // own code a lock is a container: it may go where what it holds may
            // go, and it makes nothing crossable that was not. Into code nothing
            // describes it may not go at all - the conservative answer, taken at
            // both settings on purpose, and the sentence in `note` says so.
            if CHOSEN.contains(&name.as_str()) {
                return match (into, args.is_empty()) {
                    (Destination::Foreign, _) => Crossing::MayNot {
                        part: ty.text(),
                        at: None,
                        why: Refusal::Lock,
                    },
                    // A lock with no arguments said nothing about what it holds,
                    // exactly as a bare container does.
                    (Destination::Ours, true) => Crossing::Undecided { part: ty.text() },
                    (Destination::Ours, false) => join(
                        args.iter()
                            .map(|a| walk(a, own, library, into, seen, depth - 1)),
                    ),
                };
            }
            let Some(contract) = described(name, own, library) else {
                return Crossing::Undecided { part: ty.text() };
            };
            // A type whose parts are Rust says so in one line, exactly as a
            // Rust function says `sync = true` about a body this compiler does
            // not read. The claim is about the type and not about its
            // arguments, which is why a `crosses` type with arguments is not
            // read further: nothing in `std` has one, and the day something
            // does, it is the arguments that want a rule.
            //
            // **And `crosses = false` is the row the destination's refusals
            // were built for** ([ADR-123](../../../../docs/specification/adr/adr-123.md)
            // D1): before it there was no way for a described type to answer
            // *may not*, so `NK2501` and `NK2502` had nothing to fire on.
            //
            // It answers with **or without** arguments, and the asymmetry with
            // `May` above is the polarity and not an oversight: a promise is
            // withheld where the claim might not reach (a `T` the line did not
            // speak about), and a restriction is kept where it might
            // (ADR-010 D1). Nothing a value of such a type is wrapped in makes
            // it crossable.
            if contract.crosses.may_not() {
                return Crossing::MayNot {
                    part: ty.text(),
                    at: None,
                    why: Refusal::Said,
                };
            }
            if contract.crosses.may() && args.is_empty() {
                return Crossing::May;
            }
            Some(contract.fields.as_slice())
                .filter(|fields| !fields.is_empty())
                .map(|fields| {
                    // A type that holds itself is answered by its other fields:
                    // the cycle adds no new part to be wrong about, and walking
                    // it again would not terminate.
                    if !seen.insert(name.clone()) {
                        return Crossing::May;
                    }
                    let answer = join(fields.iter().map(|field| {
                        name_the_field(
                            &field.name,
                            walk(&field.ty, own, library, into, seen, depth - 1),
                        )
                    }));
                    seen.remove(name);
                    answer
                })
                .unwrap_or_else(|| Crossing::Undecided { part: ty.text() })
        }
    }
}

/// What the ledgers say about a type, by the name a program writes.
///
/// The program's own ledger first, then `std`'s - the order every other
/// resolution in this compiler uses. A library writes the module in front of a
/// type (`fs::Mapped`) and a signature does not carry one, so the suffix is what
/// matches: name-for-name resolution (ADR-011 D2), as everywhere else.
///
/// **A type with no fields recorded is not a type with no fields.** `fs::Mapped`
/// has an entry and an empty `fields`, because its fields are Rust. So an empty
/// list is answered as "nothing is written down" rather than as "it holds
/// nothing", and `crosses` is the line that answers for such a type.
fn described<'a>(
    name: &str,
    own: &'a Ledger,
    library: &'a Ledger,
) -> Option<&'a crate::contracts::TypeContract> {
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

/// Say which field an answer came from, where it is not `May`.
///
/// The **innermost** one, because a field already named is not renamed on the
/// way out: `a.b.c` where the lock is `c`'s reports `c`, which is the field a
/// reader can look at.
fn name_the_field(field: &str, answer: Crossing) -> Crossing {
    match answer {
        Crossing::MayNot {
            part,
            at: None,
            why,
        } => Crossing::MayNot {
            part,
            at: Some(field.to_string()),
            why,
        },
        other => other,
    }
}

/// The clause a note puts between the part and its verb, for a field that has
/// a name and for one that is the value itself.
fn held(at: &Option<String>) -> String {
    match at {
        Some(field) => format!(", which its field `{field}` holds,"),
        None => String::new(),
    }
}

/// The worst answer of several, with `MayNot` worse than `Undecided`.
///
/// A refusal a reader can act on beats an admission of ignorance, which is
/// ADR-033 D9's rule for the ordering report applied to the same kind of choice:
/// of two true answers, print the one somebody can do something about.
fn join(answers: impl Iterator<Item = Crossing>) -> Crossing {
    let mut worst = Crossing::May;
    for answer in answers {
        match (&worst, &answer) {
            (Crossing::MayNot { .. }, _) => return worst,
            (_, Crossing::MayNot { .. }) => worst = answer,
            (Crossing::May, Crossing::Undecided { .. }) => worst = answer,
            _ => {}
        }
    }
    worst
}

#[cfg(test)]
fn field(name: &str, ty: super::ty::Ty) -> super::FieldContract {
    super::FieldContract {
        name: name.to_string(),
        ty,
        public: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::ty::TyOps;
    use crate::contracts::{LedgerOps, TypeContract};

    fn ledgers() -> (Ledger, Ledger) {
        (
            Ledger::empty(),
            Ledger::parse(super::super::STD).expect("std ships a ledger"),
        )
    }

    /// Asked about our own code, which is the destination most of these are
    /// about: a `spawn`ed task, and the closure overlapping builds.
    fn of(text: &str) -> Crossing {
        let (own, library) = ledgers();
        crossing(&Ty::parse(text), &own, &library, Destination::Ours)
    }

    /// …and the same type asked about code nothing describes. The pair is what
    /// ADR-045 D1 added, so most tests here come in twos now.
    fn foreign(text: &str) -> Crossing {
        let (own, library) = ledgers();
        crossing(&Ty::parse(text), &own, &library, Destination::Foreign)
    }

    /// Plain data crosses, and so does a container of it - at any depth.
    #[test]
    fn plain_data_and_containers_of_it_may_cross() {
        for text in [
            "i64",
            "String",
            "&str",
            "bool",
            "Vec[i64]",
            "HashMap[String, Vec[i64]]",
            "(i64, String)",
            "()",
            "Option[Vec[(String, f64)]]",
        ] {
            assert_eq!(of(text), Crossing::May, "{text}");
        }
    }

    /// `Shared` is answered by what it holds, at either setting - ADR-037 D6,
    /// and the one row of the table step 1 moved.
    ///
    /// There is nothing for the verdict to take the worse of: at `yes` the count
    /// is atomic wherever the analysis cannot prove nothing crosses, and at `no`
    /// it is plain everywhere because nothing can (ADR-061 D2). Either way the
    /// count suffices for what can happen at that setting, so `Shared` is a
    /// container like `Vec`.
    #[test]
    fn a_shared_is_answered_by_what_it_holds() {
        assert_eq!(of("Shared[String]"), Crossing::May);
        assert_eq!(of("&Shared[String]"), Crossing::May);
        assert_eq!(of("Vec[Shared[i64]]"), Crossing::May);
        assert_eq!(of("(i64, Shared[i64])"), Crossing::May);

        // …and it is answered by what it holds in the other direction too: a
        // `Shared` of something nothing describes is undecided, not permitted.
        assert!(matches!(of("Shared[Mapped]"), Crossing::Undecided { .. }));
        // A bare `Shared` said nothing about what it holds, and what it holds is
        // the whole question.
        assert!(matches!(of("Shared"), Crossing::Undecided { .. }));
    }

    /// **Part II 12.2's counter, which could not be written until ADR-045 D1.**
    ///
    /// `Shared[Locked[i32]]` handed to a task of our own is the program
    /// `user_parallelism = yes` exists to serve, and the verdict refused it: a
    /// lock had to take the worse of the two settings, so it answered `may not`
    /// everywhere. D2 answers `may` here, with two reasons that are each sound at
    /// their own setting and one answer that is the same at both - which is all
    /// Group B ever asked for.
    #[test]
    fn a_lock_goes_into_a_task_of_our_own() {
        for text in [
            "Locked[i32]",
            "SharedMut[i32]",
            "Shared[Locked[i32]]",
            "HashMap[String, Shared[Locked[i64]]]",
            "Vec[SharedMut[String]]",
        ] {
            assert_eq!(of(text), Crossing::May, "{text}");
        }

        // A lock makes nothing crossable that was not: it is answered by what it
        // holds, like any other container, which is Group B's transitivity and
        // not an exception to it.
        assert!(matches!(of("Locked[Mapped]"), Crossing::Undecided { .. }));
        // And a bare lock said nothing about what it holds.
        assert!(matches!(of("Locked"), Crossing::Undecided { .. }));
        assert!(matches!(of("SharedMut"), Crossing::Undecided { .. }));
    }

    /// **And it does not go into code nothing describes** - ADR-045 D3, which is
    /// deliberately the worse answer.
    ///
    /// Strictly this would be safe at `user_parallelism = yes`, where a real
    /// operating-system lock stands in the emitted program and a foreign thread
    /// may touch one. The cautious answer is taken at both settings anyway,
    /// because the alternative is a library written at one setting that does not
    /// compile at the other - §3 of that record measures it: a `SharedMut[T]` at a
    /// foreign call is a count around an operating-system lock at `yes` and around
    /// a plain one at `no`, and a Rust library has one signature.
    #[test]
    fn a_lock_does_not_go_into_code_nothing_describes() {
        for text in [
            "Locked[i32]",
            "SharedMut[i32]",
            "Shared[Locked[i32]]",
            "HashMap[String, Shared[Locked[i64]]]",
        ] {
            let answer = foreign(text);
            assert!(answer.refused().is_some(), "{text}: {answer:?}");
        }

        // The part named is the lock and not the container around it, because the
        // lock is what the answer is about.
        assert_eq!(
            foreign("Shared[Locked[i32]]").refused(),
            Some(("Locked[i32]", None))
        );

        // Everything that is neither a lock nor a shared count answers the same
        // at both destinations: the destination reaches one row of the table and
        // no other.
        for text in ["i64", "Vec[i64]", "Mapped", "?"] {
            assert_eq!(of(text), foreign(text), "{text}");
        }
    }

    /// **And a `Shared` does not go there either**
    /// ([ADR-061](../../../../docs/specification/adr/adr-061.md) D1), which was
    /// decided and not built: `Shared` is in [`CHOSEN`] *and* in [`CONTAINERS`],
    /// the container row answered first, and its row in `CHOSEN` was reached by
    /// nothing. A `Shared[String]` into code nothing describes came back `May` -
    /// the opposite of what D1 says - and the test that would have caught it
    /// asserted the wrong half and passed.
    ///
    /// **The contents still answer first**, which is why the order is what it is
    /// rather than the two lists swapped: where a lock is inside the count, the
    /// lock is the better sentence and the one `NK2503` has a code for. D1 is
    /// what is left when nothing inside refuses.
    #[test]
    fn a_shared_count_does_not_go_into_code_nothing_describes() {
        for text in ["Shared[String]", "Shared[i64]", "Shared"] {
            let answer = foreign(text);
            assert_eq!(answer.why(), Some(Refusal::Count), "{text}: {answer:?}");
            // Into a task of our own it is untouched, which is the pair: D1 is
            // about a signature outside this language and nothing else.
            assert!(of(text).refused().is_none(), "{text}: {:?}", of(text));
        }

        // And it is transitive, like everything else in this file: a container
        // of shared values is no more crossable than one of them (Group B).
        assert_eq!(foreign("Vec[Shared[i64]]").why(), Some(Refusal::Count));
        assert!(of("Vec[Shared[i64]]").refused().is_none());

        // A lock inside one is still the lock's refusal, by its own name.
        let inside = foreign("Shared[Locked[i32]]");
        assert_eq!(inside.why(), Some(Refusal::Lock));
        assert_eq!(inside.refused(), Some(("Locked[i32]", None)));

        // The sentence is the lock's reason without the lock, and it carries
        // neither the word `lock` nor any Rust in it (Part III, C.2).
        let note = foreign("Shared[String]")
            .note()
            .expect("a refusal has a note");
        assert!(note.contains("`Shared[String]`"), "{note}");
        assert!(note.contains("per value"), "{note}");
        for word in ["lock", "Rc", "Arc", "Send"] {
            assert!(!note.contains(word), "`{word}` in: {note}");
        }
        // D1's own way out: pass what is inside, a view or a copy.
        let way_out = foreign("Shared[String]")
            .way_out()
            .expect("a refusal has a way out");
        assert!(way_out.contains("a view or a copy"), "{way_out}");
    }

    /// **Nothing written down answers `MayNot` but a lock and a shared count**,
    /// at either destination.
    ///
    /// The guard the refusing arm has always had: a name quietly acquiring a
    /// refusal is the failure this catches, because a refusal is the one answer
    /// that can reject a correct program. Every type either ledger describes is
    /// asked, at both destinations, and the two families that are *meant* to
    /// refuse are named here rather than skipped silently.
    #[test]
    fn nothing_but_a_lock_or_a_count_is_refused_at_either_destination() {
        let (own, library) = ledgers();
        for into in [Destination::Ours, Destination::Foreign] {
            for name in own.types.keys().chain(library.types.keys()) {
                let answer = crossing(&Ty::named(name), &own, &library, into);
                assert!(
                    answer.refused().is_none()
                        || CHOSEN.contains(&crate::contracts::ty::base(name)),
                    "`{name}` into {into:?}: {answer:?}"
                );
            }
        }
        for text in ["Vec[i64]", "Mapped", "?"] {
            assert!(of(text).refused().is_none(), "{text}");
            assert!(foreign(text).refused().is_none(), "{text}");
        }
    }

    /// The sentences a refusal prints, which Part III C.2 requires of every
    /// diagnostic and which no test would otherwise read.
    ///
    /// Asked of the value rather than through the walk, because the field half of
    /// the message has no source that produces it yet: a struct field's type is
    /// where `at` comes from, and the shape is checked in `a_struct_is_its_fields`
    /// below.
    #[test]
    fn a_refusal_says_what_it_is_and_what_to_do_instead() {
        let refused = Crossing::MayNot {
            part: "Locked[i32]".to_string(),
            at: Some("hits".to_string()),
            why: Refusal::Lock,
        };
        assert_eq!(refused.refused(), Some(("Locked[i32]", Some("hits"))));
        assert_eq!(refused.why(), Some(Refusal::Lock));
        let note = refused.note().expect("a refusal has a note");
        assert!(note.contains("`Locked[i32]`"), "{note}");
        assert!(note.contains("field `hits`"), "{note}");
        assert!(
            note.contains("can't be passed to code the compiler knows nothing about"),
            "{note}"
        );
        // ADR-045 D3's *the answer was chosen* is the finding's own note beside
        // this one (`SAME_AT_BOTH`), said once for every crossing.
        // Part III C.2: no Rust vocabulary in a diagnostic, ever.
        for word in ["Rc", "Arc", "Send", "E0277", "lifetime", "borrow"] {
            assert!(!note.contains(word), "`{word}` in: {note}");
        }
        // The way out is D3's: open the lock, hand the inner value over. Not
        // "don't do that".
        let way_out = refused.way_out().expect("a refusal has a way out");
        assert!(way_out.contains("Open the lock"), "{way_out}");
        assert!(way_out.contains("inside it"), "{way_out}");
    }

    /// A type nothing describes is undecided, and undecided is not `May`.
    #[test]
    fn an_undescribed_type_is_undecided_and_not_permission() {
        for text in [
            "?",
            "Mapped",
            "Locked[Mapped]",
            "fn(&Stats)",
            "$V",
            "Vec[?]",
        ] {
            let answer = of(text);
            assert!(
                matches!(answer, Crossing::Undecided { .. }),
                "{text}: {answer:?}"
            );
            assert!(!answer.may(), "{text}");
        }
    }

    /// A struct is its fields, through the ledger - which is what makes the
    /// rule structural as Group B requires.
    ///
    /// Both non-`May` answers are exercised through the walk: a field nothing
    /// describes is undecided, and a field holding a lock is **refused at a
    /// foreign destination and permitted into a task** - which is ADR-045 D1
    /// reaching four fields deep, and the case that makes the destination a
    /// parameter of the walk rather than a question asked before it.
    #[test]
    fn a_struct_is_its_fields() {
        let (mut own, library) = ledgers();
        own.types.insert(
            "Reading".to_string(),
            TypeContract {
                fields: vec![
                    field("name", Ty::named("String")),
                    field("temp", Ty::named("f64")),
                ],
                ..TypeContract::empty()
            },
        );
        own.types.insert(
            "Counter".to_string(),
            TypeContract {
                fields: vec![field("hits", Ty::parse("Shared[Locked[i64]]"))],
                ..TypeContract::empty()
            },
        );
        own.types.insert(
            "Opaque".to_string(),
            TypeContract {
                fields: vec![field("held", Ty::named("Mapped"))],
                ..TypeContract::empty()
            },
        );
        // And a struct of structs, which is the transitive case.
        own.types.insert(
            "Report".to_string(),
            TypeContract {
                fields: vec![field("counter", Ty::named("Counter"))],
                ..TypeContract::empty()
            },
        );

        assert_eq!(
            crossing(&Ty::named("Reading"), &own, &library, Destination::Ours),
            Crossing::May
        );

        // A field nothing describes: undecided, which is not permission.
        let undecided = crossing(&Ty::named("Opaque"), &own, &library, Destination::Ours);
        assert!(
            matches!(undecided, Crossing::Undecided { .. }),
            "{undecided:?}"
        );
        assert!(
            undecided
                .note()
                .expect("a non-`May` answer has a note")
                .contains("Mapped"),
            "the note names the part nothing describes: {undecided:?}"
        );

        // A field holding a lock: the answer depends on where the struct is
        // going, and the message names the field it came from.
        for ty in ["Counter", "Report"] {
            assert_eq!(
                crossing(&Ty::named(ty), &own, &library, Destination::Ours),
                Crossing::May,
                "{ty} into a task of our own"
            );
            let refused = crossing(&Ty::named(ty), &own, &library, Destination::Foreign);
            assert_eq!(
                refused.refused(),
                Some(("Locked[i64]", Some("hits"))),
                "{ty} into code nothing describes"
            );
            assert_eq!(refused.why(), Some(Refusal::Lock), "{ty}");
            assert!(
                refused
                    .note()
                    .expect("a refusal has a note")
                    .contains("which its field `hits` holds"),
                "{refused:?}"
            );
        }
    }

    /// A type that holds itself terminates, and is answered by its other
    /// fields.
    #[test]
    fn a_type_that_holds_itself_terminates() {
        let (mut own, library) = ledgers();
        own.types.insert(
            "Node".to_string(),
            TypeContract {
                fields: vec![
                    field("value", Ty::named("i64")),
                    field("next", Ty::parse("Option[Node]")),
                ],
                ..TypeContract::empty()
            },
        );
        assert_eq!(
            crossing(&Ty::named("Node"), &own, &library, Destination::Ours),
            Crossing::May
        );

        own.types.insert(
            "Ring".to_string(),
            TypeContract {
                fields: vec![
                    field("held", Ty::parse("Shared[Locked[i64]]")),
                    field("next", Ty::parse("Option[Ring]")),
                ],
                ..TypeContract::empty()
            },
        );
        // The cycle terminates at both destinations, and the lock is still found
        // through it: a walk that gave up on the cycle would lose the refusal.
        assert_eq!(
            crossing(&Ty::named("Ring"), &own, &library, Destination::Ours),
            Crossing::May
        );
        assert!(
            crossing(&Ty::named("Ring"), &own, &library, Destination::Foreign)
                .refused()
                .is_some()
        );
    }

    /// A refusal beats an admission of ignorance, because a reader can act on
    /// it (ADR-033 D9's rule for the same kind of choice).
    ///
    /// Asked of [`join`] directly, because no type produces a refusal to put on
    /// one side of it any more (ADR-037 D6). The rule is still the rule, and the
    /// day a type needs `MayNot` again this is what decides which of two true
    /// answers gets printed.
    #[test]
    fn a_refusal_is_reported_over_an_undecided_part() {
        let refused = Crossing::MayNot {
            part: "Held".to_string(),
            at: None,
            why: Refusal::Lock,
        };
        let unknown = Crossing::Undecided {
            part: "?".to_string(),
        };
        for order in [
            vec![unknown.clone(), refused.clone()],
            vec![refused.clone(), unknown.clone()],
            vec![Crossing::May, unknown.clone(), refused.clone()],
        ] {
            assert_eq!(join(order.into_iter()), refused);
        }
        // …and an undecided part still beats a `May` one.
        assert_eq!(join([Crossing::May, unknown.clone()].into_iter()), unknown);
    }

    /// `std`'s own opaque types are undecided and not refused: their fields are
    /// Rust, and an entry with no fields is "nothing is written down" rather
    /// than "it holds nothing".
    #[test]
    fn a_library_type_whose_fields_are_rust_is_undecided() {
        assert!(matches!(of("Mapped"), Crossing::Undecided { .. }));
        assert!(matches!(of("Lines"), Crossing::Undecided { .. }));
    }
}
