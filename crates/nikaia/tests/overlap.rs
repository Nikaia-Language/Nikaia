//! **`overlap { … }`** — Part I 8.1.2,
//! [ADR-050](../../../docs/specification/adr/adr-050.md) D2–D6.
//!
//! The one way a program asks for overlap, now that
//! [ADR-050](../../../docs/specification/adr/adr-050.md) D1 has withdrawn the
//! automatic half. What makes it worth a form of its own is D3: **every
//! mainstream construct for "run these together" takes the programmer's word
//! for the independence, and this one checks it** — against the touch sets
//! [ADR-033](../../../docs/specification/adr/adr-033.md) built for the
//! inference that D1 removes. That machinery is kept and asked the other way
//! round: not *"may I reorder these?"* but *"you said these overlap; is that
//! true?"*

mod common;

use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit;
use nikaia::parser::parse_to_ast;
use std::process::Command;

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check(&parsed, &own, &library).findings
}

fn lower(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit::emit_program(&parsed, Default::default())
        .expect("the source lowers")
        .rust
}

/// Lower, compile and run in a directory holding three files, and hand back
/// what it printed.
fn run(purpose: &str, source: &str) -> String {
    let dir = common::scratch_dir(purpose);
    std::fs::write(dir.join("eins.txt"), "eins").expect("write");
    std::fs::write(dir.join("zwei.txt"), "zweizwei").expect("write");
    std::fs::write(dir.join("drei.txt"), "dreidrei").expect("write");

    let rust = lower(source);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "an `overlap` did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run it");
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let out = String::from_utf8_lossy(&ran.stdout).to_string();
    std::fs::remove_dir_all(&dir).ok();
    out
}

const THREE_READS: &str = "use std::fs\n\
     \n\
     fn main() {\n\
     \x20   let r = overlap {\n\
     \x20       fs::read_to_string(\"eins.txt\", fs::Root::Anywhere) catch { \"\" }\n\
     \x20       fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere) catch { \"\" }\n\
     \x20       fs::read_to_string(\"drei.txt\", fs::Root::Anywhere) catch { \"\" }\n\
     \x20   }\n\
     \x20   println(f\"{r.0.len()} {r.1.len()} {r.2.len()}\")\n\
     }";

/// D2: every branch in flight, and the value is their results **in written
/// order**.
#[test]
fn every_branch_runs_and_the_value_is_in_written_order() {
    assert_eq!(run("overlap-three", THREE_READS).trim(), "4 8 8");
}

/// D6: **a branch is started up to its first suspension point before any branch
/// that cannot suspend is run.**
///
/// The naive order — written order until each suspends — would run the
/// computation to completion before either read was submitted, and the block
/// would cost their sum rather than their maximum. So the branches that can
/// pause are handed over first, which is the ledger's `sync` column read the
/// same way ADR-033's pairs read it.
///
/// **And the value goes back into written order**, because D2 says it is the
/// tuple in written order and the reordering is a schedule. This is the test
/// that says both halves happen: the argument order is not the written order,
/// and the answer is.
#[test]
fn a_branch_that_cannot_pause_is_started_last_and_answered_in_place() {
    let source = "use std::fs\n\
         \n\
         fn expensive(n: i64) -> i64 sync {\n\
         \x20   let mut sum = 0\n\
         \x20   for i in 0..<n { sum += i }\n\
         \x20   return sum\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let r = overlap {\n\
         \x20       expensive(10)\n\
         \x20       fs::read_to_string(\"eins.txt\", fs::Root::Anywhere) catch { \"\" }\n\
         \x20       fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere) catch { \"\" }\n\
         \x20   }\n\
         \x20   println(f\"{r.0} {r.1.len()} {r.2.len()}\")\n\
         }";

    let rust = lower(source);
    let reads = rust.find("read_to_string").expect("a read is emitted");
    let computation = rust.find("expensive(10)").expect("the computation");
    assert!(
        reads < computation,
        "the computation was started before the reads:\n{rust}"
    );
    assert!(rust.contains("ADR-050 D6"), "{rust}");

    // Written order out of start order, and the program says so.
    assert_eq!(run("overlap-d6", source).trim(), "45 4 8");
}

/// A block whose branches are all of one kind is emitted without a permutation.
///
/// The negative half of the test above: reordering costs a tuple rebuild, and a
/// block that does not need one must not pay for it.
#[test]
fn a_block_that_needs_no_reordering_gets_none() {
    let rust = lower(THREE_READS);
    assert!(!rust.contains("ADR-050 D6"), "{rust}");
    assert!(!rust.contains("__nikaia_branch_"), "{rust}");
    assert!(rust.contains("task::overlap3("), "{rust}");
}

/// **D3: the branches must meet on nothing, and the compiler checks it.**
///
/// ADR-050 D3's own example, and the message names the resource. This is the
/// part no other language has: every mainstream form for *run these together*
/// takes the programmer's word for it.
#[test]
fn two_branches_that_meet_on_a_resource_are_refused() {
    let found = findings(
        "fn main() {\n\
         \x20   let r = overlap {\n\
         \x20       println(\"x\")\n\
         \x20       println(\"y\")\n\
         \x20   }\n\
         \x20   println(f\"{r.0}\")\n\
         }",
    );
    let first = found
        .iter()
        .find(|f| f.code == "NK2104")
        .unwrap_or_else(|| panic!("{found:?}"));
    assert!(
        first
            .message
            .contains("These two branches can't run at the same time"),
        "{first:?}"
    );
    assert!(
        first.notes.iter().any(|n| n.contains("stdout")),
        "the message does not name the resource: {first:?}"
    );
}

/// …and two branches that meet on nothing are not refused, or the test above
/// would pass for a checker that refuses everything.
#[test]
fn two_branches_that_meet_on_nothing_are_accepted() {
    assert!(
        findings(THREE_READS).is_empty(),
        "{:?}",
        findings(THREE_READS)
    );
}

/// **A path in an argument is a constant, and it does not stop an overlap.**
///
/// The three branches above each write `fs::Root::Anywhere`
/// ([ADR-108](../../../docs/specification/adr/adr-108.md) D1 gives every path
/// call a root), and a path used to be accounted as *a value read from somewhere
/// else* — which is true of a field and an index and not of a path. A path names
/// an **item**: a unit variant, a constructor handed over as a value, an
/// associated constant. There is nothing to read, so there is nothing for the
/// branch's closure to capture.
///
/// Asserted separately from the block above because it is a different claim: that
/// one says three reads overlap, and this one says the *reason* they still do is
/// the narrowing and not something else in the pair.
#[test]
fn a_path_in_an_argument_does_not_stop_an_overlap() {
    let with_a_root = findings(THREE_READS);
    assert!(with_a_root.is_empty(), "{with_a_root:?}");
    // And the lowering is the overlapped one rather than three statements in a
    // row, which is what says the verdict was `Operation` and not a refusal.
    let rust = lower(THREE_READS);
    assert!(rust.contains("task::overlap3("), "{rust}");
}

/// A branch that **binds** is refused: the block's value already carries every
/// branch's result, so a `let` inside one would name a thing that leaves by two
/// doors (D2).
#[test]
fn a_branch_that_binds_a_name_is_refused() {
    let found = findings(
        "use std::fs\n\
         fn main() {\n\
         \x20   let r = overlap {\n\
         \x20       let x = fs::read_to_string(\"eins.txt\", fs::Root::Anywhere) catch { \"\" }\n\
         \x20       fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere) catch { \"\" }\n\
         \x20   }\n\
         \x20   println(f\"{r.1.len()}\")\n\
         }",
    );
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK2104" && f.message.contains("can't declare `x`")),
        "{found:?}"
    );
}

/// **D5: a branch whose failure is uncaught fails the block, and the first in
/// written order wins.**
///
/// The `?`s are written in written order and `?` returns at the first `Err`, so
/// the rule is the language below's own control flow rather than a comparison
/// this compiler makes.
#[test]
fn an_uncaught_failure_in_a_branch_fails_the_block() {
    let source = "use std::fs\n\
         \n\
         fn main() throws {\n\
         \x20   let r = overlap {\n\
         \x20       fs::read_to_string(\"eins.txt\", fs::Root::Anywhere)\n\
         \x20       fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere)\n\
         \x20   }\n\
         \x20   println(f\"{r.0.len()} {r.1.len()}\")\n\
         }";

    let rust = lower(source);
    // Wrapped all or none, with the error type named rather than inferred - an
    // `async` block with a `?` and nothing to infer from is "type annotations
    // needed" about a file nobody wrote. **The one the reads throw**, since a
    // tuple's part is typed (0.0.355): `r.0.len()` was a method on nothing
    // known, which left `main`'s errors open and named the catch-all.
    assert!(
        rust.contains("Ok::<_, nikaia_std::error::Thrown<io::IoError>>("),
        "{rust}"
    );
    // **One outcome out of the branches'**, in `std`
    // ([ADR-164](../../../docs/specification/adr/adr-164.md) D1): a `?` per
    // branch written here would leave the *function*, which is what a `catch` on
    // the block cannot allow — and one place that sees every outcome is where
    // ADR-115's `secondary` list goes.
    assert!(
        rust.contains("nikaia_std::task::combine2(__nikaia_branch_0, __nikaia_branch_1)?"),
        "{rust}"
    );

    // It runs when both files are there…
    assert_eq!(run("overlap-d5", source).trim(), "4 8");
}

/// …and a branch that catches its own failure is not made to propagate.
///
/// D5: per-branch handling is a `catch` inside the branch, unchanged. The
/// guarded half of a `catch` is what `branch_can_fail` does not count, which is
/// why `THREE_READS` above needs no `throws` on its `main`.
#[test]
fn a_branch_that_catches_its_own_failure_propagates_nothing() {
    let rust = lower(THREE_READS);
    assert!(
        !rust.contains("Ok::<_, Box<dyn std::error::Error>>("),
        "{rust}"
    );
    assert!(!rust.contains("__nikaia_branch_0?"), "{rust}");
}

/// **D4: an `overlap` is not a task, so nothing is moved into a branch.**
///
/// That is what makes the form lighter than two `spawn`s, and it is visible in
/// the emitted Rust: a task's body is `async move` and a branch's is `async`.
/// A branch borrows what is around it exactly as an ordinary statement does.
#[test]
fn a_branch_is_not_a_task_and_moves_nothing() {
    let rust = lower(THREE_READS);
    assert!(rust.contains("async {"), "{rust}");
    assert!(!rust.contains("async move"), "{rust}");
    assert!(!rust.contains("TaskHandle"), "{rust}");
}

/// A block of one branch has nothing to overlap with, and a block of more
/// branches than `std` has a vehicle for says so rather than miscompiling.
#[test]
fn a_block_too_small_or_too_large_is_refused_with_its_reason() {
    let parsed = parse_to_ast("fn main() { let r = overlap {\n    1\n} println(f\"{r}\") }")
        .expect("the source parses");
    let refused = emit::emit_program(&parsed, Default::default()).expect_err("refused");
    let said = format!("{refused:#}");
    assert!(said.contains("at least two branches"), "{said}");

    let many: String = (0..9).map(|n| format!("        {n}\n")).collect();
    let parsed = parse_to_ast(&format!(
        "fn main() {{ let r = overlap {{\n{many}}} println(f\"{{r.0}}\") }}"
    ))
    .expect("the source parses");
    let refused = emit::emit_program(&parsed, Default::default()).expect_err("refused");
    let said = format!("{refused:#}");
    assert!(
        said.contains("has 9 branches, but at most 8 are supported"),
        "{said}"
    );
}

/// Nothing about the runtime reaches a `.nika` file, which is the claim Part I
/// 8.1 makes and this construct is the one most able to break.
#[test]
fn nothing_an_overlap_needs_is_written_in_nikaia() {
    for word in ["async", "await", "Future", "join", "poll"] {
        assert!(!THREE_READS.contains(word), "`{word}` in a Nikaia program");
    }
}

/// **The report answers the question it is asked**, which it did not.
///
/// `--overlaps` exists because "the refusals are the compiler's own … that is
/// only fair if the refusals can be asked about". It used to print *"which meet
/// on nothing"* as a fixed header over every block, so a block the checker
/// refuses on the next line was described as meeting on nothing - the one answer
/// the tool must never give, because a reader consults it precisely when the
/// checker has said no.
///
/// Two blocks, one of each kind, so a fix that hard-codes the other verdict
/// fails here too.
#[test]
fn the_overlap_report_reads_the_verdicts_rather_than_asserting_them() {
    use nikaia::contracts::order::overlap_report;

    let report = |source: &str| {
        let parsed = parse_to_ast(source).expect("the source parses");
        let own = Ledger::infer(&parsed);
        let library = Ledger::parse(STD).expect("std's shipped ledger parses");
        let starts = emit::branch_starts_first(&parsed, Default::default(), &own);
        overlap_report(&parsed, &own, &library, &starts)
    };

    let meets =
        report("fn main() { let r = overlap {\n    println(\"a\")\n    println(\"b\")\n} }");
    assert!(
        meets.contains("may not run together") && meets.contains("both touch stdout"),
        "a block the checker refuses is described as refused:\n{meets}"
    );
    assert!(
        !meets.contains("which meet on nothing"),
        "and not also as meeting on nothing:\n{meets}"
    );

    let free = report("fn main() { let r = overlap {\n    1\n    2\n} }");
    assert!(
        free.contains("which meet on nothing"),
        "a block that does meet on nothing still says so:\n{free}"
    );
}
