//! Every example that claims to run, checked.
//!
//! `examples/README.md` divides the directory in two: programs the bootstrap
//! compiler handles today, and files written at specification level to find out
//! what the specification forgot. Only the first kind can be checked.
//!
//! **The examples check themselves**
//! ([ADR-264](../../../docs/specification/adr/adr-264.md) D13,
//! [ADR-247](../../../docs/specification/adr/adr-247.md)): each is a package
//! whose `tests/` says what it must print and write - `NAME.stdout`,
//! `NAME.out/`, with `NAME.in/`, `.args` and `.stdin` as what it is given - and
//! this file runs `nikaia test --both-settings` in a copy of each. At **both
//! settings of `user_parallelism`**, because that is the claim the switches
//! rest on (Part I 1.2): the same source means the same thing either way.
//!
//! `1brc.nika` has a test of its own (`one_brc.rs`), because it checks more
//! than its output. The last tests here make sure no example escapes both, and
//! two ask what an example's comment claims about a bad input.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use nikaia::emit::Build;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Written at specification level: they say what the language is meant to look
/// like and the bootstrap compiler does not handle them yet. Each one's gaps
/// are listed in `examples/README.md`.
const SPECIFICATION_LEVEL: &[&str] = &["fortunes.nika"];

/// Runnable, but with a test of its own that checks more than the output.
const COVERED_ELSEWHERE: &[&str] = &["1brc.nika"];

/// Directories under `examples/` that do not check themselves, because their
/// check lives in another test file. Each is named together with
/// that test, so a directory cannot sit here unchecked and so a deleted test
/// leaves a name pointing at nothing.
const DIRECTORIES_CHECKED_ELSEWHERE: &[(&str, &str)] = &[
    // The Rust-passthrough arm and the experiment ADR-038 D7 put there. Its
    // own test builds each program in it and checks more than the output.
    ("foreign-runtime", "tests/foreign_runtime.rs"),
    // A package has no output of its own; it is built by the program that
    // depends on it.
    (
        "http",
        "tests/project.rs: the_http_package_serves_its_example",
    ),
    // That program. It is a directory rather than a file because reaching a
    // package by a path needs a manifest.
    (
        "hello-http",
        "tests/project.rs: the_http_package_serves_its_example",
    ),
    // **The C boundary against a real library**
    // ([ADR-147](../../../docs/specification/adr/adr-147.md) §5 step 5). It
    // needs `-l sqlite3`, which no build here passes and not every machine has,
    // so its test skips rather than fails where the package is missing — and a
    // run that may skip cannot be one this file asserts output for.
    (
        "sqlite",
        "tests/foreign_pointers.rs: sqlite3_from_end_to_end",
    ),
];

#[test]
fn no_example_is_neither_run_nor_declared() {
    let dir = repo_root().join("examples");
    let mut seen = 0;

    for entry in std::fs::read_dir(&dir).expect("read examples directory") {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("utf-8 file name");

        // A directory is an example too: a package that checks itself, or
        // one DIRECTORIES_CHECKED_ELSEWHERE says where it is checked.
        if path.is_dir() {
            if name == "target" {
                continue;
            }
            seen += 1;
            let known = DIRECTORIES_CHECKED_ELSEWHERE
                .iter()
                .any(|(dir, _)| *dir == name)
                || checks_itself(&path);
            assert!(
                known,
                "examples/{name}/ checks nothing: give it an output test \
                 (`tests/NAME.stdout` or `tests/NAME.out/`, ADR-264 D13, ADR-247), \
                 or add the directory to DIRECTORIES_CHECKED_ELSEWHERE with the \
                 test that checks it"
            );
            continue;
        }

        if path.extension().and_then(|e| e.to_str()) != Some("nika") {
            continue;
        }
        seen += 1;

        let known = SPECIFICATION_LEVEL.contains(&name) || COVERED_ELSEWHERE.contains(&name);

        assert!(
            known,
            "examples/{name} is a loose file no test runs: make it a package with \
             an output test (ADR-264 D13), or add it to SPECIFICATION_LEVEL with its \
             gaps in examples/README.md"
        );
    }

    assert!(seen > 0, "no examples found in {}", dir.display());
}

/// **An example that is a package with output tests checks itself**
/// ([ADR-264](../../../docs/specification/adr/adr-264.md) D13): its expected
/// output is `tests/NAME.stdout` beside it, and whatever `test` blocks it
/// writes are its own.
fn checks_itself(dir: &Path) -> bool {
    dir.join("nikaia.toml").is_file()
        && std::fs::read_dir(dir.join("tests")).is_ok_and(|entries| {
            entries.filter_map(|entry| entry.ok()).any(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|x| x == "stdout" || x == "out")
            })
        })
}

/// **Every example package passes `nikaia test`, at both settings** - which
/// runs its output tests against the program and its `test` blocks against
/// the test build, and fails an outcome that differs between the settings
/// (ADR-264 D12). Run in a copy, because a project build writes `target/`
/// into the project, and the examples are the repository's.
#[test]
fn every_example_package_passes_its_own_tests() {
    let mut packages: Vec<PathBuf> = std::fs::read_dir(repo_root().join("examples"))
        .expect("read examples directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| checks_itself(path))
        .collect();
    packages.sort();
    assert!(packages.len() >= 11, "{packages:?}");
    for package in packages {
        let name = package
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .to_string();
        let copy = common::scratch_dir(&format!("example-package-{name}"));
        copy_tree(&package, &copy);
        let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .args(["test", "--both-settings", "--project"])
            .arg(&copy)
            .env(
                "NIKAIA_CACHE_DIR",
                repo_root().join("target").join("nikaia-project-tests"),
            )
            .env_remove("CARGO_TARGET_DIR")
            .output()
            .expect("the nikaia binary runs");
        assert!(
            out.status.success(),
            "examples/{name} fails its own tests:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        std::fs::remove_dir_all(&copy).ok();
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a directory");
    for entry in std::fs::read_dir(from).expect("read a directory") {
        let path = entry.expect("an entry").path();
        let name = path.file_name().expect("a name");
        if name == "target" {
            continue;
        }
        match path.is_dir() {
            true => copy_tree(&path, &to.join(name)),
            false => {
                std::fs::copy(&path, to.join(name)).expect("copy a file");
            }
        }
    }
}

/// Every file the specification-level list names must actually be there, so a
/// renamed or deleted example cannot leave a stale excuse behind.
#[test]
fn the_lists_name_files_that_exist() {
    for name in SPECIFICATION_LEVEL.iter().chain(COVERED_ELSEWHERE) {
        let path = repo_root().join("examples").join(name);
        assert!(path.exists(), "{} is listed but not there", path.display());
    }
    for (name, checked_by) in DIRECTORIES_CHECKED_ELSEWHERE {
        let path = repo_root().join("examples").join(name);
        assert!(path.is_dir(), "{} is listed but not there", path.display());
        // The excuse names a test file; that file has to be there too, or the
        // directory is unchecked and the list says otherwise.
        let file = checked_by.split(':').next().expect("a test file");
        let test = repo_root().join("crates/nikaia").join(file);
        assert!(
            test.exists(),
            "examples/{name}/ says it is checked by {checked_by}, which is not there"
        );
    }
}

/// A malformed line is a value the program can print, not a panic and not a
/// wrong summary. The example's `catch` is what turns it into one, and this is
/// the claim its comment makes.
#[test]
fn a_malformed_line_is_reported_and_no_summary_is_printed() {
    let (dir, binary) = build("access-log/src/main.nika", Build::default());

    // The status is two digits where the format fixes three.
    let log = dir.join("broken.log");
    std::fs::write(
        &log,
        "203.0.113.7 GET /index.html 200 5120\n198.51.100.4 GET /oops 20 512\n",
    )
    .expect("write the input");

    let run = Command::new(&binary)
        .arg(&log)
        .output()
        .expect("run the compiled example");

    assert!(run.status.success(), "it should report, not fail");
    assert!(
        String::from_utf8_lossy(&run.stdout).is_empty(),
        "a rejected file must not produce a summary: {}",
        String::from_utf8_lossy(&run.stdout)
    );

    // Line 2, at the third character of the status - the offset is in the
    // whole file, not in whichever piece a core was holding, and it is a line
    // and a column rather than a byte because `dsl … from …` renders the error
    // against the input it parsed.
    let reported = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(
        reported.contains("line 2"),
        "the message should say where: {reported}"
    );
    assert!(
        reported.contains("expected a digit"),
        "the message should say what was expected: {reported}"
    );
    assert!(
        reported.contains("in STATUS"),
        "the message should say which rule wanted it: {reported}"
    );

    // …and it shows the line, with a caret under the character it stopped at.
    // A position a reader has to go and look up is half a diagnostic, and the
    // program prints this where a person will see it.
    assert!(
        reported.contains("   2 | 198.51.100.4 GET /oops 20 512"),
        "the message should show the line: {reported}"
    );
    assert!(
        reported.lines().any(|l| l.trim_start().starts_with('^')),
        "the message should point at the character: {reported}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The label in `calc.nika`'s `factor`, end to end.
///
/// A Nikaia grammar says what a rule is called (`# "expression"`), the emitter
/// hands that to the backend in the backend's own spelling, the backend
/// reports the word instead of the three ways an operand can start, and the
/// driver renders it against the input the program parsed. Four pieces, one
/// message, and this is the only place all four are exercised together.
#[test]
fn a_missing_operand_is_reported_as_an_expression() {
    let (dir, binary) = build("calc/src/main.nika", Build::default());

    let run = Command::new(&binary)
        .arg("2 +")
        .output()
        .expect("run the compiled example");

    assert!(run.status.success(), "it should report, not fail");
    assert!(
        String::from_utf8_lossy(&run.stdout).is_empty(),
        "a rejected expression must not print a result: {}",
        String::from_utf8_lossy(&run.stdout)
    );

    let reported = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(
        reported.contains("expected expression"),
        "the label should be the expectation: {reported}"
    );
    assert!(
        reported.contains("column 4"),
        "and the position should be where the operand belongs: {reported}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Lower and compile. The example is the real file: it cannot drift.
fn build(file: &str, how: Build) -> (PathBuf, PathBuf) {
    let source_path = repo_root().join("examples").join(file);

    // **A package is a directory** (ADR-047 D1), so an example in a directory of
    // its own is a package and a loose one is not. `examples/` itself is a
    // directory of *programs*: twelve files each declaring `main`, filed
    // together, which is exactly what `nikaia lower` outside a project makes
    // of them.
    let program = match file.contains('/') {
        true => nikaia::modules::Program::read(&source_path),
        false => nikaia::modules::Program::read_one(&source_path),
    }
    .unwrap_or_else(|e| panic!("{file} does not read:\n{e:#}"));
    let lowered = program
        .emit(how)
        .unwrap_or_else(|e| panic!("{file} does not lower:\n{e:#}"));

    let dir = common::scratch_dir(&format!("example-{}", file.replace('.', "-")));
    let rust = dir.join("example.rs");
    std::fs::write(&rust, &lowered.rust).expect("write the emitted Rust");

    let binary = dir.join("example");
    let compiled = common::compile(
        &rust,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "{file} did not compile under {how:?}:\n{}\n--- emitted ---\n{}",
        String::from_utf8_lossy(&compiled.stderr),
        lowered.rust
    );

    (dir, binary)
}
