//! **A sequence's count is an `i64`, as a length is** (Part I 2.2,
//! [ADR-285](../../../docs/specification/adr/adr-285.md) D1): issue #164
//! issue #164. `count()` kept the language below's `usize`, and the program met it
//! in `rustc`'s words the moment the count stood where an `i64` is wanted.
//! Each program prints the same at both settings of `user_parallelism`.

mod common;

use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn ran(purpose: &str, source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, how).expect("the source lowers").rust;
    let dir = common::scratch_dir(&format!("counts-{purpose}"));
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
        "{purpose} did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary).output().expect("run it");
    assert!(
        out.status.success(),
        "{purpose} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// **The three programs of the report**: a count returned as an `i64`, bound
/// to one, and added to a length.
#[test]
fn a_count_stands_where_an_i64_is_wanted() {
    let source = "fn count_words(text: ref String) -> i64 {\n\
        \x20   return text.split_whitespace().count()\n\
        }\n\
        \n\
        fn main() {\n\
        \x20   let n: i64 = \"a b c\".split_whitespace().count()\n\
        \x20   let sum = \"abc\".chars().count() + \"abc\".len()\n\
        \x20   let words = count_words(\"one two\")\n\
        \x20   println(f\"{words} {n} {sum}\")\n\
        }\n";
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran("report", source, how), "2 3 6", "at {how:?}");
    }
}

/// **Where the conversion binds**: `as` sits below the unary operators and a
/// method call, so a negated count and a method on a count are parenthesised,
/// as a length is.
#[test]
fn a_count_under_an_operator_or_a_method_is_parenthesised() {
    let source = "fn main() {\n\
        \x20   let xs: Vec[i64] = [1, 2, 3]\n\
        \x20   let down = -xs.iter().count()\n\
        \x20   let shown = xs.iter().count().to_string()\n\
        \x20   println(f\"{down} {shown}\")\n\
        }\n";
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran("bound", source, how), "-3 3", "at {how:?}");
    }
}

/// **A program's own `count` is left as it is**: only the one `std` gives a
/// sequence is converted, which the checker says by what the call resolved to.
#[test]
fn a_programs_own_count_is_not_converted() {
    let source = "struct Tally {\n\
        \x20   seen: String,\n\
        }\n\
        \n\
        impl Tally {\n\
        \x20   fn count(ref self) -> String {\n\
        \x20       return f\"{self.seen}!\"\n\
        \x20   }\n\
        }\n\
        \n\
        fn main() {\n\
        \x20   let t = Tally { seen: f\"x\" }\n\
        \x20   let words = \"a b\".split_whitespace().count() + 1\n\
        \x20   println(f\"{t.count()} {words}\")\n\
        }\n";
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran("own", source, how), "x! 3", "at {how:?}");
    }
}
