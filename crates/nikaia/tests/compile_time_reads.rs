//! **The files a build may read while it builds** —
//! [ADR-072](../../../docs/specification/adr/adr-072.md) and
//! [ADR-116](../../../docs/specification/adr/adr-116.md) D2.
//!
//! `comptime CONFIG: &str = asset("config.txt")` puts a file's text into the
//! program while it is built. Everything else here is about **which** files,
//! and the whole of that record is that the answer is hard to give by accident.
//!
//! **The first test is the one a project that never reads anything still
//! wants.** With no list in effect the class is off, so *this build reads
//! nothing while building* is what happens rather than a claim somebody has to
//! keep true. Every other refusal below is reached only by a build that already
//! asked for the whole thing to be switched on.
//!
//! **These tests write files and run the compiler**, because the feature is
//! about files: a test that only inspected the checker would pass for a build
//! that read the wrong one.

mod common;

use nikaia::assets::{Allowlist, Reads};
use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, STD};
use nikaia::parser::parse_to_ast;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A directory with the files this case needs, and the allowlist over them.
fn scratch(purpose: &str, files: &[(&str, &str)], listed: &[&str]) -> PathBuf {
    let dir = common::scratch_dir(purpose);
    for (name, body) in files {
        std::fs::write(dir.join(name), body).expect("write the asset");
    }
    let list = listed
        .iter()
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    std::fs::write(dir.join("reads.txt"), list).expect("write the allowlist");
    dir
}

fn reads(dir: &Path) -> Reads {
    Reads::with(
        dir,
        Allowlist::read(&dir.join("reads.txt")).expect("the list reads"),
    )
}

fn findings_reading(source: &str, reads: &Reads) -> Vec<Finding> {
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
    let mut found = findings_reading(source, reads);
    assert_eq!(found.len(), 1, "{found:#?}");
    found.remove(0)
}

const READS_IT: &str = "comptime CONFIG: ref String = asset(\"config.txt\")\n\
     \n\
     fn main() {\n\
     \x20   print(CONFIG)\n\
     }";

/// **A build given no list reads nothing** (D1), and that is the default.
///
/// The point of the test is the *default*: nothing was passed, so the whole
/// class is off. A project that never intends to read anything gets the
/// property without doing anything, which is what makes it a property rather
/// than a promise.
#[test]
fn a_build_given_no_list_reads_nothing() {
    let found = one(READS_IT, &Reads::none());
    assert_eq!(found.code, "NK1175");
    assert_eq!(found.message, "this build may not read `config.txt`");
    assert!(found.notes[0].contains("ADR-072 D1"), "{:#?}", found.notes);
    assert!(
        found
            .help
            .as_deref()
            .is_some_and(|help| help.contains("--allow-read-from-list")),
        "{:?}",
        found.help
    );
}

/// **Named in all three places, and the read happens** (D3, ADR-116 D2).
///
/// The flag is this test calling `Reads::with`, the list is `reads.txt`, and
/// the literal is the line. What comes back is the file's text, and it crosses
/// as the `&str` a `const` holds ([ADR-079](../../../docs/specification/adr/adr-079.md) D1).
#[test]
fn a_file_named_in_all_three_places_is_read() {
    let dir = scratch(
        "reads-allowed",
        &[("config.txt", "hello from the build\n")],
        &["config.txt"],
    );
    let reads = reads(&dir);
    assert!(
        findings_reading(READS_IT, &reads).is_empty(),
        "{:#?}",
        findings_reading(READS_IT, &reads)
    );
    // And what it read is what the key will carry (D7).
    let taken = reads.taken();
    assert_eq!(taken.len(), 1, "{taken:#?}");
    assert!(taken.contains_key("config.txt"));
    std::fs::remove_dir_all(&dir).ok();
}

/// **The bytes reach the program**, which is the half nothing above can say.
///
/// A file is read while the program is **built**, so what a reader sees is a
/// `const` holding its text — no open, no path, nothing at run time. This
/// compiles the lowering and runs it, because a test that matched the emitted
/// line would pass for a `const` holding the wrong file.
#[test]
fn what_the_build_read_is_in_the_program_that_runs() {
    let dir = scratch(
        "reads-runs",
        &[("config.txt", "hello from the build")],
        &["config.txt"],
    );
    let parsed = parse_to_ast(READS_IT).expect("the source parses");
    let rust = nikaia::emit::emit_program_reading(&parsed, Default::default(), &reads(&dir))
        .expect("it lowers")
        .rust;
    assert!(
        rust.contains("const CONFIG: &str = \"hello from the build\";"),
        "{rust}"
    );

    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run it");
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "hello from the build");
    std::fs::remove_dir_all(&dir).ok();
}

/// **A path the list does not name** (D3), and the way out is the list rather
/// than the flag: the three namings are not derivable from one another, so the
/// sentence has to say *which* of them is missing.
#[test]
fn a_path_the_list_does_not_name_is_refused() {
    let dir = scratch("reads-unlisted", &[("config.txt", "x")], &["other.txt"]);
    let found = one(READS_IT, &reads(&dir));
    assert_eq!(found.code, "NK1175");
    assert!(
        found.notes[0].contains("ADR-072 D3") && found.notes[0].contains("does not name"),
        "{:#?}",
        found.notes
    );
    assert!(
        found
            .help
            .as_deref()
            .is_some_and(|help| help.contains("on a line of")),
        "{:?}",
        found.help
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **A path that leaves the project root**, by `..` or by being absolute.
///
/// Checked on the **literal** and not on where a symlink points, for D4's
/// reason: what a reader can decide by looking at the line is the property the
/// three namings buy, and a path resolved behind their back is the opposite of
/// that. Both shapes are listed here, so the list is not what refuses them.
#[test]
fn a_path_that_leaves_the_root_is_refused() {
    for path in ["../secret.txt", "/etc/passwd"] {
        let dir = scratch("reads-escaping", &[], &[path]);
        let source = format!(
            "comptime A: ref String = asset(\"{path}\")\n\
             \n\
             fn main() {{ print(A) }}"
        );
        let found = one(&source, &reads(&dir));
        assert_eq!(found.code, "NK1175");
        assert!(
            found.notes[0].contains("under the project root"),
            "{:#?}",
            found.notes
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// **The literal may not be computed** (D4) — and this is the test that says
/// the refusal happens *before* the fold.
///
/// `NAME` folds to `"config.txt"`, which is a path the list names and a file
/// that is there: everything but the rule is satisfied. If the check ran on the
/// value there would be no finding at all, and *named in the code* would have
/// quietly stopped being decidable by looking at the line.
#[test]
fn a_path_the_build_works_out_is_refused_even_when_it_would_have_been_allowed() {
    let dir = scratch("reads-computed", &[("config.txt", "x")], &["config.txt"]);
    let source = "comptime NAME: ref String = \"config.txt\"\n\
         comptime A: ref String = asset(NAME)\n\
         \n\
         fn main() { print(A) }";
    let found = one(source, &reads(&dir));
    assert_eq!(found.code, "NK1176");
    assert_eq!(
        found.message,
        "`asset` takes a written path, and this one is worked out"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **Read, and not text.** What crosses to the program is a `&str`, so the
/// bytes have to be UTF-8 — and the refusal says that rather than letting a
/// lossy conversion put something else in the program.
#[test]
fn bytes_that_are_not_text_are_refused() {
    let dir = common::scratch_dir("reads-binary");
    std::fs::write(dir.join("bin.dat"), [0xff, 0xfe, 0x00]).expect("write it");
    std::fs::write(dir.join("reads.txt"), "bin.dat\n").expect("write the list");
    let source = "comptime A: ref String = asset(\"bin.dat\")\n\
         \n\
         fn main() { print(A) }";
    let found = one(source, &reads(&dir));
    assert_eq!(found.code, "NK1175");
    assert!(found.notes[0].contains("UTF-8"), "{:#?}", found.notes);
    std::fs::remove_dir_all(&dir).ok();
}

/// **`asset` stands in a `comptime` initialiser and nowhere else**
/// (ADR-116 D2), and the message names what a file read *while the program
/// runs* is called — which is what a reader who wrote it here almost certainly
/// meant.
#[test]
fn asset_outside_a_comptime_names_the_run_time_read() {
    let dir = scratch("reads-outside", &[("config.txt", "x")], &["config.txt"]);
    let source = "fn main() {\n\
         \x20   let text = asset(\"config.txt\")\n\
         \x20   print(text)\n\
         }";
    let found = one(source, &reads(&dir));
    assert_eq!(found.code, "NK1177");
    assert!(
        found
            .help
            .as_deref()
            .is_some_and(|help| help.contains("fs::read")),
        "{:?}",
        found.help
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **An entry nothing read** (D8). A list that may hold names nothing uses
/// decays into *everything we ever needed*, which is how an allowlist stops
/// being read.
#[test]
fn an_entry_nothing_read_is_named() {
    let dir = scratch(
        "reads-unused",
        &[("config.txt", "x")],
        &["config.txt", "never.txt"],
    );
    let reads = reads(&dir);
    assert!(findings_reading(READS_IT, &reads).is_empty());
    assert_eq!(reads.unused(), vec!["never.txt".to_string()]);
    std::fs::remove_dir_all(&dir).ok();
}

/// **The list is one path per line, `#` begins a comment** (D2).
///
/// No patterns and no directory listings (D5): a glob is a read of something
/// nobody named — the directory — and it would let an entry stand for files
/// that do not exist yet. Both halves defeat D3, so `*.toml` is a path like any
/// other and names a file called `*.toml`.
#[test]
fn the_list_is_lines_and_comments_and_nothing_else() {
    let entries = Allowlist::entries(
        "# the schema the driver checks against\n\
         schema.sql\n\
         \n\
         config/linux.toml   # one per target, because D4 has no patterns\n\
         *.toml\n",
    );
    assert_eq!(
        entries.iter().cloned().collect::<Vec<_>>(),
        vec![
            "*.toml".to_string(),
            "config/linux.toml".to_string(),
            "schema.sql".to_string(),
        ]
    );
}
