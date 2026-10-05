//! **A binding in scope hides an item of its name** (#451): a call through a
//! parameter of function type is the parameter, not a free function that
//! happens to share the name.

mod common;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

const SOURCE: &str = "fn twice(a: i64, b: i64, c: i64) -> i64 {\n\
     \x20   return a + b + c\n\
     }\n\
     \n\
     fn apply(twice: fn(i64) -> i64 sync) -> i64 {\n\
     \x20   return twice(4)\n\
     }\n\
     \n\
     fn main() {\n\
     \x20   println(f\"{apply(fn(n) { n * 2 })} {twice(1, 2, 3)}\")\n\
     }\n";

#[test]
fn a_parameter_hides_a_function_of_its_name() {
    let parsed = parse_to_ast(SOURCE).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let found =
        nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
            .findings;
    assert!(found.is_empty(), "{found:#?}");
    let rust = emit_program(&parsed, Build::default())
        .expect("lowers")
        .rust;
    let dir = common::scratch_dir("shadowed-callee");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "{}\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary).output().expect("runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "8 6");
}
