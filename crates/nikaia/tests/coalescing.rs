//! There is no postfix `??`, and the three things one is reached for each have
//! a spelling ([ADR-279](../../../docs/specification/adr/adr-279.md)).
//!
//! [ADR-289](../../../docs/specification/adr/adr-289.md) D19 wrote
//! `lookup(a.query("id")??)` and Part III 17.1 copied it, against a Part I 3.5
//! that defines `??` as `a ?? b` and nothing else. The question — *does the
//! language have a postfix unwrap?* — is answered **no**, and the argument is
//! the table below: every one of the three is written, compiled and run in
//! `tests/language/src/coalescing.nika`; here stays the refusal.

use nikaia::parser::parse_to_ast;

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
