//! A `catch` that ignores the error binds `_error`
//! ([ADR-090](../../../docs/specification/adr/adr-090.md)).
//!
//! Kap 7.1 gives the failure the name `error` whether or not the handler reads
//! one, so `catch { 1000 }` — the shape Part I 7.1 teaches first — used to reach
//! the author as `warning: unused variable: error` about a binding that exists
//! nowhere in their program. That is
//! [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s class one
//! severity down: the message is `rustc` reading the emitted code correctly, and
//! the only name in it is one the emitter invented.
//!
//! **The polarity is what these tests are for.** A false *"the handler mentions
//! it"* keeps yesterday's binding and yesterday's warning; a false *"it does
//! not"* writes `_error` under a handler that reads `error`, and that is a
//! program which does not compile. So every shape that mentions the name is
//! checked to still bind it — a hole, a nested block, a `let` that only passes
//! it on — and the one shape that merely *looks* like it (`errors`) is checked
//! not to.

mod common;

use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let checked =
        nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new());
    assert!(
        checked.findings.is_empty(),
        "the fixture is a correct program and the checker says otherwise: {:#?}",
        checked.findings
    );
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// The handler shapes, each with the arm it must produce.
///
/// Written as one table because the question is the same one five times, and a
/// reader comparing the left column against the right is reading the rule.
const SHAPES: &[(&str, &str, &str)] = &[
    (
        "a constant fallback",
        "catch { 1000 }",
        "Err(_error) => { 1000 },",
    ),
    (
        "a hole that reads the error",
        "catch { println(f\"{error}\") 0 }",
        "Err(error) =>",
    ),
    (
        "a `let` that only passes it on",
        "catch { let kept = error\n        println(f\"{kept}\")\n        0 }",
        "Err(error) =>",
    ),
    (
        "a name that merely looks like it",
        "catch { errors }",
        "Err(_error) => { errors },",
    ),
];

fn program(handler: &str) -> String {
    format!(
        r#"
fn parsed(text: ref String) -> i32 {{
    let errors = 5
    return text.parse() {handler}
}}

fn main() {{
    let it = parsed("7")
    println(f"{{it}}")
}}
"#
    )
}

#[test]
fn each_handler_binds_what_it_reads() {
    for (purpose, handler, arm) in SHAPES {
        let rust = lowered(&program(handler));
        assert!(
            rust.contains(arm),
            "{purpose} should lower to `{arm}`:\n{rust}"
        );
    }
}

/// **A nested block counts**, which is the shape the over-approximation exists
/// for: the walk goes into the `if`'s body rather than stopping at the
/// handler's own statements.
#[test]
fn a_nested_block_is_a_mention() {
    let rust = lowered(
        r#"
fn parsed(text: ref String) -> i32 {
    return text.parse() catch {
        if text.len() > 0 {
            println(f"{error}")
        }
        7
    }
}

fn main() {
    let it = parsed("7")
    println(f"{it}")
}
"#,
    );
    assert!(
        rust.contains("Err(error) =>"),
        "a mention inside an `if` inside the handler is still a mention:\n{rust}"
    );
}

/// **The whole point, measured where the user stands**: `rustc` on the emitted
/// file, and nothing about a name nobody wrote.
///
/// The fixture is the line from `examples/n-body/src/main.nika` that found this.
#[test]
fn the_reproduction_compiles_without_a_warning() {
    let rust = lowered(
        r#"
use std::cli

fn main() {
    let steps = cli::args().nth(1) ?? "1000"
    let n: i32 = steps.parse() catch { 1000 }
    println(f"{n}")
}
"#,
    );
    assert!(
        rust.contains("Err(_error) => { 1000 },"),
        "the handler reads nothing, so the binding is `_error`:\n{rust}"
    );

    let dir = common::scratch_dir("catch-handler-warning");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "the lowering compiles:\n{said}\n--- the Rust ---\n{rust}"
    );
    assert!(
        !said.contains("unused variable"),
        "and says nothing about a name the emitter wrote:\n{said}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The other half of the polarity, compiled and run**: a handler that does
/// read the error still gets a binding it can read, under the name Kap 7.1
/// promises. A `_error` written here would not compile at all.
#[test]
fn a_handler_that_reads_the_error_still_can() {
    let rust = lowered(
        r#"
fn parsed(text: ref String) -> i32 {
    return text.parse() catch {
        println(f"caught {error}")
        0
    }
}

fn main() {
    let it = parsed("not a number")
    println(f"{it}")
}
"#,
    );
    assert!(
        rust.contains("Err(error) =>"),
        "the handler reads it, so the binding keeps its name:\n{rust}"
    );

    let dir = common::scratch_dir("catch-handler-reads");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr),
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("the program runs");
    let printed = String::from_utf8_lossy(&ran.stdout).into_owned();
    assert!(
        printed.starts_with("caught "),
        "and the handler printed what it caught: {printed:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
