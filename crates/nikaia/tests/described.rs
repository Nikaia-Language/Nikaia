//! A crate is described before it is called
//! ([ADR-104](../../../docs/specification/adr/adr-104.md) D1).
//!
//! A call into a Rust crate no ledger describes was **silent**. Every analysis
//! this compiler has reads a contract at the boundary — what may cross a
//! thread, what the call may reach, whether it pauses, whether it can fail —
//! and where there is none they all read the same thing: nothing. D1 removes
//! the third answer. There is a described crate and a refused call, and the
//! message names the command that turns the second into the first.

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;

use nikaia::parser::parse_to_ast;

fn named(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| n.to_string()).collect()
}

fn refusals(source: &str, declared: &[&str], described: &[&str]) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    // **Nothing has moved**, which is what these tests are not about: the hash
    // rule is `described_entries.rs`'s, and a set handed in empty here keeps
    // each file measuring one thing ([ADR-104](../../../docs/specification/adr/adr-104.md)
    // D1 and D5 are two rules).
    nikaia::foreign::check(
        &parsed,
        &named(declared),
        &named(described),
        &std::collections::BTreeMap::new(),
    )
}

const CALLS: &str = "fn main() {\n\
                     \x20   let served = hyper_shim::serve_once(18080)\n\
                     \x20   println(f\"{served}\")\n\
                     }\n";

/// **The refusal, and the message is the command.**
#[test]
fn a_call_into_an_undescribed_crate_is_refused() {
    let found = refusals(CALLS, &["hyper_shim"], &[]);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK2504");
    assert!(found[0].message.contains("hyper_shim"), "{:?}", found[0]);
    assert_eq!(
        found[0].help.as_deref(),
        Some("Run `nikaia describe hyper_shim`."),
        "the way out is one command, and it is in the message (Part III C.2)"
    );
}

/// **And it is silent once the crate is described**, which is the whole shape:
/// the refusal is a difference between two sets rather than a claim about a
/// name.
#[test]
fn a_described_crate_says_nothing() {
    assert!(refusals(CALLS, &["hyper_shim"], &["hyper_shim"]).is_empty());
}

/// **Once per crate and not once per call.** The reader's next move is one
/// command for the whole crate, so four calls are one thing to do and one
/// message to read; the caret is on the first, which is where they will start.
#[test]
fn a_crate_is_named_once_however_often_it_is_called() {
    let source = "fn main() {\n\
                  \x20   let a = hyper_shim::serve_once(1)\n\
                  \x20   let b = hyper_shim::serve_once(2)\n\
                  \x20   let c = regex::compile(\"x\")\n\
                  }\n";
    let found = refusals(source, &["hyper_shim", "regex"], &[]);
    assert_eq!(found.len(), 2, "one per crate: {found:#?}");
    let named: Vec<&str> = found.iter().map(|f| f.message.as_str()).collect();
    assert!(named.iter().any(|m| m.contains("hyper_shim")), "{named:?}");
    assert!(named.iter().any(|m| m.contains("regex")), "{named:?}");
}

/// **Only where the build itself declared the crate** (Part III C.4). A
/// qualified name this compiler cannot account for — another Nikaia package, a
/// `std` path, a typo — is not this refusal's business and keeps whatever
/// message it already had. Refusing on a name nobody declared would be a
/// correct program refused, which is the worse of the two mistakes.
#[test]
fn a_name_the_build_did_not_declare_is_left_alone() {
    let source = "use std::io\n\nfn main() {\n\
                  \x20   let a = http::ok(\"hi\")\n\
                  \x20   let b = io::read_to_string()\n\
                  \x20   let c = regx::compile(\"x\")\n\
                  }\n";
    assert!(
        refusals(source, &["hyper_shim"], &[]).is_empty(),
        "nothing here is the declared crate"
    );
    // …and with nothing declared at all — a loose file, no project around it —
    // there is nothing to be undescribed.
    assert!(refusals(source, &[], &[]).is_empty());
}

/// **A written type reaches across too**, and it is the one the crossing rules
/// are about: `hyper_shim::Handle` in a parameter is as much a boundary as a
/// call is.
#[test]
fn a_type_from_the_crate_is_a_reach_across() {
    let source = "fn hold(h: hyper_shim::LocalHandle) { }\nfn main() { }\n";
    let found = refusals(source, &["hyper_shim"], &[]);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK2504");
}

/// **`hyper-shim` in the manifest is `hyper_shim` in a program**, because that
/// is the crate name Cargo makes of the key. The translation is the manifest's,
/// so every reader sees the name a call writes.
#[test]
fn the_manifest_key_and_the_written_name_are_the_same_crate() {
    let manifest = nikaia::manifest::Manifest::parse(
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\n\
         hyper-shim = { type = \"rust\", path = \"../shim\" }\n\
         other = { path = \"../other\" }\n",
    )
    .expect("the manifest parses");
    assert_eq!(manifest.foreign_crates(), named(&["hyper_shim"]));
}

/// **The description `examples/foreign-runtime/` ships is a real one**, written
/// by hand as D5 expects, and it parses as the ledger every analysis reads.
#[test]
fn the_experiments_description_parses_as_a_ledger() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for project in ["crossing", "serve", "smuggled"] {
        let path = root
            .join("examples/foreign-runtime")
            .join(project)
            .join("contracts/hyper_shim.contracts");
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        nikaia::contracts::Ledger::parse(&text)
            .unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
        // Read as a build reads it: the description and the record of what it
        // was written from beside it (ADR-251 D1).
        let ledger = nikaia::contracts::Ledger::read_beside(&path)
            .unwrap_or_else(|| panic!("{} reads", path.display()));
        assert!(
            ledger.functions.contains_key("hyper_shim::serve_once"),
            "{}",
            path.display()
        );
        // **The source hash is recorded** (D5, on ADR-100 D3's rule), in the
        // record beside the description since ADR-251: the file says which
        // crate source it was read from, and a later build compares it.
        assert!(
            ledger.sources.contains_key("src/lib.rs"),
            "the description names the source it came from: {}",
            path.display()
        );
        // **And `touches` and `locks` are absent, which is fail-closed** (D3):
        // absent `touches` reads as *touches everything*, absent `locks` is the
        // column's third answer. A signature cannot say either.
        for (name, contract) in &ledger.functions {
            assert!(
                contract.touches.is_empty(),
                "`{name}` claims a touch set a Rust signature cannot say"
            );
        }
    }
    let _ = common::rustc();
}
