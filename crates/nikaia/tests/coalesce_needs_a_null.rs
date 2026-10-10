//! **`??` after a left side that cannot be `null`** (#342, ADR-279 D12, D13,
//! #476): after a list's index it is `NK1211`, with the length check as the
//! help; after any other value it is the warning `NK1216` and the line is its
//! left side; after an index into a list of `T?` it replaces a `null` element,
//! and an index past the end still stops.
use nikaia::check::CodeOps;

mod common;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

fn run(purpose: &str, source: &str) -> (bool, String, String) {
    let errors: Vec<_> = findings(source)
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust;
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    let _ = std::fs::remove_dir_all(&dir);
    (
        ran.status.success(),
        String::from_utf8_lossy(&ran.stdout).to_string(),
        String::from_utf8_lossy(&ran.stderr).to_string(),
    )
}

#[test]
fn a_list_index_of_a_value_is_refused_with_the_length_help() {
    let source = "fn third_is(xs: ref Vec[i64], n: i64) -> bool {\n\
         \x20   let third = xs[2] ?? return false\n\
         \x20   return third == n\n\
         }\n\nfn main() {}\n";
    let found = findings(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1211");
    assert!(
        found[0]
            .help
            .as_deref()
            .is_some_and(|h| h.contains("if 2 < xs.len()")),
        "{found:#?}"
    );
}

/// **A plain number builds, with one warning** (D13): the answer is the left
/// side, and the fallback is not written.
#[test]
fn a_plain_number_warns_and_is_its_left_side() {
    let source = "fn main() {\n    let x: i64 = 5\n    let y = x ?? 0\n    println(f\"{y}\")\n}\n";
    let found = findings(source);
    let codes: Vec<(&str, bool)> = found
        .iter()
        .map(|f| (f.code_str(), f.severity == nikaia::check::Severity::Warning))
        .collect();
    assert_eq!(codes, vec![("NK1216", true)], "{found:#?}");
    let (ran, out, _) = run("coalesce-plain", source);
    assert!(ran);
    assert_eq!(out, "5\n");
}

/// **A callee that narrowed `-> User?` to `-> User`** (D13): its callers'
/// `??` and `?.` build, with one warning each.
#[test]
fn a_narrowed_callee_does_not_break_its_callers() {
    let source = "struct User {\n    name: String,\n}\n\n\
         fn find(id: i64) -> User {\n    return User { name: f\"u{id}\" }\n}\n\n\
         fn main() {\n\
         \x20   let a = find(1) ?? User { name: \"-\".clone() }\n\
         \x20   let b = find(2)?.name\n\
         \x20   println(f\"{a.name} {b}\")\n\
         }\n";
    let codes: Vec<&str> = findings(source).iter().map(|f| f.code_str()).collect();
    assert_eq!(codes, vec!["NK1216", "NK1217"]);
    let (ran, out, _) = run("coalesce-narrowed", source);
    assert!(ran);
    assert_eq!(out, "u1 u2\n");
}

#[test]
fn a_list_of_optionals_replaces_a_null_and_an_index_past_the_end_still_stops() {
    let source = "fn main() {\n\
         \x20   let xs: Vec[i64?] = [1, null, 3]\n\
         \x20   let a = xs[1] ?? 0\n\
         \x20   let ys: Vec[String?] = [\"a\", null]\n\
         \x20   let shown = ys[1] ?? \"leer\"\n\
         \x20   println(f\"{a} {shown} {ys[0] ?? \"x\"}\")\n\
         \x20   let zs: Vec[i64] = [4, 5]\n\
         \x20   println(f\"{zs.first() ?? 0}\")\n\
         \x20   println(f\"{xs[5] ?? 0}\")\n\
         }\n";
    let (ok, out, err) = run("coalesce-list-of-optionals", source);
    assert!(!ok, "an index past the end stops the program");
    assert_eq!(out, "0 leer a\n4\n");
    assert!(err.contains("index"), "{err}");
}
