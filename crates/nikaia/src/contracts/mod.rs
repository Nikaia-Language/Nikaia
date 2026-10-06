// crates/nikaia/src/contracts/mod.rs
//
// The Borrow Contract Ledger (Part III, 13.5; ADR-005 D3; ADR-020).
//
// What a caller has to know about a function it cannot see the body of: does it
// pause, can it fail, and does what it hands back point into what it was given.
// Part III 13.5 specifies a file that records exactly that, derived rather than
// written, committed like a lockfile, and **shipped with a published package**
// so that a consumer builds against contracts instead of guesses.
//
// This is that file, for what Stage 0 knows, and the three answers come from
// three different places.
//
// `throws` is *declared* in the source and recorded exactly. The borrow
// contract is *inferred from the signature* - the widest one a signature can
// support - rather than by the whole-program analysis ADR-005 D3 describes.
// `sync` is *inferred from the body* (ADR-288): a function that provably cannot
// pause gets the promise whether or not anyone wrote the word, and where the
// word is written it stays an assertion for `NK2202` to check. `sharing` is
// inferred from the bodies too (ADR-037 D7), and it is the one column that can
// only ever get *better*: its floor is the safe answer, so a ledger that says
// nothing about a `Shared` still describes a correct program.
//
// That is why the header names the inference that produced the ledger. A later
// compiler that infers more will write a different name there, and `--locked`
// will say so rather than quietly accepting the weaker answer.

pub mod keep;
pub mod keeps;
pub mod locks;
pub mod order;
pub mod send;
pub mod sharing;
pub mod sync;
pub mod tether;
pub mod throws;
pub mod touch;
pub mod trust;
pub mod ty;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, anyhow};

use crate::ast::{Expr, Item, Stmt};
use crate::contracts::ty::TyOps;
use crate::emit::{borrowing_structs, holds_view, names_borrowing};
use crate::parser::Parsed;

/// **The ledger's small records are declared in Nikaia**
/// ([ADR-294](../../../docs/specification/adr/adr-294.md) step (b)):
/// `nikaia-std/src/tools/ty.nika`, beside the type language their fields
/// hold. Re-exported here so that every reader keeps its path.
pub use nikaia_std::tools::ty::{
    ConfigContract, Crosses, FieldContract, FnContract, Ledger, Lock, Notes, Provenance, Signature,
    Sync, Threads, TypeContract, VariantContract,
};

/// What stays Rust of [`Sync`]: the name a `sync` was taken from.
pub trait SyncOps {
    fn from(&self) -> Option<&str>;
}

impl SyncOps for Sync {
    fn from(&self) -> Option<&str> {
        match self {
            Sync::From(name) => Some(name),
            _ => None,
        }
    }
}

/// The contracts `std` ships, as the library ships them.
///
/// Part III 13.5 has a consumer read a package's ledger from the package.
/// Stage 0's compiler and `std` ship together, so the file is embedded here
/// rather than looked up - the same file, read at build time instead of at run
/// time, and the same one a reviewer reads.
pub const STD: &str = include_str!("../../../nikaia-std/std.contracts");

/// The inference this ledger was produced by.
///
/// Recorded in the header so that a ledger can say what it knows, and a ledger
/// produced by reading signatures must not be mistaken for one produced by
/// reading bodies. Stage 0 reads signatures for the borrow contract and for
/// `throws`; since ADR-288 it reads **bodies** for `sync`, since ADR-023 D1 for
/// the *errors* a `throws` names, and since ADR-037 D7 for `sharing` - the name
/// says which half is which.
/// The whole-program analysis of ADR-005 D3 will read bodies for the borrow
/// contract too and will write a different name again.
pub const INFERENCE: &str = "stage0-signatures+sync-bodies+throws-bodies+sharing-bodies";

/// The format version of the file itself.
///
/// 2 since [ADR-023](../../../../docs/specification/adr/adr-023.md) D1: `throws`
/// was a boolean and is a list of the errors that can leave the function. A
/// version 1 file still reads - `true` is taken as `["?"]`, which is what it
/// always meant - and the version is what tells a *reader* that the file it has
/// may say more than it knows how to use.
pub const VERSION: u32 = 3;

/// What stays Rust of a variant's entry until its text moves (ADR-294 step
/// (c)): reading one back. The declaration and `text` are `tools/ty.nika`'s.
pub trait VariantOps {
    fn parse(text: &str) -> VariantContract;
}

impl VariantOps for VariantContract {
    fn parse(text: &str) -> VariantContract {
        nikaia_std::tools::ledger::variant_of(text)
    }
}

/// What stays Rust of a [`Signature`] (ADR-294 step (b)): the parameters
/// after the receiver, as a slice of the entry's own, and reading one back.
/// The declaration, `text` and the rest are `tools/ty.nika`'s.
pub trait SignatureOps: Sized {
    fn arguments(&self) -> &[(String, ty::Ty)];
    fn parse(text: &str) -> Result<Self>;
}

impl SignatureOps for Signature {
    /// The arguments a *call* passes, which is the parameters after a receiver.
    fn arguments(&self) -> &[(String, ty::Ty)] {
        match self.params.first() {
            Some((name, _)) if name == "self" => &self.params[1..],
            _ => &self.params,
        }
    }

    /// Read one back from the text above.
    fn parse(text: &str) -> Result<Signature> {
        nikaia_std::tools::ledger::signature_of(text).map_err(|refusal| anyhow!("{refusal}"))
    }
}

/// What the compiler does with a ledger: infer, absorb, render and read it
/// back. The record is `tools/ty.nika`'s (ADR-294 D12); these stay Rust until
/// step (c) and (d) move them.
pub trait LedgerOps: Sized {
    fn empty() -> Self;
    fn infer(parsed: &Parsed) -> Self;
    fn infer_package(units: &[&Parsed], library: &Ledger) -> Self;
    fn published(&self, foreign: &BTreeSet<String>) -> Ledger;
    fn stale_against(&self, sources: &BTreeMap<String, String>) -> Vec<String>;
    fn absorb(&mut self, module: Option<&str>, other: Ledger);
    fn absorb_renaming(
        &mut self,
        module: Option<&str>,
        renames: &std::collections::BTreeMap<String, String>,
        other: Ledger,
    );
    fn infer_checked(parsed: &Parsed) -> (Self, crate::check::Checked);
    fn infer_package_checked(
        units: &[&Parsed],
        library: &Ledger,
    ) -> (Self, Vec<crate::check::Checked>);
    /// [`LedgerOps::infer_package_checked`], and what the `sync` inference
    /// learned for the build to say ([`sync::Noted`], ADR-288 D29 and D32).
    fn infer_package_noted(
        units: &[&Parsed],
        library: &Ledger,
    ) -> (Self, Vec<crate::check::Checked>, sync::Noted);
    fn function(
        &self,
        parsed: &Parsed,
        item: &Item,
        target: Option<&str>,
        outer: &BTreeSet<String>,
    ) -> (String, FnContract);
    fn lookup(&self, name: &str) -> Option<(String, &FnContract)>;
    fn candidates(&self, method: &str) -> Vec<(&str, &FnContract)>;
    fn render(&self) -> String;
    fn render_derived(&self) -> String;
    fn read_beside(path: &std::path::Path) -> Option<Ledger>;
    fn render_description(&self, crate_name: &str, version: &str, notes: &Notes) -> String;
    fn render_from(&self, out: &mut String);
    fn render_from_with(&self, out: &mut String, notes: &Notes);
    fn parse(text: &str) -> Result<Self>;
}

impl LedgerOps for Ledger {
    /// The contracts of a parsed program.
    ///
    /// Two passes, because the second needs the first to have finished. The
    /// item loop records what each declaration *says*; then [`sync::infer`]
    /// reads the bodies and gives `sync` to what earns it, which it can only do
    /// once every function in the unit has an entry to be looked up in.
    ///
    /// The library it resolves calls against is `std`'s shipped ledger. It used
    /// to be closed for a reason that has since been answered rather than
    /// abandoned: 13.5 makes this file a pure function of (source, toolchain),
    /// so a ledger inferred against a *different* library would be a different
    /// file for the same source. [ADR-100](../../../docs/specification/adr/adr-100.md)
    /// D1 and D5 make a dependency's ledger a **build input** — one answer with
    /// one author, existing before its consumer is checked — so
    /// [`Self::infer_package`] takes the library and this entry point keeps
    /// `std` alone.
    /// A ledger with a header and nothing in it - what a program of several
    /// files starts from before it absorbs its modules.
    fn empty() -> Self {
        Ledger {
            version: VERSION,
            toolchain: toolchain(),
            inference: INFERENCE.to_string(),
            ..Ledger::blank()
        }
    }

    fn infer(parsed: &Parsed) -> Self {
        Self::infer_checked(parsed).0
    }

    /// The contracts of a **package**: every unit of it, inferred as one graph
    /// ([ADR-100](../../../docs/specification/adr/adr-100.md) D2).
    ///
    /// A package is one namespace ([ADR-286](../../../docs/specification/adr/adr-286.md)
    /// D1), so a call from one of its files to a function in another is a call
    /// this compiler can see - and the inference has to see it too. Inferring
    /// each file alone made a callee in the file next door indistinguishable
    /// from one in code nothing describes: `reach_of` found it in neither `own`
    /// nor `std` and set `blocked`, which `Sync::No` spells *"can pause **or**
    /// could not be vouched for"* and every reader takes as the first. `NK1129`
    /// is where that showed - a trait method refused as pausing for calling a
    /// plain function two lines away in another file.
    ///
    /// The units arrive in the order they were read, which Part III 13.5 needs:
    /// this file is a pure function of the source tree, and the fixpoints below
    /// walk a `BTreeMap` so that the answer does not depend on the order
    /// anyway.
    ///
    /// **`library` is `std`'s ledger and every dependency's**
    /// ([ADR-100](../../../docs/specification/adr/adr-100.md) D1), each under
    /// the name this package reaches it by. A call that leaves the package is
    /// answered from it; what is in neither it nor the package's own entries is
    /// code no ledger describes, and *that* is what fails closed
    /// ([ADR-288](../../../docs/specification/adr/adr-288.md)).
    fn infer_package(units: &[&Parsed], library: &Ledger) -> Self {
        Self::infer_package_checked(units, library).0
    }

    /// This package's **own** entries: what it publishes, with everything that
    /// belongs to a package *it* depends on left out
    /// ([ADR-286](../../../docs/specification/adr/adr-286.md) D21).
    ///
    /// A package's ledger file is the program's record of a whole build, so a
    /// library's carries its own dependencies' entries under their names. A
    /// consumer reading it (ADR-100 D1) must not take those: a transitive
    /// package is deliberately invisible, and absorbing `c::Id` under this
    /// package's name would make `lib::c::Id` — a type nothing declares and
    /// nobody can write. The derived answer has never had them, so this is what
    /// makes the believed one the *same* answer rather than a bigger one.
    ///
    /// `foreign` is the depending build's record of what words this package
    /// uses for packages of its own (`modules::Dependency::reachable`).
    fn published(&self, foreign: &BTreeSet<String>) -> Ledger {
        // In Nikaia (`tools/ledger_ops.nika`, ADR-294, #125).
        nikaia_std::tools::ledger_ops::published_entries(self, foreign)
    }

    /// Whether this ledger may be **believed** for the sources given, and which
    /// units say otherwise ([ADR-100](../../../docs/specification/adr/adr-100.md)
    /// D3).
    ///
    /// Empty means believe it. A non-empty answer names the units that moved,
    /// and the caller derives that package again rather than trusting an entry
    /// derived from a file that is no longer there.
    ///
    /// **A ledger with no `[sources]` at all is never believed against sources
    /// that exist.** It was written before this table or by hand, and the one
    /// polarity this must not get wrong is reading *nothing was recorded* as
    /// *nothing changed* — a stale `touches` is a data race with no message
    /// ([ADR-010](../../../docs/specification/adr/adr-010.md) D1). A package
    /// with a ledger and **no sources** is the third row of D3's table and is
    /// not this function's question: nothing calls it with an empty `sources`.
    fn stale_against(&self, sources: &BTreeMap<String, String>) -> Vec<String> {
        // In Nikaia (`tools/ledger_ops.nika`).
        nikaia_std::tools::ledger_ops::stale_units(self, sources)
    }

    /// Every entry of `other`, under `module::`.
    ///
    /// A program of several files has **one** ledger (Part III, 13.5 puts it at
    /// the project root), and its keys are qualified exactly the way a caller
    /// writes them - which is the shape `std.contracts` has had since ADR-020:
    /// `fs::map`, `io::lines`, `text::digit_value`.
    ///
    /// That is what makes multi-file compilation almost free for everything
    /// built on the ledger. The type checker, the `sync` check, the provenance
    /// analysis, Kap 5.1's options and ADR-025's fallible loops all resolve a
    /// call by asking a ledger for `a::b`. None of them learns a new trick to
    /// work across files; the file they ask simply has more in it.
    /// **The types inside an entry are qualified too, and not only the keys.**
    /// A module's own signature says `-> Conn`, because that is how the file
    /// declaring it writes the name, while its type is keyed `pool::Conn`. A
    /// caller who annotated `pool::Conn` and called `pool::make()` was then told
    /// the two were different types, and the help line asked for what was already
    /// written. They are one type with two spellings, and this is the only place
    /// that knows both the module and what it declares.
    fn absorb(&mut self, module: Option<&str>, other: Ledger) {
        self.absorb_renaming(module, &std::collections::BTreeMap::new(), other)
    }

    /// The same, for a package that writes **its own** word for a package this
    /// build has a word for too
    /// ([ADR-286](../../../docs/specification/adr/adr-286.md) D20).
    ///
    /// Qualifying is about the names a package declares; this is about the names
    /// it *reaches*, and they are the half that used to come out wrong. A library
    /// that depends on the same directory a program depends on writes its own
    /// manifest key in its signatures — `c::Id` where the program wrote
    /// `deep::Id` — and the checker, having only the two spellings, called them
    /// two types. They are one package, so they are one type, and `renames` is
    /// the translation `project::renames_in` computed from the directories.
    ///
    /// Applied **after** qualifying and not instead of it: a name the package
    /// declares itself is its own and is never renamed, because `qualify` has
    /// already put this package's word in front of it.
    fn absorb_renaming(
        &mut self,
        module: Option<&str>,
        renames: &std::collections::BTreeMap<String, String>,
        other: Ledger,
    ) {
        // In Nikaia (`tools/ledger_ops.nika`, ADR-294, #125).
        nikaia_std::tools::ledger_ops::absorb_into(self, module, renames, &other);
    }

    /// The contracts, and the type checker's pass that helped produce them.
    ///
    /// Three steps, and the order is forced:
    ///
    /// 1. read the declarations, so every function has an entry to be looked
    ///    up in;
    /// 2. run the **type checker** against that, which resolves each method
    ///    call to the function it goes to (ADR-288);
    /// 3. infer `sync` from the bodies, using both.
    ///
    /// **Step 2 does not depend on step 3, and that is what makes this sound
    /// rather than circular.** The checker reads `signature`, `fields` and
    /// `iterates` from a ledger and never `sync`, so resolving a method against
    /// the step-1 ledger gives the same answer as resolving it against the
    /// finished one. `the_checker_does_not_depend_on_the_sync_it_helps_infer`
    /// in `tests/contracts.rs` is that invariant, held to.
    ///
    /// The pass is handed back rather than thrown away because the compiler
    /// wants it too - it carries the findings and the fallible loops - and
    /// running the checker twice per build to get one of them would be waste,
    /// not caution.
    fn infer_checked(parsed: &Parsed) -> (Self, crate::check::Checked) {
        let (ledger, mut checked) = Self::infer_package_checked(&[parsed], std_ledger());
        (ledger, checked.remove(0))
    }

    /// The same, over every unit of a package — see [`Self::infer_package`].
    ///
    /// **One `Checked` per unit, in the units' order**, and that is not a
    /// convenience: almost everything that pass answers is keyed by the **byte**
    /// a statement starts at, which names a position in one file and nothing at
    /// all in a package. Only `methods` is keyed by a name, so only `methods` is
    /// merged - and merging it is sound for the reason the package is one graph
    /// in the first place: one namespace, so one function per name
    /// (`modules::collect` refuses the second).
    fn infer_package_checked(
        units: &[&Parsed],
        library: &Ledger,
    ) -> (Self, Vec<crate::check::Checked>) {
        let (ledger, checked, _) = Self::infer_package_noted(units, library);
        (ledger, checked)
    }

    fn infer_package_noted(
        units: &[&Parsed],
        library: &Ledger,
    ) -> (Self, Vec<crate::check::Checked>, sync::Noted) {
        let mut ledger = Ledger {
            version: VERSION,
            toolchain: toolchain(),
            inference: INFERENCE.to_string(),
            ..Ledger::blank()
        };

        // **The package's types and not the file's.** `impl_parameters` asks
        // whether a name in `impl Box[Thing]` is a type or a type *parameter*,
        // and a `Thing` declared in the file next door is a type.
        let declared: BTreeSet<String> = units.iter().flat_map(|u| declared_types(u)).collect();

        // The options whose default is computed rather than written as a
        // literal (ADR-318 D1), evaluated once the entries exist.
        let mut defaults: Vec<Computed<'_>> = Vec::new();
        for parsed in units.iter().copied() {
            // Per unit, and it has to be: a `Symbol` is interned by the parse
            // of one file, so a set of them means nothing to another.
            let borrowing = borrowing_structs(parsed);

            for item in &parsed.program.items {
                match &item.node {
                    Item::Fn { .. } => {
                        let (name, contract) =
                            ledger.function(parsed, &item.node, None, &BTreeSet::new());
                        computed_defaults(&mut defaults, &name, parsed, &item.node);
                        ledger.functions.insert(name, contract);
                    }
                    Item::Impl {
                        target, methods, ..
                    } => {
                        // `impl Stack[T]` puts `T` in scope for every method in it,
                        // so it is a name that stands for a type there too.
                        let outer: BTreeSet<String> = impl_parameters(parsed, target, &declared)
                            .into_iter()
                            .collect();
                        let target = parsed.text(target.name).to_string();
                        // ADR-295 D15: `impl Speaks for Dog` is the claim that a
                        // `Dog` may stand where a `Speaks` is asked for, and it
                        // is the only place that claim is made.
                        if let Item::Impl {
                            trait_name: Some(trait_name),
                            ..
                        } = &item.node
                        {
                            ledger
                                .implementations
                                .entry(parsed.text(*trait_name).to_string())
                                .or_default()
                                .insert(target.clone());
                        }
                        for method in methods {
                            let (name, contract) =
                                ledger.function(parsed, &method.node, Some(&target), &outer);
                            computed_defaults(&mut defaults, &name, parsed, &method.node);
                            ledger.functions.insert(name, contract);
                        }
                    }
                    // Kap 4.7: a trait's methods are recorded under the trait's own
                    // name - `Summarize::summary` - which is what lets a bound be
                    // looked up ([ADR-295](../../../docs/specification/adr/adr-295.md)
                    // D3). The same key shape an `impl`'s methods get, because a
                    // bound and a receiver ask the same question: what does a value
                    // of this thing have.
                    // **An `extern "C"` declaration is an entry like any
                    // other, and reads unlike a trait method**
                    // ([ADR-302](../../../docs/specification/adr/adr-302.md)
                    // D2). Two things are turned around, and only one of them
                    // by this record. It is **`sync`**, asserted rather than
                    // inferred, which is the shape `std`'s own hand-written
                    // entries have for the same reason: the body is in another
                    // language and this compiler does not read it. C has no
                    // suspension point at all, and a C function that sleeps
                    // *blocks* — `println`'s question (ADR-288 D21) and not this
                    // one. And it carries **no `throws`**, which is what a
                    // body-less declaration carries anyway: C has no failure
                    // channel this language reads.
                    //
                    // `touches` and `locks` are absent, which is D4 and is
                    // fail-closed: absent `touches` reads as *touches
                    // everything* (ADR-292) and absent `locks` is that column's
                    // third answer. A C signature says **less** than a Rust
                    // one, not more.
                    Item::Extern { declarations, .. } => {
                        for declaration in declarations {
                            let (_, mut contract) =
                                trait_method(parsed, "", &declaration.node, false);
                            contract.sync_claim = Sync::Asserted;
                            contract.fails_with = Vec::new();
                            ledger
                                .functions
                                .insert(parsed.text(declaration.node.name).to_string(), contract);
                        }
                    }
                    Item::Trait {
                        name,
                        methods,
                        is_public,
                    } => {
                        let own = parsed.text(*name).to_string();
                        for method in methods {
                            let (key, contract) =
                                trait_method(parsed, &own, &method.node, *is_public);
                            ledger.functions.insert(key, contract);
                        }
                        ledger.traits.insert(
                            own,
                            methods
                                .iter()
                                .map(|m| parsed.text(m.node.name).to_string())
                                .collect(),
                        );
                    }
                    // **An `enum` is a type a consumer has to know the cases of**
                    // (Part I 3.4: a `match` handles every possible case), and
                    // until this was here it had no entry at all — so a `match`
                    // over a dependency's `enum` could not be shown total and
                    // `NK1151` asked for an `else` that makes *a type gaining a
                    // variant* silent forever after.
                    Item::Enum {
                        name,
                        variants,
                        is_public,
                        ..
                    } => {
                        // An `enum` takes no type parameters in this language
                        // (Part I 4.4), so there is nothing to erase and the
                        // payload types travel as they were written.
                        let cases = variants
                            .iter()
                            .map(|variant| VariantContract {
                                name: parsed.text(variant.name).to_string(),
                                holds: match &variant.fields {
                                    crate::ast::VariantFields::Unit => Vec::new(),
                                    // Positional, so the names are the positions —
                                    // the arrangement the checker's own map uses
                                    // for a variant's payload.
                                    crate::ast::VariantFields::Tuple(types) => types
                                        .iter()
                                        .enumerate()
                                        .map(|(at, ty)| FieldContract {
                                            name: at.to_string(),
                                            ty: ty::Ty::from_ast(parsed, ty),
                                            public: true,
                                        })
                                        .collect(),
                                    crate::ast::VariantFields::Named(fields) => fields
                                        .iter()
                                        .map(|f| FieldContract {
                                            name: parsed.text(f.name).to_string(),
                                            ty: ty::Ty::from_ast(parsed, &f.ty),
                                            // A variant carries no visibility word,
                                            // so its fields are as reachable as the
                                            // `enum` is (Part I 9.2).
                                            public: true,
                                        })
                                        .collect(),
                                },
                                positional: matches!(
                                    variant.fields,
                                    crate::ast::VariantFields::Tuple(_)
                                ),
                            })
                            .collect();
                        ledger.types.insert(
                            parsed.text(*name).to_string(),
                            TypeContract {
                                public: *is_public,
                                // A `struct` has fields and an `enum` has cases,
                                // and neither has the other's.
                                fields: Vec::new(),
                                variants: cases,
                                crosses: Crosses::Undecided,
                                iterates_fallibly: false,
                                // A declared type's own parts decide whether it
                                // compares, so nothing is written here: this
                                // column is for a type whose parts are Rust.
                                compares: false,
                                copies: false,
                                touches: Vec::new(),
                                // The tether is a field's question and a variant's
                                // payload is not a field a program assigns to;
                                // `views::fields_of` flattens them for the *view*
                                // analysis, which is a different walk over the AST.
                                tethered: Vec::new(),
                                // A declared type is walked by its parts.
                                constant: String::new(),
                            },
                        );
                    }
                    Item::Struct {
                        name,
                        generics,
                        fields,
                        is_public,
                        ..
                    } => {
                        let parameters: BTreeSet<String> = generics
                            .iter()
                            .map(|g| parsed.text(g.name).to_string())
                            .collect();
                        let tethered = fields
                            .iter()
                            .filter(|f| holds_view(&f.ty) || names_borrowing(&f.ty, &borrowing))
                            .map(|f| parsed.text(f.name).to_string())
                            .collect();
                        let field_types = fields
                            .iter()
                            .map(|f| FieldContract {
                                name: parsed.text(f.name).to_string(),
                                ty: ty::Ty::from_ast(parsed, &f.ty).parameterise(&parameters),
                                public: f.is_public,
                            })
                            .collect();
                        ledger.types.insert(
                            parsed.text(*name).to_string(),
                            TypeContract {
                                public: *is_public,
                                fields: field_types,
                                variants: Vec::new(),
                                // Never inferred: a `struct` declared here records
                                // its fields, and `contracts::send` walks those.
                                // The key exists for types whose parts are Rust.
                                crosses: Crosses::Undecided,
                                // Nothing a `.nika` file declares iterates at all
                                // yet, let alone fallibly: the types that do are
                                // `std`'s, and `std` writes them down (ADR-025 D6).
                                iterates_fallibly: false,
                                // A declared type's own parts decide whether it
                                // compares, so nothing is written here: this
                                // column is for a type whose parts are Rust.
                                compares: false,
                                copies: false,
                                // Nor does a declared `struct` read anything when
                                // it is read: a field access is memory. The types
                                // that are not are `std`'s, whose bodies are Rust
                                // ([ADR-281](../../../../docs/specification/adr/adr-281.md) D34).
                                touches: Vec::new(),
                                tethered,
                                constant: String::new(),
                            },
                        );
                    }
                    // **Every `entry` rule of a grammar is an entry**
                    // ([ADR-296](../../../docs/specification/adr/adr-296.md) D25).
                    // A grammar is entered by an ordinary call — `Json.value(input)`
                    // — so the thing entered has to be an ordinary contract, and
                    // the one column it must carry is `throws`: a rule past a
                    // commit point can fail ([ADR-023](../../../docs/specification/adr/adr-023.md)
                    // D9), and a `catch` beside the entry would meet `NK1134`
                    // without it.
                    //
                    // **`["ParseError"]` since
                    // [ADR-296](../../../docs/specification/adr/adr-296.md) D38.**
                    // It used to be `["?"]` — *something this compiler cannot
                    // name* — because a parse fails with a **rendered string**
                    // and a string is not a type. It was the last `"?"` in the
                    // tree, and seven of the corpus' eight `main`s carried it
                    // into their own set; a type is all it ever needed.
                    Item::Grammar(def) => {
                        let grammar = parsed.text(def.name).to_string();
                        for rule in def.rules.iter().filter(|r| r.is_entry) {
                            let key = format!("{grammar}::{}", parsed.text(rule.name));
                            ledger.functions.insert(
                                key,
                                FnContract {
                                    public: true,
                                    fails_with: vec![PARSE_ERROR.to_string()],
                                    // **An action may not pause**
                                    // ([ADR-296](../../../docs/specification/adr/adr-296.md)
                                    // D1), so every entry is `sync` — asserted
                                    // and not inferred, because it is a rule of
                                    // the language rather than a property of
                                    // this grammar, and `NK2209` is what
                                    // happens when an action contradicts it.
                                    //
                                    // It is also what takes back the `async`
                                    // that reaching the entry by **name**
                                    // ([ADR-140](../../../docs/specification/adr/adr-140.md)
                                    // D3) had spread through every parsing
                                    // program: a caller reads this column, and
                                    // before it there was nothing in it.
                                    sync_claim: Sync::Asserted,
                                    signature: Some(Signature {
                                        // A grammar's rule takes no type
                                        // parameter, so it declares no bound.
                                        bounds: Vec::new(),
                                        mutable: Vec::new(),
                                        // The input, as every entry takes it: the
                                        // text to parse. `?` because a mapping, an
                                        // owned string and a view all reach the
                                        // parser the same way and no one type is
                                        // the true one.
                                        params: vec![(INPUT.to_string(), ty::Ty::Unknown)],
                                        config: Vec::new(),
                                        result: rule
                                            .ret_type
                                            .as_ref()
                                            .map(|t| ty::Ty::from_ast(parsed, t)),
                                    }),
                                    ..FnContract::empty()
                                },
                            );
                        }
                    }
                    _ => {}
                }
            }
        }

        // **Every unit is checked against the whole package's declarations**,
        // which is the other half of D2: a call to the file next door resolves
        // here too, so the method calls the walks below read are the package's.
        // With the other files **beside** it, so a type another file
        // declares has its fields and variants here as there (#125).
        let checked: Vec<crate::check::Checked> = units
            .iter()
            .enumerate()
            .map(|(at, parsed)| {
                let beside: Vec<&Parsed> = units
                    .iter()
                    .enumerate()
                    .filter(|(other, _)| *other != at)
                    .map(|(_, other)| *other)
                    .collect();
                crate::check::check_for_the_ledger(
                    parsed,
                    &beside,
                    &ledger,
                    library,
                    &crate::assets::Reads::none(),
                )
            })
            .collect();
        let resolved: BTreeMap<String, crate::check::MethodCalls> = checked
            .iter()
            .flat_map(|c| c.methods.iter().map(|(k, v)| (k.clone(), v.clone())))
            .collect();

        let noted = sync::infer(&mut ledger, units, library, &resolved);
        // **An expression function's `ensures`** (ADR-314 D3): after `sync`,
        // which it is only given for.
        expression_ensures(&mut ledger, units);
        // Kap 7.1: `throws` in the source says *that* it fails; this says with
        // what (ADR-023 D1). After `sync`, because both read bodies and only
        // this one needs nothing from the other - and both are handed the same
        // `resolved`, because ADR-288's whole point is that there is one
        // answer to what `a.add(v)` goes to and both walks read it.
        throws::infer(&mut ledger, units, library, &resolved);
        // **The fourth derived column** ([ADR-288](../../../docs/specification/adr/adr-288.md)
        // D2), and the one that was specified without an inference. After
        // `throws` for no reason but tidiness: it reads the same bodies through
        // the same walk and needs nothing either of the two produced.
        touch::infer(&mut ledger, units, library, &resolved);
        // ADR-037 D7: which count each `Shared` class gets. Last, because it
        // resolves a callee's parameters against the `signature` step 1 wrote
        // and a type's parts against its `fields`, and reads nothing the two
        // inferences above produced.
        //
        // **A unit at a time, against the package's ledger.** It summarises the
        // `Shared` values a body holds rather than folding a call graph, so
        // there is no fixpoint to run across units - what it needed from its
        // neighbours is the callee's `signature`, and that is in the ledger the
        // loop above built.
        for parsed in units.iter().copied() {
            sharing::infer(&mut ledger, parsed, library);
        }
        // **The fifth derived column** ([ADR-094](../../../docs/specification/adr/adr-094.md)
        // D2). Last, because its fixpoint reads a callee's `signature` — which
        // the item loop wrote — and a callee's own `keeps`, which is itself;
        // nothing above produces anything it needs, and nothing above reads
        // what it writes.
        keeps::infer(&mut ledger, units, library, &resolved);
        // Beside `keeps`, and for the same reason it runs here: it reads the
        // checker's method answers and the entries the item loop wrote.
        let stored: BTreeMap<String, crate::check::StoredCode> = checked
            .iter()
            .flat_map(|c| c.stored_code.iter().map(|(k, v)| (k.clone(), v.clone())))
            .fold(BTreeMap::new(), |mut all, (key, code)| {
                let entry: &mut crate::check::StoredCode = all.entry(key).or_default();
                entry.resolved.extend(code.resolved);
                entry.unresolved |= code.unresolved;
                entry.free.extend(code.free);
                all
            });
        locks::infer(&mut ledger, units, library, &resolved, &stored);
        // **Last, and it reads none of the columns above**
        // ([ADR-283](../../../docs/specification/adr/adr-283.md) D5): what it
        // asks is about a signature's shape and about which buffer a returned
        // view came from, and no inference above answers either. It is also the
        // one that changes no lowering — the state it writes is a
        // representation and only one of the three is built.
        tether::infer(&mut ledger, units, library);
        evaluate_defaults(&mut ledger, units, &defaults, library);
        (ledger, checked, noted)
    }

    /// One function's entry, named as a caller would reach it.
    ///
    /// `doc` is the prose standing in front of the declaration
    /// ([ADR-307](../../../../docs/specification/adr/adr-307.md) D5), and it is
    /// kept only where the entry is `pub`: what a private item says is the
    /// source's, and a consumer was never going to read it.
    fn function(
        &self,
        parsed: &Parsed,
        item: &Item,
        target: Option<&str>,
        outer: &BTreeSet<String>,
    ) -> (String, FnContract) {
        // **What the declaration says, in Nikaia** (`tools/declared.nika`,
        // ADR-294, #125): every column about the body is the inferences' to
        // answer afterwards.
        let declared = nikaia_std::tools::declared::declared_function(
            &parsed.interner,
            &parsed.aliases,
            item,
            &target.map(str::to_string),
            outer,
        )
        .expect("only a function is passed here");
        (declared.key, declared.contract)
    }

    /// A function by the name a caller wrote.
    ///
    /// **Exactly the name, since
    /// [ADR-313](../../../docs/specification/adr/adr-313.md)**: a `std` entry
    /// that lives in a module is reached through the module, `text::digit_value`
    /// and not `digit_value`, and what needs no prefix is the list on Part I's
    /// first page — whose entries are keyed **bare** here, so the exact lookup
    /// is the whole rule.
    ///
    /// It used to match on the last segment, which was name-for-name resolution
    /// (ADR-296 D17) rather than import tracking, and it is what a compiler
    /// without a module graph could honestly do before there was a written
    /// list. What it cost, beyond the prelude being undefined, was answering
    /// about the **wrong function**: a program with its own `fn read` found
    /// `io::read` in a unit that does not carry the ledger of the package
    /// beside it, and the `throws` column then spoke for a callee nobody had
    /// resolved.
    fn lookup(&self, name: &str) -> Option<(String, &FnContract)> {
        self.functions
            .get(name)
            .map(|contract| (name.to_string(), contract))
    }

    /// Every entry a bare method name could resolve to.
    ///
    /// Which one `xs.len()` *is* depends on what `xs` is, and that is the type
    /// checker's answer rather than this file's (ADR-288). But a question
    /// weaker than "which entry" can be answered without it: if **every**
    /// `::len` in the ledger reaches nothing, then `xs.len()` reaches nothing
    /// whatever `xs` turns out to be. An over-approximation over the
    /// candidates, which is the direction ADR-292 D3 requires.
    ///
    /// **Only a method is a candidate**: an entry whose signature takes no
    /// `self` is a function a call names by its path, and `x.lines()` never
    /// reaches `io::lines()`. Counted in, it made every `text.lines()` over a
    /// lent parameter keep it, because a function whose first parameter is not
    /// a lent `self` reads, to [`keeps`], as one that takes its receiver whole
    /// (found moving the ledger's reader to Nikaia, 0.0.258). An entry with no
    /// signature is still a candidate - what it takes is unknown.
    fn candidates(&self, method: &str) -> Vec<(&str, &FnContract)> {
        let suffix = format!("::{method}");
        self.functions
            .iter()
            .filter(|(key, _)| key.ends_with(&suffix))
            .filter(|(_, contract)| {
                contract
                    .signature
                    .as_ref()
                    .is_none_or(|s| s.params.first().is_some_and(|(name, _)| name == "self"))
            })
            .map(|(key, contract)| (key.as_str(), contract))
            .collect()
    }

    /// The file, as it is written out: `tools/ledger_text.nika` (ADR-294,
    /// #125), beside `ledger.nika`, which reads it back.
    fn render(&self) -> String {
        nikaia_std::tools::ledger_text::ledger_text(self)
    }

    /// **What the contract beside it was derived from**
    /// ([ADR-251](../../../docs/specification/adr/adr-251.md) D1), in Nikaia.
    fn render_derived(&self) -> String {
        nikaia_std::tools::ledger_text::derived_text(self)
    }

    /// The contract at `path` with the record beside it read in, where there
    /// is one: `nikaia.contracts` and `nikaia.derived`,
    /// `contracts/<crate>.contracts` and `contracts/<crate>.derived`.
    ///
    /// **A contract with no record beside it has no sources**, which every
    /// reader of `sources` already takes for *nothing recorded*, the
    /// fail-closed answer (ADR-100 D3).
    fn read_beside(path: &std::path::Path) -> Option<Ledger> {
        let mut ledger = Ledger::parse(&std::fs::read_to_string(path).ok()?).ok()?;
        if let Some(derived) = std::fs::read_to_string(derived_path(path))
            .ok()
            .and_then(|text| Ledger::parse(&text).ok())
        {
            ledger.toolchain = derived.toolchain;
            ledger.inference = derived.inference;
            ledger.sources = derived.sources;
        }
        Some(ledger)
    }

    /// The same file under a **description's** header
    /// ([ADR-290](../../../docs/specification/adr/adr-290.md) D5), in Nikaia.
    fn render_description(&self, crate_name: &str, version: &str, notes: &Notes) -> String {
        nikaia_std::tools::ledger_text::description_text(self, crate_name, version, notes)
    }

    /// The header line and everything after it, shared by both renderings.
    fn render_from(&self, out: &mut String) {
        self.render_from_with(out, &Notes::empty());
    }

    /// The same, with the describer's notes spliced above the entries they are
    /// about ([ADR-290](../../../docs/specification/adr/adr-290.md) D3), in
    /// Nikaia.
    fn render_from_with(&self, out: &mut String, notes: &Notes) {
        out.push_str(&nikaia_std::tools::ledger_text::contract_text(self, notes));
    }

    /// Read a ledger back - a library's, or this project's own.
    ///
    /// Deliberately a small reader for the small format `render` writes rather
    /// than a TOML parser: the file is generated, so the shapes it can take are
    /// the shapes written above, and a dependency to read one's own output back
    /// is a dependency to keep in step.
    /// **Read in Nikaia** (`tools/ledger.nika`, ADR-294 step (c)): the file's
    /// shape, what each table and key means, and a signature back from the
    /// file's spelling.
    fn parse(text: &str) -> Result<Self> {
        nikaia_std::tools::ledger::read(text).map_err(|refusal| anyhow!("{refusal}"))
    }
}

/// `std`'s ledger, parsed once.
///
/// Embedded at build time and parsed on first use rather than on every
/// inference. **Every reader of `std`'s ledger reads this one** (0.0.296):
/// seven places parsed `STD` again for themselves, which was 83 M of the 93 M
/// instructions an empty program cost. A reader that only reads borrows it; one
/// that adds a package's own entries takes [`std_library`], a copy. It failing to parse is a broken compiler, not a broken program:
/// `crates/nikaia/tests/contracts.rs` reads the same bytes and would have said
/// so long before a user got here.
pub(crate) fn std_ledger() -> &'static Ledger {
    static PARSED: std::sync::OnceLock<Ledger> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| Ledger::parse(STD).expect("std ships a ledger this compiler can read"))
}

/// `std`'s ledger as a library to **build on**: the floor every package's
/// inference starts from, before its own dependencies are absorbed into it
/// ([ADR-100](../../../docs/specification/adr/adr-100.md) D1).
///
/// A clone, because a consumer's library is `std` *plus* what its dependencies
/// published and the shipped one is shared and immutable. One `std` per package
/// inferred is a few hundred entries copied — measured against what it replaces,
/// which is deriving that dependency's whole source tree again.
pub fn std_library() -> Ledger {
    std_ledger().clone()
}

/// The receiver's type, as a caller sees it: the type the `impl` is for, with
/// the `&` the receiver was written with.
/// One method of a `trait`, as a contract a bound can be answered from.
///
/// A builder of its own rather than `Ledger::function` with the body ignored,
/// for the reason the emitter has a second writer: that one records `borrows`,
/// `sharing` and a `touches` set that later passes fill in **by reading the
/// body**, and a declaration has none. What is here is what a declaration can
/// say: whether it pauses, whether it can fail, and what its parameters and
/// result are.
///
/// **`sync` is the declaration's own word**
/// ([ADR-288](../../../docs/specification/adr/adr-288.md) D24): a trait method
/// reads like a function type, so without `sync` it **may pause**, exactly as a
/// function without the word may.
///
/// **It used to be asserted whatever the declaration said**
/// ([ADR-295](../../../docs/specification/adr/adr-295.md) D9), and that was a
/// decision rather than a default: `async fn` in a trait was something the
/// emitter had no way to ask for, so a plain `fn` was the only thing it could
/// write, and a trait whose method genuinely pauses was **refused** rather than
/// mis-lowered (`NK1129`). ADR-288 D26 takes the cause away: the trait declares
/// the **return-position** form, `fn load(&self) -> impl Future<Output = …>`,
/// and the `impl` writes `async fn`, which satisfies it. So the word can mean
/// what it says.
///
/// For a function the word says `Asserted`, its absence says `No`, and
/// `sync::infer` raises `No` to `Inferred` by reading the body. A declaration
/// has no body, so `No` stands — and `No` means *may pause*, which is D1's
/// sentence.
fn trait_method(
    parsed: &Parsed,
    trait_name: &str,
    method: &crate::ast::TraitMethod,
    public: bool,
) -> (String, FnContract) {
    // In Nikaia (`tools/declared.nika`, ADR-294, #125).
    let declared = nikaia_std::tools::declared::declared_trait_method(
        &parsed.interner,
        &parsed.aliases,
        trait_name,
        method,
        public,
    );
    (declared.key, declared.contract)
}

/// What produced the contracts: this compiler, not the one it emits Rust for.
///
/// `sync`, `throws` and the borrow contract are decided here and never by
/// `rustc`, so the version that matters to a ledger diff is Nikaia's.
/// Where the derivation record of the contract at `path` is: the same
/// directory and stem, `.derived` for `.contracts`.
pub fn derived_path(path: &std::path::Path) -> std::path::PathBuf {
    path.with_extension("derived")
}

fn toolchain() -> String {
    format!("nikaia {}", env!("CARGO_PKG_VERSION"))
}

/// A value that may itself hold a quote - which a signature does, the moment an
/// option's default is a string: `method: &str = "GET"`.
/// **And a `\n` becomes `\\n`**, which is
/// The name an error gets when the compiler cannot name it - ADR-024 D1's `?`,
/// which is the absence of a claim rather than a type.
pub const UNNAMED_ERROR: &str = "?";

/// What a parse fails with
/// ([ADR-296](../../../docs/specification/adr/adr-296.md) D38).
///
/// A `std` type with no module in front, which is `Overtaken`'s shape: a
/// program never writes a path to it, because it arrives in a `catch`.
pub const PARSE_ERROR: &str = "ParseError";

/// The one parameter every grammar entry takes: the text to parse
/// ([ADR-296](../../../docs/specification/adr/adr-296.md) D24).
///
/// A name rather than four spellings of it, because three analyses have to
/// agree on it: the loop below writes the signature, [`tether::infer`] writes
/// the state of the views a parse hands back, and [`keeps::infer`] says whether
/// the entry keeps it. A column that names a position no signature has is a
/// column nobody can read.
pub const INPUT: &str = "input";

/// The `throws` list exactly as the ledger writes it, in Nikaia
/// (`tools/ledger_text.nika`): the note is the contract (Part III C.4).
pub fn throws_text(throws: &[String]) -> String {
    nikaia_std::tools::ledger_text::throws_text(throws)
}

/// The types Part I 2.2 offers, by name.
///
/// Here rather than beside a diagnostic because two questions read it: whether
/// `as` names a type this language has ([ADR-285](../../../docs/specification/adr/adr-285.md)
/// D1), and whether an `impl`'s type argument is a parameter or a type.
const OFFERED: &[&str] = &[
    "i32", "i64", "u8", "u32", "u64", "f64", "bool", "char", "String", "str", "Self",
];

/// Every type name this file declares.
pub fn declared_types(parsed: &Parsed) -> BTreeSet<String> {
    let mut declared: BTreeSet<String> = BTreeSet::new();
    for item in &parsed.program.items {
        match &item.node {
            Item::Struct { name, .. } | Item::Enum { name, .. } => {
                declared.insert(parsed.text(*name).to_string());
            }
            // **An opaque handle is a type this file declares**
            // ([ADR-302](../../../docs/specification/adr/adr-302.md) D7). It
            // has no fields and no constructor, but a declaration is what
            // `NK1135` asks for and this is one - the block that writes it is
            // the only place its name comes from.
            Item::Extern { opaque, .. } => {
                for handle in opaque {
                    declared.insert(parsed.text(handle.node.name).to_string());
                }
            }
            _ => {}
        }
    }
    declared
}

/// The type parameters an `impl` head declares, in the order it writes them.
///
/// `impl Stack[T]` is `impl<T> Stack<T>` and `impl Stack[i64]` is
/// `impl Stack<i64>`, and what tells them apart is whether the slot names a
/// type: a bare name that is neither one of Part I 2.2's types nor one this
/// file declares stands for a type rather than being one
/// ([ADR-295](../../../docs/specification/adr/adr-295.md) D4).
///
/// **The `impl` has no parameter list of its own**, and that is the decision
/// rather than a gap: Part I 4.6 writes `struct Box[T]` and nothing writes
/// `impl[T]`, so a second list would be a spelling the specification does not
/// have. The rule above reads the one list that is written.
///
/// One function, called by the ledger and by the emitter, so the names a
/// method's signature is recorded with are the names its `impl` head declares -
/// two rules here would be one silent disagreement.
pub fn impl_parameters(
    parsed: &Parsed,
    target: &crate::ast::Type,
    declared: &BTreeSet<String>,
) -> Vec<String> {
    target
        .generics
        .iter()
        .filter(|g| g.generics.is_empty() && !g.is_tuple && !g.is_view && !g.is_nullable)
        .map(|g| parsed.text(g.name).to_string())
        .filter(|name| !OFFERED.contains(&name.as_str()) && !declared.contains(name))
        .collect()
}

/// **An option's default that is not a literal** (ADR-318 D1): the function's
/// key, the option's place in its list, the unit and the expression.
struct Computed<'a> {
    key: String,
    at: usize,
    parsed: &'a Parsed,
    default: &'a Expr,
    /// The option's name and declared type, which a compiled run needs.
    option: String,
    ty: &'a crate::ast::Type,
}

/// The options of `item` whose default is an expression to evaluate.
fn computed_defaults<'a>(
    into: &mut Vec<Computed<'a>>,
    key: &str,
    parsed: &'a Parsed,
    item: &'a Item,
) {
    let Item::Fn { config, .. } = item else {
        return;
    };
    for (at, option) in config.iter().enumerate() {
        if !a_literal(&option.default) {
            into.push(Computed {
                key: key.to_string(),
                at,
                parsed,
                default: &option.default,
                option: parsed.text(option.name).to_string(),
                ty: &option.ty,
            });
        }
    }
}

/// A literal, as the grammar once required a default to be: its text is the
/// value, in this language and the one below.
pub fn a_literal(expr: &Expr) -> bool {
    match expr {
        Expr::LitInt { .. }
        | Expr::LitFloat(_)
        | Expr::LitBool(_)
        | Expr::LitStr { .. }
        | Expr::LitChar(_)
        | Expr::LitNull => true,
        Expr::Unary {
            op: crate::ast::UnaryOp::Neg,
            expr,
        } => matches!(**expr, Expr::LitInt { .. } | Expr::LitFloat(_)),
        _ => false,
    }
}

/// **A computed default is evaluated once, where the function is declared,
/// and the ledger records its value** (ADR-318 D1-D3): a caller in this
/// package or another reads a literal, as it always did, and needs no body.
/// A value with no literal of its own here - a struct, a list - or one that
/// cannot be computed keeps the empty text, and the checker refuses it at the
/// default (D7).
fn evaluate_defaults(
    ledger: &mut Ledger,
    units: &[&Parsed],
    defaults: &[Computed<'_>],
    library: &Ledger,
) {
    if defaults.is_empty() {
        return;
    }
    let reads = crate::assets::Reads::none();
    let own = ledger.clone();
    let nothing = |_: &str| -> Option<crate::build_time::Value> { None };
    let workshop = crate::comptime_run::defaults_workshop();
    for computed in defaults {
        // **Read where a run's sub-program is inferred** (#468): the program's
        // ledger has already computed it.
        if let Some(text) = crate::comptime_run::known_default(
            computed.parsed.package.as_deref(),
            &computed.key,
            computed.at,
        ) {
            if let Some(option) = ledger
                .functions
                .get_mut(&computed.key)
                .and_then(|c| c.signature.as_mut())
                .and_then(|s| s.config.get_mut(computed.at))
            {
                option.default = text;
            }
            continue;
        }
        // **Compiled where the build has a workshop** (ADR-321 D1), as the
        // checker computes the same default; the interpreter answers a read
        // with none.
        let compiled = workshop.as_ref().and_then(|workshop| {
            let here = units
                .iter()
                .position(|unit| std::ptr::eq(*unit, computed.parsed))?;
            let ty = crate::comptime_run::as_built(crate::contracts::ty::Ty::from_ast(
                computed.parsed,
                computed.ty,
            ));
            match crate::comptime_run::compute(
                units,
                here,
                &computed.option,
                computed.default,
                &ty,
                &nothing,
                library,
                workshop,
                workshop.bounds(),
                None,
            ) {
                crate::comptime_run::Computed::Value(value) => Some(Some(value)),
                crate::comptime_run::Computed::NotHere => None,
                _ => Some(None),
            }
        });
        let value = match compiled {
            Some(value) => value,
            None => {
                crate::build_time::BuildTime::new(computed.parsed, units, &own, &reads, &nothing)
                    .evaluate(computed.default)
                    .ok()
            }
        };
        let Some(text) = value.and_then(|v| literal_of(&v)) else {
            continue;
        };
        if let Some(option) = ledger
            .functions
            .get_mut(&computed.key)
            .and_then(|c| c.signature.as_mut())
            .and_then(|s| s.config.get_mut(computed.at))
        {
            option.default = text;
        }
    }
}

/// A value as the literal both languages spell it the same way, where it has
/// one.
pub fn literal_of(value: &crate::build_time::Value) -> Option<String> {
    use crate::build_time::Value;
    match value {
        Value::Int(n) => Some(nikaia_std::tools::integers::integer_text(n)),
        Value::Float(f) if f.is_finite() => Some(format!("{f:?}")),
        Value::Bool(b) => Some(b.to_string()),
        Value::Text(text) => Some(format!("\"{}\"", crate::build_time::written(text))),
        // **A struct of literals is a literal too** (ADR-318 D3), and spelled
        // the same in both languages: `Point { x: 1, y: 2 }`.
        Value::Struct { name, fields } => {
            let mut written = Vec::with_capacity(fields.len());
            for (field, held) in fields {
                written.push(format!("{field}: {}", literal_of(held)?));
            }
            Some(format!("{name} {{ {} }}", written.join(", ")))
        }
        // **A list of numbers or `bool`s** (ADR-318 D4), written as the
        // language writes it; it crosses to a call as the view an option of
        // `ref Vec[T]` takes (`emit`'s `&vec![…]`). A list of anything else has no
        // form both sides read alike.
        Value::List(items)
            if items
                .iter()
                .all(|item| matches!(item, Value::Int(_) | Value::Float(_) | Value::Bool(_))) =>
        {
            let mut written = Vec::with_capacity(items.len());
            for item in items {
                written.push(literal_of(item)?);
            }
            Some(format!("[{}]", written.join(", ")))
        }
        Value::Variant {
            ty,
            variant,
            payload,
        } => {
            if payload.is_empty() {
                return Some(format!("{ty}::{variant}"));
            }
            let mut written = Vec::with_capacity(payload.len());
            for held in payload {
                written.push(literal_of(held)?);
            }
            Some(format!("{ty}::{variant}({})", written.join(", ")))
        }
        // **A `std` type as its constructor over literal parts** (ADR-318
        // D3, D5): `time::Duration::new(30, 0)`.
        Value::Constant { constructor, parts } => {
            let mut written = Vec::with_capacity(parts.len());
            for held in parts {
                written.push(literal_of(held)?);
            }
            Some(format!("{constructor}({})", written.join(", ")))
        }
        _ => None,
    }
}

/// **A `sync` function whose body is one `return e` publishes
/// `ensures result == e`** ([ADR-314](../../../docs/specification/adr/adr-314.md)
/// D3): the expression is the function, as SPARK reads an expression function.
/// Only a free function returning a whole number, and only where the prover
/// reads the condition back - over the parameters and `result` - so a caller
/// in another package can rely on what it says.
fn expression_ensures(ledger: &mut Ledger, units: &[&Parsed]) {
    const WHOLE: [&str; 5] = ["i32", "i64", "u8", "u32", "u64"];
    for parsed in units.iter().copied() {
        for item in &parsed.program.items {
            let Item::Fn {
                name: Some(name),
                receiver: None,
                args,
                config,
                ret_type: Some(ret),
                body,
                ..
            } = &item.node
            else {
                continue;
            };
            if !config.is_empty()
                || !ret.generics.is_empty()
                || !WHOLE.contains(&parsed.text(ret.name))
                || body.stmts.len() != 1
            {
                continue;
            }
            let Stmt::Return(Some(value)) = &body.stmts[0].node else {
                continue;
            };
            let Some(contract) = ledger.functions.get_mut(parsed.text(*name)) else {
                continue;
            };

            if !contract.sync_claim.is_sync() || !contract.ensures.is_empty() {
                continue;
            }
            // **The prover's own text of `e`**, which is what a reader reads
            // back - not the source's, which may abbreviate.
            let names: BTreeSet<String> = args
                .iter()
                .map(|a| parsed.text(a.name).to_string())
                .collect();
            let mut nodes = Vec::new();
            let Some(term) = nikaia_std::tools::prove_terms::lin_term(
                value,
                &parsed.interner,
                &|name: &str| names.contains(name),
                &mut nodes,
            ) else {
                continue;
            };
            let text = nikaia_std::tools::prove_text::term_ledger_text(term, &|at| {
                nodes[at as usize].clone()
            });
            let condition = format!("result == {text}");
            let mut readable = names;
            readable.insert("result".to_string());
            if !crate::prove::reads_back(&condition, &readable) {
                continue;
            }
            contract.ensures = vec![condition];
            contract.from = vec!["return".to_string()];
        }
    }
}
