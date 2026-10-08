//! A type nothing declares is refused rather than lowered
//! ([ADR-096](../../../docs/specification/adr/adr-096.md)).
//!
//! A **value** nothing declares has had `NK1117` since
//! [ADR-298](../../../docs/specification/adr/adr-298.md) — *"nothing declares
//! `q`"*. A type had nothing, so `let x: Widgit = 3` lowered verbatim and came
//! back as `rustc`'s *"cannot find type `Widgit` in this scope"*, about a file
//! nobody wrote. [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s
//! rule held for one half of this language's names and not the other.
//!
//! **Every test here is about the polarity.** Getting the set of known names
//! wrong refuses a *correct* program, which is the one thing the checker may
//! never do ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)) —
//! so each kind of name that must stay silent has a test saying so.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings
}

fn refused(source: &str) -> Vec<String> {
    findings(source)
        .into_iter()
        .filter(|f| f.code == "NK1135")
        .map(|f| f.message)
        .collect()
}

/// The entry's own reproduction.
#[test]
fn a_misspelled_type_is_refused() {
    let found = refused("fn main() {\n    let x: Widgit = 3\n    println(f\"{x}\")\n}\n");
    assert_eq!(found.len(), 1, "one refusal: {found:#?}");
    assert!(
        found[0].contains("`Widgit`"),
        "naming what is undeclared: {found:#?}"
    );
}

/// **Every position a type can be written in**, because a check that reads one
/// of them is a check somebody routes around without meaning to.
#[test]
fn every_position_a_type_stands_in_is_read() {
    for source in [
        "fn f(a: Widgit) { }\nfn main() { }\n",
        "fn f() -> Widgit { }\nfn main() { }\n",
        "struct Holder {\n    part: Widgit,\n}\nfn main() { }\n",
        "fn main() {\n    let x: Widgit = 3\n    println(f\"{x}\")\n}\n",
        "fn main() {\n    let x: Vec[Widgit] = 3\n    println(f\"{x}\")\n}\n",
        "struct Thing { n: i64 }\nimpl Thing {\n    fn m(ref self) -> Widgit { }\n}\nfn main() { }\n",
        "enum Message {\n    Write(Widgit),\n}\nfn main() { }\n",
        "enum Message {\n    Move { to: Widgit },\n}\nfn main() { }\n",
        "trait Show {\n    fn show(ref self) -> Widgit\n}\nfn main() { }\n",
        "trait Show {\n    fn show(ref self, at: Widgit) -> i64\n}\nfn main() { }\n",
    ] {
        assert!(
            !refused(source).is_empty(),
            "a written type is read here too:\n{source}"
        );
    }
}

/// **A nested argument is a written type too**, which is what makes the walk a
/// walk rather than a look at the head.
#[test]
fn an_argument_inside_a_type_is_read() {
    let found = refused("fn main() {\n    let x: Vec[Widgit] = 3\n    println(f\"{x}\")\n}\n");
    assert_eq!(found.len(), 1, "the argument, not the `Vec`: {found:#?}");
    assert!(found[0].contains("`Widgit`"));
}

/// **The half that decides whether this costs anything.** Each of these is a
/// name something accounts for, and each accounts for it differently — Part I
/// 2.2's own set, a collection the prelude provides, a hull, a type this
/// program declares, and a parameter the declaration around it names.
#[test]
fn every_kind_of_declared_name_is_left_alone() {
    for source in [
        "fn f(a: i64, b: f64, c: bool, d: scalar, e: String, g: ref String) { }\nfn main() { }\n",
        "fn f(a: u8, b: usize, c: i128) { }\nfn main() { }\n",
        "use std::collections\n\nfn f(a: Vec[i64], b: collections::HashMap[ref String, i64], c: collections::BTreeMap[ref String, i64]) { }\nfn main() { }\n",
        "fn f(a: Shared[i64], b: SharedMut[i64]) { }\nfn main() { }\n",
        "struct Row { n: i64 }\nfn f(a: Row) { }\nfn main() { }\n",
        "enum Colour { Red, Green }\nfn f(a: Colour) { }\nfn main() { }\n",
        "fn f[T](a: T) -> T { return a }\nfn main() { }\n",
        "struct Box[T] { held: T }\nfn main() { }\n",
        "struct Row { n: i64 }\nimpl Row {\n    fn me(ref self) -> Self { }\n}\nfn main() { }\n",
        "fn f(a: ref String?) { }\nfn main() { }\n",
        "fn f(a: (i64, String)) { }\nfn main() { }\n",
    ] {
        assert!(
            refused(source).is_empty(),
            "this is a correct program and NK1135 says otherwise:\n{source}\n{:#?}",
            findings(source)
        );
    }
}

/// **A qualified name is left alone**, and that is deliberate rather than
/// unfinished: `http::Response` names a package's type, and whether this build
/// can see that package is
/// [ADR-286](../../../docs/specification/adr/adr-286.md) D11's question with its
/// own message. Saying *"nothing declares it"* about a name a dependency does
/// declare would be [Part III C.4](../../../docs/specification/30-nikaia-tooling.md).
#[test]
fn a_name_with_a_package_in_front_is_somebody_elses_question() {
    assert!(
        refused("fn f(a: ref http::Response) { }\nfn main() { }\n").is_empty(),
        "a qualified name is the import rules' business, not this walk's"
    );
    assert!(
        refused("use std::fs\n\nfn f(a: ref fs::Mapped) { }\nfn main() { }\n").is_empty(),
        "and a `std` type resolves, by its own name and by its suffix"
    );
}

/// **The corpus, which is what found the two real defects.**
///
/// `examples/fortunes.nika` wrote `List[Fortune]` — a name the language does
/// not have, in a file that never built for another reason and so never said
/// so — and a `typecheck.rs` fixture had been standing on the absence this
/// closes. Both are fixed; this is what keeps them fixed.
#[test]
fn nothing_in_the_corpus_is_newly_refused() {
    let mut found = Vec::new();
    for directory in ["examples", "tests/samples", "crates/nikaia-std/src"] {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(directory);
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "nika") {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            if parse_to_ast(&source).is_err() {
                continue;
            }
            for message in refused(&source) {
                found.push(format!("{}: {message}", path.display()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "these programs are right and this refusal says otherwise: {found:#?}"
    );
}

/// **There is no `char`** (ADR-320, Part I 2.6): one character is a `scalar`
/// and one byte a `u8`, and the refusal names both.
#[test]
fn char_is_refused_with_both_words() {
    let found: Vec<check::Finding> =
        findings("fn f(c: char) -> bool {\n    return c == 'a'\n}\nfn main() { }\n")
            .into_iter()
            .filter(|f| f.code == "NK1135")
            .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    let help = found[0].help.clone().unwrap_or_default();
    assert!(help.contains("scalar") && help.contains("u8"), "{help}");
}
