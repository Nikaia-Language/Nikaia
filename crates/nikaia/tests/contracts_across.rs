//! **A contract crosses the package boundary**
//! ([ADR-269](../../../docs/specification/adr/adr-269.md) D18-D20): a package's
//! `assert`s are published in its ledger as `requires` and `ensures`, a
//! consumer proves the one and relies on the other, a call it does not prove
//! checks the precondition where it stands, and a contract that changes in the
//! breaking direction is said to its author.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const LIB: &str = "pub fn percent(part: i64, whole: i64) -> i64 {\n\
                   \x20   assert(whole > 0)\n\
                   \x20   return part * 100 / whole\n\
                   }\n\
                   \n\
                   pub fn clamp(n: i64) -> i64 {\n\
                   \x20   return 0 if n < 0\n\
                   \x20   assert(n >= 0)\n\
                   \x20   return n\n\
                   }\n\
                   \n\
                   pub fn shifted(x: i64, mode: i64) -> i64 {\n\
                   \x20   let y = x - 1\n\
                   \x20   if mode == 1 {\n\
                   \x20       assert(y > 0)\n\
                   \x20   }\n\
                   \x20   return y\n\
                   }\n\
                   \n\
                   fn main() {\n\
                   }\n";

const APP: &str = "use lib\n\
                   \n\
                   fn ratio(a: i64, b: i64) -> i64 {\n\
                   \x20   return lib::percent(a, b)\n\
                   }\n\
                   \n\
                   fn main() {\n\
                   \x20   let c = lib::clamp(0 - 3)\n\
                   \x20   assert(c >= 0)\n\
                   \x20   let total = 4\n\
                   \x20   println(f\"{lib::percent(1, total)} {c} {lib::shifted(5, 1)} {ratio(1, 2)}\")\n\
                   \x20   println(f\"{ratio(1, 0)}\")\n\
                   }\n";

/// A package `lib` and a program `app` that depends on it by path.
fn the_two(purpose: &str) -> PathBuf {
    let dir = common::scratch_dir(purpose);
    for (package, source, manifest) in [
        (
            "lib",
            LIB,
            "[package]\nname = \"lib\"\nversion = \"0.1.0\"\n",
        ),
        (
            "app",
            APP,
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nlib = { path = \"../lib\" }\n",
        ),
    ] {
        std::fs::create_dir_all(dir.join(package).join("src")).expect("the package");
        std::fs::write(dir.join(package).join("nikaia.toml"), manifest).expect("a manifest");
        std::fs::write(dir.join(package).join("src/main.nika"), source).expect("a source");
    }
    dir
}

fn nikaia(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(dir)
        .args(args)
        .env_remove("RUST_BACKTRACE")
        .output()
        .expect("the nikaia binary runs")
}

/// **D5, D7**: the ledger publishes the contract, the consumer proves what it
/// can - `lib::clamp`'s `result >= 0` proves `c >= 0`, and its calls with
/// numbers it knows take the unchecked entry - checks what it cannot where
/// the call stands, and the failure there names the precondition and the
/// caller's line.
#[test]
fn a_contract_is_published_proved_and_checked_across_packages() {
    let dir = the_two("contracts-across");
    let run = nikaia(&dir.join("app"), &["run"]);
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success(), "{stdout}{stderr}");
    assert_eq!(stdout, "25 0 4 50\n", "{stderr}");
    assert!(
        stderr.contains("main.nika:4")
            && stderr.contains("precondition of `lib::percent`: `whole > 0`")
            && stderr.contains("whole is 0"),
        "{stderr}"
    );

    let ledger = std::fs::read_to_string(dir.join("lib/nikaia.contracts")).expect("the ledger");
    for line in [
        "requires = [\"whole > 0\"]",
        "ensures = [\"result >= 0\"]",
        "requires = [\"!(mode == 1) || x - 1 > 0\"]",
        "from = [\"assert(y > 0)\"]",
    ] {
        assert!(ledger.contains(line), "{line}\n{ledger}");
    }

    let lowered = dir.join("app.rs");
    let out = nikaia(
        &dir.join("app"),
        &[
            "lower",
            "src/main.nika",
            "--output",
            &lowered.to_string_lossy(),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rust = std::fs::read_to_string(&lowered).expect("the lowering");
    assert!(rust.contains("lib::percent__unchecked(1, total)"), "{rust}");
    assert!(rust.contains("lib::shifted__unchecked(5, 1)"), "{rust}");
    assert!(
        rust.contains("(if !((b.clone() as i128) > (0i128))"),
        "{rust}"
    );
    // `c >= 0` is proved from what `lib::clamp` ensures: no check is left.
    assert!(!rust.contains("assert(c >= 0)"), "{rust}");
    std::fs::remove_dir_all(&dir).ok();
}

/// **D6**: a precondition made stronger or a postcondition made weaker is
/// said to the package's author against the committed ledger; the opposite
/// direction is not, and once the new ledger is written it is not said again.
#[test]
fn a_contract_changed_in_the_breaking_direction_is_said() {
    let dir = the_two("contracts-changed");
    let lib = dir.join("lib");
    let first = nikaia(&lib, &["build"]);
    assert!(
        !String::from_utf8_lossy(&first.stderr).contains("NK1208"),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let stronger = LIB
        .replace("assert(whole > 0)", "assert(whole > 1)")
        .replace("    assert(n >= 0)", "    assert(n >= 0 - 1)");
    std::fs::write(lib.join("src/main.nika"), stronger).expect("the change");
    let changed = String::from_utf8_lossy(&nikaia(&lib, &["build"]).stderr).to_string();
    assert!(
        changed.contains(
            "warning[NK1208]: `percent` asks more of its callers than the committed ledger says."
        ) && changed.contains("it required `whole > 0`, and now requires `whole > 1`.")
            && changed
                .contains("warning[NK1208]: `clamp` promises less than the committed ledger says."),
        "{changed}"
    );
    let again = String::from_utf8_lossy(&nikaia(&lib, &["build"]).stderr).to_string();
    assert!(!again.contains("NK1208"), "{again}");

    // Back the other way: weaker requires, stronger ensures - nothing to say.
    std::fs::write(lib.join("src/main.nika"), LIB).expect("the change back");
    let back = String::from_utf8_lossy(&nikaia(&lib, &["build"]).stderr).to_string();
    assert!(!back.contains("NK1208"), "{back}");
    std::fs::remove_dir_all(&dir).ok();
}
