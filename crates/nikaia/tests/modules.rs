//! A package of several files (Part I, 9.1).
//!
//! Every test here builds a real directory of `.nika` files and drives the
//! compiler over it, because what is being tested is *resolution* - which files
//! take part, what they may see of each other, and what one ledger over all of
//! them says. None of that can be checked on a string.
//!
//! **A package is a directory** ([ADR-286](../../../docs/specification/adr/adr-286.md)
//! D1): the files in it share one namespace and need no `use` between them. What
//! used to be here - `use utils`, `utils::double(21)`, a private item refused
//! across a file boundary - was the file = module model that decision replaced.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use nikaia::contracts::{Ledger, LedgerOps};
use nikaia::emit::Build;
use nikaia::modules::Program;

/// Write a set of files into a scratch directory and hand back the entry.
fn project(name: &str, files: &[(&str, &str)]) -> (PathBuf, PathBuf) {
    let dir = common::scratch_dir(&format!("modules-{name}"));
    for (file, contents) in files {
        let path = dir.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create directories");
        }
        std::fs::write(&path, contents).expect("write the file");
    }
    (dir.clone(), dir.join("main.nika"))
}

/// Lower, compile and run - the only proof that the files really became one
/// program.
fn run(entry: &Path, build: Build) -> String {
    let program = Program::read(entry).expect("the program reads");
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

const UTILS: &str = "\
pub fn double(n: i32) -> i32 {
    return n * 2
}

fn secret() -> i32 {
    return 41
}

pub fn answer() -> i32 {
    return secret() + 1
}
";

/// Two files, one package - and it runs, with no `use` between them.
#[test]
fn two_files_of_a_package_are_one_program() {
    let (dir, entry) = project(
        "two-files",
        &[
            ("utils.nika", UTILS),
            (
                "main.nika",
                "fn main() {\n\
                 \x20   println(f\"{double(21)} {answer()}\")\n\
                 }\n",
            ),
        ],
    );

    assert_eq!(run(&entry, Build::default()).trim(), "42 42");
    let _ = std::fs::remove_dir_all(dir);
}

/// **Every `.nika` beside the entry takes part, whether or not anything names
/// it** (ADR-286 D1).
///
/// The directory decides and the `use` lines do not, which is what makes moving
/// a declaration from one file to another housekeeping rather than a change to
/// the package's surface. Without this the file below would simply not be read.
#[test]
fn a_file_nothing_names_is_still_part_of_the_package() {
    let (dir, entry) = project(
        "unnamed-file",
        &[
            ("extra.nika", "pub fn seven() -> i64 {\n    return 7\n}\n"),
            (
                "main.nika",
                "fn main() {\n\x20   println(f\"{seven()}\")\n}\n",
            ),
        ],
    );
    assert_eq!(run(&entry, Build::default()).trim(), "7");
    let _ = std::fs::remove_dir_all(dir);
}

/// **A name that is not `pub` is still visible inside its package** — which is
/// the half of Part I 9.2 that moved.
///
/// It used to be `NK1110` across a file boundary. The boundary is the package
/// now, so a sibling file reaching `secret()` is ordinary code, and the check has
/// nothing to say about it.
#[test]
fn a_private_name_is_visible_across_the_files_of_its_package() {
    let (dir, entry) = project(
        "package-privacy",
        &[
            ("utils.nika", UTILS),
            (
                "main.nika",
                "fn main() {\n\x20   println(f\"{secret()}\")\n}\n",
            ),
        ],
    );

    let program = Program::read(&entry).expect("the program reads");
    let library = Ledger::parse(nikaia::contracts::STD).expect("std's ledger");
    let packages = program.package_names();
    for unit in &program.units {
        let findings =
            nikaia::check::check_program(&unit.parsed, &program.contracts, &library, &packages)
                .findings;
        assert!(findings.is_empty(), "{:#?}", findings);
    }

    assert_eq!(run(&entry, Build::default()).trim(), "41");
    let _ = std::fs::remove_dir_all(dir);
}

/// **Two files of one package may not declare the same name** (ADR-286 D1).
///
/// One namespace, so this is an error rather than a rule about which of the two a
/// line means - and it is refused here, because `rustc` would report a duplicate
/// definition against a file nobody wrote (Part III, C.1).
#[test]
fn two_files_may_not_declare_the_same_name() {
    let (dir, entry) = project(
        "collision",
        &[
            ("a.nika", "pub struct Row { pub id: i64 }\n"),
            ("main.nika", "struct Row { id: i64 }\n\nfn main() { }\n"),
        ],
    );
    let Err(error) = Program::read(&entry) else {
        panic!("two `Row`s in one namespace");
    };
    let message = format!("{error:#}");
    assert!(message.contains("`Row` is declared twice"), "{message}");
    assert!(
        message.contains("a.nika") && message.contains("main.nika"),
        "{message}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// One ledger for the package (Part III, 13.5), and **one namespace in it**.
///
/// The keys used to be `utils::double`, because a file was a unit of naming. It
/// is not one any more (ADR-286 D1), so a name belongs to the package and the
/// ledger says so - which is also what a consumer of this package will read,
/// under the package's own name rather than under the file it happens to sit in.
#[test]
fn the_ledger_is_one_file_with_one_namespace_in_it() {
    let (dir, entry) = project(
        "ledger",
        &[("utils.nika", UTILS), ("main.nika", "fn main() { }\n")],
    );

    let program = Program::read(&entry).expect("the program reads");
    let rendered = program.contracts.render();

    for key in ["double", "answer", "secret", "main"] {
        assert!(rendered.contains(&format!("[fn.\"{key}\"]")), "{rendered}");
    }
    assert!(
        !rendered.contains("utils::"),
        "no file is a prefix:\n{rendered}"
    );

    // …and it reads back, which is what `--locked` rests on. `pub` still says
    // what leaves the *package*, which is the boundary that is left.
    let read = Ledger::parse(&rendered).expect("its own output parses");
    assert!(read.functions["double"].public);
    assert!(!read.functions["secret"].public);

    let _ = std::fs::remove_dir_all(dir);
}

/// The determinism of 13.5 is about the tree, not about a file: the same sources
/// produce the same ledger however many times they are read, and the order is the
/// directory sorted rather than the order anything was discovered in.
#[test]
fn the_ledger_of_a_program_is_deterministic() {
    let (dir, entry) = project(
        "deterministic",
        &[
            ("z.nika", "pub fn two() -> i32 { return 2 }\n"),
            ("a.nika", "pub fn one() -> i32 { return 1 }\n"),
            ("main.nika", "fn main() { println(f\"{one()}{two()}\") }\n"),
        ],
    );

    let first = Program::read(&entry).expect("reads").contracts.render();
    let second = Program::read(&entry).expect("reads").contracts.render();
    assert_eq!(first, second);
    assert_eq!(run(&entry, Build::default()).trim(), "12");

    let _ = std::fs::remove_dir_all(dir);
}

/// Two files that reach into each other are one namespace, so there is no cycle
/// to break - only two files.
#[test]
fn two_files_may_reach_into_each_other() {
    let (dir, entry) = project(
        "cycle",
        &[
            ("a.nika", "pub fn one() -> i32 { return 1 }\n"),
            ("b.nika", "pub fn two() -> i32 { return one() + 1 }\n"),
            ("main.nika", "fn main() { println(f\"{two()}\") }\n"),
        ],
    );

    assert_eq!(run(&entry, Build::default()).trim(), "2");
    let _ = std::fs::remove_dir_all(dir);
}

/// **A `use` that names a file beside this one says the rule that replaced it**
/// (ADR-286 D1).
///
/// The shape that used to work is the one most likely to be written, so the
/// message is about the package rather than about a name it could not find.
#[test]
fn a_use_naming_a_sibling_file_says_to_remove_it() {
    let (dir, entry) = project(
        "sibling-use",
        &[
            ("utils.nika", UTILS),
            ("main.nika", "use utils\n\nfn main() { }\n"),
        ],
    );

    let Err(error) = Program::read(&entry) else {
        panic!("`use utils` names a file of this package");
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("the files of a package already see each other"),
        "{message}"
    );
    assert!(message.contains("Remove the line"), "{message}");
    let _ = std::fs::remove_dir_all(dir);
}

/// …and a `use` naming something no dependency declares says what to declare
/// ([ADR-286](../../../docs/specification/adr/adr-286.md) D13 from the other
/// side: a prefix must be introduced, and this is an introduction that names
/// nothing).
#[test]
fn a_use_naming_no_dependency_says_what_to_declare() {
    let (dir, entry) = project(
        "foreign-use",
        &[("main.nika", "use helpers\n\nfn main() { }\n")],
    );
    let Err(error) = Program::read(&entry) else {
        panic!("nothing is called `helpers`");
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("there's no dependency called `helpers`"),
        "{message}"
    );
    assert!(message.contains("[dependencies]"), "{message}");
    let _ = std::fs::remove_dir_all(dir);
}

/// **A `use` with a path in it names one package and brings no name in**
/// ([ADR-286](../../../docs/specification/adr/adr-286.md) D10, D11).
#[test]
fn a_use_with_a_path_is_refused_and_std_is_not() {
    let (dir, entry) = project(
        "path-use",
        &[("main.nika", "use net::http\n\nfn main() { }\n")],
    );
    let Err(error) = Program::read(&entry) else {
        panic!("a path names no package");
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("`use` takes a package name only, not a path into it"),
        "{message}"
    );
    assert!(message.contains("`use net`"), "{message}");
    let _ = std::fs::remove_dir_all(dir);

    // `use std::fs` names the library and never a package of yours, so a program
    // that only imports `std` is unaffected.
    let (dir, entry) = project(
        "std-only",
        &[("main.nika", "use std::fs\nuse std::cli\n\nfn main() { }\n")],
    );
    let program = Program::read(&entry).expect("std is not a package of yours");
    assert!(program.is_single_file());
    let _ = std::fs::remove_dir_all(dir);
}

/// **One name per file** ([ADR-286](../../../docs/specification/adr/adr-286.md)
/// D5): two packages under one name is an error rather than a rule about which
/// of them a line means.
#[test]
fn one_name_per_file() {
    let (dir, entry) = project(
        "twice",
        &[("main.nika", "use helpers\nuse helpers\n\nfn main() { }\n")],
    );
    let Err(error) = Program::read(&entry) else {
        panic!("`helpers` is named twice");
    };
    let message = format!("{error:#}");
    assert!(message.contains("twice"), "{message}");
    assert!(
        message.contains("Two packages can't have the same name in one file"),
        "{message}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// **No Rust warning about the generated file reaches the user** (Part III, C.1).
///
/// Measured before this: every multi-file build printed
/// `warning: …/gen:7: unused import: `super::*` (no Nikaia source maps to this)`
/// and told the reader to remove a `use` item this compiler had written itself.
///
/// The import is gone entirely now - one namespace needs no `mod` and nothing to
/// bring the siblings in (ADR-286 D1) - so this asserts the emitted shape as well
/// as the silence. The second half is the one that keeps holding when the next
/// machine-written construct arrives: a **warning** that maps to no Nikaia line is
/// not reported at all.
#[test]
fn a_warning_about_the_generated_file_does_not_reach_the_user() {
    let (dir, entry) = project(
        "silent-preamble",
        &[
            ("plain.nika", "pub fn seven() -> i64 {\n    return 7\n}\n"),
            (
                "main.nika",
                "fn main() {\n\x20   println(f\"{seven()}\")\n}\n",
            ),
            // **A manifest, so `nikaia lower` reads the package** (ADR-286 D1).
            // Without one the entry is compiled on its own, `seven` is
            // declared by nothing in it, and since 0.0.244 that is `NK1117`
            // rather than a lowering `rustc` would have refused.
            (
                "nikaia.toml",
                "[package]\nname = \"silent\"\nversion = \"0.1.0\"\n",
            ),
        ],
    );

    let program = Program::read(&entry).expect("the program reads");
    let lowered = program.emit(Build::default()).expect("it lowers");
    assert!(
        !lowered.rust.contains("use super::*") && !lowered.rust.contains("pub mod "),
        "one namespace is one crate root:\n{}",
        lowered.rust
    );

    // And the whole way through the CLI, which is where the user was told to
    // remove it.
    let ran = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["lower", entry.to_str().expect("utf-8 path")])
        .args([
            "--output",
            dir.join("program.rs").to_str().expect("utf-8 path"),
        ])
        .args(["--no-cache"])
        .env("NIKAIA_CACHE_DIR", dir.join("cache"))
        .output()
        .expect("the nikaia binary runs");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(ran.status.success(), "{said}");
    assert!(
        !said.contains("unused import") && !said.contains("no Nikaia source maps to this"),
        "a warning about the generated file reached the user:\n{said}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// **A type another file declares can be named and built**
/// (issue #229's cross-file type defect, since closed).
///
/// Three things were broken and only the first worked: the *call* was fine, the
/// **type name** resolved to a different type from the one the call handed back,
/// and a struct literal of a type from another file was a parse error. A file
/// could hand out behaviour and not data.
///
/// The package decision (ADR-286 D1) removed the prefix from this case rather
/// than the problem: one namespace, so the names below are bare. What the repair
/// is still for is the cross-*package* boundary, where the prefix comes back.
#[test]
fn a_type_another_file_declares_can_be_named_and_built() {
    let (dir, entry) = project(
        "foreign-type",
        &[
            (
                "pool.nika",
                "pub struct Conn { pub id: i64 }\n\
                 \n\
                 pub fn make() -> Conn {\n\
                 \x20   return Conn { id: 7 }\n\
                 }\n\
                 \n\
                 pub fn label(c: Conn) -> i64 {\n\
                 \x20   return c.id\n\
                 }\n",
            ),
            (
                "main.nika",
                "fn main() {\n\
                 \x20   let made: Conn = make()\n\
                 \x20   let built = Conn { id: 35 }\n\
                 \x20   println(f\"{label(made)} {label(built)}\")\n\
                 }\n",
            ),
        ],
    );
    assert_eq!(run(&entry, Build::default()).trim(), "7 35");
    let _ = std::fs::remove_dir_all(dir);
}

/// **A constant another file declares is read like a function it declares**
/// (ADR-286 D1): one namespace holds the package's `comptime`s too. The
/// program's half of what `tests/sysroot.rs` holds for the tools, where each
/// file bound only its own (found moving the compiler's `views` into Nikaia,
/// #125).
#[test]
fn a_constant_another_file_declares_can_be_read() {
    let (dir, entry) = project(
        "foreign-constant",
        &[
            (
                "limits.nika",
                "pub comptime LIMIT: i64 = 40\n\
                 comptime NAMES: Array[ref String, 2] = [\"a\", \"b\"]\n\
                 \n\
                 pub fn first() -> String {\n\
                 \x20   return NAMES[0].clone()\n\
                 }\n",
            ),
            (
                "main.nika",
                "fn main() {\n\
                 \x20   let n = LIMIT + 2\n\
                 \x20   println(f\"{n} {first()} {NAMES[1]}\")\n\
                 }\n",
            ),
        ],
    );
    assert_eq!(run(&entry, Build::default()).trim(), "42 a b");
    let _ = std::fs::remove_dir_all(dir);
}

/// …and the checker still says no where it should. Three shapes, each answered
/// by `rustc` about a generated file before a foreign struct's fields could be
/// read at all.
#[test]
fn a_foreign_type_is_still_checked() {
    for (line, code, says) in [
        ("let c: i64 = make()", "NK1103", "Conn"),
        ("let c = Conn { nmae: 1 }", "NK1107", "nmae"),
        ("let c = Conn { id: \"seven\" }", "NK1106", "Conn.id"),
    ] {
        let (dir, entry) = project(
            "foreign-type-refused",
            &[
                (
                    "pool.nika",
                    "pub struct Conn { pub id: i64 }\n\
                     \n\
                     pub fn make() -> Conn {\n\
                     \x20   return Conn { id: 7 }\n\
                     }\n",
                ),
                ("main.nika", &format!("fn main() {{\n    {line}\n}}\n")),
            ],
        );
        let program = Program::read(&entry).expect("the program reads");
        let library = Ledger::parse(nikaia::contracts::STD).expect("std's ledger parses");
        let packages = program.package_names();
        let found: Vec<_> = program
            .units
            .iter()
            .flat_map(|unit| {
                nikaia::check::check_program(&unit.parsed, &program.contracts, &library, &packages)
                    .findings
            })
            .collect();
        assert_eq!(found.len(), 1, "{line}: {found:#?}");
        assert_eq!(found[0].code, code, "{line}");
        assert!(
            found[0].message.contains(says),
            "{line}: {}",
            found[0].message
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// **A `Shared` in a field another file declares keeps the atomic count**
/// (issue #143's twice-decided `Shared` slot, since closed).
///
/// The sharing analysis runs once per **file**, and a package of several files is
/// still several runs of it - so neither run sees the whole of such a field: the
/// file declaring it sees the field and not this value, this one the value and not
/// what the field was decided to be. Measured before the fix, both in one
/// generated file:
///
/// ```text
/// pub db: std::sync::Arc<Conn>,      // decided in pool.nika
/// let c: std::rc::Rc<Conn> = …       // decided in main.nika
/// ```
///
/// which `rustc` refused, about a file nobody wrote. The answer is the polarity
/// this analysis already runs on: where it cannot prove that nothing crosses, it
/// does not lower.
#[test]
fn a_shared_in_a_foreign_field_keeps_the_atomic_count() {
    let (dir, entry) = project(
        "foreign-field",
        &[
            (
                "pool.nika",
                "pub struct Conn { pub id: i64 }\n\
                 \n\
                 pub struct Pool { pub db: Shared[Conn] }\n",
            ),
            (
                "main.nika",
                "fn main() {\n\
                 \x20   let c = Shared(Conn { id: 1 })\n\
                 \x20   let p = Pool { db: c }\n\
                 \x20   println(f\"{p.db.id}\")\n\
                 }\n",
            ),
        ],
    );

    // **What has to hold is that the two sides agree**, and since
    // [ADR-061](../../../docs/specification/adr/adr-061.md) D2 they agree the
    // other way round at one user thread: nothing can cross there, so every
    // count in the whole build is plain and no file can disagree with another.
    // At `yes` the floor is what makes them agree, which is what this was
    // written for. Both settings, because one of them proving it is not the
    // property.
    let program = Program::read(&entry).expect("the program reads");
    for build in [Build::default(), Build::parallel()] {
        let lowered = program.emit(build).expect("it lowers");
        let plain = lowered.rust.contains("std::rc::Rc");
        let atomic = lowered.rust.contains("std::sync::Arc");
        assert!(
            plain != atomic,
            "both sides of the field are the same count:\n{}",
            lowered.rust
        );
    }
    // And it runs, which is the only proof the two agreed.
    assert_eq!(run(&entry, Build::default()).trim(), "1");
    let _ = std::fs::remove_dir_all(dir);
}

/// **A `Shared` handed to a function another file declares keeps the atomic
/// count** — the parameter twin of the test above.
///
/// Found by a test written for something else, and it is the same defect one slot
/// over: the declaring file forces its public parameter to the atomic count, this
/// file decides the value on its own, and the two meet in one generated file as
/// `expected Arc<Conn>, found Rc<Conn>`. The floor is therefore about **any slot
/// another file owns**, not about fields.
///
/// The call is written inside a hole on purpose. `println(f"{hold(c)}")` is how
/// somebody would actually write this, and the sharing analysis did not walk into
/// a literal at all — so the handle was handed to a function it never saw, and
/// nothing forced anything. A hole is Nikaia source (ADR-032 D3), and any
/// analysis that stops at a literal is one a hole can be hidden in.
#[test]
fn a_shared_handed_to_a_foreign_function_keeps_the_atomic_count() {
    let (dir, entry) = project(
        "foreign-parameter",
        &[
            (
                "pool.nika",
                "pub struct Conn { pub id: i64 }\n\
                 \n\
                 pub fn hold(c: Shared[Conn]) -> i64 {\n\
                 \x20   return c.id\n\
                 }\n",
            ),
            (
                "main.nika",
                "fn main() {\n\
                 \x20   let c = Shared(Conn { id: 1 })\n\
                 \x20   println(f\"{hold(c)}\")\n\
                 }\n",
            ),
        ],
    );

    // **What has to hold is that the two sides agree**, and since
    // [ADR-061](../../../docs/specification/adr/adr-061.md) D2 they agree the
    // other way round at one user thread: nothing can cross there, so every
    // count in the whole build is plain and no file can disagree with another.
    // At `yes` the floor is what makes them agree, which is what this was
    // written for. Both settings, because one of them proving it is not the
    // property.
    let program = Program::read(&entry).expect("the program reads");
    for build in [Build::default(), Build::parallel()] {
        let lowered = program.emit(build).expect("it lowers");
        let plain = lowered.rust.contains("std::rc::Rc");
        let atomic = lowered.rust.contains("std::sync::Arc");
        assert!(
            plain != atomic,
            "both sides of the call are the same count:\n{}",
            lowered.rust
        );
    }
    assert_eq!(run(&entry, Build::default()).trim(), "1");
    let _ = std::fs::remove_dir_all(dir);
}

/// **A program that uses `HashMap` without importing anything from `std`
/// compiles.**
///
/// The preamble line that makes `std`'s names resolve was written only where the
/// program had a `use std::…` of its own — which is not the same question.
/// `HashMap` is a name the prelude provides and a program may write it without
/// importing anything; such a program lowered to a file where `TrustedMap` (the
/// name a trusted input's map gets, ADR-010 D5) was undeclared, and `rustc` said
/// so about a file nobody wrote (Part III, C.1).
///
/// It is written always now, with `#[allow(unused_imports)]`: what a program uses
/// is not a list the emitter keeps, and it does not need one.
#[test]
fn a_program_that_imports_nothing_may_still_use_a_map() {
    let (dir, entry) = project(
        "no-import",
        &[(
            "main.nika",
            "use std::collections\n\nfn main() {\n\
             \x20   let mut m = collections::HashMap()\n\
             \x20   m.insert(\"a\", 1)\n\
             \x20   println(f\"{m.len()}\")\n\
             }\n",
        )],
    );
    assert_eq!(run(&entry, Build::default()).trim(), "1");
    let _ = std::fs::remove_dir_all(dir);
}

/// **Names are not brought in, and the two forms that try to get a sentence**
/// ([ADR-286](../../../docs/specification/adr/adr-286.md) D11).
///
/// Both are in every language that has them, so a reader will write one — which
/// is the same ground [ADR-277](../../../docs/specification/adr/adr-277.md)
/// stands on for `fn: …`: a form somebody will reach for deserves a sentence
/// rather than a parse error at the brace.
///
/// The arm has to come **first** in the rule, because the plain arm matches
/// `use http` and leaves the rest to fail as the next item — at the same place,
/// with nothing to say. And it `peek`s the brace rather than consuming it: a
/// `fail` reports at the position it runs at, and consuming the brace first puts
/// that after the implicit whitespace, which had the caret under the line below.
#[test]
fn a_use_that_brings_a_name_in_says_to_write_the_prefix() {
    for (source, says) in [
        (
            "use http::{Request}\n\nfn main() { }\n",
            "`use` can't bring in single names.",
        ),
        (
            "use http::*\n\nfn main() { }\n",
            "`use` can't bring in every name at once.",
        ),
    ] {
        let error = nikaia::parser::parse_to_ast(source)
            .err()
            .unwrap_or_else(|| panic!("{source} must be refused"));
        let finding = nikaia::diagnostics::refused_finding(&error)
            .unwrap_or_else(|| panic!("a parse error carries its finding: {error:#}"));
        let message = nikaia::diagnostics::render_finding(finding, "app.nika", source);
        assert!(message.contains(&format!("error: {says}")), "{message}");
        assert!(
            message.contains("= help: Write `use http`"),
            "and what to write: {message}"
        );
        // The caret is on the brace or the star, not on the line below it.
        assert!(message.contains("--> app.nika:1:11"), "{message}");
    }

    // …and the three forms that are not this are untouched.
    for source in [
        "use http\n\nfn main() { }\n",
        "use http as h\n\nfn main() { }\n",
        "use std::fs\n\nfn main() { }\n",
    ] {
        nikaia::parser::parse_to_ast(source).unwrap_or_else(|e| panic!("{source}\n{e:#}"));
    }
}

/// **A `comptime` reaches across a file boundary** (0.0.115).
///
/// The *permission* to run a callee had been program-wide from the start — it
/// is two ledger columns ([ADR-287](../../../docs/specification/adr/adr-287.md)
/// D1, D2) and a program's ledger is absorbed from its units'. What was not was
/// the **body**, and no column could carry one: a ledger records what a caller
/// has to know about a function it *cannot see the body of*, which is the
/// opposite of what this needs. So the evaluator was handed the files.
///
/// **Run and not read**, because the hazard is an interner: every
/// `parse_to_ast` builds its own, so a symbol from another file resolves to
/// nothing — or to the wrong text — when read with this one's. A struct
/// literal, an interpolation and a three-file chain are exactly the shapes
/// that would show it.
#[test]
fn a_comptime_calls_across_a_file_boundary() {
    let (_, entry) = project(
        "comptime-across-files",
        &[
            (
                "main.nika",
                "comptime TOTAL: i64 = Point { x: 3, y: 4 }.doubled().sum()\n\
                 comptime LABEL: ref String = banner(\"nikaia\")\n\
                 comptime CHAINED: i64 = through(5)\n\
                 \n\
                 fn main() {\n\
                 \x20   println(f\"{TOTAL} {LABEL} {CHAINED}\")\n\
                 }\n",
            ),
            (
                "geometry.nika",
                "pub struct Point { pub x: i64, pub y: i64 }\n\
                 \n\
                 impl Point {\n\
                 \x20   pub fn doubled(ref self) -> Point sync {\n\
                 \x20       return Point { x: self.x * 2, y: self.y * 2 }\n\
                 \x20   }\n\
                 \x20   pub fn sum(ref self) -> i64 sync { return self.x + self.y }\n\
                 }\n\
                 \n\
                 pub fn banner(name: ref String) -> String sync { return f\"[{name}]\" }\n",
            ),
            (
                "deep.nika",
                "pub fn through(n: i64) -> i64 sync { return inner(n) + 1 }\n",
            ),
            (
                "inner.nika",
                "pub fn inner(n: i64) -> i64 sync { return n * 10 }\n",
            ),
        ],
    );
    assert_eq!(run(&entry, Build::default()).trim(), "14 [nikaia] 51");
}

/// **And a constant a foreign body reads is answered from *its own* file**
/// (0.0.116).
///
/// 0.0.115 refused it, on the grounds that the checker's scope is the file
/// being checked and answering from it would be a **wrong value** rather than a
/// missing one — [ADR-010](../../../docs/specification/adr/adr-010.md) D1's
/// direction. The premise was right and the conclusion was one step short: the
/// scope to read is the one the **body** came from, and while a foreign body is
/// being run that file is exactly what the evaluator is holding.
#[test]
fn a_constant_a_foreign_body_reads_comes_from_its_own_file() {
    // **A real project**, because `nikaia lower` outside one is a single file
    // (ADR-286 D1) and would meet a different wall: nothing declares `scaled`
    // at all there.
    let (dir, _) = project(
        "comptime-foreign-global",
        &[
            (
                "nikaia.toml",
                "[package]\nname = \"foreign-global\"\nversion = \"0.1.0\"\n\
                 \n[build]\nuser-parallelism = \"no\"\n",
            ),
            (
                "src/main.nika",
                "comptime X: i64 = scaled(2)\n\nfn main() { println(f\"{X}\") }\n",
            ),
            (
                "src/globals.nika",
                "comptime SCALE: i64 = 7\n\
                 \n\
                 pub fn scaled(n: i64) -> i64 sync { return n * SCALE }\n",
            ),
        ],
    );
    let entry = dir.join("src/main.nika");
    let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["lower", entry.to_str().expect("utf-8"), "--no-cache"])
        .output()
        .expect("the compiler runs");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{said}");
    let rust = std::fs::read_to_string(dir.join("src/main.rs")).expect("the lowering");
    assert!(rust.contains("const X: i64 = 14;"), "{rust}");
    let _ = std::fs::remove_dir_all(&dir);
}
