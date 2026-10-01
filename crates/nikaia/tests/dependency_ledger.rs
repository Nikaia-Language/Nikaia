//! A consumer reads a dependency's ledger, and derives it again only where its
//! sources changed ([ADR-100](../../../docs/specification/adr/adr-100.md) D1
//! and D3).
//!
//! **Why believing is the correct answer and not merely the quick one.** A
//! package's own build has that package's own dependencies in view and a
//! consumer's build deliberately does not
//! ([ADR-053](../../../docs/specification/adr/adr-053.md) D3), so an answer
//! derived on the consumer's side can only be the same or worse — and two
//! builds deriving one function differently is a program that awaits an `i64`.
//! The reverted settling pass in the issue tracker is that failure, measured.
//!
//! **And why it is safe.** A ledger is never believed against its own sources:
//! the header names the units it was derived from and their SHA-256, and a hash
//! that does not match is a derivation rather than a belief. Everything here is
//! about that one decision — which is why half of these tests are about the
//! ledger *not* being believed.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use nikaia::contracts::{Ledger, STD, Sync};
use nikaia::modules::{Dependency, Program};

/// A program and one package it depends on by path, as directories.
fn a_program_and_a_package(purpose: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = common::scratch_dir(purpose);
    std::fs::create_dir_all(dir.join("app/src")).expect("the program");
    std::fs::create_dir_all(dir.join("lib/src")).expect("the package");
    for (file, contents) in files {
        std::fs::write(dir.join(file), contents).expect("a source");
    }
    dir
}

/// What `project::packages_of` hands `Program::read_with`, built here so the
/// test can put a ledger beside the package and say what happens.
fn depends_on(root: &Path) -> Vec<Dependency> {
    vec![Dependency {
        name: "lib".to_string(),
        root: root.to_path_buf(),
        reachable: BTreeSet::new(),
        renames: BTreeMap::new(),
    }]
}

const LIB: &str = "pub fn hello() -> i64 {\n    return 1\n}\n";
const APP: &str = "use lib\n\nfn caller() -> i64 {\n    return lib::hello()\n}\n\nfn main() { }\n";

/// **The consumer resolves a call into its dependency**, which is the whole of
/// D1 from the consumer's side: `caller` earns `sync` because `lib::hello` is an
/// answer rather than an absence.
#[test]
fn a_call_into_a_dependency_is_answered_from_its_ledger() {
    let dir = a_program_and_a_package(
        "dependency-ledger-resolves",
        &[("lib/src/main.nika", LIB), ("app/src/main.nika", APP)],
    );

    let program = Program::read_with(
        &dir.join("app/src/main.nika"),
        &depends_on(&dir.join("lib")),
    )
    .expect("the program reads");
    assert_eq!(program.contracts.functions["caller"].sync, Sync::Inferred);
    assert_eq!(
        program.contracts.functions["lib::hello"].sync,
        Sync::Inferred
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **A ledger whose hashes match is believed**, down to a claim the sources do
/// not support.
///
/// The claim is the test. A ledger saying `hello` **pauses** is one no
/// inference over those sources would produce, so a consumer that comes out
/// with `Sync::No` read the file rather than the body — which is what being
/// believed means, and the only way to show it without counting work.
#[test]
fn a_ledger_whose_hashes_match_is_believed() {
    let dir = a_program_and_a_package(
        "dependency-ledger-believed",
        &[("lib/src/main.nika", LIB), ("app/src/main.nika", APP)],
    );

    let mut shipped = Ledger::empty();
    shipped.sources = BTreeMap::from([(
        "main.nika".to_string(),
        orchestrator::cache::sha256_hex(LIB.as_bytes()),
    )]);
    shipped.functions.insert(
        "hello".to_string(),
        nikaia::contracts::FnContract {
            public: true,
            sync: Sync::No,
            ..Default::default()
        },
    );
    std::fs::write(dir.join("lib/nikaia.contracts"), shipped.render()).expect("ship it");
    // **The hashes are in the record beside it** (ADR-251 D1).
    std::fs::write(dir.join("lib/nikaia.derived"), shipped.render_derived())
        .expect("and what it was derived from");

    let program = Program::read_with(
        &dir.join("app/src/main.nika"),
        &depends_on(&dir.join("lib")),
    )
    .expect("the program reads");
    assert_eq!(
        program.contracts.functions["lib::hello"].sync,
        Sync::No,
        "the shipped answer is the one that counts, not this build's"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **And a ledger whose hashes do not is derived again.**
///
/// The same false claim, with one byte of the source changed after the ledger
/// was written. Believing here would be reading a promise about a file that is
/// no longer there, which is the polarity
/// [ADR-010](../../../docs/specification/adr/adr-010.md) D1 forbids: a stale
/// `touches` is a data race with no message.
#[test]
fn a_ledger_whose_sources_moved_is_derived_again() {
    let dir = a_program_and_a_package(
        "dependency-ledger-stale",
        &[("lib/src/main.nika", LIB), ("app/src/main.nika", APP)],
    );

    let mut shipped = Ledger::empty();
    shipped.sources = BTreeMap::from([(
        "main.nika".to_string(),
        orchestrator::cache::sha256_hex(LIB.as_bytes()),
    )]);
    shipped.functions.insert(
        "hello".to_string(),
        nikaia::contracts::FnContract {
            public: true,
            sync: Sync::No,
            ..Default::default()
        },
    );
    std::fs::write(dir.join("lib/nikaia.contracts"), shipped.render()).expect("ship it");
    std::fs::write(dir.join("lib/nikaia.derived"), shipped.render_derived())
        .expect("and what it was derived from");
    std::fs::write(
        dir.join("lib/src/main.nika"),
        "pub fn hello() -> i64 {\n    return 2\n}\n",
    )
    .expect("the source moves on without it");

    let program = Program::read_with(
        &dir.join("app/src/main.nika"),
        &depends_on(&dir.join("lib")),
    )
    .expect("the program reads");
    assert_eq!(
        program.contracts.functions["lib::hello"].sync,
        Sync::Inferred,
        "a hash that does not match is a derivation, never a belief"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **A ledger with no `[sources]` at all is not believed either.**
///
/// It was written before the table existed, or by hand, and *nothing was
/// recorded* must not read as *nothing changed*. Free to get right and
/// impossible to notice getting wrong, which is why it is a test of its own.
#[test]
fn a_ledger_that_records_no_sources_is_not_believed() {
    let dir = a_program_and_a_package(
        "dependency-ledger-sourceless",
        &[("lib/src/main.nika", LIB), ("app/src/main.nika", APP)],
    );

    let mut shipped = Ledger::empty();
    shipped.functions.insert(
        "hello".to_string(),
        nikaia::contracts::FnContract {
            public: true,
            sync: Sync::No,
            ..Default::default()
        },
    );
    let rendered = shipped.render();
    assert!(!rendered.contains("[sources]"), "{rendered}");
    std::fs::write(dir.join("lib/nikaia.contracts"), rendered).expect("ship it");

    let program = Program::read_with(
        &dir.join("app/src/main.nika"),
        &depends_on(&dir.join("lib")),
    )
    .expect("the program reads");
    assert_eq!(
        program.contracts.functions["lib::hello"].sync,
        Sync::Inferred
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **An unreadable ledger is a slower build and not a failed one.**
///
/// It is a generated file; a person may have edited it and an older compiler may
/// have written it. The answer to both is the answer to a stale one — derive
/// this package again — and a build that stopped because a *cache* would not
/// parse would be the worse build.
#[test]
fn a_ledger_that_does_not_parse_is_derived_again_rather_than_refused() {
    let dir = a_program_and_a_package(
        "dependency-ledger-unreadable",
        &[("lib/src/main.nika", LIB), ("app/src/main.nika", APP)],
    );
    std::fs::write(dir.join("lib/nikaia.contracts"), "this is not a ledger\n").expect("ship it");

    let program = Program::read_with(
        &dir.join("app/src/main.nika"),
        &depends_on(&dir.join("lib")),
    )
    .expect("an unreadable ledger is not a failed build");
    assert_eq!(
        program.contracts.functions["lib::hello"].sync,
        Sync::Inferred
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The `[sources]` table survives the round trip, which `--locked` needs: it
/// compares bytes, so a table that rendered differently than it parsed would
/// fail a build that changed nothing. **It is the derivation record's and not
/// the contract's** (ADR-251 D1): the contract renders no hash.
#[test]
fn the_sources_table_renders_and_parses_back() {
    let mut ledger = Ledger::empty();
    ledger.sources = BTreeMap::from([
        ("helper.nika".to_string(), "a".repeat(64)),
        ("main.nika".to_string(), "b".repeat(64)),
    ]);
    assert!(!ledger.render().contains("[sources]"));
    let rendered = ledger.render_derived();
    assert!(rendered.contains("[sources]"), "{rendered}");

    let read = Ledger::parse(&rendered).expect("its own output parses");
    assert_eq!(read.sources, ledger.sources);
    assert_eq!(read.render_derived(), rendered);

    // And a ledger that knows no sources renders exactly the file it always
    // rendered - `std`'s among them, whose Rust half has no `.nika` to hash.
    let bare = Ledger::empty();
    assert!(!bare.render().contains("[sources]"));
    assert!(
        !Ledger::parse(STD)
            .expect("std's ledger parses")
            .render()
            .contains("[sources]")
    );
}

/// What `stale_against` answers, in the three shapes a package can move in.
#[test]
fn a_unit_that_moved_a_unit_that_arrived_and_a_unit_that_left() {
    let mut ledger = Ledger::empty();
    ledger.sources = BTreeMap::from([
        ("main.nika".to_string(), "a".repeat(64)),
        ("helper.nika".to_string(), "b".repeat(64)),
    ]);

    assert!(ledger.stale_against(&ledger.sources.clone()).is_empty());

    let changed = BTreeMap::from([
        ("main.nika".to_string(), "c".repeat(64)),
        ("helper.nika".to_string(), "b".repeat(64)),
    ]);
    assert_eq!(ledger.stale_against(&changed), ["main.nika"]);

    let arrived = BTreeMap::from([
        ("main.nika".to_string(), "a".repeat(64)),
        ("helper.nika".to_string(), "b".repeat(64)),
        ("extra.nika".to_string(), "d".repeat(64)),
    ]);
    assert_eq!(ledger.stale_against(&arrived), ["extra.nika"]);

    // A file that left takes its entries with it, so it is as much a reason to
    // derive again as one that changed.
    let left = BTreeMap::from([("main.nika".to_string(), "a".repeat(64))]);
    assert_eq!(ledger.stale_against(&left), ["helper.nika"]);
}

/// **A believed ledger hands over the package's own entries and nothing
/// further** ([ADR-053](../../../docs/specification/adr/adr-053.md) D3).
///
/// A library's ledger file is its build's record, so it carries its own
/// dependencies' entries under their names. A consumer absorbing those under
/// *its* name for the library produces `lib::c::Id` — a type nothing declares
/// and no program can write — and that is what a real diamond produced before
/// this filter existed, in `project.rs`'s own test.
#[test]
fn a_transitive_packages_entries_are_not_absorbed_with_its_parents() {
    let mut shipped = Ledger::empty();
    shipped
        .functions
        .insert("show".to_string(), nikaia::contracts::FnContract::default());
    shipped.functions.insert(
        "c::two".to_string(),
        nikaia::contracts::FnContract::default(),
    );
    shipped.types.insert(
        "Row".to_string(),
        nikaia::contracts::TypeContract::default(),
    );
    shipped.types.insert(
        "c::Id".to_string(),
        nikaia::contracts::TypeContract::default(),
    );

    let published = shipped.published(&BTreeSet::from(["c".to_string()]));
    assert!(published.functions.contains_key("show"));
    assert!(published.types.contains_key("Row"));
    assert!(!published.functions.contains_key("c::two"));
    assert!(!published.types.contains_key("c::Id"));

    // A `Type::method` key is not a package's, and losing it would take every
    // method of every type the package declares.
    let mut with_methods = Ledger::empty();
    with_methods.functions.insert(
        "Row::new".to_string(),
        nikaia::contracts::FnContract::default(),
    );
    assert!(
        with_methods
            .published(&BTreeSet::from(["c".to_string()]))
            .functions
            .contains_key("Row::new")
    );
}
