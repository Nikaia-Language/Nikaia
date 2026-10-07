//! **Attributes** (ADR-331 D1-D4, Part II 10.3, #496 steps 2-3): `@path(…)`
//! before a declaration is a value of a struct `@meta::Attribute` marks,
//! where its mark says it may stand, with arguments checked as a call's.

mod common;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn codes(source: &str) -> Vec<String> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = common::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    common::checked(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .map(|f| f.code.to_string())
        .collect()
}

const JSON: &str = "pub enum Case {\n\
     \x20   Camel,\n\
     \x20   Snake,\n\
     }\n\
     \n\
     /// The name a field has on the wire.\n\
     @meta::Attribute(field)\n\
     pub struct Name {\n\
     \x20   pub value: String,\n\
     \x20   pub case: Case = Case::Camel,\n\
     }\n\
     \n\
     @meta::Attribute(field, struct; repeatable: true)\n\
     pub struct Tag {\n\
     \x20   pub text: String,\n\
     }\n\
     \n\
     pub struct Plain {\n\
     \x20   pub value: String,\n\
     }\n";

fn with(declaration: &str) -> String {
    format!("{JSON}\n{declaration}\n\nfn main() {{\n}}\n")
}

/// The mark and its use on a field build, by position, by name after the
/// `;`, and with a variant by its name alone (D3).
#[test]
fn a_marked_struct_stands_before_a_field() {
    let source = with(
        "@Tag(\"user\")\n\
         struct User {\n\
         \x20   /// When the user was created.\n\
         \x20   @Name(\"createdAt\")\n\
         \x20   created_at: i64,\n\
         \x20   @Name(\"user_name\"; case: snake)\n\
         \x20   @Tag(\"a\")\n\
         \x20   @Tag(\"b\")\n\
         \x20   name: String,\n\
         }",
    );
    assert_eq!(codes(&source), Vec::<String>::new());
}

/// `NK1235`: a place the mark does not name, and a struct with no mark.
#[test]
fn an_attribute_out_of_place_is_refused() {
    assert_eq!(codes(&with("@Name(\"x\")\nfn f() {\n}")), vec!["NK1235"]);
    assert_eq!(
        codes(&with(
            "struct User {\n    @Plain(\"x\")\n    name: String,\n}"
        )),
        vec!["NK1235"]
    );
    assert_eq!(
        codes(&with("@meta::Attribute(field)\nfn f() {\n}")),
        vec!["NK1235"]
    );
}

/// `NK1236`: twice before one field, without `repeatable: true`.
#[test]
fn an_attribute_twice_is_refused_unless_repeatable() {
    let source = with(
        "struct User {\n\
         \x20   @Name(\"a\")\n\
         \x20   @Name(\"b\")\n\
         \x20   name: String,\n\
         }",
    );
    assert_eq!(codes(&source), vec!["NK1236"]);
}

/// D4: the fields without a default by position, those with one by name;
/// a wrong count is `NK1101`, an unknown option `NK1109`, a wrong value
/// `NK1102`.
#[test]
fn the_arguments_are_checked_as_a_calls() {
    let field = |attribute: &str| {
        with(&format!(
            "struct User {{\n    {attribute}\n    name: String,\n}}"
        ))
    };
    assert_eq!(codes(&field("@Name(; case: snake)")), vec!["NK1101"]);
    assert_eq!(codes(&field("@Name(\"x\"; cas: snake)")), vec!["NK1109"]);
    assert_eq!(codes(&field("@Name(5)")), vec!["NK1102"]);
    assert_eq!(codes(&field("@Nmae(\"x\")")), vec!["NK1135"]);
}

/// An attribute is checked and carried; nothing of it reaches the program,
/// which builds and runs as it would without it.
#[test]
fn an_attribute_changes_nothing_the_program_does() {
    let source = format!(
        "{JSON}\n\
         struct User {{\n\
         \x20   @Name(\"createdAt\")\n\
         \x20   created_at: i64,\n\
         }}\n\
         \n\
         fn main() {{\n\
         \x20   let u = User {{ created_at: 7 }}\n\
         \x20   println(f\"{{u.created_at}}\")\n\
         }}\n"
    );
    assert!(codes(&source).is_empty(), "{:?}", codes(&source));
    let dir = common::scratch_dir("attributes-run");
    let file = dir.join("main.nika");
    std::fs::write(&file, &source).expect("the source");
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .arg("run")
        .arg(&file)
        .output()
        .expect("the nikaia binary runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "7\n",
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}
