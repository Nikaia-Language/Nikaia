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
//!
//! What the programs compute is `tests/language/src/mixed_arms.nika`; here
//! stays how `is_empty` lowers.

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
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
}
