//! **Programs that reached `rustc` as a file nobody wrote, or were refused
//! though correct**, each found by running something and kept as the program
//! that found it: 0.0.244's four (`open-work.md` §1.23, §1.24, §1.25, and a
//! bare call nothing declares) and 0.0.245's loops, keys and lists of
//! functions. Every program is run, at both settings of `user_parallelism`.

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

fn runs(purpose: &str, source: &str, expected: &str) {
    runs_in(purpose, source, expected, |_| {});
}

/// [`runs`], in the scratch directory once `prepare` has put there what the
/// program reads.
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

/// **§1.23: a `sync` function calls a parameter whose type says `sync`.** The
/// caller is held to the word (`NK2206`), so the call keeps the promise; it was
/// refused as a call to something no ledger knows. A parameter *without* the
/// word is still refused.
#[test]
fn a_sync_function_calls_a_sync_parameter() {
    runs(
        "sync-parameter",
        "pub fn apply(f: fn(i64) -> i64 sync, x: i64) -> i64 sync {\n\
         \x20   return f(x)\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{apply(fn(n) { n * 2 }, 21)}\")\n\
         }\n",
        "42\n",
    );
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

/// **§1.24: a `Shared` written into a variant takes the variant's count.** The
/// part was declared with the atomic floor and the value built with the count
/// its position got, and `rustc` said *expected `Shared[E]`, found
/// `Shared[E]`*.
#[test]
fn a_shared_value_in_a_variant_has_the_parts_count() {
    runs(
        "shared-variant",
        "enum E {\n\
         \x20   Leaf(i64),\n\
         \x20   Add(Shared[E], Shared[E]),\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let e = E::Add(Shared(E::Leaf(2)), Shared(E::Leaf(3)))\n\
         \x20   match e {\n\
         \x20       E::Add(l, r) => println(\"add\"),\n\
         \x20       E::Leaf(n) => println(f\"{n}\"),\n\
         \x20   }\n\
         }\n",
        "add\n",
    );
}

/// **§1.25: a `?.` chain through two nullable fields of a lent value.** The
/// first step took `a.b` out of a lent `a`; a member that comes out as a view
/// needs its receiver lent, as one that copies did.
#[test]
fn a_chain_through_two_nullable_fields_of_a_lent_value() {
    runs(
        "nullable-chain",
        "struct C { v: i64 }\n\
         struct B { c: C? }\n\
         struct A { b: B? }\n\
         \n\
         fn f(a: ref A) -> i64 {\n\
         \x20   return a.b?.c?.v ?? -1\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let a = A { b: B { c: C { v: 7 } } }\n\
         \x20   let none = A { b: null }\n\
         \x20   let half = A { b: B { c: null } }\n\
         \x20   println(f\"{f(a)} {f(none)} {f(half)} {f(a)}\")\n\
         }\n",
        "7 -1 -1 7\n",
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
        .map(|f| (f.code, f.message.as_str(), f.help.as_deref()))
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
    runs(
        "loop-shadow",
        "use std::collections\n\
         \n\
         fn main() {\n\
         \x20   let mut seen: collections::HashMap[String, i64] = collections::HashMap()\n\
         \x20   let k = \"b\".clone()\n\
         \x20   seen[k] = 2\n\
         \x20   let mut kept: Vec[String] = []\n\
         \x20   for (k, v) in seen {\n\
         \x20       kept.push(f\"{k}={v}\")\n\
         \x20   }\n\
         \x20   println(kept[0])\n\
         }\n",
        "b=2\n",
    );
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

/// **A number, a `bool` or a `char` a `for` binds out of a place is the
/// element** (0.0.245): the body opens by reading it through the view, so a
/// field, a comparison and a map's value take it as they take any number. A
/// field was `rustc`'s *expected `i64`, found `&i64`*. And a `for (k, v)` over a
/// map or a list of pairs binds the parts' types, where both were unknown.
#[test]
fn a_number_a_loop_binds_is_the_number() {
    runs(
        "loop-copies",
        "use std::collections\n\
         \n\
         struct W { count: i64, first: bool }\n\
         \n\
         fn main() {\n\
         \x20   let nums: Vec[i64] = [3, 4]\n\
         \x20   let mut out: Vec[W] = []\n\
         \x20   for n in nums {\n\
         \x20       out.push(W { count: n, first: n == 3 })\n\
         \x20   }\n\
         \x20   let mut seen: collections::HashMap[String, i64] = collections::HashMap()\n\
         \x20   seen[\"a\".clone()] = 5\n\
         \x20   for (k, v) in seen {\n\
         \x20       out.push(W { count: v, first: k == \"a\" })\n\
         \x20   }\n\
         \x20   let pairs: Vec[(String, i64)] = [(\"x\".clone(), 6)]\n\
         \x20   for (name, v) in pairs {\n\
         \x20       out.push(W { count: v, first: false })\n\
         \x20   }\n\
         \x20   for w in out {\n\
         \x20       println(f\"{w.count} {w.first}\")\n\
         \x20   }\n\
         }\n",
        "3 true\n4 false\n5 true\n6 false\n",
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

/// **One key, read and written in one statement** (0.0.245): the read lends
/// it and the write hands it over, and the read's form was the write's too.
#[test]
fn a_key_read_and_written_in_one_statement() {
    runs(
        "key-twice",
        "use std::collections\n\
         \n\
         fn main() {\n\
         \x20   let mut seen: collections::HashMap[String, i64] = collections::HashMap()\n\
         \x20   for w in \"a b a\".split(\" \") {\n\
         \x20       seen[w.clone()] = (seen[w.clone()] ?? 0) + 1\n\
         \x20   }\n\
         \x20   let a = seen[\"a\".clone()] ?? 0\n\
         \x20   println(f\"{a} {seen.len()}\")\n\
         }\n",
        "2 2\n",
    );
}

/// **`pop` is described** (0.0.245): a call no ledger knew made every function
/// that popped `async`, and inside a generic type's own `pop` the list's
/// `pop` was read as the method calling itself.
#[test]
fn a_generic_stack_pops() {
    runs(
        "stack",
        "struct Stack[T] { items: Vec[T] }\n\
         \n\
         impl Stack[T] {\n\
         \x20   fn push(ref mut self, x: T) {\n\
         \x20       self.items.push(x)\n\
         \x20   }\n\
         \x20   fn pop(ref mut self) -> T? {\n\
         \x20       return self.items.pop()\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let mut s = Stack { items: [] }\n\
         \x20   s.push(1)\n\
         \x20   s.push(2)\n\
         \x20   let top = s.pop() ?? 0\n\
         \x20   println(f\"{top} {s.items.len()}\")\n\
         }\n",
        "2 1\n",
    );
}

/// **A function type stands as a list's element** (Part I 5.3, 0.0.245): a
/// lambda written in the list is kept as a named one is, and a parameter that
/// is a list of functions holds kept ones.
#[test]
fn a_list_of_functions_is_run() {
    runs(
        "function-list",
        "fn make_adder(n: i64) -> fn(i64) -> i64 {\n\
         \x20   return fn(x) { x + n }\n\
         }\n\
         \n\
         fn apply_all(fs: ref Vec[fn(i64) -> i64], x: i64) -> i64 {\n\
         \x20   let mut v = x\n\
         \x20   for f in fs {\n\
         \x20       v = f(v)\n\
         \x20   }\n\
         \x20   return v\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let fs: Vec[fn(i64) -> i64] = [make_adder(1), fn(x) { x * 2 }]\n\
         \x20   println(f\"{apply_all(fs, 5)}\")\n\
         }\n",
        "12\n",
    );
}

/// **`_` is the ignore pattern inside a pattern** (ADR-145 D2, 0.0.246): a
/// part is a value that arrived, and `(0, _)` and `Shape::Circle(_)` were
/// refused with the catch-all arm's message - which itself says `_` stands in
/// a tuple position.
#[test]
fn an_ignored_part_of_a_pattern() {
    runs(
        "ignored-part",
        "enum Shape {\n\
         \x20   Circle(f64),\n\
         \x20   Square(f64),\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let shapes = [Shape::Circle(1.0), Shape::Square(2.0), Shape::Square(3.0)]\n\
         \x20   let mut squares = 0\n\
         \x20   for s in shapes {\n\
         \x20       match s {\n\
         \x20           Shape::Square(_) => { squares += 1 }\n\
         \x20           Shape::Circle(_) => {}\n\
         \x20       }\n\
         \x20   }\n\
         \x20   let where_ = match (0, 5) {\n\
         \x20       (0, _) => \"on the y axis\",\n\
         \x20       else => \"elsewhere\",\n\
         \x20   }\n\
         \x20   println(f\"{squares} {where_}\")\n\
         }\n",
        "2 on the y axis\n",
    );
}

/// **A graph over a map of lists** (0.0.246), which found four at once: a set
/// asked about an untyped number took the language below's default integer
/// (`i32`); a list had no `contains`; `m[k]?.clone()` lent the map's view a
/// second time and copied the reference rather than the list; and `m[k] ?? []`
/// reached `rustc`, where it is `NK1185` with the copy to write.
#[test]
fn a_graph_over_a_map_of_lists() {
    runs(
        "graph",
        "use std::collections\n\
         \n\
         struct Graph { edges: collections::HashMap[i64, Vec[i64]] }\n\
         \n\
         impl Graph {\n\
         \x20   fn add(ref mut self, a: i64, b: i64) {\n\
         \x20       let mut list = self.edges[a]?.clone() ?? []\n\
         \x20       list.push(b)\n\
         \x20       self.edges[a] = list\n\
         \x20   }\n\
         \n\
         \x20   fn reachable(ref self, start: i64) -> Vec[i64] {\n\
         \x20       let mut seen: Vec[i64] = []\n\
         \x20       let mut todo: Vec[i64] = [start]\n\
         \x20       while todo.len() > 0 {\n\
         \x20           let n = todo.pop() ?? 0\n\
         \x20           if seen.contains(n) {\n\
         \x20               continue\n\
         \x20           }\n\
         \x20           seen.push(n)\n\
         \x20           for m in self.edges[n]?.clone() ?? [] {\n\
         \x20               todo.push(m)\n\
         \x20           }\n\
         \x20       }\n\
         \x20       seen.sort()\n\
         \x20       return seen\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let mut g = Graph { edges: collections::HashMap() }\n\
         \x20   g.add(1, 2)\n\
         \x20   g.add(2, 3)\n\
         \x20   g.add(3, 1)\n\
         \x20   g.add(4, 5)\n\
         \x20   let mut small: collections::HashSet[i64] = collections::HashSet()\n\
         \x20   small.insert(3)\n\
         \x20   let three = 3\n\
         \x20   let r = g.reachable(1)\n\
         \x20   println(f\"{r.len()} {small.contains(three)} {small.contains(4)}\")\n\
         }\n",
        "3 true false\n",
    );
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

/// **`list(item, sep)` is a rule the backend is given** (Part II 10.8,
/// `open-work.md` §1.26, 0.0.249): the backend has no such element and read
/// `list` as a rule of the grammar's own, so the macro said *expected ident*
/// about the generated file. An empty item between two separators is an item,
/// and an empty input is an empty list.
#[test]
fn a_grammar_reads_a_list_with_a_separator() {
    runs(
        "grammar-list",
        "grammar Csv {\n\
         \x20   pub rule row -> Vec[ref String] = cells:list(CELL, \",\") eof { cells }\n\
         \n\
         \x20   rule CELL -> ref String = c:text(CELL_PIECE*) { c }\n\
         \x20   rule CELL_PIECE = not(\",\") any { }\n\
         \n\
         \x20   pub rule numbers -> Vec[i64] = ns:list(number, \";\") eof { ns }\n\
         \x20   rule number -> i64 = n:dec[i64](digit+) { n }\n\
         }\n\
         \n\
         fn main() throws {\n\
         \x20   let cells = Csv::row(\"a,,bc\")\n\
         \x20   println(f\"{cells.len()} [{cells[0]}] [{cells[1]}] [{cells[2]}]\")\n\
         \x20   let ns = Csv::numbers(\"1;22;333\")\n\
         \x20   println(f\"{ns.len()} {ns[2]}\")\n\
         \x20   let none = Csv::numbers(\"\")\n\
         \x20   println(f\"{none.len()}\")\n\
         }\n",
        "3 [a] [] [bc]\n3 333\n0\n",
    );
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
                  fn letter(c: char) -> bool {\n\
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
    runs("yes-or-no", source, "true false false true 7\n");
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

/// **A view a loop or an arm binds is the view** (0.0.297): an element of a
/// `Vec[ref String]` and a variant's `ref String` part, reached through a lent
/// value, were a view of a view below - `w == "fn"` compared a `&&str` with a
/// `str` (found moving the ledger's reader onto a grammar).
#[test]
fn a_view_a_loop_or_an_arm_binds_is_the_view() {
    runs(
        "view-bindings",
        "enum Line { Table { kind: ref String }, Blank }\n\
         \n\
         fn kind_of(line: ref Line) -> String {\n\
         \x20   match line {\n\
         \x20       Line::Table { kind } => {\n\
         \x20           if kind == \"fn\" { return \"a function\" }\n\
         \x20           return kind.clone()\n\
         \x20       }\n\
         \x20       Line::Blank => { return \"nothing\" }\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let words: Vec[ref String] = [\"fn\", \"let\"]\n\
         \x20   for w in words {\n\
         \x20       if w == \"fn\" { println(\"first\") }\n\
         \x20       println(kind_of(Line::Table { kind: w }))\n\
         \x20   }\n\
         \x20   println(kind_of(Line::Blank))\n\
         }\n",
        "first\na function\nlet\nnothing\n",
    );
}
