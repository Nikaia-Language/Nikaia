//! `std`'s own types are constructed by the anonymous constructor
//! ([ADR-140](../../../docs/specification/adr/adr-140.md) D2).
//!
//! `pub fn(first: i32)` is what a `.nika` file writes (Part I 4.2) and `new` was
//! Rust's convention reaching through a hand-written ledger — two conventions
//! for one thing, which is `language-review.md` §3.3's second row. One stays,
//! and it is this language's own.
//!
//! **The ledger's key does not move.** `Type::new` is the name the *lowering*
//! writes, and the lowering is name for name
//! ([ADR-011](../../../docs/specification/adr/adr-011.md) D2); what changed is
//! that the resolution reaching for it now reaches into the library too, where
//! it used to stop at this unit.

mod common;

use nikaia::contracts::{Ledger, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust
}

fn refusals(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.code == "NK1149")
        .collect()
}

/// **`Vec()`, `String()` and `HashMap()`**, and each lowers to the name the
/// language below has.
#[test]
fn stds_types_are_called_like_a_constructor() {
    let rust = lowered(
        "use std::collections\n\nfn main() {\n\
         \x20   let mut xs = Vec()\n\
         \x20   let s = String()\n\
         \x20   let m = collections::HashMap()\n\
         \x20   xs.push(1)\n\
         \x20   println(f\"{xs.len()} {s.len()} {m.len()}\")\n\
         }\n",
    );
    assert!(rust.contains("Vec::new()"), "{rust}");
    assert!(rust.contains("String::new()"), "{rust}");
    // **And the map goes through `path`**, which is where one name below
    // depends on more than the name: at the default provenance the map is the
    // trusted one, and `new` exists only for the default hasher
    // ([ADR-010](../../../docs/specification/adr/adr-010.md) D5). Writing
    // `HashMap::new(` straight out of the constructor arm would have taken that
    // back for every `HashMap()` in a trusted program, which is what this line
    // is here to catch.
    assert!(rust.contains("TrustedMap::default()"), "{rust}");
}

/// **And it keeps its type arguments**, which is the half that had to be
/// corrected rather than added: the anonymous-constructor rule handed back
/// `Ty::named(ty)` — right for a `.nika` type, which has no parameters, and
/// wrong for `Vec::new`, whose entry declares `-> Vec[?]`. `let xs = Vec()`
/// came out as a plain `Vec` and `NK1106` refused it against every `Vec[T]` it
/// was given to.
#[test]
fn a_constructed_vec_keeps_its_element_type() {
    let parsed = parse_to_ast(
        "struct S { xs: Vec[i64] }\n\
         fn main() {\n\
         \x20   let xs = Vec()\n\
         \x20   let s = S { xs: xs }\n\
         \x20   println(f\"{s.xs.len()}\")\n\
         }\n",
    )
    .expect("it parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    let found: Vec<_> = nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.code == "NK1106")
        .collect();
    assert!(found.is_empty(), "{found:#?}");
}

/// **A written `Type::new` is refused**, in `std` as in a `.nika` file, which is
/// the whole of what D2 evens out.
#[test]
fn new_is_refused_at_a_call() {
    let found = refusals("fn main() { let xs = Vec::new() println(f\"{xs.len()}\") }");
    assert_eq!(found.len(), 1, "{found:#?}");
    let help = found[0].help.as_deref().expect("a way out");
    assert!(help.contains("`Vec(…)`"), "{help}");
}

/// **And in value position**, which is the case the record names:
/// `par_fold(M, Summary::new, …)` is how `1brc.nika` wrote it, against a type
/// declaring an anonymous constructor and no `new`.
#[test]
fn new_is_refused_as_a_value_and_the_bare_name_lowers() {
    let source = |ctor: &str| {
        format!(
            "struct S {{ n: i64 }}\n\
             impl S {{ pub fn(n: i64) -> S {{ return S {{ n: n }} }} }}\n\
             fn take(f: fn(i64) -> S) -> i64 {{ return f(1).n }}\n\
             fn main() {{ println(f\"{{take({ctor})}}\") }}\n"
        )
    };
    assert_eq!(refusals(&source("S::new")).len(), 1);
    assert!(refusals(&source("S")).is_empty());
    // The bare name is the constructor, and the language below wants its key -
    // behind the `&` a function-typed parameter takes, because `take` only calls
    // what it is handed ([ADR-094](../../../docs/specification/adr/adr-094.md)
    // D1, Part I 5.4 C).
    assert!(lowered(&source("S")).contains("take(&S::new)"));
}

/// **A qualified name is left alone**, which is `NK1135`'s convention one
/// refusal over: `http::Server::new()` names a package this build cannot see,
/// and whether it should be `http::Server()` is that package's ledger to say.
#[test]
fn a_package_that_nothing_describes_is_not_refused() {
    let found = refusals("fn main() { let s = http::Server::new() }");
    assert!(found.is_empty(), "{found:#?}");
}

fn every_finding(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

fn ran(purpose: &str, source: &str) -> String {
    let rust = lowered(source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );
    let out = std::process::Command::new(&binary)
        .output()
        .expect("run it");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// **The other three collections are built** (0.0.232), and a `std` type with
/// no constructor described is refused here rather than by `rustc`.
///
/// `collections::HashSet()` passed the check and lowered as it was written,
/// and the language below said *expected function, found type alias* about a
/// file nobody wrote. The ledger described `HashSet`, `BTreeMap` and
/// `BTreeSet` as types and none of them as constructed, so each now has its
/// `::new` entry, and `NK1190` stands where the next such gap would land.
#[test]
fn the_other_three_collections_are_built() {
    // Text for the lookups: a key that is not already a view is passed by
    // value, which is `open-work.md` §1's *a method's key of a number type*.
    let source = "use std::collections\n\
                  \n\
                  fn main() {\n\
                  \x20   let mut seen = collections::HashSet()\n\
                  \x20   seen.insert(\"a\")\n\
                  \x20   seen.insert(\"a\")\n\
                  \x20   let mut order = collections::BTreeMap()\n\
                  \x20   order.insert(\"two\", 2)\n\
                  \x20   order.insert(\"one\", 1)\n\
                  \x20   let mut kept = collections::BTreeSet()\n\
                  \x20   kept.insert(\"x\")\n\
                  \x20   let known = order.contains_key(\"one\") && seen.contains(\"a\") && kept.contains(\"x\")\n\
                  \x20   println(f\"{seen.len()} {order.len()} {kept.len()} {known}\")\n\
                  }\n";
    assert!(
        every_finding(source).is_empty(),
        "{:#?}",
        every_finding(source)
    );
    assert!(lowered(source).contains("collections::BTreeMap::new()"));
    assert_eq!(ran("three-collections", source), "1 2 1 true\n");
}

/// **What `NK1190` refuses**: a type the library names, called as its
/// constructor, with no `::new` entry beside it. `io::IoError` is a `std` type
/// nothing constructs this way.
#[test]
fn a_std_type_with_no_constructor_is_refused_here() {
    let source = "use std::io\n\
                  \n\
                  fn main() {\n\
                  \x20   let e = io::IoError()\n\
                  }\n";
    let found: Vec<_> = every_finding(source)
        .into_iter()
        .filter(|f| f.code == "NK1190")
        .collect();
    assert_eq!(found.len(), 1, "{:#?}", every_finding(source));
    assert_eq!(
        found[0].message,
        "`io::IoError` is a type, and nothing describes a constructor for it"
    );
}

/// **A struct of the program's own is built with a literal** (0.0.240):
/// called like a function with no anonymous constructor declared, it lowered to
/// `T::new(…)` and `rustc` said *no function named `new`* about a file nobody
/// wrote. `NK1190` says so, and hands over the literal.
#[test]
fn a_struct_without_a_constructor_is_not_called() {
    let source = "struct NotANumber {\n\
                  \x20   text: String,\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let e = NotANumber(\"x\")\n\
                  \x20   println(e.text)\n\
                  }\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    let found: Vec<_> = nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.code == "NK1190")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].help.as_deref(),
        Some("build it with a literal: `NotANumber { text: … }`")
    );
}
