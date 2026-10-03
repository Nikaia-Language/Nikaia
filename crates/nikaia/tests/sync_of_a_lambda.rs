//! **`sync(f)`: a function pauses only where the lambda handed to `f` does**
//! ([ADR-288](../../../docs/specification/adr/adr-288.md) D31).
//!
//! The source form of the ledger's `sync = "sync(f)"`, which `std` has written by
//! hand for `map`, `filter` and the lock doors since ADR-288 D15. The body may
//! call `f` and nothing else that pauses; a caller is `sync` where the lambda it
//! hands over is; and the function is lowered as the `async fn` its body is, so
//! a caller that cannot pause drives it once and one that can awaits it.

mod common;

use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn runs(purpose: &str, source: &str, expected: &str) {
    for how in [Build::default(), Build::parallel()] {
        let parsed = parse_to_ast(source).expect("the source parses");
        let rust = emit_program(&parsed, how).expect("it lowers").rust;
        let dir = common::scratch_dir(&format!("sync-of-{purpose}"));
        let path = dir.join("program.rs");
        std::fs::write(&path, &rust).expect("write the Rust");
        let binary = dir.join("program");
        let compiled = common::compile(
            &path,
            &["--crate-type", "bin", "-o", &binary.to_string_lossy()],
        );
        assert!(
            compiled.status.success(),
            "{purpose} did not compile at {how:?}:\n{}\n--- emitted ---\n{rust}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let out = Command::new(&binary).output().expect("run it");
        assert!(out.status.success(), "{purpose} failed at {how:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            expected,
            "{purpose} at {how:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// What the compiler says about a file, as `nikaia lower` says it.
fn said(purpose: &str, source: &str) -> (bool, String, String) {
    let dir = common::scratch_dir(&format!("sync-of-said-{purpose}"));
    let path = dir.join("m.nika");
    std::fs::write(&path, source).expect("write the source");
    let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .arg("lower")
        .arg(&path)
        .current_dir(&dir)
        .output()
        .expect("run the compiler");
    let ledger = std::fs::read_to_string(dir.join("nikaia.contracts")).unwrap_or_default();
    std::fs::remove_dir_all(&dir).ok();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        ledger,
    )
}

const APPLY: &str = "fn apply(f: fn(i64) -> i64, x: i64) -> i64 sync(f) {\n\
                     \x20   return f(x)\n\
                     }\n";

/// **A `sync` caller hands it a lambda that does not pause**, and the call is
/// `sync` - where without the word it was `NK2202`, *`apply` can pause*. The
/// ledger records the promise as the source wrote it.
#[test]
fn a_sync_caller_may_call_it_with_a_lambda_that_does_not_pause() {
    let source = format!(
        "{APPLY}\
         fn twice(x: i64) -> i64 sync {{\n\
         \x20   return apply(fn(n) {{ n * 2 }}, x)\n\
         }}\n\
         \n\
         fn main() {{\n\
         \x20   println(f\"{{twice(21)}}\")\n\
         }}\n"
    );
    runs("sync-caller", &source, "42\n");
    let (ok, stderr, ledger) = said("sync-caller", &source);
    assert!(ok, "{stderr}");
    assert!(
        ledger.contains("[fn.\"apply\"]\nsync = \"sync(f)\""),
        "{ledger}"
    );
}

/// **A lambda that pauses makes the call pause**, and a caller that may pause
/// awaits it.
#[test]
fn a_lambda_that_pauses_makes_the_call_pause() {
    runs(
        "pausing-lambda",
        &format!(
            "use std::time\n\
             {APPLY}\
             fn main() {{\n\
             \x20   let r = apply(fn(n) {{\n\
             \x20       time::sleep(1.millis())\n\
             \x20       n * 2\n\
             \x20   }}, 21)\n\
             \x20   println(f\"{{r}}\")\n\
             }}\n"
        ),
        "42\n",
    );
}

/// **In a `sync` caller that lambda is refused**, at the pause, in the
/// caller's words - the lambda's body is the caller's (Part III 13.5).
#[test]
fn a_sync_caller_may_not_hand_it_a_lambda_that_pauses() {
    let (ok, stderr, _) = said(
        "sync-caller-pausing",
        &format!(
            "use std::time\n\
             {APPLY}\
             fn twice(x: i64) -> i64 sync {{\n\
             \x20   return apply(fn(n) {{\n\
             \x20       time::sleep(1.millis())\n\
             \x20       n * 2\n\
             \x20   }}, x)\n\
             }}\n\
             \n\
             fn main() {{\n\
             \x20   println(f\"{{twice(21)}}\")\n\
             }}\n"
        ),
    );
    assert!(!ok);
    assert!(
        stderr.contains(
            "error[NK2202]: `twice` is `sync`, but it calls `time::sleep`, which can pause."
        ),
        "{stderr}"
    );
}

/// **The body keeps the promise**: a pause that is not `f`'s is refused in
/// the promise's own words.
#[test]
fn a_body_that_pauses_elsewhere_breaks_the_promise() {
    let (ok, stderr, _) = said(
        "broken",
        "use std::time\n\
         fn apply(f: fn(i64) -> i64, x: i64) -> i64 sync(f) {\n\
         \x20   time::sleep(1.millis())\n\
         \x20   return f(x)\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{apply(fn(n) { n }, 1)}\")\n\
         }\n",
    );
    assert!(!ok);
    assert!(
        stderr.contains(
            "error[NK2202]: `apply` is `sync(f)`, but it calls `time::sleep`, which can pause."
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("help: Remove `sync(f)` from `apply`"),
        "{stderr}"
    );
}

/// **`sync(f)` names code the call runs** (`NK2210`): not a name that is no
/// parameter, and not a parameter that is not a function.
#[test]
fn the_promise_names_a_function_parameter() {
    for (named, why) in [
        ("g", "has no parameter `g`"),
        ("x", "`x` is not a function"),
    ] {
        let (ok, stderr, _) = said(
            &format!("names-{named}"),
            &format!(
                "fn apply(f: fn(i64) -> i64, x: i64) -> i64 sync({named}) {{\n\
                 \x20   return f(x)\n\
                 }}\n\
                 \n\
                 fn main() {{\n\
                 \x20   println(f\"{{apply(fn(n) {{ n }}, 1)}}\")\n\
                 }}\n"
            ),
        );
        assert!(!ok, "{named}");
        assert!(stderr.contains("NK2210"), "{named}: {stderr}");
        assert!(stderr.contains(why), "{named}: {stderr}");
    }
}

/// **A copy of a type this program declares does not pause**: its `clone` is
/// the derived one. `r.clone()` on a declared `struct` was answered as a
/// method nothing describes, and the function around it - `main` too - was
/// lowered `async` (found moving the ledger's records into Nikaia, ADR-294).
#[test]
fn a_clone_of_a_declared_type_is_sync() {
    let source = r#"pub struct Row {
    pub id: i64,
}

pub struct Holder {
    pub row: Row?,
    pub name: String?,
}

impl Holder {
    pub fn row_copy(ref self) -> Row {
        return self.row?.clone() ?? Row { id: 0 }
    }

    pub fn plain(ref self, r: ref Row) -> Row {
        return r.clone()
    }

    pub fn name_copy(ref self) -> String {
        return self.name?.clone() ?? "none"
    }
}

fn main() {
    let h = Holder { row: Row { id: 3 }, name: "n" }
    println(f"{h.row_copy().id} {h.name_copy()}")
}
"#;
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust;
    assert!(!rust.contains("async fn"), "{rust}");
    runs("declared-clone", source, "3 n\n");
}

/// **Nor does a copy of a number or a tuple**, for the same reason: no ledger
/// describes `i64::clone`, and answered as a method nothing describes, every
/// function around `at.clone()` was lowered `async` (found moving
/// `contracts::keep`'s walk into Nikaia, ADR-294).
#[test]
fn a_clone_of_a_number_or_a_tuple_is_sync() {
    let source = r#"fn first(pair: (i64, String)) -> i64 {
    let copy = pair.clone()
    return copy.0.clone()
}

fn main() {
    let at: i64 = 4
    let x: String = "x"
    let pair = (at.clone(), x)
    println(first(pair))
}
"#;
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust;
    assert!(!rust.contains("async fn"), "{rust}");
    runs("number-clone", source, "4\n");
}

/// **A set of pairs is taken apart as a list of them is**: `for (a, b) in
/// links` over a `BTreeSet[(String, String)]` bound two names of unknown type,
/// so `a.clone()` resolved to nothing and the function was lowered `async`
/// (found moving `text_tiers` into Nikaia, #125).
#[test]
fn a_for_over_a_set_of_pairs_is_sync() {
    let source = r#"use std::collections

fn firsts(links: ref collections::BTreeSet[(String, String)]) -> i64 {
    let mut out: collections::BTreeSet[String] = collections::BTreeSet()
    for (a, b) in links {
        out.insert(a.clone())
        out.insert(b.clone())
    }
    return out.len()
}

fn main() {
    let mut links: collections::BTreeSet[(String, String)] = collections::BTreeSet()
    let x: String = "x"
    let y: String = "y"
    links.insert((x, y))
    println(firsts(links))
}
"#;
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust;
    assert!(!rust.contains("async fn firsts"), "{rust}");
    runs("set-of-pairs", source, "2\n");
}
