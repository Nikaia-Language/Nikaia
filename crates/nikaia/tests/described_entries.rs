//! A description's **entries** reach the analyses
//! ([ADR-290](../../../docs/specification/adr/adr-290.md) D1).
//!
//! D1's first sentence is the whole point of the file it asks for: *every
//! analysis reaches to the boundary and reads an entry there*, and `NK2504`'s
//! own message promises a reader four answers — what may cross a thread, what
//! the call may reach, whether it pauses, whether it can fail — in return for
//! writing it.
//!
//! **For two records it delivered none of them.** `contracts/<crate>.contracts`
//! was read to see whether it *parsed*, the parsed ledger was dropped on the
//! floor, and the file's only effect was silencing the refusal that had asked
//! for it. Measured on a three-line project: a description saying
//! `(a: i64, b: i64) -> i64` left a one-argument call unremarked.
//!
//! So these are the four questions asked of a described boundary, in a project
//! on disk — a manifest, a `contracts/` beside it, and one `.nika` — because
//! `Foreign::of` reads files and a ledger handed in by a test would prove
//! nothing about the wiring.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, LedgerOps};
use nikaia::parser::parse_to_ast;
use nikaia::project::Foreign;

/// A project on disk: `nikaia.toml` declaring one Rust crate, and whatever
/// description is given for it.
fn project(name: &str, description: Option<&str>) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "nikaia-described-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("contracts")).expect("a directory to work in");
    std::fs::create_dir_all(root.join("src")).expect("a directory to work in");
    std::fs::write(
        root.join("nikaia.toml"),
        "[package]\n\
         name = \"probe\"\n\
         version = \"0.1.0\"\n\
         \n\
         [dependencies]\n\
         fremd = { type = \"rust\", path = \"fremd\" }\n",
    )
    .expect("write the manifest");
    if let Some(description) = description {
        std::fs::write(root.join("contracts/fremd.contracts"), description)
            .expect("write the description");
    }
    root
}

/// The ledger, the modules and the findings a build of `root` would produce for
/// one source — assembled exactly as `project::check` assembles them.
fn findings(root: &Path, source: &str) -> Vec<Finding> {
    let found = findings_keeping(root, source);
    let _ = std::fs::remove_dir_all(root);
    found
}

/// The same, leaving the project where it is — for a test that asks twice.
fn findings_keeping(root: &Path, source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let foreign = Foreign::of(root);
    let library = foreign
        .library()
        .expect("std's ledger and the descriptions");
    let modules = foreign.packages(&BTreeSet::new());
    let mut found = check::check_program(&parsed, &own, &library, &modules).findings;
    found.extend(nikaia::foreign::check(
        &parsed,
        &foreign.declared,
        &foreign.described,
        &foreign.moved,
    ));
    found
}

const DESCRIPTION: &str = "version = 2\n\
                           inference = \"described-from-signatures\"\n\
                           \n\
                           [fn.\"fremd::zwei\"]\n\
                           pub = true\n\
                           sync = true\n\
                           signature = \"(a: i64, b: i64) -> i64\"\n\
                           \n\
                           [fn.\"fremd::kann_scheitern\"]\n\
                           pub = true\n\
                           sync = true\n\
                           signature = \"() -> i64 throws\"\n\
                           \n\
                           [fn.\"fremd::pausiert\"]\n\
                           pub = true\n\
                           sync = false\n\
                           signature = \"() -> i64\"\n";

/// **The arity of a described call is checked**, which is the smallest thing a
/// signature can be asked and the measurement that found the gap.
#[test]
fn a_described_signature_reaches_the_type_checker() {
    let root = project("arity", Some(DESCRIPTION));
    let found = findings(
        &root,
        "fn main() {\n    let n = fremd::zwei(1)\n    println(f\"{n}\")\n}",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1101");
    assert!(found[0].message.contains("fremd::zwei"), "{:?}", found[0]);
}

/// **And its result has a type**, which is the other half of a signature: the
/// call is not a `?` any more, so what is done with what it hands back is
/// checked like anything else.
#[test]
fn a_described_call_hands_back_a_type() {
    let root = project("result", Some(DESCRIPTION));
    let found = findings(
        &root,
        "fn zahl() -> String {\n    fremd::zwei(1, 2)\n}\n\
         fn main() { println(zahl()) }",
    );
    assert!(
        found.iter().any(|f| f.code == "NK1104"),
        "the described `-> i64` is what a `String` is compared against: {found:#?}"
    );
}

/// **Whether it can fail** — the third of `NK2504`'s four promises, and the one
/// a `throws` in the signature answers.
#[test]
fn a_described_call_that_can_fail_is_a_place_that_says_so() {
    let root = project("throws", Some(DESCRIPTION));
    let found = findings(
        &root,
        "fn zahl() -> i64 {\n    fremd::kann_scheitern()\n}\n\
         fn main() { println(f\"{zahl()}\") }",
    );
    assert!(
        found.iter().any(|f| f.code == "NK1104"),
        "an `i64 throws` does not fit a declared `i64`: {found:#?}"
    );
}

/// **Whether it pauses** — the fourth promise, and the one the `sync` column
/// answers. A `sync` function may not call something that can pause
/// (`NK2202`), and a described `sync = false` is what says a foreign call can.
#[test]
fn a_described_call_that_pauses_is_refused_inside_a_sync_function() {
    let root = project("pauses", Some(DESCRIPTION));
    let parsed = parse_to_ast("fn im_lock() -> i64 sync { return fremd::pausiert() }")
        .expect("the source parses");
    let own = Ledger::infer(&parsed);
    let foreign = Foreign::of(&root);
    let library = foreign
        .library()
        .expect("std's ledger and the descriptions");
    let found = nikaia::contracts::sync::check(&parsed, &own, &library);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(found.len(), 1, "{found:#?}");

    // And the one that says `sync = true` is not refused, which is the pair:
    // the column is read rather than a foreign call being assumed to pause.
    let root = project("does-not-pause", Some(DESCRIPTION));
    let parsed = parse_to_ast("fn im_lock() -> i64 sync { return fremd::zwei(1, 2) }")
        .expect("the source parses");
    let own = Ledger::infer(&parsed);
    let foreign = Foreign::of(&root);
    let library = foreign
        .library()
        .expect("std's ledger and the descriptions");
    let found = nikaia::contracts::sync::check(&parsed, &own, &library);
    let _ = std::fs::remove_dir_all(&root);
    assert!(found.is_empty(), "{found:#?}");
}

/// **And the same project without the file gets `NK2504` and nothing else**,
/// which is the pair: the refusal and the answers are two sides of one file,
/// and it is the second that was missing.
#[test]
fn an_undescribed_crate_is_refused_and_asked_nothing() {
    let root = project("absent", None);
    let found = findings(
        &root,
        "fn main() {\n    let n = fremd::zwei(1)\n    println(f\"{n}\")\n}",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK2504");
}

/// **A described crate is not a `std` module.** The set of words that may stand
/// in front of a `::` is derived from the library ledger's own keys, so merging
/// a description into it made `fremd` look like one of `std`'s — and the first
/// thing a program calling it was told was to write `use std::fremd`.
///
/// Asserted rather than left to be rediscovered: it is the one thing the merge
/// could break that no other test would notice, because it turns a correct
/// program into a refusal (Part III, C.4).
#[test]
fn a_described_crate_is_not_something_to_import_from_std() {
    let root = project("import", Some(DESCRIPTION));
    let found = findings(
        &root,
        "fn main() {\n    let n = fremd::zwei(1, 2)\n    println(f\"{n}\")\n}",
    );
    assert!(
        found.is_empty(),
        "a correct program is not refused: {found:#?}"
    );
}

/// **A description is believed while its hashes hold** — `NK2505`
/// ([ADR-290](../../../docs/specification/adr/adr-290.md) D5, on
/// [ADR-100](../../../docs/specification/adr/adr-100.md) D3's rule).
///
/// The description records what the crate's sources hashed to, and until this
/// **nothing compared it**: a crate could change under a reviewed file and
/// every analysis would go on reading the old answers. D3's own words for a
/// ledger are *a dependency is never believed against its own sources*.
///
/// **The one difference from a ledger is what happens next.** D3's second row
/// derives the ledger again; a description cannot be, because what it says is a
/// reviewer's judgement — a `crosses` read off a field, a signature that lies
/// corrected. So the row becomes a refusal and the command, which is the same
/// thing said to a person instead of to a build.
#[test]
fn a_description_is_believed_while_its_hashes_hold() {
    let root = project("hashes", None);
    // The crate, where the manifest's `fremd` means beside `nikaia.toml`
    // ([ADR-286](../../../docs/specification/adr/adr-286.md) D16). It used to
    // mean *from the generated manifest's directory*, which is a place the
    // author never sees and which Cargo stopped agreeing with.
    let crate_root = root.join("fremd");
    std::fs::create_dir_all(crate_root.join("src")).expect("a crate to hash");
    let source = crate_root.join("src/lib.rs");
    std::fs::write(&source, "pub fn zwei(a: i64, b: i64) -> i64 { a + b }\n")
        .expect("write the crate");
    let hash = orchestrator::cache::sha256_hex(&std::fs::read(&source).expect("read it back"));
    std::fs::write(
        root.join("contracts/fremd.contracts"),
        format!("{DESCRIPTION}\n[sources]\n\"src/lib.rs\" = \"{hash}\"\n"),
    )
    .expect("write the description");

    let calls = "fn main() {\n    let n = fremd::zwei(1, 2)\n    println(f\"{n}\")\n}";
    assert!(
        Foreign::of(&root).moved.is_empty(),
        "the hash holds, so nothing has moved"
    );
    assert!(
        findings_keeping(&root, calls).is_empty(),
        "and nothing is said"
    );

    // …and the moment the crate moves, the file is an answer about a crate
    // this build is not linking against.
    std::fs::write(&source, "pub fn zwei(a: i64, b: i64) -> i64 { a * b }\n")
        .expect("move the crate");
    let found = findings(&root, calls);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK2505");
    assert!(
        found[0]
            .message
            .contains("`fremd` has changed since its description was reviewed"),
        "{:?}",
        found[0]
    );
    assert!(
        found[0].notes.join(" ").contains("`src/lib.rs`"),
        "{found:#?}"
    );
    // Part III C.2: the way out is one concrete thing to do.
    assert_eq!(
        found[0].help.as_deref(),
        Some("Run `nikaia describe fremd` and review the diff before committing it.")
    );
}

/// **A description with nothing to compare says nothing**, which is the
/// polarity rather than an omission: a crate declared by version has its
/// sources in Cargo's registry cache, and a description that recorded no
/// `[sources]` recorded nothing. A refusal resting on an absence would refuse
/// every crate that comes from a registry ([Part III
/// C.4](../../../docs/specification/30-nikaia-tooling.md)).
#[test]
fn a_description_with_no_hash_to_compare_is_not_refused() {
    // The `DESCRIPTION` fixture carries no `[sources]` at all, and the crate it
    // names is not in the tree either — two absences, one answer.
    let root = project("no-hash", Some(DESCRIPTION));
    assert!(Foreign::of(&root).moved.is_empty());
    let found = findings(
        &root,
        "fn main() {\n    let n = fremd::zwei(1, 2)\n    println(f\"{n}\")\n}",
    );
    assert!(found.is_empty(), "{found:#?}");
}

/// The ledger a build assembles holds both halves, and `std` is untouched by
/// what a description says.
#[test]
fn the_library_a_build_reads_is_std_and_the_descriptions() {
    let root = project("ledger", Some(DESCRIPTION));
    let foreign = Foreign::of(&root);
    let library = foreign
        .library()
        .expect("std's ledger and the descriptions");
    assert!(library.functions.contains_key("fremd::zwei"));
    assert!(library.functions.contains_key("io::lines"));
    assert_eq!(
        foreign.described,
        BTreeSet::from(["fremd".to_string()]),
        "the set the refusal reads is unchanged by the entries"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// **At a described boundary, text crosses as the entry says**
/// ([ADR-282](../../../docs/specification/adr/adr-282.md) D27): to a
/// `ref String` for free, and to a `String` the crate may keep only as text of
/// its own - a view handed there is refused with the copy named, never copied
/// by the compiler.
#[test]
fn a_view_crosses_to_a_described_view_and_is_refused_where_the_crate_keeps() {
    const TEXT: &str = "version = 2\n\
                        inference = \"described-from-signatures\"\n\
                        \n\
                        [fn.\"fremd::zaehle\"]\n\
                        pub = true\n\
                        sync = true\n\
                        signature = \"(s: ref String) -> i64\"\n\
                        \n\
                        [fn.\"fremd::behalte\"]\n\
                        pub = true\n\
                        sync = true\n\
                        signature = \"(s: String) -> i64\"\n";
    let root = project("text-crosses", Some(TEXT));
    let read = findings_keeping(
        &root,
        "fn main() {\n    let line = \"  a b  \"\n    let n = fremd::zaehle(line.trim())\n    let m = fremd::behalte(f\"{n}\")\n    println(f\"{n} {m}\")\n}",
    );
    assert!(read.is_empty(), "{read:#?}");
    let kept = findings(
        &root,
        "fn count(text: ref String) -> i64 {\n    return fremd::behalte(text.trim())\n}\n\
         fn main() {\n    let n = count(\"x\")\n    println(f\"{n}\")\n}",
    );
    assert!(
        kept.iter().any(|f| f.code == "NK1102"
            && f.notes.iter().any(|n| n.contains("keeps its `s`"))
            && f.help.iter().any(|h| h.contains(".clone()"))),
        "{kept:#?}"
    );
}
