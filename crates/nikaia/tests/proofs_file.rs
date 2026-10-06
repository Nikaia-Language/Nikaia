//! **`nikaia.proofs`** ([ADR-270](../../../docs/specification/adr/adr-270.md)
//! D4, D19-D21, #448): a project build writes the prover's answers beside
//! `nikaia.contracts`, one line per question, and a build from the same
//! sources leaves the file as it was.

mod common;

use std::process::Command;

const SOURCE: &str = "fn half(n: i64) -> i64 {\n\
     \x20   assert(n >= 0)\n\
     \x20   return n / 2\n\
     }\n\
     \n\
     fn total(xs: ref Vec[i64]) -> i64 {\n\
     \x20   let mut sum = 0\n\
     \x20   for i in 0..<xs.len() {\n\
     \x20       sum = sum + xs[i]\n\
     \x20   }\n\
     \x20   return sum\n\
     }\n\
     \n\
     fn main() {\n\
     \x20   let xs = [1, 2, 3]\n\
     \x20   println(f\"{half(4)} {total(xs)}\")\n\
     }\n";

#[test]
fn a_build_records_the_provers_answers_and_a_second_one_keeps_them() {
    let dir = common::scratch_dir("proofs-file");
    std::fs::create_dir_all(dir.join("src")).expect("the package");
    std::fs::write(
        dir.join("nikaia.toml"),
        "[package]\nname = \"pf\"\nversion = \"0.1.0\"\n",
    )
    .expect("a manifest");
    std::fs::write(dir.join("src/main.nika"), SOURCE).expect("a source");
    let build = || {
        Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .current_dir(&dir)
            .args(["build", "--no-cache"])
            .output()
            .expect("the nikaia binary runs")
    };
    let first = build();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let written = std::fs::read_to_string(dir.join("nikaia.proofs")).expect("the file is written");
    let entries: Vec<&str> = written.lines().filter(|l| !l.starts_with('#')).collect();
    // The index `xs[i]` in a loop over `xs.len()` is proved, and the
    // `assert`'s question at `half(4)` is one of the others.
    assert!(entries.iter().any(|l| l.contains(" proved ")), "{written}");
    for line in &entries {
        let words: Vec<&str> = line.split(' ').collect();
        assert_eq!(words.len(), 3, "{line}");
        assert_eq!(words[0].len(), 64, "a SHA-256: {line}");
    }
    let mut sorted = entries.clone();
    sorted.sort();
    assert_eq!(entries, sorted, "sorted by key");

    let second = build();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let again = std::fs::read_to_string(dir.join("nikaia.proofs")).expect("still there");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(again, written);
}

/// **D22**: `--locked` builds from a file that answers every question, and
/// fails where one is missing or one is left that nothing asks - naming the
/// fix.
#[test]
fn locked_fails_on_a_file_that_does_not_answer_the_questions() {
    let dir = common::scratch_dir("proofs-locked");
    std::fs::create_dir_all(dir.join("src")).expect("the package");
    std::fs::write(
        dir.join("nikaia.toml"),
        "[package]\nname = \"pl\"\nversion = \"0.1.0\"\n",
    )
    .expect("a manifest");
    std::fs::write(dir.join("src/main.nika"), SOURCE).expect("a source");
    let build = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .current_dir(&dir)
            .args(args)
            .output()
            .expect("the nikaia binary runs")
    };
    let first = build(&["build", "--no-cache"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let written = std::fs::read_to_string(dir.join("nikaia.proofs")).expect("written");

    let locked = build(&["build", "--no-cache", "--locked"]);
    assert!(
        locked.status.success(),
        "{}",
        String::from_utf8_lossy(&locked.stderr)
    );

    // One answer gone: the build would have to search for it.
    let missing: String = {
        let mut dropped = false;
        written
            .lines()
            .filter(|line| {
                let drop = !dropped && !line.starts_with('#');
                dropped |= drop;
                !drop
            })
            .map(|line| format!("{line}\n"))
            .collect()
    };
    std::fs::write(dir.join("nikaia.proofs"), &missing).expect("rewritten");
    let refused = build(&["build", "--no-cache", "--locked"]);
    let stderr = String::from_utf8_lossy(&refused.stderr).to_string();
    assert!(!refused.status.success(), "{stderr}");
    assert!(stderr.contains("commit `nikaia.proofs`"), "{stderr}");
    // D22: the message names where the question was asked.
    assert!(stderr.contains("asked at src/main.nika"), "{stderr}");
    eprintln!("{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.join("nikaia.proofs")).expect("left alone"),
        missing,
        "--locked writes nothing"
    );

    // An entry nothing asks: the file would rot.
    std::fs::write(
        dir.join("nikaia.proofs"),
        format!("{written}{} unknown fourier-motzkin-1\n", "f".repeat(64)),
    )
    .expect("rewritten");
    let stale = build(&["build", "--no-cache", "--locked"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!stale.status.success());
    let stderr = String::from_utf8_lossy(&stale.stderr);
    assert!(stderr.contains("entries no question asked"), "{stderr}");
}
