//! **`assert`, a claim** ([ADR-245](../../../docs/specification/adr/adr-245.md)
//! D2, D3): a prelude function the compiler knows, whose condition changes
//! nothing. Outside a test it is proved while the program is built
//! ([ADR-256](../../../docs/specification/adr/adr-256.md)); a program runs
//! here at both settings of `user_parallelism`.

mod common;

use std::process::{Command, Output};

use nikaia::contracts::{Ledger, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

/// Through the command line, as a user runs it: the file has a name, so an
/// abort names the `.nika` line (ADR-044) - which is half of what D2 says a
/// false claim reports.
fn run(purpose: &str, source: &str, parallel: &str) -> Output {
    let dir = common::scratch_dir(&format!("assert-{purpose}"));
    let input = dir.join("claims.nika");
    std::fs::write(&input, source).expect("write the source");
    let path = dir.join("claims.rs");
    let lowered = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&dir)
        .args(["--input", "claims.nika", "--output", "claims.rs"])
        .args(["--user-parallelism", parallel])
        .output()
        .expect("the nikaia binary runs");
    assert!(
        lowered.status.success(),
        "{purpose} did not lower:\n{}",
        String::from_utf8_lossy(&lowered.stderr)
    );
    let binary = dir.join("claims");
    let compiled = common::compile(
        &path,
        &["--crate-type", "bin", "-o", &binary.to_string_lossy()],
    );
    assert!(
        compiled.status.success(),
        "{purpose} did not compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary)
        .env_remove("RUST_BACKTRACE")
        .output()
        .expect("run it");
    std::fs::remove_dir_all(&dir).ok();
    out
}

/// Runs at both settings and hands back what the program printed to each
/// stream, having checked it is the same at both.
fn outcome(purpose: &str, source: &str) -> (bool, String, String) {
    let found = findings(source);
    assert!(found.is_empty(), "{purpose}: {found:#?}");
    let mut seen = Vec::new();
    for parallel in ["no", "yes"] {
        let out = run(purpose, source, parallel);
        seen.push((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        ));
    }
    assert_eq!(seen[0], seen[1], "{purpose}: the two settings disagree");
    seen.remove(0)
}

fn refused(source: &str, code: &str) -> Vec<String> {
    let found: Vec<_> = findings(source)
        .into_iter()
        .filter(|f| f.code == code)
        .collect();
    assert!(!found.is_empty(), "no {code} for:\n{source}");
    found
        .into_iter()
        .map(|f| format!("{} | {}", f.message, f.notes.join(" | ")))
        .collect()
}

/// **Claims the prover holds change nothing**
/// ([ADR-256](../../../docs/specification/adr/adr-256.md) D1): the program
/// runs to its end, at both settings, and no check was written for any of
/// them.
#[test]
fn claims_the_compiler_proves_let_the_program_run() {
    let source = "fn clamp(n: i64) -> i64 sync {\n\
                  \x20   return 0 if n < 0\n\
                  \x20   assert(n >= 0)\n\
                  \x20   return n\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let blank = 2\n\
                  \x20   assert(blank + 1 == 3)\n\
                  \x20   for i in 0..<3 {\n\
                  \x20       assert(i < 3 && !(i < 0))\n\
                  \x20   }\n\
                  \x20   println(f\"all held {clamp(0 - 4)}\")\n\
                  }\n";
    let (ok, stdout, stderr) = outcome("hold", source);
    assert!(ok, "{stderr}");
    assert_eq!(stdout, "all held 0\n");
}

/// **A false claim never reaches the program** (ADR-256 D1): it is refused
/// while the program is built. What a false claim says when it *runs* is a
/// test's (`nikaia_test.rs`), the one place an `assert` is still a check.
#[test]
fn a_false_claim_is_refused_before_the_program_runs() {
    let found = refused(
        "fn main() {\n\
         \x20   let blank = 2\n\
         \x20   assert(blank + 1 == 4; message: \"an empty line is blank\")\n\
         }\n",
        "NK1202",
    );
    assert!(
        found[0].starts_with("The compiler can't prove `blank + 1 == 4`."),
        "{found:?}"
    );
}

/// **The condition changes nothing** (D3): a call that touches output, one
/// that pauses, one that changes its receiver and one nothing describes are
/// each `NK1194`, named.
#[test]
fn a_condition_that_could_change_something_is_refused() {
    let noisy = refused(
        "fn noisy() -> bool {\n\
         \x20   println(\"said\")\n\
         \x20   return true\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   assert(noisy())\n\
         }\n",
        "NK1194",
    );
    assert!(noisy[0].contains("`noisy` touches"), "{noisy:?}");
    let pauses = refused(
        "use std::fs\n\
         \n\
         fn main() throws {\n\
         \x20   assert(fs::read_to_string(\"x\"; root: fs::Root::Anywhere) == \"\")\n\
         }\n",
        "NK1194",
    );
    assert!(pauses[0].contains("can pause"), "{pauses:?}");
    let changes = refused(
        "fn main() {\n\
         \x20   let mut xs: Vec[i64] = Vec()\n\
         \x20   assert(xs.push(1) == xs.push(2))\n\
         }\n",
        "NK1194",
    );
    assert!(
        changes[0].contains("changes what it is called on"),
        "{changes:?}"
    );
    let unknown = refused(
        "fn main() {\n\
         \x20   assert(frobnicate(1))\n\
         }\n",
        "NK1194",
    );
    assert!(
        unknown[0].contains("knows nothing about `frobnicate`"),
        "{unknown:?}"
    );
}

/// **One condition, one option** (D2): `NK1195` for anything else.
#[test]
fn an_assert_of_another_shape_is_refused() {
    let shapes = [
        ("assert(5)", "a literal of another type"),
        ("assert(true; msg: \"x\")", "no option called `msg`"),
        ("assert(true, false)", "hands it 2"),
        ("assert(true; message: 3)", "`message:` has to be text"),
    ];
    for (call, says) in shapes {
        let found = refused(&format!("fn main() {{\n    {call}\n}}\n"), "NK1195");
        assert!(found[0].contains(says), "{call}: {found:?}");
    }
}

/// **A function of the program's own called `assert` is that function**: the
/// prelude's is the one nothing else declared.
#[test]
fn a_program_may_declare_its_own_assert() {
    let source = "fn assert(ok: bool) {\n\
                  \x20   println(f\"mine: {ok}\")\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   assert(1 == 2)\n\
                  }\n";
    let (ok, stdout, stderr) = outcome("own", source);
    assert!(ok, "{stderr}");
    assert_eq!(stdout, "mine: false\n");
}

/// **`--asserts` names every claim and how it is held** (D6): proved, or a
/// precondition its callers prove (ADR-256 D3).
#[test]
fn asserts_reports_every_claim() {
    let source = "fn half(n: i64) -> i64 sync {\n\
                  \x20   assert(n >= 0; message: \"not negative\")\n\
                  \x20   return n / 2\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let k = half(4)\n\
                  \x20   let four = 4\n\
                  \x20   assert(four == 4)\n\
                  \x20   println(f\"{k}\")\n\
                  }\n";
    let dir = common::scratch_dir("assert-report");
    std::fs::write(dir.join("claims.nika"), source).expect("write");
    let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&dir)
        .args([
            "--input",
            "claims.nika",
            "--output",
            "claims.rs",
            "--asserts",
        ])
        .output()
        .expect("the nikaia binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains(
            "asserts in claims.nika: 2 - proved 1, preconditions 1, checked by a test 0, \
             refused 0\n\
             \x20 claims.nika:2  assert(n >= 0; message: \"not negative\")  precondition of \
             `half`: its callers prove it\n\
             \x20 claims.nika:9  assert(four == 4)  proved\n"
        ),
        "{stdout}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **No `Ok(())` after a statement that never comes back** (0.0.240): a
/// `throws` function ending in `panic(…)` or `return` was lowered with an
/// `Ok(())` after it, and `rustc` warned *unreachable expression* about a file
/// nobody wrote. `nikaia test`'s own entry ends that way, which is where it
/// was found.
#[test]
fn a_body_that_leaves_is_not_followed_by_ok() {
    let source = "fn check(n: i64) throws {\n\
                  \x20   if n > 0 {\n\
                  \x20       return\n\
                  \x20   }\n\
                  \x20   panic(\"negative\")\n\
                  }\n\
                  \n\
                  fn done() throws {\n\
                  \x20   println(\"done\")\n\
                  \x20   return\n\
                  }\n\
                  \n\
                  fn main() throws {\n\
                  \x20   check(1)\n\
                  \x20   done()\n\
                  }\n";
    let dir = common::scratch_dir("assert-leaves");
    std::fs::write(dir.join("leaves.nika"), source).expect("write");
    let lowered = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&dir)
        .args(["--input", "leaves.nika", "--output", "leaves.rs"])
        .output()
        .expect("the nikaia binary runs");
    assert!(
        lowered.status.success(),
        "{}",
        String::from_utf8_lossy(&lowered.stderr)
    );
    let compiled = common::compile(
        &dir.join("leaves.rs"),
        &[
            "--crate-type",
            "bin",
            "-o",
            &dir.join("leaves").to_string_lossy(),
        ],
    );
    let said = String::from_utf8_lossy(&compiled.stderr);
    assert!(compiled.status.success(), "{said}");
    assert!(!said.contains("unreachable"), "{said}");
    std::fs::remove_dir_all(&dir).ok();
}
