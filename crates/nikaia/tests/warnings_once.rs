//! **A warning is said once** (#481): `nikaia run` with a file lowers it where
//! it stands and then builds a copy, and a project build lowers every member
//! before the wrapper lowers it again. Each of those used to print it.

mod common;

use std::process::Command;

const SOURCE: &str = "fn main() {\n    let x = 1\n    println(\"{x}\")\n}\n";

fn said(output: &std::process::Output) -> usize {
    String::from_utf8_lossy(&output.stderr)
        .matches("warning[NK1111]")
        .count()
}

#[test]
fn a_file_run_says_its_warning_once() {
    let dir = common::scratch_dir("warnings-once-file");
    let file = dir.join("w.nika");
    std::fs::write(&file, SOURCE).expect("a source");
    let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["--no-cache", "run"])
        .arg(&file)
        .output()
        .expect("the nikaia binary runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(said(&out), 1, "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_project_build_says_its_warning_once() {
    let dir = common::scratch_dir("warnings-once-project");
    std::fs::create_dir_all(dir.join("src")).expect("the package");
    std::fs::write(
        dir.join("nikaia.toml"),
        "[package]\nname = \"wo\"\nversion = \"0.1.0\"\n",
    )
    .expect("a manifest");
    std::fs::write(dir.join("src/main.nika"), SOURCE).expect("a source");
    let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&dir)
        .args(["build", "--no-cache"])
        .output()
        .expect("the nikaia binary runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(said(&out), 1, "{}", String::from_utf8_lossy(&out.stderr));
}
