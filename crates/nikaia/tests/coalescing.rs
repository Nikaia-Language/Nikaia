//! There is no postfix `??`, and the three things one is reached for each have
//! a spelling ([ADR-165](../../../docs/specification/adr/adr-165.md)).
//!
//! [ADR-018](../../../docs/specification/adr/adr-018.md) D3 wrote
//! `lookup(a.query("id")??)` and Part III 17.1 copied it, against a Part I 3.5
//! that defines `??` as `a ?? b` and nothing else. The question — *does the
//! language have a postfix unwrap?* — is answered **no**, and the argument is
//! the table below: every one of the three is written here, compiled and run.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

/// Lower it, compile it, run it, hand back what it printed.
fn output(purpose: &str, source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let found =
        check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings;
    assert!(
        found.is_empty(),
        "{purpose} is a correct program: {found:#?}"
    );

    let rust = emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust;
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "{purpose} compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("the program runs");
    let printed = String::from_utf8_lossy(&ran.stdout).trim().to_string();
    let _ = std::fs::remove_dir_all(&dir);
    printed
}

/// The refusal as a reader sees it: the headline, and the help under it.
fn refused(source: &str, what: &str) -> String {
    let error = parse_to_ast(source).expect_err(what);
    let finding =
        nikaia::diagnostics::refused_finding(&error).expect("a parse error carries its finding");
    nikaia::diagnostics::render_finding(finding, "app.nika", source)
}

/// **D1: `a??` is refused, and the refusal names the three ways out.**
///
/// *Expected one value, or an expression in brackets* was true and no help at
/// all to someone who wrote the two characters on purpose
/// ([Part III C.2](../../../docs/specification/30-nikaia-tooling.md) asks for
/// the reason and a concrete way out).
#[test]
fn a_postfix_question_mark_pair_is_refused_with_the_three_spellings() {
    let refused = refused(
        "fn lookup(id: String) -> String { return id }\n\
         \n\
         fn pick(q: String?) -> String {\n\
         \x20   return lookup(q??)\n\
         }\n",
        "there is no postfix `??`",
    );

    assert!(
        refused.contains("error: `??` needs a fallback after it."),
        "{refused}"
    );
    // The three, each by name, in the help.
    assert!(refused.contains("= help: Write `a ?? b`"), "{refused}");
    assert!(
        refused.contains("`a ?? b` for a default value"),
        "{refused}"
    );
    assert!(refused.contains("`a ?? throw NotFound`"), "{refused}");
    assert!(refused.contains("panic("), "{refused}");
    // And when, rather than only what: the abort is for a value that must be there.
    assert!(refused.contains("if the value must be there"), "{refused}");
}

/// The **statement head** is refused too — `while a?? { … }` — because a
/// difference between the two would be exactly the drift
/// `a_head_parses_what_a_body_parses` exists to catch.
#[test]
fn the_statement_head_refuses_it_as_well() {
    let refused = refused(
        "fn main() {\n\
         \x20   let q: bool? = null\n\
         \x20   while q?? {\n\
         \x20       break\n\
         \x20   }\n\
         }\n",
        "there is no postfix `??` in a head either",
    );
    assert!(
        refused.contains("error: `??` needs a fallback after it."),
        "{refused}"
    );
}

/// **And `??` itself is untouched**, including the chain
/// [ADR-066](../../../docs/specification/adr/adr-066.md) D4 allows.
#[test]
fn coalescing_and_its_chain_still_parse_and_run() {
    let printed = output(
        "coalescing-chain",
        "fn main() {\n\
         \x20   let a: String? = null\n\
         \x20   let b: String? = null\n\
         \x20   let s = a ?? b ?? \"last\"\n\
         \x20   println(f\"{s}\")\n\
         }\n",
    );
    assert_eq!(printed, "last");
}

/// **The first of the three: a fallback value.**
#[test]
fn a_fallback_value_is_the_operator_itself() {
    let printed = output(
        "coalescing-fallback",
        "use std::collections\n\
         \n\
         fn main() {\n\
         \x20   let mut m = collections::HashMap()\n\
         \x20   m[\"a\"] = 1\n\
         \x20   let n = m[\"b\"] ?? 0\n\
         \x20   println(f\"{n}\")\n\
         }\n",
    );
    assert_eq!(printed, "0");
}

/// **The second: a jump**, because the four of them are expressions
/// ([ADR-138](../../../docs/specification/adr/adr-138.md) D1). This is the
/// shape [ADR-165](../../../docs/specification/adr/adr-165.md) D2 puts in
/// [ADR-018](../../../docs/specification/adr/adr-018.md) D3's line: a handler
/// reaching a parameter that may be absent **answers** when it is.
#[test]
fn a_jump_is_the_second_spelling() {
    let printed = output(
        "coalescing-jump",
        "fn pick(q: String?) -> String {\n\
         \x20   let id = q ?? return \"bad request\"\n\
         \x20   return id\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let missing: String? = null\n\
         \x20   println(pick(missing))\n\
         }\n",
    );
    assert_eq!(printed, "bad request");
}

/// **The third: saying the value is known to be there**, which ends the program
/// with the program's own words rather than with punctuation
/// ([ADR-161](../../../docs/specification/adr/adr-161.md) D3). Run for the
/// half that matters here — that it is a `String` afterwards and the program
/// goes on.
#[test]
fn panic_is_the_third_spelling_and_the_value_goes_on() {
    let printed = output(
        "coalescing-panic",
        "use std::collections\n\
         \n\
         fn main() {\n\
         \x20   let mut m = collections::HashMap()\n\
         \x20   m[\"a\"] = 7\n\
         \x20   let n = m[\"a\"] ?? panic(\"a was a key a moment ago\")\n\
         \x20   println(f\"{n}\")\n\
         }\n",
    );
    assert_eq!(printed, "7");
}
