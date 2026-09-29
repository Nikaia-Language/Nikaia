//! **A declared type whose every part is a copy derives `Copy`**
//! ([ADR-252](../../../docs/specification/adr/adr-252.md) D4.1): the Rust
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
         struct Flags { on: bool, mark: char, op: Op, at: i64?, pair: (i32, f64) }\n\
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
