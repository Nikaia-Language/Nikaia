//! Owned keys, literals where text is kept, and data used after it was handed
//! over ([ADR-293](../../../docs/specification/adr/adr-293.md)).
//!
//! Each was `rustc`'s words about a file nobody wrote: a map keyed by an owned
//! `String` could not be written at all, a map keyed by an `i64` had its key
//! made a `usize`, `m[k] ?? "-"` over a map of text did not compile, a literal
//! assigned to a `String` was refused, and `xs.push(name)` followed by
//! `println(name)` was *borrow of moved value*. ADR-094 D2 decided that last one
//! is refused in this language's words, and nothing did.
//!
//! What the programs that are not refused compute is
//! `tests/language/src/handed_over.nika`; this file keeps the refusals and what
//! the lowering writes.

mod common;

use nikaia::contracts::LedgerOps;
use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
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

/// **A part of an owned value is handed over**: `p.name` is not there after it
/// went, and neither is `p` as a whole (ADR-293 D32). That the rest stays is
/// `tests/language/src/handed_over.nika`.
#[test]
fn a_part_handed_over_is_gone() {
    let head = "struct P {\n    name: String,\n    x: i64,\n}\n\n";
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

/// **`insert` keeps its key**, as the brackets do: what it writes and hands back
/// is `tests/language/src/handed_over.nika`.
#[test]
fn insert_keeps_its_key() {
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

    let reused = refused(source(
        "    let kept = next() catch { return }\n    all.push(kept)\n    println(f\"{kept.len()}\")\n",
    ));
    assert_eq!(reused.len(), 1, "{reused:?}");
    assert!(reused[0].contains("`kept`"), "{reused:?}");
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

/// **What is printed is only read** (ADR-279 D5, #473): a print call takes
/// any value by `?`, which used to count as kept, so the second print of the
/// same `String` was `NK2105`.
#[test]
fn a_printed_value_is_only_read() {
    let found = findings(
        "fn main() {\n    let s: String = \"a\".clone()\n    print(s)\n    println(s)\n    eprintln(s)\n}\n",
    );
    assert!(found.iter().all(|f| f.code != "NK2105"), "{found:#?}");
}
