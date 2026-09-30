//! Three shapes the ledger's reader met when it moved into Nikaia
//! (0.0.292, ADR-257 step (c)), each a compiler defect a program could meet
//! the same way.
//!
//! Run rather than read, as `indexing.rs` is: whether the lowering compiles is
//! the language below's question.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(purpose: &str, source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let found =
        check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings;
    assert!(
        found.is_empty(),
        "{purpose} is a correct program and the checker says otherwise: {found:#?}"
    );
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Compile and run, hand back what it printed.
fn ran(purpose: &str, source: &str) -> String {
    let rust = lowered(purpose, source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering of {purpose} does not compile:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr),
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("the program runs");
    let printed = String::from_utf8_lossy(&ran.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    printed
}

/// **`x == null` asks whether there is one**, and nothing of what `x` holds is
/// compared: the lowering is `x.is_none()`. A type that does not compare
/// (`NK1188`) was refused there all the same.
#[test]
fn null_is_asked_of_a_type_that_does_not_compare() {
    let source = r#"struct Step {
    apply: fn(i64) -> i64,
}

fn found(n: i64) -> Step? {
    if n > 0 {
        return Step { apply: fn(x) { x + n } }
    }
    return null
}

fn main() {
    let none = found(0)
    let some = found(2)
    if none == null && some != null {
        println("asked")
    }
}
"#;
    assert_eq!(ran("null-of-uncomparable", source), "asked\n");
}

/// **A `match` in statement position hands nothing back**, so an arm's block
/// ends with a statement: a map's `insert` there answers the old value, and
/// written as the arm's value it made the arms disagree with the empty one.
#[test]
fn an_arm_of_a_statement_match_ends_in_a_statement() {
    let source = r#"use std::collections

fn main() {
    let mut seen: collections::HashMap[String, i64] = collections::HashMap()
    for n in 0..<3 {
        match n {
            1 => {
                seen.insert("one", n)
            }
            else => {}
        }
    }
    println(f"{seen.len()}")
}
"#;
    assert_eq!(ran("statement-match", source), "1\n");
}

/// **A text's characters are listed at their count**: `nikaia_std::list::chars`
/// asks for the room once, where `collect` over `Chars` grew the list step by
/// step - the largest single cost of reading a ledger in Nikaia.
#[test]
fn the_characters_of_a_text_are_listed_at_their_size() {
    let source = r#"fn main() {
    let word = "héllo"
    let c: Vec[char] = word.chars().collect()
    println(f"{c.len()} {c[1]}")
}
"#;
    let rust = lowered("chars", source);
    assert!(rust.contains("nikaia_std::list::chars("), "{rust}");
    assert_eq!(ran("chars", source), "5 é\n");
}

/// **A part an arm binds `ref` is handed on as it is** (`open-work.md` 2.52):
/// ADR-242 binds `key` as a view where the arm only reads it, so a call that
/// lends it writes `key` and not `&key`, a view of a view.
#[test]
fn a_part_bound_as_a_view_is_lent_as_it_is() {
    let source = r#"enum Line {
    Pair { key: String, value: String },
    Skip,
}

fn shout(text: ref String) -> String {
    return text.to_uppercase()
}

fn main() {
    let mut lines = [Line::Pair { key: "a", value: "b" }, Line::Skip]
    for one in lines.drain() {
        match one {
            Line::Pair { key, value } => {
                let said = shout(key)
                println(f"{said}={shout(value)}")
            }
            Line::Skip => {}
        }
    }
}
"#;
    let rust = lowered("ref-part", source);
    assert!(rust.contains("ref key"), "{rust}");
    assert!(!rust.contains("shout(&key)"), "{rust}");
    assert!(!rust.contains("shout(&value)"), "{rust}");
    assert_eq!(ran("ref-part", source), "A=B\n");
}
