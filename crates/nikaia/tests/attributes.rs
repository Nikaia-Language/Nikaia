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

fn run_file(purpose: &str, source: &str) -> String {
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.nika");
    std::fs::write(&file, source).expect("the source");
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .arg("run")
        .arg(&file)
        .output()
        .expect("the nikaia binary runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).to_string()
}

/// **D6: `field.attribute(X)` and `.attributes(X)`**, a constant per
/// unrolled turn: the JSON-name generator #496 names.
#[test]
fn a_json_name_generator_reads_the_attributes() {
    let source = format!(
        "{JSON}\n\
         struct User {{\n\
         \x20   @Name(\"createdAt\")\n\
         \x20   @Tag(\"a\")\n\
         \x20   @Tag(\"b\")\n\
         \x20   created_at: i64,\n\
         \x20   name: String,\n\
         }}\n\
         \n\
         fn wire[T: Struct](value: T) {{\n\
         \x20   for field in T::fields {{\n\
         \x20       let wire = field.attribute(Name)?.value ?? field.name\n\
         \x20       let tags = field.attributes(Tag)\n\
         \x20       println(f\"{{wire}} {{tags.len()}}\")\n\
         \x20   }}\n\
         }}\n\
         \n\
         fn main() {{\n\
         \x20   wire(User {{ created_at: 1, name: \"x\" }})\n\
         }}\n"
    );
    assert_eq!(
        run_file("attributes-read", &source),
        "createdAt 2\nname 0\n"
    );
}

/// **D5: `field.default`**, its default as a `T?`.
#[test]
fn a_reflected_field_answers_its_default() {
    let source = "struct Page {\n\
         \x20   size: i64 = 50,\n\
         \x20   step: i64,\n\
         }\n\
         \n\
         fn defaults[T: Struct](value: T) {\n\
         \x20   for field in T::fields {\n\
         \x20       let d = field.default ?? 0\n\
         \x20       println(f\"{field.name} {d}\")\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   defaults(Page { step: 2 })\n\
         }\n";
    assert_eq!(run_file("attributes-default", source), "size 50\nstep 0\n");
}

/// Reading something that is no attribute is `NK1235`; an argument that
/// cannot run while the program is built is refused as a default is
/// (`NK1152`).
#[test]
fn what_cannot_be_read_is_refused() {
    let read = with(
        "struct User {\n    name: String,\n}\n\n\
         fn wire[T: Struct](value: T) {\n\
         \x20   for field in T::fields {\n\
         \x20       let p = field.attribute(Plain)\n\
         \x20   }\n\
         }",
    );
    assert_eq!(codes(&read), vec!["NK1235"]);
    let loud = with(
        "fn loud() -> String {\n\
         \x20   println(\"side effect\")\n\
         \x20   return \"x\"\n\
         }\n\
         \n\
         struct User {\n    @Name(loud())\n    name: String,\n}",
    );
    assert_eq!(codes(&loud), vec!["NK1152"]);
}

/// **D8: the ledger carries the mark and each field's attributes**, as the
/// values they came to, and reads them back.
#[test]
fn the_ledger_carries_marks_and_attributes() {
    let source = with(
        "struct User {\n\
         \x20   @Name(\"createdAt\"; case: snake)\n\
         \x20   created_at: i64,\n\
         }",
    );
    let parsed = parse_to_ast(&source).expect("the source parses");
    let written = common::infer(&parsed).render();
    assert!(
        written.contains("attribute = \"field, struct; repeatable\""),
        "{written}"
    );
    assert!(
        written.contains(
            "field_attributes = [\"created_at: Name { value: \\\"createdAt\\\", case: Case::Snake }\"]"
        ),
        "{written}"
    );
    let read = Ledger::parse(&written).expect("the ledger reads back");
    assert_eq!(read.render(), written);
}

/// **Across packages** (D8, #496 step 5): an attribute a dependency declares
/// is used here, and a dependency's type's attributes are read here - under
/// the names the consumer writes, `lib::Name`.
#[test]
fn attributes_cross_into_another_package() {
    let dir = common::scratch_dir("attributes-across");
    let lib = "pub enum Case {\n\
         \x20   Camel,\n\
         \x20   Snake,\n\
         }\n\
         \n\
         @meta::Attribute(field)\n\
         pub struct Name {\n\
         \x20   pub value: String,\n\
         \x20   pub case: Case = Case::Camel,\n\
         }\n\
         \n\
         pub struct User {\n\
         \x20   @Name(\"createdAt\"; case: snake)\n\
         \x20   pub created_at: i64,\n\
         \x20   pub name: String,\n\
         }\n\
         \n\
         fn main() {\n\
         }\n";
    let app = "struct Item {\n\
         \x20   @lib::Name(\"itemId\")\n\
         \x20   item_id: i64,\n\
         }\n\
         \n\
         fn wire[T: Struct](value: T) {\n\
         \x20   for field in T::fields {\n\
         \x20       println(field.attribute(lib::Name)?.value ?? field.name)\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   wire(Item { item_id: 1 })\n\
         \x20   wire(lib::User { created_at: 1, name: \"x\" })\n\
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
    assert_eq!(stdout, "itemId\ncreatedAt\nname\n", "{stderr}");
}

/// **A type's own attributes, and a variant's** (D6): `T::attribute(X)` makes
/// a copy per type as a walk does, and `variant.attribute(X)` is read per
/// turn of `T::variants`. The ledger writes both and reads them back (D8).
#[test]
fn a_types_and_a_variants_attributes_are_read() {
    let source = "@meta::Attribute(struct, enum, variant)\n\
         pub struct Tag {\n\
         \x20   pub text: String,\n\
         }\n\
         \n\
         @Tag(\"users\")\n\
         struct User {\n\
         \x20   name: String,\n\
         }\n\
         \n\
         enum Op {\n\
         \x20   @Tag(\"plus\")\n\
         \x20   Add,\n\
         \x20   Sub,\n\
         }\n\
         \n\
         fn table[T: Struct](value: T) {\n\
         \x20   println(T::attribute(Tag)?.text ?? \"none\")\n\
         }\n\
         \n\
         fn ops[T: Enum](value: T) {\n\
         \x20   for variant in T::variants {\n\
         \x20       println(variant.attribute(Tag)?.text ?? variant.name)\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   table(User { name: \"x\" })\n\
         \x20   ops(Op::Add)\n\
         }\n";
    assert_eq!(run_file("attributes-types", source), "users\nplus\nSub\n");
    let parsed = parse_to_ast(source).expect("the source parses");
    let written = common::infer(&parsed).render();
    assert!(
        written.contains("attributes = [\"Tag { text: \\\"users\\\" }\"]"),
        "{written}"
    );
    assert!(
        written.contains("variant_attributes = [\"Add: Tag { text: \\\"plus\\\" }\"]"),
        "{written}"
    );
    let read = Ledger::parse(&written).expect("the ledger reads back");
    assert_eq!(read.render(), written);
}

/// **D1: an argument is any build-time value**, computed once where it is
/// written, and the ledger records what it came to.
#[test]
fn a_computed_argument_is_read_as_its_value() {
    let source = format!(
        "{JSON}\n\
         comptime PREFIX = \"created\"\n\
         \n\
         fn joined(a: ref String, b: ref String) -> String {{\n\
         \x20   return a + b\n\
         }}\n\
         \n\
         struct User {{\n\
         \x20   @Name(joined(PREFIX, \"At\"))\n\
         \x20   created_at: i64,\n\
         }}\n\
         \n\
         fn wire[T: Struct](value: T) {{\n\
         \x20   for field in T::fields {{\n\
         \x20       println(field.attribute(Name)?.value ?? field.name)\n\
         \x20   }}\n\
         }}\n\
         \n\
         fn main() {{\n\
         \x20   wire(User {{ created_at: 1 }})\n\
         }}\n"
    );
    assert!(codes(&source).is_empty(), "{:?}", codes(&source));
    assert_eq!(run_file("attributes-computed", &source), "createdAt\n");
}
