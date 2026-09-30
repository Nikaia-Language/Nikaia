//! A block that joins on the executor pauses, and its branches travel in the
//! function's own channel
//! ([ADR-163](../../../docs/specification/adr/adr-163.md)).
//!
//! `overlap { … }` and `select { … }` are the two constructs that hand several
//! branches to the executor and wait there. Both were built before
//! [ADR-055](../../../docs/specification/adr/adr-055.md) made the lowering
//! `async` and before [ADR-157](../../../docs/specification/adr/adr-157.md)
//! gave the failure channel a type, and neither was asked again afterwards. So
//! each of them had a correct program that `rustc` refused, in a file the
//! author never wrote — [Part III C.1](../../../docs/specification/30-nikaia-tooling.md):
//!
//! * a block whose branches cannot pause left the enclosing function `sync`,
//!   and the vehicle's `.await` landed in a `fn` that is not `async` (`E0728`);
//! * a fallible branch was wrapped in `Ok::<_, Box<dyn Error>>` whatever the
//!   function's channel was, so the `?` under it could not convert (`E0277`).
//!
//! **These are run rather than read.** Both defects are about what the language
//! below accepts, and comparing the emitted string would say no more than that
//! this compiler agrees with itself.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// The `sync` violations a source has, as the check reports them.
fn violations(source: &str) -> Vec<nikaia::contracts::sync::Violation> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::contracts::sync::check(&parsed, &own, &library)
}

/// What the ledger derives for one function's `sync` column.
fn is_sync(source: &str, of: &str) -> bool {
    let parsed = parse_to_ast(source).expect("the source parses");
    Ledger::infer(&parsed).functions[of].sync_claim.is_sync()
}

/// Lower it, compile it, run it, hand back what it printed — which is the only
/// shape that holds C.1 closed.
fn output(purpose: &str, source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let found =
        check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings;
    assert!(
        found.is_empty(),
        "{purpose} is a correct program: {found:#?}"
    );

    let rust = lowered(source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering of {purpose} compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("the program runs");
    let printed = String::from_utf8_lossy(&ran.stdout).trim().to_string();
    let _ = std::fs::remove_dir_all(&dir);
    printed
}

/// Two branches that cannot pause, which is the ten-line program that used to
/// be refused.
const SYNC_OVERLAP: &str = "fn twice(n: i64) -> i64 {\n\
                            \x20   return n * 2\n\
                            }\n\
                            \n\
                            fn main() {\n\
                            \x20   let pair = overlap {\n\
                            \x20       twice(3)\n\
                            \x20       twice(4)\n\
                            \x20   }\n\
                            \x20   println(f\"{pair.0} {pair.1}\")\n\
                            }\n";

/// The same one construct over.
const SYNC_SELECT: &str = "fn a() -> i64 { return 1 }\n\
                           fn b() -> i64 { return 2 }\n\
                           \n\
                           fn main() {\n\
                           \x20   select {\n\
                           \x20       x = a() => { println(f\"{x}\") }\n\
                           \x20       y = b() => { println(f\"{y}\") }\n\
                           \x20   }\n\
                           }\n";

/// An error type of this program's own, so the channel is a `Thrown<E>`.
const OWN_ERROR: &str = "enum LoadError {\n\
                         \x20   Missing(String),\n\
                         \x20   Broken(String),\n\
                         }\n\
                         \n\
                         impl Error for LoadError {\n\
                         \x20   fn message(ref self) -> String {\n\
                         \x20       match self {\n\
                         \x20           LoadError::Missing(w) => { return f\"{w} is missing\" }\n\
                         \x20           LoadError::Broken(w) => { return f\"{w} is broken\" }\n\
                         \x20       }\n\
                         \x20   }\n\
                         }\n\
                         \n\
                         fn load(which: String) -> String throws {\n\
                         \x20   if which == \"a\" {\n\
                         \x20       throw LoadError::Missing(which)\n\
                         \x20   }\n\
                         \x20   if which == \"b\" {\n\
                         \x20       throw LoadError::Broken(which)\n\
                         \x20   }\n\
                         \x20   return which\n\
                         }\n";

/// **D1: an `overlap` of branches that cannot pause is still a pause.**
///
/// `task::overlap<n>` is an `async fn` and the block lowers to `.await` on it
/// whatever the branches do — so a function holding one parks on the executor,
/// and the `sync` column has to say so. It did not, and the ten lines below are
/// what that cost: `await is only allowed inside async functions`, about a file
/// the author never wrote.
#[test]
fn an_overlap_of_branches_that_cannot_pause_is_a_pause() {
    assert!(
        !is_sync(SYNC_OVERLAP, "main"),
        "the block is a suspension point, so `main` cannot be `sync`"
    );
    // And the lowering agrees with the column, which is the half `rustc` used
    // to have to point out.
    let rust = lowered(SYNC_OVERLAP);
    assert!(rust.contains("async fn __nikaia_main"), "{rust}");
    assert_eq!(output("joining-overlap-sync", SYNC_OVERLAP), "6 8");
}

/// The same for a `select`, because it is the same mechanism: `task::race<n>`
/// is awaited too.
#[test]
fn a_select_of_arms_that_cannot_pause_is_a_pause() {
    assert!(!is_sync(SYNC_SELECT, "main"), "a `select` parks too");
    let rust = lowered(SYNC_SELECT);
    assert!(rust.contains("async fn __nikaia_main"), "{rust}");
    // One of the two arms wins and the other is dropped; which one is the
    // executor's to say, so this asserts that *an* arm answered.
    let printed = output("joining-select-sync", SYNC_SELECT);
    assert!(printed == "1" || printed == "2", "{printed}");
}

/// **A branch that can pause still makes the block one**, which is the case
/// that always worked and is asserted here so the new answer is not the only
/// answer.
#[test]
fn a_branch_that_pauses_still_makes_the_block_pause() {
    let source = "use std::fs\n\
                  \n\
                  fn main() throws {\n\
                  \x20   let r = overlap {\n\
                  \x20       fs::read_to_string(\"eins.txt\", fs::Root::Anywhere)\n\
                  \x20       fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere)\n\
                  \x20   }\n\
                  \x20   println(f\"{r.0} {r.1}\")\n\
                  }\n";
    assert!(!is_sync(source, "main"));
}

/// **D1's other half: an assertion is contradicted too** (`NK2202`).
///
/// [ADR-027](../../../docs/specification/adr/adr-027.md) D4 says the inference
/// never overwrites a written `sync`, so fixing the derivation alone would have
/// left a hand-written one on a body that pauses — and `rustc` would say so
/// about the generated file, which is the defect this record is about.
#[test]
fn a_written_sync_over_a_joining_block_is_refused() {
    let source = "fn twice(n: i64) -> i64 { return n * 2 }\n\
                  \n\
                  fn both() -> i64 sync {\n\
                  \x20   let pair = overlap {\n\
                  \x20       twice(3)\n\
                  \x20       twice(4)\n\
                  \x20   }\n\
                  \x20   return pair.0 + pair.1\n\
                  }\n";
    let found = violations(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].caller, "both");
    assert_eq!(found[0].callee, "overlap");
    // **A construct has no ledger entry**, so the diagnostic says where the
    // pause is instead of naming one that does not exist.
    assert!(found[0].construct, "{found:#?}");
    let rendered = nikaia::diagnostics::render_sync_violation(&found[0], "both.nika", source);
    assert!(rendered.contains("error[NK2202]"), "{rendered}");
    assert!(
        rendered.contains("waits for its branches to finish, which is a pause"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("carries no `sync`"),
        "a construct is not a ledger entry:\n{rendered}"
    );
}

/// **D2: a branch travels in the function's own channel.**
///
/// The vehicle needs the error type written out — an `async` block with a `?`
/// in it and nothing to infer from is *type annotations needed* about a
/// generated file. Naming the box was right while every channel was one; since
/// [ADR-157](../../../docs/specification/adr/adr-157.md) it can be a
/// `Thrown<E>`, and `?` does not convert one into the other.
#[test]
fn a_branch_travels_in_the_functions_own_channel() {
    let source = format!(
        "{OWN_ERROR}\n\
         fn main() throws {{\n\
         \x20   let pair = overlap {{\n\
         \x20       load(\"c\")\n\
         \x20       load(\"d\")\n\
         \x20   }}\n\
         \x20   println(f\"{{pair.0}} {{pair.1}}\")\n\
         }}\n"
    );
    let rust = lowered(&source);
    assert!(
        rust.contains("Ok::<_, nikaia_std::error::Thrown<LoadError>>("),
        "{rust}"
    );
    assert!(
        !rust.contains("Ok::<_, Box<dyn std::error::Error>>("),
        "the box is not this function's channel:\n{rust}"
    );
    assert_eq!(output("joining-own-channel", &source), "c d");
}

/// **And a library's channel** — which travelled **bare** until
/// [ADR-115](../../../docs/specification/adr/adr-115.md) D1 put an envelope on
/// it for the sake of the `secondary` list.
///
/// The defect this was written for is the same from the other side: the `?` on
/// a branch had to convert a box into the channel, which it cannot. What
/// changed since is *which* channel, not that the branch carries the
/// function's: [ADR-159](../../../docs/specification/adr/adr-159.md) D2's
/// reason was about the **site**, and the envelope this puts on still says
/// *no site recorded*.
#[test]
fn a_branch_travels_in_a_librarys_channel_too() {
    let source = "use std::fs\n\
                  \n\
                  fn main() throws {\n\
                  \x20   let r = overlap {\n\
                  \x20       fs::read_to_string(\"eins.txt\", fs::Root::Anywhere)\n\
                  \x20       fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere)\n\
                  \x20   }\n\
                  \x20   println(f\"{r.0} {r.1}\")\n\
                  }\n";
    let rust = lowered(source);
    assert!(
        rust.contains("Ok::<_, nikaia_std::error::Thrown<io::IoError>>("),
        "{rust}"
    );
}

/// **D5 still holds, and the second failure is kept** — which is
/// [ADR-115](../../../docs/specification/adr/adr-115.md) D2, and the line this
/// test was written to have changed.
///
/// Two branches fail. The first in **written** order is the block's error
/// ([ADR-050](../../../docs/specification/adr/adr-050.md) D5), which is
/// unchanged and is the only order the source has. What is new is that the
/// second one is **under** it rather than gone: the block waits for every
/// branch, so when it ends every outcome is known and the list is a fact
/// rather than a race.
///
/// Written with the failure leaving `main`, because that is where an operator
/// reads it: a `main` that hands back an `Err` is printed through `Debug`, and
/// a short form there would be the one place the joined failures are dropped.
#[test]
fn the_block_keeps_every_failure_and_the_first_still_wins() {
    let source = format!(
        "{OWN_ERROR}\n\
         fn main() throws {{\n\
         \x20   let pair = overlap {{\n\
         \x20       load(\"a\")\n\
         \x20       load(\"b\")\n\
         \x20   }}\n\
         \x20   println(f\"{{pair.0}} {{pair.1}}\")\n\
         }}\n"
    );
    let rust = lowered(&source);
    let dir = common::scratch_dir("joining-every-failure");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "it compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("the program runs");
    let said = String::from_utf8_lossy(&ran.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(!ran.status.success(), "a failing block fails the program");
    assert!(
        said.contains("a is missing"),
        "the first branch in written order is the block's failure:\n{said}"
    );
    // **And the second is there now**, which is the whole of ADR-115.
    assert!(
        said.contains("is broken"),
        "the later failure joined rather than being dropped:\n{said}"
    );
    // **Under it, not beside it**: the order on the page is the order they
    // joined, which is written order.
    let first = said.find("a is missing").expect("the first");
    let second = said.find("is broken").expect("the second");
    assert!(first < second, "the winner is printed first:\n{said}");
    assert!(said.contains("and then:"), "{said}");
}

// ---------------------------------------------------------------------------
// [ADR-164](../../../docs/specification/adr/adr-164.md): the block's outcome is
// one, and a handler on the block binds what the branches threw.
// ---------------------------------------------------------------------------

/// **D1: `overlap { … } catch { … }` lowers** — [ADR-115](../../../docs/specification/adr/adr-115.md)
/// D4's own written example, which did not.
///
/// A `?` per branch leaves the **function**, so a `catch` on the block got a
/// `match` over the vehicle's tuple as though it were a `Result`. The branches
/// become one outcome in `std` instead, which is also the one place that sees
/// every branch's result — where that record's `secondary` list goes.
#[test]
fn a_handler_on_the_block_gets_the_blocks_outcome() {
    let source = format!(
        "{OWN_ERROR}\n\
         fn main() {{\n\
         \x20   let pair = overlap {{\n\
         \x20       load(\"a\")\n\
         \x20       load(\"b\")\n\
         \x20   }} catch {{\n\
         \x20       println(f\"caught: {{error}}\")\n\
         \x20       return\n\
         \x20   }}\n\
         \x20   println(f\"{{pair.0}} {{pair.1}}\")\n\
         }}\n"
    );
    let rust = lowered(&source);
    assert!(rust.contains("nikaia_std::task::combine2("), "{rust}");
    // **And no `?`**, because the handler is right here: the `match` around the
    // block is what takes the outcome apart.
    assert!(
        !rust.contains("combine2(__nikaia_branch_0, __nikaia_branch_1)?"),
        "a handler on the block keeps the outcome:\n{rust}"
    );
    assert_eq!(
        output("joining-catch-on-the-block", &source),
        "caught: a is missing"
    );
}

/// **D2: and the handler binds what the *branches* threw.**
///
/// The enclosing function may not be `throws` at all, so its channel is the
/// box — and `match error { LoadError::Missing(w) => … }`, which is Part I
/// 7.1's whole point, does not compile over one. The block has a set of its
/// own: the union of its branches'.
#[test]
fn a_handler_on_the_block_matches_the_branches_variants() {
    let source = format!(
        "{OWN_ERROR}\n\
         fn main() {{\n\
         \x20   let pair = overlap {{\n\
         \x20       load(\"a\")\n\
         \x20       load(\"b\")\n\
         \x20   }} catch {{\n\
         \x20       match error {{\n\
         \x20           LoadError::Missing(w) => {{ println(f\"missing {{w}}\") }}\n\
         \x20           LoadError::Broken(w) => {{ println(f\"broken {{w}}\") }}\n\
         \x20       }}\n\
         \x20       return\n\
         \x20   }}\n\
         \x20   println(f\"{{pair.0}} {{pair.1}}\")\n\
         }}\n"
    );
    let rust = lowered(&source);
    assert!(
        rust.contains("Ok::<_, nikaia_std::error::Thrown<LoadError>>("),
        "the branches travel in the block's own channel:\n{rust}"
    );
    // The envelope is opened once at the binding, as ADR-157 D2 says.
    assert!(rust.contains(".split();"), "{rust}");
    assert_eq!(
        output("joining-match-on-the-block", &source),
        "missing a",
        "the first failure in written order is what the handler is handed"
    );
}

/// **The same for a `select`**, whose arms take their failure apart themselves
/// where the handler is on the block — a `?` there would leave the function.
#[test]
fn a_handler_on_a_select_gets_the_blocks_outcome() {
    let source = format!(
        "{OWN_ERROR}\n\
         fn main() {{\n\
         \x20   select {{\n\
         \x20       x = load(\"a\") => {{ println(f\"a {{x}}\") }}\n\
         \x20       y = load(\"b\") => {{ println(f\"b {{y}}\") }}\n\
         \x20   }} catch {{\n\
         \x20       println(f\"caught: {{error}}\")\n\
         \x20   }}\n\
         }}\n"
    );
    let printed = output("joining-catch-on-a-select", &source);
    assert!(
        printed == "caught: a is missing" || printed == "b b",
        "whichever arm wins, the block answers: {printed}"
    );
}

/// **D3: and `rustc` says nothing about any of it.**
///
/// Two of the shapes above were `unused_braces` and one was *unreachable call*
/// — warnings about generated files, which is
/// [Part III C.1](../../../docs/specification/30-nikaia-tooling.md) one
/// severity down. The handler with **one statement** over a named channel is
/// what reached the first, and a `throws` function whose body is a bare `throw`
/// the second; neither needs an `overlap` in it.
#[test]
fn rustc_says_nothing_about_the_generated_file() {
    let programs = [
        // A one-statement handler over a named channel: the envelope used to be
        // opened as a block wrapped around the handler.
        format!(
            "{OWN_ERROR}\n\
             fn main() {{\n\
             \x20   let got = load(\"a\") catch {{\n\
             \x20       println(f\"caught: {{error}}\")\n\
             \x20       return\n\
             \x20   }}\n\
             \x20   println(f\"{{got}}\")\n\
             }}\n"
        ),
        // A `throws` function whose body is a bare `throw`: `Ok(` around it is
        // `Ok(return Err(…))`.
        "enum Boom { Now }\n\
         \n\
         impl Error for Boom {\n\
         \x20   fn message(ref self) -> String { return \"boom\" }\n\
         }\n\
         \n\
         fn always() -> String throws {\n\
         \x20   throw Boom::Now\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let got = always() catch { return }\n\
         \x20   println(f\"{got}\")\n\
         }\n"
        .to_string(),
    ];
    for (at, source) in programs.iter().enumerate() {
        let rust = lowered(source);
        let dir = common::scratch_dir(&format!("joining-quiet-{at}"));
        let file = dir.join("main.rs");
        std::fs::write(&file, &rust).expect("write the Rust");
        let out = common::compile(&file, &["-o", dir.join("program").to_str().expect("path")]);
        let said = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "{said}\n--- the Rust ---\n{rust}");
        assert!(
            said.trim().is_empty(),
            "rustc said something about the file it was handed:\n{said}\n--- the Rust ---\n{rust}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
