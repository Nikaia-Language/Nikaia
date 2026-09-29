//! **A function declared without parameters may omit the parentheses**
//! (Part I 5.1, *Optional Parentheses*): `fn init { … }` is `fn init() { … }`.
//! The specification's own example was a parse error - *expected `(`* - so a
//! reader who copied it was refused at the first line.
//!
//! Only the declaration may drop them. The call is still `init()`, and a bare
//! `init` is the function as a value, not a call.

mod common;

use std::process::Command;

use nikaia::ast::Item;
use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn errors(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust
}

fn runs(purpose: &str, source: &str, expected: &str) {
    let found = errors(source);
    assert!(found.is_empty(), "{purpose}: {found:#?}");
    let rust = lowered(source);
    let dir = common::scratch_dir(&format!("optional-parentheses-{purpose}"));
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &["--crate-type", "bin", "-o", &binary.to_string_lossy()],
    );
    assert!(
        compiled.status.success(),
        "{purpose} did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary).output().expect("run it");
    assert!(out.status.success(), "{purpose} failed");
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "{purpose}");
    std::fs::remove_dir_all(&dir).ok();
}

/// **The specification's example**, called the way any function is.
#[test]
fn a_function_without_parameters_omits_the_parentheses() {
    runs(
        "init",
        "fn init {\n\
         \x20   println(\"init\")\n\
         }\n\
         \n\
         fn main() { init() }\n",
        "init\n",
    );
}

/// **It is the same declaration as the one with `()`**: no receiver, no
/// arguments, no options.
#[test]
fn the_declaration_is_the_one_with_empty_parentheses() {
    let bare = parse_to_ast("fn init {\n}\n").expect("the bare form parses");
    let spelled = parse_to_ast("fn init() {\n}\n").expect("the spelled form parses");
    for program in [&bare, &spelled] {
        match &program.program.items[0].node {
            Item::Fn {
                name,
                receiver,
                args,
                config,
                spread,
                ..
            } => {
                assert!(name.is_some());
                assert!(receiver.is_none());
                assert!(args.is_empty());
                assert!(config.is_empty());
                assert!(spread.is_none());
            }
            other => panic!("not a function: {other:?}"),
        }
    }
    assert_eq!(
        lowered("fn init {\n}\nfn main() { init() }\n"),
        lowered("fn init() {\n}\nfn main() { init() }\n"),
    );
}

/// **The call keeps its parentheses.** A bare `init` names the function; it
/// does not call it, so nothing is printed.
#[test]
fn the_call_still_needs_its_parentheses() {
    runs(
        "bare-name",
        "fn init {\n\
         \x20   println(\"init\")\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   init\n\
         \x20   println(\"main\")\n\
         }\n",
        "main\n",
    );
}

/// **Every named declaration takes it**: `pub fn`, a method in an `impl`, and
/// a result type and a promise after the name.
#[test]
fn pub_functions_and_methods_take_it_too() {
    runs(
        "everywhere",
        "struct Counter { n: i64 }\n\
         \n\
         impl Counter {\n\
         \x20   pub fn start -> Counter { Counter { n: 0 } }\n\
         }\n\
         \n\
         pub fn hello {\n\
         \x20   println(\"hello\")\n\
         }\n\
         \n\
         fn answer -> i64 sync { 42 }\n\
         \n\
         fn main() {\n\
         \x20   hello()\n\
         \x20   let c = Counter::start()\n\
         \x20   println(f\"{answer()} {c.n}\")\n\
         }\n",
        "hello\n42 0\n",
    );
}

/// **A trait's signature is a declaration too**, so it reads the same way.
#[test]
fn a_trait_signature_takes_it_too() {
    let parsed =
        parse_to_ast("trait Named {\n    fn label -> String\n    fn id(ref self) -> i64\n}\n")
            .expect("the signature parses");
    match &parsed.program.items[0].node {
        Item::Trait { methods, .. } => {
            let label = &methods[0].node;
            assert!(label.receiver.is_none());
            assert!(label.args.is_empty());
            assert!(label.ret_type.is_some());
            assert!(methods[1].node.receiver.is_some());
        }
        other => panic!("not a trait: {other:?}"),
    }
}

/// **A lambda is untouched**, and the anonymous constructor of Part I 4.2
/// keeps its `pub fn(…)`: a nameless `pub fn { … }` would read as a lambda
/// standing where an item belongs.
#[test]
fn lambdas_and_the_constructor_are_untouched() {
    runs(
        "lambda",
        "fn main() {\n\
         \x20   let f: fn() = fn { println(\"lambda\") }\n\
         \x20   f()\n\
         }\n",
        "lambda\n",
    );
    assert!(
        parse_to_ast("struct P { x: i64 }\nimpl P {\n    pub fn {\n    }\n}\n").is_err(),
        "a nameless declaration still needs its parentheses"
    );
}
