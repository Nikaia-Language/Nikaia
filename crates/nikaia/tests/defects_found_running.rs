//! **Programs that reached `rustc` as a file nobody wrote, or were refused
//! though correct**, each found by running something and kept as the program
//! that found it. What those programs compute is
//! `tests/language/src/defects_found_running.nika`; here is what the checker
//! refuses beside them, what the lowering writes, and the programs that read
//! files, run at both settings of `user_parallelism`.
use nikaia::check::CodeOps;

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

/// Checks a program has no error, compiles and runs it at both settings of
/// `user_parallelism` in a scratch directory once `prepare` has put there what
/// the program reads, and compares what it prints.
fn runs_in(purpose: &str, source: &str, expected: &str, prepare: fn(&std::path::Path)) {
    let found = findings(source);
    assert!(found.is_empty(), "{purpose}: {found:#?}");
    for how in [Build::default(), Build::parallel()] {
        let parsed = parse_to_ast(source).expect("the source parses");
        let rust = emit_program(&parsed, how).expect("it lowers").rust;
        let dir = common::scratch_dir(&format!("found-running-{purpose}"));
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
        prepare(&dir);
        let out = Command::new(&binary)
            .current_dir(&dir)
            .output()
            .expect("run it");
        assert!(out.status.success(), "{purpose} failed at {how:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            expected,
            "{purpose} at {how:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// **issue #166: a `sync` function calls a parameter whose type says `sync`.** The
/// caller is held to the word (`NK2206`), so the call keeps the promise (run in
/// `tests/language/src/defects_found_running.nika`). A parameter *without* the
/// word is still refused.
#[test]
fn a_sync_function_calls_a_sync_parameter() {
    let parsed = parse_to_ast(
        "pub fn apply(f: fn(i64) -> i64, x: i64) -> i64 sync {\n\
         \x20   return f(x)\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{apply(fn(n) { n * 2 }, 21)}\")\n\
         }\n",
    )
    .expect("the source parses");
    let unsure = nikaia::contracts::sync::check(
        &parsed,
        &Ledger::infer(&parsed),
        &Ledger::parse(STD).expect("std's ledger"),
    );
    assert!(
        unsure.iter().any(|v| v.callee.contains('f')),
        "a parameter without `sync` is not taken at its word: {unsure:?}"
    );
}

/// **A function nothing declares, called by its bare name, is `NK1117`**, with
/// the near miss where there is one. It lowered as written and `rustc` said
/// *cannot find function*. A bare name has nowhere else to come from: a `use`
/// brings none in.
#[test]
fn a_bare_call_nothing_declares_is_refused() {
    let found = findings(
        "fn doubled(n: i64) -> i64 {\n\
         \x20   return n * 2\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   frobnicate(1)\n\
         \x20   println(f\"{dobled(2)}\")\n\
         }\n",
    );
    let messages: Vec<(&str, &str, Option<&str>)> = found
        .iter()
        .map(|f| (f.code_str(), f.message.as_str(), f.help.as_deref()))
        .collect();
    assert!(
        messages.contains(&(
            "NK1117",
            "There's no function called `frobnicate`.",
            Some("Declare it with `fn frobnicate(…)`, or call it through the package that has it.")
        )),
        "{messages:#?}"
    );
    assert!(
        messages.contains(&(
            "NK1117",
            "There's no function called `dobled`.",
            Some("Did you mean `doubled`?")
        )),
        "{messages:#?}"
    );
}

/// **A `for` binding is a binding of its own** (0.0.245): `for (k, v) in seen`
/// after an outer `k` was written into the map as a key read the loop's `k` as
/// the one handed over, and refused a correct program with `NK2105`. A read of
/// the outer one after the loop is still refused.
#[test]
fn a_loop_binding_is_not_the_name_it_shadows() {
    let source = "fn main() {\n\
         \x20   let mut kept: Vec[String] = []\n\
         \x20   let k = \"b\".clone()\n\
         \x20   kept.push(k)\n\
         \x20   for (k, v) in [(\"x\".clone(), 1)] {\n\
         \x20       println(f\"{k} {v}\")\n\
         \x20   }\n\
         \x20   println(k)\n\
         }\n";
    let outer = source.find("println(k)").expect("the read after the loop");
    let handed: Vec<usize> = findings(source)
        .iter()
        .filter(|f| f.code == "NK2105")
        .map(|f| f.span.at())
        .collect();
    assert_eq!(
        handed,
        vec![outer],
        "the outer `k`, read after the loop, was handed over, and the loop's is another"
    );
}

/// **A binding lent text is a view of it** (0.0.245), and one put where text
/// of its own is kept is refused with the copy to write - `rustc` said
/// *expected `String`, found `&String`*.
#[test]
fn text_a_loop_lends_is_kept_only_as_a_copy() {
    let found = findings(
        "struct Word { text: String }\n\
         \n\
         fn main() {\n\
         \x20   let names: Vec[String] = [\"a\".clone()]\n\
         \x20   let mut out: Vec[Word] = []\n\
         \x20   for s in names {\n\
         \x20       out.push(Word { text: s })\n\
         \x20   }\n\
         }\n",
    );
    assert!(
        found.iter().any(|f| f.code == "NK1106"
            && f.help.as_deref() == Some("Write `.clone()` to copy it here.")),
        "{found:#?}"
    );
}

/// **A graph over a map of lists** (0.0.246), which found four at once: a set
/// asked about an untyped number took the language below's default integer
/// (`i32`); a list had no `contains`; `m[k]?.clone()` lent the map's view a
/// second time and copied the reference rather than the list; and `m[k] ?? []`
/// reached `rustc`, where it is `NK1185` with the copy to write.
#[test]
fn a_graph_over_a_map_of_lists() {
    let found = findings(
        "use std::collections\n\
         \n\
         fn main() {\n\
         \x20   let edges: collections::HashMap[i64, Vec[i64]] = collections::HashMap()\n\
         \x20   for m in edges[1] ?? [] {\n\
         \x20       println(f\"{m}\")\n\
         \x20   }\n\
         }\n",
    );
    assert!(
        found.iter().any(|f| f.code == "NK1185"
            && f.help
                .as_deref()
                .is_some_and(|h| h.contains("`edges[1]?.clone() ?? …`"))),
        "{found:#?}"
    );
    // A fallback that leaves is not one of its own: nothing to refuse.
    let leaves = findings(
        "use std::collections\n\
         \n\
         fn main() {\n\
         \x20   let edges: collections::HashMap[i64, Vec[i64]] = collections::HashMap()\n\
         \x20   let first = edges[1] ?? panic(\"no edges from 1\")\n\
         \x20   println(f\"{first.len()}\")\n\
         }\n",
    );
    assert!(leaves.iter().all(|f| f.code != "NK1185"), "{leaves:#?}");
}

/// **What the lowering repeats, it says once** (0.0.249): a `match` that only
/// answers `true` or `false` is a `matches!`, and `name: name` in a struct
/// literal is `name`. Both long forms are what `clippy` refuses in `std`, so a
/// `.nika` file there had to write around them - `tools/dsl.nika` asked a text
/// of the set instead of a range, and `tools/http1.nika` wrote the shorthand.
#[test]
fn a_yes_or_no_match_and_a_field_named_as_its_value() {
    let source = "struct P { x: i64, y: i64 }\n\
                  \n\
                  fn letter(c: scalar) -> bool {\n\
                  \x20   return match c {\n\
                  \x20       'a'..'z' | 'A'..'Z' => true,\n\
                  \x20       else => false,\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn not_small(n: i64) -> bool {\n\
                  \x20   return match n {\n\
                  \x20       0 | 1 | 2 => false,\n\
                  \x20       else => true,\n\
                  \x20   }\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let x = 3\n\
                  \x20   let y = 4\n\
                  \x20   let p = P { x: x, y }\n\
                  \x20   println(f\"{letter('q')} {letter('1')} {not_small(1)} {not_small(9)} {p.x + p.y}\")\n\
                  }\n";
    let rust = emit_program(&parse_to_ast(source).expect("it parses"), Build::default())
        .expect("it lowers")
        .rust;
    for written in [
        "matches!(c, 'a'..='z' | 'A'..='Z')",
        "!matches!(n, 0 | 1 | 2)",
        "P { x, y }",
    ] {
        assert!(rust.contains(written), "missing `{written}`:\n{rust}");
    }
}

/// **A `std` variant built from a literal**: `fs::Root::Dir("site")` passed the
/// check and reached `rustc` as `Dir("site")`, a `&str` where `fs.rs` declares
/// `Dir(String)`. The ledger named `fs::Root` without its cases, so the checker
/// had no payload for the literal to meet; it carries `variants` now, and a
/// positional one is built by its constructor as a declared enum's is. The
/// signatures say `ref fs::Root`, the name a caller writes, so a parameter of
/// that type is a root too - it was `NK1102` against `ref Root`.
#[test]
fn a_root_under_a_directory_built_from_a_literal() {
    runs_in(
        "root-dir-literal",
        "use std::fs\n\
         \n\
         fn load(root: ref fs::Root) -> String throws {\n\
         \x20   return fs::read_to_string(\"index.html\", root)\n\
         }\n\
         \n\
         fn main() throws {\n\
         \x20   let page = fs::map(\"index.html\", fs::Root::Dir(\"site\"))\n\
         \x20   let root = fs::Root::Dir(\"site\")\n\
         \x20   let again = load(root)\n\
         \x20   println(f\"{again.len()}\")\n\
         }\n",
        "6\n",
        |dir| {
            std::fs::create_dir_all(dir.join("site")).expect("the site directory");
            std::fs::write(dir.join("site/index.html"), "<p>hi\n").expect("the page");
        },
    );
}

/// **A `mut` parameter is typed like any other** (#125): the `&mut` is the
/// compiler's to write, and the check of the argument stopped there - a
/// `Box2` handed to a `mut xs: Vec[i64]` was accepted and `rustc` refused the
/// file. It is `NK1102`, as at any other position.
#[test]
fn an_argument_for_a_mut_parameter_is_its_type() {
    let found = findings(
        "struct Box2 { items: Vec[i64] }\n\
         fn add(mut xs: Vec[i64]) { xs.push(1) }\n\
         fn main() {\n\
         \x20   let mut b = Box2 { items: [] }\n\
         \x20   add(b)\n\
         }\n",
    );
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK1102" && f.message.contains("`add` expects `xs`")),
        "{found:#?}"
    );
}

/// **A branch that ends in `continue` takes its takings with it** (#125), as
/// one that ends in `return` does (ADR-293 D30): `kept.push(name)` then
/// `continue` was refused with `NK2105` for the `println(name)` after the
/// `if`, which no path that pushed reaches. A name from outside the loop is
/// still refused, by the loop's own rule.
#[test]
fn a_branch_that_continues_takes_its_takings_with_it() {
    let outer = findings(
        "fn outer(names: ref Vec[String]) -> i64 {\n\
         \x20   let mut kept: Vec[String] = []\n\
         \x20   let first: String = \"x\"\n\
         \x20   for one in names {\n\
         \x20       if one.len() > 3 {\n\
         \x20           kept.push(first)\n\
         \x20           continue\n\
         \x20       }\n\
         \x20   }\n\
         \x20   return kept.len()\n\
         }\n\
         fn main() { }\n",
    );
    assert!(outer.iter().any(|f| f.code == "NK2105"), "{outer:#?}");
}

/// **A view of what does not copy, kept where a value is wanted, is refused
/// here** rather than by `rustc` (found moving the compiler's `views` into
/// Nikaia, #125): `for p in found { out.push(p) }` lends each element, and
/// a `Vec[P]` keeps a `P` of its own. Text had this sentence already
/// (ADR-282 D19); every other type reached the language below.
#[test]
fn a_lent_element_kept_whole_is_refused() {
    let found = findings(
        "struct P {\n\
         \x20   n: i64,\n\
         \x20   s: String,\n\
         }\n\
         \n\
         fn pushed(found: ref Vec[P]) -> Vec[P] {\n\
         \x20   let mut out: Vec[P] = []\n\
         \x20   for p in found {\n\
         \x20       out.push(p)\n\
         \x20   }\n\
         \x20   return out\n\
         }\n\
         \n\
         fn main() { }\n",
    );
    assert!(
        found.iter().any(|f| f.code == "NK1102"
            && f.message.contains("`Vec::push` expects `value` to be `P`")
            && f.help
                .as_deref()
                .is_some_and(|help| help.contains(".clone()"))),
        "{found:#?}"
    );
}

/// **A fallback block that ends in a jump is a fallback that jumps** (#457,
/// found moving the prover's `Scope` into Nikaia, #436): `?? { s.insert(name)
/// return 0 }` hands `name` over only on the path that leaves. It was
/// `NK2105` for the read after it; a block that does not leave still is.
#[test]
fn a_fallback_block_that_returns_takes_its_takings_with_it() {
    let leaves = "use std::collections\n\
                  \n\
                  fn f(known: i64?, mut s: collections::BTreeSet[String]) -> i64 {\n\
                  \x20   let name: String = \"xy\"\n\
                  \x20   let n = known ?? {\n\
                  \x20       s.insert(name)\n\
                  \x20       return 0\n\
                  \x20   }\n\
                  \x20   return n + name.len()\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let mut s: collections::BTreeSet[String] = collections::BTreeSet()\n\
                  \x20   println(f\"{f(3, s)} {f(null, s)} {s.len()}\")\n\
                  }\n";
    let stays = leaves.replace("        return 0\n", "        0\n");
    assert!(
        findings(&stays).iter().any(|f| f.code == "NK2105"),
        "a fallback that does not leave still hands `name` over"
    );
}

/// **A mapped file is text wherever text is read** (#452): sliced by a
/// range, through its `deref`, as a `ref String` a `let` declares, and handed
/// to a function that reads text - each the same as the file read whole.
/// The three first ones passed the checker and failed in `rustc`.
#[test]
fn a_mapped_file_is_read_as_text() {
    runs_in(
        "mapped-as-text",
        "use std::fs\n\
         \n\
         fn length(text: ref String) -> i64 {\n\
         \x20   return text.len()\n\
         }\n\
         \n\
         fn main() throws {\n\
         \x20   let data = fs::map(\"m.txt\", fs::Root::Anywhere)\n\
         \x20   let a = data[0..<7]\n\
         \x20   let b = data.deref()\n\
         \x20   let c: ref String = data\n\
         \x20   let whole = fs::read_to_string(\"m.txt\", fs::Root::Anywhere)\n\
         \x20   println(f\"{a}|{b == whole}|{c == whole}|{length(data)}\")\n\
         }\n",
        "Hamburg|true|true|13\n",
        |dir| std::fs::write(dir.join("m.txt"), "Hamburg;12.3\n").expect("write the input"),
    );
}

/// **A function takes the parameters the program wrote** (#459, found
/// moving the prover's `carry_back` into Nikaia, #436): eight parameters are
/// written with the `allow` for the language below's lint, seven without.
#[test]
fn a_long_parameter_list_is_allowed_below() {
    let source = "fn eight(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, h: i64) -> i64 {\n\
                  \x20   return a + b + c + d + e + f + g + h\n\
                  }\n\
                  \n\
                  fn seven(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64) -> i64 {\n\
                  \x20   return a + b + c + d + e + f + g\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   println(f\"{eight(1, 2, 3, 4, 5, 6, 7, 8)} {seven(1, 2, 3, 4, 5, 6, 7)}\")\n\
                  }\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust;
    let allowed = rust.matches("#[allow(clippy::too_many_arguments)]").count();
    assert_eq!(allowed, 1, "{rust}");
    let at = rust.find("#[allow(clippy::too_many_arguments)]").unwrap();
    assert!(
        rust[at..]
            .trim_start_matches("#[allow(clippy::too_many_arguments)]")
            .trim_start()
            .starts_with("fn eight"),
        "{rust}"
    );
}

/// **A `T?` compared with a `T` is refused** (`NK1102`, 0.0.375): it reached
/// the language below as *expected `Option<i64>`, found `i64`*. Found moving
/// `sharing` into Nikaia (#125).
#[test]
fn a_maybe_compared_with_a_value_is_refused() {
    let found = findings(
        "fn same(at: i64?, now: i64) -> bool {\n\
         \x20   return at == now\n\
         }\n\
         fn after(at: ref String?, now: ref String) -> bool {\n\
         \x20   return at != now\n\
         }\n\
         fn main() {\n\
         \x20   println(f\"{same(3, 3)} {after(null, \"a\")}\")\n\
         }\n",
    );
    let codes: Vec<&str> = found.iter().map(|f| f.code_str()).collect();
    assert_eq!(codes, ["NK1102", "NK1102"], "{found:#?}");
    assert!(found[0].message.contains("`i64?` with `i64`"), "{found:#?}");
    // `null` and another `T?` are what a `T?` compares with.
    let found = findings(
        "fn absent(at: i64?) -> bool {\n\
         \x20   return at == null\n\
         }\n\
         fn same(a: i64?, b: i64?) -> bool {\n\
         \x20   return a == b\n\
         }\n\
         fn main() {\n\
         \x20   println(f\"{absent(null)} {same(1, 1)}\")\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:#?}");
}
