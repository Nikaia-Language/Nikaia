//! Reading a map through the brackets is a `T?`
//! ([ADR-293](../../../docs/specification/adr/adr-293.md)).
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
//!
//! What the reads answer while a program runs is
//! `tests/language/src/map_reads.nika`; this file keeps what is refused, what
//! is lowered, and the `panic` that ends a program.

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

// ---------------------------------------------------------------------------
// D1: the read
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// D3: a list keeps its abort
// ---------------------------------------------------------------------------

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
/// [ADR-300](../../../docs/specification/adr/adr-300.md) D10). It was on
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

/// **A map read off a value that came out of a `catch`** (0.0.230, issue #154
/// issue #154). `examples/access-log/src/main.nika` without its `??` reached `rustc`: the
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
    // A view of what the map keeps since #297: `ref Counts?`.
    assert!(found[0].message.contains("Counts?`"), "{found:#?}");
}

/// **A `?` whose inside has no type is not written `??`** (issue #154's second
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

// ---------------------------------------------------------------------------
// ADR-293: a map of `T?` values
// ---------------------------------------------------------------------------

/// **A map of `T?` reads one `T?`** ([ADR-293](../../../docs/specification/adr/adr-293.md)
/// D1): a stored `null` and an absent key both answer `null`, through the
/// brackets and through `get`, and `contains_key` tells them apart. **`m[k] =
/// null` stores** (D2): the key is there afterwards and `len` counts it.
#[test]
fn a_map_of_maybes_reads_one_maybe() {
    let source = "\
use std::collections
enum Kind { A, B }
struct User { name: String }
fn label(k: ref Kind?) -> String {
    let known = k ?? Kind::B
    return match known {
        Kind::A => \"a\",
        Kind::B => \"b\",
    }
}
fn main() {
    let mut m: collections::BTreeMap[i64, Kind?] = collections::BTreeMap()
    m[1] = Kind::A
    m[2] = null
    m.insert(3, Kind::B)
    let present = m[1]
    let stored = m[2]
    let absent = m[9]
    println(f\"{present == null} {stored == null} {absent == null}\")
    println(f\"{label(m[1])} {label(m[2])} {label(m[9])} {label(m.get(3))}\")
    println(f\"{m.contains_key(2)} {m.contains_key(9)} {m.len()}\")
    let mut users: collections::HashMap[String, User?] = collections::HashMap()
    users[\"ann\"] = User { name: \"Ann\" }
    users[\"bob\"] = null
    println(f\"{users[\"ann\"]?.name ?? \"-\"} {users[\"bob\"]?.name ?? \"-\"} {users[\"cy\"]?.name ?? \"-\"}\")
    let mut counts: collections::BTreeMap[String, i64?] = collections::BTreeMap()
    counts[\"x\"] = 3
    counts[\"y\"] = null
    println(f\"{counts[\"x\"] ?? 0} {counts[\"y\"] ?? 0} {counts[\"z\"] ?? 0}\")
}
";
    let rust = lowered(source);
    assert!(rust.contains("nikaia_std::index::flat("), "{rust}");
}

/// **A map's read kept where a `T?` of its own is wanted** (D4): a value that
/// copies is copied out, from a map of `T` and from a map of `T?` alike; one
/// that does not is refused with the copy to write, `?.clone()`. What the
/// copies and the written-out clone hold is `map_reads.nika`'s.
#[test]
fn a_map_read_kept_is_copied_out_or_refused() {
    let refused = "\
use std::collections
struct User { name: String }
fn main() {
    let mut users: collections::BTreeMap[i64, User?] = collections::BTreeMap()
    users[1] = User { name: \"Ann\" }
    let u: User? = users[1]
    println(f\"{u == null}\")
}
";
    let found = findings(refused);
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK1102"
                && f.help.as_deref().is_some_and(|h| h.contains("?.clone()"))),
        "{found:#?}"
    );
}

/// **A name of the map's value type after `??` is lent** (ADR-279 D7):
/// `m[k] ?? spare` over a map of lists is a view whichever side answers, as
/// `r ?? spare` is over a `ref T?`. It was `NK1185`, which sent a program to
/// copy the map's list to read it (found moving `describe`'s draft into
/// Nikaia, #125).
#[test]
fn a_named_fallback_beside_a_map_read_is_lent() {
    let source = "\
use std::collections
fn count(items: ref Vec[String]) -> i64 {
    return items.len()
}
fn main() {
    let mut m: collections::BTreeMap[String, Vec[String]] = collections::BTreeMap()
    m[\"a\"] = [\"x\", \"y\"]
    let none: Vec[String] = []
    let keys: Vec[String] = [\"a\", \"b\"]
    for key in keys {
        let items = m[key] ?? none
        println(f\"{key} {count(items)} {count(m[key] ?? none)}\")
    }
}
";
    assert!(findings(source).is_empty(), "{:?}", findings(source));
    let lowered = lowered(source);
    assert!(lowered.contains("|| &none"), "{lowered}");
    assert!(!lowered.contains("clone()"), "nothing is copied: {lowered}");
}

/// …and one that does not copy is refused with the copy to write, as a `T?`
/// kept of its own is.
#[test]
fn a_read_past_a_jump_that_does_not_copy_is_refused() {
    let source = "\
use std::collections
struct Named {
    name: String,
}
fn pick(all: ref collections::BTreeMap[String, Named], key: ref String) -> i64 {
    let mut chosen = Named { name: \"\" }
    chosen = all[key] ?? return -1
    return chosen.name.len()
}
fn main() {
    let all: collections::BTreeMap[String, Named] = collections::BTreeMap()
    println(f\"{pick(all, \"a\")}\")
}
";
    let found = findings(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1102");
    assert!(
        found[0].message.contains("a map's read is a view"),
        "{found:#?}"
    );
}
