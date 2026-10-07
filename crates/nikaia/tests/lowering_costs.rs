//! **What the solver's kernels found the lowering paying for**
//! ([ADR-270](../../../docs/specification/adr/adr-270.md) D8 step 1,
//! `docs/solver-workload.md` §8): each shape below cost instructions the same
//! program written in Rust by hand does not retire, and each is now written the
//! way a person would write it. Each program is run as well, because a cheaper
//! shape that means something else is not cheaper: in
//! `tests/language/src/lowering_costs.nika`, except the one about `main`, which
//! a test block is not.

mod common;

use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust
}

fn prints(purpose: &str, source: &str, expected: &str) {
    let rust = lowered(source);
    let dir = common::scratch_dir(&format!("costs-{purpose}"));
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
    std::fs::remove_dir_all(&dir).ok();
    assert!(out.status.success(), "{purpose} failed");
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "{purpose}");
}

/// **A list the body only reads is a slice**: `&[T]` holds the buffer and the
/// length in two registers, where every read through a `&Vec<T>` loads them
/// again. A list the body changes stays `&mut Vec<T>`.
#[test]
fn a_list_the_body_only_reads_is_a_slice() {
    let source = "\
fn sum(xs: ref Vec[i64], mut out: Vec[i64]) -> i64 {
    let mut total = 0
    for x in xs {
        total += x
    }
    out.push(total)
    return total
}

fn main() {
    let xs = [1, 2, 3]
    let mut out: Vec[i64] = []
    println(f\"{sum(xs, out)} {out.len()}\")
}
";
    let rust = lowered(source);
    assert!(
        rust.contains("fn sum(xs: &[i64], out: &mut Vec<i64>)"),
        "{rust}"
    );
}

/// **An empty list resized right away is one allocation**: `vec![v; n]`, as
/// a person writes it, rather than an empty `Vec` grown afterwards.
#[test]
fn an_empty_list_resized_at_once_is_allocated_at_its_length() {
    let source = "\
fn main() {
    let n = 4
    let mut xs: Vec[i64] = []
    xs.resize(n, 7)
    println(f\"{xs.len()} {xs[3]}\")
}
";
    let rust = lowered(source);
    assert!(rust.contains("= vec![7; "), "{rust}");
    assert!(!rust.contains(".resize("), "{rust}");
}

/// **A `main` that cannot pause starts the runtime only when asked**: no I/O
/// worker is started, so the program stays single-threaded and the allocator
/// takes no locks.
#[test]
fn a_main_that_cannot_pause_starts_the_runtime_on_demand() {
    let source = "\
fn main() {
    println(\"hi\")
}
";
    let rust = lowered(source);
    assert!(rust.contains("nikaia_std::rt::on_demand("), "{rust}");
    prints("on-demand", source, "hi\n");
}

/// **A counter that starts at zero and only grows is never negative**, so it
/// indexes and compares as it is: no sign test on every read.
#[test]
fn a_counter_that_only_grows_indexes_without_a_sign_test() {
    let source = "\
fn main() {
    let xs = [5, 6, 7]
    let mut i = 0
    let mut sum = 0
    while i < xs.len() {
        sum += xs[i]
        i += 1
    }
    println(f\"{sum}\")
}
";
    let rust = lowered(source);
    assert!(rust.contains("((i) as usize) < xs.len()"), "{rust}");
    assert!(rust.contains("(i) as usize)"), "{rust}");
    assert!(!rust.contains("index::at(i)"), "{rust}");
}
