//! **A list has no operators** (Part I 4.5), and one written on it is refused
//! here — `NK1191` (0.0.234) — rather than by `rustc` about a file nobody
//! wrote.
//!
//! `[1] + [2]` passed the check and lowered as it was written, and the language
//! below said *cannot add `Vec<i64>` to `Vec<i64>`*. Whether `+` should join
//! two lists is the owner's question (`open-decisions.md`); until it is
//! answered the refusal names the way that exists, `a.extend(b)`.

mod common;

use nikaia::contracts::{Ledger, STD};
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
    assert_eq!(found[0].message, "`+` is not defined on a list");
    assert!(
        found[0].notes[0].contains("open-decisions.md"),
        "{found:#?}"
    );
    assert_eq!(
        found[0].help.as_deref(),
        Some("to add one list's elements to the end of another: `a.extend(b)`")
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
        found[0].message.starts_with("`*` is not defined on `"),
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
