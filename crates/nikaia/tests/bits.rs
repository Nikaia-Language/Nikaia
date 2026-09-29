//! **`u64`, `u32` and the bit operators**
//! ([ADR-248](../../../docs/specification/adr/adr-248.md)): the types a program
//! asked for, `&`, `|`, `^`, `<<`, `>>` and `!` on integers, a literal as wide
//! as a `u64`, and a text's bytes. The program that asked is a hash, so a hash is
//! what is run - against the value FNV-1a has everywhere else - at both settings
//! of `user_parallelism`.

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
        let dir = common::scratch_dir(&format!("bits-{purpose}"));
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
        assert!(
            out.status.success(),
            "{purpose} failed at {how:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            expected,
            "{purpose} at {how:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// **FNV-1a, written as it is written in Rust** (D1, D2, D3, D6): a `u64` whose
/// offset basis is above an `i64`, `^` over the bytes of a text, a wrapping
/// multiply, and the hash cut apart with a shift and a mask. The numbers are
/// FNV-1a's for `hello`, which every implementation agrees on.
#[test]
fn a_hash_is_written_as_it_is_everywhere_else() {
    runs(
        "fnv",
        "fn fnv(key: ref String, seed: u64) -> u64 {\n\
         \x20   let mut hash: u64 = 0xcbf29ce484222325 ^ seed\n\
         \x20   for b in key.bytes() {\n\
         \x20       hash ^= b as u64\n\
         \x20       hash = hash.wrapping_mul(0x100000001b3)\n\
         \x20   }\n\
         \x20   return hash\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let h = fnv(\"hello\", 0)\n\
         \x20   let high: u64 = h >> 32\n\
         \x20   let low: u32 = h.truncating_u32()\n\
         \x20   println(f\"{h} {high} {low} {(h & 0xff) == 0x0b}\")\n\
         }\n",
        "11831194018420276491 2754664518 2158673163 true\n",
    );
}

/// **Each operator once, on a signed and an unsigned type** (D3): and, or,
/// exclusive or, both shifts, and `!` flipping the bits of a `u32`.
#[test]
fn each_bit_operator_says_what_it_says_in_every_language() {
    runs(
        "operators",
        "fn main() {\n\
         \x20   let x: i64 = 0b1010\n\
         \x20   let flags: u32 = 10\n\
         \x20   println(f\"{x & 6} {x | 5} {x ^ 0xF} {x << 3} {x >> 1} {!flags}\")\n\
         }\n",
        "2 15 5 80 5 4294967285\n",
    );
}

/// **The refusals, each in this language's words** (D1, D3, D4): a bit
/// operator beside a comparison without parentheses, with the parenthesised
/// line handed over; `&` on two `bool`s, pointed at `&&`; and a `u64` added to
/// an `i64`.
#[test]
fn what_is_refused_is_refused_with_the_line_to_write() {
    let beside = findings(
        "fn main() {\n\
         \x20   let a: u64 = 5\n\
         \x20   let zero = a & 1 == 0\n\
         \x20   let fine = (a & 1) == 0\n\
         }\n",
    );
    assert!(
        beside
            .iter()
            .any(|f| f.code == "NK1197" && f.help.as_deref() == Some("Write `(a & 1) == 0`.")),
        "{beside:#?}"
    );
    assert_eq!(
        beside.iter().filter(|f| f.code == "NK1197").count(),
        1,
        "the parenthesised line is not refused: {beside:#?}"
    );

    let booleans = findings("fn main() {\n    let both = true & false\n}\n");
    assert!(
        booleans
            .iter()
            .any(|f| f.code == "NK1198" && f.help.as_deref() == Some("`&&` joins two `bool`s")),
        "{booleans:#?}"
    );

    let mixed = findings(
        "fn main() {\n\
         \x20   let a: u64 = 5\n\
         \x20   let b: i64 = 6\n\
         \x20   let c = a + b\n\
         }\n",
    );
    assert!(
        mixed.iter().any(|f| f.code == "NK1199"
            && f.message == "You're mixing a `u64` and an `i64` in one operation."),
        "{mixed:#?}"
    );
}

/// **A literal as wide as a `u64` holds** (D2), and no wider: above an `i64` it
/// is a `u64`'s, and refused against `i64` where no type stands beside it.
#[test]
fn a_literal_above_an_i64_is_a_u64s() {
    let bare = findings("fn main() {\n    let big = 18446744073709551615\n}\n");
    assert!(
        bare.iter().any(|f| f.code == "NK1116"
            && f.help
                .as_deref()
                .is_some_and(|h| h.contains("`let x: u64 = …`"))),
        "{bare:#?}"
    );
    runs(
        "wide-literal",
        "fn main() {\n\
         \x20   let big: u64 = 18446744073709551615\n\
         \x20   println(f\"{big}\")\n\
         }\n",
        "18446744073709551615\n",
    );
    assert!(parse_to_ast("fn main() {\n    let x: u64 = 18446744073709551616\n}\n").is_err());
}

/// **An unannotated constant takes its type from its uses**
/// ([ADR-249](../../../docs/specification/adr/adr-249.md)): with nothing
/// pinning `a`, the three names are one number, no use asks, and what they are
/// given does not fit an `i32` - so the sum is an `i64`. Beside an `i32` `a`,
/// `b` is asked to be an `i32` by `a + b`, and `3000000000` does not fit one:
/// `NK1116` at `b`, naming the use, where it was `rustc`'s *cannot add `i64` to
/// `i32`*.
#[test]
fn a_large_constant_is_an_i64_and_mixes_with_nothing_else() {
    runs(
        "widened",
        "fn main() {\n\
         \x20   let a = 1\n\
         \x20   let b = 3000000000\n\
         \x20   let c = a + b\n\
         \x20   println(f\"{c}\")\n\
         }\n",
        "3000000001\n",
    );
    let pinned = findings(
        "fn main() {\n\
         \x20   let a: i32 = 1\n\
         \x20   let b = 3000000000\n\
         \x20   let c = a + b\n\
         }\n",
    );
    assert!(
        pinned.iter().any(|f| f.code == "NK1116"
            && f.message == "`3000000000` doesn't fit in an `i32`."
            && f.notes
                .iter()
                .any(|n| n.contains("`b` is an `i32` because of how it is used"))),
        "{pinned:#?}"
    );
    assert_eq!(pinned.len(), 1, "one cause, one refusal: {pinned:#?}");
}
