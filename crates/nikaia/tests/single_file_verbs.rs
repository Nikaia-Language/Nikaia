//! **The verbs that take one file** ([ADR-260](../../../docs/specification/adr/adr-260.md)
//! D3, D5): `nikaia lower` writes the Rust and stops, `nikaia interpret` runs
//! the file, `nikaia explain` reads `rustc`'s report and places it in the
//! `.nika`. They replaced `nikaia --input`, which named an argument rather than
//! what happened to it, and whose switches three jobs shared.
//!
//! Lowering is still the one code generator there is (ADR-004 D1): a `.nika`
//! file becomes Rust source text, and compiling that is `rustc`'s.

mod common;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sample() -> PathBuf {
    repo_root().join("tests/samples/hello_world.nika")
}

fn nikaia(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(args)
        .output()
        .expect("the nikaia binary runs")
}

fn said(output: &Output) -> String {
    format!(
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// **`lower` writes the Rust and the ledger, and does not call `rustc`**: what
/// is beside the output afterwards is what it says it writes, and no program.
#[test]
fn lower_writes_rust_and_compiles_nothing() {
    let dir = common::scratch_dir("verb-lower");
    let out = dir.join("hello.rs");

    let run = nikaia(&[
        "lower",
        sample().to_str().expect("sample path"),
        "--output",
        out.to_str().expect("output path"),
    ]);
    assert!(run.status.success(), "{}", said(&run));
    let emitted = std::fs::read_to_string(&out).expect("the lowered Rust");
    assert!(emitted.contains("fn main()"), "not a program:\n{emitted}");

    let mut written: Vec<String> = std::fs::read_dir(&dir)
        .expect("the output directory")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    written.sort();
    assert_eq!(
        written,
        ["hello.rs", "nikaia.contracts", "nikaia.derived"],
        "the Rust and the ledger beside it, and nothing compiled"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The flags that explain a decision rather than changing one stay on `lower`
/// (D3), and Part I 8.1.1 writes that command line. This is what keeps the
/// specification's own line true.
#[test]
fn the_explanations_answer_on_lower() {
    let dir = common::scratch_dir("verb-lower-explains");
    let out = dir.join("hello.rs");
    let path = sample();

    let overlaps = nikaia(&[
        "lower",
        path.to_str().expect("sample path"),
        "--output",
        out.to_str().expect("output path"),
        "--overlaps",
    ]);
    assert!(overlaps.status.success(), "{}", said(&overlaps));
    let told = String::from_utf8_lossy(&overlaps.stdout);
    assert!(
        told.contains("`overlap { … }` block"),
        "`--overlaps` must report on the blocks, not be silently dropped: {told}"
    );

    let trust = nikaia(&[
        "lower",
        path.to_str().expect("sample path"),
        "--output",
        out.to_str().expect("output path"),
        "--trust",
    ]);
    assert!(trust.status.success(), "{}", said(&trust));
    assert!(
        String::from_utf8_lossy(&trust.stdout).contains("provenance"),
        "`--trust` must answer too: {}",
        said(&trust)
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **`explain` reads `rustc`'s report on stdin** and writes nothing: a report
/// with nothing in it is a file with nothing to say about it.
#[test]
fn explain_reads_the_report_on_stdin() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["explain", sample().to_str().expect("sample path")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the nikaia binary runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"")
        .expect("an empty report");
    let run = child.wait_with_output().expect("it finishes");
    assert!(run.status.success(), "{}", said(&run));
    assert!(run.stdout.is_empty(), "{}", said(&run));
}

/// **`--output` belongs to `lower` alone** (D3): `interpret` writes nothing, so
/// there is nowhere for it to go.
#[test]
fn output_belongs_to_lower_alone() {
    let run = nikaia(&[
        "interpret",
        sample().to_str().expect("sample path"),
        "--output",
        "/tmp/nowhere.rs",
    ]);
    assert!(!run.status.success(), "{}", said(&run));
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("--output"),
        "{}",
        said(&run)
    );
}

/// **`--input` and `--backend` are gone, not aliases** (D5): a deprecated
/// second name is one more thing to remove, and the language is pre-alpha.
/// Each is refused by name, so a reader with an old command line is told which
/// part of it no longer means anything.
#[test]
fn the_old_single_file_switches_are_refused() {
    for args in [
        vec!["--input", "x.nika"],
        vec!["lower", "x.nika", "--backend", "rust"],
    ] {
        let run = nikaia(&args);
        assert!(!run.status.success(), "{args:?} must be refused");
        let told = String::from_utf8_lossy(&run.stderr);
        assert!(
            told.contains(args.iter().find(|a| a.starts_with("--")).expect("a switch")),
            "the refusal names the switch: {told}"
        );
    }
}
