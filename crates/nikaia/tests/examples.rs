//! Every example that claims to run, compiled and run.
//!
//! `examples/README.md` divides the directory in two: files that the bootstrap
//! compiler handles today, and files written at specification level to find out
//! what the specification forgot. Only the first kind can be checked, and this
//! is where that check lives - lower the real file, compile the emitted Rust
//! with the `rustc` that built this test, run the binary, compare what it
//! printed.
//!
//! At **both settings of `user_parallelism`**, and the outputs must be equal.
//! That is the claim the switches rest on (Part I 1.2): the same source
//! compiles either way and means the same thing, only the runtime underneath
//! differs. A test that ran one setting would leave the interesting half
//! unchecked.
//!
//! **Most examples check themselves now**
//! ([ADR-245](../../../docs/specification/adr/adr-245.md) D8): eight are
//! packages whose expected output is `tests/NAME.stdout` beside them, and this
//! file runs `nikaia test --both-settings` in a copy of each. The table below
//! keeps the three whose result is a *file* they write, which an output test
//! does not see.
//!
//! `1brc.nika` has a test of its own (`one_brc.rs`), because it checks more
//! than its output. The last test here makes sure no example escapes both.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use nikaia::emit::Build;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// An example, and what running it must print.
struct Example {
    /// The file under `examples/`.
    file: &'static str,
    /// A file written into the scratch directory first. `{input}` in `args`
    /// stands for its path.
    input: Option<Input>,
    /// Fed to the program on **standard input**, for the examples that read it
    /// (ADR-019). A pipe is not a path, so this is not `input` with a flag.
    stdin: Option<&'static str>,
    args: &'static [&'static str],
    /// Compared after trimming, so a trailing newline is not a test.
    ///
    /// It may not name a path: the two builds run in scratch directories of
    /// their own, so an example that echoed one would print something
    /// different under each and `the_profiles_agree_on_every_example` would be
    /// right to say so.
    expected: &'static str,
    /// A file the example is expected to **write**. `{output}` in `args`
    /// stands for its path, and what it holds afterwards is compared here.
    ///
    /// Standard output is not the only thing a program produces, and an
    /// example whose result is a file would otherwise be checked by the one
    /// line it prints about it.
    wrote: Option<Output>,
}

struct Input {
    name: &'static str,
    contents: &'static str,
}

struct Output {
    name: &'static str,
    contents: &'static str,
}

/// What running an example produced.
struct Run {
    printed: String,
    /// The contents of the file it was expected to write.
    wrote: Option<String>,
}

/// The examples that run. An entry here is a promise that `cargo test` keeps.
const RUNNABLE: &[Example] = &[
    Example {
        file: "tally.nika",
        input: None,
        // A pipe, and the point of the example: the program never holds more
        // than one line, however long the stream is (ADR-025 D4).
        stdin: Some(
            "one\n\
             \n\
             the longest line in this stream\n\
             \n\
             three\n",
        ),
        args: &["{output}"],
        expected: "\
5 lines, 2 blank
longest: 31 characters
the longest line in this stream",
        // The run log it appends to, which is where Kap 5.1's `append: true`
        // ends up: a tab, because the call names `separator` too.
        wrote: Some(Output {
            name: "tally.log",
            contents: "5\t2",
        }),
    },
    Example {
        // The same program as `report.nika`, in three files (Part I, 9.1).
        // Having both is the point: what a module boundary buys and what it
        // costs, on a program small enough to hold in the head.
        file: "inventory/main.nika",
        input: Some(Input {
            name: "stock.csv",
            contents: "\
tools;Bolts & Nuts;12
paper;<plain> A4;40
tools;3\" Clamp;5
paper;Card </td>;7
",
        }),
        stdin: None,
        args: &["{input}", "{output}"],
        expected: "4 items, 64 in total",
        wrote: Some(Output {
            name: "report.html",
            contents: "\
<html>
        <head><title>Stock</title></head>
        <body>
        <h1>Stock</h1>
        <table>
        <tr><th>Category</th><th>Item</th><th>Count</th></tr>
        <tr><td>paper</td><td>&lt;plain&gt; A4</td><td>40</td></tr>\
<tr><td>tools</td><td>Bolts &amp; Nuts</td><td>12</td></tr>\
<tr><td>paper</td><td>Card &lt;/td&gt;</td><td>7</td></tr>\
<tr><td>tools</td><td>3&quot; Clamp</td><td>5</td></tr>
        </table>
        <p>64 in total</p>
        </body>
        </html>",
        }),
    },
    Example {
        file: "report.nika",
        input: Some(Input {
            name: "stock.csv",
            // Item names as people type them, which is where an escaping bug
            // comes from: an ampersand, a quote, an angle bracket, and one
            // that reads like a closing tag.
            contents: "\
tools;Bolts & Nuts;12
paper;<plain> A4;40
tools;3\" Clamp;5
paper;Card </td>;7
",
        }),
        stdin: None,
        args: &["{input}", "{output}"],
        // Path-free on purpose: what a run says on standard output has to be
        // the same under both builds, and the two run in scratch
        // directories of their own. The page is the deliverable and is
        // checked as one, below.
        expected: "4 items, 64 in total",
        wrote: Some(Output {
            name: "report.html",
            // Sorted by count, descending. Every one of the four characters
            // that changes what HTML means arrived from the *input file* and
            // is gone from the page: `&`, `<`, `>` and `"`. That is
            // ADR-017 D1 with something real to escape.
            contents: "\
<html>
        <head><title>Stock</title></head>
        <body>
        <h1>Stock</h1>
        <table>
        <tr><th>Category</th><th>Item</th><th>Count</th></tr>
        <tr><td>paper</td><td>&lt;plain&gt; A4</td><td>40</td></tr>\
<tr><td>tools</td><td>Bolts &amp; Nuts</td><td>12</td></tr>\
<tr><td>paper</td><td>Card &lt;/td&gt;</td><td>7</td></tr>\
<tr><td>tools</td><td>3&quot; Clamp</td><td>5</td></tr>
        </table>
        <p>64 in total</p>
        </body>
        </html>",
        }),
    },
];

/// Written at specification level: they say what the language is meant to look
/// like and the bootstrap compiler does not handle them yet. Each one's gaps
/// are listed in `examples/README.md`.
const SPECIFICATION_LEVEL: &[&str] = &["fortunes.nika"];

/// Runnable, but with a test of its own that checks more than the output.
const COVERED_ELSEWHERE: &[&str] = &["1brc.nika"];

/// Directories under `examples/` that no entry in `RUNNABLE` reaches into,
/// because their check lives in another test file. Each is named together with
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
fn every_runnable_example_prints_what_it_promises() {
    for example in RUNNABLE {
        for how in [Build::default(), Build::parallel()] {
            let run = build_and_run(example, how);
            assert_eq!(
                run.printed.trim(),
                example.expected,
                "{} under {how:?}",
                example.file
            );
        }
    }
}

/// An example whose result is a **file** is checked by the file.
///
/// `report.nika` prints one line about a page it wrote; checking that line
/// would check almost nothing. This reads the page back.
#[test]
fn every_example_that_writes_a_file_writes_the_right_one() {
    let mut checked = 0;
    for example in RUNNABLE.iter().filter(|e| e.wrote.is_some()) {
        let wrote = example.wrote.as_ref().expect("filtered above");
        for how in [Build::default(), Build::parallel()] {
            let run = build_and_run(example, how);
            assert_eq!(
                run.wrote.as_deref().map(str::trim),
                Some(wrote.contents),
                "{} wrote a different {} under {how:?}",
                example.file,
                wrote.name
            );
        }
        checked += 1;
    }
    assert!(checked > 0, "no example writes a file any more");
}

/// The two builds are one language, not two dialects: same source, same
/// output. Checked separately from the value above so a failure says which of
/// the two claims broke.
#[test]
fn the_switches_agree_on_every_example() {
    for example in RUNNABLE {
        let sequential = build_and_run(example, Build::default());
        let parallel = build_and_run(example, Build::parallel());
        assert_eq!(
            sequential.printed, parallel.printed,
            "{} prints differently between builds",
            example.file
        );
        assert_eq!(
            sequential.wrote, parallel.wrote,
            "{} writes a different file between builds",
            example.file
        );
    }
}

/// An example is either checked or declared unfinished. Adding a file to
/// `examples/` without deciding which is what this catches.
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

        // A directory is an example too. It is either a program spread over
        // several files, in which case some RUNNABLE entry reaches into it, or
        // a package, in which case PACKAGES says where it is checked.
        if path.is_dir() {
            if name == "target" {
                continue;
            }
            seen += 1;
            let prefix = format!("{name}/");
            let known = RUNNABLE.iter().any(|e| e.file.starts_with(&prefix))
                || DIRECTORIES_CHECKED_ELSEWHERE
                    .iter()
                    .any(|(dir, _)| *dir == name)
                || checks_itself(&path);
            assert!(
                known,
                "examples/{name}/ is in no list in tests/examples.rs and has no \
                 `tests/NAME.stdout`: give it an output test (ADR-245 D8), add its \
                 entry file to RUNNABLE with what it prints, or the directory to \
                 DIRECTORIES_CHECKED_ELSEWHERE with the test that checks it"
            );
            continue;
        }

        if path.extension().and_then(|e| e.to_str()) != Some("nika") {
            continue;
        }
        seen += 1;

        let known = RUNNABLE.iter().any(|e| e.file == name)
            || SPECIFICATION_LEVEL.contains(&name)
            || COVERED_ELSEWHERE.contains(&name);

        assert!(
            known,
            "examples/{name} is in neither list in tests/examples.rs: add it to \
             RUNNABLE with what it prints, or to SPECIFICATION_LEVEL with its gaps \
             in examples/README.md"
        );
    }

    assert!(seen > 0, "no examples found in {}", dir.display());
}

/// **An example that is a package with output tests checks itself**
/// ([ADR-245](../../../docs/specification/adr/adr-245.md) D8): its expected
/// output is `tests/NAME.stdout` beside it, and whatever `test` blocks it
/// writes are its own.
fn checks_itself(dir: &Path) -> bool {
    dir.join("nikaia.toml").is_file()
        && std::fs::read_dir(dir.join("tests")).is_ok_and(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.path().extension().is_some_and(|x| x == "stdout"))
        })
}

/// **Every example package passes `nikaia test`, at both settings** - which
/// runs its output tests against the program and its `test` blocks against
/// the test build, and fails an outcome that differs between the settings
/// (ADR-245 D7). Run in a copy, because a project build writes `target/`
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
    assert!(packages.len() >= 8, "{packages:?}");
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
    for example in RUNNABLE {
        let path = repo_root().join("examples").join(example.file);
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
    // together, which is exactly what the `--input` path outside a project makes
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

/// Lower, compile, run, and hand back what it produced.
fn build_and_run(example: &Example, how: Build) -> Run {
    let (dir, binary) = build(example.file, how);

    let input_path = example.input.as_ref().map(|input| {
        let path = dir.join(input.name);
        std::fs::write(&path, input.contents).expect("write the example's input");
        path
    });
    let output_path = example.wrote.as_ref().map(|wrote| dir.join(wrote.name));

    let args: Vec<String> = example
        .args
        .iter()
        .map(|arg| substitute(arg, input_path.as_deref(), output_path.as_deref()))
        .collect();

    let mut command = Command::new(&binary);
    command.args(&args);

    let run = match example.stdin {
        None => command.output().expect("run the compiled example"),
        Some(text) => {
            // A pipe, not a redirected file: what the program sees is what
            // ADR-019 is about.
            let mut child = command
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("spawn the compiled example");
            use std::io::Write;
            child
                .stdin
                .take()
                .expect("the child's stdin")
                .write_all(text.as_bytes())
                .expect("write to the child's stdin");
            child.wait_with_output().expect("run the compiled example")
        }
    };
    assert!(
        run.status.success(),
        "{} failed under {how:?}: {}",
        example.file,
        String::from_utf8_lossy(&run.stderr)
    );

    let printed = String::from_utf8_lossy(&run.stdout).into_owned();
    let wrote = output_path.as_ref().map(|path| {
        std::fs::read_to_string(path).unwrap_or_else(|e| {
            panic!(
                "{} said it wrote {}, and it is not there: {e}",
                example.file,
                path.display()
            )
        })
    });

    let _ = std::fs::remove_dir_all(&dir);
    Run { printed, wrote }
}

/// `{input}` stands for the path the example's input was written to, and
/// `{output}` for the path it is expected to write.
fn substitute(arg: &str, input: Option<&Path>, output: Option<&Path>) -> String {
    let mut arg = arg.to_string();
    for (name, path) in [("{input}", input), ("{output}", output)] {
        match path {
            Some(path) => arg = arg.replace(name, &path.display().to_string()),
            None => assert!(
                !arg.contains(name),
                "an argument names {name} but the example declares none"
            ),
        }
    }
    arg
}
