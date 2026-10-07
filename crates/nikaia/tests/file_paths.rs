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

const JOINED: &str = "use std::fs\n\
     \n\
     fn main() {\n\
     \x20   let p: fs::Path = \"data.csv\"\n\
     \x20   let n = 3\n\
     \x20   let bak: fs::Path = f\"{p}.{n}.bak\"\n\
     \x20   let there = fs::exists(f\"{p}.bak\", fs::Root::Anywhere) catch { return }\n\
     \x20   println(f\"{bak.display()} {there}\")\n\
     }\n";

/// **An `f"…"` with a file's name in it is a name** (ADR-319 D4): the parts
/// are joined in the platform's encoding, the name by its bytes and the rest
/// by its text, where a `let` or an argument asks for a `Path`.
#[test]
fn an_interpolated_name_is_joined_by_its_bytes() {
    let rust = lowered(JOINED);
    assert!(
        rust.contains(
            "nikaia_std::fs::path::joined(&[std::convert::AsRef::<std::ffi::OsStr>::as_ref(&(p)), \
             std::ffi::OsStr::new(\".\"), \
             std::ffi::OsStr::new(&std::string::ToString::to_string(&(n))), \
             std::ffi::OsStr::new(\".bak\")])"
        ),
        "{rust}"
    );
    let dir = common::scratch_dir("file-path-joined");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run the program");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "data.csv.3.bak false\n"
    );
}

/// **A name is printed as its bytes** (ADR-319 D4): `print`, `println`,
/// `eprint` and `eprintln` take text or a name, and a name - alone or in an
/// `f"…"` - reaches the output unchanged.
#[test]
fn a_name_is_printed_as_its_bytes() {
    let source = "use std::fs\n\
         \n\
         fn main() {\n\
         \x20   let p: fs::Path = \"data.csv\"\n\
         \x20   println(f\"found: {p}\")\n\
         \x20   print(p)\n\
         \x20   println(\"\")\n\
         \x20   eprintln(f\"{p}!\")\n\
         }\n";
    let rust = lowered(source);
    assert!(
        rust.contains("nikaia_std::fs::path::print(&(p), false, false);"),
        "{rust}"
    );
    let dir = common::scratch_dir("file-path-print");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
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
        "found: data.csv\ndata.csv\n"
    );
    assert_eq!(String::from_utf8_lossy(&run.stderr), "data.csv!\n");
}

/// The findings a source gets, with the code and the help of each.
fn refusals(source: &str) -> Vec<(String, String)> {
    let parsed = parse_to_ast(source).expect("the source parses");
    common::checked(&parsed, &common::infer(&parsed), &library())
        .findings
        .into_iter()
        .map(|f| (f.code.to_string(), f.help.unwrap_or_default()))
        .collect()
}

/// **A name in an `f"…"` used as text is `NK1201`, at the hole** (ADR-319 D4):
/// in a `let`, an argument, an assignment, a `return` and a `match` arm beside
/// text, each with both ways out in the help.
#[test]
fn a_name_in_a_text_interpolation_is_refused_at_the_hole() {
    for (body, what) in [
        ("    let s: String = f\"name {p}\"\n", "a let"),
        ("    let s = take(f\"name {p}\")\n", "an argument"),
        (
            "    let mut s = \"\"\n    s = f\"name {p}\"\n",
            "an assignment",
        ),
        (
            "    let s = match 1 {\n        1 => f\"name {p}\"\n        else => \"other\".clone()\n    }\n",
            "a match arm",
        ),
    ] {
        let source = format!(
            "use std::fs\n\nfn take(s: String) -> String {{\n    return s\n}}\n\n\
             fn main() {{\n    let p: fs::Path = \"a.txt\"\n{body}}}\n"
        );
        let found = refusals(&source);
        let hit = found.iter().find(|(code, _)| code == "NK1201");
        let (_, help) = hit.unwrap_or_else(|| panic!("{what}: {found:#?}"));
        assert!(help.contains("{p.to_text()}"), "{what}: {help}");
        assert!(help.contains("{p.display()}"), "{what}: {help}");
        assert!(
            !found
                .iter()
                .any(|(code, _)| code == "NK1103" || code == "NK1102"),
            "{what}: {found:#?}"
        );
    }
    let returned = "use std::fs\n\nfn named(p: ref fs::Path) -> String {\n    return f\"name {p}\"\n}\n\nfn main() {\n}\n";
    assert!(
        refusals(returned).iter().any(|(code, _)| code == "NK1201"),
        "{:#?}",
        refusals(returned)
    );
}

/// **A name in an `f"…"` that builds a name, or one that is printed, is
/// not refused** (D4): the use asks for a name, or writes its bytes.
#[test]
fn a_name_in_an_interpolation_that_builds_a_name_is_not_refused() {
    let source = "use std::fs\n\nfn main() {\n\
                  \x20   let p: fs::Path = \"a.txt\"\n\
                  \x20   let bak: fs::Path = f\"{p}.bak\"\n\
                  \x20   println(f\"found: {p} and {bak}\")\n\
                  \x20   let shown: String = f\"name {p.display()}\"\n\
                  \x20   println(shown)\n\
                  }\n";
    assert!(refusals(source).is_empty(), "{:#?}", refusals(source));
}

/// **`IoError`'s name variants carry an `fs::Path`, and so does
/// `fs::Root::Dir`** (ADR-319 D7): a handler that writes the name into text
/// says how, and `display()` shows it.
#[test]
fn an_io_errors_name_is_a_file_name() {
    let refused = "use std::fs\nuse std::io\n\nfn main() {\n\
                   \x20   let text = fs::read_to_string(\"nope.txt\", fs::Root::Dir(\".\")) catch {\n\
                   \x20       match error {\n\
                   \x20           io::IoError::NotFound(p) => f\"no file: {p}\"\n\
                   \x20           else => \"other\".clone()\n\
                   \x20       }\n\
                   \x20   }\n\
                   \x20   println(text)\n\
                   }\n";
    assert!(
        refusals(refused).iter().any(|(code, _)| code == "NK1201"),
        "{:#?}",
        refusals(refused)
    );
    let shown = refused.replace("{p}", "{p.display()}");
    assert!(refusals(&shown).is_empty(), "{:#?}", refusals(&shown));
}

/// The codes a source gets.
fn codes(source: &str) -> Vec<String> {
    refusals(source).into_iter().map(|(code, _)| code).collect()
}

/// **`to_text` fails only for a name the system handed over** (ADR-319 D5):
/// made from text - a literal, text, an `f"…"` or `join` over such names - it
/// needs no `throws`; a parameter and what `walk` lists do.
#[test]
fn to_text_throws_by_the_names_source() {
    let made = "use std::fs\n\nfn shown() -> String {\n\
                \x20   let p: fs::Path = \"a.txt\"\n\
                \x20   let name = \"b\"\n\
                \x20   let q: fs::Path = name\n\
                \x20   let bak: fs::Path = f\"{p}.{q}.bak\"\n\
                \x20   let joined = p.join(\"c\")\n\
                \x20   return f\"{p.to_text()} {q.to_text()} {bak.to_text()} {joined.to_text()}\"\n\
                }\n\nfn main() {\n    println(shown())\n}\n";
    assert!(codes(made).is_empty(), "{:#?}", refusals(made));

    let given = "use std::fs\n\nfn shown(p: ref fs::Path) -> String {\n\
                 \x20   return p.to_text()\n\
                 }\n\nfn main() {\n}\n";
    assert!(
        codes(given).contains(&"NK2605".to_string()),
        "{:#?}",
        refusals(given)
    );

    let walked = "use std::fs\n\nfn names() -> String throws {\n\
                  \x20   let mut out: String = \"\"\n\
                  \x20   for name in fs::walk(\".\", fs::Root::Dir(\"tree\")) {\n\
                  \x20       out = name.to_text()\n\
                  \x20   }\n\
                  \x20   return out\n\
                  }\n\nfn main() {\n}\n";
    assert!(codes(walked).is_empty(), "{:#?}", refusals(walked));
    let unsaid = walked.replace(" -> String throws {", " -> String {");
    assert!(
        codes(&unsaid).contains(&"NK2605".to_string()),
        "{:#?}",
        refusals(&unsaid)
    );
}

/// **The same verdict for every target** (D5): the source decides, so the
/// program text that needs no `throws` lowers to the call that cannot fail.
#[test]
fn a_name_made_from_text_lowers_to_the_call_that_cannot_fail() {
    let rust = lowered(
        "use std::fs\n\nfn shown() -> String {\n\
         \x20   let p: fs::Path = \"a.txt\"\n\
         \x20   return p.to_text()\n\
         }\n\nfn main() {\n    println(shown())\n}\n",
    );
    assert!(rust.contains("nikaia_std::fs::path::text_of("), "{rust}");
    assert!(!rust.contains("nikaia_std::fs::path::to_text("), "{rust}");
}
