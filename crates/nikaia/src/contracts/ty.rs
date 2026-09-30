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

use crate::ast;
use crate::parser::Parsed;

/// `Ty` and `Shape` are declared in Nikaia ([ADR-257](../../../docs/specification/adr/adr-257.md)
/// D1): `nikaia-std/src/tools/ty.nika`, with the words below and the text a
/// type is written as. What stays here is what the checker does with them, as
/// [`TyOps`], until ADR-257's step (d) moves it too.
pub use nikaia_std::tools::ty::{
    ARRAY, ENDS, PAR, PAUSES, REPLAYS, SEQ, SIZED, Shape, TEXT, TEXT_VIEW, Ty, split_args,
};

/// The stamp a lock puts on what it hands out
/// ([ADR-111](../../../../docs/specification/adr/adr-111.md) D1).
pub const SEEN: &str = "Seen";

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

/// **What the checker asks of a type**: the constructors and questions that
/// were `impl Ty` while the declaration was Rust. An extension trait, because
/// the type is `nikaia-std`'s now and an inherent `impl` belongs to the crate
/// that declares it; a reader brings it in with `use crate::contracts::ty::TyOps`.
pub trait TyOps {
    fn named(name: impl Into<String>) -> Ty;
    fn view(name: impl Into<String>) -> Ty;
    fn as_a_view(&self) -> Ty;
    fn seen(inner: Ty) -> Ty;
    fn is_seen(&self) -> bool;
    fn unseen(&self) -> Ty;
    fn is_a_view(&self) -> bool;
    fn is_unknown(&self) -> bool;
    fn fits(&self, expected: &Ty) -> bool;
    fn parse(text: &str) -> Ty;
    fn erase(&self, parameters: &BTreeSet<String>) -> Ty;
    fn parameterise(&self, parameters: &BTreeSet<String>) -> Ty;
    fn from_ast(parsed: &Parsed, ty: &ast::Type) -> Ty;
}

impl TyOps for Ty {
    fn named(name: impl Into<String>) -> Ty {
        Ty::Named {
            name: name.into(),
            args: Vec::new(),
            view: false,
        }
    }

    fn view(name: impl Into<String>) -> Ty {
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
    fn as_a_view(&self) -> Ty {
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
    fn seen(inner: Ty) -> Ty {
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
    fn is_seen(&self) -> bool {
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
    fn unseen(&self) -> Ty {
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
    fn is_a_view(&self) -> bool {
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

    fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    /// Whether a value of this type may stand where `expected` is wanted.
    ///
    /// Equality, plus the rule that makes the checker usable: **anything
    /// involving `Unknown` fits.** There is no subtyping and no implicit
    /// widening - Nikaia states that a conversion is written (ADR-013 D7), and
    /// a checker that quietly allowed `i32` where `i64` is wanted would be
    /// checking a different language from the one the specification describes.
    fn fits(&self, expected: &Ty) -> bool {
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
                    can_throw: at,
                },
                Ty::Fn {
                    params: b,
                    result: br,
                    is_sync: bsy,
                    can_throw: bt,
                },
            ) => {
                a.len() == b.len()
                    && a.iter().zip(b).all(|(a, b)| a.fits(b))
                    && match (&**ar, &**br) {
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
                    can_throw: at,
                    parallel: apar,
                    shape: ashape,
                },
                Ty::Seq {
                    item: b,
                    is_sync: bsy,
                    pauses: bp_,
                    can_throw: bt,
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

    /// Read one back: `tools/ty.nika`'s `parse` (ADR-257 step (c)), which reads
    /// what [`Ty::text`] writes and the older spellings a ledger may still hold.
    fn parse(text: &str) -> Ty {
        nikaia_std::tools::ty::parse(text)
    }

    /// The same type with every name in `parameters` replaced by `Unknown`.
    ///
    /// A generic parameter is a name that stands for a type rather than being
    /// one, and a checker that treated `T` as a type would report that `i32` is
    /// not `T` - which is the shape of a false positive this checker exists not
    /// to produce. Erasing them says exactly as much as Stage 0 knows: a
    /// generic function's parameters are checked for *number* and not for type.
    fn erase(&self, parameters: &BTreeSet<String>) -> Ty {
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
                can_throw: throws,
            } => Ty::Fn {
                params: params.iter().map(|p| p.erase(parameters)).collect(),
                result: Box::new((**result).as_ref().map(|r| r.erase(parameters))),
                is_sync: *is_sync,
                can_throw: *throws,
            },
            // The two words are the **step's** and not the item's, so they
            // travel unchanged while the item is rewritten
            // ([ADR-105](../../../../docs/specification/adr/adr-105.md) D1).
            Ty::Seq {
                item,
                is_sync,
                pauses,
                can_throw: throws,
                parallel,
                shape,
            } => Ty::Seq {
                item: Box::new(item.erase(parameters)),
                is_sync: *is_sync,
                pauses: *pauses,
                can_throw: *throws,
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
    fn parameterise(&self, parameters: &BTreeSet<String>) -> Ty {
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
                can_throw: throws,
            } => Ty::Fn {
                params: params.iter().map(|p| p.parameterise(parameters)).collect(),
                result: Box::new((**result).as_ref().map(|r| r.parameterise(parameters))),
                is_sync: *is_sync,
                can_throw: *throws,
            },
            // The two words are the **step's** and not the item's, so they
            // travel unchanged while the item is rewritten
            // ([ADR-105](../../../../docs/specification/adr/adr-105.md) D1).
            Ty::Seq {
                item,
                is_sync,
                pauses,
                can_throw: throws,
                parallel,
                shape,
            } => Ty::Seq {
                item: Box::new(item.parameterise(parameters)),
                is_sync: *is_sync,
                pauses: *pauses,
                can_throw: *throws,
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
    fn from_ast(parsed: &Parsed, ty: &ast::Type) -> Ty {
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
        if let Some(code) = &*ty.code {
            return Ty::Fn {
                params: ty
                    .generics
                    .iter()
                    .map(|g| Ty::from_ast(parsed, g))
                    .collect(),
                result: Box::new((*code.result).as_ref().map(|r| Ty::from_ast(parsed, r))),
                is_sync: code.is_sync,
                can_throw: code.can_throw,
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
            if let (Some(pattern), Some(actual)) = (&**pattern_result, &**actual_result) {
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
            can_throw: throws,
        } => Ty::Fn {
            params: params
                .iter()
                .map(|p| qualify(p, module, declared))
                .collect(),
            result: Box::new((**result).as_ref().map(|r| qualify(r, module, declared))),
            is_sync: *is_sync,
            can_throw: *throws,
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
            can_throw: throws,
        } => Ty::Fn {
            params: params.iter().map(|p| renamed(p, renames)).collect(),
            result: Box::new((**result).as_ref().map(|r| renamed(r, renames))),
            is_sync: *is_sync,
            can_throw: *throws,
        },
        Ty::Nullable(inner) => Ty::Nullable(Box::new(renamed(inner, renames))),
        other => other.clone(),
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
            can_throw: throws,
        } => Ty::Fn {
            params: params.iter().map(|p| substitute(p, bound)).collect(),
            result: Box::new((**result).as_ref().map(|r| substitute(r, bound))),
            is_sync: *is_sync,
            can_throw: *throws,
        },
        Ty::Seq {
            item,
            is_sync,
            pauses,
            can_throw: throws,
            parallel,
            shape,
        } => Ty::Seq {
            item: Box::new(substitute(item, bound)),
            is_sync: *is_sync,
            pauses: *pauses,
            can_throw: *throws,
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
