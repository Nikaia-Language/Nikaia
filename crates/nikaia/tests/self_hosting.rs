//! **How far a compiler written in Nikaia gets**, kept as a test.
//!
//! A miniature compiler - a grammar that parses `let` lines of arithmetic, a
//! syntax tree that holds itself, a check for names nothing declares, and Rust
//! written out as text - was written at 0.0.231 to measure what stood between
//! this language and a compiler written in it. It needed three workarounds
//! then: its tree held its children in a list, its output text had to be
//! annotated, and it read a number literal with a loop over `digit_value`.
//! Each was a gap, and each is closed: a type holds itself (ADR-246), a `let mut`
//! of a literal the body grows owns its text (0.0.232), and `std` reads a number
//! (`text::parse_i64`, 0.0.237).
//!
//! **What this is for is the next gap.** The program below is written the way
//! a compiler would be, without a workaround in it; a change that makes it stop
//! building or print something else is a step back on that road, and a new
//! construct a compiler needs goes in here first.

mod common;

use std::process::Command;

use nikaia::contracts::{Ledger, LedgerOps, STD};
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

fn ran(purpose: &str, source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, how).expect("it lowers").rust;
    let dir = common::scratch_dir(&format!("self-hosting-{purpose}"));
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
    assert!(
        out.status.success(),
        "{purpose} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn runs(purpose: &str, source: &str, expected: &str) {
    let found = findings(source);
    assert!(found.is_empty(), "{purpose}: {found:#?}");
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran(purpose, source, how), expected, "{purpose} at {how:?}");
    }
}

/// **`text::parse_i64` is `atoi`** (0.0.237), written in Nikaia in
/// `std/text.nika`: an optional sign and digits, and nothing where the text
/// writes no `i64` - both ends of the range included, one past either end not.
#[test]
fn text_reads_a_whole_number() {
    let source = "use std::text\n\
                  \n\
                  fn main() {\n\
                  \x20   for s in [\"42\", \"-7\", \"+3\", \"0\", \"\", \"-\", \"12a\", \" 1\", \"9223372036854775807\", \"-9223372036854775808\", \"9223372036854775808\", \"-9223372036854775809\"] {\n\
                  \x20       let found = text::parse_i64(s)\n\
                  \x20       let shown = if found == null { \"null\" } else { f\"{found ?? 0}\" }\n\
                  \x20       println(f\"[{s}] {shown}\")\n\
                  \x20   }\n\
                  \x20   let sum = (text::parse_i64(\"20\") ?? 0) + (text::parse_i64(\"x\") ?? 1)\n\
                  \x20   println(f\"{sum}\")\n\
                  }\n";
    runs(
        "parse-i64",
        source,
        "[42] 42\n[-7] -7\n[+3] 3\n[0] 0\n[] null\n[-] null\n[12a] null\n[ 1] null\n\
         [9223372036854775807] 9223372036854775807\n\
         [-9223372036854775808] -9223372036854775808\n\
         [9223372036854775808] null\n[-9223372036854775809] null\n21\n",
    );
}

/// **The miniature compiler**, with no workaround left in it: it parses, finds
/// the name nothing declares, and writes the Rust.
#[test]
fn the_miniature_compiler_builds_and_runs() {
    let source = r#"use std::collections
use std::text

enum Expr {
    Num(i64),
    Name(String),
    Add(Expr, Expr),
    Mul(Expr, Expr),
}

struct Let {
    name: String,
    value: Expr,
}

grammar Mini {
    rule WS = (" " | "\t")* { }
    rule NL = ("\n" | " " | "\t")* { }
    rule NAME -> String = s:raw_ident { s.to_string() }
    rule NUM -> i64 = d:digit+ { text::parse_i64(d) ?? 0 }
    rule atom -> Expr = WS n:NUM WS { Expr::Num(n) } | WS s:NAME WS { Expr::Name(s) }
    rule term -> Expr = a:atom "*" b:term { Expr::Mul(a, b) } | a:atom { a }
    rule expr -> Expr = a:term "+" b:expr { Expr::Add(a, b) } | a:term { a }
    rule line -> Let = NL "let" WS name:NAME WS "=" value:expr { Let { name, value } }
    pub rule program -> Vec[Let] = lines:line* NL { lines }
}

fn emit(e: ref Expr) -> String {
    return match e {
        Expr::Num(n) => f"{n}",
        Expr::Name(s) => f"{s}",
        Expr::Add(a, b) => f"({emit(a)} + {emit(b)})",
        Expr::Mul(a, b) => f"({emit(a)} * {emit(b)})",
    }
}

fn unknown(e: ref Expr, known: ref collections::HashSet[String]) -> Vec[String] {
    let mut out = Vec()
    match e {
        Expr::Num(n) => {}
        Expr::Name(s) => { if !known.contains(s) { out.push(s.clone()) } }
        Expr::Add(a, b) => { out.extend(unknown(a, known)) out.extend(unknown(b, known)) }
        Expr::Mul(a, b) => { out.extend(unknown(a, known)) out.extend(unknown(b, known)) }
    }
    return out
}

fn main() throws {
    let source = "let a = 2 + 3\nlet b = a * 4\nlet c = b + z\n"
    let lines = Mini::program(source) catch { println(f"{error}") return }
    let mut known = collections::HashSet()
    let mut rust = "fn main() {\n"
    for l in lines {
        for missing in unknown(l.value, known) {
            println(f"error: nothing declares `{missing}`")
        }
        rust = rust + f"    let {l.name} = {emit(l.value)};\n"
        known.insert(l.name.clone())
    }
    println(rust + "}")
}
"#;
    runs(
        "mini",
        source,
        "error: nothing declares `z`\n\
         fn main() {\n    let a = (2 + 3);\n    let b = (a * 4);\n    let c = (b + z);\n}\n",
    );
}

/// **The first piece of the compiler written in Nikaia** (0.0.238):
/// `std/tools/spelling.nika`, lowered ahead of time and called from the
/// checker's and `dsl`'s *did you mean* as ordinary Rust. The Rust it
/// replaced is gone, so these cases are what hold the answers where they were.
#[test]
fn the_compilers_spelling_is_nikaia() {
    use nikaia_std::tools::spelling::{distance, one_edit_apart};
    let d = |a: &str, b: &str| distance(a, b);
    assert_eq!(d("name", "name"), 0);
    assert_eq!(d("", "abc"), 3);
    assert_eq!(d("abc", ""), 3);
    assert_eq!(d("nmae", "name"), 1, "two neighbours swapped are one edit");
    assert_eq!(d("kitten", "sitting"), 3);
    assert_eq!(d("lenght", "length"), 1);
    assert_eq!(d("prnitln", "println"), 1);
    let one = |a: &str, b: &str| one_edit_apart(a, b);
    assert!(one("id", "ids"), "one put in");
    assert!(one("ids", "id"), "one taken out");
    assert!(one("name", "nome"), "one changed");
    assert!(!one("name", "name"), "the same word is not a near miss");
    assert!(!one("nmae", "name"), "a swap is two edits here");
    assert!(!one("id", "idss"));
    assert!(!one("abc", "xbz"));
}

/// **The second piece, a `dsl` body's holes** (0.0.248), held to what the Rust
/// it replaced answered: a path and a time are not holes, a name is taken once
/// and in the order the body names it, and a body without one has none.
#[test]
fn the_compilers_dsl_holes_are_nikaia() {
    use nikaia_std::tools::dsl::parameters;
    assert_eq!(
        parameters("SELECT * FROM users WHERE id = :id AND name = :name"),
        vec!["id", "name"]
    );
    assert_eq!(
        parameters("a::b and ::c"),
        Vec::<String>::new(),
        "a path is not a hole"
    );
    assert_eq!(
        parameters("at 12:30"),
        Vec::<String>::new(),
        "a time is not a hole"
    );
    assert_eq!(
        parameters(":a :b :a"),
        vec!["a", "b"],
        "each once, first first"
    );
    assert_eq!(parameters(":user_id2."), vec!["user_id2"]);
    assert_eq!(parameters("no holes here"), Vec::<String>::new());
    assert_eq!(parameters(":"), Vec::<String>::new());
    assert_eq!(
        parameters("x = :ä"),
        Vec::<String>::new(),
        "a name is ASCII"
    );
}

/// **What moving that piece needed from the lowering** (0.0.238), in a program
/// of its own: a cell of a list of lists written in place, and an element of a
/// list lent to the function compared with a value - both of which reached
/// `rustc` as a file nobody wrote.
#[test]
fn a_table_is_written_in_place_and_a_lent_element_compares() {
    let source = "fn count(long: ref Vec[char], short: ref Vec[char]) -> i64 {\n\
                  \x20   let mut n = 0\n\
                  \x20   for c in long {\n\
                  \x20       if short[0] == c { n += 1 }\n\
                  \x20       if c == 'x' { n += 10 }\n\
                  \x20       if c != short[0] && c < 'z' { n += 100 }\n\
                  \x20   }\n\
                  \x20   return n\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let mut d: Vec[Vec[i64]] = Vec()\n\
                  \x20   for i in 0..<3 {\n\
                  \x20       let mut row: Vec[i64] = Vec()\n\
                  \x20       for j in 0..<3 { row.push(0) }\n\
                  \x20       d.push(row)\n\
                  \x20   }\n\
                  \x20   d[1][2] = 7\n\
                  \x20   d[2][0] = d[1][2] + 1\n\
                  \x20   println(f\"{d[1][2]} {d[2][0]} {d[0][0]}\")\n\
                  \x20   let a: Vec[char] = \"abxa\".chars().collect()\n\
                  \x20   let b: Vec[char] = \"a\".chars().collect()\n\
                  \x20   println(f\"{count(a, b)}\")\n\
                  }\n";
    runs("table-and-view", source, "7 8 0\n212\n");
}
