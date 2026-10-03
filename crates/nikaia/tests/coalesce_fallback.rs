//! A `??`'s fallback is one value, or an expression in brackets
//! ([ADR-279](../../../docs/specification/adr/adr-279.md)).
//!
//! **`??` sat above the whole binary chain**, so its fallback reached rightwards
//! across every operator there is: `a ?? 0 > 3` was `a ?? (0 > 3)` while looking
//! like `(a ?? 0) > 3`. issue #214 carried that as a **suspicion**, because
//! the shape it had produced a type error rather than a wrong answer, and
//! whether both readings could ever type-check was *precisely what had not been
//! established*.
//!
//! It can. With `a: bool?`, `x: bool`, `y: bool`, the line `a ?? x == y` reads
//! both ways and the answers differ — measured at `false` where the line looks
//! like `true`. That is a silent wrong value, which is the worst thing in
//! `docs/README.md`'s list, so it stopped being a suspicion.

mod common;

use nikaia::parser::parse_to_ast;

/// Everything a parse refusal says: its headline, and the notes and help the
/// finding it carries adds under it.
fn everything_said(refused: &anyhow::Error) -> String {
    let mut said = refused.to_string();
    if let Some(finding) = nikaia::diagnostics::refused_finding(refused) {
        for note in &finding.notes {
            said.push_str("\nnote: ");
            said.push_str(note);
        }
        if let Some(help) = &finding.help {
            said.push_str("\nhelp: ");
            said.push_str(help);
        }
    }
    said
}

/// The program the suspicion needed and did not have.
///
/// Kept as the record of *why* this is refused rather than re-precedenced: the
/// two readings are both well typed, so nothing downstream would have caught it.
const AMBIGUOUS: &str = r#"
fn main() {
    let a: bool? = false
    let x: bool = true
    let y: bool = false
    let ohne = a ?? x == y
    println(f"{ohne}")
}
"#;

#[test]
fn the_shape_that_read_both_ways_is_refused() {
    let refused = parse_to_ast(AMBIGUOUS).expect_err("a bare binary fallback is refused");
    let said = everything_said(&refused);
    assert!(
        said.contains("The fallback after `??` is a single value unless you use brackets"),
        "and the refusal says why, in this language's words:\n{said}"
    );
    assert!(
        said.contains("`(a ?? 0) > 3`") && said.contains("`a ?? (0 > 3)`"),
        "naming both readings, so the author picks rather than guesses:\n{said}"
    );
}

/// **Both bracketed forms parse**, which is what makes the refusal a fork rather
/// than a dead end.
#[test]
fn either_bracketing_is_accepted() {
    for written in ["(a ?? x) == y", "a ?? (x == y)"] {
        let source = format!(
            r#"
fn main() {{
    let a: bool? = false
    let x: bool = true
    let y: bool = false
    let it = {written}
    println(f"{{it}}")
}}
"#
        );
        assert!(
            parse_to_ast(&source).is_ok(),
            "`{written}` is how the author says which one they meant"
        );
    }
}

/// **The ordinary fallback is untouched**, and that is the half that decides
/// whether this refusal costs anything: every `??` in `examples/` has a fallback
/// of exactly this shape — a literal, a name, a call, a field.
#[test]
fn a_simple_fallback_still_parses() {
    let shapes = [
        "\"measurements.txt\"",
        "0",
        "-1",
        "other",
        "make()",
        "row.count",
        "(x + 1)",
    ];
    for fallback in shapes {
        let source = format!(
            r#"
fn other() -> i64 {{
    return 1
}}

fn make() -> i64 {{
    return 1
}}

fn main() {{
    let a: i64? = null
    let x: i64 = 1
    let row = 0
    let it = a ?? {fallback}
    println(f"{{it}}")
}}
"#
        );
        let _ = source;
        // Parsed on its own below - the point here is the fallback's shape, and
        // a fixture that also has to type-check would be measuring two things.
        let minimal = format!("fn main() {{\n    let it = a ?? {fallback}\n}}\n");
        assert!(
            parse_to_ast(&minimal).is_ok(),
            "`a ?? {fallback}` is one value and stays legal"
        );
    }
}

/// D1 keeps the chain, which is [ADR-278](../../../docs/specification/adr/adr-278.md)
/// D4's decision and must survive a change to the rule under it.
#[test]
fn a_chain_of_fallbacks_still_parses() {
    assert!(
        parse_to_ast("fn main() {\n    let it = a ?? b ?? c\n}\n").is_ok(),
        "`a ?? b ?? c` is still a chain"
    );
}

/// D2: the note is added from what a reader **sees**, so it stays away from a
/// parse error that is not this shape.
#[test]
fn an_ordinary_parse_error_gets_no_note() {
    let said = everything_said(
        &parse_to_ast("fn main() {\n    let x = = 1\n}\n")
            .expect_err("a doubled `=` is a parse error"),
    );
    assert!(
        !said.contains("The fallback after `??`"),
        "no `??` on the line, so no note about one:\n{said}"
    );
}
