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

use nikaia::check::Finding;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit;
use nikaia::parser::parse_to_ast;
use std::process::Command;

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = common::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    common::checked(&parsed, &own, &library).findings
}

fn lower(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit::emit_program_reading(&parsed, Default::default(), &common::reads())
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
/// **A method of `std` is computed** ([ADR-321](../../../docs/specification/adr/adr-321.md)
/// D1): its body is Rust, which the interpreter could not run and refused
/// with `NK1127`; compiled, it runs as it does in the program.
#[test]
fn a_comptime_binding_may_call_a_method_of_std() {
    let source = "fn main() { comptime GREET = \"hallo\".to_uppercase() println(f\"{GREET}\") }";
    let found = findings(source);
    assert!(found.is_empty(), "{found:#?}");
    let rust = lower(source);
    assert!(rust.contains("const GREET: &str = \"HALLO\";"), "{rust}");
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

/// **A reader is told which wall they met** (0.0.113): a callee nothing
/// declares is named.
#[test]
fn a_comptime_says_which_wall_it_met() {
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

// --- ADR-287 D21: an integer `comptime` is an open number --------------------

const OPEN: &str = "fn big(n: i64) -> i64 {\n\
     \x20   return n * 1000000000\n\
     }\n\
     \n\
     fn fib(n: i64) -> i64 {\n\
     \x20   if n < 2 {\n\
     \x20       return n\n\
     \x20   }\n\
     \x20   return fib(n - 1) + fib(n - 2)\n\
     }\n\
     \n\
     fn take_i32(x: i32) -> i32 {\n\
     \x20   return x\n\
     }\n\
     \n\
     fn take_i64(x: i64) -> i64 {\n\
     \x20   return x\n\
     }\n\
     \n\
     comptime A = big(1)\n\
     comptime B = big(3)\n\
     comptime FIB_10 = fib(10)\n\
     comptime N = 4 * 1024\n\
     comptime W: i64 = big(1)\n\
     \n\
     fn main() {\n\
     \x20   let y: i64 = FIB_10\n\
     \x20   println(f\"{take_i32(A)} {take_i64(A)} {y} {take_i64(N)} {take_i32(A + 1)}\")\n\
     \x20   let x = A + 1\n\
     \x20   let m = N\n\
     \x20   println(f\"{x} {m} {B} {W}\")\n\
     }\n";

/// **Each use takes the value in the type it asks for**
/// ([ADR-287](../../../docs/specification/adr/adr-287.md) D21): one function
/// hands `A` to an `i32` and to an `i64` parameter, and `FIB_10` to an `i64`
/// `let`, which `rustc` refused while the `const` was an `i32`.
#[test]
fn an_integer_comptime_takes_the_type_each_use_asks_for() {
    assert!(findings(OPEN).is_empty(), "{:#?}", findings(OPEN));
    assert_eq!(
        run("comptime-open", OPEN),
        "1000000000 1000000000 55 4096 1000000001\n1000000001 4096 3000000000 1000000000\n"
    );
}

/// **Where no use asks, the type is the expression's** (D21): a call's result
/// type, never one picked by the value, so `big(1)` and `big(3)` are both
/// `i64`; literals alone take the first type that holds them; a written type
/// pins it everywhere.
#[test]
fn where_no_use_asks_the_type_is_the_expressions() {
    let rust = lower(OPEN);
    assert!(rust.contains("const A: i64 = 1000000000;"), "{rust}");
    assert!(rust.contains("const B: i64 = 3000000000;"), "{rust}");
    assert!(rust.contains("const N: i32 = 4096;"), "{rust}");
    assert!(rust.contains("const W: i64 = 1000000000;"), "{rust}");
    assert!(rust.contains("take_i32(1000000000i32)"), "{rust}");
}

/// **A use that cannot hold the value is `NK1116` at the use**, with the
/// `comptime` that computed it shown beside it (D21).
#[test]
fn a_use_that_cannot_hold_the_value_is_refused_where_it_stands() {
    let source = "fn big(n: i64) -> i64 {\n\
                  \x20   return n * 1000000000\n\
                  }\n\
                  \n\
                  fn take_u8(x: u8) -> u8 {\n\
                  \x20   return x\n\
                  }\n\
                  \n\
                  comptime A = big(1)\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{take_u8(A)}\")\n\
                  }\n";
    let found = findings(source);
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1116")
        .unwrap_or_else(|| panic!("a use too narrow for the value is refused: {found:#?}"));
    assert_eq!(
        refusal.message,
        "This comes to 1000000000, which doesn't fit in a `u8`."
    );
    assert!(
        refusal
            .labels
            .iter()
            .any(|l| !l.main && l.text == "`A` is computed here"),
        "{:#?}",
        refusal.labels
    );
    assert!(
        refusal
            .labels
            .iter()
            .any(|l| l.main && l.text == "taken as a `u8` here"),
        "{:#?}",
        refusal.labels
    );
}
