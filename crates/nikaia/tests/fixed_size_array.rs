//! `Array[T, N]` — a container that does not allocate
//! ([ADR-152](../../../docs/specification/adr/adr-152.md)).
//!
//! Two records pointed here and neither had its own answer.
//! [ADR-127](../../../docs/specification/adr/adr-127.md) §4 left a C value
//! struct's `[f64; 3]` field undecided, and
//! [ADR-135](../../../docs/specification/adr/adr-135.md) §4 left *a container
//! that does not allocate*, which the bare-metal profile of
//! [ADR-119](../../../docs/specification/adr/adr-119.md) will ask for on its
//! first day: a `Vec` needs an allocator and that profile has none.
//!
//! **What was new is the integer argument.** Every type parameter this language
//! had was a type, and `N` is a number — so the type language gained
//! [`Ty::Count`] rather than `Array` gaining a shape of its own, which is what
//! keeps every reader of a type that does not care about arrays unchanged.

mod common;

use nikaia::contracts::ty::Ty;
use nikaia::contracts::ty::TyOps;
use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust
}

/// **D1: `Array[T, N]` in the bracket generic the type language already has**,
/// and D2's lowering — `N` elements inline, nothing allocated.
#[test]
fn an_array_is_n_elements_inline() {
    let source = "fn main() {\n\
                  \x20   let origin: Array[f64, 3] = [0.0, 1.5, 2.5]\n\
                  \x20   println(f\"{origin[1]}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("let origin: [f64; 3] = [0.0, 1.5, 2.5];"),
        "{}",
        lowered(source)
    );
}

/// **The literal is `[…]` and not `vec![…]` where the use asks for an array**
/// (D4), and the *same* literal is a `Vec` where nothing does — which is the
/// whole of that ruling, measured in one file.
#[test]
fn the_same_literal_is_a_vec_or_an_array_by_its_use() {
    let source = "fn main() {\n\
                  \x20   let list = [1, 2, 3]\n\
                  \x20   let array: Array[i64, 3] = [1, 2, 3]\n\
                  \x20   println(f\"{list.len()} {array.len()}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("let list = vec![1, 2, 3];"), "{rust}");
    assert!(rust.contains("let array: [i64; 3] = [1, 2, 3];"), "{rust}");
}

/// **A literal whose length does not match `N` is refused naming both
/// numbers** (D4), because the length is part of the type and what the reader
/// has to do about it is count.
#[test]
fn a_literal_of_the_wrong_length_is_refused() {
    let found: Vec<_> = findings(
        "fn main() {\n\
         \x20   let a: Array[i64, 3] = [1, 2]\n\
         \x20   println(f\"{a[0]}\")\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1157")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("2 elements"), "{found:#?}");
    assert!(found[0].message.contains("3"), "{found:#?}");
}

/// **And the length is part of the type**, so two arrays of different lengths
/// are different types with no rule of their own: the ordinary fit says it.
#[test]
fn two_lengths_are_two_types() {
    assert!(!Ty::parse("Array[i64, 3]").fits(&Ty::parse("Array[i64, 4]")));
    assert!(Ty::parse("Array[i64, 3]").fits(&Ty::parse("Array[i64, 3]")));
}

/// **The integer argument writes itself back** (§5 step 1): a count renders as
/// the digits the source wrote and reads back as the same count, which is what
/// lets a signature holding one ship in the ledger.
#[test]
fn a_count_round_trips_through_its_text() {
    let ty = Ty::parse("Array[f64, 3]");
    assert_eq!(ty.text(), "Array[f64, 3]");
    let Ty::Named { args, .. } = &ty else {
        panic!("{ty:?}")
    };
    assert_eq!(args.as_slice(), [Ty::parse("f64"), Ty::Count(3)]);
    assert_eq!(Ty::parse(&ty.text()), ty);
}

/// **And a signature carrying one reaches the ledger**, which is the half a
/// consumer reads.
#[test]
fn a_signature_with_an_array_is_recorded() {
    let source = "pub fn sum(xs: Array[i64, 3]) -> i64 {\n\
                  \x20   return xs[0] + xs[1] + xs[2]\n\
                  }\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let written = Ledger::infer(&parsed).render();
    assert!(
        written.contains("signature = \"(xs: Array[i64, 3]) -> i64\""),
        "{written}"
    );
}

/// **`3` is not a type nothing declares** — `NK1135` walks every argument of a
/// written type, and a count is one of them.
#[test]
fn a_count_is_not_an_undeclared_type() {
    let source = "fn main() {\n\
                  \x20   let a: Array[i64, 2] = [1, 2]\n\
                  \x20   println(f\"{a[0]}\")\n\
                  }\n";
    assert!(
        findings(source).iter().all(|f| f.code != "NK1135"),
        "{:#?}",
        findings(source)
    );
}

/// **D2: it copies as its elements do**, which is what decides whether the
/// compiler writes the `&` at a call ([ADR-094](../../../docs/specification/adr/adr-094.md)
/// D1). An `Array[i64, 3]` is copied the way a tuple of three `i64` is.
#[test]
fn an_array_of_numbers_is_passed_by_value() {
    let source = "fn sum(xs: Array[i64, 3]) -> i64 {\n\
                  \x20   return xs[0] + xs[1] + xs[2]\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{sum([1, 2, 3])}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("fn sum(xs: [i64; 3]) -> i64"), "{rust}");
    assert!(rust.contains("sum([1, 2, 3])"), "{rust}");
}

/// **A struct's field**, which is [ADR-127](../../../docs/specification/adr/adr-127.md)
/// §4's position — the one that pointed here.
#[test]
fn a_struct_field_holds_one() {
    let source = "struct Vector3 { parts: Array[f64, 3] }\n\
                  \n\
                  fn main() {\n\
                  \x20   let v = Vector3 { parts: [1.0, 2.0, 3.0] }\n\
                  \x20   println(f\"{v.parts[2]}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("parts: [f64; 3],"), "{rust}");
    assert!(
        rust.contains("Vector3 { parts: [1.0, 2.0, 3.0] }"),
        "{rust}"
    );
}

/// **A declared result is a use too** (D4).
#[test]
fn a_return_takes_the_declared_array() {
    let source = "fn zeros() -> Array[i64, 2] {\n\
                  \x20   return [0, 0]\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{zeros()[1]}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("-> [i64; 2] { [0, 0] }"),
        "{}",
        lowered(source)
    );
}

/// **An element that is not what the array holds is the position's own
/// message**, under the code that position always used — one refusal, and no
/// code of its own for a mismatch that is not new.
#[test]
fn an_element_of_the_wrong_type_is_the_lets_own_refusal() {
    let found: Vec<_> = findings(
        "fn main() {\n\
         \x20   let a: i64 = 1\n\
         \x20   let b: Array[ref String, 1] = [a]\n\
         \x20   println(f\"{b[0]}\")\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1103")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("Array[i64, 1]"), "{found:#?}");
    assert!(
        found[0].message.contains("Array[ref String, 1]"),
        "{found:#?}"
    );
}

/// **An array inside another type takes its shape too**, which is the position
/// D4's rule did not reach when it was first built: four positions write a use
/// and each read the type it was given whole, so `Vec[Array[f64, 2]]` refused
/// the literal that is right for it — a correct program refused, which is
/// [Part III C.4](../../../docs/specification/30-nikaia-tooling.md)'s class.
#[test]
fn a_vec_of_arrays_takes_its_shape_one_level_down() {
    let source = "fn main() {\n\
                  \x20   let grid: Vec[Array[f64, 2]] = [[1.0, 2.0], [3.0, 4.0]]\n\
                  \x20   println(f\"{grid[0][1]}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("let grid: Vec<[f64; 2]> = vec![[1.0, 2.0], [3.0, 4.0]];"),
        "{}",
        lowered(source)
    );
}

/// **And an array of arrays**, where the walk through is the outer array's own.
#[test]
fn an_array_of_arrays_lowers_both_levels() {
    let source = "fn main() {\n\
                  \x20   let deep: Array[Array[i64, 2], 2] = [[1, 2], [3, 4]]\n\
                  \x20   println(f\"{deep[1][0]}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("let deep: [[i64; 2]; 2] = [[1, 2], [3, 4]];"),
        "{}",
        lowered(source)
    );
}

/// **Every element is walked and not only the first**, because each one's
/// length is its own refusal — the short one is the second here.
#[test]
fn an_inner_literal_of_the_wrong_length_is_refused_wherever_it_stands() {
    let found: Vec<_> = findings(
        "fn main() {\n\
         \x20   let bad: Vec[Array[i64, 2]] = [[1, 2], [3]]\n\
         \x20   println(f\"{bad.len()}\")\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1157")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("1 element,"), "{found:#?}");
}

/// **A list of anything else is untouched by the walk through**: a `Vec[T]` is
/// opened only where an array is what it holds.
#[test]
fn a_plain_list_is_still_a_vec() {
    let source = "fn main() {\n\
                  \x20   let plain: Vec[i64] = [1, 2, 3]\n\
                  \x20   println(f\"{plain.len()}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert!(
        lowered(source).contains("let plain: Vec<i64> = vec![1, 2, 3];"),
        "{}",
        lowered(source)
    );
}

/// **D3, measured where it matters: it compiles, it runs, and `len()` is the
/// `N` it was declared with** — known while the program is built, because
/// `[T; N]::len` is a constant below.
#[test]
fn an_array_compiles_and_runs() {
    let rust = lowered(
        r#"
struct Vector3 { parts: Array[f64, 3] }

fn scaled(v: Vector3, by: f64) -> Array[f64, 3] {
    return [v.parts[0] * by, v.parts[1] * by, v.parts[2] * by]
}

fn main() {
    let v = Vector3 { parts: [1.0, 2.0, 3.0] }
    let out = scaled(v, 2.0)
    println(f"{out[0]} {out[1]} {out[2]} {out.len()}")
}
"#,
    );
    let dir = common::scratch_dir("fixed-size-array");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "2 4 6 3",
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
