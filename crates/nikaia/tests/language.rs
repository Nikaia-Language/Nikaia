//! **What the language means, tested in the language** (ADR-269).
//!
//! `tests/language` and `tests/build-time` are Nikaia packages, a file per
//! topic, whose `test` blocks run programs and look at what they computed.
//! `nikaia test --both-settings` builds each once per setting and runs each
//! test in a process of its own, where a test of this crate used to call
//! `rustc` and link `std` for each program on its own.
//!
//! **Two packages, not one**, because a value computed while a package is
//! built is a run over the whole package (#468): the topics with `comptime`s
//! and computed defaults are kept in a small package of their own.
//!
//! **In place, not in a copy**: each package keeps one path, so its builds are
//! Cargo's to reuse and a run with nothing changed compiles nothing.

use std::path::PathBuf;
use std::process::Command;

fn passes(package: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests")
        .join(package);
    let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["test", "--both-settings", "--project"])
        .arg(&dir)
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs");
    assert!(
        out.status.success(),
        "tests/{package} fails its own tests:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_language_package_passes_its_own_tests() {
    passes("language");
}

#[test]
fn the_build_time_package_passes_its_own_tests() {
    passes("build-time");
}
