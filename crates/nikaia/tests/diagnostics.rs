//! An error about the emitted Rust, reported on the Nikaia line that caused it.
//!
//! This is the whole pipeline, not a simulation of it: the fixture is lowered,
//! the emitted Rust is handed to the same `rustc` that built this test, and the
//! JSON diagnostics that come back - including the parser backend's frame
//! check, which runs inside a proc macro - are mapped through the emitter's
//! source map onto the `.nika` file.

mod common;

use nikaia::diagnostics::{self, Diagnostic};
use nikaia::emit::{Build, Lowered, emit_program};
use nikaia::parser::parse_to_ast;

const BROKEN: &str = include_str!("fixtures/broken_frame.nika");
/// Self-contained on purpose: it declares every name it uses, so what it
/// compiles to has nothing to complain about.
const GOOD: &str = include_str!("fixtures/digits.nika");

fn lower(source: &str) -> Lowered {
    let parsed = parse_to_ast(source).expect("the fixture parses");
    emit_program(&parsed, Build::default()).expect("the fixture lowers")
}

/// Compile emitted Rust and return rustc's JSON diagnostics.
///
/// `--emit=metadata` is enough: the frame check runs during macro expansion, so
/// there is no reason to pay for code generation to hear about it.
fn compile(rust: &str) -> String {
    let dir = common::scratch_dir("diagnostics");
    let source = dir.join("lowered.rs");
    std::fs::write(&source, rust).expect("write emitted Rust");

    let output = common::compile(
        &source,
        &[
            "--crate-type",
            "lib",
            "--emit=metadata",
            "--error-format",
            "json",
            "-o",
            dir.join("lowered.rmeta").to_str().expect("utf-8 path"),
        ],
    );

    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8(output.stderr).expect("rustc writes utf-8")
}

fn errors_for(source: &str) -> Vec<Diagnostic> {
    let lowered = lower(source);
    let json = compile(&lowered.rust);
    let all = diagnostics::translate(&json, &lowered.map, source);

    assert!(
        !json.trim().is_empty() || all.is_empty(),
        "rustc said nothing at all"
    );
    all.into_iter().filter(|d| d.level == "error").collect()
}

/// The 1-based line a piece of the fixture is on, so the assertions below name
/// the code rather than a number that shifts when a comment is edited.
fn line_of(source: &str, needle: &str) -> usize {
    source
        .lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` is not in the fixture"))
        + 1
}

#[test]
fn a_rejected_frame_is_reported_on_the_nika_line_that_caused_it() {
    let errors = errors_for(BROKEN);

    assert_eq!(
        errors.len(),
        1,
        "expected exactly the frame check to fail: {errors:#?}"
    );
    let error = &errors[0];

    // The backend's message is already about the Nikaia source, because the
    // lowering is name for name (ADR-011 D2) - `NAME` and `MEASUREMENT` are
    // the user's own rule names. Only the position had to be corrected.
    assert!(
        error.message.contains("until"),
        "unexpected message: {}",
        error.message
    );
    assert!(
        error.message.contains("NAME") && error.message.contains("MEASUREMENT"),
        "the message does not name the rules: {}",
        error.message
    );

    let location = error
        .location
        .as_ref()
        .expect("the frame error maps back to the source");

    // `s:until(";")` - the pattern that cannot be cut at, not the rule, not the
    // grammar, and above all not a line of generated Rust.
    assert_eq!(location.line, line_of(BROKEN, "s:until(\";\")"));
    assert_eq!(
        &BROKEN[location.span.bytes()],
        "until(\";\")",
        "the span should cover the offending pattern"
    );

    // It really was reported somewhere else first.
    assert!(
        error
            .generated_line
            .is_some_and(|line| line != location.line),
        "the emitted file should have blamed a different line"
    );
}

#[test]
fn the_rendered_message_shows_the_nika_line() {
    let errors = errors_for(BROKEN);
    let rendered = diagnostics::render(&errors[0], "broken_frame.nika", BROKEN, "lowered.rs");

    assert!(rendered.starts_with("error: "), "{rendered}");
    assert!(rendered.contains("--> broken_frame.nika:15:"), "{rendered}");
    assert!(rendered.contains("s:until(\";\")"), "{rendered}");
    assert!(rendered.contains("^^^^^"), "{rendered}");
}

#[test]
fn a_grammar_the_backend_accepts_reports_nothing() {
    // The counterpart, and the one that would catch a source map that maps
    // everything to line 1: a fixture that compiles has nothing to translate.
    let errors = errors_for(GOOD);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn the_map_leads_from_emitted_code_back_to_what_wrote_it() {
    // The map is what everything above rests on, so it is worth checking
    // directly rather than only through a diagnostic.
    let lowered = lower(BROKEN);
    assert!(!lowered.map.is_empty());

    let attribute = lowered.rust.find("#[frame(").expect("the frame attribute");
    let span = lowered
        .map
        .source_span(attribute)
        .expect("the attribute maps back");
    assert!(
        BROKEN[span.bytes()].starts_with("@frame(boundary: \"\\n\")"),
        "the attribute should lead back to the rule that carries it"
    );
}

#[test]
fn an_unmapped_diagnostic_says_so_rather_than_guessing() {
    // Nothing in the preamble comes from the source, so an error reported
    // against it has no Nikaia line - and inventing one would be worse than
    // admitting it.
    let lowered = lower(GOOD);
    let json = r#"{"$message_type":"diagnostic","message":"something about the preamble","level":"error","spans":[{"file_name":"lowered.rs","byte_start":0,"byte_end":4,"line_start":1,"column_start":1,"is_primary":true}],"children":[]}"#;

    let translated = diagnostics::translate(json, &lowered.map, GOOD);
    assert_eq!(translated.len(), 1);
    assert!(translated[0].location.is_none());

    let rendered = diagnostics::render(&translated[0], "digits.nika", GOOD, "lowered.rs");
    assert!(rendered.contains("lowered.rs:1"), "{rendered}");
    assert!(
        rendered.contains("no Nikaia source maps to this"),
        "{rendered}"
    );
}

/// What **`cargo`** writes is read by the same function, and it is the project
/// build's only channel (ADR-005 D7, Part III C.1).
///
/// `cargo --message-format=json` wraps each `rustc` diagnostic in a line of its
/// own and mixes in lines of its own that are not diagnostics at all. Both have
/// to be handled here, because until they were, `nikaia build` handed Cargo's
/// stderr to the terminal and every backend error reached the user as Rust about
/// a generated file.
///
/// The diagnostic below is an `E0277`, which is the class ADR-005 D7 was short
/// of: `Send`, `sync` and every future marker rule surfaces as a trait bound,
/// and D7's seven were all borrow, ownership or lifetime errors.
#[test]
fn a_trait_bound_error_from_cargo_is_placed_in_the_nika_file() {
    let lowered = lower(GOOD);
    // A byte the map knows. Which byte is the emitter's business, so it is
    // found rather than guessed: the first one the map covers at all.
    let offset = (0..lowered.rust.len())
        .find(|at| lowered.map.source_span(*at).is_some())
        .expect("the map covers some of what was emitted");

    let end = offset + 7;
    let message = format!(
        r#"{{"reason":"compiler-message","message":{{"message":"`Rc<String>` cannot be sent between threads safely","code":{{"code":"E0277"}},"level":"error","spans":[{{"file_name":"digits.rs","byte_start":{offset},"byte_end":{end},"line_start":7,"column_start":5,"is_primary":true}}],"children":[{{"level":"help","message":"within `LocalHandle`, the trait `Send` is not implemented for `Rc<String>`"}}]}}}}"#
    );
    let json = format!(
        "{}\n{message}\n{}\n",
        r#"{"reason":"compiler-artifact","target":{"name":"digits"}}"#,
        r#"{"reason":"build-finished","success":false}"#,
    );

    let translated = diagnostics::translate_units(&json, &lowered.map, &[GOOD]);
    assert_eq!(
        translated.len(),
        1,
        "Cargo's own lines carry no diagnostic: {translated:#?}"
    );
    let placed = translated[0]
        .location
        .as_ref()
        .expect("the map knows where the entry came from");
    assert_eq!(placed.unit, 0);

    let rendered = diagnostics::render(&translated[0], "digits.nika", GOOD, "digits.rs");
    assert!(rendered.starts_with("error: "), "{rendered}");
    assert!(rendered.contains("--> digits.nika:"), "{rendered}");
    assert!(
        !rendered.contains("digits.rs"),
        "and not against the generated Rust: {rendered}"
    );
    assert!(
        rendered.contains("the trait `Send` is not implemented"),
        "the note comes through - the **text** is still Rust's, which ADR-005 \
         D7 records as the open half: {rendered}"
    );
}

/// A note that tells the reader about **Rust** rather than about their program
/// is dropped; one they can act on is kept.
///
/// ADR-012: a diagnostic is about the `.nika` file the user wrote. Most of what
/// the backend says survives translation because it is true in either language -
/// a literal that does not fit a range does not fit it here either. Two classes
/// do not, and the test for both is whether the reader can act on it: a lint
/// attribute cannot be written in this language at all, and Rust's tooling is not
/// the reader's, whose file is `nikaia.toml`.
///
/// The two kept notes were checked against the compiler rather than assumed:
/// `let _unused = 5` is accepted, so that remedy works here, and the range
/// sentence is plainly about the program.
///
/// **One of them stopped being kept.** *"consider using the type `u32`"* was
/// kept on the same ground - it compiles - and
/// [ADR-048](../../../docs/specification/adr/adr-048.md) D2 took it away: the
/// numeric surface is the one Part I 2.2 names, and `u32` is deliberately not on
/// it. A remedy that works is kept; one that leads out of the language is not.
#[test]
fn a_note_about_rust_rather_than_the_program_is_dropped() {
    let lowered = lower(GOOD);
    let children = [
        // Dropped: the backend explaining its own configuration.
        ("`#[deny(overflowing_literals)]` on by default", false),
        (
            "`#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default",
            false,
        ),
        // Dropped: tooling the reader does not have.
        (
            "if you wanted to use a crate named `fremd`, use `cargo add fremd` to add it to your `Cargo.toml`",
            false,
        ),
        (
            "run with `RUST_BACKTRACE=full` for a verbose backtrace",
            false,
        ),
        // Dropped: a remedy in a type the specification does not offer. It was
        // kept here once, checked - `let x: u32 = 3000000000` compiles - and
        // ADR-048 D2 is what changed: the numeric surface is the one Part I 2.2
        // names, and `u32` is deliberately not on it. The answer here is `i64`,
        // or a use that widens the literal.
        ("consider using the type `u32` instead", false),
        // Kept: both of these are remedies that work in Nikaia.
        (
            "the literal `3000000000` does not fit into the type `i32` whose range is `-2147483648..=2147483647`",
            true,
        ),
        ("if this is intentional, prefix it with an underscore", true),
    ];

    let notes: String = children
        .iter()
        .map(|(text, _)| {
            format!(
                r#",{{"level":"note","message":{}}}"#,
                serde_json::to_string(text).expect("a JSON string")
            )
        })
        .collect::<String>();
    let json = format!(
        r#"{{"$message_type":"diagnostic","message":"literal out of range","level":"error","spans":[{{"file_name":"lowered.rs","byte_start":0,"byte_end":4,"line_start":1,"column_start":1,"is_primary":true}}],"children":[{{"level":"note","message":"kept for shape"}}{notes}]}}"#
    );

    let translated = diagnostics::translate(&json, &lowered.map, GOOD);
    assert_eq!(translated.len(), 1, "{translated:#?}");
    let kept = &translated[0].notes;

    for (text, keep) in children {
        assert_eq!(
            kept.iter().any(|n| n == text),
            keep,
            "{}: {text}",
            match keep {
                true => "must be kept",
                false => "must be dropped",
            }
        );
    }
}

/// **A name this compiler substituted on the way out is put back on the way in**
/// (issue #144, the relayed type name).
///
/// The emitter writes a trusted input's map as `TrustedMap`, which is
/// `HashMap<K, V, BuildHasherDefault<FxHasher>>` (ADR-010 D5). Measured before
/// this: `let m = HashMap()` — a real defect in the *program*, reported
/// against the right line — said *"type annotations needed for `HashMap<_, _,
/// BuildHasherDefault<FxHasher>>`"*, naming a hasher nothing in the program
/// mentions.
///
/// **Only a substitution that is purely a name is undone**, which is `map_name`'s
/// own rule: same table, same API, same equality. `Shared[T]` is deliberately not
/// undone even though the emitter substitutes it too — `Rc` and `Arc` are
/// different types, and *"expected `Shared[T]`, found `Shared[T]`"* would hide a
/// defect in this compiler instead of translating one of Rust's words.
#[test]
fn the_hasher_this_compiler_chose_is_not_in_the_message() {
    let lowered = lower(GOOD);
    let json = format!(
        r#"{{"$message_type":"diagnostic","message":{},"level":"error","spans":[{{"file_name":"lowered.rs","byte_start":0,"byte_end":4,"line_start":1,"column_start":1,"is_primary":true}}],"children":[{{"level":"note","message":{}}}]}}"#,
        serde_json::to_string(
            "type annotations needed for `HashMap<_, _, BuildHasherDefault<FxHasher>>`"
        )
        .expect("a JSON string"),
        serde_json::to_string("`TrustedMap<ref String, i64>` is the type of `m`")
            .expect("a JSON string"),
    );

    let translated = diagnostics::translate(&json, &lowered.map, GOOD);
    assert_eq!(translated.len(), 1, "{translated:#?}");
    assert_eq!(
        translated[0].message,
        "type annotations needed for `HashMap<_, _>`"
    );
    assert_eq!(
        translated[0].notes[0],
        "`HashMap<ref String, i64>` is the type of `m`"
    );
    for said in [&translated[0].message, &translated[0].notes[0]] {
        assert!(!said.contains("FxHasher"), "{said}");
        assert!(!said.contains("Trusted"), "{said}");
    }
}

/// …and so is the one that used to be left alone
/// ([ADR-056](../../../docs/specification/adr/adr-056.md) D1).
///
/// `Rc<Conn>` and `Arc<Conn>` are both `Shared[Conn]`, and this message used to
/// reach the user in Rust's words on the ground that translating it would say
/// the same thing twice and hide a defect in this compiler. It does say the same
/// thing twice — and that is **reported**, not avoided (D2): a reader who wrote
/// `Shared[Conn]` is no better served by `Rc` and `Arc`, two words the page does
/// not have, and Part III C.1 calls an untranslated backend error reaching them a
/// bug in this compiler.
#[test]
fn a_translation_that_collapses_a_distinction_is_an_internal_error() {
    let lowered = lower(GOOD);
    let offset = (0..lowered.rust.len())
        .find(|at| lowered.map.source_span(*at).is_some())
        .expect("the map covers some of what was emitted");
    let end = offset + 4;
    let json = format!(
        r#"{{"$message_type":"diagnostic","message":{},"level":"error","spans":[{{"file_name":"lowered.rs","byte_start":{offset},"byte_end":{end},"line_start":1,"column_start":1,"is_primary":true}}],"children":[]}}"#,
        serde_json::to_string("expected struct `Arc<Conn>`, found struct `Rc<Conn>`")
            .expect("a JSON string"),
    );

    let translated = diagnostics::translate(&json, &lowered.map, GOOD);
    assert_eq!(
        translated[0].message, "expected struct `Shared[Conn]`, found struct `Shared[Conn]`",
        "the name is put back like every other"
    );
    assert_eq!(
        translated[0].internal.as_deref(),
        Some("expected struct `Arc<Conn>`, found struct `Rc<Conn>`"),
        "and the backend's own words are kept, because that is what a bug report needs"
    );

    let rendered = diagnostics::render(&translated[0], "p.nika", GOOD, "p.rs");
    assert!(
        rendered.starts_with("internal error: This is a bug in Nikaia")
            && rendered.contains("--> p.nika:"),
        "{rendered}"
    );
    assert!(
        rendered.contains("This is a bug in Nikaia, not in your program."),
        "the reader is told whose mistake it is: {rendered}"
    );
    assert!(
        rendered.contains("`Arc<Conn>`") && rendered.contains("`Rc<Conn>`"),
        "and shown what the backend said: {rendered}"
    );
}

/// **A message rustc itself wrote that way is its own business.**
///
/// Two types of one name, from two crates, is a real thing to say about a real
/// program — so the rule is not "a message that names one thing twice" but "a
/// message that only *became* that way here" (D2). Getting this backwards would
/// turn somebody's correct diagnostic into a bug report about this compiler.
#[test]
fn a_message_the_backend_wrote_that_way_is_not_an_internal_error() {
    let lowered = lower(GOOD);
    let offset = (0..lowered.rust.len())
        .find(|at| lowered.map.source_span(*at).is_some())
        .expect("the map covers some of what was emitted");
    let end = offset + 4;
    let json = format!(
        r#"{{"$message_type":"diagnostic","message":{},"level":"error","spans":[{{"file_name":"lowered.rs","byte_start":{offset},"byte_end":{end},"line_start":1,"column_start":1,"is_primary":true}}],"children":[]}}"#,
        serde_json::to_string("expected struct `Conn`, found struct `Conn`")
            .expect("a JSON string"),
    );

    let translated = diagnostics::translate(&json, &lowered.map, GOOD);
    assert_eq!(
        translated[0].internal, None,
        "nothing here was collapsed by this compiler"
    );
    let rendered = diagnostics::render(&translated[0], "p.nika", GOOD, "p.rs");
    assert!(
        rendered.starts_with("error: ") && rendered.contains("--> p.nika:"),
        "so it stays a message about the program: {rendered}"
    );
}

/// And a hull inside a hull comes through whole, which is what says the brackets
/// are **matched** rather than searched for.
#[test]
fn a_nested_shared_type_is_translated_whole() {
    let lowered = lower(GOOD);
    let offset = (0..lowered.rust.len())
        .find(|at| lowered.map.source_span(*at).is_some())
        .expect("the map covers some of what was emitted");
    let end = offset + 4;
    let json = format!(
        r#"{{"$message_type":"diagnostic","message":{},"level":"error","spans":[{{"file_name":"lowered.rs","byte_start":{offset},"byte_end":{end},"line_start":1,"column_start":1,"is_primary":true}}],"children":[]}}"#,
        serde_json::to_string("`Arc<Vec<Rc<Conn>>>` is not what this takes")
            .expect("a JSON string"),
    );

    let translated = diagnostics::translate(&json, &lowered.map, GOOD);
    assert_eq!(
        translated[0].message,
        "`Shared[Vec<Shared[Conn]>]` is not what this takes"
    );
    assert_eq!(translated[0].internal, None);
}

/// **The words put back are Nikaia's own** (`tools/rustc_words.nika`, #125):
/// a hull nested in a hull comes through whole, a bare `Rc<` does not eat the
/// tail of a name that ends in it, and the lock's and the map's shapes go back
/// as the names the program wrote.
#[test]
fn a_rustc_message_is_put_in_the_programs_words() {
    use nikaia_std::tools::rustc_words::in_this_language;
    assert_eq!(
        in_this_language("expected `Rc<Vec<Arc<Conn>>>`, found `MyRc<u8>`"),
        "expected `Shared[Vec<Shared[Conn]>]`, found `MyRc<u8>`"
    );
    assert_eq!(
        in_this_language("`TrustedMap<K, V, BuildHasherDefault<FxHasher>>` in `lock::Crossing<T>`"),
        "`HashMap<K, V>` in `Locked<T>`"
    );
}

/// **The layout of a finding is written in Nikaia** (`tools/render.nika`,
/// #125), and a span is in bytes where a column is in characters: on a line
/// with `ü` before the place, the caret stands under the word and the header
/// counts what an editor counts. Two labels, `-` and `^`, the lines between
/// them elided, the notes and the help as sentences.
#[test]
fn a_finding_is_laid_out_in_characters() {
    let source = "fn main() {\n    let grüße = 0\n    println(grüße)\n    \n    grüße += 1\n}\n";
    let declared = source.find("let grüße").expect("declared");
    let changed = source.find("grüße +=").expect("changed");
    let finding = nikaia::check::Finding {
        severity: nikaia::check::Severity::Error,
        span: nikaia::ast::Span::new(changed, changed + "grüße += 1".len()),
        code: "NK1139",
        message: "you're changing `grüße`, but it wasn't declared as mutable".to_string(),
        notes: vec!["a name is fixed unless it says `mut`".to_string()],
        help: Some("add `mut` where it's declared".to_string()),
        labels: vec![
            nikaia::check::Label {
                span: nikaia::ast::Span::new(declared, declared + "let grüße = 0".len()),
                word: "grüße".to_string(),
                text: "declared here without `mut`".to_string(),
                main: false,
            },
            nikaia::check::Label {
                span: nikaia::ast::Span::new(changed, changed + "grüße += 1".len()),
                word: "grüße".to_string(),
                text: "changed here".to_string(),
                main: true,
            },
        ],
    };
    assert_eq!(
        diagnostics::render_finding(&finding, "app.nika", source),
        "error[NK1139]: You're changing `grüße`, but it wasn't declared as mutable.\n\
         \x20 --> app.nika:5:5\n\
         \x20  |\n\
         \x202 |     let grüße = 0\n\
         \x20  |         ----- declared here without `mut`\n\
         \x20  ...\n\
         \x205 |     grüße += 1\n\
         \x20  |     ^^^^^ changed here\n\
         \x20  |\n\
         \x20  = note: A name is fixed unless it says `mut`.\n\
         \x20  = help: Add `mut` where it's declared.\n"
    );
    // A finding that names no place is marked where it stands, as wide as
    // its first line.
    let unlabelled = nikaia::check::Finding {
        labels: Vec::new(),
        ..finding
    };
    let said = diagnostics::render_finding(&unlabelled, "app.nika", source);
    assert!(
        said.contains(" 5 |     grüße += 1\n   |     ^^^^^^^^^^\n"),
        "{said}"
    );
}
