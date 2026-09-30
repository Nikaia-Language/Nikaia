//! **The two bounds that ask what a type *is*** — Part II 10.3,
//! [ADR-088](../../../docs/specification/adr/adr-088.md) D2 and D3.
//!
//! `[T: Struct]` is not a trait anybody declares and no `impl` answers it. What
//! answers it is the **declaration**, which is the whole of D2: the bound is
//! what makes a type's shape reachable, and it is reachable because the
//! compiler has read the `struct` line.
//!
//! **What D3 buys is one whole class of failure moved to the call.** Zig's
//! `anytype` is the counter-example the record cites: the signature says
//! nothing, so a caller learns that its type does not fit only when something
//! deep in the body fails to compile, with the message pointing into somebody
//! else's code. `NK1164` answers *you passed something that is not a struct*
//! where the call is.
//!
//! **And the bound does not travel.** Rust has no trait called `Struct`, so one
//! written into the generated file would come back as *cannot find trait
//! `Struct` in this scope* — [Part III
//! C.1](../../../docs/specification/30-nikaia-tooling.md)'s class, and the hole
//! this package would have opened if the emitter had not been told.

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

fn one(source: &str) -> Finding {
    let mut found = findings(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    found.remove(0)
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
        "a shape bound did not compile:\n{}\n--- emitted ---\n{rust}",
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

const SHAPES: &str = "struct Point { x: i64, y: i64 }\n\
     enum Op { Add, Sub }\n\
     \n\
     fn describe[T: Struct](value: T) -> ref String {\n\
     \x20   return \"a struct\"\n\
     }\n\
     \n\
     fn name_it[T: Enum](value: T) -> ref String {\n\
     \x20   return \"an enum\"\n\
     }\n";

/// **The bound is legal and the program runs**, which is the whole of D2 for
/// this package: `NK1135` used to refuse `Struct` as a trait nothing declares,
/// which was true and not the answer.
#[test]
fn a_shape_bound_is_a_bound_and_the_program_runs() {
    let source = format!(
        "{SHAPES}\n\
         fn main() {{\n\
         \x20   println(describe(Point {{ x: 1, y: 2 }}))\n\
         \x20   println(name_it(Op::Add))\n\
         }}"
    );
    assert!(findings(&source).is_empty(), "{:#?}", findings(&source));
    assert_eq!(run("shape-bound", &source), "a struct\nan enum\n");
}

/// **The bound does not travel into the generated file**, because the language
/// below has no trait by either name. This is the assertion that stops
/// [Part III C.1](../../../docs/specification/30-nikaia-tooling.md) from
/// re-opening the moment somebody writes one.
#[test]
fn a_shape_bound_does_not_reach_the_language_below() {
    let rust = lower(&format!(
        "{SHAPES}\nfn main() {{ println(describe(Point {{ x: 1, y: 2 }})) }}"
    ));
    assert!(rust.contains("fn describe<T>("), "{rust}");
    assert!(!rust.contains("Struct"), "{rust}");
    assert!(!rust.contains(": Enum"), "{rust}");
}

/// **The other shape is refused at the call** (D3), and the way out is a
/// declaration rather than an `impl`: `impl Struct for Op` is not something
/// anybody can write, so offering it would be a way out that cannot be taken
/// ([Part III C.2](../../../docs/specification/30-nikaia-tooling.md)).
#[test]
fn an_enum_does_not_answer_for_a_struct_bound() {
    let found = one(&format!(
        "{SHAPES}\n\
         fn main() {{\n\
         \x20   println(describe(Op::Add))\n\
         }}"
    ));
    assert_eq!(found.code, "NK1164");
    assert_eq!(
        found.message,
        "`describe` needs a `Struct` here, but `Op` isn't one."
    );
    assert!(
        found.notes[0].contains("asks what a type is, not what it does")
            && found.notes[0].contains("`Op` is an `enum`"),
        "{:#?}",
        found.notes
    );
    assert!(
        found
            .help
            .as_deref()
            .is_some_and(|help| help.contains("`struct`") && !help.contains("impl")),
        "the way out is a declaration and never an `impl`: {:?}",
        found.help
    );
}

/// **And a type Part I 2.2 offers is refused too**, which is the half of D3
/// that catches the ordinary mistake. Nothing declares an `i64` either shape,
/// and this compiler has read that page.
#[test]
fn a_type_part_one_offers_does_not_answer_for_a_shape_bound() {
    let source = format!(
        "{SHAPES}\n\
         fn main() {{\n\
         \x20   let n: i64 = 7\n\
         \x20   println(describe(n))\n\
         }}"
    );
    let found = one(&source);
    assert_eq!(found.code, "NK1164");
    assert_eq!(
        found.message,
        "`describe` needs a `Struct` here, but `i64` isn't one."
    );
}

/// **A struct does not answer for `Enum`** either, which is the same rule read
/// the other way and the guard that says the check is about the declaration.
#[test]
fn a_struct_does_not_answer_for_an_enum_bound() {
    let found = one(&format!(
        "{SHAPES}\n\
         fn main() {{\n\
         \x20   println(name_it(Point {{ x: 1, y: 2 }}))\n\
         }}"
    ));
    assert_eq!(found.code, "NK1164");
    assert_eq!(
        found.message,
        "`name_it` needs an `Enum` here, but `Point` isn't one."
    );
}

/// **It fails open where this compiler has read no declaration**, which is
/// [Part III C.4](../../../docs/specification/30-nikaia-tooling.md)'s half of
/// the same page: a `std` type, a foreign one and a name no ledger classifies
/// all look alike from here, and a correct program refused is the worse
/// mistake. What is left refused is what the compiler *has* read.
#[test]
fn a_type_this_compiler_has_not_classified_is_not_refused() {
    let source = format!(
        "{SHAPES}\n\
         fn main() {{\n\
         \x20   let xs = [1, 2, 3]\n\
         \x20   println(describe(xs))\n\
         }}"
    );
    assert!(
        findings(&source).iter().all(|found| found.code != "NK1164"),
        "{:#?}",
        findings(&source)
    );
}

/// **A program that declares the name wins.** `Struct` is a language word here
/// the way `sync` is, and a `trait Struct { … }` beside it is an ordinary trait
/// answered by an ordinary `impl` — so nothing takes the name away from a
/// program that wanted it.
#[test]
fn a_program_that_declares_the_trait_keeps_it() {
    let source = "trait Struct {\n\
         \x20   fn label(self) -> ref String\n\
         }\n\
         \n\
         struct Point { x: i64, y: i64 }\n\
         \n\
         impl Struct for Point {\n\
         \x20   fn label(self) -> ref String { return \"point\" }\n\
         }\n\
         \n\
         fn describe[T: Struct](value: T) -> ref String {\n\
         \x20   return value.label()\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(describe(Point { x: 1, y: 2 }))\n\
         }";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    // And the bound **does** travel now, because there is a trait to name and
    // an `impl` that answers it — which is the half the emitter has to get
    // right, since dropping it here would be `rustc`'s *no method named
    // `label`* about the generated file.
    assert!(
        lower(source).contains("fn describe<T: Struct>("),
        "{}",
        lower(source)
    );
}

/// **And what the bound makes reachable is built since 0.0.129**
/// ([ADR-181](../../../docs/specification/adr/adr-181.md)), which is what this
/// test used to assert the absence of.
///
/// The hole it was written for is still closed and is now closed the other
/// way: before `[T: Struct]` was a legal bound, `T::fields` was refused one
/// line up with `NK1135`; making the bound legal turned that into **silence**,
/// and `NK1171` stood in for the feature until the feature arrived.
/// `tests/reflection.rs` is where it runs.
#[test]
fn the_shape_the_bound_reaches_is_built() {
    let source = "struct Point { x: i64, y: i64 }\n\
         \n\
         fn describe[T: Struct](value: T) -> ref String {\n\
         \x20   for field in T::fields {\n\
         \x20       println(field.name)\n\
         \x20   }\n\
         \x20   return \"done\"\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(describe(Point { x: 1, y: 2 }))\n\
         }";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lower(source);
    assert!(rust.contains("fn describe__Point("), "{rust}");
    assert!(!rust.contains("T::fields"), "{rust}");
}

/// **Each shape member is reached under its own bound**: `T::variants` under
/// `[T: Enum]`, so written under `[T: Struct]` it is `NK1171` naming `Enum`.
#[test]
fn the_other_shape_is_refused_by_name() {
    let found = one("struct Point { x: i64 }\n\
         \n\
         fn tell[T: Struct](value: T) -> ref String {\n\
         \x20   for v in T::variants {\n\
         \x20       println(\"x\")\n\
         \x20   }\n\
         \x20   return \"done\"\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(tell(Point { x: 1 }))\n\
         }");
    assert_eq!(found.code, "NK1171");
    assert_eq!(
        found.message,
        "You can't reach `T::variants` directly: it's reached through a type parameter bound by `Enum`."
    );
}

/// **And without the bound it says the bound is what reaches it** (D2), which
/// is the way out the reader takes: the step after it is built now, so the
/// note names it rather than warning them off.
#[test]
fn the_shape_without_a_bound_names_the_bound() {
    let found = one("fn plain[T](value: T) -> ref String {\n\
         \x20   for field in T::fields {\n\
         \x20       println(field.name)\n\
         \x20   }\n\
         \x20   return \"done\"\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(plain(7))\n\
         }");
    assert_eq!(found.code, "NK1171");
    assert!(
        found.notes[0].contains("`[T: Struct]`"),
        "{:#?}",
        found.notes
    );
}
