//! **`&[T]`, a view of a run of elements** —
//! [ADR-179](../../../docs/specification/adr/adr-179.md).
//!
//! [ADR-311](../../../docs/specification/adr/adr-311.md) D1 said a build-time
//! result arrives in its **view** form — `Vec[T]` as `&[T]`, `String` as
//! `&str` — and half of it was built: text crossed as a `&str`, a list crossed
//! as an `Array[T, N]` because that is what the type language could spell, and
//! `&[T]` stayed unspellable. Which was fine until the length was not one
//! number: a **field** whose run differs per value has no `Array` to be.
//!
//! **These tests run programs**, because what a view points at is not a
//! question a string comparison answers. A `const` compared against text would
//! pass for a lowering that wrote the right characters and pointed them at a
//! temporary.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings
}

fn lowered(source: &str) -> String {
    let found = findings(source);
    assert!(found.is_empty(), "a correct program: {found:#?}");
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Compile and run, hand back what it printed.
fn ran(purpose: &str, source: &str) -> String {
    let rust = lowered(source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering of {purpose} does not compile:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr),
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("the program runs");
    let printed = String::from_utf8_lossy(&ran.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    printed
}

/// **The whole of D1 in one program**: a run of `struct`s a build computed, a
/// byte buffer, and every way a program reads one.
///
/// `ROWS` is the shape that could not cross before this: two `Row`s whose
/// count is not in the type, so `Array[Row, N]` would have had to name a
/// number the declaration has no business knowing.
#[test]
fn a_run_crosses_and_the_program_reads_it() {
    let source = "struct Row { a: i64, name: ref String }\n\
                  \n\
                  comptime ROWS: ref Array[Row] = [Row { a: 1, name: \"one\" }, Row { a: 2, name: \"two\" }]\n\
                  comptime NS: ref Array[i64] = [10, 20, 30]\n\
                  comptime MAGIC: ref Array[u8] = [0x7F, 0x45, 0x4C, 0x46]\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{ROWS.len()} {NS.len()} {MAGIC.len()}\")\n\
                  \x20   println(f\"{NS[1]}\")\n\
                  \x20   println(f\"{ROWS[0].name}\")\n\
                  \x20   for r in ROWS { println(f\"{r.a}={r.name}\") }\n\
                  }\n";
    let rust = lowered(source);
    // **The `&` is written where the declaration says slice**, and the array
    // literal behind it is promoted to `'static` by the `const` itself.
    assert!(rust.contains("const NS: &[i64] = &[10, 20, 30];"), "{rust}");
    assert!(
        rust.contains("const MAGIC: &[u8] = &[127, 69, 76, 70];"),
        "{rust}"
    );
    assert!(
        rust.contains(
            "const ROWS: &[Row] = &[Row { a: 1, name: \"one\" }, Row { a: 2, name: \"two\" }];"
        ),
        "{rust}"
    );

    assert_eq!(
        ran("a run that crosses", source),
        "2 3 4\n20\none\n1=one\n2=two\n"
    );
}

/// **A `struct` field may be one**, which is the whole reason this type exists.
///
/// `Array[Setting, N]` cannot type a field whose run is a different length per
/// value, and a `Vec` owns memory a `const` cannot hold. This is the shape
/// both corpus grammars produce
/// ([ADR-177](../../../docs/specification/adr/adr-177.md) §5's measurement).
#[test]
fn a_field_may_view_a_run() {
    let source = "struct Setting { key: ref String, value: ref String }\n\
                  struct Section { name: ref String, settings: ref Array[Setting] }\n\
                  \n\
                  comptime MAIN: Section = Section {\n\
                  \x20   name: \"main\",\n\
                  \x20   settings: [Setting { key: \"host\", value: \"h\" }, Setting { key: \"port\", value: \"8080\" }],\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{MAIN.name} {MAIN.settings.len()}\")\n\
                  \x20   for s in MAIN.settings { println(f\"{s.key}={s.value}\") }\n\
                  }\n";
    let rust = lowered(source);
    assert!(rust.contains("settings: &[Setting {"), "{rust}");
    assert_eq!(
        ran("a field that views a run", source),
        "main 2\nhost=h\nport=8080\n"
    );
}

/// **A `Vec` is lent to a parameter and the caller writes nothing**
/// ([ADR-094](../../../docs/specification/adr/adr-094.md) D1, the rule `&str`
/// already had): the callee reads and the caller keeps.
#[test]
fn a_run_is_lent_to_a_parameter() {
    let source = "fn total(xs: ref Array[i64]) -> i64 {\n\
                  \x20   let mut sum = 0\n\
                  \x20   for n in xs { sum = sum + n }\n\
                  \x20   return sum\n\
                  }\n\
                  \n\
                  comptime NS: ref Array[i64] = [1, 2, 3]\n\
                  \n\
                  fn main() {\n\
                  \x20   let mut built: Vec[i64] = []\n\
                  \x20   built.push(10)\n\
                  \x20   built.push(20)\n\
                  \x20   println(f\"{total(built)} {total(NS)}\")\n\
                  }\n";
    assert_eq!(ran("a run lent to a parameter", source), "30 6\n");
}

/// **`NK1179`: a run this body owns, in a field that views one.**
///
/// The fit holds — a `Vec[u8]` *lends* a run of `u8`, which is the C
/// boundary's rule, where the call lends for its own duration — and a struct
/// **outlives the expression that fills it**, so the same fit here is a view of
/// something already gone. Without this `rustc` answered *expected `&[i64]`,
/// found `Vec<i64>`* about the generated file
/// ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
#[test]
fn a_run_this_body_owns_is_refused_by_name() {
    let found = findings(
        "struct Bag { items: ref Array[i64] }\n\
         \n\
         fn main() {\n\
         \x20   let mut v: Vec[i64] = []\n\
         \x20   v.push(1)\n\
         \x20   let b = Bag { items: v }\n\
         \x20   println(f\"{b.items.len()}\")\n\
         }\n",
    );
    let refused: Vec<_> = found.iter().filter(|f| f.code == "NK1179").collect();
    assert_eq!(refused.len(), 1, "{found:#?}");
    assert!(
        refused[0].message.contains("`Bag.items` only views a list"),
        "{:#?}",
        refused[0]
    );
    // **The way out names the parameter case**, because the two lines look the
    // same and only one of them needs a word.
    assert!(
        refused[0].notes[1].contains("Passing a list to a parameter lends it automatically"),
        "{:#?}",
        refused[0].notes
    );
    assert!(
        refused[0]
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("Vec[i64]"),
        "{:#?}",
        refused[0]
    );
}

/// …and **inside a grammar action the sentence is a different one**
/// ([ADR-179](../../../docs/specification/adr/adr-179.md) D4).
///
/// The message may not print a `?` the reader would have to write ([Part III
/// C.2](../../../docs/specification/30-nikaia-tooling.md): *a way out that
/// cannot be taken is not one*). Since a rule's binding has its type (#500),
/// `settings:setting*` is a `Vec[Setting]`, and that is what the sentence and
/// the way out name.
#[test]
fn an_action_that_fills_a_view_names_the_vec() {
    let found = findings(
        "pub struct Setting { key: ref String, value: ref String }\n\
         \n\
         pub struct Section { name: ref String, settings: ref Array[Setting] }\n\
         \n\
         grammar Cfg {\n\
         \x20   rule WS = multispace0 { }\n\
         \x20   rule NAME -> ref String = s:raw_ident { s }\n\
         \x20   rule setting -> Setting = key:NAME \"=\" value:NAME { Setting { key, value } }\n\
         \x20   entry rule section -> Section =\n\
         \x20       \"[\" name:NAME \"]\" settings:setting* { Section { name, settings } }\n\
         }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    let refused: Vec<_> = found.iter().filter(|f| f.code == "NK1179").collect();
    assert_eq!(refused.len(), 1, "{found:#?}");
    assert!(
        refused[0]
            .message
            .contains("a `Vec[Setting]` this body owns")
            && !refused[0].message.contains('?'),
        "no `?` in the sentence: {:#?}",
        refused[0]
    );
    assert!(
        refused[0]
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("`Vec[Setting]`"),
        "{:#?}",
        refused[0]
    );
}

/// **A cut with an open end** (Part I 2.6, #527): `[..<b]` and `[..b]` start
/// at `0`, `[a..]` runs to the end - on text after `find`, and on a list.
#[test]
fn a_cut_with_an_open_end() {
    let source = "fn main() {\n\
         \x20   let line = \"Hamburg;12.0\"\n\
         \x20   let sep = line.find(\";\") ?? return\n\
         \x20   let name = line[..<sep]\n\
         \x20   let rest = line[sep + 1..]\n\
         \x20   let head = line[..2]\n\
         \x20   let xs = [1, 2, 3, 4]\n\
         \x20   let tail = xs[1..]\n\
         \x20   println(f\"{name} {rest} {head} {tail.len()}\")\n\
         }\n";
    assert_eq!(ran("open-cuts", source), "Hamburg 12.0 Ham 3\n");
    let rust = lowered(source);
    assert!(rust.contains("at(sep + 1..)"), "{rust}");
}
