//! What a literal is, and what survives the lowering.
//!
//! Three of these came out of writing `examples/json/src/main.nika`, which is what the
//! examples are for: a character literal was not a thing the language had, a
//! string could not hold a `\u{…}` escape, and output could not be composed
//! without a newline attached to every piece.

mod common;

use nikaia::ast::{Expr, Item, MatchPattern, Stmt};
use nikaia::contracts::LedgerOps;
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn emit(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Kap 2.2: `'a'` is a character, and its body is kept as written.
///
/// The language below spells a character literal the same way, so the lowering
/// is a transcription: nothing here decides what `\n` means, which is the same
/// bargain `LitStr` makes.
#[test]
fn a_character_literal_is_kept_as_written() {
    let parsed = parse_to_ast("fn main() { let c = '\\n' }").expect("parses");
    let Item::Fn { body, .. } = &parsed.program.items[0].node else {
        panic!("expected a function");
    };
    let Stmt::Let { value, .. } = &body.stmts[0].node else {
        panic!("expected a `let`");
    };
    assert!(matches!(value, Expr::LitChar(c) if c == "\\n"), "{value:?}");

    assert!(emit("fn main() { let c = '\\n' }").contains("let c = '\\n';"));
}

/// A quote and a backslash are characters like any other.
#[test]
fn the_awkward_characters_are_characters() {
    let emitted = emit("fn main() { let q = '\\'' let b = '\\\\' let s = ' ' }");
    assert!(emitted.contains("let q = '\\'';"), "{emitted}");
    assert!(emitted.contains("let b = '\\\\';"), "{emitted}");
    assert!(emitted.contains("let s = ' ';"), "{emitted}");
}

/// Kap 3.4: a character is a pattern, which is what makes a `match` over
/// `chars()` read as one - `examples/json/src/main.nika` decodes its escapes that way.
#[test]
fn a_character_literal_is_a_pattern() {
    let source = "fn f(c: char) { match c { 'n' => { println(\"newline\") } else => { } } }";
    let parsed = parse_to_ast(source).expect("parses");
    let Item::Fn { body, .. } = &parsed.program.items[0].node else {
        panic!("expected a function");
    };
    let Stmt::Expr(Expr::Match { arms, .. }) = &body.stmts[0].node else {
        panic!("expected a `match`");
    };
    assert!(
        matches!(&arms[0].pattern, MatchPattern::Literal(Expr::LitChar(c)) if c == "n"),
        "{:?}",
        arms[0].pattern
    );
    assert!(emit(source).contains("'n' =>"), "{}", emit(source));
}

/// A `{` inside an escape is part of the escape, not a hole.
///
/// A string's body reaches the emitter as it was written - the parser keeps the
/// escapes rather than decoding them - so an interpolation scanner that does
/// not know that reads `"\u{0041}"` as a hole named `0041` and emits a
/// `format!` with an argument nobody wrote.
#[test]
fn a_unicode_escape_is_not_a_hole() {
    let emitted = emit(r#"fn main() { println("\u{0041}") }"#);
    assert!(emitted.contains(r#"println!("\u{0041}")"#), "{emitted}");
    assert!(!emitted.contains("0041)"), "{emitted}");
}

/// … and a hole beside one is still a hole.
#[test]
fn an_escape_does_not_swallow_the_interpolation_after_it() {
    let emitted = emit(r#"fn main() { let x = 1 println(f"\u{0041}{x}\n") }"#);
    assert!(
        emitted.contains(r#"println!("\u{0041}{}\n", x)"#),
        "{emitted}"
    );
}

/// `print` writes without a newline, which is what output composed piece by
/// piece needs - a pretty-printer cannot put one after every fragment.
#[test]
fn print_is_a_macro_like_println() {
    let emitted = emit(r#"fn main() { print("a") eprint("b") }"#);
    assert!(emitted.contains(r#"print!("a")"#), "{emitted}");
    assert!(emitted.contains(r#"eprint!("b")"#), "{emitted}");
}

/// **A hole is code as written** ([ADR-262](../../../docs/specification/adr/adr-262.md)
/// D3): the grammar parses it, so a string inside one is written with plain
/// quotes, as in any other expression.
#[test]
fn a_hole_may_hold_a_string_literal() {
    let emitted =
        emit(r#"fn f(s: ref String) -> i32 { return 1 } fn main() { println(f"{f("a")}") }"#);
    assert!(emitted.contains(r#"f("a")"#), "{emitted}");

    // The inner string's own escapes are its own.
    let escaped =
        emit(r#"fn f(s: ref String) -> i32 { return 1 } fn main() { println(f"{f("a\nb")}") }"#);
    assert!(escaped.contains(r#"f("a\nb")"#), "{escaped}");
}

/// **A brace is a brace** (ADR-035 D1).
///
/// The case this was written for is `examples/json/src/main.nika`, where a program
/// whose job is to print `{` had to write `print("{{}}")` - and every string
/// in every program paid for a feature one string in four uses.
#[test]
fn a_plain_string_is_text_and_a_brace_is_part_of_it() {
    let emitted = emit(r#"fn main() { let x = "{ \"a\": 1 }" }"#);
    assert!(emitted.contains(r#"let x = "{ \"a\": 1 }";"#), "{emitted}");
}

/// …and Rust's macro must not read that brace as a hole of its own.
///
/// The one place the two languages disagree about a string literal: `print` is
/// a macro below, so a plain string's braces are doubled on the way down. That
/// is a transcription detail and not a rule anybody writes.
#[test]
fn a_plain_string_printed_keeps_its_braces() {
    let emitted = emit(r#"fn main() { print("{}") }"#);
    assert!(emitted.contains(r#"print!("{{}}")"#), "{emitted}");
}

/// An escape is copied whole, `\u{…}` included - doubling the braces inside one
/// would hand `println!` a `\u` with nothing after it.
#[test]
fn an_escape_is_not_a_brace_to_double() {
    let emitted = emit(r#"fn main() { println("\u{0041}") }"#);
    assert!(emitted.contains(r#"println!("\u{0041}")"#), "{emitted}");
}

/// `f"…"` is what says there is code in here (ADR-035 D1).
#[test]
fn an_f_string_interpolates_and_a_plain_one_does_not() {
    let woven = emit(r#"fn main() { let n = 1 let s = f"n is {n}" }"#);
    assert!(woven.contains(r#"format!("n is {}", n)"#), "{woven}");

    let plain = emit(r#"fn main() { let n = 1 let s = "n is {n}" }"#);
    assert!(plain.contains(r#"let s = "n is {n}";"#), "{plain}");
}

/// **The type comes from the syntax, not from the text** (ADR-035 D3).
///
/// Before the `f`, adding a brace to a string changed its type: `"a"` was a
/// view of static text and `"a {b}"` a `String`, and which one you had written
/// depended on whether the *content* happened to hold a hole. Now the first
/// character says it, so an `f"…"` with nothing in it is still a `String`.
#[test]
fn an_f_string_is_a_string_even_with_no_hole_in_it() {
    let emitted = emit(r#"fn main() { let s = f"no holes here" }"#);
    assert!(
        emitted.contains(r#""no holes here".to_string()"#),
        "{emitted}"
    );
}

/// The `f` and the quote are **one token**, so whitespace cannot change what a
/// program means: `f "x"` is the variable `f` beside a string, and it is not
/// an interpolation that lost its nerve.
#[test]
fn a_space_after_the_f_is_not_an_interpolation() {
    let parsed = parse_to_ast(r#"fn main() { let f = 1 let s = "{f}" }"#).expect("parses");
    let Item::Fn { body, .. } = &parsed.program.items[1 - 1].node else {
        panic!("expected a function");
    };
    let Stmt::Let { value, .. } = &body.stmts[1].node else {
        panic!("expected a `let`");
    };
    assert!(
        matches!(value, Expr::LitStr { .. }),
        "a plain string became an interpolation: {value:?}"
    );
}

/// **A number no use constrains takes the first type that holds it**
/// ([ADR-060](../../../docs/specification/adr/adr-060.md) D2, Part I 2.4).
///
/// `let big = 3000000000` is a correct program and was refused in the backend's
/// words about a type it never wrote — *"literal out of range for `i32`"* —
/// because Rust's integer **default** was inherited along with its inference. So
/// a literal an `i32` does not hold is written as an `i64`, and nothing else
/// changes.
#[test]
fn a_literal_an_i32_does_not_hold_is_written_as_an_i64() {
    let emitted = emit("fn main() { let big = 3000000000 }");
    assert!(emitted.contains("let big = 3000000000i64;"), "{emitted}");
}

/// **And one that fits is left alone**, which is what keeps the first half of
/// Part I 2.4 working: the use decides, so a suffix here would pin what the use
/// is supposed to answer. `let small = 42` handed to a parameter taking an
/// `i64` is that page's own example, and it compiles because `42` carries no
/// type of its own.
#[test]
fn a_literal_that_fits_carries_no_type_of_its_own() {
    let emitted = emit("fn main() { let small = 42 }");
    assert!(emitted.contains("let small = 42;"), "{emitted}");
    assert!(!emitted.contains("42i64"), "{emitted}");
}

/// **The value, and not the digits** (D3's negation).
///
/// `-2147483648` is exactly `i32::MIN` and `2147483648` is one past `i32::MAX`,
/// so a rule that read the literal alone would widen the one number where
/// widening is wrong. The negation is folded before the question is asked.
#[test]
fn a_negation_is_folded_before_the_width_is_decided() {
    let emitted = emit("fn main() { let edge = -2147483648 }");
    assert!(emitted.contains("let edge = -2147483648;"), "{emitted}");
    assert!(!emitted.contains("i64"), "{emitted}");

    // One further out is an `i64`, and the sign is still written.
    let wider = emit("fn main() { let past = -2147483649 }");
    assert!(wider.contains("-2147483649i64"), "{wider}");
}

// --- a constant sum widens the way a constant does (ADR-063) -----------------

/// **A constant written only in literals takes the first type that holds it**
/// ([ADR-063](../../../docs/specification/adr/adr-063.md) D1).
///
/// [ADR-060](../../../docs/specification/adr/adr-060.md) gave that to a literal
/// and the rule did not reach a sum, so `3000000000 + 1` compiled and
/// `2000000000 + 2000000000` did not — refused in the backend's words, *"this
/// arithmetic operation will overflow"*, on the Nikaia line that wrote it. Which
/// of the two works was not predictable from any page.
#[test]
fn a_constant_sum_an_i32_does_not_hold_is_written_wide() {
    let emitted = emit("fn main() { let c = 2000000000 + 2000000000 }");
    assert!(
        emitted.contains("2000000000i64 + 2000000000i64"),
        "{emitted}"
    );
}

/// **Every literal in it, not just the outermost.** Rust computes in the type of
/// the operands, so one suffix on one half would be two types meeting across a
/// `+`. The decision is taken once, on the outermost expression that folds, and
/// reaches every literal under it.
#[test]
fn the_whole_expression_agrees_about_its_type() {
    let emitted = emit("fn main() { let c = 1000000000 * 2 + 1000000000 * 2 }");
    assert!(
        emitted.contains("1000000000i64 * 2i64 + 1000000000i64 * 2i64"),
        "{emitted}"
    );

    // Including a negation, whose own fast path writes the sign itself.
    let signed = emit("fn main() { let c = -2000000000 - 2000000000 }");
    assert!(
        signed.contains("-2000000000i64 - 2000000000i64"),
        "{signed}"
    );
}

/// **And one that fits is left alone**, which is the whole of what keeps every
/// program that compiles today compiling: a suffix pins what a use is supposed
/// to decide (Part I 2.4).
#[test]
fn a_constant_sum_that_fits_carries_no_type_of_its_own() {
    let emitted = emit("fn main() { let small = 2 + 3 }");
    assert!(emitted.contains("let small = 2 + 3;"), "{emitted}");
    assert!(!emitted.contains("i64"), "{emitted}");
}

/// **Not where the position decides the type.** A sequence index and a repeat
/// count are `usize`, and both are deliberately left bare for Rust's own
/// inference to answer — `index::at(0)` has nothing to infer from. A suffix
/// written into one would pin the type the position is there to give, and what
/// came back would be a message about the generated file.
#[test]
fn a_position_that_infers_the_type_is_left_alone() {
    let indexed = emit("fn main() { let xs = \"abc\"\n let c = xs[1 + 1] }");
    assert!(!indexed.contains("1i64"), "{indexed}");

    let counted = emit("fn main() { let s = \"ab\".repeat(2 + 1) }");
    assert!(!counted.contains("i64"), "{counted}");
}

// ---------------------------------------------------------------------------
// The escape set is this language's, and a word that is not in it is refused
// here ([ADR-188](../../../docs/specification/adr/adr-188.md))
// ---------------------------------------------------------------------------

/// Every finding the checker has about a source.
fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

/// The `NK1184` a source raises, where it raises one.
fn refused(source: &str) -> Option<nikaia::check::Finding> {
    findings(source).into_iter().find(|f| f.code == "NK1184")
}

/// **The line the entry was written for.** `println("a\qb")` used to be
/// `rustc`'s, ending *for more information, visit
/// doc.rust-lang.org/reference/tokens.html*: a Nikaia program sent to the Rust
/// reference to find out what it may write
/// ([Part III C.2](../../../docs/specification/30-nikaia-tooling.md)).
#[test]
fn an_escape_the_set_does_not_name_is_refused_here() {
    let found = refused("fn main() { println(\"a\\qb\") }").expect("refused");
    assert!(found.message.contains("`\\q`"), "{}", found.message);
    // **The set is in the message**, because the page and the message print
    // the same list and a set written twice is a set that disagrees with
    // itself.
    assert!(
        found.notes.iter().any(|n| n.contains("\\u{…}")),
        "{:?}",
        found.notes
    );
    // A way out the program can take: the backslash was meant literally.
    assert!(
        found
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("a\\\\qb"),
        "{:?}",
        found.help
    );
}

/// **Every escape the set does name is accepted**, which is the half that keeps
/// this from refusing a correct program
/// ([C.4](../../../docs/specification/30-nikaia-tooling.md)).
#[test]
fn the_whole_set_is_accepted() {
    let source = "fn main() { println(\"\\n \\r \\t \\0 \\\\ \\\" \\x41 \\u{1F600}\") }";
    assert!(refused(source).is_none());
    // And it lowers, because the language below reads the same bytes back.
    assert!(emit(source).contains("\\u{1F600}"));
}

/// **A character literal and an `f"…"` are the same literal one shape over**,
/// and each keeps its body as written — so each is asked the same question.
#[test]
fn a_character_and_an_interpolation_are_asked_too() {
    assert!(refused("fn main() { let c = '\\q' }").is_some());
    assert!(refused("fn main() { println(f\"x\\qy\") }").is_some());
    // A string inside a hole is its own literal, and its escapes are its own.
    assert!(refused("fn main() { println(f\"{greet(\"a\")}\") }\nfn greet(s: ref String) -> String { return s.clone() }").is_none());
}

/// **A malformed `\x` or `\u{…}` is refused with the form it should have had**,
/// which is the other way out and not the backslash: a program that wrote
/// `\x80` meant a character rather than a backslash
/// ([C.2](../../../docs/specification/30-nikaia-tooling.md) — a way out that
/// cannot be taken is not one).
#[test]
fn a_malformed_numeric_escape_is_told_the_form() {
    for source in [
        "fn main() { println(\"a\\x80b\") }",
        "fn main() { println(\"a\\u{ZZ}b\") }",
        "fn main() { println(\"a\\u{\") }",
    ] {
        let found = refused(source).unwrap_or_else(|| panic!("refused: {source}"));
        assert!(
            found
                .help
                .as_deref()
                .unwrap_or_default()
                .contains("\\u{1F600}"),
            "{source}: {:?}",
            found.help
        );
    }
}

/// **One table, two readers** ([ADR-188](../../../docs/specification/adr/adr-188.md) D2).
///
/// The refusal and the build-time decoder walk the same escapes, so everything
/// one refuses is something the other cannot decode — and nothing the decoder
/// accepts is refused. Two copies of the set would drift, and the one that
/// drifted open would refuse a literal the other reads.
#[test]
fn the_refusal_and_the_decoder_read_one_set() {
    for literal in [
        "\\n",
        "\\r",
        "\\t",
        "\\0",
        "\\\\",
        "\\'",
        "\\\"",
        "\\x41",
        "\\u{1F600}",
        "\\u{1_F600}",
        "\\u{00_0041}",
        "plain",
    ] {
        assert!(
            nikaia::build_time::an_escape_nothing_names(literal).is_none(),
            "{literal}"
        );
        assert!(nikaia::build_time::decoded(literal).is_some(), "{literal}");
    }
    for literal in [
        "\\q",
        "\\x80",
        "\\xZZ",
        "\\x+1",
        "\\u{ZZ}",
        "\\u{+41}",
        "\\u{_41}",
        "\\u{}",
        "\\u{0000041}",
        "\\u{D800}",
        "\\u41",
        "a\\",
    ] {
        assert!(
            nikaia::build_time::an_escape_nothing_names(literal).is_some(),
            "{literal}"
        );
        assert!(nikaia::build_time::decoded(literal).is_none(), "{literal}");
    }
}

// ---------------------------------------------------------------------------
// What a hole may hold: `NK1201`
// ---------------------------------------------------------------------------

/// **A list in a hole is refused here** and not by `rustc`. `f"Reading:
/// {data}"` over `[1, 2, 3]` lowered to a `{}` and the language below said
/// *`Vec<i64>` doesn't implement `std::fmt::Display`* about a file nobody
/// wrote. Part I 2.5 gives a hole an expression and a collection no text form,
/// so the help is the way out that exists: join the elements first.
#[test]
fn a_list_in_a_hole_is_refused_with_the_join_that_writes_it() {
    let source = "fn main() {\n\
                  \x20   let data = [1, 2, 3]\n\
                  \x20   println(f\"Reading: {data}\")\n\
                  }\n";
    let found = findings(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1201");
    assert_eq!(
        found[0].message,
        "You can't put a list in an `f\"…\"` hole."
    );
    assert_eq!(
        found[0].notes[0],
        "A hole is written as its value's text, and lists, maps and sets have no text \
         form. `data` is a list."
    );
    assert_eq!(
        found[0].help.as_deref(),
        Some(
            "Join its elements into text first: `let text = data.iter().join(\", \")`, then write `{text}`."
        )
    );
}

/// **A map and a set are the same refusal**, two holes in one literal, and a
/// hole that says `:?` is left to the form it asked for.
#[test]
fn a_map_or_a_set_in_a_hole_is_refused_too() {
    let source = "use std::collections\n\
                  \n\
                  fn main() {\n\
                  \x20   let m: collections::HashMap[String, i64] = collections::HashMap()\n\
                  \x20   let s: collections::HashSet[i64] = collections::HashSet()\n\
                  \x20   println(f\"{m} and {s}\")\n\
                  \x20   println(f\"{m:?}\")\n\
                  }\n";
    let found: Vec<_> = findings(source)
        .into_iter()
        .filter(|f| f.code == "NK1201")
        .collect();
    assert_eq!(found.len(), 2, "{:#?}", findings(source));
    assert!(
        found[0].message.starts_with("You can't put `"),
        "{found:#?}"
    );
    assert!(
        found[1].notes[0].contains("`s` is `collections::HashSet[i64]`"),
        "{found:#?}"
    );
}

/// **What a hole can print it still prints**: a list's length, one element,
/// and the joined text the help names, which compiles and runs. A value this
/// checker cannot type is not refused on a guess (Part III C.4).
#[test]
fn the_way_out_runs_and_what_prints_is_untouched() {
    let source = "fn main() {\n\
                  \x20   let data = [1, 2, 3]\n\
                  \x20   let text = data.iter().join(\", \")\n\
                  \x20   let words = [\"a\", \"b\"]\n\
                  \x20   let joined = words.iter().join(\"-\")\n\
                  \x20   println(f\"Reading: {text} {data.len()} {data[0]} {joined}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = emit(source);
    let dir = common::scratch_dir("literals-joined-hole");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );
    let out = std::process::Command::new(&binary)
        .output()
        .expect("run it");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Reading: 1, 2, 3 3 1 a-b\n"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **The grammar parses a hole** ([ADR-262](../../../docs/specification/adr/adr-262.md)
/// D1, D2): the literal's parts are text runs and holes, and a hole is an
/// expression of the tree - with its format, where one follows the `:` - and
/// the emitter builds the format string from them (D5).
#[test]
fn the_tree_holds_the_parts_of_an_interpolation() {
    use nikaia::ast::{Expr, FPart, Item, Stmt};
    let parsed = nikaia::parser::parse_to_ast(
        "fn main() { let n = 3 println(f\"a {n:>4} b {g(\"x\")} {{c}}\") }",
    )
    .expect("it parses");
    let Item::Fn { body, .. } = &parsed.program.items[0].node else {
        panic!("a function");
    };
    let Stmt::Expr(Expr::Call { args, .. }) = &body.stmts[1].node else {
        panic!("a call: {:?}", body.stmts[1].node);
    };
    let Expr::LitInterpolated { parts } = &args[0] else {
        panic!("an interpolation: {:?}", args[0]);
    };
    let holes: Vec<_> = parts
        .iter()
        .filter_map(|p| match p {
            FPart::Hole { expr, spec, at } => Some((expr, spec.as_deref(), *at)),
            FPart::Text(_) => None,
        })
        .collect();
    // Each hole is found again by the byte its `{` stands at (D2).
    assert_eq!((holes[0].2, holes[1].2), (34, 43), "{holes:?}");
    let holes: Vec<_> = holes.into_iter().map(|(e, s, _)| (e, s)).collect();
    assert_eq!(holes.len(), 2, "{parts:?}");
    assert!(
        matches!(holes[0], (Expr::Variable(_), Some(">4"))),
        "{:?}",
        holes[0]
    );
    assert!(
        matches!(holes[1], (Expr::Call { .. }, None)),
        "{:?}",
        holes[1]
    );
    assert!(
        parts
            .iter()
            .any(|p| matches!(p, FPart::Text(t) if t == " {{c}}")),
        "{parts:?}"
    );
    // **And the emitter builds the format string from them** (D5).
    let emitted = emit(
        "fn g(s: ref String) -> String { return s.clone() }\n\
         fn main() { let n = 3 println(f\"a {n:>4} b {g(\"x\")} {{c}}\") }",
    );
    assert!(emitted.contains(r#"a {:>4} b {} {{c}}"#), "{emitted}");
}

/// **A broken hole is the parser's**, said where it is (ADR-262 D3, D4): a
/// quote the hole escaped, a `{` the string ends inside of, and a `}` with
/// no `{`.
#[test]
fn a_broken_hole_is_refused_by_the_parser() {
    let said = |source: &str| {
        let refused = nikaia::parser::parse_to_ast(source).expect_err("a parse error");
        let finding = nikaia::diagnostics::refused_finding(&refused).expect("a finding");
        nikaia::diagnostics::render_finding(finding, "app.nika", source)
    };
    let escaped = said("fn main() { println(f\"{g(\\\"a\\\")}\") }");
    assert!(escaped.contains("plain quotes"), "{escaped}");
    let unclosed = said("fn main() { let n = 1 println(f\"x {n\") }");
    assert!(unclosed.contains("is never closed"), "{unclosed}");
    let stray = said("fn main() { println(f\"x } y\") }");
    assert!(stray.contains("has no `{` before it"), "{stray}");
    // The caret on the `}` - with text after it on the line, a space found
    // there once sent it to the line's first column.
    assert!(stray.contains("app.nika:1:25"), "{stray}");
}
