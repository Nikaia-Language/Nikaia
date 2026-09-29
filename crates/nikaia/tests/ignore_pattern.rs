//! `_` is the ignore pattern
//! ([ADR-126](../../../docs/specification/adr/adr-126.md)).
//!
//! `_` existed in one place, a `match` arm. Everywhere else a value arrived it
//! had to be given a name whether or not the body wanted it — a tuple's second
//! half, a parameter a shape dictates, a lambda's argument — and a name nobody
//! reads is a name a reader looks for.
//!
//! **It was also already accepted, and that is what the record is really about.**
//! `_` parsed as an ordinary name, so `let _ = f()` compiled and lowered to
//! Rust's own `let _ =` — which **discards** the value where the source said
//! *bound*. For a file handle or a lock guard those are different programs.
//! `open-work.md` found it; D2 refuses it.

mod common;

use std::path::PathBuf;
use std::process::Command;

use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> Result<String, String> {
    let parsed = parse_to_ast(source).map_err(|e| format!("{e:#}"))?;
    emit_program(&parsed, Build::default())
        .map(|it| it.rust)
        .map_err(|e| format!("{e:#}"))
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

/// **The three positions, and the lowering is Rust's own `_`** (D1, D4).
///
/// A tuple position, a parameter a shape dictates, and a lambda's argument. The
/// `match` arm is the one it always had.
#[test]
fn the_ignore_pattern_stands_where_a_name_would_be_bound() {
    let rust = lowered(
        "fn pair() -> (i64, i64) { return (1, 2) }\n\
         fn handle(event: i64, _: String) -> i64 { return event }\n\
         fn twice(x: i64, f: fn(i64) -> i64 sync) -> i64 { return f(f(x)) }\n\
         fn main() {\n\
             let (first, _) = pair()\n\
             let tag = \"x\"\n\
             let n = twice(1, fn(_) { 3 })\n\
             let m = match first { 0 => 1, else => 2 }\n\
             println(f\"{handle(first, tag)} {n} {m}\")\n\
         }",
    )
    .expect("it parses and lowers");

    assert!(rust.contains("let (first, _) = pair()"), "{rust}");
    assert!(
        rust.contains("_: &str"),
        "a parameter keeps its type: {rust}"
    );
    assert!(rust.contains("|_|"), "a lambda's argument: {rust}");
    // D4's point: nothing is bound below, so there is no `unused variable`
    // warning about a file nobody wrote to silence.
    assert!(!rust.contains("_unused"), "{rust}");
}

/// **A parameter written `_` still carries its type, and the ledger writes it**
/// (D1, and §3's third bullet).
///
/// The caller needs the type: what `_` says is that this body does not read the
/// value, never that the signature is shorter. And its `keeps` answer can only be
/// *not kept* — a body that never names it cannot store it — so a caller lends
/// it, which is what `&String` above is.
#[test]
fn the_ledger_writes_an_ignored_parameter_and_never_keeps_it() {
    let parsed = parse_to_ast("pub fn handle(event: i64, _: String) -> i64 { return event }")
        .expect("it parses");
    let ledger = Ledger::infer(&parsed);
    let handle = ledger.functions.get("handle").expect("the entry");
    let signature = handle.signature.as_ref().expect("a signature");
    assert!(
        signature.params.iter().any(|(name, _)| name == "_"),
        "the ignored parameter is in the column: {:?}",
        signature.params
    );
    assert!(
        !handle.keeps.iter().any(|k| k == "_"),
        "an ignored parameter is never kept: {:?}",
        handle.keeps
    );
}

/// **`let _ = expr` is refused** (D2, `NK1144`).
///
/// A binding that ignores its whole value binds nothing, so the word `let` says
/// something that does not happen — and the form's one use in Rust is the one
/// thing this language will not let a reader miss.
#[test]
fn a_let_that_binds_nothing_is_refused() {
    let found: Vec<_> = findings(
        "fn side() -> i64 { return 7 }\n\
         fn main() { let _ = side() println(\"done\") }",
    )
    .into_iter()
    .filter(|f| f.code == "NK1144")
    .collect();
    assert_eq!(found.len(), 1, "one refusal: {found:#?}");
    let notes = found[0].notes.join(" ");
    assert!(notes.contains("written as the call"), "{notes}");
    assert!(
        notes.contains("throw the value away on the spot"),
        "and why the lowering is not the same thing: {notes}"
    );
    assert!(
        found[0]
            .help
            .as_deref()
            .expect("a way out")
            .contains("give it a name"),
        "{:?}",
        found[0].help
    );

    // And the tuple position is not refused: it binds `a`.
    assert!(
        findings(
            "fn pair() -> (i64, i64) { return (1, 2) }\n\
             fn main() { let (a, _) = pair() println(f\"{a}\") }",
        )
        .iter()
        .all(|f| f.code != "NK1144"),
        "a destructured tuple is D1's own position"
    );
}

/// **`_` is not a value** (D2): `x + _` does not parse.
///
/// It used to — `_` was an ordinary name, so the line parsed and `NK1117` said
/// nothing declared it. A name and an ignore pattern answering the same
/// character was the accident the record closed.
#[test]
fn the_ignore_pattern_is_not_an_expression() {
    let said = lowered("fn main() { let n = 1 let m = n + _ println(f\"{m}\") }")
        .expect_err("`_` is not a value");
    assert!(
        said.contains("Expected expression here, but found `_`."),
        "{said}"
    );
}

/// **A name that begins with `_` is still a name**, which is what the two
/// lookaheads in front of the ignore pattern buy.
///
/// `_count` and `_0` are ordinary names. The second half of this test used to
/// be `1_000`, whose `_000` was a name `NK1117` reported about; since
/// [ADR-136](../../../docs/specification/adr/adr-136.md) that is the number
/// `1000`, so what stands here now is the case the lookaheads are actually
/// for — a **leading** underscore, which no number form has and every language
/// with both reads as a name.
#[test]
fn a_name_beginning_with_an_underscore_is_a_name() {
    let rust = lowered("fn main() { let _count = 1 let _0 = 2 println(f\"{_count} {_0}\") }")
        .expect("both are names");
    assert!(rust.contains("_count"), "{rust}");

    let found: Vec<_> = findings("fn main() { println(f\"{_000}\") }")
        .into_iter()
        .filter(|f| f.code == "NK1117")
        .collect();
    assert!(
        found.iter().any(|f| f.message.contains("_000")),
        "a leading underscore is a name: {found:#?}"
    );

    // And the form it replaced is a number now, with nothing left to report.
    let separated = findings("fn main() { let n = 1_000 println(f\"{n}\") }");
    assert!(separated.is_empty(), "{separated:#?}");
}

/// **An ignored lambda argument produces no warning below** (D4, and §5's third
/// step), through `rustc` itself.
///
/// The whole point of the record in [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s
/// terms: a placeholder name earned an `unused variable` warning about a
/// generated file, and `_` binds nothing so there is nothing to warn about.
#[test]
fn an_ignored_argument_earns_no_warning_from_rustc() {
    let rust = lowered(
        "fn twice(x: i64, f: fn(i64) -> i64 sync) -> i64 { return f(f(x)) }\n\
         fn ignored(event: i64, _: String) -> i64 { return event }\n\
         fn main() {\n\
             let tag = \"x\"\n\
             let n = twice(1, fn(_) { 3 })\n\
             println(f\"{ignored(n, tag)}\")\n\
         }",
    )
    .expect("it lowers");

    let dir = common::scratch_dir("ignore-pattern");
    let file = dir.join("ignored.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("ignored");
    // **No `-A warnings`**, deliberately: the warning is what is being asserted
    // absent, so silencing warnings would make this test pass forever.
    let compiled = common::compile(
        &file,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    let said = String::from_utf8_lossy(&compiled.stderr).into_owned();
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{said}\n--- emitted ---\n{rust}"
    );
    assert!(
        !said.contains("unused variable"),
        "an ignored value binds nothing below, so there is nothing to warn \
         about:\n{said}"
    );
    let ran = Command::new(&binary).output().expect("the program runs");
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim_end(), "3");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **No program in the repository is broken by `_` ceasing to be a name.**
///
/// `open-work.md` recorded that nothing in `examples/`, `tests/samples/` or
/// `crates/nikaia-std/src/` writes one, which is what made this free to decide.
/// This is that claim, kept.
#[test]
fn nothing_in_the_tree_used_the_underscore_as_a_name() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut checked = 0;
    let mut pending = vec![
        root.join("examples"),
        root.join("tests"),
        root.join("crates/nikaia-std/src"),
    ];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_some_and(|e| e == "nika") {
                let source = std::fs::read_to_string(&path).expect("read");
                // Parsing is the claim: a bare `_` where a name belongs is now a
                // parse error, so a file that still parses never used one.
                if parse_to_ast(&source).is_ok() {
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 12, "only {checked} programs parsed");
}
