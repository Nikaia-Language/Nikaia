//! Reading a map through the brackets is a `T?`
//! ([ADR-114](../../../docs/specification/adr/adr-114.md)).
//!
//! A map has a value only where the key is, and *there is nothing there* is
//! data about the world rather than a bug in the program. So the bracket says
//! what `get` says, and a program that knows better says so on the right of a
//! `??` — which is the abort it used to get for free, now written.
//!
//! **A list is the other half and does not move** (D3): `xs[i]` is a `T`, and an
//! index outside it ends the program as
//! [Part III A.2](../../../docs/specification/30-nikaia-tooling.md) says. The
//! line between the two containers is the line between arithmetic and data.

mod common;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

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

/// Lower it, compile it, run it. A read that answers the wrong shape is a
/// `rustc` error about a generated file, so reading the Rust is not enough.
fn output(purpose: &str, source: &str) -> String {
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        ran.status.success(),
        "the program runs:\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    String::from_utf8_lossy(&ran.stdout).trim().to_string()
}

// ---------------------------------------------------------------------------
// D1: the read
// ---------------------------------------------------------------------------

/// **An absent key is a value and not an abort** (D1), which is the whole of
/// the record: before it, this program ended.
#[test]
fn an_absent_key_is_a_value() {
    let printed = output(
        "map-absent",
        "use std::collections\n\
         fn main() {\n\
         \x20   let mut counts = collections::HashMap()\n\
         \x20   counts[\"a\"] = 1\n\
         \x20   let there = counts[\"a\"] ?? 0\n\
         \x20   let absent = counts[\"b\"] ?? -1\n\
         \x20   println(f\"{there}\")\n\
         \x20   println(f\"{absent}\")\n\
         }\n",
    );
    assert_eq!(printed, "1\n-1");
}

/// **And the read is a `T?` the checker knows about**, so reaching a member off
/// one with a plain `.` is `NK1125` rather than a `rustc` error about the
/// generated file.
#[test]
fn a_member_off_a_map_read_is_refused() {
    let found: Vec<_> = findings(
        "use std::collections\n\
         struct Stats { min: i64 }\n\
         fn main() {\n\
         \x20   let mut m = collections::HashMap()\n\
         \x20   m[\"a\"] = Stats { min: 1 }\n\
         \x20   let s = m[\"a\"]\n\
         \x20   println(f\"{s.min}\")\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1125")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
}

/// **A `&` in front of it does not lose the question** — a view of a `T?` is a
/// nullable view, which is the shape Part I 2.3 writes `&str?`. Answering
/// *unknown* there is what let `let s = &m[k]` through to `rustc`.
#[test]
fn a_view_of_a_map_read_is_still_nullable() {
    let found: Vec<_> = findings(
        "use std::collections\n\
         struct Stats { min: i64 }\n\
         fn main() {\n\
         \x20   let mut m = collections::HashMap()\n\
         \x20   m[\"a\"] = Stats { min: 1 }\n\
         \x20   let s = ref m[\"a\"]\n\
         \x20   println(f\"{s.min}\")\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1125")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
}

/// **A value that does not copy is reached as a view** (D4), which is what lets
/// a map of structs be read without an allocation the program did not write
/// ([ADR-008](../../../docs/specification/adr/adr-008.md) D5).
#[test]
fn a_value_that_does_not_copy_is_a_view() {
    let printed = output(
        "map-view",
        "use std::collections\n\
         struct Stats { min: i64, max: i64 }\n\
         fn main() {\n\
         \x20   let mut m = collections::HashMap()\n\
         \x20   m[\"a\"] = Stats { min: 1, max: 9 }\n\
         \x20   let s = m[\"a\"] ?? panic(f\"a was a key a moment ago\")\n\
         \x20   println(f\"{s.min}/{s.max}\")\n\
         }\n",
    );
    assert_eq!(printed, "1/9");
}

// ---------------------------------------------------------------------------
// D2: writing
// ---------------------------------------------------------------------------

/// **Writing is unchanged** (D2): `m[k] = v` inserts or replaces, and what it
/// inserts is a `V` — the read's question is not the write's.
#[test]
fn a_write_is_unchanged() {
    let rust = lowered(
        "use std::collections\n\
         fn main() {\n\
         \x20   let mut m = collections::HashMap()\n\
         \x20   m[\"a\"] = 1\n\
         }\n",
    );
    assert!(rust.contains("nikaia_std::index::set("), "{rust}");
    assert!(!rust.contains("Some(1)"), "{rust}");
}

/// **A compound assignment is written out** (D2, `NK1162`), because it reads
/// the slot as well as writing it and the read is a `T?` — so the line has to
/// say what an absent key counts as.
#[test]
fn a_compound_write_to_a_map_is_refused() {
    let found: Vec<_> = findings(
        "use std::collections\n\
         fn main() {\n\
         \x20   let mut counts = collections::HashMap()\n\
         \x20   counts[\"a\"] = 1\n\
         \x20   counts[\"a\"] += 1\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1162")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    let help = found[0].help.as_deref().unwrap_or_default();
    assert!(help.contains("?? 0"), "{help}");
}

/// **And the form the message hands over is a program that runs.**
#[test]
fn the_written_out_form_runs() {
    let printed = output(
        "map-counted",
        "use std::collections\n\
         fn main() {\n\
         \x20   let mut counts = collections::HashMap()\n\
         \x20   counts[\"a\"] = (counts[\"a\"] ?? 0) + 1\n\
         \x20   counts[\"a\"] = (counts[\"a\"] ?? 0) + 1\n\
         \x20   let n = counts[\"a\"] ?? 0\n\
         \x20   println(f\"{n}\")\n\
         }\n",
    );
    assert_eq!(printed, "2");
}

// ---------------------------------------------------------------------------
// D3: a list keeps its abort
// ---------------------------------------------------------------------------

/// **A list is untouched** (D3): `xs[i]` is a `T`, needing no `??` at all.
#[test]
fn a_list_read_is_a_value() {
    let printed = output(
        "list-read",
        "fn main() {\n\
         \x20   let xs = [10, 20, 30]\n\
         \x20   println(f\"{xs[1]}\")\n\
         \x20   let mut ys = [1, 2]\n\
         \x20   ys[0] = 5\n\
         \x20   println(f\"{ys[0] + ys[1]}\")\n\
         }\n",
    );
    assert_eq!(printed, "20\n7");
}

/// **And a compound assignment on a list is not `NK1162`'s**, because the read
/// is a `T` and there is no absent case to say anything about.
#[test]
fn a_compound_write_to_a_list_is_left_alone() {
    let found: Vec<_> = findings(
        "fn main() {\n\
         \x20   let mut xs = [1, 2, 3]\n\
         \x20   xs[0] += 1\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1162")
    .collect();
    assert!(found.is_empty(), "{found:#?}");
}

// ---------------------------------------------------------------------------
// `panic`, because it is D1's own way out
// ---------------------------------------------------------------------------

/// **`panic(…)` ends the program with the program's own words**, at the Nikaia
/// line ([Part III A.2](../../../docs/specification/30-nikaia-tooling.md),
/// [ADR-044](../../../docs/specification/adr/adr-044.md) D2). It was on
/// Part I 1.3's list and did not exist, so D1's own written way out lowered to
/// a call to nothing.
#[test]
fn panic_ends_the_program_at_the_nikaia_line() {
    let source = "use std::collections\n\
                  fn main() {\n\
                  \x20   let mut m = collections::HashMap()\n\
                  \x20   m[\"a\"] = 1\n\
                  \x20   let n = m[\"b\"] ?? panic(f\"b was a key a moment ago\")\n\
                  \x20   println(f\"{n}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    let dir = common::scratch_dir("map-panic");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    assert!(!ran.status.success(), "the program stops");
    let said = String::from_utf8_lossy(&ran.stderr).to_string();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(said.contains("was a key a moment ago"), "{said}");
    assert!(said.contains("main.rs") || said.contains(".nika"), "{said}");
}

/// **A map read off a value that came out of a `catch`** (0.0.230, `open-work.md`
/// §1.18). `examples/access-log/src/main.nika` without its `??` reached `rustc`: the
/// `catch` around the parse typed as nothing, so `report.paths[path]` was a
/// lookup on a map nobody knew, and the field read after it was not refused.
/// The `catch` is the guarded value's type where its handler leaves.
#[test]
fn a_map_read_off_a_caught_value_is_refused() {
    let found: Vec<_> = findings(
        "use std::collections\n\n\
         pub struct Counts { hits: i64 }\n\
         pub struct Report { paths: collections::HashMap[ref String, Counts] }\n\
         fn load() -> Report throws { return Report { paths: collections::HashMap() } }\n\
         fn main() {\n\
         \x20   let report = load() catch { return }\n\
         \x20   let counts = report.paths[\"a\"]\n\
         \x20   println(f\"{counts.hits}\")\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1125")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("`Counts?`"), "{found:#?}");
}

/// **A `?` whose inside has no type is not written `??`** (§1.18's second
/// fault): that is the operator, and the message said the operator may be
/// absent. It says the value may be missing.
#[test]
fn a_read_of_a_map_whose_values_are_unknown_names_no_operator() {
    let found: Vec<_> = findings(
        "use std::collections\n\n\
         pub struct Counts { hits: i64 }\n\
         fn main() {\n\
         \x20   let mut m = collections::HashMap()\n\
         \x20   m.insert(\"a\", Counts { hits: 1 })\n\
         \x20   let c = m[\"a\"]\n\
         \x20   println(f\"{c.hits}\")\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1125")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(!found[0].message.contains("`??`"), "{found:#?}");
    assert!(
        found[0].message.contains("This value may be missing"),
        "{found:#?}"
    );
}

/// **A map keyed by numbers can be read** (0.0.235, `open-work.md` §1.21).
///
/// Two ways it was not, both `rustc` about a file nobody wrote. A lookup
/// method's key went below by value, where Rust's `get`, `contains_key`,
/// `contains` and `remove` take it by reference - the checker now records the
/// `&` for exactly those `std` entries, and a key of no known type goes
/// through `index::AsKey`, which lends a number and passes a view through.
/// And on a map whose key type nothing pinned, `m[1] = 2` and `m[k]` went
/// through `index::at`, which is for a list position: the key is now handed
/// over as it is on a write and read through `index::key` - neither side is a
/// position on a map. A `String` key looked up is lent and not handed over,
/// so it is still there to use after.
#[test]
fn a_map_keyed_by_numbers_is_read() {
    let source = "use std::collections\n\
                  \n\
                  fn main() {\n\
                  \x20   let mut m = collections::HashMap()\n\
                  \x20   m[1] = 2\n\
                  \x20   m.insert(5, 6)\n\
                  \x20   let k = 1\n\
                  \x20   println(f\"{m[k] ?? 0} {m[5] ?? 0} {m[9] ?? -1} {m.get(k) ?? 0} {m.contains_key(k)} {m.len()}\")\n\
                  \x20   let mut t = collections::HashMap()\n\
                  \x20   t[\"a\"] = 10\n\
                  \x20   let name = \"a\"\n\
                  \x20   println(f\"{t[name] ?? 0} {t.get(name) ?? 0} {t.contains_key(name)}\")\n\
                  \x20   let mut owned: collections::HashMap[String, i64] = collections::HashMap()\n\
                  \x20   let who: String = \"b\"\n\
                  \x20   owned[who.clone()] = 3\n\
                  \x20   println(f\"{owned[who] ?? 0} {owned.get(who) ?? 0} {owned.contains_key(who)}\")\n\
                  \x20   let mut typed: collections::HashMap[i64, i64] = collections::HashMap()\n\
                  \x20   typed[4] = 40\n\
                  \x20   let four: i64 = 4\n\
                  \x20   println(f\"{typed[four] ?? 0} {typed.get(four) ?? 0} {typed.contains_key(four)}\")\n\
                  \x20   let mut seen = collections::HashSet()\n\
                  \x20   seen.insert(7)\n\
                  \x20   let seven = 7\n\
                  \x20   println(f\"{seen.contains(seven)} {seen.remove(seven)} {seen.len()}\")\n\
                  }\n";
    assert_eq!(
        output("number-keys", source),
        "2 6 -1 2 true 2\n10 10 true\n3 3 true\n40 40 true\ntrue true 0"
    );
}
