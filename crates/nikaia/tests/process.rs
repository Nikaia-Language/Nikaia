//! **`std::process`: another program, started and waited for**
//! ([ADR-243](../../../docs/specification/adr/adr-243.md), on
//! [ADR-195](../../../docs/specification/adr/adr-195.md) D4).
//!
//! Every test here **runs a program**, at both settings of
//! `user_parallelism`, because what is claimed is behaviour: the exit code and
//! both streams arrive, a non-zero exit is an answer rather than a failure, a
//! program that is not there is a `catch`, and the wait gives the thread up.

mod common;

use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

/// Lowered at `how`, compiled, and run in a directory of its own, which is
/// also where the program's children start.
fn ran(purpose: &str, source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, how).expect("it lowers").rust;
    let dir = common::scratch_dir(&format!("process-{purpose}"));
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &["--crate-type", "bin", "-o", &binary.to_string_lossy()],
    );
    assert!(
        compiled.status.success(),
        "{purpose} did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run it");
    assert!(
        out.status.success(),
        "{purpose} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn runs(purpose: &str, source: &str, expected: &str) {
    let found = findings(source);
    assert!(found.is_empty(), "{purpose}: {found:#?}");
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran(purpose, source, how), expected, "{purpose} at {how:?}");
    }
}

/// **The exit code and both streams arrive, and a non-zero exit is an answer.**
/// `exit 3` is not a failure the program has to `catch`: it is `code` and `ok`,
/// and what the tool printed on the way out is still there to read.
#[test]
fn a_program_runs_and_what_it_said_comes_back() {
    let source = "use std::process\n\
                  \n\
                  fn main() throws {\n\
                  \x20   let out = process::run(\"sh\", [\"-c\", \"echo hello; echo oops >&2; exit 3\"])\n\
                  \x20   println(f\"code={out.code} ok={out.ok}\")\n\
                  \x20   print(f\"out={out.stdout}\")\n\
                  \x20   print(f\"err={out.stderr}\")\n\
                  \x20   let fine = process::run(\"true\", [])\n\
                  \x20   println(f\"code={fine.code} ok={fine.ok}\")\n\
                  }\n";
    runs(
        "streams",
        source,
        "code=3 ok=false\nout=hello\nerr=oops\ncode=0 ok=true\n",
    );
}

/// **`dir` is where the child starts**, and the arguments reach it one by one:
/// a space inside one is not a second argument, because there is no shell in
/// between unless the program asks for one.
#[test]
fn the_directory_and_the_arguments_are_the_callers() {
    let source = "use std::process\n\
                  \n\
                  fn main() throws {\n\
                  \x20   let here = process::run(\"pwd\", []; dir: \"/\")\n\
                  \x20   print(here.stdout)\n\
                  \x20   let mut args: Vec[String] = [\"-c\", \"echo $#: $1\", \"sh\"]\n\
                  \x20   args.push(\"one two\")\n\
                  \x20   args.push(\"three\")\n\
                  \x20   let counted = process::run(\"sh\", args)\n\
                  \x20   print(counted.stdout)\n\
                  }\n";
    runs("dir-args", source, "/\n2: one two\n");
}

/// **A program that is not there is a `catch`**, with the name that was looked
/// for in it, as `fs` says which path was not found.
#[test]
fn a_program_that_is_not_there_is_caught() {
    let source = "use std::process\n\
                  use std::io\n\
                  \n\
                  fn main() {\n\
                  \x20   let out = process::run(\"no-such-program-anywhere\", []) catch {\n\
                  \x20       match error {\n\
                  \x20           io::IoError::NotFound(what) => println(f\"not found: {what}\"),\n\
                  \x20           else => println(\"something else\"),\n\
                  \x20       }\n\
                  \x20       return\n\
                  \x20   }\n\
                  \x20   println(f\"ran: {out.code}\")\n\
                  }\n";
    runs("missing", source, "not found: no-such-program-anywhere\n");
}

/// **The wait gives the thread up** (D3), which is the claim a timing test
/// could only suggest.
///
/// At `user_parallelism = no` there is one thread for user code. The first
/// branch starts a child that waits - bounded, so a failure is an answer and
/// not a hang - for a file the second branch's child makes. If waiting for the
/// first child held the thread, the second branch could not start its child
/// until the first gave up, and the first would say `timed out`.
#[test]
fn waiting_for_a_program_gives_the_thread_up() {
    let source = "use std::process\n\
                  \n\
                  fn main() throws {\n\
                  \x20   let (waited, made) = overlap {\n\
                  \x20       process::run(\"sh\", [\"-c\", \"i=0; while [ ! -f marker ]; do i=$((i+1)); if [ $i -gt 500 ]; then echo timed out; exit 1; fi; sleep 0.01; done; echo saw it\"])\n\
                  \x20       process::run(\"sh\", [\"-c\", \"sleep 0.2; touch marker\"])\n\
                  \x20   }\n\
                  \x20   print(waited.stdout)\n\
                  \x20   println(f\"{made.ok}\")\n\
                  }\n";
    runs("gives-up", source, "saw it\ntrue\n");
}
