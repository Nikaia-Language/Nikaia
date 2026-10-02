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
// `sync` is *inferred from the body* (ADR-027): a function that provably cannot
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

use crate::ast::Item;
use crate::contracts::ty::TyOps;
use crate::emit::{borrowing_structs, holds_view, names_borrowing};
use crate::parser::Parsed;

/// **The ledger's small records are declared in Nikaia**
/// ([ADR-257](../../../docs/specification/adr/adr-257.md) step (b)):
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
/// `throws`; since ADR-027 it reads **bodies** for `sync`, since ADR-023 D1 for
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

/// What stays Rust of a variant's entry until its text moves (ADR-257 step
/// (c)): reading one back. The declaration and `text` are `tools/ty.nika`'s.
pub trait VariantOps {
    fn parse(text: &str) -> VariantContract;
}

impl VariantOps for VariantContract {
    fn parse(text: &str) -> VariantContract {
        nikaia_std::tools::ledger::variant_of(text)
    }
}

/// What stays Rust of a [`Signature`] (ADR-257 step (b)): the parameters
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
/// back. The record is `tools/ty.nika`'s (ADR-257 D1); these stay Rust until
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
    /// learned for the build to say ([`sync::Noted`], ADR-244 D2 and D5).
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
    /// A package is one namespace ([ADR-047](../../../docs/specification/adr/adr-047.md)
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
    /// ([ADR-027](../../../docs/specification/adr/adr-027.md)).
    fn infer_package(units: &[&Parsed], library: &Ledger) -> Self {
        Self::infer_package_checked(units, library).0
    }

    /// This package's **own** entries: what it publishes, with everything that
    /// belongs to a package *it* depends on left out
    /// ([ADR-053](../../../docs/specification/adr/adr-053.md) D3).
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
        let theirs = |key: &str| {
            key.split_once("::")
                .is_some_and(|(first, _)| foreign.contains(first))
        };
        Ledger {
            functions: self
                .functions
                .iter()
                .filter(|(key, _)| !theirs(key))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            types: self
                .types
                .iter()
                .filter(|(key, _)| !theirs(key))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            traits: self
                .traits
                .iter()
                .filter(|(key, _)| !theirs(key))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            ..self.clone()
        }
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
        let mut moved: BTreeSet<String> = BTreeSet::new();
        for (unit, hash) in sources {
            if self.sources.get(unit) != Some(hash) {
                moved.insert(unit.clone());
            }
        }
        // A unit the ledger names and the package no longer has is as much a
        // reason to derive again as one that changed: what left took its
        // entries with it.
        for unit in self.sources.keys() {
            if !sources.contains_key(unit) {
                moved.insert(unit.clone());
            }
        }
        moved.into_iter().collect()
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
    /// ([ADR-053](../../../docs/specification/adr/adr-053.md) D2).
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
        let declared: std::collections::BTreeSet<String> = other.types.keys().cloned().collect();
        // **What a function field's stored code reaches** (ADR-230 D1), under
        // the name a caller writes the type with.
        for (field, holds) in &other.code_locks {
            let key = match module {
                Some(module) if declared.contains(field.split('.').next().unwrap_or("")) => {
                    format!("{module}::{field}")
                }
                _ => field.clone(),
            };
            self.code_locks.insert(key, *holds);
        }
        // **This package's own word for itself is never renamed.** Qualifying has
        // just put `module::` in front of every type this package declares, and a
        // package may perfectly well key one of its dependencies with the word a
        // consumer happens to use for the package itself. Dropping that key here
        // is cheaper than asking every call site to know about it, and it costs
        // nothing: a name this package declares is this package's whatever
        // anybody else calls that word.
        let renames: std::collections::BTreeMap<String, String> = renames
            .iter()
            .filter(|(from, _)| Some(from.as_str()) != module)
            .map(|(from, to)| (from.clone(), to.clone()))
            .collect();
        let qualify = |ty: &ty::Ty| {
            let ty = match module {
                Some(module) => ty::qualify(ty, module, &declared),
                None => ty.clone(),
            };
            match renames.is_empty() {
                true => ty,
                false => ty::renamed(&ty, &renames),
            }
        };
        for (name, mut contract) in other.functions {
            if let Some(signature) = contract.signature.as_mut() {
                for (_, ty) in signature.params.iter_mut() {
                    *ty = qualify(ty);
                }
                for option in signature.config.iter_mut() {
                    option.ty = qualify(&option.ty);
                }
                if let Some(result) = signature.result.as_mut() {
                    *result = qualify(result);
                }
            }
            let key = match module {
                Some(module) => format!("{module}::{name}"),
                None => name,
            };
            self.functions.insert(key, contract);
        }
        for (name, mut contract) in other.types {
            for field in contract.fields.iter_mut() {
                field.ty = qualify(&field.ty);
            }
            let key = match module {
                Some(module) => format!("{module}::{name}"),
                None => name,
            };
            self.types.insert(key, contract);
        }
        // Kap 4.7: a trait's methods went into `functions` above, under
        // `Summarize::summary`, and were qualified with the rest. This carries
        // the **names**, which is what says `Summarize` is a trait at all
        // ([ADR-078](../../../docs/specification/adr/adr-078.md) D3).
        //
        // Measured the hard way: without it a one-file program's bound resolved
        // twice and failed the third time, because the program's ledger is
        // absorbed from the unit's and this map was the one thing left behind.
        for (trait_name, types) in other.implementations {
            // **Both spellings, on both sides**, because the question is asked
            // from both: inside the package the `impl` is `Handler for Dog`, and
            // to a consumer it is `pets::Handler for pets::Dog`. A bound may name
            // a path since [ADR-106](../../../docs/specification/adr/adr-106.md)
            // D1, so the qualified half is what a consumer's bound looks the
            // answer up under — and an extra spelling can only make the check
            // fail *open*, which is the side [Part III
            // C.4](../../../docs/specification/30-nikaia-tooling.md) puts the
            // benefit of the doubt on.
            let mut widened: BTreeSet<String> = types.clone();
            if let Some(module) = module {
                widened.extend(types.iter().map(|ty| format!("{module}::{ty}")));
            }
            let names = match module {
                Some(module) => vec![format!("{module}::{trait_name}"), trait_name],
                None => vec![trait_name],
            };
            for name in names {
                self.implementations
                    .entry(name)
                    .or_default()
                    .extend(widened.iter().cloned());
            }
        }
        for (name, methods) in other.traits {
            let key = match module {
                Some(module) => format!("{module}::{name}"),
                None => name,
            };
            self.traits.insert(key, methods);
        }
    }

    /// The contracts, and the type checker's pass that helped produce them.
    ///
    /// Three steps, and the order is forced:
    ///
    /// 1. read the declarations, so every function has an entry to be looked
    ///    up in;
    /// 2. run the **type checker** against that, which resolves each method
    ///    call to the function it goes to (ADR-028);
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

        for parsed in units.iter().copied() {
            // Per unit, and it has to be: a `Symbol` is interned by the parse
            // of one file, so a set of them means nothing to another.
            let borrowing = borrowing_structs(parsed);

            for item in &parsed.program.items {
                match &item.node {
                    Item::Fn { .. } => {
                        let (name, contract) =
                            ledger.function(parsed, &item.node, None, &BTreeSet::new());
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
                        // ADR-174 D1: `impl Speaks for Dog` is the claim that a
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
                            ledger.functions.insert(name, contract);
                        }
                    }
                    // Kap 4.7: a trait's methods are recorded under the trait's own
                    // name - `Summarize::summary` - which is what lets a bound be
                    // looked up ([ADR-078](../../../docs/specification/adr/adr-078.md)
                    // D3). The same key shape an `impl`'s methods get, because a
                    // bound and a receiver ask the same question: what does a value
                    // of this thing have.
                    // **An `extern "C"` declaration is an entry like any
                    // other, and reads unlike a trait method**
                    // ([ADR-124](../../../docs/specification/adr/adr-124.md)
                    // D2). Two things are turned around, and only one of them
                    // by this record. It is **`sync`**, asserted rather than
                    // inferred, which is the shape `std`'s own hand-written
                    // entries have for the same reason: the body is in another
                    // language and this compiler does not read it. C has no
                    // suspension point at all, and a C function that sleeps
                    // *blocks* — `println`'s question (ADR-067 D1) and not this
                    // one. And it carries **no `throws`**, which is what a
                    // body-less declaration carries anyway: C has no failure
                    // channel this language reads.
                    //
                    // `touches` and `locks` are absent, which is D4 and is
                    // fail-closed: absent `touches` reads as *touches
                    // everything* (ADR-033) and absent `locks` is that column's
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
                                // ([ADR-169](../../../../docs/specification/adr/adr-169.md) D1).
                                touches: Vec::new(),
                                tethered,
                            },
                        );
                    }
                    // **Every `pub` rule of a grammar is an entry**
                    // ([ADR-082](../../../docs/specification/adr/adr-082.md) D2).
                    // A grammar is entered by an ordinary call — `Json.value(input)`
                    // — so the thing entered has to be an ordinary contract, and
                    // the one column it must carry is `throws`: a rule past a
                    // commit point can fail ([ADR-023](../../../docs/specification/adr/adr-023.md)
                    // D9), and a `catch` beside the entry would meet `NK1134`
                    // without it.
                    //
                    // **`["ParseError"]` since
                    // [ADR-173](../../../docs/specification/adr/adr-173.md) D1.**
                    // It used to be `["?"]` — *something this compiler cannot
                    // name* — because a parse fails with a **rendered string**
                    // and a string is not a type. It was the last `"?"` in the
                    // tree, and seven of the corpus' eight `main`s carried it
                    // into their own set; a type is all it ever needed.
                    Item::Grammar(def) => {
                        let grammar = parsed.text(def.name).to_string();
                        for rule in def.rules.iter().filter(|r| r.is_public) {
                            let key = format!("{grammar}::{}", parsed.text(rule.name));
                            ledger.functions.insert(
                                key,
                                FnContract {
                                    public: true,
                                    fails_with: vec![PARSE_ERROR.to_string()],
                                    // **An action may not pause**
                                    // ([ADR-142](../../../docs/specification/adr/adr-142.md)
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
                crate::check::check_against(
                    parsed,
                    &beside,
                    &ledger,
                    library,
                    &BTreeSet::new(),
                    &crate::check::Newly::default(),
                    &crate::assets::Reads::none(),
                )
            })
            .collect();
        let resolved: BTreeMap<String, crate::check::MethodCalls> = checked
            .iter()
            .flat_map(|c| c.methods.iter().map(|(k, v)| (k.clone(), v.clone())))
            .collect();

        let noted = sync::infer(&mut ledger, units, library, &resolved);
        // Kap 7.1: `throws` in the source says *that* it fails; this says with
        // what (ADR-023 D1). After `sync`, because both read bodies and only
        // this one needs nothing from the other - and both are handed the same
        // `resolved`, because ADR-028's whole point is that there is one
        // answer to what `a.add(v)` goes to and both walks read it.
        throws::infer(&mut ledger, units, library, &resolved);
        // **The fourth derived column** ([ADR-067](../../../docs/specification/adr/adr-067.md)
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
        // ([ADR-008](../../../docs/specification/adr/adr-008.md) D7): what it
        // asks is about a signature's shape and about which buffer a returned
        // view came from, and no inference above answers either. It is also the
        // one that changes no lowering — the state it writes is a
        // representation and only one of the three is built.
        tether::infer(&mut ledger, units, library);
        (ledger, checked, noted)
    }

    /// One function's entry, named as a caller would reach it.
    ///
    /// `doc` is the prose standing in front of the declaration
    /// ([ADR-139](../../../../docs/specification/adr/adr-139.md) D2), and it is
    /// kept only where the entry is `pub`: what a private item says is the
    /// source's, and a consumer was never going to read it.
    fn function(
        &self,
        parsed: &Parsed,
        item: &Item,
        target: Option<&str>,
        outer: &BTreeSet<String>,
    ) -> (String, FnContract) {
        let Item::Fn {
            name,
            generics,
            args,
            config,
            ret_type,
            is_sync,
            sync_by,
            is_public,
            can_throw: throws,
            ..
        } = item
        else {
            unreachable!("only a function is passed here");
        };

        // A generic parameter is a name that stands for a type rather than
        // being one. The ledger records it as a **variable** (`$T`), so a call
        // site binds it from what it passes and reads the result off the same
        // signature - the machinery ADR-031 built for a library's `$V`, now
        // pointed at a Nikaia function's own parameters
        // ([ADR-074](../../../docs/specification/adr/adr-074.md) D2).
        let mut parameters = outer.clone();
        parameters.extend(generics.iter().map(|g| parsed.text(g.name).to_string()));

        // The anonymous constructor of Kap 4.2 is `Type::new` to a caller,
        // because that is what the lowering names it.
        let own = match name {
            Some(name) => parsed.text(*name).to_string(),
            None => "new".to_string(),
        };
        let key = match target {
            Some(target) => format!("{target}::{own}"),
            None => own,
        };

        let returns_view = ret_type.as_ref().is_some_and(holds_view);
        let borrows = if returns_view {
            // **The receiver is a position too**, and `self` is what names it:
            // a `ref self` is a view the caller gave, so a result that is a view
            // may point into the subject. It was left out while nothing read the
            // column across a receiver, and a reader that does then read *no
            // position at all* for the accessor a program most often writes.
            let subject = match item {
                Item::Fn {
                    receiver: Some(receiver),
                    ..
                } if receiver.is_ref => Some("self".to_string()),
                _ => None,
            };
            subject
                .into_iter()
                .chain(
                    args.iter()
                        .filter(|a| holds_view(&a.ty))
                        .map(|a| parsed.text(a.name).to_string()),
                )
                .collect()
        } else {
            Vec::new()
        };

        // A method's receiver is a parameter named `self`, so a caller reads
        // the arguments off the same list either way.
        let mut params: Vec<(String, ty::Ty)> = Vec::new();
        if let Item::Fn {
            receiver: Some(receiver),
            ..
        } = item
        {
            params.push(("self".to_string(), receiver_type(parsed, receiver, target)));
        }
        params.extend(args.iter().map(|a| {
            (
                parsed.text(a.name).to_string(),
                ty::Ty::from_ast(parsed, &a.ty).parameterise(&parameters),
            )
        }));

        (
            key,
            FnContract {
                public: *is_public,
                // `tether::infer` writes it, after the signature this loop
                // records: what it asks is about the signature's shape and
                // about which buffer a returned view came from.
                views: Vec::new(),
                // What the *declaration* says. `sync::infer` reads the body
                // afterwards and may raise a `No` to `Inferred`; it never
                // touches this one, because an assertion is what `NK2202`
                // exists to contradict.
                // `sync(f)` is the source's promise that only `f`'s lambda
                // may make it pause ([ADR-244](../../../docs/specification/adr/adr-244.md)
                // D4) - the entry `std` has written by hand as `from(f)` since
                // ADR-029 D3.
                sync_claim: match (*is_sync, sync_by.is_empty()) {
                    (true, _) => Sync::Asserted,
                    (false, false) => Sync::From(
                        sync_by
                            .iter()
                            .map(|name| parsed.text(*name))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                    (false, true) => Sync::No,
                },
                // What the *declaration* says, which is nothing: a `.nika`
                // file has no syntax for a touch set, and there is no reason to
                // give it one - what a body reaches is read off the body.
                // `touch::infer` answers it afterwards, the way `sync` and
                // `throws` are answered ([ADR-067](../../../docs/specification/adr/adr-067.md)
                // D2). Until it has run, "nobody said" is the answer, and that
                // orders against everything (ADR-033 D4).
                touches: Vec::new(),
                touches_known: false,
                // The *declaration* says only that it can fail. Which errors
                // is a question about the body and about everything the body
                // reaches, so `throws::infer` answers it afterwards - the same
                // arrangement `sync` has since ADR-027.
                fails_with: if *throws {
                    vec![UNNAMED_ERROR.to_string()]
                } else {
                    Vec::new()
                },
                signature: Some(Signature {
                    // **The bounds, where the declaration writes them**
                    // ([ADR-205](../../../docs/specification/adr/adr-205.md) D1):
                    // `fn tell[T: greet::Speaks](x: T)` records `T: greet::Speaks`,
                    // and that is what lets a **consumer's** call be checked
                    // against it. Before this the bound lived only in the AST of
                    // the unit that declared the function, so a call from another
                    // package was answered by `rustc` about the type it picked
                    // (issue #162).
                    // **Only a parameter with a bound**: one with none is
                    // declared by its use in the signature (ADR-251 D4 writes
                    // every one in brackets), and every reader of this list
                    // asks for a bound.
                    bounds: generics
                        .iter()
                        .filter(|g| !g.bounds.is_empty())
                        .map(|g| {
                            (
                                parsed.text(g.name).to_string(),
                                g.bounds
                                    .iter()
                                    .map(|b| parsed.text(*b).to_string())
                                    .collect(),
                            )
                        })
                        .collect(),
                    params,
                    // **The declaration and not an inference** (ADR-094 D3):
                    // `mut out: Vec[i64]` is the claim that the caller's value
                    // changes, and a parameter without the word does not make
                    // it whatever its body does.
                    mutable: args
                        .iter()
                        .filter(|a| a.mutable)
                        .map(|a| parsed.text(a.name).to_string())
                        .collect(),
                    config: config
                        .iter()
                        .map(|c| ConfigContract {
                            name: parsed.text(c.name).to_string(),
                            ty: ty::Ty::from_ast(parsed, &c.ty).parameterise(&parameters),
                            default: literal_text(parsed, &c.default),
                        })
                        .collect(),
                    result: ret_type
                        .as_ref()
                        .map(|t| ty::Ty::from_ast(parsed, t).parameterise(&parameters)),
                }),
                borrows,
                // What the *declaration* says is nothing again, and this one
                // has no syntax at all: a parameter is a view unless the body
                // keeps it, which is a question about the body
                // ([ADR-094](../../../docs/specification/adr/adr-094.md) D2).
                // `keeps::infer` answers it afterwards. Empty until then, and
                // empty is the *permissive* answer here rather than the safe
                // one — which is why nothing may read this column before that
                // pass has run.
                keeps: Vec::new(),
                // **And this one is the declaration and not the body.** D3's
                // whole sentence is that mutation of a subject is written where
                // it is declared, so there is nothing to infer: `&mut self` is
                // the claim, and a receiver written `&self` or `self` is not.
                mutates: matches!(item, Item::Fn { receiver: Some(r), .. } if r.is_mut && r.is_ref),
                // **And this one is the body**, which `locks::infer` reads
                // afterwards for `keeps`' reason: it is a question about what
                // the whole call graph reaches, and nothing here has seen it
                // yet. `false` until then, and `false` is the *permissive*
                // answer, which is why nothing may read the column before that
                // pass has run.
                touches_a_lock: Lock::No,
                // **Nothing a `.nika` file declares says it**, and nothing here
                // infers it: a Nikaia function that wants another thread writes
                // a `task`, which the compiler sees and which is not this
                // question. `threads` is about a body written in *another*
                // language ([ADR-193](../../../docs/specification/adr/adr-193.md)
                // D1), so *nobody said* is the honest answer for every entry
                // this loop writes.
                threads: Threads::Undecided,
                // `sharing::infer` reads the bodies afterwards, for the same
                // reason `sync` does: the answer is about where a value goes
                // and not about how it was declared. Empty until then, which is
                // the floor written out - the safe answer needs no line.
                sharing: Vec::new(),
                // A `.nika` function cannot hand back a sequence (ADR-105 D4),
                // so there is no result for this column to be about.
                ends_by_length: false,
                // A source is where bytes enter the program from outside, and
                // nothing a `.nika` file can write is one: `fs` and `io` are
                // `std`, and `std` states its own (ADR-010 D2).
                provenance: None,
                // **The body's `assert`s, not its declaration**
                // ([ADR-269](../../../docs/specification/adr/adr-269.md) D18):
                // the prover publishes them after the check, through the
                // lowering. Empty until then.
                requires: Vec::new(),
                ensures: Vec::new(),
                from: Vec::new(),
            },
        )
    }

    /// A function by the name a caller wrote.
    ///
    /// **Exactly the name, since
    /// [ADR-154](../../../docs/specification/adr/adr-154.md)**: a `std` entry
    /// that lives in a module is reached through the module, `text::digit_value`
    /// and not `digit_value`, and what needs no prefix is the list on Part I's
    /// first page — whose entries are keyed **bare** here, so the exact lookup
    /// is the whole rule.
    ///
    /// It used to match on the last segment, which was name-for-name resolution
    /// (ADR-011 D2) rather than import tracking, and it is what a compiler
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
    /// checker's answer rather than this file's (ADR-028). But a question
    /// weaker than "which entry" can be answered without it: if **every**
    /// `::len` in the ledger reaches nothing, then `xs.len()` reaches nothing
    /// whatever `xs` turns out to be. An over-approximation over the
    /// candidates, which is the direction ADR-033 D4 requires.
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

    /// The file, as it is written out.
    ///
    /// Only what is *true* is recorded: a `sync = false` on every entry would
    /// treble the file and say nothing, and a diff should show a promise being
    /// made or withdrawn rather than a column of falses.
    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# AUTO-GENERATED by `nikaia`. Commit this file like a lockfile.\n");
        out.push_str("# Do not edit by hand - it is regenerated on every build.\n");
        out.push_str("#\n");
        out.push_str("# What a caller has to know about a function it cannot see the body of:\n");
        out.push_str("# whether it may pause (`sync`), whether it may fail and with what\n");
        out.push_str(
            "# (`throws`), and what its result may point into (`returns`). Part III, 13.5.\n",
        );
        out.push_str("#\n");
        out.push_str("# A `\"?\"` among the errors is the absence of a claim: it fails, with\n");
        out.push_str("# something this compiler cannot name.\n");
        self.render_from(&mut out);
        out
    }

    /// **What the contract beside it was derived from**
    /// ([ADR-251](../../../docs/specification/adr/adr-251.md) D1): the
    /// compiler, the inference, and the SHA-256 of every source it read. It
    /// decides whether a consumer may believe the contract
    /// ([ADR-100](../../../docs/specification/adr/adr-100.md) D3) and never
    /// what the contract says, which is why it is a file of its own.
    fn render_derived(&self) -> String {
        let mut out = String::new();
        out.push_str("# AUTO-GENERATED by `nikaia`, beside the contract it describes.\n");
        out.push_str("# What that contract was derived from: the compiler, the inference and\n");
        out.push_str("# the sources. A consumer believes the contract while the sources hash\n");
        out.push_str("# as recorded here. Part III, 13.5.\n");
        out.push_str(&format!("version = {}\n", self.version));
        out.push_str(&format!("toolchain = \"{}\"\n", self.toolchain));
        out.push_str(&format!("inference = \"{}\"\n", self.inference));
        if !self.sources.is_empty() {
            out.push_str("\n[sources]\n");
            for (unit, hash) in &self.sources {
                out.push_str(&format!("\"{unit}\" = \"{hash}\"\n"));
            }
        }
        out
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
    /// ([ADR-104](../../../docs/specification/adr/adr-104.md) D5).
    ///
    /// **A different header and the same body**, which is the whole of the
    /// difference: `render`'s says *do not edit by hand - it is regenerated on
    /// every build*, and that is exactly wrong here. A description is written
    /// once, **reviewed like code**, and hand-edited where a signature could
    /// not say what a field does — D5 expects the edit rather than tolerating
    /// it, and a file that told its reader not to make one would be telling
    /// them not to do the thing the record asks of them.
    fn render_description(&self, crate_name: &str, version: &str, notes: &Notes) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "# The boundary of `{crate_name}`, described before it is called\n"
        ));
        out.push_str("# ([ADR-104](../../docs/specification/adr/adr-104.md) D2).\n");
        out.push_str("#\n");
        out.push_str(&format!(
            "# Written by `nikaia describe {crate_name}` from the crate's `pub` signatures,\n"
        ));
        out.push_str(
            "# translated by Part III 15.2's table. **Committed and reviewed like code**,\n",
        );
        out.push_str("# and a hand edit is expected: what a signature cannot say is written\n");
        out.push_str("# fail-closed, and what it says wrongly is caught by the reviewer or by\n");
        out.push_str("# nobody (D5).\n");
        out.push_str("#\n");
        out.push_str(
            "# What the signatures do not say: no `touches` on any entry, which reads as\n",
        );
        out.push_str("# *touches everything*; no `locks`, which is that column's third answer.\n");
        out.push_str("# Neither is something a Rust signature can tell anybody, and reading\n");
        out.push_str("# silence as nothing at all is the polarity ADR-010 D1 forbids.\n");
        out.push_str("#\n");
        out.push_str(&format!("# crate: {crate_name} {version}\n"));
        // **What the describer saw and did not claim**
        // ([ADR-193](../../../../docs/specification/adr/adr-193.md) D3, D5),
        // before the entries because it is about the crate rather than about
        // one of them.
        if !notes.about_the_crate.is_empty() {
            out.push_str("#\n");
            for line in &notes.about_the_crate {
                out.push_str(&format!("# {line}\n"));
            }
        }
        self.render_from_with(&mut out, notes);
        out
    }

    /// The header line and everything after it, shared by both renderings.
    fn render_from(&self, out: &mut String) {
        self.render_from_with(out, &Notes::empty());
    }

    /// The same, with the describer's notes spliced above the entries they are
    /// about.
    ///
    /// **A comment and not a column** ([ADR-193](../../../../docs/specification/adr/adr-193.md)
    /// D3): what the describer saw does not *entail* an answer, so writing it
    /// as a claim could refuse a correct program. It is never parsed back —
    /// this is a sentence for the person who reviews the file, and the whole
    /// point is that they write the column or do not.
    fn render_from_with(&self, out: &mut String, notes: &Notes) {
        // **The contract and nothing it was derived from**
        // ([ADR-251](../../../docs/specification/adr/adr-251.md) D1): which
        // compiler wrote it, by which inference, from which sources, is
        // [`Ledger::render_derived`]'s file beside this one. A toolchain
        // upgrade or an edited comment in a source changes that file and not
        // this, so a diff here is a contract that moved.
        out.push_str(&format!("version = {}\n", self.version));

        for (name, contract) in &self.functions {
            out.push('\n');
            if let Some(lines) = notes.about_a_function.get(name) {
                for line in lines {
                    out.push_str(&format!("# {line}\n"));
                }
            }
            out.push_str(&format!("[fn.\"{name}\"]\n"));
            if contract.public {
                out.push_str("pub = true\n");
            }
            // `true` is the promise the source made, `"inferred"` the one the
            // body implies. Absent is still "not `sync`", so a reader that only
            // asks `is_sync` reads this file exactly as it did before.
            match &contract.sync_claim {
                Sync::Asserted => out.push_str("sync = true\n"),
                // `Unpromised` is the checker's reading of an `"inferred"`
                // from another package (ADR-244 D1) and is not meant to be
                // rendered; where it is, it is still that fact.
                Sync::Inferred | Sync::Unpromised => out.push_str("sync = \"inferred\"\n"),
                // **The source's own word** ([ADR-244](../../../../docs/specification/adr/adr-244.md)
                // D4, [ADR-251](../../../../docs/specification/adr/adr-251.md) D4).
                Sync::From(name) => out.push_str(&format!("sync = \"sync({name})\"\n")),
                Sync::No => {}
            }
            if !contract.fails_with.is_empty() {
                out.push_str(&format!("throws = {}\n", throws_text(&contract.fails_with)));
            }
            // **In Nikaia's spelling** (ADR-251 D4): what the result points
            // into is written `ref(a | b)` in the signature's result, and the
            // receiver as the source writes it. `returns` and `mutates` are
            // written only where the signature has no place for them - a
            // result with no `ref` in it, a function with no receiver.
            let spelled = contract.signature.as_ref().map(|signature| {
                nikaia_std::tools::ledger::spell(
                    &signature.text(),
                    name,
                    &contract.borrows,
                    contract.mutates,
                )
            });
            let (borrows_said, mutates_said) = spelled
                .as_ref()
                .map(|s| (s.borrows_said, s.mutates_said))
                .unwrap_or((false, false));
            if !contract.borrows.is_empty() && !borrows_said {
                out.push_str(&format!(
                    "returns = \"ref({})\"\n",
                    contract.borrows.join(" | ")
                ));
            }
            // Beside `returns`, which is the other thing a caller reads off a
            // signature about where a value goes
            // ([ADR-094](../../../../docs/specification/adr/adr-094.md) D2).
            if !contract.keeps.is_empty() {
                out.push_str(&format!(
                    "keeps = [{}]\n",
                    contract
                        .keeps
                        .iter()
                        .map(|p| format!("\"{p}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            // Beside `keeps`, because the two together are what a caller has to
            // know before it may hand a name over rather than lend it
            // ([ADR-094](../../../../docs/specification/adr/adr-094.md) D3).
            if contract.mutates && !mutates_said {
                out.push_str("mutates = true\n");
            }
            // Beside `touches`, which is the other column about what a body
            // reaches ([ADR-039](../../../../docs/specification/adr/adr-039.md) D3).
            match contract.touches_a_lock {
                Lock::No => {}
                Lock::Holds => out.push_str("locks = true\n"),
                // `"?"` is the absence of a claim, which is what it means in
                // `throws` (ADR-024 D1) said once more.
                Lock::Undecided => out.push_str("locks = \"?\"\n"),
            }
            // Beside `locks`, because both are claims about what a body does
            // that no signature shows and a person writes
            // ([ADR-193](../../../../docs/specification/adr/adr-193.md) D1).
            match contract.threads {
                Threads::Undecided => {}
                Threads::May => out.push_str("threads = true\n"),
                Threads::MayNot => out.push_str("threads = false\n"),
            }
            if contract.touches_known {
                out.push_str(&format!(
                    "touches = [{}]\n",
                    contract
                        .touches
                        .iter()
                        .map(|t: &touch::Touch| format!("\"{}\"", t.text()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if let Some(provenance) = contract.provenance {
                out.push_str(&format!("provenance = \"{}\"\n", provenance.as_str()));
            }
            if !contract.views.is_empty() {
                out.push_str(&format!(
                    "views = [{}]\n",
                    contract
                        .views
                        .iter()
                        .map(|held| format!("\"{}\"", held.text()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if !contract.sharing.is_empty() {
                out.push_str(&format!(
                    "sharing = [{}]\n",
                    contract
                        .sharing
                        .iter()
                        .map(|class| format!("\"{}\"", class.text()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if contract.ends_by_length {
                out.push_str("ends_by_length = true\n");
            }
            // **The function's contract from its `assert`s**
            // ([ADR-269](../../../../docs/specification/adr/adr-269.md) D18):
            // what a caller establishes, what it may rely on, and the `assert`
            // each came from.
            for (key, list) in [
                ("requires", &contract.requires),
                ("ensures", &contract.ensures),
                ("from", &contract.from),
            ] {
                if !list.is_empty() {
                    out.push_str(&format!(
                        "{key} = [{}]\n",
                        list.iter()
                            .map(|c| format!("\"{}\"", escape(c)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            if let Some(spelled) = &spelled {
                out.push_str(&format!("signature = \"{}\"\n", escape(&spelled.text)));
            }
        }

        // **A trait a package publishes** ([ADR-106](../../../../docs/specification/adr/adr-106.md)
        // D3). The table carries the one word a checker needs — *this name is a
        // trait* — and no `fields`: its **methods are the `fn` entries above**,
        // under `Handler::handle`, which is the key shape an `impl`'s get and the
        // one `NK1130` already compares against. Writing them twice would be a
        // second source of truth for one fact.
        //
        // [ADR-078](../../../../docs/specification/adr/adr-078.md) §4 left this
        // as *a question about modules*; D1 and D3 of that later record answered
        // it, and until they were built a bound could not name a path and nothing
        // outside a unit could name one of these traits.
        for name in self.traits.keys() {
            out.push_str(&format!("\n[trait.\"{name}\"]\n"));
        }

        // **And each `impl`, in the ledger of the package that wrote it**
        // ([ADR-106](../../../../docs/specification/adr/adr-106.md) D4). An
        // `impl` may be written in the trait's package, in the type's, or in a
        // consumer for its own type, so no single ledger can list a trait's
        // implementors completely — and a list read as complete would turn
        // absence into an answer, which
        // [ADR-010](../../../../docs/specification/adr/adr-010.md) D1 forbids.
        // Each ledger says only what it wrote, and the question at a call is
        // answered over every ledger this program reads plus its own.
        for (trait_name, types) in &self.implementations {
            for ty in types {
                out.push_str(&format!("\n[impl.\"{trait_name} for {ty}\"]\n"));
            }
        }

        for (name, contract) in &self.types {
            out.push_str(&format!("\n[type.\"{name}\"]\n"));
            if contract.public {
                out.push_str("pub = true\n");
            }
            if !contract.fields.is_empty() {
                out.push_str(&format!(
                    "fields = [{}]\n",
                    contract
                        .fields
                        .iter()
                        // `pub ` in front, where it is - the same word the
                        // source writes, so the line reads as the declaration it
                        // came from.
                        .map(|field| {
                            format!(
                                "\"{}{}: {}\"",
                                match field.public {
                                    true => "pub ",
                                    false => "",
                                },
                                field.name,
                                field.ty.text()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            // **The other shape of type**: an `enum`'s cases, which is what lets
            // a consumer's `match` be total (Part I 3.4).
            if !contract.variants.is_empty() {
                out.push_str(&format!(
                    "variants = [{}]\n",
                    contract
                        .variants
                        .iter()
                        .map(|variant| format!("\"{}\"", variant.text()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            // Both claims are written and the third is silence, which is what
            // makes every ledger already on disk mean what it meant
            // ([ADR-123](../../../../docs/specification/adr/adr-123.md) D1).
            match contract.crosses {
                Crosses::May => out.push_str("crosses = true\n"),
                Crosses::MayNot => out.push_str("crosses = false\n"),
                Crosses::Undecided => {}
            }
            if contract.compares {
                out.push_str("compares = true\n");
            }
            if contract.copies {
                out.push_str("copies = true\n");
            }
            if contract.iterates_fallibly {
                out.push_str("iterates = \"throws\"\n");
            }
            if !contract.touches.is_empty() {
                out.push_str(&format!(
                    "touches = [{}]\n",
                    contract
                        .touches
                        .iter()
                        .map(|t| format!("\"{t}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if !contract.tethered.is_empty() {
                out.push_str(&format!(
                    "tethered = [{}]\n",
                    contract
                        .tethered
                        .iter()
                        .map(|f| format!("\"{f}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
    }

    /// Read a ledger back - a library's, or this project's own.
    ///
    /// Deliberately a small reader for the small format `render` writes rather
    /// than a TOML parser: the file is generated, so the shapes it can take are
    /// the shapes written above, and a dependency to read one's own output back
    /// is a dependency to keep in step.
    /// **Read in Nikaia** (`tools/ledger.nika`, ADR-257 step (c)): the file's
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
/// ([ADR-109](../../../docs/specification/adr/adr-109.md) D1): a trait method
/// reads like a function type, so without `sync` it **may pause**, exactly as a
/// function without the word may.
///
/// **It used to be asserted whatever the declaration said**
/// ([ADR-078](../../../docs/specification/adr/adr-078.md) D4), and that was a
/// decision rather than a default: `async fn` in a trait was something the
/// emitter had no way to ask for, so a plain `fn` was the only thing it could
/// write, and a trait whose method genuinely pauses was **refused** rather than
/// mis-lowered (`NK1129`). ADR-109 D3 takes the cause away: the trait declares
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
    let mut params: Vec<(String, ty::Ty)> = Vec::new();
    if let Some(receiver) = &method.receiver {
        params.push((
            "self".to_string(),
            receiver_type(parsed, receiver, Some(trait_name)),
        ));
    }
    params.extend(method.args.iter().map(|a| {
        (
            parsed.text(a.name).to_string(),
            ty::Ty::from_ast(parsed, &a.ty),
        )
    }));
    (
        format!("{trait_name}::{}", parsed.text(method.name)),
        FnContract {
            public,
            // ADR-109 D1: the declaration's own word, and its absence is the
            // claim that it may pause.
            sync_claim: match method.is_sync {
                true => Sync::Asserted,
                false => Sync::No,
            },
            fails_with: if method.can_throw {
                vec![UNNAMED_ERROR.to_string()]
            } else {
                Vec::new()
            },
            signature: Some(Signature {
                // A described foreign function's bounds are Rust's, and a Nikaia
                // caller picks no type for one: the describer writes the
                // signature and nothing in it is generic.
                bounds: Vec::new(),
                params,
                mutable: method
                    .args
                    .iter()
                    .filter(|a| a.mutable)
                    .map(|a| parsed.text(a.name).to_string())
                    .collect(),
                config: method
                    .config
                    .iter()
                    .map(|c| ConfigContract {
                        name: parsed.text(c.name).to_string(),
                        ty: ty::Ty::from_ast(parsed, &c.ty),
                        default: literal_text(parsed, &c.default),
                    })
                    .collect(),
                result: method
                    .ret_type
                    .as_ref()
                    .map(|t| ty::Ty::from_ast(parsed, t)),
            }),
            ..FnContract::empty()
        },
    )
}

fn receiver_type(parsed: &Parsed, receiver: &crate::ast::Receiver, target: Option<&str>) -> ty::Ty {
    let _ = parsed;
    let name = target.unwrap_or("Self");
    if receiver.is_ref {
        ty::Ty::view(name)
    } else {
        ty::Ty::named(name)
    }
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

/// A literal, written back the way the source wrote it.
///
/// Only a literal reaches here - the grammar allows nothing else as a default -
/// and Nikaia spells every one of them the way the language below does
/// (ADR-011 D2), which is what lets a ledger record the text and an emitter
/// print it.
fn literal_text(parsed: &Parsed, expr: &crate::ast::Expr) -> String {
    use crate::ast::{Expr, UnaryOp};
    let _ = parsed;
    match expr {
        Expr::LitBool(true) => "true".to_string(),
        Expr::LitBool(false) => "false".to_string(),
        // A default of `null` is a default of `None`, and the ledger records
        // what the emitter prints (ADR-011 D2).
        Expr::LitNull => "None".to_string(),
        Expr::LitInt { value, negative } => crate::ast::int_value(*value, *negative).to_string(),
        Expr::LitFloat(f) => f.clone(),
        Expr::LitChar(c) => format!("'{c}'"),
        Expr::LitStr { text: s, .. } => format!("\"{s}\""),
        // A default is a constant (Kap 5.1), and `f"…"` is a call to `format!`.
        // The parser admits it here, so this says no in words rather than
        // recording something a reader would take for text.
        Expr::LitInterpolated { .. } => {
            unreachable!("a default is a literal, and `f\"…\"` is built at run time")
        }
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
        } => format!("-{}", literal_text(parsed, expr)),
        // The grammar admits nothing else, so this is unreachable rather than
        // a case with an answer.
        other => unreachable!("a default is a literal, found {other:?}"),
    }
}

/// A value that may itself hold a quote - which a signature does, the moment an
/// option's default is a string: `method: &str = "GET"`.
/// **And a `\n` becomes `\\n`**, which is
/// [ADR-139](../../../docs/specification/adr/adr-139.md) D2's one demand on
/// this format: a doc comment holds its line breaks and the file is read a
/// line at a time. Nothing else written here has ever held one, so every
/// ledger already on disk renders and reads back exactly as it did.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// The name an error gets when the compiler cannot name it - ADR-024 D1's `?`,
/// which is the absence of a claim rather than a type.
pub const UNNAMED_ERROR: &str = "?";

/// What a parse fails with
/// ([ADR-173](../../../docs/specification/adr/adr-173.md) D1).
///
/// A `std` type with no module in front, which is `Overtaken`'s shape: a
/// program never writes a path to it, because it arrives in a `catch`.
pub const PARSE_ERROR: &str = "ParseError";

/// The one parameter every grammar entry takes: the text to parse
/// ([ADR-082](../../../docs/specification/adr/adr-082.md) D1).
///
/// A name rather than four spellings of it, because three analyses have to
/// agree on it: the loop below writes the signature, [`tether::infer`] writes
/// the state of the views a parse hands back, and [`keeps::infer`] says whether
/// the entry keeps it. A column that names a position no signature has is a
/// column nobody can read.
pub const INPUT: &str = "input";

/// The `throws` list exactly as the ledger writes it.
///
/// One function, so that a diagnostic quoting the contract quotes the bytes a
/// reader will find in `nikaia.contracts` rather than a paraphrase of them -
/// Part III C.4's rule that **the note is the contract**.
pub fn throws_text(throws: &[String]) -> String {
    let names = throws
        .iter()
        .map(|e| format!("\"{e}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{names}]")
}

/// The types Part I 2.2 offers, by name.
///
/// Here rather than beside a diagnostic because two questions read it: whether
/// `as` names a type this language has ([ADR-054](../../../docs/specification/adr/adr-054.md)
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
            // ([ADR-147](../../../docs/specification/adr/adr-147.md) D3). It
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
/// ([ADR-074](../../../docs/specification/adr/adr-074.md) D4).
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
