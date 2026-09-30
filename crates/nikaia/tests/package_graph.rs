//! The inference graph is the **package**, not the file
//! ([ADR-100](../../../docs/specification/adr/adr-100.md) D2).
//!
//! Every derived column is a fixpoint over a call graph, and until this the
//! graph stopped at the file boundary: `reach_of` found a callee in the file
//! next door in neither `own` nor `std` and set `blocked`. `Sync::No` spells
//! that *"can pause **or** could not be vouched for"*, and every reader takes
//! the first — so `NK1129` refused a trait method for calling a plain function
//! two lines away in another file, `throws` came out `"?"` where the neighbour
//! names its error, and `touches` came out *"everything"* where the neighbour
//! says what it reaches.
//!
//! **The polarity is what these tests are for.** Making an analysis answer
//! where it could not is only correct if it still declines where it should, so
//! every test that shows a claim arriving has a twin showing the claim withheld
//! — a genuinely pausing neighbour is still `NK1129`, and a neighbour outside
//! the package is still unresolved.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use nikaia::contracts::{Ledger, LedgerOps, STD, Sync};
use nikaia::emit::Build;
use nikaia::modules::Program;

/// Write a package of files into a scratch directory and hand back the entry.
fn project(name: &str, files: &[(&str, &str)]) -> (PathBuf, PathBuf) {
    let dir = common::scratch_dir(&format!("package-graph-{name}"));
    for (file, contents) in files {
        std::fs::write(dir.join(file), contents).expect("write the file");
    }
    (dir.clone(), dir.join("main.nika"))
}

/// The ledger one package of files produces.
fn contracts(name: &str, files: &[(&str, &str)]) -> Ledger {
    let (dir, entry) = project(name, files);
    let program = Program::read(&entry).expect("the program reads");
    let contracts = program.contracts.clone();
    let _ = std::fs::remove_dir_all(dir);
    contracts
}

/// Every finding the checker has about a package, read the way the compiler
/// reads it.
///
/// **`check_program` over the finished ledger**, which is the only entry point
/// that runs after the four inferences have filled their columns
/// ([ADR-080](../../../docs/specification/adr/adr-080.md) §2) — a test on the
/// inner one would find `NK1129` silent and conclude it was never raised.
fn findings(program: &Program) -> Vec<String> {
    let library = Ledger::parse(STD).expect("std ships a ledger this compiler can read");
    let packages = program.package_names();
    program
        .units
        .iter()
        .flat_map(|unit| {
            nikaia::check::check_program(&unit.parsed, &program.contracts, &library, &packages)
                .findings
        })
        .map(|f| format!("{f:?}"))
        .collect()
}

/// Lower, compile and run — the only proof that the two files really became one
/// program rather than one the checker merely stopped objecting to.
fn run(entry: &Path, build: Build) -> String {
    let program = Program::read(entry).expect("the program reads");
    let found = findings(&program);
    assert!(found.is_empty(), "a correct program is refused: {found:#?}");

    let lowered = program.emit(build).expect("the program lowers");
    let dir = entry.parent().expect("a directory");
    let rust = dir.join("program.rs");
    std::fs::write(&rust, &lowered.rust).expect("write the Rust");

    let binary = dir.join("program");
    let compiled = common::compile(
        &rust,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{}",
        String::from_utf8_lossy(&compiled.stderr),
        lowered.rust
    );
    let ran = Command::new(&binary).output().expect("run it");
    assert!(
        ran.status.success(),
        "the program failed:\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    String::from_utf8_lossy(&ran.stdout).to_string()
}

/// The shorter of `open-work.md`'s two reproductions, and the one that needs no
/// package at all: two files of **one** package, a trait method, and a plain
/// function next door.
///
/// It was `NK1129`: *"`Thing::go` can pause, and `Simple` declares it as a
/// method that cannot"*, about a body whose only call is `n + 1`.
#[test]
fn a_trait_method_calling_the_file_next_door_is_not_refused() {
    let (dir, entry) = project(
        "trait-method",
        &[
            (
                "helper.nika",
                "pub fn plain(n: i64) -> i64 {\n    return n + 1\n}\n",
            ),
            (
                "main.nika",
                "trait Simple {\n\
                 \x20   fn go(ref self) -> i64\n\
                 }\n\
                 \n\
                 struct Thing {\n\
                 \x20   n: i64\n\
                 }\n\
                 \n\
                 impl Simple for Thing {\n\
                 \x20   fn go(ref self) -> i64 {\n\
                 \x20       return plain(self.n)\n\
                 \x20   }\n\
                 }\n\
                 \n\
                 fn main() {\n\
                 \x20   let t = Thing { n: 1 }\n\
                 \x20   println(f\"{t.go()}\")\n\
                 }\n",
            ),
        ],
    );

    assert_eq!(run(&entry, Build::default()).trim(), "2");
    let _ = std::fs::remove_dir_all(dir);
}

/// **And the refusal is still there where the declaration says `sync`.**
///
/// The fix was that the graph reaches the file next door, not that a trait
/// method stopped being compared — so a neighbour that genuinely pauses still
/// takes the claim away and `NK1129` still says so. Without this test the
/// change above is indistinguishable from relaxing the rule, which would trade
/// a false refusal for a silent miscompilation.
///
/// **The `sync` on the declaration is what makes it a refusal**, since
/// [ADR-109](../../../docs/specification/adr/adr-109.md) D1: a trait method
/// without the word *may* pause, so the same `impl` under a wordless
/// declaration is a program — which the test below holds.
#[test]
fn a_trait_method_that_really_pauses_in_the_file_next_door_is_still_refused() {
    let (dir, entry) = project(
        "trait-method-pauses",
        &[
            (
                "helper.nika",
                "use std::io\n\npub fn reads() -> String {\n\
                 \x20   return io::read_to_string() catch { \"\" }\n\
                 }\n",
            ),
            (
                "main.nika",
                "trait Simple {\n\
                 \x20   fn go(ref self) -> i64 sync\n\
                 }\n\
                 \n\
                 struct Thing {\n\
                 \x20   n: i64\n\
                 }\n\
                 \n\
                 impl Simple for Thing {\n\
                 \x20   fn go(ref self) -> i64 {\n\
                 \x20       return reads().len() as i64\n\
                 \x20   }\n\
                 }\n\
                 \n\
                 fn main() {\n\
                 \x20   let t = Thing { n: 1 }\n\
                 \x20   println(f\"{t.go()}\")\n\
                 }\n",
            ),
        ],
    );

    let program = Program::read(&entry).expect("the program reads");
    let found = findings(&program);
    assert!(
        found.iter().any(|f| f.contains("NK1129")),
        "a body that reaches a pausing neighbour is still refused: {found:#?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The `sync` column itself, which is what `NK1129` reads.
///
/// Three functions and three answers, so the claim is not simply handed out:
/// the one that calls next door earns it, the one that calls a pausing
/// neighbour does not, and the pausing neighbour does not either.
#[test]
fn sync_is_inferred_across_the_files_of_a_package() {
    let ledger = contracts(
        "sync-column",
        &[
            (
                "helper.nika",
                "use std::io\n\npub fn plain(n: i64) -> i64 {\n\
                 \x20   return n + 1\n\
                 }\n\
                 \n\
                 pub fn reads() -> String {\n\
                 \x20   return io::read_to_string() catch { \"\" }\n\
                 }\n",
            ),
            (
                "main.nika",
                "fn calls_plain(n: i64) -> i64 {\n\
                 \x20   return plain(n)\n\
                 }\n\
                 \n\
                 fn calls_reader() -> String {\n\
                 \x20   return reads()\n\
                 }\n\
                 \n\
                 fn main() { }\n",
            ),
        ],
    );

    assert_eq!(ledger.functions["calls_plain"].sync_claim, Sync::Inferred);
    assert_eq!(ledger.functions["calls_reader"].sync_claim, Sync::No);
    assert_eq!(ledger.functions["reads"].sync_claim, Sync::No);
}

/// **The same correction reaches `throws`**, which ADR-100 §3 names and nothing
/// else here would have shown: the column carried `"?"` — the absence of a
/// claim — where the neighbour names the error it throws.
#[test]
fn a_throwing_neighbour_is_named_rather_than_unknown() {
    let ledger = contracts(
        "throws-column",
        &[
            (
                "helper.nika",
                "pub enum LeereZeile { Leer }\n\
                 \n\
                 pub fn pruefe(n: ref i64) -> i64 throws {\n\
                 \x20   if n == 0 { throw LeereZeile::Leer }\n\
                 \x20   return 1\n\
                 }\n",
            ),
            (
                "main.nika",
                "fn sortiere(xs: Vec[i64]) -> i64 throws {\n\
                 \x20   xs.sort_by_key fn(v) { pruefe(v) }\n\
                 \x20   return 1\n\
                 }\n\
                 \n\
                 fn main() { }\n",
            ),
        ],
    );

    assert_eq!(ledger.functions["pruefe"].fails_with, ["LeereZeile"]);
    assert_eq!(ledger.functions["sortiere"].fails_with, ["LeereZeile"]);
}

/// **And `touches`**, where the polarity runs the other way round and is
/// therefore the sharper test.
///
/// An empty `touches` means *touches everything* unless `touches_known` says
/// otherwise ([ADR-033](../../../docs/specification/adr/adr-033.md) D4), so a
/// call the walk could not resolve did not read as an unknown — it read as an
/// answer nobody could use. Two `overlap` branches over such a function order
/// against each other for no reason.
#[test]
fn what_a_neighbour_reaches_is_what_the_caller_reaches() {
    let ledger = contracts(
        "touches-column",
        &[
            (
                "helper.nika",
                "pub fn say(n: i64) {\n\
                 \x20   println(f\"{n}\")\n\
                 }\n",
            ),
            (
                "main.nika",
                "fn twice(n: i64) {\n\
                 \x20   say(n)\n\
                 \x20   say(n)\n\
                 }\n\
                 \n\
                 fn main() { }\n",
            ),
        ],
    );

    let twice = &ledger.functions["twice"];
    assert!(twice.touches_known, "every step of it is described");
    let reached: Vec<String> = twice
        .touches
        .iter()
        .map(|t| format!("{} {}", t.kind, if t.write { "write" } else { "read" }))
        .collect();
    assert_eq!(reached, vec!["stdout write".to_string()]);
}

/// **The answer does not depend on the order the units arrive in**, which
/// Part III 13.5 needs: the ledger is a pure function of the source tree and
/// `--locked` compares it byte for byte.
///
/// The fixpoints walk a `BTreeMap` and repeat until nothing changes, so this
/// holds by construction — and it is checked rather than argued, because the
/// construction is now one graph over several files and a `Vec` of them is the
/// obvious thing to iterate in the order it came.
#[test]
fn the_ledger_does_not_depend_on_the_order_the_units_arrive_in() {
    let a = nikaia::parser::parse_to_ast(
        "pub fn plain(n: i64) -> i64 { return n + 1 }\n\
         fn calls_across() -> i64 { return twice(1) }\n",
    )
    .expect("the source parses");
    let b = nikaia::parser::parse_to_ast("pub fn twice(n: i64) -> i64 { return plain(n) * 2 }\n")
        .expect("the source parses");

    let library = Ledger::parse(STD).expect("std ships a ledger this compiler can read");
    let forwards = Ledger::infer_package(&[&a, &b], &library).render();
    let backwards = Ledger::infer_package(&[&b, &a], &library).render();
    assert_eq!(forwards, backwards);

    // And it is an answer rather than two matching absences: the call graph
    // here runs `calls_across` → `twice` → `plain`, across the boundary in both
    // directions, and every step of it is `sync`.
    let ledger = Ledger::infer_package(&[&a, &b], &library);
    assert_eq!(ledger.functions["calls_across"].sync_claim, Sync::Inferred);
    assert_eq!(ledger.functions["twice"].sync_claim, Sync::Inferred);
}
