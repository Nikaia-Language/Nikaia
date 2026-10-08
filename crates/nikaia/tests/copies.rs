//! **A declared type whose every part is a copy derives `Copy`**
//! ([ADR-294](../../../docs/specification/adr/adr-294.md) D9.1): the Rust
//! below may copy it, and nothing the language says about the value changes.

mod common;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("parses");
    emit_program(&parsed, Build::default())
        .expect("lowers")
        .rust
}

/// The derive line written for the type named `name`.
fn derive_of(rust: &str, name: &str) -> String {
    let lines: Vec<&str> = rust.lines().collect();
    let at = lines
        .iter()
        .position(|line| {
            line.starts_with(&format!("pub struct {name}"))
                || line.starts_with(&format!("pub enum {name}"))
                || line.starts_with(&format!("struct {name}"))
                || line.starts_with(&format!("enum {name}"))
        })
        .unwrap_or_else(|| panic!("`{name}` is declared:\n{rust}"));
    lines[..at]
        .iter()
        .rev()
        .find(|line| line.starts_with("#[derive("))
        .unwrap_or_else(|| panic!("`{name}` has a derive:\n{rust}"))
        .to_string()
}

#[test]
fn a_unit_only_enum_and_a_struct_of_numbers_are_copies() {
    let rust = lowered(
        "enum Op { Add, Sub }\n\
         struct Span { start: u32, end: u32 }\n\
         struct Flags { on: bool, mark: scalar, op: Op, at: i64?, pair: (i32, f64) }\n\
         fn main() { print(\"x\") }\n",
    );
    for name in ["Op", "Span", "Flags"] {
        assert!(derive_of(&rust, name).contains("Copy"), "{name}:\n{rust}");
    }
    // **And `rustc` agrees**: a `Copy` derive on a type with a part that is
    // not one is refused, so the lowering compiling is the proof.
    let dir = common::scratch_dir("copies");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let out = common::compile(
        &file,
        &["-o", dir.join("program").to_str().expect("utf-8 path")],
    );
    assert!(
        out.status.success(),
        "{}\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn text_a_list_a_type_variable_or_a_ring_of_types_is_not() {
    let rust = lowered(
        "struct Named { name: String }\n\
         struct Many { xs: Vec[i64] }\n\
         struct Holder[T] { value: T }\n\
         enum Chain { End, Link(i64, Chain) }\n\
         struct Wrap { inner: Named }\n\
         fn main() { print(\"x\") }\n",
    );
    for name in ["Named", "Many", "Holder", "Chain", "Wrap"] {
        assert!(!derive_of(&rust, name).contains("Copy"), "{name}:\n{rust}");
    }
}

/// A copy found further down the file still makes the type above it one: the
/// answer is asked again until nothing more joins.
#[test]
fn a_copy_declared_below_its_holder_is_found() {
    let rust = lowered(
        "struct Outer { inner: Inner }\n\
         struct Inner { n: i64 }\n\
         fn main() { print(\"x\") }\n",
    );
    assert!(derive_of(&rust, "Outer").contains("Copy"), "{rust}");
}

/// **A generic struct is seen through** (ADR-294): `Spanned[Pattern]`
/// holds a `Pattern` inline, so a `Pattern` holding one is on a ring and the
/// member is boxed; and `Spanned[Stmt]` compares only if `Stmt` does, so a
/// `Block` of them derives no `PartialEq` a `Stmt` without one would refuse.
/// Both reached `rustc` before, found by lowering the compiler's own tree.
#[test]
fn a_generic_struct_is_seen_through_for_rings_and_comparison() {
    let rust = lowered(
        "struct Spanned[T] { node: T, at: u32 }\n\
         enum Pattern { Cut, Group(Spanned[Pattern]), Seq(Vec[Spanned[Pattern]]) }\n\
         enum Stmt { Say(String), Nested(Vec[Stmt]), Code(fn(i64) -> i64) }\n\
         struct Block { stmts: Vec[Spanned[Stmt]] }\n\
         fn main() { print(\"x\") }\n",
    );
    assert!(rust.contains("Group(Box<Spanned<Pattern>>)"), "{rust}");
    assert!(rust.contains("Seq(Vec<Spanned<Pattern>>)"), "{rust}");
    assert!(!derive_of(&rust, "Block").contains("PartialEq"), "{rust}");
    let dir = common::scratch_dir("generic-seen-through");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let out = common::compile(
        &file,
        &["-o", dir.join("program").to_str().expect("utf-8 path")],
    );
    assert!(
        out.status.success(),
        "{}\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// **A type with a cleanup or a `Drop` is never a copy**: a copied value would
/// run it twice, and `rustc` refuses the pair (E0184). Part I's own
/// `FileHandle { fd: i32 }` found it, through the specification's corpus.
#[test]
fn a_type_that_is_dropped_is_not_a_copy() {
    let rust = lowered(
        "struct FileHandle { fd: i32 }\n\
         impl Drop for FileHandle {\n\
         \x20   fn drop(ref mut self) { println(\"closing\") }\n\
         }\n\
         fn main() { print(\"x\") }\n",
    );
    assert!(!derive_of(&rust, "FileHandle").contains("Copy"), "{rust}");
}

/// **A described Rust type that says it copies is a part that copies**
/// ([ADR-294](../../../docs/specification/adr/adr-294.md) D9.3), and one that
/// says nothing is not: the line is a reviewed claim, and its absence is *no*.
#[test]
fn a_described_type_that_copies_is_a_part_that_copies() {
    use nikaia::check::check_program;
    use nikaia::contracts::{Ledger, LedgerOps};

    let library = |copies: &str| {
        Ledger::parse(&format!(
            "version = 2\n\
             toolchain = \"probe\"\n\
             inference = \"probe\"\n\
             \n\
             [type.\"fremd::Pair\"]\n\
             pub = true\n\
             {copies}"
        ))
        .expect("the test's ledger parses")
    };
    let parsed = parse_to_ast(
        "struct Held { pair: fremd::Pair, n: i64 }\n\
         fn main() { print(\"x\") }\n",
    )
    .expect("parses");
    let copied = |ledger: &Ledger| {
        check_program(&parsed, &Ledger::empty(), ledger, &Default::default())
            .copies
            .contains("Held")
    };
    assert!(copied(&library("copies = true\n")));
    assert!(!copied(&library("")));
}
