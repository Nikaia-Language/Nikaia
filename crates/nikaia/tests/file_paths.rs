//! **A file's name is `fs::Path`, the platform's own bytes**
//! ([ADR-319](../../../docs/specification/adr/adr-319.md), #449).
//!
//! The type, and text standing wherever one is asked (D2): an owned name is
//! made from the text, and a view of one borrows it without a copy.

mod common;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

const PROGRAM: &str = "use std::fs\n\
     \n\
     enum ConfigError {\n\
     \x20   NotFound(fs::Path),\n\
     \x20   Invalid(String),\n\
     }\n\
     \n\
     struct Job {\n\
     \x20   at: fs::Path,\n\
     }\n\
     \n\
     fn owned(p: fs::Path) -> fs::Path {\n\
     \x20   return p\n\
     }\n\
     \n\
     fn viewed(p: ref fs::Path) -> bool {\n\
     \x20   return true\n\
     }\n\
     \n\
     fn main() {\n\
     \x20   let p: fs::Path = \"a.txt\"\n\
     \x20   let name = \"b.txt\"\n\
     \x20   let q = owned(name)\n\
     \x20   let job = Job { at: \"c.txt\" }\n\
     \x20   let e = ConfigError::NotFound(\"d.txt\")\n\
     \x20   println(f\"{viewed(\"e.txt\")} {viewed(name)}\")\n\
     }\n";

fn library() -> nikaia::contracts::Ledger {
    use nikaia::contracts::LedgerOps;
    nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger")
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust
}

/// **The type, in each position a value is put** (D1, D2): a `let`, an
/// argument, a field and a variant's part take text, and Part I 7.1's
/// `NotFound(Path)` is a type `std` declares.
#[test]
fn text_stands_where_a_file_name_is_asked() {
    let parsed = parse_to_ast(PROGRAM).expect("the source parses");
    let found = common::checked(&parsed, &common::infer(&parsed), &library()).findings;
    assert!(
        found
            .iter()
            .all(|f| f.severity != nikaia::check::Severity::Error),
        "{found:#?}"
    );
    let rust = lowered(PROGRAM);
    for written in [
        "fn owned(p: fs::Path) -> fs::Path",
        "let p: fs::Path = std::path::PathBuf::from(\"a.txt\");",
        "owned(std::path::PathBuf::from(name))",
        "Job { at: std::path::PathBuf::from(\"c.txt\") }",
        "ConfigError::NotFound(std::path::PathBuf::from(\"d.txt\"))",
    ] {
        assert!(rust.contains(written), "{written}\n{rust}");
    }
}

/// **A view of a name is a `&std::path::Path`, lent by text without a copy**
/// (D2): no `PathBuf` is made for it.
#[test]
fn a_view_of_a_file_name_borrows_the_text() {
    let rust = lowered(PROGRAM);
    assert!(
        rust.contains("fn viewed(p: &std::path::Path) -> bool"),
        "{rust}"
    );
    assert!(
        rust.contains("viewed(std::path::Path::new(&(\"e.txt\")))")
            && rust.contains("viewed(std::path::Path::new(&(name)))"),
        "{rust}"
    );
}

/// **And the program runs.**
#[test]
fn a_program_with_file_names_compiles_and_runs() {
    let dir = common::scratch_dir("file-paths");
    let file = dir.join("main.rs");
    std::fs::write(&file, lowered(PROGRAM)).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(String::from_utf8_lossy(&run.stdout), "true true\n");
}

/// **Something that is not text is refused as before.**
#[test]
fn a_number_is_not_a_file_name() {
    let source = "use std::fs\n\nfn main() {\n    let n: i64 = 3\n    let p: fs::Path = n\n}\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let found = common::checked(&parsed, &common::infer(&parsed), &library()).findings;
    assert!(found.iter().any(|f| f.code == "NK1103"), "{found:#?}");
}

/// **`std::fs` asks for a file's name** (ADR-319 D2, step 3): text and a
/// `fs::Path` both stand there, and neither is copied - text is borrowed as a
/// view, and a `Path` is lent.
#[test]
fn std_fs_takes_a_file_name_and_text_stands_there() {
    let source = "use std::fs\n\
         \n\
         fn main() {\n\
         \x20   let dir = \".\"\n\
         \x20   let name: String = \"x.txt\"\n\
         \x20   let p: fs::Path = \"x.txt\"\n\
         \x20   let a = fs::exists(name, fs::Root::Anywhere) catch { return }\n\
         \x20   let b = fs::exists(f\"{dir}/x.txt\", fs::Root::Anywhere) catch { return }\n\
         \x20   let c = fs::exists(p, fs::Root::Anywhere) catch { return }\n\
         \x20   println(f\"{a} {b} {c}\")\n\
         }\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let found = common::checked(&parsed, &common::infer(&parsed), &library()).findings;
    assert!(
        found
            .iter()
            .all(|f| f.severity != nikaia::check::Severity::Error),
        "{found:#?}"
    );
    let rust = lowered(source);
    assert!(
        rust.contains("fs::exists(std::path::Path::new(&(name)), &fs::Root::Anywhere)"),
        "{rust}"
    );
    assert!(
        rust.contains(
            "fs::exists(std::path::Path::new(&(format!(\"{}/x.txt\", dir))), &fs::Root::Anywhere)"
        ),
        "{rust}"
    );
    assert!(
        rust.contains("fs::exists(&p, &fs::Root::Anywhere)"),
        "{rust}"
    );
    assert!(!rust.contains("PathBuf::from(name)"), "{rust}");
}

const OPERATIONS: &str = "use std::fs\n\
     \n\
     fn main() {\n\
     \x20   let p: fs::Path = \"dir/data.csv\"\n\
     \x20   let q: fs::Path = \"dir//data.csv\"\n\
     \x20   let csv = p.ends_with(\".csv\")\n\
     \x20   let dir = p.starts_with(\"dir/\")\n\
     \x20   let bak = p.with_extension(\"bak\")\n\
     \x20   let up = p.parent() ?? \".\"\n\
     \x20   let joined = up.join(\"other.txt\")\n\
     \x20   let name = p.file_name() ?? \"-\"\n\
     \x20   let ext = p.extension() ?? \"-\"\n\
     \x20   println(f\"{csv} {dir} {bak.display()} {joined.display()} {name.display()} {ext.display()}\")\n\
     \x20   println(f\"{p == \"dir/data.csv\"} {p == q} {\"x\" != p}\")\n\
     }\n";

/// **What a program does with a name, it does on the `Path`** (ADR-319 D3):
/// each operation is `std`'s function, because Rust's `Path` has methods of
/// these names that compare whole components - `"data.csv".ends_with(".csv")`
/// would be false there.
#[test]
fn a_file_names_operations_are_stds_and_compare_bytes() {
    let rust = lowered(OPERATIONS);
    for written in [
        "nikaia_std::fs::path::ends_with(&(p), std::path::Path::new(&(\".csv\")))",
        "nikaia_std::fs::path::with_extension(&(p), std::path::Path::new(&(\"bak\")))",
        "nikaia_std::fs::path::same(&(p), &(\"dir/data.csv\"))",
        "!nikaia_std::fs::path::same(&(\"x\"), &(p))",
    ] {
        assert!(rust.contains(written), "{written}\n{rust}");
    }
}

/// **And they run**: `==` is byte for byte, so `dir//data.csv` is another
/// name.
#[test]
fn a_file_names_operations_run() {
    let dir = common::scratch_dir("file-path-operations");
    let file = dir.join("main.rs");
    std::fs::write(&file, lowered(OPERATIONS)).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "true true dir/data.bak dir/other.txt data.csv csv\ntrue false true\n"
    );
}
