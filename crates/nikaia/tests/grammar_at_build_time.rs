//! **A grammar runs while the program is built, by compiling the parser it
//! generates** — issue #178,
//! Part II 10.2 A.
//!
//! **It is not interpreted, and that is the whole entry.** `winnow-grammar` is
//! a code generator with no interpreter in it, so walking the grammar tree here
//! would be a *second implementation of the same semantics* — and Part II
//! 10.2's promise that one grammar means the same thing at both stages would
//! stop being a property and become a hope. The disagreements would land in the
//! corners (implicit whitespace, repetition bounds, the commit point, frames,
//! interning, spans) and would present as *this file parsed while the program
//! was built and fails while it runs*, for the same file and the same grammar.
//!
//! So the tests here **run programs**. A test that compared the emitted `const`
//! against a string would pass for a parser that read the file wrongly, and a
//! test that asked the checker would pass for one that never ran at all.
//!
//! **They are slow on purpose**: each one compiles a small Cargo project. That
//! is the cost [ADR-026](../../../docs/specification/adr/adr-026.md) Q4 named,
//! and it is paid once per grammar rather than once per build.

mod common;

use nikaia::assets::Reads;
use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A scratch directory with somewhere to compile a parser.
fn workshop(purpose: &str) -> (PathBuf, Reads) {
    let dir = common::scratch_dir(purpose);
    let reads = Reads::at(&dir).building_in(dir.join("build-time"));
    (dir, reads)
}

fn findings_in(source: &str, reads: &Reads) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_against(
        &parsed,
        &[],
        &own,
        &library,
        &BTreeSet::new(),
        &check::Newly::new(),
        reads,
    )
    .findings
}

fn one(source: &str, reads: &Reads) -> Finding {
    let mut found = findings_in(source, reads);
    assert_eq!(found.len(), 1, "{found:#?}");
    found.remove(0)
}

/// Lowers and runs, which is the only way to see that the parse happened and
/// landed in the program.
fn run(dir: &Path, reads: &Reads, source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = nikaia::emit::emit_program_reading(&parsed, Default::default(), reads)
        .expect("it lowers")
        .rust;
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(&binary)
        .current_dir(dir)
        .output()
        .expect("run it");
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    String::from_utf8_lossy(&ran.stdout).to_string()
}

/// A grammar whose result **crosses**: a list of structs whose fields are text.
const SETTINGS: &str = "pub struct Setting {\n\
     \x20   key: ref String,\n\
     \x20   value: ref String,\n\
     }\n\
     \n\
     grammar Cfg {\n\
     \x20   rule WSE = multispace1 { }\n\
     \x20   rule WS = (WSE | COMMENT)* { }\n\
     \x20   rule COMMENT = \"#\" until(line_ending) { }\n\
     \x20   rule NAME -> ref String = s:raw_ident { s }\n\
     \x20   rule VALUE -> ref String = s:until(\"#\" | line_ending) { s.trim() }\n\
     \x20   rule setting -> Setting = key:NAME \"=\" value:VALUE { Setting { key, value } }\n\
     \x20   pub rule file -> Vec[Setting] = settings:setting* { settings }\n\
     }\n";

/// **The parse happens while the program is built, and what is left is the
/// answer** (Part II 10.2 A: *the result is embedded in the binary at no
/// runtime cost*).
///
/// The program below has no parser in it: `SETTINGS` is a `const` array of
/// structs, and the grammar's own action — `s.trim()` on the value — has
/// already run. `port = 8080  # the usual one` is `8080`, which is the thing a
/// test against the checker could not see.
#[test]
fn a_grammar_runs_while_the_program_is_built_and_the_answer_is_a_const() {
    let (dir, reads) = workshop("grammar-at-build");
    let source = format!(
        "{SETTINGS}\n\
         comptime SETTINGS: Array[Setting, 2] = \
         Cfg::file(\"host = example.com\\nport = 8080  # the usual one\\n\")\n\
         \n\
         fn main() {{\n\
         \x20   for s in SETTINGS {{\n\
         \x20       println(f\"{{s.key}}={{s.value}}\")\n\
         \x20   }}\n\
         }}"
    );
    assert!(
        findings_in(&source, &reads).is_empty(),
        "{:#?}",
        findings_in(&source, &reads)
    );

    let parsed = parse_to_ast(&source).expect("parses");
    let rust = nikaia::emit::emit_program_reading(&parsed, Default::default(), &reads)
        .expect("lowers")
        .rust;
    assert!(
        rust.contains(
            "const SETTINGS: [Setting; 2] = [Setting { key: \"host\", value: \"example.com\" }, \
             Setting { key: \"port\", value: \"8080\" }];"
        ),
        "{rust}"
    );
    // And nothing of the parse is left to happen at run time.
    assert!(!rust.contains("parse_file()"), "{rust}");

    assert_eq!(run(&dir, &reads, &source), "host=example.com\nport=8080\n");
    std::fs::remove_dir_all(&dir).ok();
}

/// **Invalid input fails the build**, in the parser's own words and against the
/// bytes that were parsed (Part II 10.2 A).
///
/// The diagnostic is **relayed whole** rather than re-worded: it counts its
/// line and column against the input, which is the one place they mean
/// anything.
#[test]
fn invalid_input_fails_the_build_in_the_parsers_own_words() {
    let (dir, reads) = workshop("grammar-bad-input");
    let source = format!(
        "{SETTINGS}\n\
         comptime SETTINGS: Array[Setting, 1] = Cfg::file(\"host = ok\\n!!! broken\\n\")\n\
         \n\
         fn main() {{ println(f\"{{SETTINGS.len()}}\") }}"
    );
    let found = one(&source, &reads);
    assert_eq!(found.code, "NK1178");
    assert!(
        found
            .message
            .contains("`Cfg::file` couldn't parse the input it was given"),
        "{}",
        found.message
    );
    assert!(
        found.notes[0].contains("!!! broken") && found.notes[0].contains("line 2"),
        "the parser's own words, against the input: {:#?}",
        found.notes
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A grammar whose rule hands back an **`enum`**, with the four shapes a
/// variant can be: no payload, text, a float, and two things at once.
const SHADES: &str = "enum Shade { Odd, Even, Named(ref String), Weight(f64) }\n\
     \n\
     grammar Pick {\n\
     \x20   rule WSE = multispace1 { }\n\
     \x20   rule WS = WSE* { }\n\
     \x20   rule NUM -> f64 = n:dec[f64](text(digit+ (\".\" digit+)?)) { n }\n\
     \x20   rule ONE -> Shade = \"odd\" { Shade::Odd }\n\
     \x20   rule TWO -> Shade = \"even\" { Shade::Even }\n\
     \x20   rule THREE -> Shade = \"n:\" s:raw_ident { Shade::Named(s) }\n\
     \x20   rule FOUR -> Shade = \"w:\" n:NUM { Shade::Weight(n) }\n\
     \x20   rule SHADE -> Shade = s:(ONE | TWO | THREE | FOUR) { s }\n\
     \x20   pub rule many -> Vec[Shade] = shades:SHADE* { shades }\n\
     }\n";

/// **An `enum` crosses, by the variant the value *is*.**
///
/// This was refused by name until 0.0.125, and the sentence it was refused
/// with was true of this compiler rather than of the language below: Rust
/// holds a `const S: Shade = Shade::Odd` perfectly well. What was missing was
/// a build-time value with a variant in it
/// ([ADR-079](../../../docs/specification/adr/adr-079.md) D1's set), and with
/// one the dump is a `match` the generator writes an arm per variant of.
///
/// The test **runs** the program, because that is the only thing that says
/// the arm bound the payload the parser actually built: a `const` compared
/// against a string would pass for a dumper that wrote the first variant every
/// time.
#[test]
fn an_enum_crosses_from_a_grammar_by_the_variant_the_value_is() {
    let (dir, reads) = workshop("grammar-enum");
    let source = format!(
        "{SHADES}\n\
         comptime SHADES: Array[Shade, 4] = Pick::many(\"odd even n:blue w:2.5\")\n\
         \n\
         fn main() {{\n\
         \x20   for s in SHADES {{\n\
         \x20       match s {{\n\
         \x20           Shade::Odd => {{ println(\"odd\") }}\n\
         \x20           Shade::Even => {{ println(\"even\") }}\n\
         \x20           Shade::Named(n) => {{ println(f\"named {{n}}\") }}\n\
         \x20           Shade::Weight(w) => {{ println(f\"weight {{w}}\") }}\n\
         \x20       }}\n\
         \x20   }}\n\
         }}"
    );
    assert!(
        findings_in(&source, &reads).is_empty(),
        "{:#?}",
        findings_in(&source, &reads)
    );

    let parsed = parse_to_ast(&source).expect("parses");
    let rust = nikaia::emit::emit_program_reading(&parsed, Default::default(), &reads)
        .expect("lowers")
        .rust;
    assert!(
        rust.contains(
            "const SHADES: [Shade; 4] = [Shade::Odd, Shade::Even, Shade::Named(\"blue\"), \
             Shade::Weight(2.5)];"
        ),
        "{rust}"
    );

    assert_eq!(
        run(&dir, &reads, &source),
        "odd\neven\nnamed blue\nweight 2.5\n"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **And a float crosses as itself**, which was the other half of the same
/// absence.
///
/// The dump writes it with `{:?}` rather than `{}` — Rust's `Debug` for a
/// float is the shortest text that reads back as the same bits, and what this
/// build hands the program has to be the number the parser had, not a rounding
/// of it.
#[test]
fn a_float_crosses_from_a_grammar_as_the_bits_the_parser_had() {
    let (dir, reads) = workshop("grammar-float");
    let source = "grammar Num {\n\
         \x20   rule WS = multispace0 { }\n\
         \x20   pub rule one -> f64 = n:dec[f64](text(digit+ (\".\" digit+)?)) { n }\n\
         }\n\
         \n\
         comptime N: f64 = Num::one(\"0.1\")\n\
         \n\
         fn main() { println(f\"{N}\") }";
    assert!(
        findings_in(source, &reads).is_empty(),
        "{:#?}",
        findings_in(source, &reads)
    );
    assert_eq!(run(&dir, &reads, source), "0.1\n");
    std::fs::remove_dir_all(&dir).ok();
}

/// **A shape with no crossed form is still refused before a parser is
/// compiled**, and 0.0.125 moved where that line is rather than removing it.
///
/// A variant with **named** fields is the one an `enum` still cannot cross as:
/// a build-time value carries a variant's payload by position, and whether a
/// named-field variant should carry a field map is a question nobody has
/// answered. The sentence says *that* — a shape nobody has decided — rather
/// than something about `const`, which holds `Shape::Spot { x: 1 }` fine.
#[test]
fn a_variant_with_named_fields_is_refused_by_name() {
    let (dir, reads) = workshop("grammar-named-variant");
    let source = "enum Shape { Spot { x: i64 } }\n\
         \n\
         grammar Pick {\n\
         \x20   rule WS = multispace0 { }\n\
         \x20   pub rule one -> Shape = \"spot\" { Shape::Spot { x: 1 } }\n\
         }\n\
         \n\
         comptime CHOICE: Shape = Pick::one(\"spot\")\n\
         \n\
         fn main() { println(\"hi\") }";
    let found = one(source, &reads);
    assert_eq!(found.code, "NK1178");
    assert!(
        found
            .message
            .contains("`Pick::one` returns a `Shape`, which can't be computed at build time"),
        "{}",
        found.message
    );
    assert!(
        found.notes[0].contains("`Shape::Spot` has named fields")
            && found.notes[0]
                .contains("variants with named fields can't be computed at build time"),
        "{:#?}",
        found.notes
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **A build with nowhere to compile a parser says so**, which is every bare
/// `check` in a test and nothing a person runs.
#[test]
fn a_build_with_nowhere_to_build_says_so() {
    let source = format!(
        "{SETTINGS}\n\
         comptime SETTINGS: Array[Setting, 1] = Cfg::file(\"a = 1\\n\")\n\
         \n\
         fn main() {{ println(f\"{{SETTINGS.len()}}\") }}"
    );
    let found = one(&source, &Reads::none());
    assert_eq!(found.code, "NK1178");
    assert!(
        found.message.contains("nowhere to compile its parser"),
        "{}",
        found.message
    );
}

/// **The encoder is generated and the decoder is written by hand**, which is
/// two halves of one format — the shape [`nikaia::fixed`] already carries a
/// warning about. This is the cheap half of holding them together, and the
/// running tests above are the half that counts.
#[test]
fn the_decoder_reads_every_shape_the_dumper_writes() {
    use nikaia::build_time::Value;
    use nikaia::grammar_run::decode;

    assert_eq!(decode("(i 42)").expect("an integer"), Value::Int(42));
    assert_eq!(decode("(b true)").expect("a bool"), Value::Bool(true));
    assert_eq!(
        decode("(s \"a\\nb\")").expect("text"),
        Value::Text("a\nb".to_string())
    );
    assert_eq!(
        decode("(l (i 1) (i 2))").expect("a list"),
        Value::List(vec![Value::Int(1), Value::Int(2)])
    );
    assert_eq!(decode("(f 1.5)").expect("a float"), Value::Float(1.5));
    // **The variant and its type are two words**, and the one defect this
    // decoder had was reading the second from where the first ended: `(v Shade
    // Odd)` came back as a variant with no name, and the payload loop then met
    // an `O`. The shape below is the one the generated dumper writes.
    assert_eq!(
        decode("(v Shade Odd)").expect("a unit variant"),
        Value::Variant {
            ty: "Shade".to_string(),
            variant: "Odd".to_string(),
            payload: Vec::new(),
        }
    );
    assert_eq!(
        decode("(v Shape Pair (i 7) (s \"x\"))").expect("a variant with a payload"),
        Value::Variant {
            ty: "Shape".to_string(),
            variant: "Pair".to_string(),
            payload: vec![Value::Int(7), Value::Text("x".to_string())],
        }
    );
    let Value::Struct { name, fields } =
        decode("(t Setting (key (s \"a\")) (value (i 1)))").expect("a struct")
    else {
        panic!("a struct");
    };
    assert_eq!(name, "Setting");
    assert_eq!(fields["key"], Value::Text("a".to_string()));
    assert_eq!(fields["value"], Value::Int(1));

    // And a shape it does not know is said rather than guessed at.
    assert!(decode("(q 1)").is_err());
}

/// **A rule handing back a run crosses into a `&[T]`**
/// ([ADR-179](../../../docs/specification/adr/adr-179.md) D1, D3), which is the
/// crossing [ADR-177](../../../docs/specification/adr/adr-177.md) §5's
/// measurement said neither corpus grammar had.
///
/// The count is not in the type, so the same program takes a file with three
/// settings in it without the declaration changing — which is the whole of what
/// an `Array[Setting, N]` could not do.
///
/// **D3 is what makes it work at all**: a parser builds a `Vec`, so the
/// sub-program owns what the program views, and the dump — generated from the
/// **program's** declaration — reads a run either way.
#[test]
fn a_rule_that_hands_back_a_run_crosses_as_a_view() {
    let (dir, reads) = workshop("grammar-run-view");
    let source = format!(
        "{SETTINGS}\n\
         comptime SETTINGS: ref Array[Setting] = \
         Cfg::file(\"host = example.com\\nport = 8080\\nuser = ada\\n\")\n\
         \n\
         fn main() {{\n\
         \x20   println(f\"{{SETTINGS.len()}}\")\n\
         \x20   for s in SETTINGS {{\n\
         \x20       println(f\"{{s.key}}={{s.value}}\")\n\
         \x20   }}\n\
         }}"
    );
    assert!(
        findings_in(&source, &reads).is_empty(),
        "{:#?}",
        findings_in(&source, &reads)
    );

    let parsed = parse_to_ast(&source).expect("parses");
    let rust = nikaia::emit::emit_program_reading(&parsed, Default::default(), &reads)
        .expect("lowers")
        .rust;
    assert!(
        rust.contains("const SETTINGS: &[Setting] = &[Setting { key: \"host\""),
        "{rust}"
    );

    assert_eq!(
        run(&dir, &reads, &source),
        "3\nhost=example.com\nport=8080\nuser=ada\n"
    );
    std::fs::remove_dir_all(&dir).ok();
}
