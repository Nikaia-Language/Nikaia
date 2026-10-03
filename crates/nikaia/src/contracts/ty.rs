// crates/nikaia/src/contracts/ty.rs
//
// The type language the checker reasons in, and the one the ledger records.
//
// It is `ast::Type` with one addition and one subtraction. The addition is
// `Unknown`, which is not a type but the absence of a claim - see below. The
// subtraction is the interner: a ledger is a file, so a name here is text, and
// two types are the same when they are written the same. That is name-for-name
// (ADR-296 D17) applied to types: nothing resolves a module or an alias, so
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

use crate::ast;
use crate::parser::Parsed;

/// `Ty` and `Shape` are declared in Nikaia ([ADR-294](../../../docs/specification/adr/adr-294.md)
/// D1): `nikaia-std/src/tools/ty.nika`, with the words below, the text a type
/// is written as, and what the checker asks of one.
pub use nikaia_std::tools::ty::{
    ARRAY, ENDS, PAR, PAUSES, REPLAYS, SEEN, SEQ, SIZED, Shape, TEXT, TEXT_VIEW, Ty, bind, qualify,
    renamed, split_args, substitute,
};

/// **What `T::fields` walks**, one element of it
/// ([ADR-304](../../../docs/specification/adr/adr-304.md) D2,
/// [ADR-304](../../../docs/specification/adr/adr-304.md)).
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
/// **Since [ADR-313](../../../docs/specification/adr/adr-313.md) D3 a `std`
/// type carries its module**: `collections::HashMap`, `time::Duration`,
/// `foreign::CStr`. The module says where the name is *reached from* and the
/// last segment is the type, so every rule here that names a type by hand — the
/// copy list, the crossing analysis's containers, the hash a map gets — asks
/// this rather than the written name.
///
/// Asked for every key of every ledger in some walks, so the last `:` is found
/// by its byte rather than by a searcher for `"::"` built per call; a `:` that
/// is not half of a `::` - which no key writes - takes the general search.
pub fn base(name: &str) -> &str {
    match name.rfind(':') {
        None => name,
        Some(at) if at > 0 && name.as_bytes()[at - 1] == b':' => &name[at + 1..],
        Some(_) => name.rsplit_once("::").map_or(name, |(_, last)| last),
    }
}

/// **What stays Rust of a type**: its constructors from a name, which take
/// anything that becomes text, reading one back (`tools/ty.nika`'s `parse`),
/// and the type a `.nika` declaration names, which reads this compiler's tree.
/// What the checker asks of a type (`fits`, `erase`, `is_a_view`, …) and the
/// walks over one (`bind`, `substitute`, `qualify`, `renamed`) are
/// `tools/ty.nika`'s since ADR-294 step (d) (0.0.295). An extension trait,
/// because the type is `nikaia-std`'s and an inherent `impl` belongs to the
/// crate that declares it.
pub trait TyOps {
    fn named(name: impl Into<String>) -> Ty;
    fn view(name: impl Into<String>) -> Ty;
    fn seen(inner: Ty) -> Ty;
    fn parse(text: &str) -> Ty;
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

    /// **What a lock handed out**
    /// ([ADR-281](../../../../docs/specification/adr/adr-281.md) D22).
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

    /// Read one back: `tools/ty.nika`'s `parse` (ADR-294 step (c)), which reads
    /// what [`Ty::text`] writes and the older spellings a ledger may still hold.
    fn parse(text: &str) -> Ty {
        nikaia_std::tools::ty::parse(text)
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
        // **What the C boundary lends** (ADR-302 D5), read before the tuple for
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
            // ([ADR-302](../../../docs/specification/adr/adr-302.md) D14), which
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
        // ([ADR-277](../../../docs/specification/adr/adr-277.md) D6), read
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
        // ([ADR-286](../../../../docs/specification/adr/adr-286.md) D12). One
        // call here rather than one at every reader of a type, which is why the
        // map is on `Parsed` and not on a pass of its own.
        let name = parsed.unaliased(parsed.text(ty.name));
        // **Text that is a view or its own per value is read as text of its
        // own** ([ADR-282](../../../docs/specification/adr/adr-282.md) D15):
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
        // ([ADR-282](../../../docs/specification/adr/adr-282.md)) and `str` is
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

    /// A function type reads back the way it was written (ADR-288).
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
