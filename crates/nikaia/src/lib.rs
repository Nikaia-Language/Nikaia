// crates/nikaia/src/lib.rs
//
// The compiler front-end as a library, so integration tests (and later other
// tools) can drive it. The binary in `main.rs` is the CLI on top of this.
//
// **The parser's combinators are instantiated deeper than rustc's default
// limit of 128 allows** since the ledger inference and the bounds walk read
// conditions back through `parse_to_ast` (ADR-314 D3).
#![recursion_limit = "256"]

pub mod assets;
pub mod ast;
pub mod bounds;
pub mod bounds_report;
pub mod build_time;
pub mod check;
pub mod comptime_run;
pub mod contracts;
pub mod describe;
pub mod diagnostics;
pub mod dsl;
pub mod emit;
pub mod fixed;
pub mod fold;
pub mod foreign;
pub mod grammar_run;
pub mod interpreter;
pub mod libraries;
pub mod manifest;
pub mod modules;
pub mod parser;
pub mod project;
pub mod proofs;
pub mod prove;
pub mod specbook;
pub mod sysroot;
pub mod text_tiers;
pub mod traits;
pub mod types;
pub mod views;
