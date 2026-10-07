//! **`nikaia.proofs`** ([ADR-270](../../../docs/specification/adr/adr-270.md)
//! D4, D19-D21, #448): a project build writes the prover's answers beside
//! `nikaia.contracts`, one line per question, and a build from the same
//! sources leaves the file as it was.

mod common;

use std::path::Path;
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

/// The keys of a `nikaia.proofs`.
fn keys(file: &Path) -> std::collections::BTreeSet<String> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split(' ').next().map(str::to_string))
        .collect()
}

/// **D23**: a dependency's own questions are in its own `nikaia.proofs`, not
/// in the program's that uses it.
#[test]
fn a_dependencys_questions_are_its_own() {
    let root = common::scratch_dir("proofs-dependency");
    let package = root.join("sums");
    std::fs::create_dir_all(package.join("src")).expect("the package");
    std::fs::write(
        package.join("nikaia.toml"),
        "[package]\nname = \"sums\"\nversion = \"0.1.0\"\n",
    )
    .expect("a manifest");
    std::fs::write(
        package.join("src/lib.nika"),
        "pub fn total(xs: ref Vec[i64]) -> i64 {\n\
         \x20   let mut sum = 0\n\
         \x20   for i in 0..<xs.len() {\n\
         \x20       sum = sum + xs[i]\n\
         \x20   }\n\
         \x20   return sum\n\
         }\n",
    )
    .expect("a source");
    let program = root.join("app");
    std::fs::create_dir_all(program.join("src")).expect("the program");
    std::fs::write(
        program.join("nikaia.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nsums = { path = \"../sums\" }\n",
    )
    .expect("a manifest");
    std::fs::write(
        program.join("src/main.nika"),
        "use sums\n\nfn main() {\n    let xs = [1, 2, 3]\n    println(f\"{sums::total(xs)}\")\n}\n",
    )
    .expect("a source");
    let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&program)
        .args(["build", "--no-cache"])
        .output()
        .expect("the nikaia binary runs");
    let theirs = keys(&package.join("nikaia.proofs"));
    let ours = keys(&program.join("nikaia.proofs"));
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !theirs.is_empty(),
        "the package's own file holds its questions"
    );
    assert!(
        theirs.is_disjoint(&ours),
        "the program's file holds the package's questions: {:?}\n{}",
        theirs.intersection(&ours).collect::<Vec<_>>(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// **The committed file is part of the cache key** (#448): a build that hits
/// the cache asks nothing, so an edited `nikaia.proofs` has to be a miss - here
/// the entry taken out of it is asked, and written, again.
#[test]
fn an_edited_proof_file_is_a_new_build() {
    let dir = common::scratch_dir("proofs-file-key");
    std::fs::create_dir_all(dir.join("src")).expect("the package");
    std::fs::write(
        dir.join("nikaia.toml"),
        "[package]\nname = \"pk\"\nversion = \"0.1.0\"\n",
    )
    .expect("a manifest");
    std::fs::write(dir.join("src/main.nika"), SOURCE).expect("a source");
    let build = || {
        let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .current_dir(&dir)
            .arg("build")
            .output()
            .expect("the nikaia binary runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    build();
    build();
    let file = dir.join("nikaia.proofs");
    let whole = std::fs::read_to_string(&file).expect("the file is written");
    let taken = whole
        .lines()
        .find(|l| !l.starts_with('#'))
        .expect("an entry")
        .to_string();
    let without: String = whole
        .lines()
        .filter(|l| *l != taken)
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&file, without).expect("edit the file");
    build();
    let again = std::fs::read_to_string(&file).expect("the file");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        again.lines().any(|l| l == taken),
        "the entry was not asked again:\n{again}"
    );
}
