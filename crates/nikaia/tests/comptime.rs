//! **`comptime`** — Part II 10.2,
//! [ADR-287](../../../docs/specification/adr/adr-287.md).
//!
//! The keyword's whole content is a **demand** rather than an ability (D3). The
//! compiler folded constants before this existed — a `let` bound to `2 * 3` is
//! folded twice on the way through ([ADR-285](../../../docs/specification/adr/adr-285.md))
//! — so what `comptime` adds is that the fold *has* to succeed, and that a program
//! which cannot be folded is refused rather than quietly computed while it runs.
//!
//! That is what these tests are about, in both directions: what reaches the
//! language below is the **folded value**, and what does not fold reaches the
//! reader as `NK1127`.

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

fn run(purpose: &str, source: &str) -> String {
    let dir = common::scratch_dir(purpose);
    let rust = lower(source);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "a `comptime` did not compile:\n{}\n--- emitted ---\n{rust}",
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

const FOUR: &str = "fn main() {\n\
     \x20   comptime LIMIT = 4 * 1024\n\
     \x20   comptime BIG = 3000000000\n\
     \x20   comptime WIDE: i64 = 7\n\
     \x20   comptime ON = true\n\
     \x20   println(f\"{LIMIT} {BIG} {WIDE} {ON}\")\n\
     }";

/// **The arithmetic does not survive into the program**, which is the visible
/// half of D3: a reader of the generated file can see that `4 * 1024` was done
/// while the program was built.
///
/// Note which word is on which side. Nikaia writes `comptime`, because the
/// keyword says *when* rather than *whether it changes*
/// ([ADR-287](../../../docs/specification/adr/adr-287.md)); the language below
/// writes `const`, because that is Rust's word for the same slot.
#[test]
fn what_reaches_the_language_below_is_the_folded_value() {
    let rust = lower(FOUR);
    assert!(rust.contains("const LIMIT: i32 = 4096;"), "{rust}");
    assert!(
        !rust.contains("4 * 1024"),
        "the multiplication survived:\n{rust}"
    );
}

/// The type is written where the program wrote one and inferred where it did
/// not (D4) — and a constant no `i32` holds takes the next type that does,
/// which is [ADR-285](../../../docs/specification/adr/adr-285.md)'s widening
/// reaching a second position rather than a rule of its own.
#[test]
fn the_type_is_written_or_the_first_one_that_holds_it() {
    let rust = lower(FOUR);
    for want in [
        "const BIG: i64 = 3000000000;",
        "const WIDE: i64 = 7;",
        "const ON: bool = true;",
    ] {
        assert!(rust.contains(want), "missing `{want}`:\n{rust}");
    }
}

/// And it runs, which is the part no amount of reading the emitted file
/// replaces.
#[test]
fn a_program_with_comptime_bindings_compiles_and_prints_them() {
    assert_eq!(run("comptime-four", FOUR).trim(), "4096 3000000000 7 true");
}

/// **What does not fold is refused, not computed later** (D3, D5). The way out
/// is in the message, and it is `let`: the program may well want the value
/// computed while it runs, and then it was never a constant.
///
/// **This test used to hold `comptime GREET = "hallo"`**, because text at build
/// time did not exist and a refusal was the whole of what a string literal got.
/// It does exist now ([ADR-311](../../../docs/specification/adr/adr-311.md) D1,
/// 0.0.112), so the example moved to one that still cannot fold.
///
/// **And the reason 0.0.112 gave for it was wrong**, which 0.0.113 corrects
/// twice over. `.to_uppercase()` is not refused because a question about the
/// *value* cannot be answered — it never gets that far, and that question is
/// answered now anyway, because text is **decoded**. What it is refused for is
/// that this evaluator has **no value to call it on**: a method of a `struct`
/// declared here folds since 0.0.114, and everything else is `std`'s or a
/// package's, whose body is Rust.
#[test]
fn a_comptime_binding_this_compiler_cannot_evaluate_is_refused_by_name() {
    let found =
        findings("fn main() { comptime GREET = \"hallo\".to_uppercase() println(f\"{GREET}\") }");
    let refused: Vec<&Finding> = found.iter().filter(|f| f.code == "NK1127").collect();
    assert_eq!(refused.len(), 1, "{found:#?}");
    let said = &refused[0];
    assert!(
        said.message
            .contains("`GREET` can't be computed while the program is built"),
        "{said:#?}"
    );
    assert!(
        said.help
            .as_deref()
            .unwrap_or_default()
            .contains("let GREET"),
        "the way out is named: {said:#?}"
    );
}

/// A constant reached **through another constant** folds, which is what makes
/// the fold's lookup worth having here: `constant_of` asks the scope, and a
/// `comptime` puts its value there the way an immutable `let` does.
#[test]
fn a_comptime_binding_may_be_built_out_of_another() {
    let rust = lower(
        "fn main() {\n\
         \x20   comptime PAGE = 4096\n\
         \x20   comptime PAIR = PAGE * 2\n\
         \x20   println(f\"{PAIR}\")\n\
         }",
    );
    assert!(rust.contains("const PAIR: i32 = 8192;"), "{rust}");
}

/// **No `mut`** (D6): a constant is a value rather than a place, so the grammar
/// has nothing for a second assignment to reach. It does not parse at all,
/// which is the cheapest place to say so.
#[test]
fn a_comptime_binding_cannot_be_mutable() {
    assert!(parse_to_ast("fn main() { comptime mut X = 1 }").is_err());
}

/// And the literal that used to stand in the test above **folds now**, which is
/// the other half of the same change: a `comptime` over text reaches the
/// generated file as the `&str` a `const` can hold.
#[test]
fn a_comptime_binding_over_text_folds() {
    let rust = lower("fn main() { comptime GREET = \"hallo\" println(f\"{GREET}\") }");
    assert!(rust.contains("const GREET: &str = \"hallo\";"), "{rust}");
}

/// **A list of text crosses element for element**
/// ([ADR-311](../../../docs/specification/adr/adr-311.md) D1).
///
/// D1's rule is that a build-time value the program cannot own reaches it as a
/// view: a list crosses as an `Array[T, N]` and text as a `&str`. An array of
/// text is both of those at once and nothing more, so `[&str; N]` follows from
/// the rule rather than extending it — and it is what lets a `comptime` table
/// be walked by a `for` over the keys that built it.
#[test]
fn a_comptime_list_of_text_crosses_as_an_array_of_views() {
    let rust = lower(
        "comptime NAMES: Array[ref String, 3] = [\"get\", \"post\", \"put\"]\n\
         fn main() { for name in NAMES { println(name) } }",
    );
    assert!(
        rust.contains("const NAMES: [&str; 3] = [\"get\", \"post\", \"put\"];"),
        "{rust}"
    );
}

/// **Three walls, and a reader is told which one they met** (0.0.113). The
/// generic catalogue is right for a shape this evaluator does not read and is
/// the wrong answer everywhere else — it invites somebody to go looking for the
/// spelling that works when there is none.
#[test]
fn a_comptime_says_which_wall_it_met() {
    // `std`'s body is Rust, and moving the call does not help.
    let method = findings("comptime X = \"a\".to_uppercase()\nfn main() { println(X) }");
    let said = method.iter().find(|f| f.code == "NK1127").expect("NK1127");
    assert!(
        said.message
            .contains("`X` can't be computed while the program is built"),
        "the headline is the binding's, as the generic one is: {}",
        said.message
    );
    assert!(
        said.notes[0].contains("`.to_uppercase()`") && said.notes[0].contains("which it can't run"),
        "the note names what it met and which wall: {:#?}",
        said.notes
    );
    assert!(
        said.help.as_deref().is_some_and(
            |h| h.contains("calls to functions in this file") && h.contains("`let X = …`")
        ),
        "the wall's way out, and the one every `comptime` has: {:?}",
        said.help
    );
    // **`sync` is the permission and not the ability**: the note says what the
    // compiler *can* call at build time, which ends the confusion without
    // naming `sync` at all.
    assert!(
        said.notes[0].contains("can only call methods your program declares"),
        "{:#?}",
        said.notes
    );

    // And a callee in another file of the same program is the third: the files
    // of a package share one namespace (Part I 9.1) and this walk reads one
    // file, which is a limit of the walk rather than of the language.
    let elsewhere = findings("comptime N = doubled(21)\nfn main() { println(f\"{N}\") }");
    let said = elsewhere
        .iter()
        .find(|f| f.code == "NK1127")
        .expect("NK1127");
    assert!(
        said.notes[0].contains("`doubled`"),
        "it names the callee: {:#?}",
        said.notes
    );
}
