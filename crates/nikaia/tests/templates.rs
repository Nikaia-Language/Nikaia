//! The `html` template DSL (ADR-017), compiled where the template is written.
//!
//! Three decisions, and each has a test that would fail if it were quietly
//! relaxed: every hole escaped unconditionally, "already markup" said with a
//! type, and a hole only where escaping is enough.

use nikaia::emit::template::{self, Position, Segment};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn emit(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

fn refuse(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    format!(
        "{:#}",
        emit_program(&parsed, Build::default()).expect_err("the template is refused")
    )
}

fn template(body: &str) -> String {
    format!(
        "use std::html\nfn page(name: ref String) -> String {{ return dsl html {{ {body} }} eod }}"
    )
}

/// D1: every hole goes through `html::Render`, and there is no other path.
#[test]
fn every_hole_is_rendered() {
    let emitted = emit(&template("<p>{name}</p>"));
    assert!(
        emitted.contains("::nikaia_std::html::Render::render(&name)"),
        "{emitted}"
    );
    // The literal text is written as it stands: it is markup the template
    // author typed.
    assert!(emitted.contains(r#"__html.push_str("<p>")"#), "{emitted}");
}

/// A hole holds an *expression* of this language, not a name.
#[test]
fn a_hole_holds_an_expression() {
    let emitted = emit(
        "use std::html\n\
         pub struct R { id: i32 }\n\
         fn page(r: R) -> String { return dsl html { <td>{r.id}</td> } eod }",
    );
    assert!(
        emitted.contains("::nikaia_std::html::Render::render(&r.id)"),
        "{emitted}"
    );
}

/// D3: the positions escaping is enough for, and the ones it is not.
#[test]
fn a_hole_is_only_legal_where_escaping_is_enough() {
    // Legal: a text node and a quoted attribute that is not a URL.
    emit(&template(r#"<p class="x{name}">{name}</p>"#));

    for (body, expected) in [
        ("<script>var x = {name}</script>", "`<script>` body"),
        (r#"<a href="{name}">x</a>"#, "URL attribute"),
        ("<div class={name}>x</div>", "unquoted attribute value"),
        ("<!-- {name} -->", "HTML comment"),
        ("<style>a {{ color: {name} }}</style>", "`<style>` block"),
    ] {
        let message = refuse(&template(body));
        assert!(
            message.contains(expected),
            "{body}\nshould name {expected}, said:\n{message}"
        );
        assert!(message.contains("escaping cannot make safe"), "{message}");
    }
}

/// The refusal says every hole it refuses, not the first: a template with three
/// of them should say so three times rather than once per build.
#[test]
fn every_illegal_hole_is_named_at_once() {
    let message = refuse(&template("<script>{name}</script><a href=\"{name}\">x</a>"));
    assert!(message.contains("`<script>` body"), "{message}");
    assert!(message.contains("URL attribute"), "{message}");
}

/// The scanner reads the position off the *literal* text, which is sound
/// because a hole's value is escaped and so cannot open a tag or close a
/// string.
#[test]
fn the_scan_tracks_where_it_is() {
    let segments = template::split(r#"<p class="a">x</p><script>y</script>"#).expect("splits");
    assert_eq!(segments.len(), 1, "{segments:?}");

    let with_holes =
        template::split(r#"<p class="{a}">{b}</p><script>{c}</script>"#).expect("splits");
    let positions: Vec<Position> = with_holes
        .iter()
        .filter_map(|s| match s {
            Segment::Hole { at, .. } => Some(*at),
            _ => None,
        })
        .collect();
    assert_eq!(
        positions,
        [Position::QuotedAttribute, Position::Text, Position::Script]
    );
}

/// A quoted attribute is safe unless it is a URL, where HTML escaping is not
/// the escaping that is needed - `javascript:` survives all five characters.
#[test]
fn a_url_attribute_is_not_a_quoted_attribute() {
    for (body, at) in [
        (r#"<a href="{x}">"#, Position::Url),
        (r#"<img src="{x}">"#, Position::Url),
        (r#"<form action="{x}">"#, Position::Url),
        (r#"<a title="{x}">"#, Position::QuotedAttribute),
        (r#"<div data-id="{x}">"#, Position::QuotedAttribute),
    ] {
        let segments = template::split(body).expect("splits");
        let found = segments
            .iter()
            .find_map(|s| match s {
                Segment::Hole { at, .. } => Some(*at),
                _ => None,
            })
            .expect("a hole");
        assert_eq!(found, at, "{body}");
    }
}

/// `{{` is a literal brace, the same rule an interpolated string follows: a
/// template shares the syntax rather than inventing a second one.
#[test]
fn a_doubled_brace_is_a_literal_one() {
    let segments = template::split("<style>a {{ color: red }}</style>").expect("splits");
    assert_eq!(
        segments,
        vec![Segment::Text("<style>a { color: red }</style>".to_string())]
    );
}

/// An unclosed hole is an error rather than a template that swallows the rest
/// of itself.
#[test]
fn an_unclosed_hole_is_refused() {
    let message = format!(
        "{:#}",
        template::split("<p>{name</p>").expect_err("refused")
    );
    assert!(message.contains("unclosed"), "{message}");
}

/// Only `html` is compiled as a template here, and a foreign statement with no
/// hole at all is refused: its holes are what ADR-007 D5 gives a meaning to,
/// and a body without them belongs to a grammar this compiler does not have.
/// Saying so is better than lowering it to something that reads like a template
/// and is not one.
#[test]
fn another_dsl_target_says_what_it_needs() {
    let message = refuse("fn q() -> String { return dsl sql { SELECT 1 } eod }");
    assert!(message.contains("has no `:name` holes"), "{message}");
    assert!(
        message.contains("`sql` isn't a language the compiler knows"),
        "{message}"
    );
}

// --- Control flow in the markup ---------------------------------------------

/// `<for row in :rows> … </for>`: written as an *element*, because the file is
/// markup and an editor that highlights it keeps working.
#[test]
fn a_loop_repeats_its_body() {
    let emitted = emit(
        "use std::html\n\
         pub struct R { n: i32 }\n\
         fn page(rows: Vec[R]) -> String {\n\
             return dsl html { <table><for r in :rows><tr><td>{r.n}</td></tr></for></table> } eod\n\
         }",
    );
    // `.iter()` and not `&`, which is the ordinary `for`'s rule (ADR-094 D4):
    // since D1 a parameter the body only reads is already a view.
    assert!(emitted.contains("for r in rows.iter() {"), "{emitted}");
    assert!(
        emitted.contains("::nikaia_std::html::Render::render(&r.n)"),
        "{emitted}"
    );
    // The directive itself is not markup and is not written out.
    assert!(!emitted.contains("<for"), "{emitted}");
    assert!(!emitted.contains("</for>"), "{emitted}");
}

/// The body is a template like any other, so a loop may hold a loop.
#[test]
fn a_loop_may_hold_a_loop() {
    let segments = template::split("<for a in :xs><for b in :ys>{b}</for></for>").expect("splits");
    let Segment::For { binding, body, .. } = &segments[0] else {
        panic!("expected a loop: {segments:?}");
    };
    assert_eq!(binding, "a");
    assert!(matches!(body[0], Segment::For { .. }), "{body:?}");
}

/// What is legal inside a loop is what is legal outside: a hole in a `<script>`
/// does not become safe by being repeated, and the surrounding markup still
/// decides the position because a `<for>` is not markup.
#[test]
fn the_position_check_runs_through_a_loop() {
    let message = refuse(
        "use std::html\n\
         fn page(xs: Vec[i32]) -> String {\n\
             return dsl html { <script><for x in :xs>{x}</for></script> } eod\n\
         }",
    );
    assert!(message.contains("`<script>` body"), "{message}");
}

/// The collection carries `:` because it is captured from the enclosing scope
/// (ADR-007 D4), and leaving it off is worth a sentence rather than a parse
/// error about a missing angle bracket.
#[test]
fn the_capture_marker_is_required_and_explained() {
    let message = format!(
        "{:#}",
        template::split("<for r in rows>{r}</for>").expect_err("refused")
    );
    assert!(message.contains(":rows"), "{message}");
    assert!(message.contains("enclosing scope"), "{message}");
}

/// A loop that is never closed, and a `</for>` with nothing open, each say so.
#[test]
fn an_unbalanced_loop_is_refused() {
    let open = format!(
        "{:#}",
        template::split("<for r in :rows>{r}").expect_err("refused")
    );
    assert!(open.contains("never closed"), "{open}");

    let close = format!("{:#}", template::split("x</for>").expect_err("refused"));
    assert!(close.contains("without a `<for"), "{close}");
}

/// An element whose name merely begins with `for` is an element.
#[test]
fn only_the_keyword_and_a_blank_make_a_directive() {
    let segments = template::split("<form action=\"/x\">y</form>").expect("splits");
    assert_eq!(segments.len(), 1, "{segments:?}");
    assert!(matches!(segments[0], Segment::Text(_)));
}

/// **Characters, not bytes** (0.0.252): the scan keeps the last nine
/// characters to recognise `<!--` and `</script`, and the Rust it replaced kept
/// nine *bytes* - which cut a character outside ASCII in two and aborted on
/// `byte index is not a char boundary`. The splitter is Nikaia now
/// (`nikaia-std/src/tools/template.nika`), and walks characters.
#[test]
fn text_outside_ascii_is_scanned_by_character() {
    let segments =
        template::split("<p title=\"été\">ééééééééé {name}</p><!-- ü --><a href=\"{link}\">")
            .expect("it splits");
    let positions: Vec<Position> = segments
        .iter()
        .filter_map(|segment| match segment {
            Segment::Hole { at, .. } => Some(*at),
            _ => None,
        })
        .collect();
    assert_eq!(positions, vec![Position::Text, Position::Url]);
    assert_eq!(
        template::literal_length(&segments),
        "<p title=\"été\">ééééééééé </p><!-- ü --><a href=\"\">".len()
    );
}
