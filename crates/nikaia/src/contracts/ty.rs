// crates/nikaia/src/contracts/ty.rs
//
// The type language the checker reasons in, and the one the ledger records.
//
// It is `ast::Type` with one addition and one subtraction. The addition is
// `Unknown`, which is not a type but the absence of a claim - see below. The
// subtraction is the interner: a ledger is a file, so a name here is text, and
// two types are the same when they are written the same. That is name-for-name
// (ADR-011 D2) applied to types: nothing resolves a module or an alias, so
// `postgres::Connection` and `Connection` are different types, which is correct
// for a compiler that does not know they are not.
//
// **`Unknown` is the whole design.** Stage 0 has no signatures for the Rust
// half of `std` beyond what `std.contracts` writes down, and a Nikaia program
// calls `push_str`, `entry`, `map_or` and `chars` freely. A checker that had to
// answer for those would either need a second frontend for Rust or would have
// to guess - and a type checker that guesses reports errors that are not there,
// which is worse than one that says less. So the rule is:
//
//   * anything involving `Unknown` is compatible with anything;
//   * an error is reported only where **both** sides are known and disagree.
//
// The checker is therefore sound in the direction that matters for a tool
// people run: it never rejects a correct program. It does not catch every
// wrong one, and what it does not catch is a function of how much of `std` is
// written down - which is a number that goes up as ADR-014 proceeds, without
// this file changing.

use std::collections::BTreeSet;
use std::fmt;

use crate::ast;
use crate::parser::Parsed;

/// A type, or the absence of a claim about one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    /// No claim. Compatible with everything, in both directions.
    Unknown,
    /// A name, its arguments, and whether it is a view: `&str`, `Vec[Row]`,
    /// `HashMap[&str, Stats]`.
    Named {
        name: String,
        args: Vec<Ty>,
        view: bool,
    },
    /// `(A, B)` - a fixed number of parts and no name.
    Tuple(Vec<Ty>),
    /// **What the C boundary lends**
    /// ([ADR-147](../../../docs/specification/adr/adr-147.md) D1): `&mut T`,
    /// `&[u8]` and `&mut [u8]`, each a view that lives for the call.
    ///
    /// **A variant rather than two flags on `Named`**, which is the opposite of
    /// how the AST records it and for the reason that decided `Nullable`: there
    /// are seventy-eight places in this compiler that build a `Named`, and not
    /// one of them can produce either shape. A field they would all have to
    /// answer for is a field for one position's sake.
    ///
    /// A plain `&T` is **not** this. That is the view every declaration in this
    /// language writes and it is `Named` with `view`, here as everywhere; what
    /// is new is the `mut` and the run of elements, and a shape that held the
    /// third case too would have two spellings for one type.
    Pointed {
        /// What is pointed at: the `u8` of `&[u8]`, the `T` of `&mut T`.
        item: Box<Ty>,
        /// `[T]` rather than `T` - a run of them, and the length is the
        /// caller's to pass (D2).
        slice: bool,
        /// `&mut` rather than `&`: the callee may write through it.
        mutable: bool,
    },
    /// **A number where a type argument stands**
    /// ([ADR-152](../../../docs/specification/adr/adr-152.md) D1): the `3` of
    /// `Array[f64, 3]`.
    ///
    /// It is a `Ty` rather than a second kind of argument for the reason the
    /// AST's is an alternative of `type_ref`: everything that walks a type's
    /// arguments already walks these, and a second list beside `args` would
    /// have to be threaded through every one of them for one type's sake.
    ///
    /// **It is compatible with nothing but itself.** A count is not a type, so
    /// `Array[f64, 3]` and `Array[f64, 4]` are different types and the ordinary
    /// argument-by-argument comparison says so with no rule of its own - which
    /// is the whole of D4's *the length is part of the type*.
    Count(i64),
    /// `$V` - a name in a *library's* signature that stands for a type the
    /// receiver supplies (ADR-030).
    ///
    /// `HashMap::entry(&HashMap[$K, $V], key: ?) -> Entry[$V]` says the result
    /// holds whatever the map holds. At a call site the receiver's actual type
    /// binds the variables and they are **substituted away**; one that stays
    /// unbound becomes `Unknown`, never a name.
    ///
    /// That last sentence is the whole safety argument, and it is why this does
    /// not contradict [ADR-024] D4. D4 erases a Nikaia function's `T` to `?`
    /// because a `T` that survives into a comparison makes the checker report
    /// that `i32` is not `T` - a false positive. A variable here never survives
    /// into a comparison: it is bound and replaced, or it is `?`.
    ///
    /// The sigil is not decoration. `T`, `K`, `V` are also perfectly good type
    /// names, and a rule that guessed from capitalisation would silently turn
    /// somebody's type into a hole.
    ///
    /// [ADR-024]: ../../../docs/specification/adr/adr-024.md
    Var {
        name: String,
        /// `&$V` - the `&` belongs to the *use*, not to what the receiver
        /// bound. `and_modify` hands its lambda a reference to whatever the map
        /// holds, so the signature writes `fn(&$V)` and `$V` is still `Stats`.
        view: bool,
    },
    /// `fn(&Stats)` - a parameter that takes a lambda, and what the lambda is
    /// handed when it runs (ADR-029).
    ///
    /// **Only a ledger writes one.** Nikaia's own grammar has no syntax for a
    /// function type, so no `.nika` source can declare a parameter of this
    /// shape; these exist because `std`'s higher-order functions are Rust and
    /// something has to say what `and_modify` passes its lambda. Without that,
    /// the `a` in `fn { a.add(t) }` has no type, nothing it is called on
    /// resolves, and the function around it cannot be shown to be `sync`.
    ///
    /// **Since [ADR-102](../../../docs/specification/adr/adr-102.md) D1 a
    /// `.nika` source writes one too**, and the three fields beside `params`
    /// are what a written one says: `fn(Request) -> Response`, and `sync` and
    /// `throws` in the positions a declaration puts them. The defaults are the
    /// language's (D2) — without `sync` the code may pause, without `throws` it
    /// cannot fail — so a ledger entry that says neither reads exactly as a
    /// declaration that says neither.
    Fn {
        params: Vec<Ty>,
        /// `-> R`, absent where the code hands nothing back. `std`'s own
        /// entries write none: the ledger's type language had no result on a
        /// lambda until D1, and what those entries are read for is the arity
        /// and the `sync` column.
        result: Option<Box<Ty>>,
        is_sync: bool,
        throws: bool,
    },
    /// `T?` - Part I 2.3's nullable type, and the whole of it: a type is
    /// non-nullable unless it says otherwise.
    ///
    /// **A wrapper rather than a flag**, which is the opposite of how the AST
    /// records it, and for a reason: the AST's flag sits beside `is_view`
    /// because `&str?` is a nullable view and the two are independent, while
    /// here every reader of a type already recurses, so a wrapper is one arm
    /// per reader instead of a condition inside each of them.
    ///
    /// `Option<T>` is what it lowers to and **not** what it is called: the
    /// specification's own mapping is Rust `Option<T>` to Nikaia `T?`
    /// (Part III, 15.2), so a diagnostic that said `Option[String]` would name a
    /// type the program cannot write (Part III, C.1).
    Nullable(Box<Ty>),
    /// **Elements produced step by step**
    /// ([ADR-105](../../../../docs/specification/adr/adr-105.md) D1):
    /// `Seq[T]`, what `keys()`, `chars()` and `xs.map fn …` hand back. Elements
    /// of a type, produced one at a time when asked for, with no length until
    /// the end and nothing laid out in memory.
    ///
    /// **A container is not this.** A `Vec[T]` has its elements already, a
    /// length and an index, and is walked as often as one likes; a `Vec`,
    /// a `HashMap` and a range are walked *as* a `Seq` by a `for` and are not
    /// one.
    ///
    /// `sync`, `pauses` and `throws` after it say what **one step** may do.
    /// `throws` is [ADR-102](../../../../docs/specification/adr/adr-102.md)
    /// D2's reading — without it a step cannot fail — and the other two are
    /// **three** states rather than two
    /// ([ADR-172](../../../../docs/specification/adr/adr-172.md) D1):
    ///
    /// * **`sync`** — the step does not pause. `keys()` reads memory.
    /// * **`pauses`** — the step suspends, and a `for` over it gives its thread
    ///   up rather than holding it. `io::lines()` reads the pipe.
    /// * **neither** — nobody said, which is what a `map`'s step is: it runs
    ///   the lambda, and what *that* does is the caller's. Such a step keeps
    ///   the blocking form, because the two ways of being wrong are not
    ///   symmetric — awaiting something that has no pausing step does not
    ///   compile, and holding a thread that could have been given up costs a
    ///   thread. D1 says which of the two is taken.
    ///
    /// **`sync` and `pauses` are not both**, and a ledger that writes both is
    /// refused rather than read as either.
    ///
    /// `Par[T]` is the same shape with `parallel` set (D3), and the one
    /// dimension that needs a second word is the caller's rule: a lambda given
    /// to a method on a `Par[T]` runs on several cores at once and must be
    /// `sync`, where one given to a `Seq[T]` may pause.
    ///
    /// **None of the words is in the surface type grammar** (D4). A program
    /// writes `keys()`, `for` and `collect()`; the ledger does the naming.
    Seq {
        item: Box<Ty>,
        is_sync: bool,
        /// The step suspends, and the `for` over it is written as one that
        /// does ([ADR-172](../../../../docs/specification/adr/adr-172.md) D1).
        ///
        /// **A positive claim and not the absence of `sync`**, which is the
        /// whole of why it is a third word: the absence is *nobody said*, and
        /// writing `.await` on it would be a correct program refused by
        /// `rustc` about a file nobody wrote (Part III, C.1 and C.4).
        pauses: bool,
        throws: bool,
        parallel: bool,
        /// What the sequence is **as a whole** rather than what one step does
        /// ([ADR-212](../../../../docs/specification/adr/adr-212.md) D1): whether
        /// it can be walked from the back, whether its length is known, and
        /// whether walking it leaves it there.
        shape: Shape,
    },
}

/// **What a sequence is as a whole**
/// ([ADR-212](../../../../docs/specification/adr/adr-212.md) D1): three words
/// after `Seq[T]`, beside the three about a step.
///
/// They are the language below's `DoubleEndedIterator`, `ExactSizeIterator` and
/// `Copy`, said in this language's words so that the checker can answer what
/// `rustc` would otherwise answer about a file nobody wrote: `io::lines().rev()`
/// is refused here, in terms of the program, and not in the generated Rust.
///
/// * **`ends`** — it can be walked from the back as well as the front.
///   `chars()`, `drain()` and a range can; `keys()` and `io::lines()` cannot.
/// * **`sized`** — its length is known before it is walked. A range's is;
///   a `filter`'s is not, because the lambda decides.
/// * **`replays`** — walking it does not consume it, so `NK2702` does not
///   apply. A range is a value, two numbers, and walking one walks a copy.
///
/// **Two words for the back and not one**, because the language below needs the
/// length to walk some things from the back and not others: a `filter` is
/// walked backwards without it, a `take` and a `zip` are not. One word would
/// have refused `chars().rev()` or allowed `lines.take(3).rev()`.
///
/// **In a receiver's position they are a demand** — `rev` writes
/// `(Seq[$T] ends)` — and **in a result's they pass through**: a result has a
/// word where the entry writes it and every sequence handed in has it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shape {
    pub ends: bool,
    pub sized: bool,
    pub replays: bool,
}

impl Shape {
    /// Whether a sequence of this shape can go where `wanted` is demanded.
    pub fn meets(&self, wanted: &Shape) -> bool {
        self.missing(wanted).is_empty()
    }

    /// The words `wanted` demands and this shape does not have, in the order
    /// they are written.
    pub fn missing(&self, wanted: &Shape) -> Vec<&'static str> {
        [
            (wanted.ends && !self.ends, ENDS),
            (wanted.sized && !self.sized, SIZED),
            (wanted.replays && !self.replays, REPLAYS),
        ]
        .into_iter()
        .filter_map(|(missing, word)| missing.then_some(word))
        .collect()
    }
}

/// [`Shape::ends`]'s word.
pub const ENDS: &str = "ends";
/// [`Shape::sized`]'s word.
pub const SIZED: &str = "sized";
/// [`Shape::replays`]'s word.
pub const REPLAYS: &str = "replays";

/// The stamp a lock puts on what it hands out
/// ([ADR-111](../../../../docs/specification/adr/adr-111.md) D1).
pub const SEEN: &str = "Seen";

/// The ledger's word for a produced sequence
/// ([ADR-105](../../../../docs/specification/adr/adr-105.md) D1).
pub const SEQ: &str = "Seq";

/// The same, where the steps run at once (D3).
pub const PAR: &str = "Par";

/// The word a `Seq` whose **step suspends** carries
/// ([ADR-172](../../../../docs/specification/adr/adr-172.md) D1).
pub const PAUSES: &str = "pauses";

/// **A fixed-size array**
/// ([ADR-152](../../../../docs/specification/adr/adr-152.md) D1): `Array[T, N]`,
/// `N` elements inline and nothing allocated.
///
/// A name and not a shape, so every reader of a type that does not care about
/// arrays is unchanged: it is a `Named` with two arguments, and the second is a
/// [`Ty::Count`].
pub const ARRAY: &str = "Array";

/// **What `T::fields` walks**, one element of it
/// ([ADR-088](../../../docs/specification/adr/adr-088.md) D2,
/// [ADR-181](../../../docs/specification/adr/adr-181.md)).
///
/// A name **no program can write**: `$` is not in an identifier, so a reader
/// never meets it and a `.nika` file cannot declare one. It is the type a
/// `for field in T::fields` binding has, and the two members it answers —
/// `.name` and `.of(value)` — are the whole of what Part II 10.3 gives a
/// reflected field.
///
/// **A type and not a `struct` in `std`**, because it never reaches the
/// language below: the loop is **unrolled**, so what is emitted is the field
/// reads a program would have written by hand and there is nothing left for a
/// descriptor to be at run time.
pub const FIELD: &str = "$Field";

/// **One of an `enum`'s variants, as `T::variants` hands it out**: `.name`,
/// the variant's own name as text, and `.is(value)`, whether a value is that
/// variant. Unrolled as a field is, so it never reaches the language below.
pub const VARIANT: &str = "$Variant";

/// **The one type text has** ([ADR-107](../../../docs/specification/adr/adr-107.md)),
/// under the name a program writes it with.
///
/// The compiler's own noun for a view of it stays `str`, because that is the
/// word the language below uses and the emitter writes name for name
/// ([ADR-011](../../../docs/specification/adr/adr-011.md) D2). What
/// [ADR-184](../../../docs/specification/adr/adr-184.md) D2 changed is the
/// **surface**: `ref String` is what is written and what is printed, and `str`
/// is not a type a program may name.
pub const TEXT: &str = "String";

/// The compiler's own noun for a view of [`TEXT`], which no program writes.
pub(crate) const TEXT_VIEW: &str = "str";

/// A type's own name, with the module it lives in taken off.
///
/// **Since [ADR-154](../../../docs/specification/adr/adr-154.md) D3 a `std`
/// type carries its module**: `collections::HashMap`, `time::Duration`,
/// `foreign::CStr`. The module says where the name is *reached from* and the
/// last segment is the type, so every rule here that names a type by hand — the
/// copy list, the crossing analysis's containers, the hash a map gets — asks
/// this rather than the written name.
pub fn base(name: &str) -> &str {
    match name.rsplit_once("::") {
        Some((_, last)) => last,
        None => name,
    }
}

impl Ty {
    pub fn named(name: impl Into<String>) -> Ty {
        Ty::Named {
            name: name.into(),
            args: Vec::new(),
            view: false,
        }
    }

    pub fn view(name: impl Into<String>) -> Ty {
        Ty::Named {
            name: name.into(),
            args: Vec::new(),
            view: true,
        }
    }

    /// The same type, read as a **view** of it
    /// ([ADR-191](../../../docs/specification/adr/adr-191.md) D1).
    ///
    /// The normalisation both doors already do, in one place a third one can
    /// call: a view of `String` is `str`
    /// ([ADR-184](../../../docs/specification/adr/adr-184.md) D2, which is why
    /// `parse` and `from_ast` each carry the same line), and a view of anything
    /// else is that name with the word on it.
    ///
    /// **A type this compiler cannot name has no view**, and neither does one
    /// that is already one: `Unknown` stays the absence of a claim, and a
    /// second `ref` on a view would be a type nothing writes.
    pub fn as_a_view(&self) -> Ty {
        match self {
            Ty::Named { view: true, .. } => self.clone(),
            Ty::Named { name, args, .. } if base(name) == TEXT && args.is_empty() => {
                Ty::view(TEXT_VIEW)
            }
            Ty::Named { name, args, .. } => Ty::Named {
                name: name.clone(),
                args: args.clone(),
                view: true,
            },
            other => other.clone(),
        }
    }

    /// **What a lock handed out**
    /// ([ADR-111](../../../../docs/specification/adr/adr-111.md) D1).
    ///
    /// `Seen[T]` is a type here and in the ledger's type language, and it is
    /// **not** a type in the language below: the emitter erases it, so a
    /// `Seen[i64]` is an `i64`, a field declared `Seen[i64]` is an `i64` field,
    /// and a signature with `Seen` in it is one without. No counter, no marker,
    /// no check at run time, no bytes.
    ///
    /// What it buys is that the **shape** of a read-modify-write through two
    /// doors is visible: the value a `set` is given carries where it came from.
    pub fn seen(inner: Ty) -> Ty {
        Ty::Named {
            name: SEEN.to_string(),
            args: vec![inner],
            view: false,
        }
    }

    /// Whether a lock handed this out, at any depth a stamp can be at.
    ///
    /// A nullable of a stamped value is stamped: `kasse.get()` through a `?.`
    /// is still what the lock said.
    pub fn is_seen(&self) -> bool {
        match self {
            // With its argument: a bare `Seen` is a type the program declared.
            Ty::Named { name, args, .. } if name == SEEN => args.len() == 1,
            Ty::Nullable(inner) => inner.is_seen(),
            _ => false,
        }
    }

    /// The type under the stamp, or this type where there is none.
    ///
    /// **There is no word for this in the language** (D4): a program cannot
    /// take a stamp off, and this exists for the emitter, which erases the
    /// whole thing, and for a fit that has to compare what is underneath.
    pub fn unseen(&self) -> Ty {
        match self {
            Ty::Named { name, args, .. } if name == SEEN && args.len() == 1 => args[0].clone(),
            Ty::Nullable(inner) => Ty::Nullable(Box::new(inner.unseen())),
            other => other.clone(),
        }
    }

    /// Whether this type **is** a view — `&str`, `&Vec[Row]`, `&$V`.
    ///
    /// What hangs on it is whether a second `&` would be written in front of
    /// one ([ADR-094](../../../docs/specification/adr/adr-094.md) D1), so a
    /// nullable of a view counts and a tuple does not: `&(A, B)` is not a
    /// spelling this language has.
    pub fn is_a_view(&self) -> bool {
        match self {
            Ty::Named { view, .. } | Ty::Var { view, .. } => *view,
            Ty::Nullable(inner) => inner.is_a_view(),
            // **`&[T]` is a view of a run**
            // ([ADR-179](../../../docs/specification/adr/adr-179.md) D1), and
            // saying so is what makes the compiler write the `&` a caller does
            // not ([ADR-094](../../../docs/specification/adr/adr-094.md) D1):
            // `total(xs)` for an `xs: Vec[i64]` used to reach `rustc` as
            // *expected `&[i64]`, found `Vec<i64>`*, about a file nobody wrote.
            //
            // **`&mut [T]` is not**, and the distinction is the word: a
            // position the callee may write through is a declaration
            // ([ADR-094](../../../docs/specification/adr/adr-094.md) D3) and
            // gains its `&mut` from that, so both answering would put two
            // references on one parameter.
            Ty::Pointed {
                slice: true,
                mutable: false,
                ..
            } => true,
            _ => false,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    /// Whether a value of this type may stand where `expected` is wanted.
    ///
    /// Equality, plus the rule that makes the checker usable: **anything
    /// involving `Unknown` fits.** There is no subtyping and no implicit
    /// widening - Nikaia states that a conversion is written (ADR-013 D7), and
    /// a checker that quietly allowed `i32` where `i64` is wanted would be
    /// checking a different language from the one the specification describes.
    pub fn fits(&self, expected: &Ty) -> bool {
        match (self, expected) {
            (Ty::Unknown, _) | (_, Ty::Unknown) => true,
            // **A stamp passes through**
            // ([ADR-111](../../../../docs/specification/adr/adr-111.md) D2): a
            // `Seen[i64]` goes wherever an `i64` goes, and what it reaches is
            // stamped in turn. That covers every sink a program has — `f"…"`,
            // `println`, a file's data, a response body — so *a `Seen` reaches
            // the world without a word written for it*.
            //
            // **What it does not pass is a `set`**, and that is a refusal of its
            // own (`NK2205`) rather than a hole in the fit: a type that could
            // not be handed on would need a word to take the stamp off, and D4
            // says there is none.
            (a, b) if a.is_seen() || b.is_seen() => a.unseen().fits(&b.unseen()),
            (Ty::Tuple(a), Ty::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.fits(b))
            }
            // **A count fits the same count and nothing else**
            // ([ADR-152](../../../docs/specification/adr/adr-152.md) D4): the
            // length is part of the type, so `Array[f64, 3]` and
            // `Array[f64, 4]` are different types with no rule of their own -
            // the argument-by-argument comparison one arm down reaches this and
            // says it. A count against a *type* falls to `false` below, which
            // is right: neither is the other.
            (Ty::Count(a), Ty::Count(b)) => a == b,
            // **What the C boundary lends fits the same shape and nothing
            // else** ([ADR-147](../../../docs/specification/adr/adr-147.md) D1):
            // a `&mut [u8]` is not a `&[u8]`, because the second promises not
            // to write - and it is not a `&u8` either, because one of them is a
            // run and the other is one element.
            (
                Ty::Pointed {
                    item: a,
                    slice: asl,
                    mutable: am,
                },
                Ty::Pointed {
                    item: b,
                    slice: bsl,
                    mutable: bm,
                },
            ) => a.fits(b) && asl == bsl && am == bm,
            // **An `Array[T, N]` fits an `Array[T]`, whatever `N` is**
            // ([ADR-184](../../../docs/specification/adr/adr-184.md) D3): the
            // declaration says *an array of any length* and the call is what
            // says which, so the function is generic over it and every call
            // knows its own. `Array[T, N]` against `Array[T, M]` is the
            // argument-by-argument comparison below and stays two types
            // ([ADR-152](../../../docs/specification/adr/adr-152.md) D4).
            //
            // **One direction only.** An `Array[T]` does not fit an
            // `Array[T, 3]`: *any length* is not *three*, and accepting it
            // would be a claim the declaration does not make.
            (
                Ty::Named {
                    name: found,
                    args: given,
                    view: false,
                },
                Ty::Named {
                    name: want,
                    args: declared,
                    view: false,
                },
            ) if found == ARRAY && want == ARRAY && declared.len() == 1 && given.len() == 2 => {
                given[0].fits(&declared[0])
            }
            // **And what a caller may hand to one**
            // ([ADR-147](../../../docs/specification/adr/adr-147.md) D1): the
            // declaration says what C wants and the caller writes what this
            // language has, so the fit is where the two meet. A `Vec[u8]`, an
            // `Array[u8, N]` and text all hand a run of `u8` to a `&[u8]`; a
            // plain value fits a `&T` the way it fits any other view, because
            // the reference is the compiler's to write (ADR-094 D1).
            //
            // **One direction only.** Nothing fits *out* of a boundary type:
            // what a C function hands back is an address, and the value this
            // language would have to make of it is D3's handle or D4's copy.
            (found, Ty::Pointed { item, slice, .. }) => match slice {
                true => lends_a_run_of(found, item),
                false => found.fits(item),
            },
            // Two lambdas fit when they take the same things. A lambda never
            // fits a named type and no named type fits a lambda - which is a
            // claim, so it is only made where both sides are written down, and
            // `Unknown` above has already taken every other case.
            //
            // **And a lambda that does less fits a type that allows more**
            // ([ADR-102](../../../docs/specification/adr/adr-102.md) D2), which
            // is where the two promises are read: one that never pauses goes
            // where pausing is allowed and one that cannot fail goes where
            // failing is, and the other direction is the assertion `NK2206` and
            // `NK2606` refuse.
            (
                Ty::Fn {
                    params: a,
                    result: ar,
                    is_sync: asy,
                    throws: at,
                },
                Ty::Fn {
                    params: b,
                    result: br,
                    is_sync: bsy,
                    throws: bt,
                },
            ) => {
                a.len() == b.len()
                    && a.iter().zip(b).all(|(a, b)| a.fits(b))
                    && match (ar, br) {
                        (Some(a), Some(b)) => a.fits(b),
                        // A result nobody wrote is the absence of a claim, which
                        // `Unknown` is everywhere else in this file.
                        _ => true,
                    }
                    && (*asy || !*bsy)
                    && (!*at || *bt)
            }
            // **Two sequences fit when their items do**
            // ([ADR-105](../../../../docs/specification/adr/adr-105.md) D1), and
            // the two words are read as a function type's are one arm up: a
            // sequence whose steps never pause goes where pausing is allowed, and
            // one whose steps cannot fail goes where failing is. `Par` fits
            // `Seq` and not the other way round, which is D3 - a `Par`'s surface
            // *is* a `Seq`'s, and a `Seq` is not promised to run at once.
            (
                Ty::Seq {
                    item: a,
                    is_sync: asy,
                    pauses: ap_,
                    throws: at,
                    parallel: apar,
                    shape: ashape,
                },
                Ty::Seq {
                    item: b,
                    is_sync: bsy,
                    pauses: bp_,
                    throws: bt,
                    parallel: bpar,
                    shape: bshape,
                },
                // **`pauses` fits the same way `throws` does and the opposite
                // way to `sync`** ([ADR-172](../../../../docs/specification/adr/adr-172.md)
                // D1): it is a claim about what a step *does*, so a sequence
                // whose step pauses does not fit a position that did not say
                // it would, and one that says nothing fits either. `sync` is
                // the other polarity because it is a *promise* rather than a
                // warning, which is the asymmetry D1 rests on.
            ) => {
                a.fits(b)
                    && (*asy || !*bsy)
                    && (!*ap_ || *bp_)
                    && (!*at || *bt)
                    && (*apar || !*bpar)
                    // A demand is met by a sequence that has the word
                    // (ADR-212 D1), exactly as `sync` is.
                    && ashape.meets(bshape)
            }
            // A variable that reaches a comparison was never bound, and an
            // unbound variable is the absence of a claim rather than a claim
            // about a type called `$V`. `substitute` is supposed to have
            // removed it; this is the belt to that pair of braces.
            (Ty::Var { .. }, _) | (_, Ty::Var { .. }) => true,
            // Two nullables fit when what they may hold fits.
            (Ty::Nullable(a), Ty::Nullable(b)) => a.fits(b),
            // **And a plain value fits a nullable slot**, which is the one
            // widening this checker has. Part I 2.3 writes it: `let mut m:
            // &str? = null` and then `m = "World"`, a `&str` into a `&str?`.
            // The other direction is not a fit - a `T?` where a `T` is wanted
            // is the whole point of the type being separate - and `??` (3.5) is
            // how a program gets from one to the other.
            (found, Ty::Nullable(want)) => found.fits(want),
            (
                Ty::Named {
                    name: a,
                    args: aa,
                    view: av,
                },
                Ty::Named {
                    name: b,
                    args: ba,
                    view: bv,
                },
            ) => {
                a == b
                    && av == bv
                    && aa.len() == ba.len()
                    && aa.iter().zip(ba).all(|(a, b)| a.fits(b))
            }
            _ => false,
        }
    }

    /// The type as a ledger writes it, which is how the source writes it.
    pub fn text(&self) -> String {
        self.to_string()
    }

    /// Read one back.
    pub fn parse(text: &str) -> Ty {
        let text = text.trim();
        if text.is_empty() || text == "?" {
            return Ty::Unknown;
        }
        // **A run of digits is a count**
        // ([ADR-152](../../../docs/specification/adr/adr-152.md) D1), read
        // first because nothing else in this language can be one: no type name
        // begins with a digit, so there is nothing for this to take away.
        if let Ok(n) = text.parse::<i64>() {
            return Ty::Count(n);
        }
        // **The two boundary shapes are read before the `?`**
        // ([ADR-155](../../../docs/specification/adr/adr-155.md) D5): on a
        // `&mut T` or a `&[T]` the trailing `?` is the **pointee's**, so
        // `&mut sqlite3?` is a slot that holds a handle or nothing rather than
        // a view that may be absent. A plain `&T?` is untouched and is Part I
        // 2.3's own nullable view, which is what `let mut m: &str? = null`
        // writes.
        if let Some(pointed) = pointed_at(text) {
            return pointed;
        }
        // A trailing `?` is Part I 2.3's nullable marker, read before anything
        // else so that `&str?` and `Vec[i64]?` reach the branches below as the
        // types they are nullable *of*. `"?"` alone is `Unknown` and was taken
        // one line up, which is what keeps the two spellings apart.
        if let Some(inner) = text.strip_suffix('?') {
            return Ty::Nullable(Box::new(Ty::parse(inner)));
        }
        if let Some(inner) = text.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
            return Ty::Tuple(split_args(inner).iter().map(|p| Ty::parse(p)).collect());
        }
        // `fn(&Stats)`, and `fn()` for a lambda that is handed nothing. Read
        // before the `&`, because a function type is never a view.
        //
        // **Since [ADR-102](../../../docs/specification/adr/adr-102.md) D1 the
        // closing `)` is not the end**: `fn(Request) -> Response sync throws`
        // is the whole spelling, so the parenthesis is matched rather than
        // found at the end, and what follows it is read backwards — the two
        // words first, because the result is whatever is left in front of them.
        if let Some(rest) = text.strip_prefix("fn(")
            && let Some(close) = closing_paren(rest)
        {
            let mut tail = rest[close + 1..].trim();
            let mut is_sync = false;
            let mut throws = false;
            loop {
                if let Some(shorter) = word_off(tail, "throws") {
                    throws = true;
                    tail = shorter;
                    continue;
                }
                if let Some(shorter) = word_off(tail, "sync") {
                    is_sync = true;
                    tail = shorter;
                    continue;
                }
                break;
            }
            let result = tail
                .strip_prefix("->")
                .map(|r| Box::new(Ty::parse(r)))
                .filter(|_| !tail.is_empty());
            return Ty::Fn {
                params: split_args(&rest[..close])
                    .iter()
                    .map(|p| Ty::parse(p))
                    .collect(),
                result,
                is_sync,
                throws,
            };
        }
        // **`Seq[T] sync throws`, read the way a function type's tail is**
        // ([ADR-105](../../../../docs/specification/adr/adr-105.md) D1): the
        // two words stand after the type and say what one **step** may do. The
        // closing bracket is matched rather than found at the end, for the same
        // reason `fn(` matches its parenthesis - `Seq[HashMap[$K, $V]] sync` has
        // a `]` in the middle.
        //
        // Before the `&`, because a produced sequence is never a view: it is
        // walked by value (D2), which is what the once-only rule rests on.
        for (word, parallel) in [(SEQ, false), (PAR, true)] {
            let Some(rest) = text.strip_prefix(word).map(str::trim_start) else {
                continue;
            };
            let Some(rest) = rest.strip_prefix('[') else {
                continue;
            };
            let Some(close) = closing_bracket(rest) else {
                continue;
            };
            let mut tail = rest[close + 1..].trim();
            let mut is_sync = false;
            let mut pauses = false;
            let mut throws = false;
            let mut shape = Shape::default();
            loop {
                // **The shape's three words** (ADR-212 D1), in any order among
                // the step's.
                let words = [
                    (ENDS, &mut shape.ends),
                    (SIZED, &mut shape.sized),
                    (REPLAYS, &mut shape.replays),
                ];
                let mut took = false;
                for (word, flag) in words {
                    if let Some(shorter) = word_off(tail, word) {
                        *flag = true;
                        tail = shorter;
                        took = true;
                        break;
                    }
                }
                if took {
                    continue;
                }
                if let Some(shorter) = word_off(tail, "throws") {
                    throws = true;
                    tail = shorter;
                    continue;
                }
                if let Some(shorter) = word_off(tail, "sync") {
                    is_sync = true;
                    tail = shorter;
                    continue;
                }
                // **The third word** ([ADR-172](../../../../docs/specification/adr/adr-172.md)
                // D1), read here and nowhere else: a step that suspends says so
                // rather than being inferred from the absence of `sync`.
                if let Some(shorter) = word_off(tail, PAUSES) {
                    pauses = true;
                    tail = shorter;
                    continue;
                }
                break;
            }
            // Both words is not a type, and reading it as either would be a
            // claim the file does not make - the same rule the leftover tail
            // below is refused by.
            if is_sync && pauses {
                continue;
            }
            // Anything left over is not this: `Sequence[T] of stuff` is a name
            // with arguments and a tail nobody wrote, and reading it as a `Seq`
            // would be a claim the file does not make.
            if !tail.is_empty() {
                continue;
            }
            return Ty::Seq {
                item: Box::new(Ty::parse(&rest[..close])),
                is_sync,
                pauses,
                throws,
                parallel,
                shape,
            };
        }
        let (view, rest) = match a_view_of(text) {
            Some(rest) => (true, rest.trim()),
            None => (false, text),
        };
        // After the `&`, because `&$V` is a view of what `$V` binds to and not
        // a type whose name begins with a dollar.
        if let Some(name) = rest.strip_prefix('$').filter(|name| !name.is_empty()) {
            return Ty::Var {
                name: name.to_string(),
                view,
            };
        }
        match rest.find('[') {
            Some(at) if rest.ends_with(']') => Ty::Named {
                name: rest[..at].trim().to_string(),
                args: split_args(&rest[at + 1..rest.len() - 1])
                    .iter()
                    .map(|p| Ty::parse(p))
                    .collect(),
                view,
            },
            // **A view of `String` is a view of text**
            // ([ADR-184](../../../docs/specification/adr/adr-184.md) D2), and
            // that is the whole of what the second noun was for: `str` is not
            // a type a program may write, so `ref String` and the `&str` it
            // replaces are read back as one thing. Without this they were two,
            // and a call that passed one where the other was declared was
            // `NK1102` about a distinction the language does not have.
            _ if view && rest == TEXT => Ty::view(TEXT_VIEW),
            _ => Ty::Named {
                name: rest.to_string(),
                args: Vec::new(),
                view,
            },
        }
    }

    /// The same type with every name in `parameters` replaced by `Unknown`.
    ///
    /// A generic parameter is a name that stands for a type rather than being
    /// one, and a checker that treated `T` as a type would report that `i32` is
    /// not `T` - which is the shape of a false positive this checker exists not
    /// to produce. Erasing them says exactly as much as Stage 0 knows: a
    /// generic function's parameters are checked for *number* and not for type.
    pub fn erase(&self, parameters: &BTreeSet<String>) -> Ty {
        match self {
            Ty::Unknown => Ty::Unknown,
            // A count is a number and not a name, so no parameter can stand
            // for it (ADR-152 §4 leaves an integer parameter of anything else
            // undecided).
            Ty::Count(n) => Ty::Count(*n),
            Ty::Pointed {
                item,
                slice,
                mutable,
            } => Ty::Pointed {
                item: Box::new(item.erase(parameters)),
                slice: *slice,
                mutable: *mutable,
            },
            Ty::Tuple(parts) => Ty::Tuple(parts.iter().map(|p| p.erase(parameters)).collect()),
            Ty::Fn {
                params,
                result,
                is_sync,
                throws,
            } => Ty::Fn {
                params: params.iter().map(|p| p.erase(parameters)).collect(),
                result: result.as_ref().map(|r| Box::new(r.erase(parameters))),
                is_sync: *is_sync,
                throws: *throws,
            },
            // The two words are the **step's** and not the item's, so they
            // travel unchanged while the item is rewritten
            // ([ADR-105](../../../../docs/specification/adr/adr-105.md) D1).
            Ty::Seq {
                item,
                is_sync,
                pauses,
                throws,
                parallel,
                shape,
            } => Ty::Seq {
                item: Box::new(item.erase(parameters)),
                is_sync: *is_sync,
                pauses: *pauses,
                throws: *throws,
                parallel: *parallel,
                shape: *shape,
            },
            // A library's variable is not a Nikaia function's generic, and
            // erasing one is not the other's business.
            Ty::Var { name, view } => Ty::Var {
                name: name.clone(),
                view: *view,
            },
            Ty::Nullable(inner) => Ty::Nullable(Box::new(inner.erase(parameters))),
            Ty::Named { name, args, view } => {
                if args.is_empty() && parameters.contains(name) {
                    return Ty::Unknown;
                }
                Ty::Named {
                    name: name.clone(),
                    args: args.iter().map(|a| a.erase(parameters)).collect(),
                    view: *view,
                }
            }
        }
    }

    /// The same type with every name in `parameters` turned into a variable a
    /// call site binds.
    ///
    /// This is [`Self::erase`]'s sibling and the difference between them is the
    /// whole of [ADR-074]: a name that stands for a type is recorded as a
    /// **variable** rather than as the absence of a claim, so `fn hand[T](x: T)
    /// -> T` tells a caller that what comes back is what went in. `erase` stays
    /// for `Self`, which is a name for the type an `impl` is on and is bound by
    /// nothing a call site passes.
    ///
    /// `Ty::Var`'s safety argument is unchanged and is what makes this legal:
    /// a variable is bound and replaced, or it becomes `Unknown`. It never
    /// survives into a comparison, so no caller is ever told that `i64` is not
    /// `T` - which is the false positive [ADR-024] D4 erased generics to avoid.
    ///
    /// [ADR-024]: ../../../docs/specification/adr/adr-024.md
    /// [ADR-074]: ../../../docs/specification/adr/adr-074.md
    pub fn parameterise(&self, parameters: &BTreeSet<String>) -> Ty {
        match self {
            Ty::Unknown => Ty::Unknown,
            Ty::Count(n) => Ty::Count(*n),
            Ty::Pointed {
                item,
                slice,
                mutable,
            } => Ty::Pointed {
                item: Box::new(item.parameterise(parameters)),
                slice: *slice,
                mutable: *mutable,
            },
            Ty::Tuple(parts) => {
                Ty::Tuple(parts.iter().map(|p| p.parameterise(parameters)).collect())
            }
            Ty::Fn {
                params,
                result,
                is_sync,
                throws,
            } => Ty::Fn {
                params: params.iter().map(|p| p.parameterise(parameters)).collect(),
                result: result
                    .as_ref()
                    .map(|r| Box::new(r.parameterise(parameters))),
                is_sync: *is_sync,
                throws: *throws,
            },
            // The two words are the **step's** and not the item's, so they
            // travel unchanged while the item is rewritten
            // ([ADR-105](../../../../docs/specification/adr/adr-105.md) D1).
            Ty::Seq {
                item,
                is_sync,
                pauses,
                throws,
                parallel,
                shape,
            } => Ty::Seq {
                item: Box::new(item.parameterise(parameters)),
                is_sync: *is_sync,
                pauses: *pauses,
                throws: *throws,
                parallel: *parallel,
                shape: *shape,
            },
            Ty::Var { name, view } => Ty::Var {
                name: name.clone(),
                view: *view,
            },
            Ty::Nullable(inner) => Ty::Nullable(Box::new(inner.parameterise(parameters))),
            Ty::Named { name, args, view } => {
                if args.is_empty() && parameters.contains(name) {
                    return Ty::Var {
                        name: name.clone(),
                        view: *view,
                    };
                }
                Ty::Named {
                    name: name.clone(),
                    args: args.iter().map(|a| a.parameterise(parameters)).collect(),
                    view: *view,
                }
            }
        }
    }

    /// The type a `.nika` declaration names.
    pub fn from_ast(parsed: &Parsed, ty: &ast::Type) -> Ty {
        // **An integer argument** (ADR-152 D1), read first: it has no name, no
        // arguments and no `?`, so none of the branches below has anything to
        // say about it.
        if let Some(n) = ty.count {
            return Ty::Count(n);
        }
        // **`Array[T]` with no count is the run itself**
        // ([ADR-184](../../../docs/specification/adr/adr-184.md) D3), and under
        // a `ref` it is a view of one — the same type the bracket form spells.
        // An `Array[T, N]` carries its length and is laid out inline
        // ([ADR-152](../../../docs/specification/adr/adr-152.md) D4); with the
        // `N` gone there is no length in the type and nothing to lay out, so
        // what is left is a run somebody else keeps.
        //
        // Read here rather than in the grammar because the second alternative
        // of `type_ref` already parses it: one name, one argument, and a `ref`
        // in front. Adding a third alternative would make the **spelling**
        // decide what is one question about the type.
        if ty.is_view && !ty.is_slice && parsed.text(ty.name) == ARRAY && ty.generics.len() == 1 {
            let item = Ty::from_ast(parsed, &ty.generics[0]);
            let item = match ty.is_nullable {
                true => Ty::Nullable(Box::new(item)),
                false => item,
            };
            return Ty::Pointed {
                item: Box::new(item),
                slice: true,
                mutable: ty.is_mut,
            };
        }
        // **What the C boundary lends** (ADR-147 D1), read before the tuple for
        // its reason: the element sits where a tuple's parts sit, and the
        // branches below would read it as an argument of a type called `slice`.
        if ty.is_slice || ty.is_mut {
            let item = match ty.generics.first() {
                Some(element) => Ty::from_ast(parsed, element),
                // `&mut T`, whose `T` is the name rather than an argument.
                None => Ty::Named {
                    name: parsed.unaliased(parsed.text(ty.name)),
                    args: ty
                        .generics
                        .iter()
                        .map(|g| Ty::from_ast(parsed, g))
                        .collect(),
                    view: false,
                },
            };
            // **A `?` on one of these is the *pointee's***
            // ([ADR-155](../../../docs/specification/adr/adr-155.md) D5), which
            // is where it parts company with Part I 2.3's own reading of a
            // trailing `?`. A view at the C boundary lives for the call and is
            // never absent, so a nullable view would be a shape nothing writes;
            // `&mut sqlite3?` is the **out-parameter** — a slot that holds a
            // handle or nothing — and that is what every C library with one
            // means by it.
            let item = match ty.is_nullable {
                true => Ty::Nullable(Box::new(item)),
                false => item,
            };
            return Ty::Pointed {
                item: Box::new(item),
                slice: ty.is_slice,
                mutable: ty.is_mut,
            };
        }
        if ty.is_tuple {
            return Ty::Tuple(
                ty.generics
                    .iter()
                    .map(|g| Ty::from_ast(parsed, g))
                    .collect(),
            );
        }
        // **A parameter that is code**
        // ([ADR-102](../../../docs/specification/adr/adr-102.md) D1), read
        // before the `?` for the tuple's reason: what a `fn(…)?` would mean is
        // not written anywhere, and the grammar gives the form no `?` to begin
        // with.
        if let Some(code) = &ty.code {
            return Ty::Fn {
                params: ty
                    .generics
                    .iter()
                    .map(|g| Ty::from_ast(parsed, g))
                    .collect(),
                result: code
                    .result
                    .as_ref()
                    .map(|r| Box::new(Ty::from_ast(parsed, r))),
                is_sync: code.is_sync,
                throws: code.throws,
            };
        }
        // The `?` wraps whatever the rest of the declaration says, so it is
        // read last here and first in `parse` - the same order either way round.
        if ty.is_nullable {
            let inner = ast::Type {
                is_nullable: false,
                ..ty.clone()
            };
            return Ty::Nullable(Box::new(Ty::from_ast(parsed, &inner)));
        }
        // `unaliased`, because a type may be written with this file's own
        // name for the package that declares it - `h::Request` where the file
        // wrote `use http as h`
        // ([ADR-046](../../../../docs/specification/adr/adr-046.md) D3). One
        // call here rather than one at every reader of a type, which is why the
        // map is on `Parsed` and not on a pass of its own.
        let name = parsed.unaliased(parsed.text(ty.name));
        // **Text that is a view or its own per value is read as text of its
        // own** ([ADR-223](../../../docs/specification/adr/adr-223.md) D3):
        // below it is `EitherText`, which is lent as a `&str` and copied by
        // `.clone()` exactly as a `String` is. What goes *into* one is the
        // checker's to accept per position (`expected_either`,
        // `field_either`).
        if ty.either {
            return Ty::Named {
                name: TEXT.to_string(),
                args: Vec::new(),
                view: false,
            };
        }
        // **A view of `String` is a view of text**
        // ([ADR-184](../../../docs/specification/adr/adr-184.md) D2), the same
        // normalisation [`Ty::parse`] makes at the other door. Text is one type
        // ([ADR-107](../../../docs/specification/adr/adr-107.md)) and `str` is
        // the compiler's own noun for a view of it — so `ref String` written in
        // a signature and `ref String` printed for a literal have to arrive as
        // one thing, or the message is *this is `ref String`, and the `let`
        // says `ref String`*.
        let name = match (ty.is_view, name.as_str()) {
            (true, TEXT) => TEXT_VIEW.to_string(),
            _ => name,
        };
        Ty::Named {
            name,
            args: ty
                .generics
                .iter()
                .map(|g| Ty::from_ast(parsed, g))
                .collect(),
            view: ty.is_view,
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Unknown => f.write_str("?"),
            // The digits and nothing around them, so `Array[f64, 3]` reads in a
            // message exactly as the source wrote it (Part III, C.1).
            Ty::Count(n) => write!(f, "{n}"),
            // `&mut [u8]` and not the pointer it lowers to, for the same
            // reason: a message names what the program wrote.
            Ty::Pointed {
                item,
                slice,
                mutable,
            } => {
                f.write_str("ref ")?;
                if *mutable {
                    f.write_str("mut ")?;
                }
                // The `?` a nullable pointee carries comes out with it, which
                // is the spelling `parse` above reads back (ADR-155 D5).
                match slice {
                    true => match item.as_ref() {
                        Ty::Nullable(inner) => write!(f, "{ARRAY}[{inner}]?"),
                        item => write!(f, "{ARRAY}[{item}]"),
                    },
                    false => write!(f, "{item}"),
                }
            }
            Ty::Tuple(parts) => {
                let parts: Vec<String> = parts.iter().map(|p| p.to_string()).collect();
                write!(f, "({})", parts.join(", "))
            }
            Ty::Fn {
                params,
                result,
                is_sync,
                throws,
            } => {
                let params: Vec<String> = params.iter().map(|p| p.to_string()).collect();
                write!(f, "fn({})", params.join(", "))?;
                if let Some(result) = result {
                    write!(f, " -> {result}")?;
                }
                if *is_sync {
                    f.write_str(" sync")?;
                }
                if *throws {
                    f.write_str(" throws")?;
                }
                Ok(())
            }
            Ty::Var { name, view } => {
                if *view {
                    f.write_str("ref ")?;
                }
                write!(f, "${name}")
            }
            // `T?` and never `Option[T]`: the program cannot write the second
            // one, so a message must not either (Part III, C.1).
            Ty::Nullable(inner) => write!(f, "{inner}?"),
            Ty::Named { name, args, view } => {
                if *view {
                    f.write_str("ref ")?;
                }
                // **`str` is the compiler's noun and `String` is the
                // language's** ([ADR-184](../../../docs/specification/adr/adr-184.md)
                // D2). A message that named `str` would name a type its reader
                // cannot write, which is
                // [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)
                // about this compiler's own vocabulary rather than about the
                // file below.
                let name = match (*view, name.as_str()) {
                    (true, TEXT_VIEW) => TEXT,
                    _ => name.as_str(),
                };
                f.write_str(name)?;
                if !args.is_empty() {
                    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                    write!(f, "[{}]", args.join(", "))?;
                }
                Ok(())
            }
            Ty::Seq {
                item,
                is_sync,
                pauses,
                throws,
                parallel,
                shape,
            } => {
                let word = match parallel {
                    true => PAR,
                    false => SEQ,
                };
                write!(f, "{word}[{item}]")?;
                if *is_sync {
                    f.write_str(" sync")?;
                }
                // Written back in the order it is read, so a ledger that goes
                // through this file comes out byte for byte (Part III, 13.5).
                if *pauses {
                    write!(f, " {PAUSES}")?;
                }
                if *throws {
                    f.write_str(" throws")?;
                }
                for word in Shape::default().missing(shape) {
                    write!(f, " {word}")?;
                }
                Ok(())
            }
        }
    }
}

/// **What the C boundary lends**
/// ([ADR-147](../../../docs/specification/adr/adr-147.md) D1), read out of its
/// text: `&mut T`, `&[T]` and the two together.
///
/// A plain `&T` is **not** one of these — that is the view every other
/// declaration in this language writes, and it stays a `Named` with `view`.
///
/// A trailing `?` belongs to the **pointee**
/// ([ADR-155](../../../docs/specification/adr/adr-155.md) D5), which is why
/// this is read before `parse` strips one.
/// **The view marker off the front of a written type**
/// ([ADR-184](../../../docs/specification/adr/adr-184.md) D1).
///
/// `ref T` is what this language writes and `&T` is the spelling it replaces —
/// both are read while the corpus moves, and D4 is where the second one leaves.
/// The **word** needs the space after it, for the reason `mut ` does one line
/// down: without it `reference` would be read as a view of `erence`.
pub(crate) fn a_view_of(text: &str) -> Option<&str> {
    if let Some(rest) = text.strip_prefix("ref ") {
        return Some(rest.trim_start());
    }
    text.strip_prefix('&').map(str::trim_start)
}

fn pointed_at(text: &str) -> Option<Ty> {
    let rest = a_view_of(text)?;
    // The space after `mut` is what keeps `mutable` from being read as a type
    // whose name begins with those three letters.
    let (mutable, rest) = match rest.strip_prefix("mut ") {
        Some(shorter) => (true, shorter.trim_start()),
        None => (false, rest),
    };
    let (rest, absent) = match rest.strip_suffix('?') {
        Some(shorter) => (shorter.trim_end(), true),
        None => (rest, false),
    };
    // **`Array[T]` is the run and `[T]` is the spelling it replaces**
    // ([ADR-184](../../../docs/specification/adr/adr-184.md) D3, D4). An
    // `Array[T, N]` is **not** one: the count is what makes it a type laid out
    // inline, so the comma is what tells the two apart and nothing else has to.
    let named = rest
        .strip_prefix(ARRAY)
        .and_then(|args| args.strip_prefix('['))
        .and_then(|args| args.strip_suffix(']'))
        .filter(|args| split_args(args).len() == 1);
    let bracketed = rest.starts_with('[') && rest.ends_with(']');
    let slice = named.is_some() || bracketed;
    if !mutable && !slice {
        return None;
    }
    let inner = match (named, bracketed) {
        (Some(args), _) => args,
        (None, true) => &rest[1..rest.len() - 1],
        (None, false) => rest,
    };
    let item = Ty::parse(inner);
    let item = match absent {
        true => Ty::Nullable(Box::new(item)),
        false => item,
    };
    Some(Ty::Pointed {
        item: Box::new(item),
        slice,
        mutable,
    })
}

/// Whether a value of this type lends a **run** of `item` laid out in memory
/// ([ADR-147](../../../docs/specification/adr/adr-147.md) D1).
///
/// What a `&[u8]` parameter may be handed: a list, a fixed-size array, and -
/// where the element is `u8` - text, which is a run of bytes and is what every
/// C function taking a `char *` is given. `Unknown` says nothing, here as
/// everywhere.
fn lends_a_run_of(found: &Ty, item: &Ty) -> bool {
    match found {
        Ty::Unknown => true,
        Ty::Named { name, args, .. } => match (name.as_str(), args.as_slice()) {
            ("Vec" | "List", [element]) => element.fits(item),
            (ARRAY, [element, _]) => element.fits(item),
            // **A slice is a run already** (ADR-215 D3): `ref xs[1..<3]` and a
            // list's `windows` hand out `ref Array[T]`, which is what a
            // declaration writes to take one.
            (ARRAY, [element]) => element.fits(item),
            // Text is a run of bytes, and `u8` is what a declaration writes for
            // one. Both spellings, because a `&str` and a `String` hand over
            // the same bytes.
            ("String" | "str", []) => matches!(item, Ty::Named { name, .. } if name == "u8"),
            _ => false,
        },
        _ => false,
    }
}

/// Split `a, b[c, d], (e, f)` on the commas that are not inside a bracket.
pub fn split_args(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '[' | '(' => {
                depth += 1;
                current.push(c);
            }
            ']' | ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts.iter().map(|p| p.trim().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A type is written the way the source writes it, and reads back the same.
    #[test]
    fn a_type_round_trips_through_its_text() {
        for text in [
            "i32",
            "ref String",
            "String",
            "Vec[Row]",
            "HashMap[ref String, Stats]",
            "(Op, i64)",
            "Vec[(ref String, i64)]",
            "?",
        ] {
            let ty = Ty::parse(text);
            assert_eq!(ty.text(), text, "{text}");
            assert_eq!(Ty::parse(&ty.text()), ty, "{text}");
        }
    }

    /// **The spelling `ref` replaces still reads back, and reads back as the
    /// same type** ([ADR-184](../../../docs/specification/adr/adr-184.md) D1,
    /// D4). Both parse while the corpus and the pages move; what comes out is
    /// the new spelling, because there is one and a ledger carries it.
    ///
    /// `&String` is here for D2: a view of text written either way is one
    /// type, and it was two until the normalisation stood at **both** doors.
    #[test]
    fn the_spelling_it_replaces_reads_back_as_the_same_type() {
        for (old, new) in [
            ("&str", "ref String"),
            ("&String", "ref String"),
            ("&Stats", "ref Stats"),
            ("&mut sqlite3", "ref mut sqlite3"),
            ("&[u8]", "ref Array[u8]"),
            ("ref [u8]", "ref Array[u8]"),
            ("&$V", "ref $V"),
            ("HashMap[&str, Stats]", "HashMap[ref String, Stats]"),
        ] {
            assert_eq!(Ty::parse(old), Ty::parse(new), "{old} and {new}");
            assert_eq!(Ty::parse(old).text(), new, "{old}");
        }
    }

    /// Unknown fits everything, in both directions. That is what lets the
    /// checker say nothing about the half of `std` that is Rust.
    #[test]
    fn unknown_fits_anything() {
        let known = Ty::named("i32");
        assert!(Ty::Unknown.fits(&known));
        assert!(known.fits(&Ty::Unknown));
        assert!(Ty::Unknown.fits(&Ty::Unknown));
    }

    /// Two known types fit only when they are written the same. There is no
    /// implicit widening: Nikaia states that a conversion is written, and a
    /// checker that allowed it here would be checking a different language.
    #[test]
    fn two_known_types_fit_only_when_equal() {
        assert!(Ty::named("i32").fits(&Ty::named("i32")));
        assert!(!Ty::named("i32").fits(&Ty::named("i64")));
        assert!(!Ty::view("str").fits(&Ty::named("String")));
        assert!(Ty::parse("Vec[Row]").fits(&Ty::parse("Vec[Row]")));
        assert!(!Ty::parse("Vec[Row]").fits(&Ty::parse("Vec[Hit]")));
    }

    /// …and an unknown *part* is still no claim about the whole.
    #[test]
    fn an_unknown_argument_makes_the_whole_fit() {
        assert!(Ty::parse("Vec[?]").fits(&Ty::parse("Vec[Row]")));
        assert!(Ty::parse("(i32, ?)").fits(&Ty::parse("(i32, String)")));
        assert!(!Ty::parse("(i32, ?)").fits(&Ty::parse("(String, String)")));
    }
}

#[cfg(test)]
mod fn_type_tests {
    use super::*;

    /// A function type reads back the way it was written (ADR-029).
    #[test]
    fn a_function_type_round_trips() {
        for text in ["fn()", "fn(ref Stats)", "fn(ref String, i64)", "fn(?)"] {
            assert_eq!(Ty::parse(text).text(), text, "{text}");
        }
    }

    /// `fn(&Stats)` is not a type named `fn(&Stats)`.
    ///
    /// The parse is ordered so that a function type is recognised before the
    /// `&` and the `[…]` are looked for; without that it fell through to
    /// `Named` and the whole spelling became a name.
    #[test]
    fn a_function_type_is_not_a_name() {
        let parsed = Ty::parse("fn(&Stats)");
        assert!(matches!(parsed, Ty::Fn { .. }), "{parsed:?}");
        let Ty::Fn { params, .. } = parsed else {
            unreachable!("just matched")
        };
        assert_eq!(params, vec![Ty::view("Stats")]);
    }

    /// It fits another lambda of the same shape, and nothing else that is
    /// written down.
    #[test]
    fn a_function_type_fits_its_own_shape() {
        let one = Ty::parse("fn(&Stats)");
        assert!(one.fits(&Ty::parse("fn(&Stats)")));
        assert!(!one.fits(&Ty::parse("fn(i64)")));
        assert!(!one.fits(&Ty::parse("fn()")));
        // A named type and a lambda are different claims.
        assert!(!one.fits(&Ty::named("Stats")));
        assert!(!Ty::named("Stats").fits(&one));
        // `?` is the absence of a claim, so it still fits both ways.
        assert!(one.fits(&Ty::Unknown));
        assert!(Ty::Unknown.fits(&one));
    }
}

/// Bind a library signature's type variables from the receiver's actual type
/// (ADR-031).
///
/// **One level, positional, receiver only.** `HashMap[$K, $V]` against
/// `HashMap[&str, Stats]` binds `$K` and `$V`; a pattern that is not a variable
/// is compared no further, and nothing is bound from an argument. That is not
/// an implementation shortcut - it is the decision. Binding from arguments and
/// matching nested patterns is where a signature language grows into a
/// unification algorithm, and each step of that wants its own reason.
///
/// A mismatch binds nothing rather than failing. This is not a check: the
/// question is what the receiver can *tell* the signature, and a receiver that
/// tells it nothing leaves the variables unbound, which `substitute` turns into
/// `?`.
pub fn bind(pattern: &Ty, actual: &Ty, out: &mut std::collections::BTreeMap<String, Ty>) {
    match (pattern, actual) {
        // The `&` in `&$V` says how the *method* takes it, not what the
        // receiver holds, so it is dropped when binding and reapplied when
        // substituting.
        (Ty::Var { name, .. }, actual) => {
            out.entry(name.clone()).or_insert_with(|| actual.clone());
        }
        (
            Ty::Named {
                name: pattern_name,
                args: pattern_args,
                ..
            },
            Ty::Named {
                name: actual_name,
                args: actual_args,
                ..
            },
        ) if pattern_name == actual_name && pattern_args.len() == actual_args.len() => {
            for (pattern, actual) in pattern_args.iter().zip(actual_args) {
                bind(pattern, actual, out);
            }
        }
        // **A produced sequence binds through its item**
        // ([ADR-105](../../../../docs/specification/adr/adr-105.md) D1), so
        // `Seq::collect(Seq[$T]) -> Vec[$T]` says what a chain hands on. The two
        // words are not compared: they are what a **step** may do, and a
        // signature writes the ones its own steps have rather than a demand on
        // the receiver. `Par` binds against `Seq` for D3's *otherwise `Par[T]`
        // has `Seq[T]`'s surface* - the entry a `Par` falls back to is written
        // with a `Seq` receiver and has to bind against the value that reached
        // it.
        (Ty::Seq { item: pattern, .. }, Ty::Seq { item: actual, .. }) => bind(pattern, actual, out),
        // **A lambda binds through its parameters and what it comes to**
        // ([ADR-212](../../../../docs/specification/adr/adr-212.md) D5):
        // `Seq::map(Seq[$T], f: fn($T) -> $U) -> Seq[$U]` learns `$U` from the
        // lambda it was handed, which is how a chain keeps its element type.
        (
            Ty::Fn {
                params: pattern_params,
                result: pattern_result,
                ..
            },
            Ty::Fn {
                params: actual_params,
                result: actual_result,
                ..
            },
        ) => {
            for (pattern, actual) in pattern_params.iter().zip(actual_params) {
                bind(pattern, actual, out);
            }
            if let (Some(pattern), Some(actual)) = (pattern_result, actual_result) {
                bind(pattern, actual, out);
            }
        }
        // The view flag is deliberately not compared: `&HashMap[$K, $V]` must
        // bind against a `HashMap[…]` held by value and the other way round,
        // because a signature writes the receiver the way the method takes it
        // and a caller holds it however it holds it.
        _ => {}
    }
}

/// Replace a signature's variables with what the receiver bound them to.
///
/// **An unbound variable becomes `Unknown`, never a name.** That is the whole
/// safety argument for [`Ty::Var`] and the reason this does not contradict
/// ADR-024 D4: a variable never survives into a comparison, so the checker is
/// never in a position to report that `i32` is not `$V`.
/// Every named type in `ty` that `declared` lists, written with `module::` in
/// front of it.
///
/// **A type's name in the ledger is the name a caller writes** (ADR-011 D2, the
/// same rule that makes `fs::map` the key rather than `map`). A module's own
/// signature says `-> Conn`, because that is how the file that declares it writes
/// it, and the ledger keys the type `pool::Conn` - so a caller annotating
/// `pool::Conn` and calling `pool::make()` was told the two were different types.
/// They are one type with two spellings, and this is where the spelling is made
/// one: at the moment a unit is absorbed into the program's ledger, which is the
/// only place that knows both the module and what it declares.
///
/// `declared` is the unit's **own** type names, so a name from somewhere else is
/// left alone: a `-> Row` whose `Row` this module did not declare is not this
/// module's `Row`.
pub fn qualify(ty: &Ty, module: &str, declared: &std::collections::BTreeSet<String>) -> Ty {
    match ty {
        Ty::Named { name, args, view } => Ty::Named {
            name: match declared.contains(name) {
                true => format!("{module}::{name}"),
                false => name.clone(),
            },
            args: args.iter().map(|a| qualify(a, module, declared)).collect(),
            view: *view,
        },
        Ty::Tuple(parts) => Ty::Tuple(parts.iter().map(|p| qualify(p, module, declared)).collect()),
        Ty::Fn {
            params,
            result,
            is_sync,
            throws,
        } => Ty::Fn {
            params: params
                .iter()
                .map(|p| qualify(p, module, declared))
                .collect(),
            result: result
                .as_ref()
                .map(|r| Box::new(qualify(r, module, declared))),
            is_sync: *is_sync,
            throws: *throws,
        },
        other => other.clone(),
    }
}

/// A type named through one package's word for another package, renamed to the
/// word this build uses ([ADR-053](../../../docs/specification/adr/adr-053.md)
/// D2).
///
/// Only the **prefix** is touched, and only where the map has it: `c::Id`
/// becomes `deep::Id` where `c` and `deep` are two manifest keys for one
/// directory, and every other name is handed back exactly as it came. A name
/// with no `::` in it names nothing outside its package and is never a
/// candidate.
pub fn renamed(ty: &Ty, renames: &std::collections::BTreeMap<String, String>) -> Ty {
    match ty {
        Ty::Named { name, args, view } => Ty::Named {
            // A `&` in front is a view's spelling and belongs to the type rather
            // than to the package, so it is put back where it was.
            name: match name.strip_prefix('&') {
                Some(rest) => format!("&{}", rename_path(rest, renames)),
                None => rename_path(name, renames),
            },
            args: args.iter().map(|a| renamed(a, renames)).collect(),
            view: *view,
        },
        Ty::Tuple(parts) => Ty::Tuple(parts.iter().map(|p| renamed(p, renames)).collect()),
        Ty::Fn {
            params,
            result,
            is_sync,
            throws,
        } => Ty::Fn {
            params: params.iter().map(|p| renamed(p, renames)).collect(),
            result: result.as_ref().map(|r| Box::new(renamed(r, renames))),
            is_sync: *is_sync,
            throws: *throws,
        },
        Ty::Nullable(inner) => Ty::Nullable(Box::new(renamed(inner, renames))),
        other => other.clone(),
    }
}

/// The index of the `)` that closes a `fn(` already stripped from the front,
/// or nothing where there is none.
///
/// Counted rather than searched from the end, because
/// [ADR-102](../../../docs/specification/adr/adr-102.md) D1 lets a function
/// type stand wherever a type may — including inside another one's parameters —
/// and `fn(fn(i64)) -> i64`'s last `)` closes the wrong thing. Brackets are
/// counted with it for a `Vec[fn(i64)]`.
fn closing_paren(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (at, c) in text.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ']' => depth = depth.checked_sub(1)?,
            ')' => match depth {
                0 => return Some(at),
                _ => depth -= 1,
            },
            _ => {}
        }
    }
    None
}

/// The `]` that closes the `[` this text is the inside of, if there is one.
///
/// [`closing_paren`]'s twin, for
/// [ADR-105](../../../../docs/specification/adr/adr-105.md) D1's `Seq[T] sync`:
/// the tail begins after the bracket, so the bracket has to be **matched**
/// rather than found at the end - `Seq[HashMap[$K, $V]] sync` has one in the
/// middle.
fn closing_bracket(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (at, c) in text.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' => depth = depth.checked_sub(1)?,
            ']' => match depth {
                0 => return Some(at),
                _ => depth -= 1,
            },
            _ => {}
        }
    }
    None
}

/// `text` without a trailing `word`, where what is left ends at a boundary.
///
/// The boundary is what keeps a type from being clipped: a result called
/// `Resync` ends in `sync` and is not one.
fn word_off<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let shorter = text.strip_suffix(word)?;
    match shorter.chars().next_back() {
        None => Some(shorter),
        Some(c) if c.is_whitespace() => Some(shorter.trim_end()),
        Some(_) => None,
    }
}

fn rename_path(name: &str, renames: &std::collections::BTreeMap<String, String>) -> String {
    match name.split_once("::") {
        Some((package, rest)) => match renames.get(package) {
            Some(ours) => format!("{ours}::{rest}"),
            None => name.to_string(),
        },
        None => name.to_string(),
    }
}

pub fn substitute(ty: &Ty, bound: &std::collections::BTreeMap<String, Ty>) -> Ty {
    match ty {
        Ty::Var { name, view } => match bound.get(name) {
            Some(Ty::Named { name, args, .. }) if *view => Ty::Named {
                name: name.clone(),
                args: args.clone(),
                view: true,
            },
            Some(bound) => bound.clone(),
            None => Ty::Unknown,
        },
        Ty::Named { name, args, view } => Ty::Named {
            name: name.clone(),
            args: args.iter().map(|a| substitute(a, bound)).collect(),
            view: *view,
        },
        Ty::Tuple(parts) => Ty::Tuple(parts.iter().map(|p| substitute(p, bound)).collect()),
        Ty::Count(n) => Ty::Count(*n),
        Ty::Pointed {
            item,
            slice,
            mutable,
        } => Ty::Pointed {
            item: Box::new(substitute(item, bound)),
            slice: *slice,
            mutable: *mutable,
        },
        Ty::Fn {
            params,
            result,
            is_sync,
            throws,
        } => Ty::Fn {
            params: params.iter().map(|p| substitute(p, bound)).collect(),
            result: result.as_ref().map(|r| Box::new(substitute(r, bound))),
            is_sync: *is_sync,
            throws: *throws,
        },
        Ty::Seq {
            item,
            is_sync,
            pauses,
            throws,
            parallel,
            shape,
        } => Ty::Seq {
            item: Box::new(substitute(item, bound)),
            is_sync: *is_sync,
            pauses: *pauses,
            throws: *throws,
            parallel: *parallel,
            shape: *shape,
        },
        Ty::Nullable(inner) => Ty::Nullable(Box::new(substitute(inner, bound))),
        Ty::Unknown => Ty::Unknown,
    }
}

#[cfg(test)]
mod variable_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn bound_from(pattern: &str, actual: &str) -> BTreeMap<String, Ty> {
        let mut out = BTreeMap::new();
        bind(&Ty::parse(pattern), &Ty::parse(actual), &mut out);
        out
    }

    /// A variable reads back the way it is written, and is not a name.
    #[test]
    fn a_variable_round_trips_and_is_not_a_name() {
        assert_eq!(
            Ty::parse("$V"),
            Ty::Var {
                name: "V".to_string(),
                view: false
            }
        );
        assert_eq!(Ty::parse("ref $V").text(), "ref $V");
        assert_eq!(Ty::parse("$V").text(), "$V");
        assert_eq!(Ty::parse("Entry[$V]").text(), "Entry[$V]");
        assert_eq!(Ty::parse("fn(ref $V)").text(), "fn(ref $V)");
        // A type genuinely called `V` is still a type called `V`.
        assert_eq!(Ty::parse("V"), Ty::named("V"));
    }

    /// The receiver binds the variables, one level and by position.
    #[test]
    fn the_receiver_binds_what_the_signature_names() {
        let bound = bound_from("&HashMap[$K, $V]", "HashMap[&str, Stats]");
        assert_eq!(bound["K"], Ty::view("str"));
        assert_eq!(bound["V"], Ty::named("Stats"));
    }

    /// A receiver that says nothing binds nothing, and nothing is `?`.
    ///
    /// `let m = HashMap::new()` gives `HashMap[?, ?]`, and the honest answer
    /// downstream is "no claim" rather than a guess.
    #[test]
    fn a_receiver_that_says_nothing_leaves_the_variables_unbound() {
        let bound = bound_from("&HashMap[$K, $V]", "HashMap[?, ?]");
        assert_eq!(
            substitute(&Ty::parse("Entry[$V]"), &bound),
            Ty::parse("Entry[?]")
        );

        // A different type altogether binds nothing at all.
        let none = bound_from("&HashMap[$K, $V]", "Vec[i64]");
        assert!(none.is_empty());
        assert_eq!(substitute(&Ty::parse("$V"), &none), Ty::Unknown);
    }

    /// An unbound variable becomes `?` - never a type called `$V`.
    ///
    /// This is the property ADR-024 D4 was protecting when it erased a generic
    /// to `?`, kept here by substitution rather than by erasure.
    #[test]
    fn an_unbound_variable_becomes_unknown() {
        let empty = BTreeMap::new();
        assert_eq!(substitute(&Ty::parse("$V"), &empty), Ty::Unknown);
        assert_eq!(
            substitute(&Ty::parse("fn(&$V)"), &empty),
            Ty::parse("fn(?)"),
        );
        // And it fits anything, so a leak cannot become a false rejection.
        assert!(Ty::parse("$V").fits(&Ty::named("i32")));
        assert!(Ty::named("i32").fits(&Ty::parse("$V")));
    }

    /// The chain the whole decision exists for.
    ///
    /// `HashMap[&str, Stats]` → `Entry[Stats]` → `fn(&Stats)`, which is what
    /// gives the `a` in `.and_modify fn { a.add(t) }` a type.
    #[test]
    fn the_chain_from_a_map_to_a_lambda_parameter() {
        let at_entry = bound_from("&HashMap[$K, $V]", "HashMap[&str, Stats]");
        let entry = substitute(&Ty::parse("Entry[$V]"), &at_entry);
        assert_eq!(entry, Ty::parse("Entry[Stats]"));

        let at_and_modify = bound_from("Entry[$V]", &entry.text());
        let lambda = substitute(&Ty::parse("fn(&$V)"), &at_and_modify);
        assert_eq!(lambda, Ty::parse("fn(&Stats)"));
    }

    /// The view flag does not stop a binding.
    ///
    /// A signature writes the receiver the way the method takes it, and a
    /// caller holds it however it holds it; requiring the two to agree would
    /// make every `&`-taking method fail to bind.
    #[test]
    fn a_view_binds_against_a_value() {
        let bound = bound_from("&Vec[$T]", "Vec[i64]");
        assert_eq!(bound["T"], Ty::named("i64"));
    }
}
