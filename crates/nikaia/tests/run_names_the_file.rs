//! **`nikaia run f.nika` says a backend refusal against `f.nika`** (#510).
//!
//! A file outside any project is built as a project of its own in the cache
//! (ADR-260 D4), and the language below sees the copy. Its refusal was mapped
//! to the right line (ADR-005 D7) of the wrong file: the copy's path, under
//! `~/.cache/nikaia/run/`, which no editor of the author's has open.

mod common;

use std::process::Command;

/// A program whose lowering `rustc` still refuses: `String::from` has no type
/// in the checker (#509), so the comparison beside a map's view is written as
/// it stands. When #509 is built this needs another such program, or none.
const REFUSED_BELOW: &str = "use std::collections\n\
     \n\
     fn same(m: ref collections::HashMap[String, String], k: ref String) -> bool {\n\
     \x20   let s = m[k] ?? return false\n\
     \x20   let other = String::from(\"x\")\n\
     \x20   return s == other\n\
     }\n\
     \n\
     fn main() {\n\
     \x20   let mut m: collections::HashMap[String, String] = collections::HashMap()\n\
     \x20   m.insert(\"a\", \"x\")\n\
     \x20   println(f\"{same(m, \"a\")}\")\n\
     }\n";

#[test]
fn a_backend_refusal_names_the_file_that_was_run() {
    let dir = common::scratch_dir("run-names-the-file");
    let input = dir.join("written.nika");
    std::fs::write(&input, REFUSED_BELOW).expect("write the source");
    let ran = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&dir)
        .args(["run", "--no-cache", "written.nika"])
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs");
    let said = String::from_utf8_lossy(&ran.stderr).to_string();
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        !ran.status.success(),
        "it was meant to be refused below:\n{said}"
    );
    let arrow = said
        .lines()
        .find(|line| line.trim_start().starts_with("-->"))
        .unwrap_or_else(|| panic!("no location in:\n{said}"));
    assert!(arrow.contains("written.nika:6:"), "{said}");
    assert!(
        !arrow.contains("/run/"),
        "the cache's copy is named:\n{said}"
    );
}

/// **And a panic while it runs names it too**: the abort table (ADR-300 D9)
/// is written by the build of the copy, under the name of the file run.
#[test]
fn a_panic_names_the_file_that_was_run() {
    let dir = common::scratch_dir("run-names-the-file-panic");
    let input = dir.join("stops.nika");
    std::fs::write(
        &input,
        "fn main() {\n    let xs = [1, 2, 3]\n    let i = 7\n    println(f\"{xs[i]}\")\n}\n",
    )
    .expect("write the source");
    let ran = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&dir)
        .args(["run", "--no-cache", "stops.nika"])
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs");
    let said = String::from_utf8_lossy(&ran.stderr).to_string();
    std::fs::remove_dir_all(&dir).ok();
    assert!(!ran.status.success(), "it was meant to stop:\n{said}");
    let line = said
        .lines()
        .find(|line| line.contains("the program stopped"))
        .unwrap_or_else(|| panic!("no stop in:\n{said}"));
    assert!(line.starts_with("stops.nika:4:"), "{said}");
}
