//! Owned keys, literals where text is kept, and data used after it was handed
//! over ([ADR-293](../../../docs/specification/adr/adr-293.md)).
//!
//! Each was `rustc`'s words about a file nobody wrote: a map keyed by an owned
//! `String` could not be written at all, a map keyed by an `i64` had its key
//! made a `usize`, `m[k] ?? "-"` over a map of text did not compile, a literal
//! assigned to a `String` was refused, and `xs.push(name)` followed by
//! `println(name)` was *borrow of moved value*. ADR-094 D2 decided that last one
//! is refused in this language's words, and nothing did.

mod common;

use nikaia::contracts::LedgerOps;
use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
}

fn ran(purpose: &str, source: &str, how: Build) -> String {
    let rust = lowered(source, how);
    let dir = common::scratch_dir(&format!("handed-{purpose}"));
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
    let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    std::fs::remove_dir_all(&dir).ok();
    printed
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

/// Both settings, the same output, and no refusal on the way.
fn runs(purpose: &str, source: &str, expected: &str) {
    assert!(
        findings(source).is_empty(),
        "{purpose}: {:#?}",
        findings(source)
    );
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran(purpose, source, how), expected, "{purpose} at {how:?}");
    }
}

// --- D1: a map whose keys are owned ------------------------------------------

/// **The wall this package began with**: a map keyed by an owned `String`,
/// written with a literal and with a name, read with a name that stays usable,
/// and a text fallback after `??` on a map of text.
#[test]
fn a_map_keyed_by_owned_text_is_written_and_read() {
    runs(
        "string-keys",
        "use std::collections\n\n\
         fn main() {\n\
         \x20   let mut m: collections::HashMap[String, i64] = collections::HashMap()\n\
         \x20   m[\"a\"] = 1\n\
         \x20   let name: String = \"b\"\n\
         \x20   m[name] = 2\n\
         \x20   let probe: String = \"a\"\n\
         \x20   let first = m[probe] ?? 0\n\
         \x20   let second = m[\"b\"] ?? 0\n\
         \x20   let third = m[probe.to_uppercase()] ?? 9\n\
         \x20   println(f\"{m.len()} {first} {second} {third} {probe}\")\n\
         }\n",
        "2 1 2 9 a",
    );
}

/// **A key is not a position**: a map keyed by an `i64` had its key turned into
/// a `usize` by the brackets. And a map of text, read with a literal after
/// `??`, is a view of text - nothing copied.
#[test]
fn a_map_keyed_by_numbers_holds_text() {
    runs(
        "int-keys",
        "use std::collections\n\n\
         fn main() {\n\
         \x20   let mut ids: collections::HashMap[i64, String] = collections::HashMap()\n\
         \x20   let k = 7\n\
         \x20   ids[k] = \"seven\"\n\
         \x20   ids[k + 1] = \"eight\"\n\
         \x20   let a = ids[7] ?? \"-\"\n\
         \x20   let b = ids[k + 1] ?? \"-\"\n\
         \x20   let c = ids[3] ?? \"-\"\n\
         \x20   println(f\"{a} {b} {c}\")\n\
         }\n",
        "seven eight -",
    );
}

/// **A map whose keys are only ever views is a map keyed by views**
/// ([ADR-282](../../../docs/specification/adr/adr-282.md) D12): the key
/// written `String` is a view below, so nothing is copied - where this used to
/// be refused with ADR-282's sentence asking for a `.clone()`.
#[test]
fn a_map_keyed_only_by_views_is_keyed_by_views() {
    runs(
        "view-keys",
        "use std::collections\n\n\
         fn count(text: ref String) -> i64 {\n\
         \x20   let mut m: collections::HashMap[String, i64] = collections::HashMap()\n\
         \x20   for w in text.split(\" \") {\n\
         \x20       m[w] = (m[w] ?? 0) + 1\n\
         \x20   }\n\
         \x20   return m[\"a\"] ?? 0\n\
         }\n\
         fn main() { println(count(\"a b a\")) }\n",
        "2",
    );
}

// --- D2: a literal assigned to text ----------------------------------------------

/// **An assignment keeps what it is given**, so a literal assigned to a
/// `String` is built into one there, as at every other place that keeps text.
#[test]
fn a_literal_assigned_to_text_is_built_there() {
    runs(
        "assign-literal",
        "fn main() {\n\
         \x20   let mut s: String = \"a\"\n\
         \x20   s = \"b\"\n\
         \x20   println(s)\n\
         }\n",
        "b",
    );
}

// --- D3: used after it was handed over -----------------------------------------

fn codes_of(found: &[nikaia::check::Finding]) -> Vec<&'static str> {
    found.iter().map(|f| f.code).collect()
}

fn one_refusal(source: &str, words: &str) {
    let found = findings(source);
    assert_eq!(codes_of(&found), ["NK2105"], "{found:#?}");
    assert!(found[0].message.contains(words), "{}", found[0].message);
}

const PERSON: &str = "struct Person {\n    name: String,\n}\n\n";

#[test]
fn a_value_pushed_is_given_away() {
    one_refusal(
        "fn main() {\n\
         \x20   let mut xs: Vec[String] = []\n\
         \x20   let name: String = \"b\"\n\
         \x20   xs.push(name)\n\
         \x20   println(name)\n\
         }\n",
        "`name` again, but it was passed to `push`, which keeps it",
    );
}

#[test]
fn a_key_written_is_given_away() {
    one_refusal(
        "use std::collections\n\n\
         fn main() {\n\
         \x20   let mut m: collections::HashMap[String, i64] = collections::HashMap()\n\
         \x20   let name: String = \"b\"\n\
         \x20   m[name] = 2\n\
         \x20   println(name)\n\
         }\n",
        "`name` again, but it was used as a map key",
    );
}

#[test]
fn a_field_holds_what_it_is_given() {
    one_refusal(
        &format!(
            "{PERSON}fn main() {{\n\
             \x20   let name: String = \"b\"\n\
             \x20   let p = Person {{ name: name }}\n\
             \x20   println(name)\n\
             }}\n"
        ),
        "`name` again, but it was stored in `Person.name`",
    );
    one_refusal(
        &format!(
            "{PERSON}fn main() {{\n\
             \x20   let name: String = \"b\"\n\
             \x20   let p = Person {{ name }}\n\
             \x20   println(name)\n\
             }}\n"
        ),
        "`name` again, but it was stored in `Person.name`",
    );
}

#[test]
fn a_rename_moves() {
    one_refusal(
        "fn main() {\n\
         \x20   let name: String = \"b\"\n\
         \x20   let t = name\n\
         \x20   println(name)\n\
         }\n",
        "bound to `t`",
    );
}

#[test]
fn a_loop_hands_it_over_again() {
    one_refusal(
        "fn main() {\n\
         \x20   let mut xs: Vec[String] = []\n\
         \x20   let name: String = \"b\"\n\
         \x20   for i in 0..<3 {\n\
         \x20       xs.push(name)\n\
         \x20   }\n\
         }\n",
        "inside a loop",
    );
}

/// **None of these is refused**: a copy handed over, data that is copied
/// anyway, two arms of one choice, a name given a value again, and
/// the `http` example's shape - hand over and leave, several times in a row.
#[test]
fn what_is_not_given_away_is_not_refused() {
    let found = findings(
        "fn stop(s: String) {\n\
         \x20   let mut xs: Vec[String] = []\n\
         \x20   xs.push(s)\n\
         }\n\n\
         fn serve(connection: String, a: i64) {\n\
         \x20   if a == 0 {\n\
         \x20       stop(connection)\n\
         \x20       return\n\
         \x20   }\n\
         \x20   if a == 1 {\n\
         \x20       stop(connection)\n\
         \x20       return\n\
         \x20   }\n\
         \x20   stop(connection)\n\
         }\n\n\
         fn main() {\n\
         \x20   let mut xs: Vec[String] = []\n\
         \x20   let name: String = \"b\"\n\
         \x20   xs.push(name.clone())\n\
         \x20   println(name)\n\
         \x20   let n = 3\n\
         \x20   let mut ys: Vec[i64] = []\n\
         \x20   ys.push(n)\n\
         \x20   println(f\"{n}\")\n\
         \x20   let c: String = \"c\"\n\
         \x20   if n > 2 {\n\
         \x20       xs.push(c)\n\
         \x20   } else {\n\
         \x20       println(c)\n\
         \x20   }\n\
         \x20   let mut again: String = \"d\"\n\
         \x20   xs.push(again)\n\
         \x20   again = \"e\"\n\
         \x20   println(again)\n\
         \x20   serve(\"x\".clone(), n)\n\
         }\n",
    );
    assert_eq!(codes_of(&found), Vec::<&str>::new(), "{found:#?}");
}

/// **And the programs that are not refused run**: the refusal is only worth
/// having if what it lets through is what the language below accepts.
#[test]
fn what_is_not_refused_runs() {
    runs(
        "not-refused",
        "fn stop(s: String) -> i64 {\n\
         \x20   let mut xs: Vec[String] = []\n\
         \x20   xs.push(s)\n\
         \x20   return xs.len()\n\
         }\n\n\
         fn serve(connection: String, a: i64) -> i64 {\n\
         \x20   if a == 0 {\n\
         \x20       return stop(connection)\n\
         \x20   }\n\
         \x20   return stop(connection) + 1\n\
         }\n\n\
         fn main() {\n\
         \x20   let mut xs: Vec[String] = []\n\
         \x20   let c: String = \"c\"\n\
         \x20   if xs.len() > 2 {\n\
         \x20       xs.push(c)\n\
         \x20   } else {\n\
         \x20       println(c)\n\
         \x20   }\n\
         \x20   let mut again: String = \"d\"\n\
         \x20   xs.push(again)\n\
         \x20   again = \"e\"\n\
         \x20   println(f\"{again} {xs.len()} {serve(\"x\".clone(), 1)}\")\n\
         }\n",
        "c\ne 1 2",
    );
}

// --- ADR-293: within one statement, parts of a value, and no stray warning ---

/// **Two hand-overs in one statement** are one after the other: the second
/// argument is read after the first was given away (ADR-293 D31).
#[test]
fn a_statement_that_hands_over_twice_is_refused() {
    one_refusal(
        "fn keep(a: String, b: String) -> i64 {\n\
         \x20   let mut xs: Vec[String] = []\n\
         \x20   xs.push(a)\n\
         \x20   xs.push(b)\n\
         \x20   return xs.len()\n\
         }\n\n\
         fn main() {\n\
         \x20   let name: String = \"n\"\n\
         \x20   println(f\"{keep(name, name)}\")\n\
         }\n",
        "`name` again, but it was passed to `keep`, which keeps it",
    );
}

/// **`name = f(name)` gives back what it took**, in the same statement.
#[test]
fn a_statement_that_takes_and_gives_back_is_a_program() {
    runs(
        "given-back",
        "fn main() {\n\
         \x20   let mut name: String = \"n\"\n\
         \x20   name = name + \"x\"\n\
         \x20   println(name)\n\
         }\n",
        "nx",
    );
}

/// **A part of an owned value is handed over, and the rest stays**: `p.x` is
/// still there after `p.name` went, `p.name` is not, and neither is `p` as a
/// whole (ADR-293 D32).
#[test]
fn a_part_handed_over_leaves_the_rest() {
    let head = "struct P {\n    name: String,\n    x: i64,\n}\n\n";
    runs(
        "part-rest",
        &format!(
            "{head}fn main() {{\n\
             \x20   let mut xs: Vec[String] = []\n\
             \x20   let p = P {{ name: \"a\", x: 1 }}\n\
             \x20   xs.push(p.name)\n\
             \x20   println(f\"{{p.x}} {{xs.len()}}\")\n\
             }}\n"
        ),
        "1 1",
    );
    one_refusal(
        &format!(
            "{head}fn main() {{\n\
             \x20   let mut xs: Vec[String] = []\n\
             \x20   let p = P {{ name: \"a\", x: 1 }}\n\
             \x20   xs.push(p.name)\n\
             \x20   println(p.name)\n\
             }}\n"
        ),
        "`p.name` again, but it was passed to `push`, which keeps it",
    );
    one_refusal(
        &format!(
            "{head}fn main() {{\n\
             \x20   let mut xs: Vec[String] = []\n\
             \x20   let p = P {{ name: \"a\", x: 1 }}\n\
             \x20   xs.push(p.name)\n\
             \x20   let q = p\n\
             }}\n"
        ),
        "You're using `p`, but `p.name` was passed to `push`",
    );
    // And a part assigned again is there again.
    runs(
        "part-given-back",
        &format!(
            "{head}fn main() {{\n\
             \x20   let mut xs: Vec[String] = []\n\
             \x20   let mut p = P {{ name: \"a\", x: 1 }}\n\
             \x20   xs.push(p.name)\n\
             \x20   p.name = \"b\"\n\
             \x20   println(p.name)\n\
             }}\n"
        ),
        "b",
    );
}

/// **A parameter whose part is handed over is kept**: it was lent (`&P`) and
/// the body took a field out of the loan, which `rustc` refused.
#[test]
fn a_parameter_whose_part_is_handed_over_is_kept() {
    runs(
        "part-of-parameter",
        "struct P {\n    name: String,\n    x: i64,\n}\n\n\
         fn names(p: P) -> i64 {\n\
         \x20   let mut xs: Vec[String] = []\n\
         \x20   xs.push(p.name)\n\
         \x20   return xs.len() + p.x\n\
         }\n\n\
         fn main() {\n\
         \x20   let p = P { name: \"z\", x: 5 }\n\
         \x20   println(f\"{names(p)}\")\n\
         }\n",
        "6",
    );
}

/// **`NK2106`: a part of something lent is not given away** - a `ref`
/// parameter and a `for` over a list alike.
#[test]
fn a_part_of_a_loan_is_not_handed_over() {
    for source in [
        "struct P {\n    name: String,\n}\n\n\
         fn f(p: ref P) {\n\
         \x20   let mut xs: Vec[String] = []\n\
         \x20   xs.push(p.name)\n\
         }\n\
         fn main() {}\n",
        "struct P {\n    name: String,\n}\n\n\
         fn f(ps: Vec[P]) {\n\
         \x20   let mut xs: Vec[String] = []\n\
         \x20   for p in ps {\n\
         \x20       xs.push(p.name)\n\
         \x20   }\n\
         }\n\
         fn main() {}\n",
    ] {
        let found = findings(source);
        assert_eq!(codes_of(&found), ["NK2106"], "{found:#?}");
        assert!(
            found[0]
                .help
                .as_deref()
                .is_some_and(|h| h.contains("p.name.clone()")),
            "{:?}",
            found[0].help
        );
    }
}

/// **The part is named once**: an argument of a `std` call is walked more than
/// once, and each walk extended the read, so the message said `self.body.body`
/// and its help wrote a copy of a field that does not exist. The data is what
/// `fs::write` keeps; the file's name it only reads (ADR-319 D2).
#[test]
fn a_part_handed_to_std_is_named_once() {
    let source = "use std::fs\n\n\
                  struct Log {\n    path: String,\n    body: String,\n}\n\n\
                  impl Log {\n\
                  \x20   fn save(ref self) throws {\n\
                  \x20       fs::write(self.path, fs::Root::Anywhere, self.body)\n\
                  \x20   }\n\
                  }\n\
                  fn main() {}\n";
    let found = findings(source);
    let part: Vec<_> = found.iter().filter(|f| f.code == "NK2106").collect();
    assert_eq!(part.len(), 1, "{found:#?}");
    assert!(
        part[0]
            .message
            .starts_with("`self.body` is passed to `fs::write`, which keeps it"),
        "{found:#?}"
    );
    assert_eq!(
        part[0].help.as_deref(),
        Some("Hand over a copy: `self.body.clone()`.")
    );
}

/// **A read through the brackets carries no parentheses of its own**: they
/// stood around every read, and `m[k] ?? 0` and `let x = xs[1]` were
/// `rustc`'s *unnecessary parentheses* about a file nobody wrote (ADR-293 D33).
/// Where a postfix follows, they are still there, because there they are
/// needed.
#[test]
fn a_read_through_the_brackets_warns_about_nothing() {
    let source = "use std::collections\n\n\
                  fn main() {\n\
                  \x20   let mut m: collections::HashMap[String, i64] = collections::HashMap()\n\
                  \x20   m[\"a\"] = 3\n\
                  \x20   let a = m[\"a\"] ?? 0\n\
                  \x20   let xs = [1, 2, 3]\n\
                  \x20   let v = xs[1]\n\
                  \x20   let w = xs[2] * 2 + xs[0]\n\
                  \x20   let f = xs[2] as f64\n\
                  \x20   let neg = -xs[0]\n\
                  \x20   let words = [\"ab\", \"c\"]\n\
                  \x20   let n = words[0].len()\n\
                  \x20   println(f\"{a} {v} {w} {f} {neg} {n}\")\n\
                  }\n";
    let rust = lowered(source, Build::default());
    let dir = common::scratch_dir("handed-parentheses");
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
    let said = String::from_utf8_lossy(&compiled.stderr);
    assert!(compiled.status.success(), "{said}\n{rust}");
    assert!(!said.contains("unnecessary parentheses"), "{said}\n{rust}");
    let out = Command::new(&binary).output().expect("run it");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "3 2 7 3 -1 2");
    std::fs::remove_dir_all(&dir).ok();
}

// --- ADR-293: the rest of ADR-293 --------------------------------------------

/// **`insert` is the write under the language below's name**, handing back
/// what it replaced; a literal key is built into the map's own text.
#[test]
fn insert_writes_and_hands_back_what_it_replaced() {
    runs(
        "insert",
        "use std::collections\n\n\
         fn main() {\n\
         \x20   let mut m: collections::HashMap[String, i64] = collections::HashMap()\n\
         \x20   m.insert(\"a\", 1)\n\
         \x20   let before = m.insert(\"a\", 2) ?? 0\n\
         \x20   let now = m[\"a\"] ?? 0\n\
         \x20   println(f\"{before} {now} {m.len()}\")\n\
         }\n",
        "1 2 1",
    );
    one_refusal(
        "use std::collections\n\n\
         fn main() {\n\
         \x20   let mut m: collections::HashMap[String, i64] = collections::HashMap()\n\
         \x20   let k: String = \"a\"\n\
         \x20   m.insert(k, 1)\n\
         \x20   println(k)\n\
         }\n",
        "`k` again, but it was passed to `insert`, which keeps it",
    );
}

/// **A copy is a copy of a view too**: a `String` the body only reads is a
/// `&str` below, and `.clone()` of that was the reference.
#[test]
fn a_copy_of_a_view_is_text_of_its_own() {
    runs(
        "clone-view",
        "fn copy(name: String) -> String {\n\
         \x20   return name.clone()\n\
         }\n\n\
         fn main() {\n\
         \x20   let c = copy(\"x\")\n\
         \x20   let ys: Vec[i64] = [1, 2]\n\
         \x20   let zs = ys.clone()\n\
         \x20   println(f\"{c} {zs.len()} {ys.len()}\")\n\
         }\n",
        "x 2 2",
    );
}

// --- ADR-282: one word for a copy ------------------------------------------------

/// **`.to_owned()` is refused, naming `.clone()`**: the language below needs
/// the second word because its `.clone()` of a reference copies the
/// reference; this one has no reference to copy.
#[test]
fn a_copy_has_one_word() {
    let found = findings(
        "fn main() {\n\
         \x20   let name: String = \"a\"\n\
         \x20   let copy = name.to_owned()\n\
         \x20   println(copy)\n\
         }\n",
    );
    assert_eq!(codes_of(&found), ["NK1189"], "{found:#?}");
    assert!(
        found[0]
            .help
            .as_deref()
            .is_some_and(|h| h.contains("name.clone()")),
        "{:?}",
        found[0].help
    );
    // `.to_string()` is the text form of a value, for every type, and stays.
    assert_eq!(
        codes_of(&findings(
            "fn main() {\n\
             \x20   let n = 3\n\
             \x20   let a = n.to_string()\n\
             \x20   let b = \"x\"\n\
             \x20   println(f\"{a}{b}\")\n\
             }\n"
        )),
        Vec::<&str>::new()
    );
}

/// **A copy of text is text of its own and a copy of a slice is a list**, each
/// written below as what makes one (ADR-282 D9).
#[test]
fn a_copy_is_owned_whatever_it_copied() {
    runs(
        "copies",
        "fn first(xs: ref Array[i64]) -> Vec[i64] {\n\
         \x20   return xs.clone()\n\
         }\n\n\
         fn shout(name: ref String) -> String {\n\
         \x20   let copy = name.clone()\n\
         \x20   return copy + \"!\"\n\
         }\n\n\
         fn main() {\n\
         \x20   let xs = [1, 2, 3]\n\
         \x20   let part = ref xs[0..<2]\n\
         \x20   let owned = first(part)\n\
         \x20   println(f\"{owned.len()} {shout(\"hey\")}\")\n\
         }\n",
        "2 hey!",
    );
}

/// **The text form of text is the text itself** (ADR-282 D8): a view stays a
/// view and a literal a literal, so nothing is copied and nothing is written
/// below - and a view put where text of its own is kept is refused naming
/// `.clone()`, as it is without the `.to_string()`.
#[test]
fn the_text_form_of_text_is_the_text() {
    let rust = lowered(
        "fn main() {\n\
         \x20   let a = \"x\".to_string()\n\
         \x20   let n = 3\n\
         \x20   println(f\"{a}{n.to_string()}\")\n\
         }\n",
        Build::default(),
    );
    assert!(rust.contains("let a = \"x\";"), "{rust}");
    assert!(rust.contains("n.to_string()"), "{rust}");
    let refused = findings(
        "struct P { name: String }\n\n\
         fn main() {\n\
         \x20   let v = \"x\"\n\
         \x20   let p = P { name: v.to_string() }\n\
         \x20   println(p.name)\n\
         }\n",
    );
    assert_eq!(refused.len(), 1, "{refused:#?}");
    assert!(
        refused[0]
            .help
            .as_deref()
            .unwrap_or_default()
            .contains(".clone()"),
        "{refused:#?}"
    );
}

/// **A literal is enough wherever text of its own is kept** (ADR-282 D3): in a
/// tuple, in every link of an `else if` chain, after a `??` inside an f-string
/// hole - the places a program used to write `.to_string()` to get a `String`.
#[test]
fn a_bare_literal_is_text_of_its_own_where_it_is_kept() {
    runs(
        "bare-literals",
        "struct U { name: String }\n\n\
         impl U {\n\
         \x20   fn copy(ref self) -> String? { return null }\n\
         \x20   fn rest(ref self) -> ref String? { return self.name.strip_prefix(\"A\") }\n\
         }\n\n\
         fn pair() -> (i64, String) {\n\
         \x20   return (1, \"one\")\n\
         }\n\n\
         fn grade(score: i64) -> String {\n\
         \x20   let g: String = if score >= 90 { \"A\" } else if score >= 80 { \"B\" } else { \"F\" }\n\
         \x20   return g\n\
         }\n\n\
         fn main() {\n\
         \x20   let u = U { name: \"Ada\" }\n\
         \x20   let (n, word) = pair()\n\
         \x20   println(f\"{n} {word} {grade(85)}\")\n\
         \x20   println(f\"{u.copy() ?? \"none\"}\")\n\
         \x20   println(f\"{u.rest() ?? \"n/a\"}\")\n\
         }\n",
        "1 one B\nnone\nda",
    );
}

/// **A name bound again is a new binding** (0.0.230). Two loops that each bind
/// `more` and hand it to something that keeps it are two names to the reader,
/// and were one to the walk that finds a read after a hand-over: the second
/// loop's `more.len()` was taken for a read of the first loop's `more`. It was
/// found when a `catch` began to have a type, so `examples/http`'s
/// `connection.read() catch { return }` was data at last. A `let` gives the
/// name a value again, as an assignment does; a real read after a hand-over is
/// still refused.
#[test]
fn a_name_bound_again_is_not_the_one_handed_over() {
    let source = |tail: &str| {
        format!(
            "fn next() -> Vec[i64] throws {{ return [1, 2] }}\n\
             fn main() {{\n\
             \x20   let mut all = Vec()\n\
             \x20   while all.len() < 4 {{\n\
             \x20       let more = next() catch {{ return }}\n\
             \x20       all.push(more)\n\
             \x20   }}\n\
             \x20   while all.len() < 8 {{\n\
             \x20       let more = next() catch {{ return }}\n\
             \x20       if more.len() == 0 {{ return }}\n\
             \x20       all.push(more)\n\
             \x20   }}\n\
             {tail}\
             \x20   println(f\"{{all.len()}}\")\n\
             }}\n"
        )
    };
    let refused = |source: String| -> Vec<String> {
        findings(&source)
            .into_iter()
            .filter(|f| f.code == "NK2105")
            .map(|f| f.message)
            .collect()
    };
    assert!(refused(source("")).is_empty(), "{:?}", refused(source("")));
    runs("rebound-in-two-loops", &source(""), "8");

    let reused = refused(source(
        "    let kept = next() catch { return }\n    all.push(kept)\n    println(f\"{kept.len()}\")\n",
    ));
    assert_eq!(reused.len(), 1, "{reused:?}");
    assert!(reused[0].contains("`kept`"), "{reused:?}");
}

/// **A `let mut` of a literal that the body grows holds text of its own**
/// (0.0.232). `let mut s = "a"` then `s = s + "b"` was `NK1105` with a help to
/// take a view of the sum; found writing a miniature compiler in Nikaia, where
/// emitted code is built up this way. A literal is built into text of its own
/// wherever it is kept (ADR-282 D4), and a binding the program appends to
/// keeps it. The growing is read off `+`, `+=` and an f-string, which can only
/// be owned text.
#[test]
fn a_literal_binding_the_body_grows_is_text_of_its_own() {
    let source = "fn width(s: ref String) -> i64 { return s.len() as i64 }\n\
                  fn main() {\n\
                  \x20   let mut out = \"fn main() {\\n\"\n\
                  \x20   let before = width(out)\n\
                  \x20   out = out + \"    let a = 1;\\n\"\n\
                  \x20   out += \"    let b = 2;\\n\"\n\
                  \x20   out = f\"{out}}}\"\n\
                  \x20   println(f\"{before} {width(out)}\")\n\
                  \x20   println(out)\n\
                  }\n";
    runs(
        "literal-grown",
        source,
        "12 43\nfn main() {\n    let a = 1;\n    let b = 2;\n}",
    );

    // **A literal assigned is still a view**, and so is a name that may be
    // one: the best of a list of views is the ordinary shape, and reading it
    // as owned text would refuse it.
    let views = "fn main() {\n\
                 \x20   let words = [\"a\", \"bbb\", \"cc\"]\n\
                 \x20   let mut best = \"\"\n\
                 \x20   for w in words { if w.len() > best.len() { best = w } }\n\
                 \x20   best = \"-\"\n\
                 \x20   println(best)\n\
                 }\n";
    runs("literal-views", views, "-");
}

/// **A bare name of owned text assigned to a literal's binding** is still
/// refused, because a name may be a view, and the help says the one thing that
/// works rather than *write `ref` in front of it*.
#[test]
fn owned_text_into_a_literal_binding_names_the_annotation() {
    let source = "fn main() {\n\
                  \x20   let mut s = \"a\"\n\
                  \x20   let t: String = \"q\"\n\
                  \x20   s = t\n\
                  \x20   println(s)\n\
                  }\n";
    let found = findings(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1105");
    assert_eq!(
        found[0].help.as_deref(),
        Some("Declare it as text of its own: `let mut s: String = \"a\"`.")
    );
}
