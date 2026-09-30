//! **Every `assert` outside a test is proved while the program is built**
//! ([ADR-256](../../../docs/specification/adr/adr-256.md)), or the program is
//! refused. A proved claim leaves nothing behind in the emitted Rust.

mod common;

use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
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

fn codes(source: &str) -> Vec<&'static str> {
    findings(source).iter().map(|f| f.code).collect()
}

fn the_one(source: &str, code: &str) -> nikaia::check::Finding {
    let found = findings(source);
    let mut matching: Vec<_> = found.iter().filter(|f| f.code == code).cloned().collect();
    assert_eq!(matching.len(), 1, "{found:#?}");
    matching.remove(0)
}

/// Lowers, compiles and runs; the program's output, and the Rust it came from.
fn ran(purpose: &str, source: &str) -> (String, String) {
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust;
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );
    let out = std::process::Command::new(&binary)
        .output()
        .expect("run it");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
    (String::from_utf8_lossy(&out.stdout).to_string(), rust)
}

/// **D1, D5: a guard proves what follows it, and the proof costs nothing.**
/// `return 250 if speed > 250` leaves `speed <= 250` (ADR-255).
#[test]
fn a_guard_proves_the_claim_after_it_and_no_check_is_emitted() {
    let (out, rust) = ran(
        "prove-guard",
        "fn limit(speed: i64) -> i64 {\n\
         \x20   return 250 if speed > 250\n\
         \x20   assert(speed <= 250)\n\
         \x20   return speed\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{limit(300)} {limit(100)}\")\n\
         }\n",
    );
    assert_eq!(out, "250 100\n");
    assert!(!rust.contains("assertion_failed"), "{rust}");
}

/// **D5: the facts the prover reads** - an `if`'s branch, a `let`'s value, a
/// `for` over a range, `!=` ruled out by a guard, and integer tightening
/// (`x >= 5` proves `x > 4`).
#[test]
fn branches_lets_ranges_and_guards_are_facts() {
    let source = "fn f(x: i64, d: i64) -> i64 {\n\
                  \x20   return 0 if d == 0\n\
                  \x20   assert(d != 0)\n\
                  \x20   let y = x + 3\n\
                  \x20   assert(y - x == 3)\n\
                  \x20   if x > 10 {\n\
                  \x20       assert(y > 13)\n\
                  \x20   } else {\n\
                  \x20       assert(x <= 10)\n\
                  \x20   }\n\
                  \x20   for i in 0..<10 {\n\
                  \x20       assert(i >= 0 && i < 10)\n\
                  \x20   }\n\
                  \x20   return 0 if x < 5\n\
                  \x20   assert(x > 4)\n\
                  \x20   return x / d\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{f(12, 3)}\")\n\
                  }\n";
    assert!(codes(source).is_empty(), "{:#?}", findings(source));
}

/// **D1: a claim nothing shows is refused**, and the message says what to do.
#[test]
fn a_claim_nothing_shows_is_refused() {
    let refusal = the_one(
        "fn f(x: i64) -> i64 {\n\
         \x20   let y = x * 2\n\
         \x20   assert(y > 0)\n\
         \x20   return y\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{f(1)}\")\n\
         }\n",
        "NK1202",
    );
    assert_eq!(refusal.message, "The compiler can't prove `y > 0`.");
    assert!(
        refusal
            .notes
            .iter()
            .any(|n| n == "Nothing before this line shows it."),
        "{refusal:#?}"
    );
    assert!(
        refusal
            .help
            .unwrap_or_default()
            .contains("return … if !(y > 0)")
    );
}

/// **D5: a claim the prover cannot read is refused, not guessed.**
#[test]
fn a_claim_outside_what_the_prover_reads_is_refused() {
    let refusal = the_one(
        "fn main() {\n\
         \x20   let name = \"ab\"\n\
         \x20   assert(name == \"ab\")\n\
         }\n",
        "NK1202",
    );
    assert!(
        refusal.notes[0].starts_with("The prover reads comparisons of whole numbers"),
        "{refusal:#?}"
    );
}

/// **D3: a claim about parameters is the caller's to prove**: a literal
/// argument proves it, a guard proves it, and a call that shows nothing is
/// `NK1203` - at the call.
#[test]
fn a_parameter_claim_is_a_precondition_the_caller_proves() {
    let (out, rust) = ran(
        "prove-precondition",
        "fn percent(part: i64, whole: i64) -> i64 {\n\
         \x20   assert(whole > 0)\n\
         \x20   return part * 100 / whole\n\
         }\n\
         \n\
         fn report(done: i64, total: i64) -> i64 {\n\
         \x20   return 0 if total <= 0\n\
         \x20   return percent(done, total)\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{percent(1, 4)} {report(3, 4)} {report(3, 0)}\")\n\
         }\n",
    );
    assert_eq!(out, "25 75 0\n");
    assert!(!rust.contains("assertion_failed"), "{rust}");

    let refusal = the_one(
        "fn percent(part: i64, whole: i64) -> i64 {\n\
         \x20   assert(whole > 0)\n\
         \x20   return part * 100 / whole\n\
         }\n\
         \n\
         fn report(done: i64, total: i64) -> i64 {\n\
         \x20   return percent(done, total)\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{report(3, 4)}\")\n\
         }\n",
        "NK1203",
    );
    assert_eq!(
        refusal.message,
        "This call to `percent` doesn't show `whole > 0`."
    );
}

/// **D3: where a precondition cannot be carried yet**, the claim is refused
/// with the reason: a `pub fn`, a method, and a function handed on as a value.
#[test]
fn a_precondition_is_refused_where_it_cannot_be_carried() {
    let public = the_one(
        "pub fn half(n: i64) -> i64 {\n\
         \x20   assert(n >= 0)\n\
         \x20   return n / 2\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{half(4)}\")\n\
         }\n",
        "NK1202",
    );
    assert!(
        public
            .notes
            .iter()
            .any(|n| n.contains("A `pub fn` can't have a precondition yet")),
        "{public:#?}"
    );

    let value = the_one(
        "fn positive(n: i64) -> i64 {\n\
         \x20   assert(n > 0)\n\
         \x20   return n\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let f = positive\n\
         \x20   println(f\"{f(1)}\")\n\
         }\n",
        "NK1204",
    );
    assert!(
        value.message.contains("`positive` has a precondition"),
        "{value:#?}"
    );
}

/// **D4: a claim about data from outside the program** is refused with where
/// it came from, and proved once a guard has checked it.
#[test]
fn data_from_outside_is_checked_by_a_guard_and_not_asserted() {
    let refusal = the_one(
        "use std::process\n\
         \n\
         fn main() throws {\n\
         \x20   let out = process::run(\"true\", [])\n\
         \x20   let code: i64 = out.code\n\
         \x20   assert(code == 0)\n\
         }\n",
        "NK1202",
    );
    assert!(
        refusal.notes[0].contains("`code` comes from outside the program"),
        "{refusal:#?}"
    );
    assert!(
        refusal
            .help
            .unwrap_or_default()
            .contains("Check it where it arrives")
    );

    let guarded = "use std::process\n\
                   \n\
                   fn main() throws {\n\
                   \x20   let out = process::run(\"true\", [])\n\
                   \x20   let code: i64 = out.code\n\
                   \x20   return if code != 0\n\
                   \x20   assert(code == 0)\n\
                   }\n";
    assert!(
        !codes(guarded).contains(&"NK1202"),
        "{:#?}",
        findings(guarded)
    );
}

/// **D2: a test's `assert` is the test's verdict**, checked when it runs and
/// never refused for want of a proof.
#[test]
fn a_tests_claim_is_not_the_provers() {
    let source = "fn double(n: i64) -> i64 {\n\
                  \x20   return n * 2\n\
                  }\n\
                  \n\
                  test \"doubling\" {\n\
                  \x20   assert(double(2) == 4)\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  }\n";
    assert!(
        !codes(source).contains(&"NK1202"),
        "{:#?}",
        findings(source)
    );
}

/// **A list's length is a number the prover reads** (ADR-256 D5, 0.0.273):
/// `xs.len()` of a binding that does not change, never negative, known for a
/// literal, and carried into a callee's precondition.
#[test]
fn a_length_is_a_variable_of_the_proof() {
    let source = "fn first(xs: Vec[i64]) -> i64 {\n\
                  \x20   assert(xs.len() > 0)\n\
                  \x20   return xs[0]\n\
                  }\n\
                  \n\
                  fn head_or(xs: Vec[i64], fallback: i64) -> i64 {\n\
                  \x20   return fallback if xs.len() == 0\n\
                  \x20   assert(xs.len() >= 1)\n\
                  \x20   return first(xs)\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let xs = [3, 4]\n\
                  \x20   assert(xs.len() == 2)\n\
                  \x20   println(f\"{first(xs)} {head_or(xs, 0)}\")\n\
                  }\n";
    assert!(codes(source).is_empty(), "{:#?}", findings(source));

    // And a call that shows nothing about the length is refused at the call.
    let refusal = the_one(
        "fn first(xs: Vec[i64]) -> i64 {\n\
         \x20   assert(xs.len() > 0)\n\
         \x20   return xs[0]\n\
         }\n\
         \n\
         fn use_it(ys: Vec[i64]) -> i64 {\n\
         \x20   return first(ys)\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{use_it([1])}\")\n\
         }\n",
        "NK1203",
    );
    assert!(refusal.message.contains("xs.len() > 0"), "{refusal:#?}");
}

/// **A lambda keeps what holds around it**, and its own parameters are new
/// names: a guard outside proves a claim inside, and a parameter that shadows
/// an outer name knows nothing of it.
#[test]
fn a_lambda_sees_the_facts_around_it_and_not_through_its_parameters() {
    let source = "fn scale(xs: Vec[i64], k: i64) -> Vec[i64] {\n\
                  \x20   return [] if k <= 0\n\
                  \x20   return xs.map fn(x) {\n\
                  \x20       assert(k > 0)\n\
                  \x20       x * 2\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{scale([1], 2).len()}\")\n\
                  }\n";
    assert!(
        !codes(source).contains(&"NK1202"),
        "{:#?}",
        findings(source)
    );

    let shadowed = "fn f(k: i64) -> Vec[i64] {\n\
                    \x20   return [] if k <= 0\n\
                    \x20   return [1, 2].map fn(k) {\n\
                    \x20       assert(k > 0)\n\
                    \x20       k\n\
                    \x20   }\n\
                    }\n\
                    \n\
                    fn main() {\n\
                    \x20   println(f\"{f(1).len()}\")\n\
                    }\n";
    assert!(
        codes(shadowed).contains(&"NK1202"),
        "{:#?}",
        findings(shadowed)
    );
}
