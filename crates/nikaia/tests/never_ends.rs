//! A loop that cannot end needs no unreachable `return`
//! ([ADR-276](../../../docs/specification/adr/adr-276.md)).
//!
//! [ADR-276](../../../docs/specification/adr/adr-276.md) D6: a function that
//! genuinely never returns — an accept loop, an event loop, a supervisor — had
//! to end with a `return 0` that cannot be reached, and a reader of that line
//! could not tell dead code from a mistake.
//!
//! **Two tests, not one**, and the second is the one
//! [ADR-276](../../../docs/specification/adr/adr-276.md) added: before `break`
//! existed, the condition was the whole question. Now a `while true` a jump
//! leaves is a loop that ends, and the shape is a walk of the body rather than
//! a look at the head.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings
}

fn hands_back(source: &str) -> Vec<String> {
    findings(source)
        .into_iter()
        .filter(|f| f.code == "NK1104")
        .map(|f| f.message)
        .collect()
}

/// The entry's own reproduction.
#[test]
fn a_body_that_is_an_unconditional_loop_needs_no_return() {
    let source = r#"
fn forever() -> i32 {
    while true {
        let x = 1
    }
}

fn main() {
    let n = forever()
    println(f"{n}")
}
"#;
    assert!(
        hands_back(source).is_empty(),
        "nothing follows the loop because nothing can: {:#?}",
        findings(source)
    );
}

/// **And the two shapes that still have to declare what they hand back**, which
/// is where the whole value of this is: the claim is made where it is certain
/// and nowhere else.
#[test]
fn a_loop_that_can_end_still_answers_for_its_type() {
    let with_a_break = r#"
fn f(n: i32) -> i32 {
    while true {
        if n > 3 {
            break
        }
    }
}

fn main() {
    println("x")
}
"#;
    assert_eq!(
        hands_back(with_a_break).len(),
        1,
        "a `break` bound to this loop is a way out, so the function reaches its end"
    );

    let not_the_literal = r#"
fn f(flag: bool) -> i32 {
    while flag {
        let x = 1
    }
}

fn main() {
    println("x")
}
"#;
    assert_eq!(
        hands_back(not_the_literal).len(),
        1,
        "a name that happens to be true is not the literal `true`"
    );
}

/// **A `break` bound to a loop written inside is not a way out of this one**,
/// which is the half that decides whether the walk is a walk or a `contains`.
#[test]
fn a_jump_that_leaves_an_inner_loop_leaves_this_one_running() {
    let source = r#"
fn f() -> i32 {
    while true {
        for i in 0..3 {
            break
        }
    }
}

fn main() {
    println("x")
}
"#;
    assert!(
        hands_back(source).is_empty(),
        "the `break` is the `for`'s: {:#?}",
        findings(source)
    );
}

/// **And the language below agrees**, which is what makes the claim safe rather
/// than merely quiet.
///
/// [ADR-276](../../../docs/specification/adr/adr-276.md) emits `loop` for this
/// shape, and `loop { }` is `!` in Rust — so a body that never ends fits any
/// declared type. Before that record the checker could not have claimed this at
/// all: `while true { }` is `()` below, and the refusal would only have moved
/// from here to `rustc`, about a file nobody wrote.
#[test]
fn what_is_emitted_compiles_against_the_declared_type() {
    let source = r#"
fn forever() -> i32 {
    while true {
        let x = 1
    }
}

fn main() {
    println("reached")
}
"#;
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust;
    assert!(
        rust.contains("loop {"),
        "the unconditional form is Rust's `loop`:\n{rust}"
    );

    let dir = common::scratch_dir("never-ends");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "`loop` is `!`, so it fits `-> i32`:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr),
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("the program runs");
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "reached",
        "and the program that never calls it still runs"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
