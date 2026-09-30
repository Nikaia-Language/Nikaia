//! **A list has no operators** (Part I 4.5), and one written on it is refused
//! here — `NK1191` (0.0.234) — rather than by `rustc` about a file nobody
//! wrote.
//!
//! `[1] + [2]` passed the check and lowered as it was written, and the language
//! below said *cannot add `Vec<i64>` to `Vec<i64>`*. `+` does not join two
//! lists ([ADR-253](../../../docs/specification/adr/adr-253.md)), and the
//! refusal names the way that does, `a.extend(b)`.
//!
//! **A declared `struct` or `enum` has none either**, under the same
//! code: `a -= 30` on an `Account` reached `rustc` as E0368.

mod common;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

fn ran(purpose: &str, source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let rust = emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust;
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );
    let out = std::process::Command::new(&binary)
        .output()
        .expect("run it");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn two_lists_added_together_are_refused_with_the_way_that_exists() {
    let source = "fn main() {\n\
                  \x20   let a = [1, 2]\n\
                  \x20   let b = [3]\n\
                  \x20   let c = a + b\n\
                  }\n";
    let found = findings(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1191");
    assert_eq!(found[0].message, "You can't use `+` on a list.");
    assert!(
        found[0].notes[0] == "Lists have no operators: `+` doesn't join two lists.",
        "{found:#?}"
    );
    assert_eq!(
        found[0].help.as_deref(),
        Some("To add one list's elements to the end of another, write `a.extend(b)`.")
    );
}

/// **Any arithmetic on any collection**, and no help where there is no
/// obvious other way to say it.
#[test]
fn arithmetic_on_a_map_is_refused_too() {
    let source = "use std::collections\n\
                  \n\
                  fn main() {\n\
                  \x20   let mut m: collections::HashMap[String, i64] = collections::HashMap()\n\
                  \x20   let n = m * 2\n\
                  }\n";
    let found: Vec<_> = findings(source)
        .into_iter()
        .filter(|f| f.code == "NK1191")
        .collect();
    assert_eq!(found.len(), 1, "{:#?}", findings(source));
    assert!(
        found[0].message.starts_with("You can't use `*` on `"),
        "{found:#?}"
    );
    assert_eq!(found[0].help, None);
}

/// **Numbers and text keep their operators**, and the way the refusal names
/// runs.
///
/// And **text read out of a list joins without being taken out of it**
/// (0.0.234): `w[0] + w[2]` over a `Vec[String]` moved the element out, and
/// `rustc` refused it; each side is now lent, and `w[0]` is still there to
/// print afterwards. A list of views joins the same way.
#[test]
fn numbers_and_text_are_untouched_and_extend_joins_two_lists() {
    let source = "fn main() {\n\
                  \x20   let mut a = [1, 2]\n\
                  \x20   a.extend([3])\n\
                  \x20   let mut w: Vec[String] = [\"x\"]\n\
                  \x20   w.extend([\"y\", \"z\"])\n\
                  \x20   let n = a[0] + a[2] * 2\n\
                  \x20   let t = w[0] + w[2]\n\
                  \x20   let v = [\"p\", \"q\"]\n\
                  \x20   let u = v[1] + v[0] + w[1]\n\
                  \x20   println(f\"{a.len()} {w.len()} {n} {t} {u} {w[0]}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert_eq!(ran("list-extend", source), "3 3 7 xz qpy x\n");
}

/// **`xs += [3]` is `+` on a list too**: a compound assignment was
/// never asked, and the language below said *binary assignment operation `+=`
/// cannot be applied to type `Vec<i64>`*. The help writes the line back whole.
#[test]
fn a_list_added_onto_in_place_is_refused_with_its_own_extend() {
    let source = "fn main() {\n\
                  \x20   let mut xs = [1, 2]\n\
                  \x20   xs += [3]\n\
                  }\n";
    let found = findings(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1191");
    assert_eq!(found[0].message, "You can't use `+` on a list.");
    assert_eq!(
        found[0].help.as_deref(),
        Some("To add one list's elements to the end of another, write `xs.extend([3])`.")
    );
}

/// **A declared `struct` has no arithmetic**, and neither `-=` nor
/// `-` on one reaches `rustc`'s E0368 and E0369. There is no overloading and no
/// trait that gives a type an operator (Part I 4.7), so nothing a program
/// declares has one. One numeric field is the one field the author can have
/// meant, and the help names it.
#[test]
fn arithmetic_on_a_struct_is_refused_and_names_its_one_number() {
    let source = "struct Account { balance: i32 }\n\
                  \n\
                  fn main() {\n\
                  \x20   let mut a = Account { balance: 1 }\n\
                  \x20   a -= 30\n\
                  \x20   let b = a - 30\n\
                  }\n";
    let found = findings(source);
    assert_eq!(found.len(), 2, "{found:#?}");
    for f in &found {
        assert_eq!(f.code, "NK1191");
        assert_eq!(f.message, "You can't use `-` on an `Account`.");
        assert_eq!(
            f.notes[0],
            "`Account` is a `struct`, and a `struct` has no arithmetic: operators belong \
             to numbers, and `+` to text as well."
        );
    }
    assert_eq!(
        found[0].help.as_deref(),
        Some("Did you mean a field, like `a.balance -= 30`?")
    );
    assert_eq!(
        found[1].help.as_deref(),
        Some("Did you mean a field, like `a.balance - 30`?")
    );
}

/// **No guess where there is more than one field it could be**, and an `enum`
/// is refused the same way, on either side of the operator.
#[test]
fn a_struct_with_two_numbers_and_an_enum_are_refused_without_a_guess() {
    let source = "struct Point { x: i64, y: i64 }\n\
                  enum Shade { Light, Dark }\n\
                  \n\
                  fn main() {\n\
                  \x20   let p = Point { x: 1, y: 2 }\n\
                  \x20   let q = p * 2\n\
                  \x20   let t = 1 + Shade::Light\n\
                  }\n";
    let found = findings(source);
    assert_eq!(found.len(), 2, "{found:#?}");
    assert_eq!(found[0].message, "You can't use `*` on a `Point`.");
    assert_eq!(
        found[0].help.as_deref(),
        Some(
            "Do the arithmetic on one of its fields, or write a method that says what `*` \
             means for it."
        )
    );
    assert_eq!(found[1].code, "NK1191");
    assert_eq!(found[1].message, "You can't use `+` on a `Shade`.");
    assert!(
        found[1].notes[0].contains("an `enum` has no arithmetic"),
        "{found:#?}"
    );
}

/// **What has operators keeps them**: a struct's numeric field, and text
/// joined with `+` and `+=`, are not refused, and the program runs.
#[test]
fn a_structs_fields_and_text_keep_their_operators() {
    let source = "struct Account { balance: i32, owner: String }\n\
                  \n\
                  fn main() {\n\
                  \x20   let mut a = Account { balance: 1, owner: \"ada\" }\n\
                  \x20   a.balance -= 30\n\
                  \x20   let b = a.balance * 2\n\
                  \x20   let mut t = a.owner + \"!\"\n\
                  \x20   t += \"?\"\n\
                  \x20   println(f\"{a.balance} {b} {t}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    assert_eq!(ran("struct-fields", source), "-29 -58 ada!?\n");
}
