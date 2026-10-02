//! **Every `assert` outside a test is proved while the program is built
//! where it can be, and checked when the program runs where it cannot**
//! ([ADR-269](../../../docs/specification/adr/adr-269.md) D4). A proved claim
//! leaves nothing behind in the emitted Rust.

mod common;

use nikaia::contracts::{Ledger, LedgerOps, STD};
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

/// How each `assert` of the program is held, in the order they are written.
fn held(source: &str) -> Vec<nikaia::prove::Held> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .claims
        .into_values()
        .filter_map(|c| c.held)
        .collect()
}

/// How many calls in the emitted Rust check a precondition where they
/// stand (ADR-269 D20): `(if !… , call).1`.
fn call_checks(rust: &str) -> usize {
    rust.matches("(if !").count() + rust.matches("({ if !").count()
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

/// **D4, D9: a guard proves what follows it, and the proof costs nothing.**
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

/// **D9: the facts the prover reads** - an `if`'s branch, a `let`'s value, a
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

/// **D4: a claim nothing shows is checked when the program runs**, and says
/// why it was not proved.
#[test]
fn a_claim_nothing_shows_is_checked_when_the_program_runs() {
    // After a loop the claim is not carried back to the entry (ADR-269 D16),
    // so it is no precondition either.
    let source = "fn f(x: i64) -> i64 {\n\
                  \x20   let y = x * 2\n\
                  \x20   for i in 0..<1 {\n\
                  \x20   }\n\
                  \x20   assert(y > 0)\n\
                  \x20   return y\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{f(1)}\")\n\
                  }\n";
    assert_eq!(
        held(source),
        [nikaia::prove::Held::AtRunTime(
            "nothing before it shows it".to_string()
        )]
    );
    let (out, rust) = ran("prove-at-run-time", source);
    assert_eq!(out, "2\n");
    assert!(
        rust.contains("assertion_failed(\"assert(y > 0)\""),
        "{rust}"
    );
}

/// **D9: a claim the prover cannot read is not guessed**: it is checked when
/// the program runs.
#[test]
fn a_claim_outside_what_the_prover_reads_is_checked() {
    let source = "fn main() {\n\
                  \x20   let name = \"ab\"\n\
                  \x20   assert(name == \"ab\")\n\
                  }\n";
    assert_eq!(
        held(source),
        [nikaia::prove::Held::AtRunTime(
            "it is not a comparison of whole numbers the prover reads".to_string()
        )]
    );
    let (_, rust) = ran("prove-unread", source);
    assert!(rust.contains("assertion_failed"), "{rust}");
}

/// **D5: a claim about parameters is the caller's to prove**: a literal
/// argument proves it, a guard proves it, and a call that shows nothing
/// checks it - at the call, with its arguments in place of the parameters.
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
    assert_eq!(call_checks(&rust), 0, "{rust}");
    assert!(rust.contains("percent__unchecked(1, 4)"), "{rust}");

    let (out, rust) = ran(
        "prove-call-check",
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
    );
    assert_eq!(out, "75\n");
    assert!(
        rust.contains(
            "(if !((total.clone() as i128) > (0i128)) { nikaia_std::abort::assertion_failed("
        ),
        "{rust}"
    );
    assert_eq!(call_checks(&rust), 1, "{rust}");
}

/// **ADR-269 D20: a caller the prover doesn't see reaches the checked
/// entry**: a `pub fn` and a method have a precondition like any function,
/// and a function handed on as a value is its checked entry, while the calls
/// the prover saw prove theirs and take the unchecked one.
#[test]
fn a_caller_the_prover_does_not_see_reaches_the_checked_entry() {
    let source = "struct Counter {\n\
                  \x20   total: i64,\n\
                  }\n\
                  \n\
                  impl Counter {\n\
                  \x20   fn add(self, n: i64) -> i64 {\n\
                  \x20       assert(n > 0)\n\
                  \x20       return self.total + n\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  pub fn half(n: i64) -> i64 {\n\
                  \x20   assert(n >= 0)\n\
                  \x20   return n / 2\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let c = Counter { total: 1 }\n\
                  \x20   let g = half\n\
                  \x20   println(f\"{half(4)} {g(6)} {c.add(2)}\")\n\
                  }\n";
    assert_eq!(
        held(source),
        [
            nikaia::prove::Held::Precondition("Counter::add".to_string(), 0),
            nikaia::prove::Held::Precondition("half".to_string(), 0),
        ]
    );
    let (out, rust) = ran("prove-entries", source);
    assert_eq!(out, "2 3 3\n");
    assert!(rust.contains("pub fn half__unchecked(n: i64)"), "{rust}");
    assert!(rust.contains("half__unchecked(4)"), "{rust}");
    assert!(rust.contains("let g = half;"), "{rust}");
    assert!(rust.contains("c.add(2)"), "{rust}");
    assert!(rust.contains("self.add__unchecked(n)"), "{rust}");
}

/// **D6: a claim about data from outside the program** is refused with where
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

/// **D11: a test's `assert` is the test's verdict**, checked when it runs and
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

/// **A list's length is a number the prover reads** (ADR-269 D9, 0.0.273):
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

    // And a call that shows nothing about the length checks it, with the
    // argument's length in place of the parameter's.
    let (out, rust) = ran(
        "prove-length-call",
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
    );
    assert_eq!(out, "1\n");
    assert!(
        rust.contains("(if !((ys.len() as i128) > (0i128))"),
        "{rust}"
    );
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
        matches!(
            held(shadowed).as_slice(),
            [nikaia::prove::Held::AtRunTime(_)]
        ),
        "{:#?}",
        held(shadowed)
    );
}

/// **A binding that reads the name it shadows teaches nothing about the new
/// one** (0.0.353): `let x = x + 1` was taken as the fact `x = x + 1`, which
/// is a contradiction, so every claim after it was "proved" and its check left
/// out - a false `assert` included. The same for a range that reads its own
/// binding.
#[test]
fn a_shadowing_binding_is_not_a_contradiction() {
    let source = "fn main() {\n\
                  \x20   let x = 1\n\
                  \x20   let x = x + 1\n\
                  \x20   assert(x == 0)\n\
                  \x20   let i = 3\n\
                  \x20   for i in 0..<i {\n\
                  \x20       assert(i > 5)\n\
                  \x20   }\n\
                  }\n";
    assert!(
        held(source)
            .iter()
            .all(|h| matches!(h, nikaia::prove::Held::AtRunTime(_))),
        "{:#?}",
        held(source)
    );
}

/// The program's warnings with one code.
fn warned(source: &str, code: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Warning && f.code == code)
        .collect()
}

/// **ADR-269 D8: a claim the facts rule out is a warning with values**, and
/// stays a check. Shown false only where it is false every time it is
/// reached - proved so, certificate and all - with a model of what is known
/// for the values.
#[test]
fn a_claim_false_every_time_it_is_reached_is_a_warning_with_values() {
    let source = "fn twice(n: i64) -> i64 {\n\
                  \x20   return 0 if n < 0\n\
                  \x20   let m = n * 2\n\
                  \x20   assert(m < 0)\n\
                  \x20   return m\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let blank = 2\n\
                  \x20   assert(blank + 1 == 4)\n\
                  \x20   println(f\"{twice(3)}\")\n\
                  }\n";
    let found = warned(source, "NK1207");
    assert_eq!(found.len(), 2, "{found:#?}");
    assert_eq!(
        found[0].message,
        "`m < 0` is false every time it is reached."
    );
    assert!(found[0].notes[0].ends_with("`m` is 0."), "{found:#?}");
    assert_eq!(
        found[1].message,
        "`blank + 1 == 4` is false every time it is reached."
    );
    assert!(found[1].notes[0].ends_with("`blank` is 2."), "{found:#?}");
    // Both stay checks: a warning is not a proof of anything.
    assert!(
        held(source)
            .iter()
            .all(|h| matches!(h, nikaia::prove::Held::AtRunTime(why) if why.starts_with("it is false every time"))),
        "{:#?}",
        held(source)
    );
}

/// **A call that breaks a precondition every time** is the warning at the
/// call, with the parameter the precondition reads as this call gives it.
#[test]
fn a_call_that_always_breaks_a_precondition_is_a_warning() {
    let found = warned(
        "fn percent(part: i64, whole: i64) -> i64 {\n\
         \x20   assert(whole > 0)\n\
         \x20   return part * 100 / whole\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let none = 0\n\
         \x20   println(f\"{percent(1, none)}\")\n\
         }\n",
        "NK1207",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].message,
        "This call breaks `percent`'s precondition `whole > 0` every time it is reached."
    );
    assert_eq!(found[0].notes[0], "Here `whole` is 0.");
}

/// **No warning where a claim is only not proved**: `y > 0` after
/// `let y = x * 2` can hold, and a value of `x` the facts allow is not a value
/// the program is shown to reach.
#[test]
fn a_claim_that_can_hold_is_not_warned_about() {
    let source = "fn f(x: i64) -> i64 {\n\
                  \x20   let y = x * 2\n\
                  \x20   assert(y > 0)\n\
                  \x20   return y\n\
                  }\n\
                  \n\
                  fn g(n: i64) -> i64 {\n\
                  \x20   return f(n)\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{f(1)} {g(2)}\")\n\
                  }\n";
    assert!(warned(source, "NK1207").is_empty());
}

/// **ADR-269 D15: a claim is carried back to the entry.** A parameter changed
/// before the claim (`let x = x + 1`), a value computed from one, a branch and
/// a guard: each claim is a precondition over the parameters, its callers
/// prove it, and no check is left in the program.
#[test]
fn a_claim_carried_back_to_the_entry_is_a_precondition() {
    let source = "fn f(x: i64, mode: i64) -> i64 {\n\
                  \x20   let y = x - 1\n\
                  \x20   if mode == 1 {\n\
                  \x20       assert(y > 0)\n\
                  \x20   }\n\
                  \x20   return y\n\
                  }\n\
                  \n\
                  fn g(x: i64) -> i64 {\n\
                  \x20   let x = x + 1\n\
                  \x20   assert(x > 3)\n\
                  \x20   return x\n\
                  }\n\
                  \n\
                  fn h(n: i64) -> i64 {\n\
                  \x20   return 0 if n < 0\n\
                  \x20   let m = n - 5\n\
                  \x20   assert(m >= 0)\n\
                  \x20   return m\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{f(5, 1)} {f(0, 2)} {g(3)} {h(7)} {h(0 - 1)}\")\n\
                  }\n";
    assert!(
        held(source)
            .iter()
            .all(|h| matches!(h, nikaia::prove::Held::Precondition(_, 0))),
        "{:#?}",
        held(source)
    );
    let (out, rust) = ran("prove-carried-back", source);
    assert_eq!(out, "4 -1 4 2 0\n");
    assert_eq!(call_checks(&rust), 0, "{rust}");
}

/// **A call that breaks a computed precondition** is told the condition at
/// the entry and the `assert` it came from (ADR-269 D21), and the call checks
/// the condition at the entry with its arguments in place (D7).
#[test]
fn a_computed_precondition_names_both_ends() {
    let source = "fn f(x: i64, mode: i64) -> i64 {\n\
                  \x20   let y = x - 1\n\
                  \x20   if mode == 1 {\n\
                  \x20       assert(y > 0)\n\
                  \x20   }\n\
                  \x20   return y\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{f(0, 1)}\")\n\
                  }\n";
    let found = warned(source, "NK1207");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].message,
        "This call breaks `f`'s precondition `mode == 1 → x - 1 > 0` every time it is reached."
    );
    assert_eq!(
        found[0].notes[..2],
        [
            "Here `mode` is 1, `x` is 0.".to_string(),
            "The precondition is `assert(y > 0)` in `f`, carried back to its entry.".to_string()
        ]
    );
    assert_eq!(
        held(source),
        [nikaia::prove::Held::Precondition("f".to_string(), 1)]
    );
}

/// **ADR-269 D17: a claim about the value returned, shown at every exit, is a
/// postcondition**, and a caller knows it of the result: `clamp`'s
/// `result >= 0` proves `a >= 0`, and a chain of them proves `t >= 0`. One
/// exit that does not show it - `wrong`'s `return -1` - and it is none, so
/// `b >= 0` stays a check.
#[test]
fn a_claim_shown_at_every_exit_is_a_postcondition() {
    use nikaia::prove::Held::{AtRunTime, Proved};
    let source = "fn clamp(n: i64) -> i64 {\n\
                  \x20   return 0 if n < 0\n\
                  \x20   assert(n >= 0)\n\
                  \x20   return n\n\
                  }\n\
                  \n\
                  fn wrong(n: i64) -> i64 {\n\
                  \x20   return 0 - 1 if n < 0\n\
                  \x20   let m = n\n\
                  \x20   assert(m >= 0)\n\
                  \x20   return m\n\
                  }\n\
                  \n\
                  fn twice_clamped(n: i64) -> i64 {\n\
                  \x20   let c = clamp(n)\n\
                  \x20   let d = c * 2\n\
                  \x20   assert(d >= 0)\n\
                  \x20   return d\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let a = clamp(0 - 5)\n\
                  \x20   assert(a >= 0)\n\
                  \x20   let b = wrong(5)\n\
                  \x20   assert(b >= 0)\n\
                  \x20   let t = twice_clamped(3)\n\
                  \x20   assert(t >= 0)\n\
                  \x20   println(f\"{a} {b} {t}\")\n\
                  }\n";
    let held = held(source);
    assert!(
        matches!(
            held.as_slice(),
            [Proved, Proved, Proved, Proved, AtRunTime(_), Proved]
        ),
        "{held:#?}"
    );
    let (out, rust) = ran("prove-postcondition", source);
    assert_eq!(out, "0 5 6\n");
    assert_eq!(rust.matches("assertion_failed(").count(), 1, "{rust}");
}

/// **What a postcondition is not shown by**: a `return` inside a lambda is
/// the lambda's, and a body that can end without a `return` has an exit no
/// claim stands before.
#[test]
fn an_exit_that_shows_nothing_makes_no_postcondition() {
    use nikaia::prove::Held::{AtRunTime, Proved};
    let source = "fn open_end(n: i64) -> i64 {\n\
                  \x20   if n > 0 {\n\
                  \x20       assert(n > 0)\n\
                  \x20       return n\n\
                  \x20   }\n\
                  \x20   println(\"none\")\n\
                  \x20   return 0 - 1\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let a = open_end(2)\n\
                  \x20   assert(a > 0)\n\
                  }\n";
    let held = held(source);
    assert!(
        matches!(held.as_slice(), [Proved, AtRunTime(_)]),
        "{held:#?}"
    );
}
