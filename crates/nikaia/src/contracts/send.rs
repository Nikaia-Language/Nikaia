// crates/nikaia/src/contracts/send.rs
//
// Whether a value may cross a thread (ADR-005 §1 Group B, `NK25xx`).
//
// One question, asked of a **type and a destination** (ADR-312 D6):
//
//     may a value of this type go to *this* destination?
//
// ADR-303 D7 states the first half as "a value may only cross into a foreign
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
// justification may differ per setting; the answer may not. Before ADR-312 the
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
// **And one row came back** ([ADR-312](../../../../docs/specification/adr/adr-312.md)
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
//     overlapping emits (ADR-292) - is a step the compiler was never obliged to
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
// speculatively (ADR-288 D11).
//
// Every name in [`PLAIN`] and [`CONTAINERS`] is safe both to **move** to
// another thread and to be **looked at** from one, which is why a view (`&T`)
// asks the same question as the value. The day a name belongs in one column and
// not the other, this needs a second column; stating that here is cheaper than
// discovering it.
//
// **[`CHOSEN`] is that day, and the column it needed turned out to be the
// destination** (ADR-312 predicted this file would want two columns here). A
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

/// **The verdict is written in Nikaia** (`tools/threads.nika`, #125): what a
/// type is (`Crossing`), why a refusal is one (`Refusal`), where the value is
/// going (`Going`, here `Destination`), and the walk through the ledgers that
/// answers it - plain data, containers, the lock family whose answer is the
/// destination's, and a described type's own word or fields.
pub use nikaia_std::tools::threads::{Crossing, Going as Destination, Refusal};

/// The refused part and the field it was found in, where the answer is a
/// refusal - the two halves `tools/threads.nika` answers one at a time,
/// because a nullable pair is not a type Part I 2.3 writes.
pub trait CrossingOps {
    fn refused(&self) -> Option<(String, Option<String>)>;
}

impl CrossingOps for Crossing {
    fn refused(&self) -> Option<(String, Option<String>)> {
        self.refused_part().map(|part| (part, self.refused_at()))
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
/// ([ADR-055](../../../../docs/specification/adr/adr-055.md) §2 D6): bound
/// before a pause, and named after it (`tools/threads.nika`). Everything is a
/// byte position; `named` is the *last* place each name was mentioned.
pub fn held_across_a_pause(
    bound: &[(String, usize)],
    pauses: &[usize],
    named: &std::collections::BTreeMap<String, usize>,
) -> BTreeSet<String> {
    let bound: Vec<(String, i64)> = bound
        .iter()
        .map(|(name, at)| (name.clone(), *at as i64))
        .collect();
    let pauses: Vec<i64> = pauses.iter().map(|at| *at as i64).collect();
    let named: std::collections::BTreeMap<String, i64> = named
        .iter()
        .map(|(name, at)| (name.clone(), *at as i64))
        .collect();
    nikaia_std::tools::threads::held_across_a_pause(&bound, &pauses, &named)
}

/// Whether a value of `ty` may go to `into`.
///
/// `own` is the program's ledger and `library` is `std`'s: between them they
/// hold the fields of every type that is written down (ADR-024), which is what
/// makes this **structural and transitive** as Group B requires - a struct with
/// one lock field is no more crossable than the lock itself.
pub fn crossing(ty: &Ty, own: &Ledger, library: &Ledger, into: Destination) -> Crossing {
    nikaia_std::tools::threads::crossing(
        ty,
        &nikaia_std::tools::threads::Walking { own, library, into },
    )
}

/// The worst of several answers, as the walk joins them.
#[cfg(test)]
fn join(answers: Vec<Crossing>) -> Crossing {
    answers
        .into_iter()
        .fold(Crossing::May, nikaia_std::tools::threads::worst_of)
}

#[cfg(test)]
fn field(name: &str, ty: super::ty::Ty) -> super::FieldContract {
    super::FieldContract {
        name: name.to_string(),
        ty,
        public: true,
        default: String::new(),
        attributes: Vec::new(),
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
    /// ADR-312 D6 added, so most tests here come in twos now.
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
    /// it is plain everywhere because nothing can (ADR-312 D10). Either way the
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

    /// **Part II 12.2's counter, which could not be written until ADR-312 D6.**
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

    /// **And it does not go into code nothing describes** - ADR-312 D8, which is
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
            Some(("Locked[i32]".to_string(), None))
        );

        // Everything that is neither a lock nor a shared count answers the same
        // at both destinations: the destination reaches one row of the table and
        // no other.
        for text in ["i64", "Vec[i64]", "Mapped", "?"] {
            assert_eq!(of(text), foreign(text), "{text}");
        }
    }

    /// **And a `Shared` does not go there either**
    /// ([ADR-312](../../../../docs/specification/adr/adr-312.md) D9), which was
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
        assert_eq!(inside.refused(), Some(("Locked[i32]".to_string(), None)));

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
                        || ["Locked", "SharedMut", "Shared"]
                            .contains(&crate::contracts::ty::base(name)),
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
        assert_eq!(
            refused.refused(),
            Some(("Locked[i32]".to_string(), Some("hits".to_string())))
        );
        assert_eq!(refused.why(), Some(Refusal::Lock));
        let note = refused.note().expect("a refusal has a note");
        assert!(note.contains("`Locked[i32]`"), "{note}");
        assert!(note.contains("field `hits`"), "{note}");
        assert!(
            note.contains("can't be passed to code the compiler knows nothing about"),
            "{note}"
        );
        // ADR-312 D8's *the answer was chosen* is the finding's own note beside
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
    /// foreign destination and permitted into a task** - which is ADR-312 D6
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
                Some(("Locked[i64]".to_string(), Some("hits".to_string()))),
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
    /// it (ADR-292 D2's rule for the same kind of choice).
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
            assert_eq!(join(order), refused);
        }
        // …and an undecided part still beats a `May` one.
        assert_eq!(join(vec![Crossing::May, unknown.clone()]), unknown);
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
