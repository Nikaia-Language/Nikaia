//! **A map whose keys the build knew** —
//! [ADR-176](../../../docs/specification/adr/adr-176.md).
//!
//! These tests **run** the program, and that is the point of them rather than
//! a preference. The table is built twice by two programs that never meet:
//! `crates/nikaia/src/fixed.rs` decides where each key lands while the compiler
//! runs, and `crates/nikaia-std/src/fixed.rs` finds it again while the program
//! runs. Two implementations of one hash function, in two crates, with no type
//! holding them together — so a test that only looked at the emitted `const`,
//! or only at what the checker said, would pass for a table that answers every
//! lookup with its neighbour's value.
//!
//! A lookup is only right if it comes back with the right number. So the tests
//! below compile the file and print what it found.

mod common;

use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit;
use nikaia::parser::parse_to_ast;
use std::process::Command;

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check(&parsed, &own, &library).findings
}

fn lower(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit::emit_program(&parsed, Default::default())
        .expect("the source lowers")
        .rust
}

fn run(purpose: &str, source: &str) -> String {
    let dir = common::scratch_dir(purpose);
    let rust = lower(source);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "a table did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run it");
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let out = String::from_utf8_lossy(&ran.stdout).to_string();
    std::fs::remove_dir_all(&dir).ok();
    out
}

/// Three keys: under [`nikaia::fixed::HASHED_FROM`], so the walked shape.
const SMALL: &str = "comptime ROUTES: Fixed[ref String, i64] = [(\"get\", 1), (\"post\", 2), (\"put\", 3)]\n\
     \n\
     fn main() {\n\
     \x20   println(f\"{ROUTES.get(\"get\") ?? 0}\")\n\
     \x20   println(f\"{ROUTES.get(\"post\") ?? 0}\")\n\
     \x20   println(f\"{ROUTES.get(\"put\") ?? 0}\")\n\
     \x20   println(f\"{ROUTES.get(\"patch\") ?? 0}\")\n\
     \x20   println(f\"{ROUTES.len()}\")\n\
     }";

/// Fourteen keys: from [`nikaia::fixed::HASHED_FROM`] up, so the hashed shape.
const LARGE: &str = "comptime WORDS: Fixed[ref String, i64] = [\n\
     \x20   (\"alpha\", 1), (\"bravo\", 2), (\"charlie\", 3), (\"delta\", 4),\n\
     \x20   (\"echo\", 5), (\"foxtrot\", 6), (\"golf\", 7), (\"hotel\", 8),\n\
     \x20   (\"india\", 9), (\"juliet\", 10), (\"kilo\", 11), (\"lima\", 12),\n\
     \x20   (\"mike\", 13), (\"november\", 14),\n\
     ]\n\
     \n\
     fn main() {\n\
     \x20   println(f\"{WORDS.get(\"alpha\") ?? 0}\")\n\
     \x20   println(f\"{WORDS.get(\"golf\") ?? 0}\")\n\
     \x20   println(f\"{WORDS.get(\"november\") ?? 0}\")\n\
     \x20   println(f\"{WORDS.get(\"zulu\") ?? 0}\")\n\
     \x20   println(f\"{WORDS.len()}\")\n\
     }";

/// **The small table answers every key it holds, and refuses the one it does
/// not** (D3's walked shape).
///
/// `?? 0` is what turns the missing key into a number to print; `0` is not a
/// value in the table, so the fourth line is the absence and not a hit.
#[test]
fn a_walked_table_finds_each_key_while_the_program_runs() {
    assert_eq!(run("fixed-small", SMALL), "1\n2\n3\n0\n3\n");
}

/// **The hashed table answers the same way** (D3's CHD shape), which is the
/// test the two hash implementations exist for: the compiler put `golf` in a
/// slot, and `std` has to hash its way back to the same one.
#[test]
fn a_hashed_table_finds_each_key_while_the_program_runs() {
    assert_eq!(run("fixed-large", LARGE), "1\n7\n14\n0\n14\n");
}

/// **Every key, not a sample.** A slot mix-up need not be visible in three
/// lookups — the CHD generator moves keys around, and a table where two of them
/// swapped values would pass a test that asked about neither. So this asks
/// about all fourteen and the answer is the sum of one to fourteen.
#[test]
fn a_hashed_table_answers_all_of_its_keys() {
    let source = "comptime WORDS: Fixed[ref String, i64] = [\n\
         \x20   (\"alpha\", 1), (\"bravo\", 2), (\"charlie\", 3), (\"delta\", 4),\n\
         \x20   (\"echo\", 5), (\"foxtrot\", 6), (\"golf\", 7), (\"hotel\", 8),\n\
         \x20   (\"india\", 9), (\"juliet\", 10), (\"kilo\", 11), (\"lima\", 12),\n\
         \x20   (\"mike\", 13), (\"november\", 14),\n\
         ]\n\
         comptime NAMES: Array[ref String, 14] = [\n\
         \x20   \"alpha\", \"bravo\", \"charlie\", \"delta\", \"echo\", \"foxtrot\", \"golf\",\n\
         \x20   \"hotel\", \"india\", \"juliet\", \"kilo\", \"lima\", \"mike\", \"november\",\n\
         ]\n\
         \n\
         fn main() {\n\
         \x20   let mut total = 0\n\
         \x20   for name in NAMES {\n\
         \x20       total = total + (WORDS.get(name) ?? 0)\n\
         \x20   }\n\
         \x20   println(f\"{total}\")\n\
         }";
    assert_eq!(run("fixed-every-key", source), "105\n");
}

/// **The small table is four static arrays and no displacements** (D2, D3).
///
/// What the shape is, is data: an empty `disps` *is* the walked table. The
/// compiler holds no branch that says "this one is small" past the generator,
/// and `std` holds none past the `is_empty()` in `get`.
#[test]
fn under_the_threshold_the_table_carries_no_displacements() {
    let rust = lower(SMALL);
    let line = rust
        .lines()
        .find(|line| line.contains("const ROUTES"))
        .expect("the table reached the generated file");
    assert_eq!(
        line.trim(),
        "const ROUTES: Fixed<i64> = Fixed::new(0, &[], &[\"get\", \"post\", \"put\"], &[1, 2, 3]);"
    );
}

/// **From the threshold there are displacements**, one pair per bucket — and
/// the keys are in slot order rather than the order they were written, which is
/// the visible sign that a table was built rather than a list copied.
#[test]
fn from_the_threshold_the_table_carries_displacements() {
    let rust = lower(LARGE);
    let line = rust
        .lines()
        .find(|line| line.contains("const WORDS"))
        .expect("the table reached the generated file");
    assert!(
        !line.contains("&[], &["),
        "a table of fourteen keys should be hashed:\n{line}"
    );
    assert!(
        !line.contains("&[\"alpha\", \"bravo\""),
        "a hashed table's keys are in slot order, not written order:\n{line}"
    );
}

/// **The value is whatever a `const` can hold**, and text is one of those (D2).
///
/// Nothing in the table is about `i64`: `rust_constant_type` answers for the
/// declared value type and `rust_value` writes the value, which is the same
/// pair every other `comptime` goes through.
#[test]
fn a_tables_values_may_be_text() {
    let source = r#"comptime MIME: Fixed[ref String, ref String] = [("html", "text/html"), ("json", "application/json")]

fn main() {
    println(MIME.get("json") ?? "?")
    println(MIME.get("css") ?? "?")
    println(f"{MIME.has("html")}")
}"#;
    assert_eq!(run("fixed-text", source), "application/json\n?\ntrue\n");
}

/// **A table of nothing is a table**, and it used to be `NK1166`.
///
/// `[]` is a `Vec[?]`, so the crossing asked whether `?` was a pair of the
/// declared types and got no — and the refusal read *this is a `Vec[?]` and the
/// `const` says `Fixed[&str, i64]`*, whose way out asks the reader to write
/// what they already wrote ([C.2](../../../docs/specification/30-nikaia-tooling.md),
/// [C.4](../../../docs/specification/30-nikaia-tooling.md)). `?` fits
/// everything ([ADR-024](../../../docs/specification/adr/adr-024.md) D1), and
/// the array crossing one shape over had always read it that way —
/// `comptime XS: Array[i64, 0] = []` lowered the whole time.
#[test]
fn a_table_of_nothing_is_a_table() {
    let source = r#"comptime EMPTY: Fixed[ref String, i64] = []

fn main() {
    println(f"{EMPTY.len()} {EMPTY.is_empty()} {EMPTY.get("get") ?? 0}")
}"#;
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert_eq!(run("fixed-empty", source), "0 true 0\n");
}

/// **A key written twice is `NK1169`** (D4), and it is the *only* refusal: a
/// duplicate is a mistake the compiler can name, so `NK1127` saying it cannot
/// evaluate the constant would be the same refusal again with less in it.
#[test]
fn a_key_written_twice_is_refused_once() {
    let source = "comptime ROUTES: Fixed[ref String, i64] = [(\"get\", 1), (\"post\", 2), (\"get\", 3)]\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{ROUTES.len()}\")\n\
         }";
    let codes: Vec<&str> = findings(source).iter().map(|found| found.code).collect();
    assert_eq!(codes, vec!["NK1169"]);
}

/// **A key that is not text is `NK1170`** (D5), once, for the same reason.
///
/// The way out it offers is a real one on both sides: text keys are this table,
/// and a `collections::HashMap` built while the program runs takes any key at
/// all.
#[test]
fn a_key_that_is_not_text_is_refused_once() {
    let source = "comptime ROUTES: Fixed[i64, i64] = [(1, 1), (2, 2)]\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{ROUTES.len()}\")\n\
         }";
    let codes: Vec<&str> = findings(source).iter().map(|found| found.code).collect();
    assert_eq!(codes, vec!["NK1170"]);
}

/// **The two hash implementations agree**, checked where they live rather than
/// through a program.
///
/// This is the weaker test of the pair and it is here for the message it gives
/// when it fails: the running tests above say *the table answered wrongly*,
/// and this one says *the two `fnv`s disagree*, which is the first thing to
/// look at. It proves nothing on its own — both could be wrong together — which
/// is why it is not the only test.
#[test]
fn the_compiler_and_the_library_hash_the_same_bytes() {
    for key in ["", "a", "get", "november", "a longer key with spaces", "ü"] {
        for seed in [0u64, 1, 7, 199] {
            assert_eq!(
                nikaia::fixed::fnv(key, seed),
                nikaia_std::fixed::fnv(key, seed),
                "`{key}` at seed {seed}"
            );
        }
    }
}

/// **The threshold is where the measurement put it**
/// ([ADR-176](../../../docs/specification/adr/adr-176.md) D3).
///
/// Twelve is not arbitrary and it is not free to move: the tests above are
/// written around it, one on each side. A change here is a re-measurement, and
/// this line is what makes that deliberate.
#[test]
fn the_threshold_is_twelve() {
    assert_eq!(nikaia::fixed::HASHED_FROM, 12);
}

/// **A table may hold a `struct`, as a view of one**
/// ([ADR-180](../../../docs/specification/adr/adr-180.md) D1).
///
/// It was `NK1127` — *this compiler cannot evaluate it* — for a value that
/// evaluated perfectly well: every part of it crosses on its own, and only the
/// combination did not. What stood in the way is `get`'s shape, which hands
/// back a **value** and needs that value to be `Copy` (0.0.118's own
/// correction). A Nikaia `struct` is not, and **a reference to one is** — so
/// the table holds `&'static Row` and nothing about `get` changes.
///
/// **The row is read through `?.`**, which is what a `T?` already asks for
/// (Part I 2.3), and a `&[&str]` inside it gets its `&` from
/// [ADR-179](../../../docs/specification/adr/adr-179.md) D2 — this is the one
/// place the two records meet.
#[test]
fn a_table_holds_a_declared_type_and_the_program_reads_it() {
    let source = "struct Row { a: i64, tags: ref Array[ref String] }\n\
                  enum Shade { Odd, Even }\n\
                  \n\
                  comptime TABLE: Fixed[ref String, Row] = [\n\
                  \x20   (\"x\", Row { a: 1, tags: [\"one\", \"uno\"] }),\n\
                  \x20   (\"y\", Row { a: 2, tags: [\"two\"] }),\n\
                  ]\n\
                  comptime SHADES: Fixed[ref String, Shade] = [(\"a\", Shade::Odd), (\"b\", Shade::Even)]\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{TABLE.get(\"y\")?.a ?? 0}\")\n\
                  \x20   println(f\"{TABLE.get(\"x\")?.tags?.len() ?? 0}\")\n\
                  \x20   println(f\"{TABLE.get(\"zz\")?.a ?? -1}\")\n\
                  \x20   println(f\"{SHADES.len()}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lower(source);
    // **A view of the row**, and the `&` in front of each one.
    assert!(
        rust.contains(
            "const TABLE: Fixed<&'static Row> = Fixed::new(0, &[], &[\"x\", \"y\"], &[&Row {"
        ),
        "{rust}"
    );
    assert!(
        rust.contains("tags: &[\"one\", \"uno\"]"),
        "the run inside the row is a view too: {rust}"
    );
    assert!(
        rust.contains("const SHADES: Fixed<&'static Shade>"),
        "an `enum` is the same case: {rust}"
    );

    assert_eq!(run("a table of rows", source), "2\n2\n-1\n2\n");
}

/// …and **a part of a row the language below cannot write is still `NK1167`**
/// ([ADR-180](../../../docs/specification/adr/adr-180.md) D3).
///
/// The table opened a second door to the same place: a row holding a `Vec`
/// lowered and `rustc` answered *expected `Vec<i64>`, found `[{integer}; 2]`*
/// about the generated file, which is
/// [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s class.
/// The walk that asks the question now reaches a **pair**, which is what a
/// table's rows are.
#[test]
fn a_row_that_owns_memory_is_refused_by_name() {
    let found = findings(
        "struct Bad { items: Vec[i64] }\n\
         \n\
         comptime T: Fixed[ref String, Bad] = [(\"x\", Bad { items: [1, 2] })]\n\
         \n\
         fn main() { println(f\"{T.len()}\") }\n",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1167");
    assert!(
        found[0].message.contains("`T.items` is a `Vec[i64]`"),
        "{}",
        found[0].message
    );
}
