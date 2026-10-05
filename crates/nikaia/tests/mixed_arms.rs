//! Two defects found moving the interval propagation into Nikaia (#435), each
//! fixed in the compiler (ADR-294 D3).
//!
//! * **Arms that meet at `T?` because one is a `T?` already**: a plain arm
//!   beside one is `Some(…)`, as it is beside a `null`, and a block that ends
//!   in `null` is a `null` arm. Wrapped as a whole, the choice went below as
//!   `match … { … }.into()` and `rustc` said *`match` arms have incompatible
//!   types*.
//! * **`is_empty` on a map or a set is `std`'s** (#450): without an entry the
//!   call resolved to nothing, and the function around it lowered as
//!   `async`.

mod common;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

fn ran(source: &str) -> String {
    let rust = lowered(source);
    let dir = common::scratch_dir("mixed-arms");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new(&binary).output().expect("runs");
    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8_lossy(&run.stdout).trim().to_string()
}

#[test]
fn a_plain_arm_beside_an_optional_one_is_some() {
    let source = "fn maybe(n: i64) -> i64? {\n\
         \x20   if n > 10 {\n\
         \x20       return null\n\
         \x20   }\n\
         \x20   return n * 2\n\
         }\n\
         \n\
         fn mixed(n: i64) -> i64? {\n\
         \x20   return match n {\n\
         \x20       1 => 5,\n\
         \x20       else => maybe(n),\n\
         \x20   }\n\
         }\n\
         \n\
         fn block_null(n: i64) -> i64? {\n\
         \x20   return match n {\n\
         \x20       1 => 5,\n\
         \x20       2 => {\n\
         \x20           if n > 0 {\n\
         \x20               return maybe(n)\n\
         \x20           }\n\
         \x20           null\n\
         \x20       },\n\
         \x20       else => maybe(n),\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{mixed(1) ?? -1} {mixed(3) ?? -1} {mixed(30) ?? -1} {block_null(1) ?? -1} {block_null(2) ?? -1} {block_null(30) ?? -1}\")\n\
         }\n";
    assert_eq!(ran(source), "5 6 -1 5 4 -1");
}

#[test]
fn is_empty_on_a_map_or_a_set_does_not_pause() {
    let source = "use std::collections\n\
         \n\
         fn none(m: ref collections::BTreeMap[String, i64], s: ref collections::HashSet[i64]) -> bool {\n\
         \x20   return m.is_empty() && s.is_empty()\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let m: collections::BTreeMap[String, i64] = collections::BTreeMap()\n\
         \x20   let s: collections::HashSet[i64] = collections::HashSet()\n\
         \x20   println(f\"{none(m, s)}\")\n\
         }\n";
    let rust = lowered(source);
    assert!(
        rust.contains("\nfn none("),
        "`none` is a plain function:\n{rust}"
    );
    assert_eq!(ran(source), "true");
}
