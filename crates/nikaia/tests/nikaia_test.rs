//! **`nikaia test`** ([ADR-245](../../../docs/specification/adr/adr-245.md) D1,
//! D7): `test "name" { … }` blocks in any file of a package, compiled only by
//! this command, each run in a process of its own - at the project's setting,
//! or at both with `--both-settings`. Driven through the real binary, because
//! the command, the build and the runner are the thing under test.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The cache `tests/project.rs` shares, for its reason: not the developer's.
fn shared_cache_dir() -> PathBuf {
    repo_root().join("target").join("nikaia-project-tests")
}

fn a_project(purpose: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = common::scratch_dir(purpose);
    std::fs::create_dir_all(dir.join("src")).expect("src");
    std::fs::write(
        dir.join("nikaia.toml"),
        "[package]\nname = \"tally\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest");
    for (name, text) in files {
        std::fs::write(dir.join("src").join(name), text).expect("source");
    }
    dir
}

fn nikaia(args: &[&str], dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(args)
        .arg("--project")
        .arg(dir)
        .env("NIKAIA_CACHE_DIR", shared_cache_dir())
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("RUST_BACKTRACE")
        .output()
        .expect("the nikaia binary runs")
}

fn said(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

const MAIN: &str = r#"use std::text

fn main() {
    println(f"{double(21)}")
}

fn number(s: ref String) -> i64 throws {
    let n = text::parse_i64(s) ?? throw NotANumber { text: s.clone() }
    return n
}

struct NotANumber {
    text: String,
}

impl Error for NotANumber {
    fn message(ref self) -> String {
        return f"not a number: {self.text}"
    }
}

test "a claim that holds" {
    let twelve = number("12")
    assert(double(2) == 4 && twelve == 12)
}

test "a false claim fails" {
    let n = double(3)
    assert(n == 7; message: "three doubled")
}

test "a failure that leaves the body fails the test" {
    let n = number("12x")
    assert(n == 12)
}

test "a panic fails the test" {
    panic("gave up")
}
"#;

const HELPERS: &str = r#"fn double(n: i64) -> i64 sync {
    return n * 2
}

test "a test in another file sees this one" {
    assert(double(5) == 10)
}
"#;

/// **The whole of D1 and D7 in one build per setting**: every kind of outcome,
/// in two files, at both settings, with the exit status a script reads.
#[test]
fn each_test_runs_by_itself_and_says_how_it_went() {
    let dir = a_project(
        "nikaia-test-outcomes",
        &[("main.nika", MAIN), ("helpers.nika", HELPERS)],
    );
    let out = nikaia(&["test", "--both-settings"], &dir);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    for line in [
        "running 5 tests",
        "test \"a claim that holds\" (src/main.nika:22) ... ok",
        "test \"a false claim fails\" (src/main.nika:27) ... FAILED",
        "test \"a failure that leaves the body fails the test\" (src/main.nika:32) ... FAILED",
        "test \"a panic fails the test\" (src/main.nika:37) ... FAILED",
        "test \"a test in another file sees this one\" (src/helpers.nika:5) ... ok",
        "test result: FAILED. 2 passed; 3 failed",
    ] {
        assert!(stdout.contains(line), "missing `{line}`:\n{}", said(&out));
    }
    // What each failure said, in the words D2 and D1 give it.
    for words in [
        "main.nika:29: the program stopped: assertion failed: three doubled",
        "    assert(n == 7; message: \"three doubled\")\n    n is 6",
        "not a number: 12x",
        "main.nika:38: the program stopped: gave up",
    ] {
        assert!(stderr.contains(words), "missing `{words}`:\n{}", said(&out));
    }
    // **A build leaves them out** (D1): the program is the program.
    let run = nikaia(&["run"], &dir);
    assert!(run.status.success(), "{}", said(&run));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
    std::fs::remove_dir_all(&dir).ok();
}

/// **A test block is compiled only by `nikaia test`** (D1): a mistake in one
/// does not stop the program from building, and the test build refuses it.
#[test]
fn a_test_is_checked_by_the_test_build_and_by_nothing_else() {
    let dir = a_project(
        "nikaia-test-only",
        &[(
            "main.nika",
            "fn main() {\n    println(\"built\")\n}\n\ntest \"wrong\" {\n    let n: i64 = \"a\"\n}\n",
        )],
    );
    let run = nikaia(&["run"], &dir);
    assert!(run.status.success(), "{}", said(&run));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "built\n");
    let test = nikaia(&["test"], &dir);
    assert!(!test.status.success(), "{}", said(&test));
    assert!(said(&test).contains("main.nika:6"), "{}", said(&test));
    std::fs::remove_dir_all(&dir).ok();
}

/// **No test is not a failure**, and it is said rather than built.
#[test]
fn a_package_without_tests_says_so() {
    let dir = a_project("nikaia-test-none", &[("main.nika", "fn main() {\n}\n")]);
    let out = nikaia(&["test"], &dir);
    assert!(out.status.success(), "{}", said(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("no tests"),
        "{}",
        said(&out)
    );
    std::fs::remove_dir_all(&dir).ok();
}
