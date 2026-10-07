//! **A function named as a value has its declaration's type** (#502, #507):
//! `double`, a type's name for its anonymous constructor (`P`), and a method
//! (`P::merge`), whose first parameter is the receiver. A wrong use is
//! refused here; a right one still runs.

mod common;

use nikaia::check::Finding;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;
use std::process::Command;

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = common::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    common::checked(&parsed, &own, &library).findings
}

const DECLARED: &str = "struct P { x: i64 }\n\
     impl P {\n\
     \x20   fn() -> P { return P { x: 1 } }\n\
     \x20   fn merge(ref mut self, other: P) { self.x = self.x + other.x }\n\
     }\n\
     fn double(n: i64) -> i64 sync { return n * 2 }\n\
     fn apply(f: fn(i64) -> i64 sync, x: i64) -> i64 sync { return f(x) }\n\
     fn make(g: fn() -> P sync) -> P sync { return g() }\n";

#[test]
fn a_function_named_as_a_value_is_typed() {
    for (value, ty) in [
        ("double", "fn(i64) -> i64 sync"),
        ("P", "fn() -> P sync"),
        ("P::merge", "fn(ref P, P) sync"),
    ] {
        let source = format!(
            "{DECLARED}fn main() {{\n    let wrong: bool = {value}\n    println(\"x\")\n}}\n"
        );
        let found = findings(&source);
        assert!(
            found
                .iter()
                .any(|f| f.code == "NK1103" && f.message.contains(&format!("`{ty}`"))),
            "{value}: {found:#?}"
        );
    }
}

/// Passed straight to a parameter that runs it, a declared function is passed
/// as it is - the `&*` a kept closure gets would not compile (#502).
#[test]
fn a_declared_function_passed_as_an_argument_runs() {
    let dir = common::scratch_dir("function-values");
    std::fs::write(
        dir.join("passed.nika"),
        format!(
            "{DECLARED}fn main() {{\n    println(f\"{{apply(double, 5)}} {{make(P).x}}\")\n}}\n"
        ),
    )
    .expect("write the source");
    let ran = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&dir)
        .args(["run", "--no-cache", "passed.nika"])
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs");
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "10 1");
}

/// **A tuple's position it does not have is `NK1107`** (#515), where it
/// reached `rustc` as *no field `2`*; the help names the ones it has.
#[test]
fn a_tuple_position_it_does_not_have_is_refused() {
    let found = findings(
        "fn main() {\n    let t = (\"a\", 2)\n    let u = t.2\n    println(f\"{u}\")\n}\n",
    );
    let refused = found
        .iter()
        .find(|f| f.code == "NK1107")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert_eq!(refused.message, "This tuple has no part `.2`.");
    assert!(
        refused
            .help
            .as_deref()
            .unwrap_or("")
            .contains("`.0` to `.1`")
    );
}

/// **`.collect()` after a list's `map` is `NK1210`** (#517): the `map` is a
/// list already, and the call reached `rustc`.
#[test]
fn a_list_is_not_collected() {
    let found = findings(
        "fn double(n: i64) -> i64 sync { return n * 2 }\n\
         fn main() {\n    let ys = [1, 2, 3].map(double).collect()\n    println(f\"{ys.len()}\")\n}\n",
    );
    let refused = found
        .iter()
        .find(|f| f.code == "NK1210")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert_eq!(refused.help.as_deref(), Some("Leave out `.collect()`."));
}

/// **A parameter that may pause runs a declared function either way** (#516):
/// one that pauses (`slow`) and one that never does (`double`), which the
/// lowering hands over inside an `async` closure.
#[test]
fn a_parameter_that_may_pause_runs_either_kind_of_function() {
    let dir = common::scratch_dir("function-values-pause");
    std::fs::write(
        dir.join("either.nika"),
        "use std::time\n\
         \n\
         fn slow(n: i64) -> i64 {\n    time::sleep(1.millis())\n    return n + 1\n}\n\
         fn double(n: i64) -> i64 { return n * 2 }\n\
         fn apply(f: fn(i64) -> i64, x: i64) -> i64 { return f(x) }\n\
         fn main() {\n    println(f\"{apply(slow, 5)} {apply(double, 5)}\")\n}\n",
    )
    .expect("write the source");
    let ran = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&dir)
        .args(["run", "--no-cache", "either.nika"])
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs");
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "6 10");
}
