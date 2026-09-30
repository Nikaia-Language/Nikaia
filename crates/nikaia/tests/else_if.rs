//! `else if` ([ADR-132](../../../docs/specification/adr/adr-132.md)).
//!
//! `if a { … } else if b { … } else { … }` was a parse error — *expected `{`,
//! found `if`* — and every language a reader comes from has it. The examples
//! worked around it with sequential `if`s, which reads as three decisions where
//! there is one.
//!
//! **Nothing is added to the language.** An `else if` is an `else` whose block
//! holds exactly one `if`, with that block's braces left out, so the chain *is*
//! an `if` inside an `if` and every rule of `if` holds at every link. `else if`
//! is two words with whitespace between them, not a keyword.

mod common;

use nikaia::contracts::LedgerOps;
use std::path::PathBuf;
use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> Result<String, String> {
    let parsed = parse_to_ast(source).map_err(|e| format!("{e:#}"))?;
    emit_program(&parsed, Build::default())
        .map(|it| it.rust)
        .map_err(|e| format!("{e:#}"))
}

/// **A three-link chain's value, run** (D1, and §5's third step).
///
/// The value and not the shape: a chain whose branches carry the `let`'s value
/// is where *the branches agree on one type and the chain ends with an `else`*
/// has to hold, and the only way to say it held is to run the program.
#[test]
fn a_three_link_chain_answers_at_every_link() {
    let source = "fn grade(score: i64) -> String {\n\
                  \x20   let g: String = if score >= 90 {\n\
                  \x20       \"A\"\n\
                  \x20   } else if score >= 80 {\n\
                  \x20       \"B\"\n\
                  \x20   } else if score >= 70 {\n\
                  \x20       \"C\"\n\
                  \x20   } else {\n\
                  \x20       \"F\"\n\
                  \x20   }\n\
                  \x20   return g\n\
                  }\n\
                  fn main() {\n\
                  \x20   println(f\"{grade(95)}{grade(85)}{grade(75)}{grade(10)}\")\n\
                  }";
    let rust = lowered(source).expect("it parses and lowers");

    // **D2: Rust's own `else if`**, so the generated file reads as the source
    // does. What this takes away is the chain's *depth*: three links nested
    // three deep is a line the reader has to unwind.
    assert!(
        rust.contains("} else if score >= 80 {") || rust.contains("else if score >= 80"),
        "the chain stays flat: {rust}"
    );
    assert!(!rust.contains("else { if "), "and is not nested: {rust}");

    let dir = common::scratch_dir("else-if");
    let file = dir.join("chain.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("chain");
    let compiled = common::compile(
        &file,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = Command::new(&binary).output().expect("the program runs");
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim_end(),
        "ABCF",
        "every link answers for its own range"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **An `else` whose block holds an `if` *and* other statements stays a block**
/// (D2).
///
/// The half that says the flat form is a reading of what the author wrote rather
/// than a rewrite of it: two statements cannot be an `else if`, and folding them
/// into one would change what runs.
#[test]
fn an_else_block_with_more_than_an_if_stays_a_block() {
    let rust = lowered(
        "fn f(n: i64) -> i64 {\n\
             if n > 0 {\n\
                 return 1\n\
             } else {\n\
                 let m = n - 1\n\
                 if m < 0 { return 2 }\n\
             }\n\
             return 3\n\
         }\n\
         fn main() { println(f\"{f(1)}\") }",
    )
    .expect("it lowers");
    assert!(rust.contains("else {"), "{rust}");
    assert!(!rust.contains("else if"), "{rust}");
}

/// **A `return` inside a link leaves the function** (D1), which is the rule of
/// `if` the chain inherits rather than a rule of its own.
///
/// `examples/http`'s `status_line` is this shape, and it is the program §5's
/// third step names: four statements that read as three decisions, now one.
#[test]
fn the_http_examples_status_line_is_one_decision() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/http/src/main.nika");
    let source = std::fs::read_to_string(&path).expect("examples/http/src/main.nika");
    assert!(
        source.contains("} else if status == 404 {"),
        "the example is written as a chain"
    );
    let rust = lowered(&source).expect("the example lowers");
    let line = rust
        .lines()
        .find(|l| l.contains("fn status_line"))
        .expect("the function is emitted");
    // **Five links now**, which is the server's arrival and not a rewrite: the
    // refusals it answers are a 408, a 413 and a 431, and each is a status this
    // function had no line for.
    assert_eq!(
        line.matches("else if").count(),
        5,
        "five links between the first `if` and the final `else`: {line}"
    );
    assert!(!line.contains("else { if "), "{line}");
}

/// **`else if` is two words and `elseif` is not one**, which a new alternative in
/// the `else` rule could have blurred.
///
/// The grammar is scannerless, so a word it has no rule for is read as a **name**
/// and a name on its own is a legal statement — which is why this is `NK1117` and
/// not a parse error, and why it is worth pinning: an `elseif` that quietly
/// became the keyword would be a second spelling nobody decided on.
#[test]
fn else_and_if_are_two_words() {
    let source = "fn f(n: i64) -> i64 { if n > 0 { return 1 } elseif n < 0 { return 2 } return 3 }";
    let parsed = parse_to_ast(source).expect("it parses - `elseif` is a name");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library = nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std's ledger");
    let found = nikaia::check::check(&parsed, &own, &library).findings;
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK1117" && f.message.contains("elseif")),
        "nothing declares `elseif`: {found:#?}"
    );
}
