//! **Build-time code calls its package's Nikaia and its Nikaia dependencies**
//! ([ADR-321](../../../docs/specification/adr/adr-321.md) D13, #479): a
//! `comptime` and a grammar action at build time reach a dependency's
//! functions, compiled into the build-time program as the module the program
//! names the package by; a path to C is refused, and nothing runs.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A program `app` that depends on a package `lib`, with these sources.
fn projects(purpose: &str, lib: &str, app: &str) -> PathBuf {
    let root = common::scratch_dir(purpose);
    std::fs::create_dir_all(root.join("lib/src")).expect("the package");
    std::fs::write(
        root.join("lib/nikaia.toml"),
        "[package]\nname = \"lib\"\nversion = \"0.1.0\"\n",
    )
    .expect("a manifest");
    std::fs::write(root.join("lib/src/lib.nika"), lib).expect("a source");
    std::fs::create_dir_all(root.join("app/src")).expect("the program");
    std::fs::write(
        root.join("app/nikaia.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nlib = { path = \"../lib\" }\n",
    )
    .expect("a manifest");
    std::fs::write(root.join("app/src/main.nika"), app).expect("a source");
    root
}

fn run(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(root.join("app"))
        .args(["run", "--no-cache"])
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs")
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const LIB: &str = "pub fn tripled(n: i64) -> i64 sync {\n    return n * 3\n}\n";

/// A `comptime` that calls a dependency's function is computed while the
/// program is built - it was refused as *can pause*, against a ledger that
/// had no entry for the dependency.
#[test]
fn a_comptime_calls_a_dependency() {
    let root = projects(
        "build-time-dependency-comptime",
        LIB,
        "use lib\n\ncomptime N: i64 = lib::tripled(14)\n\nfn main() {\n    println(f\"{N}\")\n}\n",
    );
    let out = run(&root);
    let generated =
        std::fs::read_to_string(root.join("app/target/nikaia/gen/app/app.rs")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&root);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "42");
    assert!(generated.contains("const N: i64 = 42;"), "{generated}");
}

/// A grammar's action at build time calls a helper of its own file and a
/// function of a dependency.
#[test]
fn a_grammar_action_calls_a_dependency_at_build_time() {
    let root = projects(
        "build-time-dependency-grammar",
        LIB,
        "use lib\n\n\
         fn plus_one(n: i64) -> i64 {\n    return n + 1\n}\n\n\
         grammar Num {\n    entry rule n -> i64 = d:dec[i64](digit+) { lib::tripled(plus_one(d)) }\n}\n\n\
         comptime N: i64 = Num::n(\"13\")\n\n\
         fn main() {\n    println(f\"{N}\")\n}\n",
    );
    let out = run(&root);
    let generated =
        std::fs::read_to_string(root.join("app/target/nikaia/gen/app/app.rs")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&root);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "42");
    assert!(generated.contains("const N: i64 = 42;"), "{generated}");
}

/// A dependency's function whose path reaches C may not run while the program
/// is built (D13, D15).
#[test]
fn a_dependency_that_reaches_c_is_refused_at_build_time() {
    let root = projects(
        "build-time-dependency-c",
        "extern {\n    fn abs(n: i32) -> i32\n}\n\n\
         pub fn tripled(n: i64) -> i64 {\n    return n * 3 + unsafe { abs(0) } as i64\n}\n",
        "use lib\n\ncomptime N: i64 = lib::tripled(14)\n\nfn main() {\n    println(f\"{N}\")\n}\n",
    );
    let out = run(&root);
    let _ = std::fs::remove_dir_all(&root);
    assert!(!out.status.success(), "{}", said(&out));
    assert!(said(&out).contains("NK1152"), "{}", said(&out));
}
