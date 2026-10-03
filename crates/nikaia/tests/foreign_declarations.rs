//! `extern "C"` and `unsafe`, the two words Part III 15.1 writes
//! ([ADR-124](../../../docs/specification/adr/adr-124.md)).
//!
//! The page wrote C interoperability out in full and the language had **none**
//! of the three things the example needs: `extern` was not reserved and the
//! item was not in the grammar, `unsafe` was not reserved either — so
//! `unsafe { … }` parsed as a name and a block and met `NK1117` — and
//! `Pointer[u8]` is a type nothing declares, which is still true and is that
//! record's §4.
//!
//! **The number that allowed two reserved words is zero.** Nothing in
//! `examples/`, in `tests/` or in the three pages wrote either as a name, and
//! nothing is released — which is [ADR-276](../../../docs/specification/adr/adr-276.md)'s
//! own standard, and the reason they are reserved **with their constructs**
//! rather than ahead of them ([ADR-298](../../../docs/specification/adr/adr-298.md)).

mod common;

use std::process::Command;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

const DECLARED: &str = "extern \"C\" {\n    fn getpid() -> i32\n}\n";

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// **Part III 15.1's shape parses**, which it did not: *expected end of input;
/// found `extern`*.
#[test]
fn the_pages_two_forms_parse() {
    parse_to_ast(&format!(
        "{DECLARED}fn main() {{ let id = unsafe {{ getpid() }}\n    println(f\"{{id}}\") }}\n"
    ))
    .expect("both forms parse");
}

/// **The two words are names no longer** (D1), which is what a reserved word
/// is: `let unsafe = 3` does not parse as a `let` of a name at all.
#[test]
fn neither_word_is_a_name_any_more() {
    for source in [
        "fn main() { let unsafe = 3 }\n",
        "fn main() { let extern = 3 }\n",
    ] {
        assert!(
            parse_to_ast(source).is_err(),
            "a reserved word is not a name (ADR-298 D1): {source}"
        );
    }
}

/// **An `extern` declaration is `sync` and carries no `throws`**, without
/// writing either word (D2).
///
/// Two things are turned around against a trait method, and only one of them by
/// this record. C has no suspension point at all, and a C function that sleeps
/// **blocks a thread** — `println`'s question
/// ([ADR-288](../../../docs/specification/adr/adr-288.md) D21) and not this one —
/// so calling it *pausing* would make every C call an `.await` of a future
/// nobody produced. And C has no failure channel this language reads.
#[test]
fn a_declaration_is_sync_and_cannot_throw() {
    let parsed = parse_to_ast(DECLARED).expect("the source parses");
    let ledger = Ledger::infer(&parsed);
    let entry = ledger
        .functions
        .get("getpid")
        .unwrap_or_else(|| panic!("{:#?}", ledger.functions.keys().collect::<Vec<_>>()));
    assert!(entry.sync_claim.is_sync(), "{:?}", entry.sync_claim);
    assert!(entry.fails_with.is_empty(), "{:?}", entry.fails_with);
    assert_eq!(
        entry.signature.as_ref().expect("a signature").text(),
        "() -> i32"
    );
    // **And what the signature does not say is fail-closed** (D4): absent
    // `touches` reads as *touches everything*, absent `locks` is that column's
    // third answer. A C signature says **less** than a Rust one, not more.
    assert!(entry.touches.is_empty(), "{:?}", entry.touches);
}

/// **`NK1143`: the call is written inside `unsafe { … }` and nowhere else**
/// (D3). That is the whole of what the word buys — the boundary visible *at the
/// call*, in the body somebody reads, rather than in a file beside it.
#[test]
fn a_foreign_call_outside_unsafe_is_refused() {
    let refused = findings(&format!("{DECLARED}fn main() {{ let id = getpid() }}\n"));
    let about = refused
        .iter()
        .find(|f| f.code == "NK1143")
        .unwrap_or_else(|| panic!("{refused:#?}"));
    assert!(about.message.contains("getpid"), "{}", about.message);
    assert!(
        about
            .help
            .as_deref()
            .is_some_and(|h| h.contains("unsafe {")),
        "the way out is paste-ready (Part III C.2): {:?}",
        about.help
    );

    // …and inside one, nothing is said: the block makes no other rule.
    let fine = findings(&format!(
        "{DECLARED}fn main() {{ let id = unsafe {{ getpid() }}\n    println(f\"{{id}}\") }}\n"
    ));
    assert!(fine.is_empty(), "{fine:#?}");
}

/// **Only a name this file declared `extern`.** A Nikaia function and a C
/// declaration are both ledger entries, and only one of them is this; refusing
/// an ordinary call would be
/// [Part III C.4](../../../docs/specification/30-nikaia-tooling.md)'s correct
/// program refused.
#[test]
fn an_ordinary_call_is_not_this_refusal() {
    let refused = findings(
        "fn helper() -> i32 { return 1 }\n\
         fn main() { let n = helper()\n    println(f\"{n}\") }\n",
    );
    assert!(!refused.iter().any(|f| f.code == "NK1143"), "{refused:#?}");
}

/// **The lowering is Rust's own, on both sides**, and the whole of it: the form
/// means the same thing in both languages.
#[test]
fn it_lowers_to_rusts_own_extern_and_unsafe() {
    let rust = lowered(&format!(
        "{DECLARED}fn main() {{ let id = unsafe {{ getpid() }}\n    println(f\"{{id}}\") }}\n"
    ));
    assert!(rust.contains("extern \"C\" {"), "{rust}");
    assert!(rust.contains("fn getpid() -> i32;"), "{rust}");
    assert!(rust.contains("unsafe {"), "{rust}");
}

/// **A second ABI is refused by this compiler**, not by the backend: the
/// grammar takes any string so that the message about an unknown one names what
/// is written rather than arriving about a file nobody wrote (Part III C.1).
#[test]
fn an_abi_this_compiler_does_not_write_is_refused_here() {
    let parsed = parse_to_ast("extern \"stdcall\" {\n    fn f() -> i32\n}\nfn main() { }\n")
        .expect("it parses — the refusal is the emitter's");
    let error = format!(
        "{:#}",
        emit_program(&parsed, Build::default()).expect_err("the ABI is refused")
    );
    assert!(error.contains("stdcall"), "{error}");
    assert!(error.contains("extern \"C\""), "{error}");
}

/// **And it compiles and runs**, which is the only proof that the two forms
/// mean in the language below what they say here.
#[test]
fn the_whole_thing_compiles_and_runs() {
    let rust = lowered(&format!(
        "{DECLARED}fn main() {{ let id = unsafe {{ getpid() }}\n\
         \x20   println(f\"{{id > 0}}\") }}\n"
    ));
    let dir = common::scratch_dir("extern-c");
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary).output().expect("run it");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "true");
    std::fs::remove_dir_all(&dir).ok();
}
