//! A span of time, and the call that waits one out
//! ([ADR-150](../../../docs/specification/adr/adr-150.md)).
//!
//! Part II 12.4 wrote `time::sleep(5.seconds())` and
//! [ADR-141](../../../docs/specification/adr/adr-141.md) D2 marked it
//! *unspecified*: the type was not in dispute, only where it lived and how it
//! was spelled. D1 answers `std::time::Duration`, D2 answers *a method on an
//! integer*, D3 answers *and never a suffix on a literal*, and D4 is what the
//! **language** learns from all of it: nothing at all.
//!
//! So what these tests hold is a `std` surface and a runtime promise — five
//! names on either integer type, and a `sleep` that gives the thread up rather
//! than holding it.
//!
//! What a span reads back as and what a wait lets run are
//! `tests/language/src/duration.nika`; this file keeps what is refused, what is
//! lowered, and the waits that are timed.

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

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Build the Rust, run it, and hand back what it printed.
fn output(purpose: &str, source: &str) -> (String, std::time::Duration) {
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
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
    let started = std::time::Instant::now();
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    let took = started.elapsed();
    assert!(
        ran.status.success(),
        "the program runs:\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
    (
        String::from_utf8_lossy(&ran.stdout).trim().to_string(),
        took,
    )
}

/// **D2: five names, and they are methods on the integer** — which is the
/// spelling Part II 12.4 already wrote, chosen by whoever wrote the sentence
/// rather than by this record.
///
/// The number is written with the type it takes (#417): `5.seconds()` has an
/// `i32` and an `i64` method below, so the number alone was ambiguous there.
#[test]
fn the_five_names_are_methods_on_an_integer() {
    for name in ["seconds", "millis", "micros", "minutes", "hours"] {
        let source = format!(
            "use std::time\n\
         \n\
         fn main() {{\n\
             \x20   let span = 5.{name}()\n\
             \x20   time::sleep(span)\n\
             }}\n"
        );
        assert!(
            findings(&source).is_empty(),
            "{name}: {:#?}",
            findings(&source)
        );
        assert!(
            lowered(&source).contains(&format!("let span = 5i32.{name}();")),
            "{name}\n{}",
            lowered(&source)
        );
    }
}

/// **D3: there is no suffix literal.** `5s` is not a number followed by a name
/// this language knows — it is a parse error, and it stays one.
///
/// The check is deliberately weak about *which* error: what the record decided
/// is that the spelling does not exist, not what the parser says about it.
#[test]
fn a_suffix_is_not_a_duration() {
    assert!(
        parse_to_ast("use std::time\n\nfn main() { time::sleep(5s) }\n").is_err(),
        "`5s` is not a spelling this language has (ADR-150 D3)"
    );
}

/// **A span is handed over whole**, and the `&` that ADR-094 D1 puts in front
/// of a value that moves is not there.
///
/// This is the `usize` defect one type over
/// ([ADR-302](../../../docs/specification/adr/adr-302.md) D6): a type the copy
/// list does not name is one that **moves**, and the first program to write
/// `time::sleep(50.millis())` was the first to meet it.
#[test]
fn a_span_is_not_lent() {
    let rust = lowered("use std::time\n\nfn main() { time::sleep(50.millis()) }\n");
    assert!(
        rust.contains("time::sleep(50i32.millis())"),
        "a duration copies, so nothing lends it\n{rust}"
    );
    assert!(!rust.contains("time::sleep(&"), "{rust}");
}

/// **`sleep` is a suspension point**, which is the missing `sync` line in the
/// ledger and the `.await` here.
///
/// It is also what decided the entry's **key**. A bare call takes its `.await`
/// from an exact lookup in `std`'s ledger, so a function a program writes bare
/// is keyed bare — as `print` and `println` already are. The alternative was to
/// resolve a bare name by its last segment, and that is the emitter guessing:
/// `examples/inventory` writes its own `read`, and a unit built by `nikaia lower`
/// does not carry the ledger of the package beside it.
#[test]
fn a_sleep_carries_an_await() {
    let rust = lowered("use std::time\n\nfn main() { time::sleep(1.millis()) }\n");
    assert!(rust.contains("time::sleep(1i32.millis()).await;"), "{rust}");
    assert!(rust.contains("async fn __nikaia_main()"), "{rust}");
}

/// **A program's trait method on a `std` type is awaited** (#376,
/// ADR-295 D14): the call resolves to the program's own entry, which may
/// pause, and not to `Duration`'s ledger entry, which has no `label`.
#[test]
fn a_program_trait_method_on_a_duration_is_awaited() {
    let rust = lowered(
        "use std::time\n\
         \n\
         trait Label { fn label(self) -> String }\n\
         \n\
         impl Label for time::Duration {\n\
         \x20   fn label(self) -> String { return \"span\" }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(5.seconds().label())\n\
         \x20   let span = 3.seconds()\n\
         \x20   println(span.label())\n\
         }\n",
    );
    assert!(rust.contains("5i32.seconds().label().await"), "{rust}");
    assert!(rust.contains("span.label().await"), "{rust}");
}

/// **Either integer type**, which is why the ledger has ten entries and not
/// five: `5` is an `i32` where nothing asks otherwise, and a count that came
/// from a length is an `i64` ([ADR-285](../../../docs/specification/adr/adr-285.md)
/// D1).
#[test]
fn a_count_may_be_either_integer() {
    let source = "use std::time\n\
         \n\
         fn main() {\n\
                  \x20   let wide: i64 = 2\n\
                  \x20   time::sleep(wide.millis())\n\
                  \x20   time::sleep(3.millis())\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
}

/// **Part II 12.4's line, as a program that runs** (§5 step 4) — shortened,
/// because a test that waits five seconds is a test people learn to skip.
#[test]
fn a_sleeping_program_compiles_and_runs() {
    let (printed, took) = output(
        "duration-sleeps",
        "use std::time\n\
         \n\
         fn main() {\n\
         \x20   time::sleep(120.millis())\n\
         \x20   println(\"awake\")\n\
         }\n",
    );
    assert_eq!(printed, "awake");
    assert!(
        took >= std::time::Duration::from_millis(100),
        "the program waited: {took:?}"
    );
}

/// **A count below zero is no time at all**, rather than an abort on a
/// subtraction that came out the wrong way: a duration is unsigned below, and
/// what a program asking to wait wants is to carry on.
#[test]
fn a_span_below_zero_is_no_wait() {
    let (printed, took) = output(
        "duration-negative",
        "use std::time\n\
         \n\
         fn main() {\n\
         \x20   let back: i64 = 0 - 5\n\
         \x20   time::sleep(back.seconds())\n\
         \x20   println(\"through\")\n\
         }\n",
    );
    assert_eq!(printed, "through");
    assert!(took < std::time::Duration::from_secs(2), "{took:?}");
}

/// **D4: the language learns nothing.** `seconds` is not a keyword, and a
/// program may still use the word for its own.
#[test]
fn nothing_here_is_a_keyword() {
    let source = "use std::time\n\
         \n\
         fn seconds(of: i64) -> i64 { return of * 2 }\n\
                  \n\
                  fn main() {\n\
                  \x20   let doubled = seconds(3)\n\
                  \x20   println(f\"{doubled}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains(
            "use std::time\n\
         \n\
         fn seconds(of: i64) -> i64"
        ),
        "{}",
        lowered(source)
    );
}

/// **The answer's type is the ledger's**: the entries are what the checker
/// reads, so an `i64` put where a `String` goes is refused in this language's
/// words rather than in `rustc`'s.
#[test]
fn a_count_read_back_is_an_i64() {
    let source = "use std::time\n\
         \n\
         fn main() {\n\
         \x20   let wrong: String = 5.seconds().in_seconds()\n\
         \x20   println(wrong)\n\
         }\n";
    assert!(!findings(source).is_empty(), "the ledger types the answer");
}
