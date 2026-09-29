//! **`from` is an ordinary name** —
//! [ADR-116](../../../docs/specification/adr/adr-116.md) D1 and D3.
//!
//! It was reserved for `dsl X from e`, a form [ADR-082](../../../docs/specification/adr/adr-082.md)
//! removed. What a reservation buys is one sentence — the one a reader who
//! writes the word gets — and the sentence for a removed form is a `fail` in
//! the grammar, which needs the **token** and not the reservation. The two are
//! different things, and this file is about telling them apart.
//!
//! It is also the most common name on the list: Part III 17.1 writes
//! `pub fn rename(from: Path, to: Path, root: Root)`, and that declaration did
//! not parse.

mod common;

use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;
use std::process::Command;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

/// **It left the list** (D1), which is what the rest of this file rests on.
#[test]
fn from_left_the_reserved_list() {
    assert!(
        !nikaia::parser::RESERVED_WORDS.contains(&"from"),
        "`from` is a name now"
    );
}

/// **A name in every position that declares one**, and the program **runs** —
/// because a name that parses and then lowers to something `rustc` refuses
/// would pass a parse test and fail a reader.
#[test]
fn from_is_a_name_everywhere_and_the_program_runs() {
    let source = "struct Move { from: i64, to: i64 }\n\
         \n\
         fn step(from: i64, to: i64) -> i64 {\n\
         \x20   return to - from\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let from = 10\n\
         \x20   let m = Move { from: from, to: 42 }\n\
         \x20   println(f\"{step(m.from, m.to)}\")\n\
         \x20   println(f\"{from}\")\n\
         }";
    let found = findings(source);
    assert!(found.is_empty(), "{found:#?}");

    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust;
    // Rust does not reserve `from` either, so nothing is escaped — which is
    // the half that makes this a one-table change rather than two.
    assert!(!rust.contains("r#from"), "{rust}");

    let dir = common::scratch_dir("from-is-a-name");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run it");
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "32\n10\n");
    std::fs::remove_dir_all(&dir).ok();
}

/// **Part III 17.1's declaration parses**, which is D3 and the reason the word
/// was worth taking off the list at all.
#[test]
fn the_paths_two_names_parse_as_the_page_writes_them() {
    assert!(
        parse_to_ast(
            "fn rename(from: ref String, to: ref String) -> i64 {\n\
         \x20   return from.len() + to.len()\n\
         }\n\
         fn main() { println(f\"{rename(\\\"a\\\", \\\"b\\\")}\") }"
        )
        .is_ok()
    );
}

/// **The removed form keeps its sentence** (D1's *loses nothing*).
///
/// This is the whole of what the reservation was paying for, and it is paid
/// for by the grammar's token instead — a `fail` at the word, which beats the
/// alternatives at that position exactly as it did before.
#[test]
fn the_removed_dsl_form_still_says_what_happened() {
    let source = "fn main() { let x = dsl Json from \"a.json\" }";
    let refused = parse_to_ast(source).expect_err("the removed form does not parse");
    let finding =
        nikaia::diagnostics::refused_finding(&refused).expect("a parse error carries its finding");
    let said = nikaia::diagnostics::render_finding(finding, "app.nika", source);
    assert!(
        said.contains("`dsl X from e` is no longer supported.")
            && said.contains("help: Write `X::rule(e)`"),
        "{said}"
    );
}

/// **The page and the table say the same thing.**
///
/// Part I 2.1 prints the list a reader learns and `parser::RESERVED_WORDS` is
/// the one the compiler enforces, and they had drifted: `select` arrived with
/// its construct ([ADR-148](../../../docs/specification/adr/adr-148.md)) and
/// reached the table and not the page. A reader counting the words on that
/// page would have got a different answer from the compiler.
#[test]
fn the_page_and_the_table_list_the_same_words() {
    let page = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/specification/10-nikaia-light.md"),
    )
    .expect("Part I");
    let block = page
        .split_once("reserved words are:")
        .and_then(|(_, rest)| rest.split_once("```text"))
        .and_then(|(_, rest)| rest.split_once("```"))
        .map(|(block, _)| block)
        .expect("the list is a fenced block after that sentence");

    let mut printed: Vec<&str> = block.split_whitespace().collect();
    printed.sort_unstable();
    let mut enforced: Vec<&str> = nikaia::parser::RESERVED_WORDS.to_vec();
    enforced.sort_unstable();
    assert_eq!(
        printed, enforced,
        "Part I 2.1 and `parser::RESERVED_WORDS` have drifted apart"
    );
}
