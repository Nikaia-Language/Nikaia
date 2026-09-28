//! **Four programs that reached `rustc` as a file nobody wrote** (0.0.244),
//! each found by running something and kept as the program that found it
//! (`open-work.md` §1.23, §1.24, §1.25, and a bare call nothing declares).
//! Every one is run, at both settings of `user_parallelism`.

mod common;

use std::process::Command;

use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

fn runs(purpose: &str, source: &str, expected: &str) {
    let found = findings(source);
    assert!(found.is_empty(), "{purpose}: {found:#?}");
    for how in [Build::default(), Build::parallel()] {
        let parsed = parse_to_ast(source).expect("the source parses");
        let rust = emit_program(&parsed, how).expect("it lowers").rust;
        let dir = common::scratch_dir(&format!("found-running-{purpose}"));
        let path = dir.join("program.rs");
        std::fs::write(&path, &rust).expect("write the Rust");
        let binary = dir.join("program");
        let compiled = common::compile(
            &path,
            &["--crate-type", "bin", "-o", &binary.to_string_lossy()],
        );
        assert!(
            compiled.status.success(),
            "{purpose} did not compile at {how:?}:\n{}\n--- emitted ---\n{rust}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let out = Command::new(&binary).output().expect("run it");
        assert!(out.status.success(), "{purpose} failed at {how:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            expected,
            "{purpose} at {how:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// **§1.23: a `sync` function calls a parameter whose type says `sync`.** The
/// caller is held to the word (`NK2206`), so the call keeps the promise; it was
/// refused as a call to something no ledger knows. A parameter *without* the
/// word is still refused.
#[test]
fn a_sync_function_calls_a_sync_parameter() {
    runs(
        "sync-parameter",
        "pub fn apply(f: fn(i64) -> i64 sync, x: i64) -> i64 sync {\n\
         \x20   return f(x)\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{apply(fn(n) { n * 2 }, 21)}\")\n\
         }\n",
        "42\n",
    );
    let parsed = parse_to_ast(
        "pub fn apply(f: fn(i64) -> i64, x: i64) -> i64 sync {\n\
         \x20   return f(x)\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{apply(fn(n) { n * 2 }, 21)}\")\n\
         }\n",
    )
    .expect("the source parses");
    let unsure = nikaia::contracts::sync::check(
        &parsed,
        &Ledger::infer(&parsed),
        &Ledger::parse(STD).expect("std's ledger"),
    );
    assert!(
        unsure.iter().any(|v| v.callee.contains('f')),
        "a parameter without `sync` is not taken at its word: {unsure:?}"
    );
}

/// **§1.24: a `Shared` written into a variant takes the variant's count.** The
/// part was declared with the atomic floor and the value built with the count
/// its position got, and `rustc` said *expected `Shared[E]`, found
/// `Shared[E]`*.
#[test]
fn a_shared_value_in_a_variant_has_the_parts_count() {
    runs(
        "shared-variant",
        "enum E {\n\
         \x20   Leaf(i64),\n\
         \x20   Add(Shared[E], Shared[E]),\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let e = E::Add(Shared(E::Leaf(2)), Shared(E::Leaf(3)))\n\
         \x20   match e {\n\
         \x20       E::Add(l, r) => println(\"add\"),\n\
         \x20       E::Leaf(n) => println(f\"{n}\"),\n\
         \x20   }\n\
         }\n",
        "add\n",
    );
}

/// **§1.25: a `?.` chain through two nullable fields of a lent value.** The
/// first step took `a.b` out of a lent `a`; a member that comes out as a view
/// needs its receiver lent, as one that copies did.
#[test]
fn a_chain_through_two_nullable_fields_of_a_lent_value() {
    runs(
        "nullable-chain",
        "struct C { v: i64 }\n\
         struct B { c: C? }\n\
         struct A { b: B? }\n\
         \n\
         fn f(a: ref A) -> i64 {\n\
         \x20   return a.b?.c?.v ?? -1\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let a = A { b: B { c: C { v: 7 } } }\n\
         \x20   let none = A { b: null }\n\
         \x20   let half = A { b: B { c: null } }\n\
         \x20   println(f\"{f(a)} {f(none)} {f(half)} {f(a)}\")\n\
         }\n",
        "7 -1 -1 7\n",
    );
}

/// **A function nothing declares, called by its bare name, is `NK1117`**, with
/// the near miss where there is one. It lowered as written and `rustc` said
/// *cannot find function*. A bare name has nowhere else to come from: a `use`
/// brings none in.
#[test]
fn a_bare_call_nothing_declares_is_refused() {
    let found = findings(
        "fn doubled(n: i64) -> i64 {\n\
         \x20   return n * 2\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   frobnicate(1)\n\
         \x20   println(f\"{dobled(2)}\")\n\
         }\n",
    );
    let messages: Vec<(&str, &str, Option<&str>)> = found
        .iter()
        .map(|f| (f.code, f.message.as_str(), f.help.as_deref()))
        .collect();
    assert!(
        messages.contains(&(
            "NK1117",
            "nothing declares a function `frobnicate`",
            Some("declare it with `fn frobnicate(…)`, or call it through the package that has it")
        )),
        "{messages:#?}"
    );
    assert!(
        messages.contains(&(
            "NK1117",
            "nothing declares a function `dobled`",
            Some("did you mean `doubled`?")
        )),
        "{messages:#?}"
    );
}
