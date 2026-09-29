//! `/* … */` ([ADR-134](../../../docs/specification/adr/adr-134.md)).
//!
//! A comment was `//` to the end of the line and nothing else, which is enough
//! beside a statement and not enough for commenting out a block while debugging,
//! for a paragraph above a function, or for a note inside an argument list. D1
//! makes `/*` whitespace wherever whitespace may stand, D2 makes it nest, and D3
//! says two stars is still a comment and not a doc comment the language does not
//! have.

use nikaia::parser::parse_to_ast;

/// What the source lowers to, or the refusal.
fn lowered(source: &str) -> Result<String, String> {
    let parsed = parse_to_ast(source).map_err(|e| format!("{e:#}"))?;
    nikaia::emit::emit_program(&parsed, nikaia::emit::Build::default())
        .map(|it| it.rust)
        .map_err(|e| format!("{e:#}"))
}

/// **A block comment stands where whitespace stands, including inside an
/// argument list** (D1), **and it nests** (D2).
///
/// One program for both, because they are one rule: the nested comment is inside
/// the argument list, so a scan that ended at the first `*/` would leave `c */`
/// where an argument belongs and the parse would say so.
#[test]
fn a_nested_block_comment_stands_inside_an_argument_list() {
    let rust = lowered(
        "/* a paragraph above a function,\n\
         \x20  across lines. */\n\
         fn add(a: i64, /* a /* nested */ note */ b: i64) -> i64 {\n\
             return a + b\n\
         }\n\
         fn main() { println(f\"{add(1, 2)}\") }",
    )
    .expect("it parses and lowers");
    assert!(
        rust.contains("fn add(a: i64, b: i64) -> i64"),
        "the comment is whitespace and leaves no trace: {rust}"
    );
}

/// **A `/*` inside a string literal is text** (D1).
///
/// `STRING` is a lexical rule, so no implicit whitespace runs between its
/// characters and the comment rule is never asked - but that is an argument about
/// the grammar, and this is the program that says it.
#[test]
fn a_block_comment_opener_inside_a_string_is_text() {
    let rust = lowered("fn main() { let text = \"a /* b */ c\" println(text) }")
        .expect("it parses and lowers");
    assert!(
        rust.contains("\"a /* b */ c\""),
        "the literal is what the author wrote: {rust}"
    );
}

/// **`//` inside a block comment is comment and not a second comment** (D1).
///
/// The case a scan that treated `//` as a line comment everywhere would get
/// wrong: the `*/` is on a line whose start is `//`, so reading that line as a
/// line comment would swallow the close and the comment would never end.
#[test]
fn a_line_comment_inside_a_block_comment_is_comment() {
    let rust = lowered(
        "fn main() {\n\
             /* a note\n\
             \x20  // and the close is behind this\n\
             \x20  */\n\
             println(\"ok\")\n\
         }",
    )
    .expect("it parses and lowers");
    assert!(rust.contains("println!(\"ok\")"), "{rust}");
}

/// **An unclosed block comment is reported at the `/*` that opened it** (D2).
///
/// Not at the end of the file, where the parser gave up: a comment that
/// swallowed the rest of a program fails at its last byte, and a caret there
/// points at nothing. This is the one place a parse error's position is the
/// reader's to be told rather than the parser's.
#[test]
fn an_unclosed_block_comment_is_reported_at_its_opening() {
    let source = "fn main() {\n    /* never closed\n    println(\"x\")\n}\n";
    let said = lowered(source).expect_err("an unclosed comment is a refusal");
    assert!(
        said.contains("This block comment is never closed."),
        "{said}"
    );
    assert!(
        said.contains("line 2, column 5"),
        "the opening and not the end of the file: {said}"
    );
    let refused = parse_to_ast(source).expect_err("still a refusal");
    let finding =
        nikaia::diagnostics::refused_finding(&refused).expect("a parse error carries its finding");
    let rendered = nikaia::diagnostics::render_finding(finding, "app.nika", source);
    assert!(
        rendered.contains(" 2 |     /* never closed"),
        "the line is shown: {rendered}"
    );

    // And the *outer* opening where a nested one is what is missing a close:
    // the reader has one comment too few, and the one to fix is the one that is
    // still open at the end.
    let said = lowered("fn main() {\n    /* outer /* inner */\n    println(\"x\")\n}\n")
        .expect_err("still unclosed");
    assert!(said.contains("line 2, column 5"), "{said}");
}

/// **`/** … */` and `/*! … */` are block comments and nothing more** (D3).
///
/// The language has no doc comment and this record does not add one by the back
/// door. Asserted because a reader from Rust will write the form and has to get a
/// comment rather than a refusal or a meaning.
#[test]
fn two_stars_and_a_bang_are_comments() {
    for opener in ["/**", "/*!"] {
        let rust = lowered(&format!(
            "{opener} not a doc comment */\nfn main() {{ println(\"ok\") }}"
        ))
        .unwrap_or_else(|e| panic!("`{opener}` is a comment: {e}"));
        assert!(
            !rust.contains("not a doc comment"),
            "nothing is carried into the generated Rust (§3): {rust}"
        );
    }
}

/// **A `/` that is not a comment is still division**, which is the regression a
/// new lexical rule beside `COMMENT` can cause.
#[test]
fn division_is_untouched() {
    let rust = lowered("fn main() { let n = 10 / 2 println(f\"{n}\") }").expect("it lowers");
    assert!(rust.contains("10 / 2"), "{rust}");
}
