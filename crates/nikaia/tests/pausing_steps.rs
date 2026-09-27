//! A `for` may iterate something whose step **pauses**
//! ([ADR-172](../../../docs/specification/adr/adr-172.md)).
//!
//! The question `docs/open-decisions.md` carried, answered A: the claim is a
//! word on the **type** in the ledger — exactly as
//! [ADR-025](../../../docs/specification/adr/adr-025.md) D6's `throws` on a
//! `Seq` already says a step can fail — and the emitter writes the awaiting
//! form for it.
//!
//! **The word is positive and the absence is not it**, which is the whole of
//! D1. A `Seq` that says `sync` does not pause, one that says `pauses` does,
//! and one that says neither is *nobody said* — a `map`'s step runs the
//! lambda, and what that does is the caller's. The two ways of being wrong are
//! not symmetric: awaiting a step that has none is `rustc` refusing a correct
//! program about a file nobody wrote ([Part III
//! C.1](../../../docs/specification/30-nikaia-tooling.md), C.4), and holding a
//! thread that could have been given up costs a thread.

mod common;

use std::collections::BTreeSet;

use nikaia::contracts::ty::Ty;
use nikaia::contracts::{Ledger, STD};
use nikaia::emit;
use nikaia::parser::parse_to_ast;

fn rust(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit::emit_program(&parsed, emit::Build::default())
        .expect("the program lowers")
        .rust
}

/// **The third word parses and writes itself back**, which is what makes it a
/// word of the ledger rather than a flag in this compiler (Part III, 13.5).
#[test]
fn a_sequence_says_whether_its_step_pauses() {
    let pausing = Ty::parse("Seq[String] pauses throws");
    assert!(
        matches!(
            &pausing,
            Ty::Seq {
                pauses: true,
                throws: true,
                is_sync: false,
                ..
            }
        ),
        "{pausing:?}"
    );
    assert_eq!(pausing.to_string(), "Seq[String] pauses throws");

    // The two states it is told apart from.
    assert!(matches!(
        Ty::parse("Seq[i64] sync"),
        Ty::Seq {
            is_sync: true,
            pauses: false,
            ..
        }
    ));
    assert!(matches!(
        Ty::parse("Seq[i64]"),
        Ty::Seq {
            is_sync: false,
            pauses: false,
            ..
        }
    ));
}

/// **`sync` and `pauses` are not both.** A ledger that writes both has said two
/// things about one step, and reading it as either would be a claim the file
/// does not make — the same rule a leftover word in the tail is refused by.
#[test]
fn a_step_does_not_both_pause_and_not() {
    assert!(
        !matches!(Ty::parse("Seq[i64] sync pauses"), Ty::Seq { .. }),
        "both words is not a sequence"
    );
    assert!(
        !matches!(Ty::parse("Seq[i64] pauses sync"), Ty::Seq { .. }),
        "and the order does not make it one"
    );
}

/// `pauses` fits the way `throws` does and the opposite way to `sync`: it is a
/// warning rather than a promise, so a step that pauses does not fit a position
/// that did not say it would.
#[test]
fn a_pausing_step_does_not_fit_where_nothing_said_it_would() {
    let pausing = Ty::parse("Seq[String] pauses");
    let quiet = Ty::parse("Seq[String]");
    assert!(!pausing.fits(&quiet));
    assert!(quiet.fits(&pausing));
    assert!(pausing.fits(&pausing));
}

/// **The loop gives its thread up**, which is the whole of D1 in one line of
/// emitted Rust: the sequence is bound once and stepped with a `.next().await`.
#[test]
fn a_for_over_a_pausing_sequence_awaits_each_step() {
    let rust = rust(
        "use std::io\n\nfn count() -> i64 throws {\n\
         \x20   let mut n = 0\n\
         \x20   for line in io::lines() { n += 1 }\n\
         \x20   return n\n\
         }",
    );
    assert!(
        rust.contains("let mut __nikaia_sequence = io::lines().await;"),
        "{rust}"
    );
    assert!(
        rust.contains("while let Some(line) = __nikaia_sequence.next().await"),
        "{rust}"
    );
    // The `for` is gone, not wrapped: two loops would be two walks of one
    // sequence, which ADR-105 D2 refuses outright.
    assert!(!rust.contains("for line in"), "{rust}");
}

/// **And nothing else moves.** A `for` over a container, a range, or a sequence
/// that says nothing keeps the shape it always had — which is the half a rule
/// about loops has to get right, because the other kind of mistake is every
/// program in the language.
#[test]
fn every_other_loop_keeps_its_shape() {
    for (source, expected) in [
        ("fn main() { for i in 0..<5 { } }", "for i in 0..5"),
        (
            "fn main(xs: Vec[i64]) { for x in xs { } }",
            "for x in xs.iter()",
        ),
        (
            "fn main(m: HashMap[String, i64]) { for k in m.keys() { } }",
            "for k in m.keys()",
        ),
        (
            "fn main(xs: Vec[i64]) { for x in xs.map fn { a } { } }",
            "for x in xs",
        ),
    ] {
        let rust = rust(source);
        assert!(rust.contains(expected), "{source}\n{rust}");
        assert!(!rust.contains("__nikaia_sequence"), "{source}\n{rust}");
    }
}

/// **The eager walks pause and can fail, and both words come from the
/// receiver.** `collect`, `count`, `nth` and `join` hand back a value, so each
/// is a loop around the step — and the step of this sequence does two things
/// the entry's own columns do not mention.
///
/// **This is the half whose absence was a wrong answer.**
/// `io::lines().count()` compiled before
/// [ADR-172](../../../docs/specification/adr/adr-172.md) D5, because `Lines`
/// was an `Iterator` over `Result<String, …>` — so it counted the *failures* as
/// lines. The ledger said `-> i64` and the program got one, with nothing
/// anywhere saying the number could be wrong. That is the bug class Part I 6.4
/// refuses by name, and it was in `std`.
#[test]
fn an_eager_walk_of_a_pausing_sequence_awaits_and_propagates() {
    for (call, expected) in [
        ("count()", "io::lines().await.count().await?"),
        ("collect()", "io::lines().await.collect().await?"),
        ("nth(2)", "io::lines().await.nth(2).await?"),
    ] {
        let rust = rust(&format!(
            "use std::io\n\nfn walk() -> i64 throws {{\n\
             \x20   let it = io::lines().{call}\n\
             \x20   return 0\n\
             }}"
        ));
        assert!(rust.contains(expected), "{call}\n{rust}");
    }
}

/// **And a function that walks one says `throws`** — `NK2701`, the same code
/// and the same rule a `for` over the same sequence gets
/// ([ADR-025](../../../docs/specification/adr/adr-025.md) D1): a step that can
/// fail fails the function around it, and nothing marks the call.
#[test]
fn a_walk_of_a_failing_sequence_makes_the_function_say_so() {
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let findings = |source: &str| {
        let parsed = parse_to_ast(source).expect("the source parses");
        let own = Ledger::infer(&parsed);
        nikaia::check::check_program(&parsed, &own, &library, &BTreeSet::new()).findings
    };

    let found = findings(
        "use std::io\n\nfn walk() -> i64 {\n\
         \x20   return io::lines().count()\n\
         }",
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK2701")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert!(
        refusal
            .message
            .contains("a step of what `count` walks can fail"),
        "{}",
        refusal.message
    );

    // …and with the `throws` it is an ordinary program.
    let found = findings(
        "use std::io\n\nfn walk() -> i64 throws {\n\
         \x20   return io::lines().count()\n\
         }",
    );
    assert!(!found.iter().any(|f| f.code == "NK2701"), "{found:#?}");
}

/// **The whole of it, compiled and run**, which is the assertion this package
/// exists for: the number.
///
/// `count()` gave the wrong one for as long as the entry existed — it counted
/// the failures as lines — and a test that only compiled the lowering would
/// have been green for that too. So this feeds three lines in and asks for a
/// three back.
#[test]
fn an_eager_walk_counts_the_lines_and_not_the_failures() {
    let rust = rust(
        "use std::io\n\n\
         fn walk() -> i64 throws {\n\
         \x20   return io::lines().count()\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{walk() catch { -1 }}\")\n\
         }",
    );
    let dir = common::scratch_dir("pausing-walk-run");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mut child = std::process::Command::new(&binary)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the program runs");
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("a pipe");
        stdin.write_all(b"one\ntwo\nthree\n").expect("written");
    }
    let done = child.wait_with_output().expect("the program ends");
    assert_eq!(
        String::from_utf8_lossy(&done.stdout).trim(),
        "3",
        "three lines in, three out"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **And the lowering compiles for each of them**, which is the only thing that
/// says the `.await` and the `?` landed on something that has them.
#[test]
fn the_eager_walks_compile() {
    let rust = rust(
        "use std::io\n\n\
         fn walk() -> i64 throws {\n\
         \x20   let lines = io::lines().collect()\n\
         \x20   return io::lines().count()\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{walk() catch { 0 }}\")\n\
         }",
    );
    let dir = common::scratch_dir("pausing-walks");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "the lowering compiles:\n{said}\n--- the Rust ---\n{rust}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_lazy_walk_of_a_pausing_sequence_is_refused_too() {
    let parsed = parse_to_ast(
        "use std::io\n\nfn main() {\n\
         \x20   let ls = io::lines().map fn { a }\n\
         }",
    )
    .expect("the source parses");
    let refused = emit::emit_program(&parsed, emit::Build::default())
        .expect_err("a walk with no form is refused");
    assert!(
        format!("{refused:#}")
            .contains("`map` walks a sequence whose step can fail as well as pause"),
        "{refused:#}"
    );
}

/// **A walk of a sequence that does not pause is untouched**, which is the half
/// that matters most: `keys()`, `chars()` and `drain()` are every other
/// sequence in `std`, and a refusal that reached them would be this rule
/// refusing correct programs (C.4).
#[test]
fn a_walk_of_an_ordinary_sequence_is_not_refused() {
    for source in [
        "fn main(m: HashMap[String, i64]) { let ks = m.keys().collect() }",
        "fn main(s: String) { let n = s.chars().count() }",
        "fn main(xs: Vec[i64]) { let j = xs.drain().join(\", \") }",
    ] {
        let parsed = parse_to_ast(source).expect("the source parses");
        assert!(
            emit::emit_program(&parsed, emit::Build::default()).is_ok(),
            "{source}"
        );
    }
}

/// A `break` and a `continue` mean in the awaiting loop what they mean in the
/// plain one, because the language below spells both the same way in a
/// `while let` — asserted rather than assumed, since the construct changed
/// underneath them.
#[test]
fn break_and_continue_still_mean_what_they_did() {
    let rust = rust(
        "use std::io\n\nfn count() -> i64 throws {\n\
         \x20   let mut n = 0\n\
         \x20   for line in io::lines() {\n\
         \x20       if line.len() == 0 { continue }\n\
         \x20       if line.len() > 80 { break }\n\
         \x20       n += 1\n\
         \x20   }\n\
         \x20   return n\n\
         }",
    );
    assert!(rust.contains("continue;"), "{rust}");
    assert!(rust.contains("break;"), "{rust}");
}
