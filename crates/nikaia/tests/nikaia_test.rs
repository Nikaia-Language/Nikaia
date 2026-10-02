//! **`nikaia test`** ([ADR-264](../../../docs/specification/adr/adr-264.md) D1,
//! D12): `test "name" { … }` blocks in any file of a package, compiled only by
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

/// **An output test is three files and no code** (D13): `tests/NAME.stdout`,
/// and beside it what the program is given. The program is built as every
/// other build builds it, run with the arguments and the input, and passes
/// where it ends successfully and prints exactly the file. A `test` block in
/// the same package runs beside them, from its own build.
#[test]
fn an_output_test_is_what_the_program_prints() {
    let dir = a_project(
        "nikaia-test-output",
        &[(
            "main.nika",
            "use std::cli\n\
             use std::io\n\
             \n\
             fn main() throws {\n\
             \x20   let who = cli::args().nth(1) ?? \"nobody\"\n\
             \x20   let text = io::read_to_string()\n\
             \x20   let mut n = 0\n\
             \x20   for line in text.split(\"\\n\") {\n\
             \x20       if line != \"\" {\n\
             \x20           n += 1\n\
             \x20       }\n\
             \x20   }\n\
             \x20   println(f\"{who}: {n} lines\")\n\
             }\n\
             \n\
             test \"a block beside the output tests\" {\n\
             \x20   assert(1 + 1 == 2)\n\
             }\n",
        )],
    );
    let tests = dir.join("tests");
    std::fs::create_dir_all(&tests).expect("tests/");
    for (name, text) in [
        ("two.stdout", "ada: 2 lines\n"),
        ("two.stdin", "a\nb\n"),
        ("two.args", "ada\n"),
        ("none.stdout", "nobody: 0 lines\n"),
        ("wrong.stdout", "something else\n"),
    ] {
        std::fs::write(tests.join(name), text).expect("an output test");
    }
    let out = nikaia(&["test"], &dir);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    for line in [
        "running 4 tests",
        "test \"a block beside the output tests\" (src/main.nika:16) ... ok",
        "output \"none\" (tests/none.stdout) ... ok",
        "output \"two\" (tests/two.stdout) ... ok",
        "output \"wrong\" (tests/wrong.stdout) ... FAILED",
        "standard output (- expected, + printed):\n+ nobody: 0 lines\n- something else\n",
        "test result: FAILED. 3 passed; 1 failed",
    ] {
        assert!(stdout.contains(line), "missing `{line}`:\n{}", said(&out));
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// **A program whose result is a file** ([ADR-247](../../../docs/specification/adr/adr-247.md)):
/// it runs in a fresh directory of its own (D1) with `NAME.in/` copied in and
/// `NAME.out/` compared after (D2); nothing reaches the package; `--bless`
/// writes the expectations from what it did (D3); a failure is a difference
/// (D4); and a `test` block writes under `fs::scratch()` (D5).
#[test]
fn a_file_the_program_writes_is_an_expectation_and_is_blessed() {
    let dir = a_project(
        "nikaia-test-files",
        &[(
            "main.nika",
            "use std::cli\n\
             use std::fs\n\
             \n\
             fn shout(text: ref String) -> String {\n\
             \x20   return text.to_uppercase()\n\
             }\n\
             \n\
             fn main() throws {\n\
             \x20   let from = cli::args().nth(1) ?? \"in.txt\"\n\
             \x20   let to = cli::args().nth(2) ?? \"out.txt\"\n\
             \x20   let text = fs::read_to_string(ref from, fs::Root::Anywhere)\n\
             \x20   fs::write(ref to, fs::Root::Anywhere, shout(text))\n\
             \x20   println(\"written\")\n\
             }\n\
             \n\
             test \"a file written under a scratch root reads back\" {\n\
             \x20   let root = fs::scratch()\n\
             \x20   fs::write(\"loud.txt\", root, shout(\"hi\"))\n\
             \x20   let back = fs::read_to_string(\"loud.txt\", root)\n\
             \x20   assert(back == \"HI\")\n\
             }\n",
        )],
    );
    let tests = dir.join("tests");
    std::fs::create_dir_all(tests.join("loud.in")).expect("loud.in/");
    std::fs::create_dir_all(tests.join("loud.out")).expect("loud.out/");
    std::fs::write(tests.join("loud.in/in.txt"), "one\ntwo\n").expect("input");
    std::fs::write(tests.join("loud.args"), "in.txt\nout.txt\n").expect("args");
    // Started empty and blessed, as D3 says a new test is.
    std::fs::write(tests.join("loud.stdout"), "").expect("stdout");
    std::fs::write(tests.join("loud.out/out.txt"), "").expect("out");

    let first = nikaia(&["test"], &dir);
    let said_first = String::from_utf8_lossy(&first.stdout).to_string();
    assert_eq!(first.status.code(), Some(1), "{}", said(&first));
    for words in [
        "output \"loud\" (tests/loud.stdout) ... FAILED",
        "standard output (- expected, + printed):\n+ written\n",
        "out.txt (- expected, + written):\n+ ONE\n+ TWO\n",
        "test \"a file written under a scratch root reads back\" (src/main.nika:16) ... ok",
    ] {
        assert!(
            said_first.contains(words),
            "missing `{words}`:\n{}",
            said(&first)
        );
    }

    let blessed = nikaia(&["test", "--both-settings", "--bless"], &dir);
    assert!(blessed.status.success(), "{}", said(&blessed));
    assert!(
        String::from_utf8_lossy(&blessed.stdout)
            .contains("output \"loud\" (tests/loud.stdout) ... blessed"),
        "{}",
        said(&blessed)
    );
    assert_eq!(
        std::fs::read_to_string(tests.join("loud.stdout")).expect("blessed"),
        "written\n"
    );
    assert_eq!(
        std::fs::read_to_string(tests.join("loud.out/out.txt")).expect("blessed"),
        "ONE\nTWO\n"
    );

    let again = nikaia(&["test", "--both-settings"], &dir);
    assert!(again.status.success(), "{}", said(&again));
    // **Nothing the program wrote reached the package** (D1).
    assert!(!dir.join("out.txt").exists() && !dir.join("tests/out.txt").exists());
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
