//! A DSL with deferred parameters generates a shadow type (ADR-007 D5).
//!
//! Three claims, and each has a test that would fail if it were quietly
//! relaxed: the type is generated from the body's `:name` holes and from
//! nothing else; the call site supplies them after the `;` and the emitted Rust
//! builds one stack value out of them; and both ways of getting it wrong - a
//! parameter missing, a parameter that is not there - are errors against the
//! `.nika` file the user wrote (ADR-012).
//!
//! The first of those is checked by compiling the emitted Rust and running it,
//! because a lowering that produces plausible-looking Rust which does not build
//! is worth nothing.

mod common;

use std::path::PathBuf;
use std::process::Command;

use nikaia::check::Severity;
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn emit(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// What the compiler says about a source, in its own words.
fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library = nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

/// **The checker lets the driver use its spread**: `...args: Self::dsl` is a
/// parameter, so `return args` names something declared. It was `NK1117`,
/// which stopped `nikaia build` on this fixture while the lowering alone ran.
#[test]
fn a_spread_is_a_parameter_the_body_may_use() {
    let found = findings(&fixture("sql_statement.nika"));
    assert!(found.is_empty(), "{found:#?}");
}

/// The one that has to be true before any of the others are worth anything:
/// the emitted Rust compiles, runs, and prints what the program says.
#[test]
fn a_deferred_parameter_dsl_lowers_compiles_and_runs() {
    let source = fixture("sql_statement.nika");
    let rust = emit(&source);

    let dir = common::scratch_dir("dsl-parameters");
    let path = dir.join("statement.rs");
    std::fs::write(&path, &rust).expect("write the emitted Rust");

    let binary = dir.join("statement");
    let compiled = common::compile(
        &path,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr),
    );

    let run = Command::new(&binary).output().expect("run it");
    assert!(run.status.success(), "it should run");

    // The statement reaches the driver **with its holes as written**: a
    // deferred parameter is not string interpolation, so nothing was spliced
    // into the SQL (Part III, 15.3). The values arrived beside it.
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "people.db: SELECT name FROM people WHERE id = :id AND active = :active\n\
         id=501 active=true"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The type carries the body's names, and takes its field types from the call
/// site - because nothing in `… id = :id …` says what `:id` is, and an emitter
/// that decided would be guessing (ADR-011 D2).
#[test]
fn the_shadow_type_carries_the_names_and_the_call_site_the_types() {
    let rust = emit(&fixture("sql_statement.nika"));

    assert!(
        rust.contains("pub struct NikaiaDslParams_active_id<P0, P1>"),
        "{rust}"
    );
    assert!(rust.contains("pub id: P0,"), "{rust}");
    assert!(rust.contains("pub active: P1,"), "{rust}");

    // One stack value, built where the parameters are known.
    assert!(
        rust.contains("NikaiaDslParams_active_id { id: 501, active: true }"),
        "{rust}"
    );

    // The driver's spread is a generic parameter, which is what "monomorphised
    // per DSL string" is in the language below.
    assert!(
        rust.contains(
            "pub fn prepare<NikaiaDsl>(&self, statement: &str, args: NikaiaDsl) -> NikaiaDsl"
        ),
        "{rust}"
    );
}

/// The body's holes, and nothing that only looks like one.
#[test]
fn only_a_colon_and_a_name_is_a_hole() {
    let rust = emit(
        "fn main() {\n\
         \x20   let s = dsl sql { SELECT a::b FROM t WHERE at = 12:30 AND id = :id } eod\n\
         \x20   println(f\"{s}\")\n\
         }\n",
    );
    assert!(rust.contains("pub struct NikaiaDslParams_id<P0>"), "{rust}");
    assert!(!rust.contains("NikaiaDslParams_b"), "{rust}");
}

/// A `dsl html` keeps the binding ADR-017 built for it: `:rows` there is an
/// **immediate** capture from the enclosing scope, not a deferred parameter,
/// and D5 must not quietly change it.
#[test]
fn a_template_capture_is_not_a_deferred_parameter() {
    let rust = emit(
        "use std::html\n\
         pub struct Row { id: i32 }\n\
         fn page(rows: List[Row]) -> String {\n\
         \x20   return dsl html { <ul><for r in :rows><li>{r.id}</li></for></ul> } eod\n\
         }\n",
    );
    assert!(!rust.contains("NikaiaDslParams"), "{rust}");
    // `.iter()` and not `&`, which is the ordinary `for`'s rule
    // ([ADR-094](../../../docs/specification/adr/adr-094.md) D4): since D1 a
    // parameter the body only reads is already a view, and `&rows` would then
    // be a reference to one.
    assert!(rust.contains("for r in rows.iter()"), "{rust}");
}

/// A parameter the statement declares and the call does not pass.
#[test]
fn a_missing_parameter_is_an_error_against_the_nika_source() {
    let source = fixture("sql_statement.nika").replace(", active: true", "");
    let found = findings(&source);
    let missing = found
        .iter()
        .find(|f| f.code == "NK1112")
        .unwrap_or_else(|| panic!("no NK1112 in {found:#?}"));

    assert_eq!(missing.severity, Severity::Error);
    assert!(
        missing.message.contains("`query` needs `:active`"),
        "{}",
        missing.message
    );
    assert!(
        missing
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("active:"),
        "the way out has to be paste-ready (Part III, C.2): {missing:#?}"
    );

    // …on the statement that is wrong, in the file the user wrote.
    let line = source[..missing.span.at()].lines().count();
    assert!(
        source
            .lines()
            .nth(line - 1)
            .unwrap_or_default()
            .contains("db.prepare"),
        "the caret is on line {line}: {:?}",
        source.lines().nth(line - 1)
    );

    // …and it reads like every other diagnostic this compiler prints: the
    // place, the line it is about, a caret under it, the reason and one way
    // out (ADR-012, Part III C.2).
    let rendered = nikaia::diagnostics::render_finding(missing, "statement.nika", &source);
    assert!(rendered.starts_with("error[NK1112]: "), "{rendered}");
    assert!(
        rendered.contains(&format!("--> statement.nika:{line}:")),
        "{rendered}"
    );
    assert!(
        rendered.contains("    = note: The statement's parameters are"),
        "{rendered}"
    );
    assert!(rendered.contains("    = help: "), "{rendered}");
    assert!(
        rendered.lines().any(|l| l
            .trim_start()
            .strip_prefix('|')
            .is_some_and(|rest| rest.trim_start().starts_with('^'))),
        "{rendered}"
    );
}

/// A name the call passes and the statement does not have. The near-miss is
/// named, because that is what the mistake usually is.
#[test]
fn an_unknown_parameter_is_an_error_and_names_the_near_miss() {
    let source = fixture("sql_statement.nika").replace("active: true", "activ: true");
    let found = findings(&source);
    let unknown = found
        .iter()
        .find(|f| f.code == "NK1113")
        .unwrap_or_else(|| panic!("no NK1113 in {found:#?}"));

    assert!(
        unknown
            .message
            .contains("`query` has no parameter called `:activ`"),
        "{}",
        unknown.message
    );
    assert_eq!(unknown.help.as_deref(), Some("Did you mean `active`?"));

    // And the one it did not pass is reported too: two mistakes, two messages,
    // rather than one build per mistake.
    assert!(found.iter().any(|f| f.code == "NK1112"), "{found:#?}");
}

/// The statement may be the receiver or a subject, and the check does not care
/// which: Part II 10.5 writes `query.execute(target_age: …)` with the statement
/// as the receiver, and the fixture puts it before a `;` as a subject. Both
/// spell one protocol.
///
/// **The deferred parameters write no `;` where nothing stands before them**
/// ([ADR-133](../../../docs/specification/adr/adr-133.md) D1). The *signature*
/// keeps its own — `pub fn execute(ref self; ...args: Self::dsl)` has the subject
/// D3 names — and a receiver is outside the parentheses, so at the call there
/// is no zone for the separator to stand between.
#[test]
fn the_statement_may_be_the_receiver_or_a_subject() {
    let found = findings(
        "pub struct Db { name: String }\n\
         impl Db {\n\
         \x20   pub fn execute(ref self; ...args: Self::dsl) -> Self::dsl { return args }\n\
         }\n\
         fn go() {\n\
         \x20   let query = dsl mysql { SELECT 1 WHERE age >= :target_age } eod\n\
         \x20   let rows = query.execute(targt_age: 30)\n\
         }\n",
    );
    assert!(
        found.iter().any(|f| f.code == "NK1112"),
        "the one it forgot: {found:#?}"
    );
    assert!(
        found.iter().any(|f| f.code == "NK1113"),
        "the one it invented: {found:#?}"
    );
}

/// A name that stood for a statement and then stopped is no longer one: the
/// check follows what the `let` last said, and never claims about a call it
/// cannot place.
#[test]
fn a_rebound_name_is_no_longer_a_statement() {
    let found = findings(
        "pub struct Db { name: String }\n\
         impl Db {\n\
         \x20   pub fn execute(ref self; ...args: Self::dsl) -> Self::dsl { return args }\n\
         }\n\
         fn go() {\n\
         \x20   let query = dsl mysql { SELECT 1 WHERE age >= :target_age } eod\n\
         \x20   let query = 3\n\
         \x20   let rows = query.execute(anything: 30)\n\
         }\n",
    );
    assert!(
        found.iter().all(|f| !matches!(f.code, "NK1112" | "NK1113")),
        "{found:#?}"
    );
}

/// A call that gets it right says nothing at all - the checker never refuses a
/// program that is correct.
#[test]
fn a_complete_call_is_silent() {
    let found = findings(&fixture("sql_statement.nika"));
    assert!(
        found.iter().all(|f| !matches!(f.code, "NK1112" | "NK1113")),
        "{found:#?}"
    );
}

/// The spread has exactly one spelling, and a declaration that misses it is
/// told what to write rather than left at a parse error on the colon.
#[test]
fn a_spread_must_be_written_self_dsl() {
    let error = parse_to_ast(
        "pub struct Db { name: String }\n\
         impl Db {\n\
         \x20   pub fn prepare(ref self, statement: ref String; ...args: Params) -> i32 { return 1 }\n\
         }\n",
    )
    .expect_err("a spread of any other type is refused");
    let message = format!("{error:#}");
    assert!(
        message.contains("A spread parameter is written `...name: Self::dsl`."),
        "{message}"
    );
    // The rule is stated in the reader's words, on the line it is about, and
    // no longer by the number of the decision that made it.
    assert!(message.contains("line 3"), "{message}");
    assert!(!message.contains("ADR-"), "{message}");
}
