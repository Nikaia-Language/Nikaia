//! **`dsl G { … } eod` over a grammar this program declares** (#518, Part II
//! 10.5, ADR-299 D20): the grammar's entry runs on the block's text while the
//! program is built, the value is built into the program, and the block's
//! type is the entry's result.

mod common;

use nikaia::check::Finding;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;
use std::process::Command;

const NUM: &str = "grammar Num {\n    entry rule n -> i64 = d:dec[i64](digit+) { d }\n}\n\n";

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = common::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    common::checked(&parsed, &own, &library).findings
}

#[test]
fn a_block_over_a_grammar_is_its_entry_s_value() {
    let dir = common::scratch_dir("dsl-grammar");
    std::fs::write(
        dir.join("num.nika"),
        format!(
            "{NUM}fn main() {{\n    let x = dsl Num {{ 42 }} eod\n    println(f\"{{x + 1}}\")\n}}\n"
        ),
    )
    .expect("write the source");
    let ran = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&dir)
        .args(["run", "--no-cache", "num.nika"])
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs");
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "43");
}

#[test]
fn the_block_has_the_entry_s_type() {
    let found = findings(&format!(
        "{NUM}fn main() {{\n    let b: bool = dsl Num {{ 42 }} eod\n    println(f\"{{b}}\")\n}}\n"
    ));
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK1103" && f.message.contains("`i64`")),
        "{found:#?}"
    );
}

/// Text the grammar does not accept is refused while the program is built,
/// in the grammar's words.
#[test]
fn text_the_grammar_refuses_is_refused_at_the_block() {
    let found = findings(&format!(
        "{NUM}fn main() {{\n    let x = dsl Num {{ x }} eod\n    println(f\"{{x}}\")\n}}\n"
    ));
    assert!(found.iter().any(|f| f.code == "NK1178"), "{found:#?}");
}
