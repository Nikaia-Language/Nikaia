//! Defects a move of the toolchain into Nikaia found (ADR-294 D3, #125).
//!
//! Each program here was accepted by the checker and lowered to Rust that
//! `rustc` refused, so the test is the whole path: lower, compile, run, and
//! compare what it printed.

mod common;

use std::process::Command;

/// Lowers `source` as a one-file program, compiles the Rust it came to and
/// returns what the program printed.
fn printed(purpose: &str, source: &str) -> String {
    let dir = common::scratch_dir(&format!("selfhosting-{purpose}"));
    std::fs::write(dir.join("p.nika"), source).expect("write the source");
    let lowered = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&dir)
        .args(["lower", "p.nika", "--output", "p.rs"])
        .output()
        .expect("the nikaia binary runs");
    assert!(
        lowered.status.success(),
        "{purpose} did not lower:\n{}",
        String::from_utf8_lossy(&lowered.stderr)
    );
    let binary = dir.join("p");
    let compiled = common::compile(
        &dir.join("p.rs"),
        &["--crate-type", "bin", "-o", &binary.to_string_lossy()],
    );
    assert!(
        compiled.status.success(),
        "{purpose} did not compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary).output().expect("run it");
    assert!(out.status.success(), "{purpose} did not run");
    std::fs::remove_dir_all(&dir).ok();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// #559: a view cut from a call's result is used after the statement, so the
/// result must live as long as the view.
#[test]
fn a_view_of_a_call_result_outlives_the_statement() {
    let said = printed(
        "view-of-a-call",
        r#"
fn f() -> String { return "  abc,d  " }
fn main() {
    let v = f().trim_start()
    let w = f().trim().split(",").collect()
    let n = f().trim_end().len()
    println(f"[{v}] {w.len()} {n}")
}
"#,
    );
    assert_eq!(said.trim(), "[abc,d  ] 2 7");
}

/// #560: `Wrapped(x, at)` is the constructor of a struct another file of the
/// package declares, as it is of one this file declares.
#[test]
fn a_constructor_call_of_a_struct_in_another_file_is_the_new_it_has() {
    let dir = common::scratch_dir("selfhosting-constructor-beside");
    std::fs::write(
        dir.join("a.nika"),
        "pub struct Wrapped[T] {\n    pub node: T,\n    pub at: i64,\n}\n\n\
         impl Wrapped[T] {\n    pub fn new(node: T, at: i64) -> Wrapped[T] {\n        \
         return Wrapped { node: node, at: at }\n    }\n}\n",
    )
    .expect("write a");
    std::fs::write(
        dir.join("b.nika"),
        "pub fn wrap(name: String, at: i64) -> Wrapped[String] {\n    return Wrapped(name, at)\n}\n\n\
         pub fn wrap_written(name: String, at: i64) -> Wrapped[String] {\n    \
         return Wrapped { node: name, at: at }\n}\n",
    )
    .expect("write b");
    let lowered = nikaia::sysroot::lower_tools(&dir).expect("the package lowers");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        lowered.contains("Wrapped::new(name, at)"),
        "the call spelling reaches the `new`:\n{lowered}"
    );
    assert!(
        lowered.contains("Wrapped { node: name, at }"),
        "and the struct spelling stays a struct:\n{lowered}"
    );
}

/// #565: the lowered tools package allows the two rustc lints that valid
/// Nikaia trips (`let mut s = init` then branches that always assign `s`, and
/// a `while true { … return … }` then a `return`). Measured once by adding both
/// shapes to a tools file and running `cargo check -p nikaia-std`: two
/// warnings without the attribute, none with it.
#[test]
fn the_tools_package_allows_what_valid_nikaia_trips() {
    let lib = include_str!("../../nikaia-std/src/lib.rs");
    let before_the_module = lib
        .split("pub mod tools {")
        .next()
        .expect("the tools module is declared");
    let attribute = before_the_module
        .rsplit("#[allow(")
        .next()
        .expect("an allow stands over it");
    for lint in ["unused_assignments", "unreachable_code"] {
        assert!(attribute.contains(lint), "{lint} is allowed over `tools`");
    }
}

/// #563: a `String` local reassigned from a view of itself, in a loop, is a
/// mixed text; the view is copied before it replaces the buffer it points
/// into. The tools package also needs `IntoEither` in scope for it, which
/// `nikaia-std`'s `tools` module brings (measured with a probe in a tools
/// file: E0599 without the import, none with it).
#[test]
fn a_string_reassigned_from_a_view_of_itself_in_a_loop() {
    let said = printed(
        "string-from-its-own-view",
        r#"
fn bare(lines: ref Vec[String]) -> Vec[String] {
    let mut out: Vec[String] = []
    for line in lines {
        let mut rest: String = line.clone()
        let mut again = true
        while again {
            again = false
            if rest.starts_with(">") {
                rest = rest.strip_prefix(">") ?? ""
                again = true
            }
        }
        out.push(rest.clone())
    }
    return out
}
fn main() {
    let v: Vec[String] = [">>a", "b"]
    println(f"{bare(v).join(",")}")
}
"#,
    );
    assert_eq!(said.trim(), "a,b");
    let lib = include_str!("../../nikaia-std/src/lib.rs");
    assert!(
        lib.contains("use crate::either_text::{IntoEither, IntoEitherMaybe};"),
        "the tools module has the conversions in scope"
    );
}

/// #564: `drain()` over a `mut` parameter takes the elements out of the
/// caller's list as owned values.
#[test]
fn drain_over_a_mut_parameter_empties_the_callers_list() {
    let said = printed(
        "drain-a-mut-parameter",
        r#"
fn take_all(mut from: Vec[String], mut into: Vec[String]) -> i64 {
    let mut count = 0
    for x in from.drain() {
        into.push(x)
        count += 1
    }
    return count
}
fn main() {
    let mut source: Vec[String] = ["a", "b"]
    let mut kept: Vec[String] = []
    let n = take_all(source, kept)
    println(f"{n} {source.len()} {kept.len()}")
}
"#,
    );
    assert_eq!(said.trim(), "2 0 2");
}

/// #561: a `??` whose fallback is an index read names a value, not a reference.
#[test]
fn a_fallback_read_at_an_index_is_a_value() {
    let said = printed(
        "fallback-at-an-index",
        r#"
fn word(maybe: String?, list: ref Vec[String]) -> String {
    let x = maybe ?? list[0]
    return x
}
fn number(maybe: i64?, list: ref Vec[i64]) -> i64 {
    let x = maybe ?? list[0]
    return x
}
fn main() {
    let words: Vec[String] = ["z"]
    let numbers: Vec[i64] = [7]
    println(f"{word(null, words)} {word("a", words)} {number(null, numbers)} {number(3, numbers)}")
}
"#,
    );
    assert_eq!(said.trim(), "z a 7 3");
}

/// #576: a `mut` parameter is a `&mut T` below, so assigning it whole writes
/// through it and reading a number out of it reads the number - `n = n + 1`
/// was `n = n + 1;` against a `&mut i64`, which `rustc` refused, on exactly
/// the program `NK1138` asks for. Handed to another `mut` parameter it is
/// handed on as it is.
#[test]
fn a_mut_parameter_is_assigned_and_read_through() {
    let said = printed(
        "mut-parameter-assigned",
        r#"
struct P {
    x: i64,
}

fn bump(mut n: i64) {
    n = n + 1
    n += 2
    if n == 4 {
        n = n * 10
    }
}

fn twice(mut n: i64) {
    bump(n)
    bump(n)
}

fn step(mut n: i64, xs: ref Vec[i64]) {
    n = xs[n] + n.abs() - (-n)
}

fn flip(mut b: bool) {
    b = !b
}

fn reset(mut p: P) {
    p = P { x: 0 }
}

fn refill(mut xs: Vec[i64]) {
    xs = [7, 8]
    xs.push(9)
}

fn main() {
    let mut n = 1
    bump(n)
    let mut m = 0
    twice(m)
    let mut k = 1
    step(k, [10, 20, 30])
    let mut b = false
    flip(b)
    let mut p = P { x: 5 }
    reset(p)
    let mut xs: Vec[i64] = []
    refill(xs)
    println(f"{n} {m} {k} {b} {p.x} {xs.len()}")
}
"#,
    );
    assert_eq!(said.trim(), "40 6 22 true 0 3");
}
