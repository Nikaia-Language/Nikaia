//! What a parse fails with — `ParseError`
//! ([ADR-173](../../../docs/specification/adr/adr-173.md)).
//!
//! A `dsl` entry rule's ledger entry wrote `throws = ["?"]` — *something this
//! compiler cannot name* — because a parse fails with a **rendered string** and
//! a string is not a type. It was the last `"?"` written anywhere in this tree.
//!
//! **And naming it turned up something worse than a missing name.** A grammar's
//! entry is written straight into the ledger
//! ([ADR-082](../../../docs/specification/adr/adr-082.md) D1) rather than
//! inferred from an `Item::Fn`, so the `throws` fixpoint — which walks the
//! graph it built from functions — had no set for it and **what a parse threw
//! reached no caller at all**. That was invisible while the entry threw `"?"`:
//! a set of one `"?"` is the boxed channel, and a box takes a `String`. It
//! stopped being invisible the moment the caller's set had a *named* member
//! from somewhere else.

mod common;

use std::collections::BTreeSet;

use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{self, Build};
use nikaia::parser::parse_to_ast;

const GRAMMAR: &str = "use std::fs\n\n\
                       grammar Tiny {\n\
                       \x20   pub rule number -> i64 = n:dec[i64](digit+) { n }\n\
                       }\n";

fn ledger_of(source: &str) -> Ledger {
    let parsed = parse_to_ast(source).expect("the source parses");
    Ledger::infer(&parsed)
}

fn rust(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit::emit_program(&parsed, Build::default())
        .expect("the program lowers")
        .rust
}

/// **The entry's own set has a name.**
#[test]
fn a_grammar_entry_throws_a_named_error() {
    let ledger = ledger_of(&format!(
        "{GRAMMAR}\nfn read(data: ref String) -> i64 throws {{\n\
         \x20   return Tiny::number(data)\n\
         }}\n"
    ));
    assert_eq!(
        ledger.functions["Tiny::number"].fails_with,
        ["ParseError"],
        "the entry rule"
    );
}

/// **And it reaches its caller**, which it did not before: the fixpoint builds
/// its graph from `Item::Fn` and a grammar's entry is not one, so the lookup
/// found nothing and the caller's set came out empty — which the ledger then
/// wrote as `["?"]`, the shape that hid it.
#[test]
fn what_a_parse_throws_reaches_its_caller() {
    let ledger = ledger_of(&format!(
        "{GRAMMAR}\nfn read(data: ref String) -> i64 throws {{\n\
         \x20   return Tiny::number(data)\n\
         }}\n"
    ));
    assert_eq!(ledger.functions["read"].fails_with, ["ParseError"]);
}

/// **A parse beside an `io::IoError` is a sum of two named members**
/// ([ADR-160](../../../docs/specification/adr/adr-160.md) D1) — which is what
/// naming this buys: a program can tell *the file was not there* from *the file
/// was not the shape the grammar says*.
#[test]
fn a_parse_beside_a_read_is_a_named_sum() {
    let source = format!(
        "{GRAMMAR}\nfn both(path: ref String) -> i64 throws {{\n\
         \x20   let data = fs::read_to_string(path, fs::Root::Anywhere)\n\
         \x20   let n = Tiny::number(data)\n\
         \x20   return n\n\
         }}\n\
         \n\
         fn main() {{ println(f\"{{both(\\\"x\\\") catch {{ -1 }}}}\") }}\n"
    );
    let ledger = ledger_of(&source);
    assert_eq!(
        ledger.functions["both"].fails_with,
        ["ParseError", "io::IoError"]
    );

    let rust = rust(&source);
    assert!(
        rust.contains("enum __NikaiaThrows_ParseError__io_IoError"),
        "{rust}"
    );
    assert!(
        rust.contains("ParseError::of(error.render(_source))"),
        "{rust}"
    );
}

/// **And it compiles**, which is the assertion this exists for.
///
/// Before the fixpoint learned to read a grammar entry's set, the same program
/// was emitted as `Result<i64, io::IoError>` — the parse contributing nothing —
/// with a `?` inside it on a `Result<_, String>`. `io::IoError` has no
/// `From<String>`, so `rustc` refused the **generated file**, which is the one
/// thing [Part III C.1](../../../docs/specification/30-nikaia-tooling.md) says
/// may not happen. A ledger assertion alone would not have caught it; only
/// `rustc` says this.
#[test]
fn the_two_member_channel_compiles() {
    let rust = rust(&format!(
        "{GRAMMAR}\nfn both(path: ref String) -> i64 throws {{\n\
         \x20   let data = fs::read_to_string(path, fs::Root::Anywhere)\n\
         \x20   let n = Tiny::number(data)\n\
         \x20   return n\n\
         }}\n\
         \n\
         fn main() {{ println(f\"{{both(\\\"x\\\") catch {{ -1 }}}}\") }}\n"
    ));
    let dir = common::scratch_dir("parse-error-channel");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The corpus is unmoved except where a grammar is**, which is the guard: a
/// program with no `dsl` in it has nothing to do with this record, and a
/// refusal or a changed channel there would be this change reaching further
/// than it says.
#[test]
fn only_a_program_with_a_grammar_changes() {
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut with_a_grammar = 0;
    let mut checked = 0;
    for path in every_program(&root) {
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(parsed) = parse_to_ast(&source) else {
            continue;
        };
        let ledger = Ledger::infer(&parsed);
        let names = ledger
            .functions
            .values()
            .any(|c| c.fails_with.iter().any(|e| e == "ParseError"));
        if names {
            with_a_grammar += 1;
            assert!(
                source.contains("grammar ") || source.contains("dsl "),
                "{} names `ParseError` and has no grammar",
                path.display()
            );
        }
        // …and nothing is refused for it.
        let found = nikaia::check::check_program(&parsed, &ledger, &library, &BTreeSet::new());
        assert!(
            !found.findings.iter().any(|f| {
                f.severity == nikaia::check::Severity::Error && f.notes.join(" ").contains("Parse")
            }),
            "{}: {:#?}",
            path.display(),
            found.findings
        );
        checked += 1;
    }
    assert!(checked >= 12, "only {checked} programs were checked");
    assert!(
        with_a_grammar >= 1,
        "no program in the corpus names it, so nothing was measured"
    );
}

fn every_program(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![root.join("examples"), root.join("tests/samples")];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("nika") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}
