//! **A struct's field may have a default** (ADR-331 D5, D8, #496 step 1): a
//! build-time value a literal that leaves the field out takes, evaluated once
//! where the struct is declared, recorded in the ledger, and refused with
//! `NK1234` where a literal leaves out a field that has none.

mod common;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program_reading};
use nikaia::parser::parse_to_ast;

fn codes(source: &str) -> Vec<String> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = common::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    common::checked(&parsed, &own, &library)
        .findings
        .into_iter()
        .map(|f| f.code.to_string())
        .collect()
}

fn ran(source: &str) -> String {
    assert!(codes(source).is_empty(), "{:?}", codes(source));
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program_reading(&parsed, Build::default(), &common::reads())
        .expect("the source lowers")
        .rust;
    let dir = common::scratch_dir("field-defaults");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8_lossy(&run.stdout).to_string()
}

const PAGE: &str = "pub struct Page {\n\
     \x20   pub size: i64 = 50,\n\
     \x20   pub cursor: String?,\n\
     \x20   pub label: String = \"all = \\\"x\\\"\",\n\
     \x20   pub limit: i64? = 7,\n\
     \x20   pub step: i64 = 3 * 4,\n\
     }\n";

/// `Page { cursor: null }` has a `size` of 50; a literal, text, a value into
/// a `T?` and a computed default each reach the literal that leaves them out.
#[test]
fn a_field_left_out_takes_its_default() {
    let source = format!(
        "{PAGE}\n\
         fn main() {{\n\
         \x20   let p = Page {{ cursor: null }}\n\
         \x20   let q = Page {{ cursor: \"c\", size: 10 }}\n\
         \x20   println(f\"{{p.size}} {{p.label}} {{p.limit ?? 0}} {{p.step}} {{q.size}}\")\n\
         }}\n"
    );
    assert_eq!(ran(&source), "50 all = \"x\" 7 12 10\n");
}

/// A literal that leaves out a field without a default is `NK1234`.
#[test]
fn a_field_without_a_default_may_not_be_left_out() {
    let source = format!(
        "{PAGE}\n\
         fn main() {{\n\
         \x20   let p = Page {{ size: 1 }}\n\
         \x20   println(f\"{{p.size}}\")\n\
         }}\n"
    );
    assert_eq!(codes(&source), vec!["NK1234".to_string()]);
}

/// A default of the wrong type is refused at the default, as a `comptime` of
/// the wrong type is (`NK1166`).
#[test]
fn a_default_of_the_wrong_type_is_refused() {
    let source = "struct Page {\n\
         \x20   size: i64 = \"x\",\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let p = Page {}\n\
         \x20   println(f\"{p.size}\")\n\
         }\n";
    assert_eq!(codes(source), vec!["NK1166".to_string()]);
}

/// A variant is built whole: its fields take no default.
#[test]
fn a_variant_field_takes_no_default() {
    let source = "enum Shape {\n\
         \x20   Spot { x: i64 = 1 },\n\
         }\n\
         \n\
         fn main() {\n\
         }\n";
    let refused = parse_to_ast(source).expect_err("the parse refuses it");
    assert!(
        refused
            .to_string()
            .contains("variant's field has no default"),
        "{refused}"
    );
}

/// D8: the ledger records each default's value after the type, and reads it
/// back, text with its quotes and a computed value as what it came to.
#[test]
fn a_default_is_recorded_in_the_ledger_and_read_back() {
    let parsed = parse_to_ast(PAGE).expect("the source parses");
    let written = common::infer(&parsed).render();
    assert!(written.contains("pub size: i64 = 50"), "{written}");
    assert!(written.contains("pub step: i64 = 12"), "{written}");
    let read = Ledger::parse(&written).expect("the ledger reads back");
    assert_eq!(read.render(), written);
    let page = read.types.get("Page").expect("Page is recorded");
    let label = page
        .fields
        .iter()
        .find(|f| f.name == "label")
        .expect("label");
    assert_eq!(label.default, "\"all = \\\"x\\\"\"");
}

/// **A dependency adds a defaulted field, and a consumer's literal still
/// builds** (ADR-331 D5, D8): the value crosses in the library's ledger.
#[test]
fn a_default_reaches_another_package() {
    let dir = common::scratch_dir("field-defaults-across");
    let lib = "pub struct Page {\n\
         \x20   pub cursor: String?,\n\
         \x20   pub size: i64 = 25 * 2,\n\
         }\n\
         \n\
         fn main() {\n\
         }\n";
    let app = "fn main() {\n\
         \x20   let p = lib::Page { cursor: null }\n\
         \x20   println(f\"{p.size}\")\n\
         }\n";
    for (package, source, manifest) in [
        (
            "lib",
            lib,
            "[package]\nname = \"lib\"\nversion = \"0.1.0\"\n",
        ),
        (
            "app",
            app,
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nlib = { path = \"../lib\" }\n",
        ),
    ] {
        std::fs::create_dir_all(dir.join(package).join("src")).expect("the package");
        std::fs::write(dir.join(package).join("nikaia.toml"), manifest).expect("a manifest");
        std::fs::write(dir.join(package).join("src/main.nika"), source).expect("a source");
    }
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(dir.join("app"))
        .arg("run")
        .output()
        .expect("the nikaia binary runs");
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(stdout, "50\n", "{stderr}");
}
