//! What leaves the compiler when the **program** is wrong.
//!
//! Part III C.1 says a message about the generated Rust must not reach the user.
//! This is the same promise from the other side: nothing about how this compiler
//! is built may reach somebody who only wrote a program. Both used to leave by
//! the same door and the door was Rust's - a type error in a `.nika` file ended as
//! an `anyhow::Error` out of `main`, so after the diagnostics had said their piece
//! the terminal also got `Error: 1 type error` and, where `RUST_BACKTRACE` is set,
//! ten frames of `nikaia::project::check` and friends.
//!
//! Every test here sets `RUST_BACKTRACE=1`, because that is the state the defect
//! was found in and the state a developer's shell is usually in.

mod common;

use std::path::Path;
use std::process::{Command, Output};

/// Run the compiler on one file, with a backtrace asked for.
fn refuse(dir: &Path, source: &str) -> Output {
    let input = dir.join("main.nika");
    std::fs::write(&input, source).expect("the source");
    Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["--input", input.to_str().expect("utf-8 path")])
        .args([
            "--output",
            dir.join("main.rs").to_str().expect("utf-8 path"),
        ])
        .args(["--no-cache"])
        .env("NIKAIA_CACHE_DIR", dir.join("cache"))
        .env("RUST_BACKTRACE", "1")
        .output()
        .expect("the nikaia binary runs")
}

/// Nothing of this compiler's own machinery, whatever went wrong in the program.
fn says_nothing_of_the_compiler(said: &str) {
    for leak in [
        "Stack backtrace",
        "anyhow",
        "nikaia::",
        "__libc_start_main",
        "core::panicking",
    ] {
        assert!(!said.contains(leak), "`{leak}` reached the user:\n{said}");
    }
}

/// A type error: the diagnostic, the tally, and nothing else.
#[test]
fn a_refused_program_says_what_is_wrong_and_no_more() {
    let dir = common::scratch_dir("refusal-types");
    let ran = refuse(
        &dir,
        "fn main() {\n    \
             let n: i64 = \"seven\"\n    \
             println(f\"{n}\")\n\
         }\n",
    );
    assert!(!ran.status.success(), "this program must be refused");
    let said = String::from_utf8_lossy(&ran.stderr);

    assert!(said.contains("error[NK1103]"), "{said}");
    assert!(said.contains("1 type error"), "the tally stays: {said}");
    // And the tally is not introduced as an `Error`, which is Rust's word for
    // something this compiler failed at rather than something it refused.
    assert!(
        !said.contains("Error: 1 type error"),
        "the tally must not arrive as an `Error:`:\n{said}"
    );
    says_nothing_of_the_compiler(&said);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A program that does not parse leaves the same way, and still names its file.
///
/// The file name is the point of the second assertion: the path is added as
/// context *around* the refusal, so the refusal is no longer the outermost error
/// - and formatting it into a string would have lost the type that says it is one.
#[test]
fn a_program_that_does_not_parse_says_where_and_no_more() {
    let dir = common::scratch_dir("refusal-parse");
    let ran = refuse(&dir, "fn main() {\n    let x =\n}\n");
    assert!(!ran.status.success(), "this program must be refused");
    let said = String::from_utf8_lossy(&ran.stderr);

    assert!(
        said.starts_with("error: Expected expression here, but found `}`."),
        "{said}"
    );
    assert!(said.contains("main.nika:3:1"), "it names the file: {said}");
    says_nothing_of_the_compiler(&said);
    let _ = std::fs::remove_dir_all(&dir);
}

/// **And a failure of this compiler keeps its backtrace**, which is the other
/// half of the rule rather than an exception to it.
///
/// A file that is not there is nothing a program said wrong, and the frames are
/// then the most useful thing on the screen. Without this assertion the change
/// above would be indistinguishable from one that swallowed every trace.
#[test]
fn a_failure_of_the_compiler_keeps_its_trace() {
    let ran = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["--input", "/nowhere/at/all.nika"])
        .env("RUST_BACKTRACE", "1")
        .output()
        .expect("the nikaia binary runs");
    assert!(!ran.status.success());
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(said.contains("Error:"), "{said}");
    assert!(said.contains("Stack backtrace"), "{said}");
}

/// **Every statement about the program leaves without a backtrace, wherever in
/// the compiler it is made** (`docs/open-work.md`, since closed).
///
/// The first fix covered the type checker and the parser, which is where a
/// refusal usually comes from. It is not the only place: the emitter refuses a
/// `dsl` naming a grammar that does not exist, the manifest reader refuses an
/// unknown key, the project driver refuses a missing entry point, and the CLI
/// refuses a backend nobody has. Every one of those used to arrive with
/// `Error:` in front of it and ten frames of this compiler behind it.
///
/// The four here are four different modules on purpose - `emit`, `manifest`,
/// `project` and `main` - because what is being tested is that the rule is not
/// one path's habit.
#[test]
fn a_refusal_from_anywhere_in_the_compiler_carries_no_trace() {
    let dir = common::scratch_dir("refusal-everywhere");

    // The CLI: a backend that does not exist.
    let source = dir.join("main.nika");
    std::fs::write(&source, "fn main() { }\n").expect("a source");
    let said = run(&[
        "--input",
        source.to_str().expect("utf-8 path"),
        "--backend",
        "bogus",
    ]);
    assert_refusal(&said, "There's no backend called `bogus`.");

    // The emitter: a `dsl` naming a grammar this compiler does not have.
    std::fs::write(
        &source,
        // `} eod` is what closes a `dsl` block, and it matters here: without it
        // this is not a `dsl` at all and the *checker* speaks first.
        "fn main() {\n    let q = dsl postgres {\n        SELECT 1\n    } eod\n}\n",
    )
    .expect("a source");
    let said = run(&[
        "--input",
        source.to_str().expect("utf-8 path"),
        "--output",
        dir.join("out.rs").to_str().expect("utf-8 path"),
    ]);
    assert_refusal(&said, "`postgres` isn't a language the compiler knows");

    // The project driver: no entry point.
    let project = common::scratch_dir("refusal-no-entry");
    std::fs::write(project.join("nikaia.toml"), "[package]\nname = \"x\"\n").expect("a manifest");
    let said = run(&["build", "--project", project.to_str().expect("utf-8 path")]);
    assert_refusal(&said, "is not there");

    // The manifest reader: a key nobody has.
    std::fs::create_dir_all(project.join("src")).expect("src");
    std::fs::write(project.join("src/main.nika"), "fn main() { }\n").expect("an entry");
    std::fs::write(
        project.join("nikaia.toml"),
        "[package]\nname = \"x\"\n\n[build]\nnonsense = true\n",
    )
    .expect("a manifest");
    let said = run(&["build", "--project", project.to_str().expect("utf-8 path")]);
    assert_refusal(&said, "has no key called `nonsense`");

    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(project);
}

/// What every refusal above has to look like: the message, and nothing of this
/// compiler's insides. `RUST_BACKTRACE` is **set**, which is the case the rule is
/// about - a developer has it set, and it used to turn every refusal into ten
/// frames of `nikaia::project`.
fn assert_refusal(said: &str, about: &str) {
    assert!(said.contains(about), "{said}");
    assert!(!said.contains("Stack backtrace"), "{said}");
    assert!(!said.contains("nikaia::"), "{said}");
    assert!(!said.starts_with("Error:"), "{said}");
}

fn run(args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(args)
        .arg("--no-cache")
        .env("RUST_BACKTRACE", "1")
        .output()
        .expect("the nikaia binary runs");
    assert!(!out.status.success(), "this must be refused: {args:?}");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}
