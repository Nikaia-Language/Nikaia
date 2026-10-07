//! Running Nikaia while the program is built
//! ([ADR-287](../../../docs/specification/adr/adr-287.md) D6's second stage,
//! bounded by [ADR-287](../../../docs/specification/adr/adr-287.md)).
//!
//! `comptime` has had an evaluator since the word existed and what it knew was
//! an integer literal, a name whose value already folded, a negation and
//! `+ - * / %` — `fold.rs`, 124 lines with no call in it. D5 called that the
//! first stage and wrote the second one as *when Q4 is answered*: a call, and
//! with it the file reading [ADR-310](../../../docs/specification/adr/adr-310.md)
//! waits behind. [ADR-287](../../../docs/specification/adr/adr-287.md) answered
//! Q4, and three records have been waiting on this since
//! (issue #178).
//!
//! What a built program reads back from its constants - the run half of the
//! tests below whose value crosses into the program - is
//! `tests/build-time/src/build_time.nika`; this file keeps what is refused and
//! what is lowered.

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::Build;
use nikaia::parser::parse_to_ast;

/// **Where a `comptime`'s program is compiled and run** (ADR-321 D1): one
/// workshop for the file, as a build keeps one beside its cache.
fn reads() -> nikaia::assets::Reads {
    let at = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("comptime-compiled");
    nikaia::assets::Reads::at(&at).building_in(&at)
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check_against(
        &parsed,
        &[],
        &own,
        &library,
        &std::collections::BTreeSet::new(),
        &nikaia::check::Newly::new(),
        &reads(),
    )
    .findings
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    nikaia::emit::emit_program_reading(&parsed, Build::default(), &reads())
        .expect("it lowers")
        .rust
}

/// **A call in an initialiser**, which is the whole of D5's second stage.
#[test]
fn a_call_is_evaluated_while_the_program_is_built() {
    let source = "fn double(n: i64) -> i64 { return n * 2 }\n\
                  comptime ANSWER = double(21)\n\
                  fn main() { println(f\"{ANSWER}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const ANSWER: i64 = 42;"),
        "{}",
        lowered(source)
    );
}

/// **A recursion with a base case**, which needs the `if` that returns out of
/// one arm and falls through the other — the shape a body is written in.
#[test]
fn a_recursion_with_a_base_case_is_evaluated() {
    let source = "fn factorial(n: i64) -> i64 {\n\
                  \x20   if n < 2 { return 1 }\n\
                  \x20   return n * factorial(n - 1)\n\
                  }\n\
                  comptime SIX = factorial(3)\n\
                  fn main() { println(f\"{SIX}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(lowered(source).contains("const SIX: i64 = 6;"));
}

/// **What the name is worth is what was evaluated, not what folded**, which
/// they were the same thing until there was a call. Binding the fold left
/// `comptime ANSWER = double(21)` visible as a name with no value, so the next
/// constant that read it was `NK1127` although the one before it had just been
/// computed.
#[test]
fn a_constant_can_read_the_one_above_it() {
    let source = "fn double(n: i64) -> i64 { return n * 2 }\n\
                  comptime ANSWER = double(21)\n\
                  comptime BIG = ANSWER > 40\n\
                  fn main() { println(f\"{BIG}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(lowered(source).contains("const BIG: bool = true;"));
}

/// **A callee that can pause may not be called** (D1), and the message says
/// which rule and why.
#[test]
fn a_pausing_callee_is_refused() {
    let found: Vec<_> = findings(
        "use std::io\n\nfn slow() -> i64 throws {\n\
         \x20   let t = io::read_to_string()\n\
         \x20   return t.len()\n\
         }\n\
         comptime N = slow()\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1152")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].notes.join(" ").contains("It can pause"),
        "{found:#?}"
    );
}

/// **And one that reaches the world** (D2). `println` is the case the record
/// names as a reasonable thing to want and a later decision with its own
/// reasons — refused here as a consequence of the boundary rather than a
/// judgement about printing.
#[test]
fn a_callee_that_touches_the_world_is_refused() {
    let found: Vec<_> = findings(
        "fn noisy(n: i64) -> i64 {\n\
         \x20   println(\"building\")\n\
         \x20   return n\n\
         }\n\
         comptime N = noisy(1)\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1152")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0]
            .notes
            .join(" ")
            .contains("touches the world outside the program"),
        "{found:#?}"
    );
}

/// **A recursion with no base case says so rather than taking the stack.**
///
/// [ADR-287](../../../docs/specification/adr/adr-287.md) D16 deliberately has no
/// step budget and wrote down what that costs: a body that does not terminate
/// hangs the build. This is not that — a recursion without a base case would
/// overflow *this compiler's* stack, and a compiler that falls over is not the
/// hang D4 accepted.
#[test]
fn an_unbounded_recursion_is_refused_rather_than_crashing() {
    let found: Vec<_> = findings(
        "fn forever(n: i64) -> i64 { return forever(n + 1) }\n\
         comptime N = forever(0)\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1152")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("calls deep"), "{found:#?}");
}

/// **A callee of `std`'s is asked the rule as any other is**
/// ([ADR-321](../../../docs/specification/adr/adr-321.md) D2): its Rust half
/// runs at build time where its entry allows it, and reading standard input
/// does not - `NK1152`, by its path. It was `NK1127` while the interpreter
/// could run only code written in the program.
#[test]
fn a_callee_of_std_that_reads_input_is_forbidden() {
    let found: Vec<_> = findings("comptime N = io::read_to_string()\n")
        .into_iter()
        .filter(|f| f.code == "NK1127" || f.code == "NK1152")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].message, "You can't call `io::read_to_string` while the program is built.",
        "{found:#?}"
    );
}

/// **A `for` over a range**, which is issue #178's second step: the
/// loop the evaluator used to refuse.
#[test]
fn a_for_over_a_range_is_evaluated() {
    let source = "fn total(n: i64) -> i64 {\n\
                  \x20   let mut t = 0\n\
                  \x20   for i in 0..<n { t += i }\n\
                  \x20   return t\n\
                  }\n\
                  comptime SIX = total(4)\n\
                  fn main() { println(f\"{SIX}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const SIX: i64 = 6;"),
        "{}",
        lowered(source)
    );
}

/// **`..` and `..<` are different loops**
/// ([ADR-291](../../../docs/specification/adr/adr-291.md) D12, D13), and the
/// evaluator reads the end the same way the emitter does rather than assuming
/// one.
#[test]
fn an_inclusive_range_counts_one_further() {
    let source = "fn total(n: i64) -> i64 {\n\
                  \x20   let mut t = 0\n\
                  \x20   for i in 0..n { t += i }\n\
                  \x20   return t\n\
                  }\n\
                  comptime TEN = total(4)\n\
                  fn main() { println(f\"{TEN}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const TEN: i64 = 10;"),
        "{}",
        lowered(source)
    );
}

/// **A `while`**, which is the loop
/// [ADR-287](../../../docs/specification/adr/adr-287.md) D16 said would get no
/// step budget: this one ends because its body ends it, and nothing here
/// counted the turns.
#[test]
fn a_while_is_evaluated_and_nothing_counts_its_turns() {
    let source = "fn halvings(n: i64) -> i64 {\n\
                  \x20   let mut left = n\n\
                  \x20   let mut steps = 0\n\
                  \x20   while left > 1 {\n\
                  \x20       left = left / 2\n\
                  \x20       steps += 1\n\
                  \x20   }\n\
                  \x20   return steps\n\
                  }\n\
                  comptime STEPS = halvings(64)\n\
                  fn main() { println(f\"{STEPS}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const STEPS: i64 = 6;"),
        "{}",
        lowered(source)
    );
}

/// **`break` leaves the loop and `continue` starts its next turn**, and the two
/// are separate statements here for the reason the tree keeps them separate:
/// one ends a loop and one does not.
#[test]
fn break_and_continue_are_both_read() {
    let source = "fn first_over(limit: i64) -> i64 {\n\
                  \x20   let mut found = 0\n\
                  \x20   for i in 0..<100 {\n\
                  \x20       if i % 3 != 0 { continue }\n\
                  \x20       if i > limit {\n\
                  \x20           found = i\n\
                  \x20           break\n\
                  \x20       }\n\
                  \x20   }\n\
                  \x20   return found\n\
                  }\n\
                  comptime FOUND = first_over(10)\n\
                  fn main() { println(f\"{FOUND}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const FOUND: i64 = 12;"),
        "{}",
        lowered(source)
    );
}

/// **A `return` out of a loop leaves the function**, not the loop — which is
/// the one way a loop's body hands a value back at all, since
/// [ADR-276](../../../docs/specification/adr/adr-276.md) D13 says `break`
/// carries none.
#[test]
fn a_return_inside_a_loop_leaves_the_function() {
    let source = "fn first_square_over(limit: i64) -> i64 {\n\
                  \x20   for i in 0..<100 {\n\
                  \x20       if i * i > limit { return i }\n\
                  \x20   }\n\
                  \x20   return 0\n\
                  }\n\
                  comptime ROOT = first_square_over(50)\n\
                  fn main() { println(f\"{ROOT}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const ROOT: i64 = 8;"),
        "{}",
        lowered(source)
    );
}

/// **The loop's name does not outlive the loop**, which is Part I 3.3's rule
/// and matters here because the evaluator's frame is shared between a loop and
/// the body around it.
#[test]
fn the_loops_binding_does_not_outlive_it() {
    let found: Vec<_> = findings(
        "fn leaks() -> i64 {\n\
         \x20   for i in 0..<3 { }\n\
         \x20   return i\n\
         }\n\
         comptime N = leaks()\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1127" || f.code == "NK1152")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
}

/// **The aggregate value** (issue #174,
/// [ADR-311](../../../docs/specification/adr/adr-311.md)'s *evaluator that
/// can loop and push*).
///
/// The loop was built; what a loop had nowhere to put was a **value**. A table
/// computed while the program is built reaches the generated file as a Rust
/// `const` array — which is what [ADR-152](../../../docs/specification/adr/adr-152.md)'s
/// `Array[T, N]` is for, because a `Vec` allocates and a `const` cannot hold
/// one.
///
/// **And it is an array rather than a `push`** for a reason that is measured
/// and not chosen: `.push` on a list hands back a `Vec[?]`, and `NK1104`
/// refuses that against an `Array[i64, 5]` long before this evaluator is
/// reached. So a build-time table is written at its length and filled by index.
#[test]
fn a_table_is_computed_while_the_program_is_built() {
    let source = "fn squares() -> Array[i64, 5] {\n\
                  \x20   let mut xs: Array[i64, 5] = [0, 0, 0, 0, 0]\n\
                  \x20   for i in 0..<xs.len() {\n\
                  \x20       xs[i] = i * i\n\
                  \x20   }\n\
                  \x20   return xs\n\
                  }\n\
                  comptime TABLE: Array[i64, 5] = squares()\n\
                  fn main() { println(f\"{TABLE[4]}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const TABLE: [i64; 5] = [0, 1, 4, 9, 16];"),
        "{}",
        lowered(source)
    );
}

/// A table written out by hand, which [ADR-311](../../../docs/specification/adr/adr-311.md)
/// §3 calls the *toy* version and which was the harder one to reach: the
/// annotation is a **use**, so it is what says the literal is an array rather
/// than a list (ADR-152 D4).
///
/// **This crashed the compiler.** `comptime PRIMES: Array[i64, 4] = [2, 3, 5, 7]`
/// reached `expect` with a word that had no diagnostic code and panicked on an
/// `unreachable!` — [Part I 6.8](../../../docs/specification/10-nikaia-light.md)'s
/// *a raw internal error reaching you is a Nikaia bug*, met by the compiler
/// itself on a correct program.
#[test]
fn a_table_written_out_by_hand_is_an_array() {
    let source = "comptime PRIMES: Array[i64, 4] = [2, 3, 5, 7]\n\
                  fn main() { println(f\"{PRIMES[3]}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const PRIMES: [i64; 4] = [2, 3, 5, 7];"),
        "{}",
        lowered(source)
    );
}

/// `NK1166`, the code that panic left missing: a `comptime` whose value is not
/// what it says it is.
#[test]
fn a_comptime_that_disagrees_with_its_own_type_is_refused() {
    let found = findings("comptime X: bool = [2, 3]\nfn main() { println(f\"{X}\") }\n");
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1166")
        .unwrap_or_else(|| panic!("NK1166 rather than a panic: {found:#?}"));
    assert!(
        refusal.message.contains("bool"),
        "it names what was declared: {}",
        refusal.message
    );
}

/// **An index the array does not have stops the build where it would stop
/// the program** ([ADR-321](../../../docs/specification/adr/adr-321.md) D1):
/// the build-time run is the program's own code, so it aborts as
/// [ADR-285](../../../docs/specification/adr/adr-285.md) D1 aborts at run time,
/// pointed at the line. It was `NK1165` while an interpreter computed it.
#[test]
fn a_build_time_index_the_array_does_not_have_is_named() {
    let source = "fn out() -> i64 {\n\
                  \x20   let xs: Array[i64, 3] = [1, 2, 3]\n\
                  \x20   return xs[7]\n\
                  }\n\
                  comptime BAD: i64 = out()\n\
                  fn main() { println(f\"{BAD}\") }\n";
    // **It stops as the program stops** (ADR-321 D1), in the program's words.
    let found = findings(source);
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1152")
        .unwrap_or_else(|| panic!("NK1152: {found:#?}"));
    assert_eq!(
        refusal.message,
        "`BAD` stopped while the program was built: index out of bounds: the len is 3 \
         but the index is 7."
    );
    // **And it is said once.** `NK1127` used to follow every refusal by name,
    // which is not a second fact — it is the first one with less in it.
    assert!(
        !found.iter().any(|f| f.code == "NK1127"),
        "one mistake, one error: {found:#?}"
    );
}

/// The same rule where the refusal is `NK1152`'s, which is the case that shows
/// the doubling was general rather than the new code's.
#[test]
fn a_forbidden_callee_is_also_said_once() {
    let source = "use std::fs\n\
                  fn read() -> i64 { let t = fs::read_to_string(\"x\", fs::Root::Anywhere) catch { \"\" } return t.len() }\n\
                  comptime N: i64 = read()\n\
                  fn main() { println(f\"{N}\") }\n";
    let found = findings(source);
    assert!(
        found.iter().any(|f| f.code == "NK1152"),
        "the callee the rule forbids: {found:#?}"
    );
    assert!(
        !found.iter().any(|f| f.code == "NK1127"),
        "one mistake, one error: {found:#?}"
    );
}

/// **An array knows its own length**, which nothing said before: `xs.len()` on
/// an `Array[T, N]` resolved to no ledger entry, and an unresolved call costs
/// the *enclosing* function its touch set (ADR-292) — so the natural spelling
/// of a build-time loop was refused with *nothing says what it touches* while
/// `0..<5` was fine.
#[test]
fn an_array_has_a_length_in_the_ledger() {
    let library = Ledger::parse(STD).expect("std's ledger");
    let entry = library
        .functions
        .get("Array::len")
        .expect("`Array::len` is described");
    assert!(entry.sync_claim.is_sync() && entry.touches_known && entry.touches.is_empty());
    assert!(library.functions.contains_key("Array::is_empty"));
}

/// **Growable going in, fixed coming out** —
/// [ADR-311](../../../docs/specification/adr/adr-311.md)'s own title, and the
/// half of it this evaluator can now do. A body builds its table with `push`,
/// which is the shape §3 of that record asked for; what crosses into the
/// program is fixed, because `const X: Vec<T>` is not a thing the language
/// below has and `const X: [T; N]` is.
///
/// **0.0.108 said there was no `push` and gave a measurement for it.** The
/// measurement was right — `.push` hands back a `Vec[?]` and `NK1104` refuses
/// that against an `Array[i64, 5]` — and the conclusion drawn from it was too
/// narrow: the refusal is about a **function's declared result**, and a
/// `comptime` is not one. By the time the declaration is compared, the build
/// has computed the value, so its length is a fact.
#[test]
fn a_table_built_with_push_crosses_as_an_array() {
    let source = "fn squares() -> Vec[i64] {\n\
                  \x20   let mut xs = []\n\
                  \x20   for i in 0..<5 {\n\
                  \x20       xs.push(i * i)\n\
                  \x20   }\n\
                  \x20   return xs\n\
                  }\n\
                  comptime TABLE: Array[i64, 5] = squares()\n\
                  fn main() { println(f\"{TABLE[4]}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const TABLE: [i64; 5] = [0, 1, 4, 9, 16];"),
        "{}",
        lowered(source)
    );
}

/// The same with **no annotation at all**, which is where the element type has
/// to come from somewhere: the checker's, and not the values'. Reading it off
/// the values makes `[0, 1, 4]` an `[i32; 3]`, and the first `i64` arithmetic
/// on it is `rustc`'s complaint about a file nobody wrote.
#[test]
fn an_unannotated_table_takes_the_element_type_the_checker_has() {
    let source = "fn squares() -> Vec[i64] {\n\
                  \x20   let mut xs = []\n\
                  \x20   for i in 0..<5 {\n\
                  \x20       xs.push(i * i)\n\
                  \x20   }\n\
                  \x20   return xs\n\
                  }\n\
                  comptime TABLE = squares()\n\
                  fn main() { println(f\"{TABLE[4]}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const TABLE: [i64; 5] ="),
        "the checker's `i64`, not the values' `i32`:\n{}",
        lowered(source)
    );
}

/// `NK1157`'s second sentence. The rule is the one a literal gets — an
/// `Array[T, N]` takes exactly `N` — and the **way out** is not, because *write
/// five elements* is advice nobody can take about a number that came out of a
/// body.
#[test]
fn a_computed_table_of_the_wrong_length_says_both_numbers() {
    let source = "fn squares() -> Vec[i64] {\n\
                  \x20   let mut xs = []\n\
                  \x20   for i in 0..<5 {\n\
                  \x20       xs.push(i * i)\n\
                  \x20   }\n\
                  \x20   return xs\n\
                  }\n\
                  comptime TABLE: Array[i64, 3] = squares()\n\
                  fn main() { println(f\"{TABLE[0]}\") }\n";
    let found = findings(source);
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1157")
        .unwrap_or_else(|| panic!("NK1157: {found:#?}"));
    assert!(
        refusal.message.contains("computes 5 elements") && refusal.message.contains("holds 3"),
        "both numbers: {}",
        refusal.message
    );
    assert!(
        refusal
            .help
            .as_deref()
            .is_some_and(|h| h.contains("Change the array's declared length to match")),
        "a way out that can be taken: {:?}",
        refusal.help
    );
}

/// **`NK1167`** ([ADR-311](../../../docs/specification/adr/adr-311.md) D2): a
/// `comptime` whose value owns memory is refused for *what it is* — and the way
/// out names the length, because the build has just computed it.
#[test]
fn a_constant_declared_a_vec_is_told_the_length_it_computed() {
    let source = "fn squares() -> Vec[i64] {\n\
                  \x20   let mut xs = []\n\
                  \x20   for i in 0..<5 {\n\
                  \x20       xs.push(i * i)\n\
                  \x20   }\n\
                  \x20   return xs\n\
                  }\n\
                  comptime TABLE: Vec[i64] = squares()\n\
                  fn main() { println(f\"{TABLE[0]}\") }\n";
    let found = findings(source);
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1167")
        .unwrap_or_else(|| panic!("NK1167: {found:#?}"));
    assert!(
        refusal
            .help
            .as_deref()
            .is_some_and(|h| h.contains("Array[i64, 5]")),
        "the way out names the length the build computed: {:?}",
        refusal.help
    );
    assert!(
        !found.iter().any(|f| f.code == "NK1127"),
        "one mistake, one error: {found:#?}"
    );
}

/// **An `Array` written without its length is told the length**, not handed
/// back the type it already wrote: `Array[ref String]` and a two-element list
/// used to say *make it an `Array[ref String]`*.
#[test]
fn an_array_without_a_length_is_told_the_length() {
    let source = "comptime WORDS: Array[ref String] = [\"a\", \"b\"]\n\
                  fn main() { println(WORDS[0]) }\n";
    let found = findings(source);
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1166")
        .unwrap_or_else(|| panic!("NK1166: {found:#?}"));
    assert!(refusal.message.contains("no length"), "{refusal:#?}");
    assert!(
        refusal
            .help
            .as_deref()
            .is_some_and(|h| h.contains("Array[ref String, 2]")),
        "{refusal:#?}"
    );
    assert_eq!(found.len(), 1, "one mistake, one error: {found:#?}");
}

/// **And a body that could not be run is not told its declaration is wrong.**
/// `NK1152` says the callee may not run while the program is built; adding
/// *this is a `Vec[i64]` and the `const` says `Array[i64, 1]`* would send the
/// reader to the one line that is right, which is
/// [Part III C.4](../../../docs/specification/30-nikaia-tooling.md)'s failure
/// with the refusal already made.
#[test]
fn an_unevaluable_body_does_not_also_blame_its_declaration() {
    let source = "use std::fs\n\
                  fn lines() -> Vec[i64] {\n\
                  \x20   let mut xs = []\n\
                  \x20   let t = fs::read_to_string(\"x\", fs::Root::Anywhere) catch { \"\" }\n\
                  \x20   xs.push(t.len())\n\
                  \x20   return xs\n\
                  }\n\
                  comptime TABLE: Array[i64, 1] = lines()\n\
                  fn main() { println(f\"{TABLE[0]}\") }\n";
    let found = findings(source);
    assert!(
        found.iter().any(|f| f.code == "NK1152"),
        "the callee the rule forbids: {found:#?}"
    );
    assert!(
        !found
            .iter()
            .any(|f| f.code == "NK1166" || f.code == "NK1127"),
        "and nothing about the declaration, which is right: {found:#?}"
    );
}

/// **Text while the program is built**, which the `NK1127` note had been
/// calling out as missing since the evaluator gained a call.
///
/// [ADR-311](../../../docs/specification/adr/adr-311.md) D1's other half, and
/// the simpler one: a `String` arrives as a `&str`, there is no length in the
/// type, and `const X: &str` is what the language below has where
/// `const X: String` is not.
#[test]
fn text_is_computed_while_the_program_is_built() {
    let source = "comptime NAME: ref String = \"nikaia\"\n\
                  fn main() { println(NAME) }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const NAME: &str = \"nikaia\";"),
        "{}",
        lowered(source)
    );
}

/// **`f"…"` is what makes it worth having.** A hole is Nikaia
/// ([ADR-309](../../../docs/specification/adr/adr-309.md) D13), so this
/// evaluator reads it like every other analysis does — and a banner built from
/// two other constants is the case a person actually writes.
#[test]
fn an_interpolation_is_built_from_what_the_build_knows() {
    let source = "comptime MAJOR: i64 = 0\n\
                  comptime MINOR: i64 = 1\n\
                  comptime BANNER: ref String = f\"nikaia {MAJOR}.{MINOR}\"\n\
                  fn main() { println(BANNER) }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const BANNER: &str = \"nikaia 0.1\";"),
        "{}",
        lowered(source)
    );
}

/// **The escapes are the source's, unchanged**, which is what holding the
/// *written* form buys: the same literal produces the same bytes whether it is
/// read at build time or at run time, because the emitter passes a `.nika`
/// string's escapes into the Rust literal untouched and so does this.
#[test]
fn the_escapes_are_the_ones_the_source_wrote() {
    let source = "comptime GREETING: ref String = \"a\\tb\\nc \\\"quoted\\\" {brace}\"\n\
                  comptime JOINED: ref String = \"left\" + \"/\" + \"right\"\n\
                  fn main() { println(GREETING) println(JOINED) }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(
        rust.contains("const GREETING: &str = \"a\\tb\\nc \\\"quoted\\\" {brace}\";"),
        "{rust}"
    );
    assert!(
        rust.contains("const JOINED: &str = \"left/right\";"),
        "{rust}"
    );
}

/// A body that hands back a `String`, which is the crossing rather than a
/// literal: the declaration says `&str` and the value is the build's, so what
/// the program holds is a view of it.
#[test]
fn a_string_a_body_built_crosses_as_a_view() {
    let source = "fn greeting() -> String {\n\
                  \x20   return f\"hello {1}\"\n\
                  }\n\
                  comptime NAME: ref String = greeting()\n\
                  fn main() { println(NAME) }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const NAME: &str = \"hello 1\";"),
        "{}",
        lowered(source)
    );
}

/// `NK1167`'s text half ([ADR-311](../../../docs/specification/adr/adr-311.md)
/// D2). The way out used to be `NK1166`'s *write `.to_string()`*, which is
/// advice that makes the problem worse: a `String` is the one thing a `const`
/// cannot hold.
#[test]
fn a_constant_declared_a_string_is_sent_to_the_view() {
    let found = findings("comptime NAME: String = \"nikaia\"\nfn main() { println(NAME) }\n");
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1167")
        .unwrap_or_else(|| panic!("NK1167: {found:#?}"));
    assert!(
        refusal
            .help
            .as_deref()
            .is_some_and(|h| h.contains("Declare it as `ref String`")),
        "the view-shaped equivalent, not `.to_string()`: {:?}",
        refusal.help
    );
    // In the language's words (ADR-184 D2, D4): `&str` is a spelling the
    // parser refuses, so a way out that named it could not be taken.
    let said = format!("{:?} {:?} {}", refusal.notes, refusal.help, refusal.message);
    assert!(!said.contains('&') && !said.contains("[T; N]"), "{said}");
}

/// **A question about the value is answered**, which 0.0.112 refused and
/// 0.0.113 does not.
///
/// That refusal was a *representation* showing through: text was held as the
/// source wrote it, so `"\u{0041}"` was six characters and `== "A"` was
/// unanswerable. Which escapes exist is not a question this language left open,
/// though no page states it — the parser takes `\` and any character and hands
/// the literal to the backend, so `rustc` decides, and `"a\qb"` is *unknown
/// character escape* on the `.nika` line. So a decoder is a faithful reading.
#[test]
fn a_question_about_the_value_is_answered() {
    let source = "comptime SAME: bool = \"\\u{0041}\" == \"A\"\n\
                  comptime N: i64 = \"\\u{0041}\".len()\n\
                  fn main() { println(f\"{SAME} {N}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("const SAME: bool = true;"), "{rust}");
    assert!(
        rust.contains("const N: i64 = 1;"),
        "one character, not six:\n{rust}"
    );
}

/// An escape `rustc` would reject is not given a meaning here either: the
/// program does not compile whichever stage reads it, and this says *cannot
/// evaluate* rather than inventing one.
#[test]
fn an_escape_the_backend_rejects_is_not_invented_here() {
    let found = findings("comptime X: ref String = \"a\\qb\"\nfn main() { println(X) }\n");
    assert!(found.iter().any(|f| f.code == "NK1127"), "{found:#?}");
}

/// **A `sync` method of this program's own folds**, which is what the owner
/// expected of it and what 0.0.113 had to say it did not do.
///
/// The wall was not `sync` — that is [ADR-287](../../../docs/specification/adr/adr-287.md)
/// D1's **permission**, and the ledger check it drives applies to a method's
/// key exactly as to a function's. It was that a method needs a **value** to be
/// called on, and this evaluator had none to make.
#[test]
fn a_method_of_this_program_folds() {
    let source = "struct Point { x: i64, y: i64 }\n\
                  impl Point {\n\
                  \x20   fn scaled(ref self, by: i64) -> Point sync {\n\
                  \x20       return Point { x: self.x * by, y: self.y * by }\n\
                  \x20   }\n\
                  \x20   fn sum(ref self) -> i64 sync { return self.x + self.y }\n\
                  \x20   fn twice_the_sum(ref self) -> i64 sync { return self.sum() * 2 }\n\
                  }\n\
                  comptime ORIGIN: Point = Point { x: 1, y: 2 }\n\
                  comptime BIG: Point = ORIGIN.scaled(10)\n\
                  comptime TOTAL: i64 = BIG.twice_the_sum()\n\
                  fn main() { println(f\"{TOTAL}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    // A `struct` of values that own nothing is already its own view, so it
    // lands like a number does.
    assert!(
        rust.contains("const ORIGIN: Point = Point { x: 1, y: 2 };"),
        "{rust}"
    );
    // A method **on a named constant**, handing back a struct.
    assert!(
        rust.contains("const BIG: Point = Point { x: 10, y: 20 };"),
        "{rust}"
    );
    // And a method that calls another one on `self`.
    assert!(rust.contains("const TOTAL: i64 = 60;"), "{rust}");
}

/// **The permission is still the ledger's**, which is the half that must not
/// have moved: a method that reaches the world is `NK1152` by its own key, the
/// same sentence a free function gets ([ADR-287](../../../docs/specification/adr/adr-287.md)
/// D1, D2).
#[test]
fn a_method_the_rule_forbids_is_named_by_its_key() {
    let source = "use std::fs\n\
                  struct Reader { n: i64 }\n\
                  impl Reader {\n\
                  \x20   fn read(ref self) -> i64 {\n\
                  \x20       let t = fs::read_to_string(\"x\", fs::Root::Anywhere) catch { \"\" }\n\
                  \x20       return t.len() + self.n\n\
                  \x20   }\n\
                  }\n\
                  comptime N: i64 = Reader { n: 1 }.read()\n\
                  fn main() { println(f\"{N}\") }\n";
    let found = findings(source);
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1152")
        .unwrap_or_else(|| panic!("NK1152: {found:#?}"));
    assert!(
        refusal.message.contains("`Reader::read`"),
        "by the key a call resolves to: {}",
        refusal.message
    );
}

/// **A `struct` is only as writable as its fields**, and the way out has to be
/// one that can be taken — which the first shape of this was not: it asked the
/// **value**, so it told a program that had already declared `Array[i64, 3]` to
/// declare `Array[T, N]`. It asks the **declaration** now.
#[test]
fn a_field_a_const_cannot_hold_is_named_and_the_way_out_works() {
    let refused = "struct Bag { items: Vec[i64] }\n\
                   comptime B: Bag = Bag { items: [1, 2, 3] }\n\
                   fn main() { println(f\"{B.items.len()}\") }\n";
    let found = findings(refused);
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1167")
        .unwrap_or_else(|| panic!("NK1167: {found:#?}"));
    assert!(
        refusal.message.contains("`B.items` is a `Vec[i64]`"),
        "it names the field and what it is: {}",
        refusal.message
    );

    // And the way out is taken, which is the assertion that matters.
    let taken = "struct Bag { items: Array[i64, 3], label: ref String }\n\
                 impl Bag {\n\
                 \x20   fn total(ref self) -> i64 sync {\n\
                 \x20       let mut sum = 0\n\
                 \x20       for i in 0..<3 { sum = sum + self.items[i] }\n\
                 \x20       return sum\n\
                 \x20   }\n\
                 }\n\
                 comptime B: Bag = Bag { items: [1, 2, 3], label: \"bag\" }\n\
                 comptime TOTAL: i64 = B.total()\n\
                 fn main() { println(f\"{B.label} {TOTAL}\") }\n";
    assert!(findings(taken).is_empty(), "{:#?}", findings(taken));
}

/// **A constant is an item, so it is visible wherever its file is** (0.0.116).
///
/// A function declared below its caller has always been callable — items are
/// order-independent — and a constant was not, because the walk that binds them
/// goes down the file. `comptime A = B * 2` above `comptime B = 21` was
/// `NK1117`, *nothing declares `B`*: a **correct program refused**
/// ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)) with a
/// sentence that was not true, since the next line declares it.
#[test]
fn a_constant_may_stand_above_the_one_it_reads() {
    let source = "comptime A: i64 = B * 2\n\
                  comptime B: i64 = 21\n\
                  fn main() { println(f\"{A}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("const A: i64 = 42;"), "{rust}");
    assert!(rust.contains("const B: i64 = 21;"), "{rust}");
}

/// **`NK1168`: and the ring that buys**, refused by name.
///
/// Once a constant may read one declared later, `comptime A = B` beside
/// `comptime B = A` becomes writable — and it has no base case to reach, so it
/// is not the call depth that catches it ([ADR-287](../../../docs/specification/adr/adr-287.md)
/// D4's neighbour, which is about a recursion that *would* end if the stack
/// were deeper). The stack of names being worked out is, and it can say which.
#[test]
fn a_ring_of_constants_is_refused_once_and_named() {
    let found = findings(
        "comptime A: i64 = B\n\
         comptime B: i64 = A\n\
         fn main() { println(f\"{A}\") }\n",
    );
    let rings: Vec<&nikaia::check::Finding> = found.iter().filter(|f| f.code == "NK1168").collect();
    // **One ring, one error.** Both constants are circular and each would
    // report the same loop from a different corner.
    assert_eq!(rings.len(), 1, "{found:#?}");
    assert!(
        rings[0].message.contains("`A` depends on itself"),
        "the constant on this line: {}",
        rings[0].message
    );
    assert!(
        rings[0].notes[0].contains("`B` → `A` → `B`"),
        "and the ring it goes round: {:#?}",
        rings[0].notes
    );
}

/// **A float is a build-time value** (0.0.125).
///
/// It was not, and the absence was invisible from the outside: `comptime R =
/// 1.5` had nothing to evaluate to, so a `comptime` with a float anywhere in it
/// was `NK1127` — *this compiler cannot evaluate it* — for a value the language
/// below writes as `const R: f64 = 1.5;`.
///
/// **The literal is written with `{:?}` and not `{}`**, because Rust's `Debug`
/// for a float is the shortest text that reads back as the same bits. `0.1` is
/// the case that says so: `{}` and `{:?}` agree here, and they stop agreeing
/// the moment arithmetic has run.
#[test]
fn a_float_crosses_as_the_bits_the_build_had() {
    let source = "fn half(n: f64) -> f64 { return n / 2.0 }\n\
                  comptime R: f64 = 1.5\n\
                  comptime H: f64 = half(R)\n\
                  fn main() { println(f\"{R} {H}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("const R: f64 = 1.5;"), "{rust}");
    assert!(rust.contains("const H: f64 = 0.75;"), "{rust}");
}

/// **A list of lists crosses as an array of arrays** (0.0.125).
///
/// `rust_array_type` read the first element and asked what *it* was, and the
/// answer for a list was nothing — so `[[1, 2], [3, 4]]` was `NK1127` while
/// `[1, 2, 3]` had crossed since [ADR-311](../../../docs/specification/adr/adr-311.md)
/// D1. One recursion, and the rule is the same at every depth.
#[test]
fn a_nested_list_crosses_as_a_nested_array() {
    let source = "comptime GRID: Array[Array[i64, 2], 2] = [[1, 2], [3, 4]]\n\
                  fn main() { println(f\"{GRID[1][0]}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const GRID: [[i64; 2]; 2] = [[1, 2], [3, 4]];"),
        "{}",
        lowered(source)
    );
}

/// **And a `struct` may hold one** (0.0.125).
///
/// A field declared `Array[T, N]` was refused by the walk that asks whether a
/// field can be written as a `const` — it read `Array` as a type the language
/// below has no constant form for, which is the opposite of true. The check is
/// now the same recursion the value takes.
#[test]
fn a_struct_field_may_be_a_fixed_array() {
    let source = "struct Row { a: i64, b: i64 }\n\
                  struct Holder { rows: Array[Row, 2] }\n\
                  comptime HELD: Holder = Holder { rows: [Row { a: 1, b: 2 }, Row { a: 3, b: 4 }] }\n\
                  fn main() { println(f\"{HELD.rows[1].a}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains(
            "const HELD: Holder = Holder { rows: [Row { a: 1, b: 2 }, Row { a: 3, b: 4 }] };"
        ),
        "{}",
        lowered(source)
    );
}

/// **An `enum` crosses, by the variant the value *is*** (0.0.125).
///
/// A variant had no build-time value at all, so `comptime PICK = Shade::Odd`
/// was `NK1127` for a line Rust writes as `const PICK: Shade = Shade::Odd;`.
/// The four shapes are here at once: no payload, a float, two things at a time,
/// and a variant inside an array — because what a variant carries is the part
/// this has to get right, and a test with one variant in it would pass for a
/// compiler that wrote the first one every time.
#[test]
fn an_enum_variant_crosses_with_what_it_carries() {
    let source = "enum Shape { Empty, Num(f64), Pair(i64, ref String) }\n\
                  comptime A: Shape = Shape::Num(1.5)\n\
                  comptime B: Shape = Shape::Pair(7, \"x\")\n\
                  comptime C: Array[Shape, 2] = [Shape::Empty, Shape::Num(2.0)]\n\
                  fn main() {\n\
                  \x20   match B {\n\
                  \x20       Shape::Pair(n, s) => { println(f\"{n}{s}\") }\n\
                  \x20       else => { println(\"other\") }\n\
                  \x20   }\n\
                  \x20   match A {\n\
                  \x20       Shape::Num(v) => { println(f\"{v}\") }\n\
                  \x20       else => { println(\"other\") }\n\
                  \x20   }\n\
                  \x20   println(f\"{C.len()}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("const A: Shape = Shape::Num(1.5);"), "{rust}");
    assert!(
        rust.contains("const B: Shape = Shape::Pair(7, \"x\");"),
        "{rust}"
    );
    assert!(
        rust.contains("const C: [Shape; 2] = [Shape::Empty, Shape::Num(2.0)];"),
        "{rust}"
    );
}

/// **`NK1167` asks the value and not the type**, which is what an `enum` makes
/// unavoidable.
///
/// `Shape::Empty` is a `const` and `Shape::Many([1, 2])` is not, and the two
/// are the same `Shape` — so a walk over the declared *type* has no answer to
/// give. Without this the second one lowered and `rustc` answered about the
/// generated file, which is [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s
/// class.
///
/// The sentence names the **variant**, because what a reader changes is that
/// variant's declaration — and it says the rest of the `enum` is fine, because
/// it is.
#[test]
fn a_variant_that_owns_memory_is_refused_by_name() {
    let found = findings(
        "enum Shape { Empty, Many(Vec[i64]) }\n\
         comptime M: Shape = Shape::Many([1, 2])\n\
         fn main() { println(\"hi\") }\n",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1167");
    assert!(
        found[0]
            .message
            .contains("`Shape::Many` carries a `Vec[i64]`"),
        "{}",
        found[0].message
    );
    assert!(
        found[0].notes[0].contains("Another variant of the same `enum` may be fine"),
        "{:#?}",
        found[0].notes
    );
}

/// **A variant with named fields wears a struct literal, and it is its `enum`**
/// (0.0.125).
///
/// `Shape::Spot { x: 1 }` typed as `Shape::Spot`, which is not a type any
/// declaration can be written as — so a correct function was `NK1104`, *hands
/// back `Shape::Spot` and declares `Shape`*, with a way out that cannot be
/// taken ([Part III C.2](../../../docs/specification/30-nikaia-tooling.md)).
/// Its fields went unchecked in the same breath, because nothing had them
/// under that name.
#[test]
fn a_variant_with_named_fields_is_its_enum() {
    let source = "enum Shape { Dot, Spot { x: i64 } }\n\
                  fn make() -> Shape { return Shape::Spot { x: 3 } }\n\
                  comptime S: Shape = Shape::Spot { x: 4 }\n\
                  fn main() {\n\
                  \x20   match make() {\n\
                  \x20       Shape::Spot { x } => { println(f\"{x}\") }\n\
                  \x20       else => { println(\"dot\") }\n\
                  \x20   }\n\
                  \x20   match S {\n\
                  \x20       Shape::Spot { x } => { println(f\"{x}\") }\n\
                  \x20       else => { println(\"dot\") }\n\
                  \x20   }\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("const S: Shape = Shape::Spot { x: 4 };"),
        "{}",
        lowered(source)
    );
}

/// …and its fields are checked, which is the half that was silent.
#[test]
fn a_variant_field_of_the_wrong_type_is_refused() {
    let found = findings(
        "enum Shape { Dot, Spot { x: i64 } }\n\
         fn make() -> Shape { return Shape::Spot { x: \"no\" } }\n\
         fn main() { match make() { else => { println(\"hi\") } } }\n",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1106");
    assert!(
        found[0]
            .message
            .contains("`Shape::Spot.x` holds `i64`, but you're giving it `ref String`"),
        "{}",
        found[0].message
    );
}

/// The same operations through a call, so the evaluator answers and not the
/// fold in front of it.
#[test]
fn build_time_integers_through_a_call() {
    let source = "fn mask(a: i64, b: i64) -> i64 { return a & b }\n\
                  fn halve(a: i64) -> i64 { return a >> 1 }\n\
                  fn left(a: i64, b: i64) -> i64 { return a % b }\n\
                  fn below(a: i64, b: i64) -> bool { return a < b }\n\
                  fn lowest() -> i64 { return 0 - 9223372036854775807 - 1 }\n\
                  comptime MASKED: i64 = mask(-6, 7)\n\
                  comptime HALVED: i64 = halve(-7)\n\
                  comptime LEFT: i64 = left(-7, 3)\n\
                  comptime ORDER: bool = below(-2, 1)\n\
                  comptime LOWEST: i64 = lowest()\n\
                  fn main() {\n\
                  println(f\"{MASKED} {HALVED} {LEFT} {ORDER} {LOWEST}\")\n\
                  }\n";
    assert!(
        lowered(source).contains("const MASKED"),
        "{}",
        lowered(source)
    );
}
