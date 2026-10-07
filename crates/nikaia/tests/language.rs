//! **What the language means, tested in the language** (ADR-269).
//!
//! `tests/language` is one package, a file per topic, whose `test` blocks run
//! programs and look at what they computed. `nikaia test` builds it once per
//! setting and runs each test in a process of its own, where a test of this
//! crate used to call `rustc` and link `std` for each program on its own.
//!
//! **In place, not in a copy**: the package keeps one path, so its build is
//! Cargo's to reuse and a run with nothing changed compiles nothing.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn the_language_package_passes_its_own_tests() {
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/language");
    let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["test", "--both-settings", "--project"])
        .arg(&package)
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs");
    assert!(
        out.status.success(),
        "tests/language fails its own tests:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
