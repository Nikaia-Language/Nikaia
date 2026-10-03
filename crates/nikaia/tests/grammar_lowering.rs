//! The lowering, checked against its own output - and by running it.
//!
//! The two files under `fixtures/` ending in `_expected.rs` are what the
//! emitter produces for the `.nika` files beside them. This test does two
//! things with them that a snapshot alone cannot: it `include!`s them, so the
//! parser backend has to accept every line, and it drives the code they
//! generate.
//!
//! Each generated file goes in a module of its own, because each is a whole
//! compilation unit and brings its own imports.

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

const FIXTURE: &str = include_str!("fixtures/measurements.nika");
const EXPECTED: &str = include_str!("fixtures/measurements_expected.rs");
const DIGITS: &str = include_str!("fixtures/digits.nika");

fn emit(source: &str, build: Build) -> String {
    let parsed = parse_to_ast(source).expect("the fixture parses");
    emit_program(&parsed, build)
        .expect("the fixture lowers")
        .rust
}

// --- What the emitter produced, compiled ---

mod measurements {
    //! The names `measurements.nika` uses without declaring live here: the
    //! grammar module is generated with `use super::*`, so they resolve where
    //! the grammar is placed.

    include!("fixtures/measurements_expected.rs");

    /// `std::text`, as the fixture reaches it: the source writes
    /// `text::digit_value` since [ADR-313](../../../docs/specification/adr/adr-313.md)
    /// put a name that lives in a module behind its prefix, so the stub is a
    /// module of that name rather than a bare function.
    pub mod text {
        pub fn digit_value(c: char) -> i32 {
            c as i32 - '0' as i32
        }
    }

    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct Summary {
        pub count: i64,
        pub tenths: i64,
        /// Only here to read the view: `NAME` is a slice of the input, and a
        /// summary that never touched it would not notice if it were wrong.
        pub name_bytes: i64,
    }

    impl Summary {
        fn new() -> Summary {
            Summary::default()
        }

        fn record(self, m: Reading<'_>) -> Summary {
            Summary {
                count: self.count + 1,
                tenths: self.tenths + i64::from(m.temp),
                name_bytes: self.name_bytes + m.name.len() as i64,
            }
        }

        fn merge(a: Summary, b: Summary) -> Summary {
            Summary {
                count: a.count + b.count,
                tenths: a.tenths + b.tenths,
                name_bytes: a.name_bytes + b.name_bytes,
            }
        }
    }
}

mod digits {
    include!("fixtures/digits_expected.rs");

    /// What `Digits.pair(data)` lowers to, written out so that the compiler
    /// has to accept it and a test can run it.
    /// `a_sequential_entry_rule_gets_no_piece_driver` is what keeps this copy
    /// and the emitter's output the same code.
    // `&*` is redundant for a `&str` and necessary for everything else the
    // emitter has to accept there - a mapping, an owned string - so the copy
    // keeps it and clippy is told why.
    #[allow(clippy::borrow_deref_ref)]
    pub fn sequential_driver(data: &str) -> Result<Pair, String> {
        // The emitted form is this call followed by `?`, in a `throws`
        // function; here it is the return value, with the error as its text.
        nikaia_std::grammar::parse(&*data, Digits::parse_pair()).map_err(|error| error.to_string())
    }
}

// --- The emitted text ---

#[test]
fn the_emitted_rust_is_the_file_this_test_compiles() {
    assert_eq!(
        emit(FIXTURE, Build::default()),
        EXPECTED,
        "the emitter and fixtures/measurements_expected.rs have drifted apart; \
         regenerate it with `cargo run -p nikaia --example dump -- \
         crates/nikaia/tests/fixtures/measurements.nika`"
    );
}

#[test]
fn a_frame_becomes_the_backend_attribute() {
    let emitted = emit(FIXTURE, Build::default());
    // ADR-296 D9: keyed, and the boundary keeps its escape - the backend is
    // handed the same "\n" the source wrote.
    assert!(
        emitted.contains(r#"#[frame(boundary = "\n")]"#),
        "no frame attribute in:\n{emitted}"
    );
}

#[test]
fn a_view_takes_the_input_lifetime_and_the_struct_with_it() {
    let emitted = emit(FIXTURE, Build::default());
    // ADR-283: `&str` is a view marker. The lifetime is the emitter's, never
    // the source's - and a struct that holds one is tied to the input too.
    assert!(emitted.contains("rule NAME -> &'a str ="), "{emitted}");
    assert!(
        emitted.contains("rule MEASUREMENT -> Reading<'a> ="),
        "{emitted}"
    );
    assert!(emitted.contains("pub struct Reading<'a> {"), "{emitted}");
    // The struct is `pub` and its fields are not, because the source says so:
    // Part I 9.2 makes them two questions, and 9.3's point is that a public
    // type keeping its parts to itself is the ordinary case. The emitter used
    // to write `pub` on every field regardless, which nothing could tell apart
    // until a program had more than one file.
    assert!(emitted.contains("    name: &'a str,"), "{emitted}");
    assert!(!emitted.contains("pub name: &'a str,"), "{emitted}");
}

#[test]
fn a_bare_par_fold_gets_the_binding_its_rule_needs() {
    let emitted = emit(FIXTURE, Build::default());
    // A `par_fold` is the whole body of its rule (ADR-296 D10), so there is
    // nothing for an action to add - and the emitter supplies the one the
    // backend wants rather than making the user write it.
    assert!(
        emitted.contains(
            "folded:par_fold(MEASUREMENT, Summary::new, |acc, m| { acc.record(m) }, Summary::merge)"
        ),
        "{emitted}"
    );
    assert!(emitted.contains("-> { folded }"), "{emitted}");
}

#[test]
fn operators_keep_their_meaning_and_lose_their_noise() {
    // Nikaia and Rust bind these the same way, so the emitter parenthesises
    // only where the source's own grouping demands it.
    let source = r#"
fn arithmetic() {
    let flat = a * 10 + b
    let grouped = (a + b) * c
    let right = a - (b - c)
    let mixed = a + b < c && d
}
"#;
    let emitted = emit(source, Build::default());

    assert!(emitted.contains("let flat = a * 10 + b;"), "{emitted}");
    assert!(emitted.contains("let grouped = (a + b) * c;"), "{emitted}");
    assert!(emitted.contains("let right = a - (b - c);"), "{emitted}");
    assert!(emitted.contains("let mixed = a + b < c && d;"), "{emitted}");
}

// --- The driver ---

const WITH_DSL: &str = r#"
grammar Measurements {
    @frame(boundary: "\n")
    rule MEASUREMENT -> i32 = t:i32 frame_end { t }

    pub rule file -> i64 =
        par_fold(MEASUREMENT, zero, fn(acc, m) { acc + m }, add)
}

fn summarize() {
    let totals = Measurements::file(data)
}
"#;

#[test]
fn user_parallelism_chooses_the_parallelism_and_nothing_else() {
    // ADR-296 D10 and ADR-037 D2: parallelism is not a dialect. The same source
    // compiles at either setting; at `0` the driver is told not to cut, which
    // is what "degrades to a sequential fold" means in code.
    let parallel = emit(WITH_DSL, Build::parallel());
    let sequential = emit(WITH_DSL, Build::default());

    assert!(
        parallel.contains(
            "nikaia_std::grammar::parse_pieces(&*data, |source| Measurements::parse_file_pieces(source, &ParseContext::<()>::default(), Parallelism::Auto))"
        ),
        "{parallel}"
    );
    assert!(
        sequential.contains(
            "nikaia_std::grammar::parse_pieces(&*data, |source| Measurements::parse_file_pieces(source, &ParseContext::<()>::default(), Parallelism::Off))"
        ),
        "{sequential}"
    );

    // Only the executor differs.
    assert_eq!(
        parallel.replace("Parallelism::Auto", "Parallelism::Off"),
        sequential
    );
}

/// ADR-037 D2: a target without threads pins the driver to `Off` however
/// `user_parallelism` was set - a switch bounds what may run at once, it
/// cannot conjure a thread the machine does not have.
#[test]
fn a_target_without_threads_pins_the_driver_sequential() {
    let asked_for_parallel = Build {
        target: nikaia::emit::Target::Wasm32Unknown,
        user_parallelism: nikaia::emit::UserParallelism::Yes,
        ..Build::default()
    };
    let emitted = emit(WITH_DSL, asked_for_parallel);

    assert!(emitted.contains("Parallelism::Off"), "{emitted}");
    assert!(!emitted.contains("Parallelism::Auto"), "{emitted}");
}

#[test]
fn a_sequential_entry_rule_gets_no_piece_driver() {
    // Without a `par_fold` there is nothing to cut and nothing to merge, so the
    // lowering drives the rule's own parser over the whole input (ADR-296 D19).
    let source = format!("{DIGITS}\n\nfn read() {{\n    let p = Digits::pair(data)\n}}\n");
    let emitted = emit(&source, Build::default());

    assert!(!emitted.contains("_pieces("), "{emitted}");
    assert!(
        emitted.contains("nikaia_std::grammar::parse(&*data, Digits::parse_pair())?"),
        "the emitted driver and the one `digits::sequential_driver` compiles \
         have drifted:\n{emitted}"
    );
}

/// `dsl … from …` propagates its failure on its own, and a `catch` beside it
/// is the one place that wants the `Result` instead. Emitting the `?` there
/// too produced `match <value> { Ok(..) => .., Err(..) => .. }` - a handler
/// matching on something already unwrapped, which is not a message about a
/// program but a compiler bug (Kap 7.1).
#[test]
fn a_dsl_with_a_catch_is_handed_the_result_and_not_the_value() {
    let source = format!(
        "{DIGITS}\n\nfn read() throws {{\n    \
         let p = Digits::pair(data) catch {{\n        return\n    }}\n}}\n"
    );
    let emitted = emit(&source, Build::default());

    assert!(
        emitted.contains("Ok(value) => value"),
        "the catch should match on the parse:\n{emitted}"
    );
    assert!(
        !emitted.contains("}? {"),
        "the catch is matching on an unwrapped value:\n{emitted}"
    );
}

/// The backend's error knows an offset and not the text it came from, so only
/// the driver can turn one into a line and a column - and it is the only place
/// that has both. Without this a program that rejects a file says *what* was
/// expected and never *where* (ADR-296 D11, `docs/error-corpus.md`).
///
/// **And what comes out of it has a name**
/// ([ADR-296](../../../docs/specification/adr/adr-296.md) D38): the rendered
/// text goes into a `ParseError`, which is the type the rule's ledger entry
/// says it throws. The text is unchanged, so a program that prints the error
/// prints what it printed before.
#[test]
fn a_rejected_parse_is_rendered_against_the_input_it_parsed() {
    let emitted = emit(WITH_DSL, Build::default());
    assert!(
        emitted.contains("nikaia_std::grammar::parse_pieces(&*data, |source| "),
        "{emitted}"
    );
    // `std` renders it against the text it was handed (0.0.297): the line and
    // the column, not an offset.
    let refused = digits::sequential_driver("12;30").expect_err("a `;` is not a `,`");
    assert!(
        refused.contains("1:3") || refused.contains("line 1"),
        "{refused}"
    );
}

/// `# "…"` between a rule's return type and its `=`: what the rule is called
/// when it fails where it began. The backend's own spelling, because the
/// lowering is name for name (ADR-296 D17) and a second spelling for the same
/// thing would be one more thing to know.
#[test]
fn a_rule_label_is_lowered_where_the_backend_expects_it() {
    let source = concat!(
        "grammar G {\n",
        "    rule atom -> i32 # \"expression\" =\n",
        "        n:digit+ { 1 }\n",
        "      | \"(\" e:atom \")\" { e }\n",
        "}\n"
    );
    let emitted = emit(source, Build::default());
    assert!(
        emitted.contains("rule atom -> i32 # \"expression\" ="),
        "{emitted}"
    );
}

/// A rule without one is emitted exactly as it was.
#[test]
fn a_rule_without_a_label_gains_nothing() {
    let source = "grammar G {\n    rule atom -> i32 = n:digit+ { 1 }\n}\n";
    let emitted = emit(source, Build::default());
    assert!(emitted.contains("rule atom -> i32 ="), "{emitted}");
    // The preamble's `#[allow(unused_imports)]` is not part of the grammar, and
    // it is there in every file now - so the claim is about the rule's own line.
    let rule = emitted
        .lines()
        .find(|line| line.contains("rule atom"))
        .expect("the rule is emitted");
    assert!(!rule.contains('#'), "{emitted}");
}

/// `(A, B)` as a type, `(a, b)` as a value, `t.0` to read a part - Part I 4.5.
///
/// The parts of a tuple type live where a named type's arguments live, which is
/// what lets the view analysis reach them: a `(&str, i64)` in a struct field
/// ties that struct to the input exactly as a bare `&str` would.
#[test]
fn a_tuple_is_a_type_a_value_and_a_field_access() {
    let source = concat!(
        "fn pair() -> (i64, i64) {\n",
        "    let p = (1, 2)\n",
        "    return (p.0, p.1)\n",
        "}\n"
    );
    let emitted = emit(source, Build::default());
    assert!(emitted.contains("fn pair() -> (i64, i64)"), "{emitted}");
    assert!(emitted.contains("let p = (1, 2);"), "{emitted}");
    // A trailing `return` is the block's value, so it comes out unwrapped.
    assert!(emitted.contains("(p.0, p.1)"), "{emitted}");
}

/// A tuple in a struct field carries the input lifetime the same way a bare
/// view does - the analysis walks the parts because they sit where arguments
/// sit (ADR-283, and `a_view_takes_the_input_lifetime_and_the_struct_with_it`).
#[test]
fn a_tuple_of_views_ties_its_struct_to_the_input() {
    let source = concat!(
        "pub struct Pair {\n",
        "    both: (ref String, i64),\n",
        "}\n"
    );
    let emitted = emit(source, Build::default());
    assert!(
        emitted.contains("pub struct Pair<'a>") && emitted.contains("(&'a str, i64)"),
        "{emitted}"
    );
}

/// `(a)` is still a parenthesised expression, not a one-part tuple: the comma
/// is what makes a tuple, in Nikaia as in the language it lowers to.
#[test]
fn parentheses_without_a_comma_are_still_grouping() {
    let source = "fn f() -> i64 {\n    return (1 + 2) * 3\n}\n";
    let emitted = emit(source, Build::default());
    assert!(emitted.contains("(1 + 2) * 3"), "{emitted}");
    assert!(!emitted.contains("((1 + 2))"), "{emitted}");
}

/// **A lambda that names no arguments takes none, whatever its body says**
/// ([ADR-277](../../../docs/specification/adr/adr-277.md) D2).
///
/// This used to be the opposite test. A lambda's arity was read off which of `a`,
/// `b`, `c` its body mentioned, and a string's holes were the one place a body
/// could mention one without the AST showing it - so `fn { f"{a.0}" }` was
/// generated with a parameter. Nothing is read off a body now, and the name in
/// the hole is a name nothing declares.
#[test]
fn a_lambda_that_names_nothing_takes_nothing() {
    let source =
        "fn f(xs: List) -> String {\n    return xs.map fn { f\"{a.0}={a.1}\" }.join(\",\")\n}\n";
    let emitted = emit(source, Build::default());
    assert!(emitted.contains("map(||"), "{emitted}");
    assert!(!emitted.contains("map(|a|"), "{emitted}");

    // …and the name in the hole is refused, which is what makes the emitted
    // shape unreachable rather than merely different (`NK1117`).
    let parsed = nikaia::parser::parse_to_ast(source).expect("it parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std's ledger parses");
    let found = nikaia::check::check(&parsed, &own, &library).findings;
    assert!(
        found.iter().any(|f| f.code == "NK1117"),
        "the hole names `a`, and nothing declares it: {found:#?}"
    );
}

/// Kap 4.4: the three shapes a variant can have, and no others.
#[test]
fn an_enum_lowers_its_three_variant_shapes() {
    let source = concat!(
        "pub enum Message {\n",
        "    Quit,\n",
        "    Write(String),\n",
        "    Move { x: i32, y: i32 },\n",
        "}\n"
    );
    let emitted = emit(source, Build::default());
    assert!(emitted.contains("pub enum Message {"), "{emitted}");
    assert!(emitted.contains("    Quit,"), "{emitted}");
    assert!(emitted.contains("    Write(String),"), "{emitted}");
    assert!(
        emitted.contains("    Move { x: i32, y: i32 },"),
        "{emitted}"
    );
}

/// An enum holds views the same way a struct does, so it takes the input
/// lifetime the same way (ADR-283, and `borrowing_structs`).
#[test]
fn an_enum_that_carries_a_view_takes_the_input_lifetime() {
    let source = "enum Token {\n    End,\n    Word(ref String),\n}\n";
    let emitted = emit(source, Build::default());
    assert!(emitted.contains("enum Token<'a>"), "{emitted}");
    assert!(emitted.contains("Word(&'a str)"), "{emitted}");
}

/// Kap 3.4: every pattern shape, and each is the shape the language below
/// spells the same way - a transcription rather than a translation.
#[test]
fn a_match_lowers_every_pattern_shape() {
    let source = concat!(
        "fn describe(m: Message) -> i32 {\n",
        "    return match m {\n",
        "        Message::Quit => 0,\n",
        "        Message::Write(text) => 1,\n",
        "        Message::Move { x, y } => 2,\n",
        "        7 => 3,\n",
        "        other => 4,\n",
        "        else => 5,\n",
        "    }\n",
        "}\n"
    );
    let emitted = emit(source, Build::default());
    for arm in [
        "Message::Quit => 0,",
        "Message::Write(text) => 1,",
        "Message::Move { x, y } => 2,",
        "7 => 3,",
        "other => 4,",
        // **The one arm whose spelling differs below**: the source writes
        // `else` ([ADR-291](../../../docs/specification/adr/adr-291.md) D16) and
        // the language below has `_`, which is what the arm was written with
        // here before.
        "_ => 5,",
    ] {
        assert!(emitted.contains(arm), "missing `{arm}`:\n{emitted}");
    }
}

/// `match value {` must not read `value { … }` as a struct literal - the same
/// trap `if`'s condition has, and the same answer.
#[test]
fn a_match_value_is_not_read_as_a_struct_literal() {
    let source = "fn f(v: i32) -> i32 {\n    return match v {\n        else => 1,\n    }\n}\n";
    let emitted = emit(source, Build::default());
    assert!(emitted.contains("match v {"), "{emitted}");
}

// --- Running what was generated ---

const MEASUREMENTS: &str = "Hamburg;12.0\nAbha;-23.0\nSaint-Pierre;9.1\nHamburg;-0.4\n";

#[test]
fn the_generated_parser_gives_the_same_answer_however_it_is_cut() {
    use measurements::{Measurements, Summary};
    use winnow_grammar::ParseContext;
    use winnow_grammar::rt::Parallelism;

    let expected = Summary {
        count: 4,
        tenths: 120 - 230 + 91 - 4,
        // "Hamburg", "Abha", "Saint-Pierre", "Hamburg" - the station names are
        // slices of the input, and a cut in the wrong place would show here.
        name_bytes: 7 + 4 + 12 + 7,
    };

    for how in [
        Parallelism::Off,
        Parallelism::Pieces(1),
        Parallelism::Pieces(3),
        Parallelism::Pieces(64),
        Parallelism::Auto,
    ] {
        let got =
            Measurements::parse_file_pieces(MEASUREMENTS, &ParseContext::<()>::default(), how)
                .unwrap_or_else(|e| panic!("{how:?}: {}", e.render(MEASUREMENTS)));
        assert_eq!(got, expected, "{how:?}");
    }
}

#[test]
fn a_frame_that_does_not_parse_is_rejected_whatever_the_cut() {
    use measurements::Measurements;
    use winnow_grammar::ParseContext;
    use winnow_grammar::rt::Parallelism;

    let broken = "Hamburg;12.0\nAbha;not-a-temperature\n";

    for how in [Parallelism::Off, Parallelism::Pieces(2), Parallelism::Auto] {
        assert!(
            Measurements::parse_file_pieces(broken, &ParseContext::<()>::default(), how).is_err(),
            "{how:?} accepted a broken frame"
        );
    }
}

#[test]
fn the_sequential_driver_runs() {
    let pair = digits::sequential_driver("12,30").expect("parses");
    assert_eq!((pair.left, pair.right), (12, 30));
    assert!(digits::sequential_driver("12;30").is_err());
}

// --- The example this exists for ---

/// The `grammar` item and the struct it builds, taken out of the real
/// `examples/1brc.nika` rather than copied - a copy would drift, and the point
/// of this test is that the example itself lowers.
fn one_brc_grammar_half() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/1brc.nika");
    let source = std::fs::read_to_string(&path).expect("examples/1brc.nika");

    let mut out = String::new();
    let mut inside = false;
    for line in source.lines() {
        if line.starts_with("grammar Measurements {") || line.starts_with("pub struct Reading") {
            inside = true;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
        if inside && line == "}" {
            inside = false;
        }
    }

    assert!(
        out.contains("par_fold(MEASUREMENT"),
        "the extraction missed the grammar:\n{out}"
    );
    out
}

#[test]
fn the_grammar_half_of_the_1brc_example_lowers() {
    // ADR-296: the file as a whole does not compile yet - its `impl` blocks,
    // `throws`/`catch` and string interpolation are not lowered. Its grammar
    // does, and that is the half this work was about.
    let emitted = emit(&one_brc_grammar_half(), Build::default());

    assert!(
        emitted.contains(r#"#[frame(boundary = "\n")]"#),
        "{emitted}"
    );
    assert!(
        emitted.contains("rule MEASUREMENT -> Reading<'a> ="),
        "{emitted}"
    );
    // **`Summary` and not `Summary::new`**, and the difference is the slice
    // rather than the language ([ADR-140](../../../docs/specification/adr/adr-140.md)
    // D2). A type is constructed by its anonymous constructor and the emitter
    // writes the key back — but only for a type this unit **declares**, and the
    // extraction above takes the grammar and leaves `struct Summary` behind. A
    // name nothing describes is left alone, which is
    // [Part III C.4](../../../docs/specification/30-nikaia-tooling.md)'s rule
    // and is what lets a `.nika` file name a Rust-side item at all.
    assert!(
        emitted
            .contains("par_fold(MEASUREMENT, Summary, |acc, m| { acc.record(m) }, Summary::merge)"),
        "{emitted}"
    );
    // And the **whole** file, where the `struct` is in the unit, writes the key.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/1brc.nika");
    let source = std::fs::read_to_string(&path).expect("examples/1brc.nika");
    let parsed = parse_to_ast(&source).expect("1brc parses");
    let whole = emit_program(&parsed, Build::default())
        .expect("1brc lowers")
        .rust;
    assert!(
        whole.contains("par_fold(MEASUREMENT, Summary::new,"),
        "the constructor is the key where the type is declared"
    );
    assert!(emitted.contains("s:until(\";\" | frame_end)"), "{emitted}");
    assert!(emitted.contains("whole:digit{1,2}"), "{emitted}");
}

// --- Type arguments to a built-in ---

#[test]
fn a_type_argument_is_written_with_brackets_and_emitted_with_angles() {
    // Kap 4.3: Nikaia spells a type argument with brackets everywhere, which
    // is what keeps `<` free elsewhere in the language. The backend's
    // built-ins take theirs between angles, and the emitter is the one place
    // the two meet - the source never writes the backend's spelling.
    let source = r#"
grammar Ids {
    rule N -> i32 = n:dec[i32](digit{1,2}) { n }
    rule T -> ref String = t:text(alpha1 digit*) { t }
    pub rule entry -> i32 = n:N { n }
}
"#;
    let emitted = emit(source, Build::default());

    assert!(emitted.contains("n:dec<i32>(digit{1,2})"), "{emitted}");
    // A built-in without type arguments keeps its call exactly as written.
    assert!(emitted.contains("t:text(alpha1 digit*)"), "{emitted}");
}

/// **A grammar with two `pub` rules is entered by either**
/// ([ADR-296](../../../docs/specification/adr/adr-296.md) D25), which is the
/// defect that record closes rather than a feature it adds.
///
/// The emitter used to pick the entry itself — the first `pub` rule, and a
/// `par_fold` one ahead of an earlier one — so a grammar with two of them got
/// one of them by source order, silently. Here `both` stands first and is a
/// `par_fold`, so the old choice would have taken it whichever was asked for.
#[test]
fn a_grammar_with_two_public_rules_is_entered_by_either() {
    let source = r#"
grammar Two {
    @frame(boundary: "\n")
    rule M -> i32 = t:i32 frame_end { t }
    rule N -> i64 = d:dec[i64](digit+) { d }

    pub rule both -> i64 = par_fold(M, zero, fn(acc, m) { acc + m }, add)
    pub rule one -> i64 = n:N { n }
}

fn zero() -> i64 { return 0 }
fn add(a: i64, b: i64) -> i64 { return a + b }

fn pieces() { let p = Two::both(data) }
fn single() { let s = Two::one(data) }
"#;
    let emitted = emit(source, Build::default());

    // The parallel rule gets the piece driver…
    assert!(emitted.contains("Two::parse_both_pieces("), "{emitted}");
    // …and the sequential one does not, in the same program.
    assert!(emitted.contains("Two::parse_one()"), "{emitted}");
    assert!(!emitted.contains("parse_one_pieces("), "{emitted}");
}

/// **The dot is refused, and the message names the `::`**
/// ([ADR-140](../../../docs/specification/adr/adr-140.md) D3).
///
/// In the **checker**, because only this side knows the receiver names a
/// grammar: `Nums.number(text)` and `text.number(x)` are the same five tokens,
/// and a grammar name is not a value the parser can tell apart from one.
#[test]
fn a_rule_reached_through_a_dot_is_refused() {
    let source = "grammar Nums {\n\
                  \x20   pub rule number -> i64 = d:dec[i64](digit+) { d }\n\
                  }\n\
                  \n\
                  fn read(text: ref String) { let n = Nums.number(text) catch { 0 } }\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    let found: Vec<_> = nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.code == "NK1147")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    let help = found[0].help.as_deref().expect("a way out");
    assert!(help.contains("Nums::number(…)"), "{help}");
}

/// **A method call on an ordinary value is untouched**, which is what the
/// refusal has to leave alone: it asks whether the receiver *is* a grammar of
/// this file and whether the name *is* one of its rules, so a value that
/// happens to share a spelling is not this.
#[test]
fn a_method_on_a_value_is_not_a_grammar_entry() {
    let source = "grammar Nums {\n\
                  \x20   pub rule number -> i64 = d:dec[i64](digit+) { d }\n\
                  }\n\
                  \n\
                  fn read(text: String) -> i64 { return text.len() }\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    assert!(
        nikaia::check::check(&parsed, &own, &library)
            .findings
            .iter()
            .all(|f| f.code != "NK1147")
    );
}

/// **A rule that is not `pub` is not an entry** (D2), and the message says which
/// of the two it is.
///
/// A grammar name can only stand where a callee stands, so `Two::N(data)` is not
/// a path that happens to miss — it is an entry naming a rule that is there and
/// is private. Saying *there is no such rule* about one written three
/// lines up is the message a reader cannot act on.
#[test]
fn a_private_rule_is_not_an_entry() {
    let source = r#"
grammar Two {
    rule N -> i64 = d:dec[i64](digit+) { d }
    pub rule one -> i64 = n:N { n }
}

fn reach() { let s = Two::N(data) }
"#;
    let parsed = parse_to_ast(source).expect("the source parses");
    let Err(error) = emit_program(&parsed, Build::default()) else {
        panic!("a private rule was entered");
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("`N` in grammar `Two` isn't `pub`"),
        "{message}"
    );
    assert!(message.contains("Write `pub rule N`."), "{message}");
}

/// **The old spelling is refused, and the message names the new one**
/// ([ADR-296](../../../docs/specification/adr/adr-296.md) D24) — the shape
/// [ADR-277](../../../docs/specification/adr/adr-277.md) gave `fn:`: a form the
/// specification taught deserves a sentence rather than a parse error at
/// whatever token happens to come next.
#[test]
fn the_old_from_form_is_refused_with_the_call_in_the_message() {
    let source = "grammar Nums {\n\
                  \x20   pub rule number -> i64 = d:dec[i64](digit+) { d }\n\
                  }\n\
                  \n\
                  fn read() { let n = dsl Nums from text }\n";
    let Err(error) = parse_to_ast(source) else {
        panic!("`dsl X from e` was removed and still parses");
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("`dsl X from e` is no longer supported."),
        "{message}"
    );
    // The headline says what was removed; the help it carries names the call
    // that replaced it - as a path, because the dot it named until 0.0.267 is
    // `NK1147` (Part II 10.2, ADR-140 D3), a help that led to a refusal.
    let finding = nikaia::diagnostics::refused_finding(&error).expect("a parse refusal");
    assert!(
        finding
            .help
            .as_deref()
            .is_some_and(|h| h.contains("X::rule(e)") && !h.contains("X.rule")),
        "{finding:#?}"
    );
}
