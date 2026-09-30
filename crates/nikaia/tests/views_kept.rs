//! **Views kept past a call** (Part I 6.6): where a destination already names
//! the buffer a view parameter points into, the program is lowered.

mod common;

use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
}

fn ran(purpose: &str, source: &str, how: Build) -> String {
    let rust = lowered(source, how);
    let dir = common::scratch_dir(&format!("views-{purpose}"));
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "{purpose} did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary).output().expect("run it");
    assert!(
        out.status.success(),
        "{purpose} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    std::fs::remove_dir_all(&dir).ok();
    printed
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

/// Both settings, the same output, and no refusal on the way.
fn runs(purpose: &str, source: &str, expected: &str) {
    assert!(
        findings(source).is_empty(),
        "{purpose}: {:#?}",
        findings(source)
    );
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran(purpose, source, how), expected, "{purpose} at {how:?}");
    }
}

/// **A view put into the struct a function hands back** points into the one
/// buffer the result can view: the parameter's.
#[test]
fn a_view_in_a_struct_handed_back_is_the_parameters() {
    runs(
        "handed-back",
        "struct Reading { name: ref String, temp: i64 }\n\n\
         fn make(name: ref String, temp: i64) -> Reading {\n\
         \x20   return Reading { name: name, temp: temp }\n\
         }\n\n\
         fn main() {\n\
         \x20   let text: String = \"oslo\"\n\
         \x20   let r = make(text, 3)\n\
         \x20   println(f\"{r.name} {r.temp}\")\n\
         }\n",
        "oslo 3",
    );
}

/// **A view stored in a field of a struct the caller lends**, where the
/// struct holds views: the parameter is a view of that struct's buffer.
#[test]
fn a_view_stored_in_a_lent_struct_is_its_buffers() {
    runs(
        "lent-struct",
        "struct Summary { label: ref String, count: i64 }\n\n\
         fn relabel(mut s: Summary, name: ref String) {\n\
         \x20   s.label = name\n\
         }\n\n\
         fn main() {\n\
         \x20   let text: String = \"a b\"\n\
         \x20   let mut s = Summary { label: text.trim(), count: 0 }\n\
         \x20   relabel(s, text)\n\
         \x20   println(f\"{s.label}\")\n\
         }\n",
        "a b",
    );
}

/// **A view that could point into two buffers is still refused**: the
/// result names neither.
#[test]
fn a_view_from_one_of_two_buffers_is_refused() {
    let found = findings(
        "struct Reading { name: ref String }\n\n\
         fn pick(a: ref String, b: ref String) -> Reading {\n\
         \x20   return Reading { name: a }\n\
         }\n\n\
         fn main() { println(\"x\") }\n",
    );
    assert!(found.iter().any(|f| f.code == "NK2302"), "{found:#?}");
}

/// **A call on a subject that has nowhere to keep a view keeps none.** A
/// method of a struct of three `bool`s handed a `ref` on to another method of
/// the same struct was refused (`NK2302`, *a call on `Shape` might keep what
/// it's given*): the struct cannot hold the view whatever the method does,
/// which is the rule a field of the subject already had.
#[test]
fn a_call_on_a_subject_that_holds_no_view_keeps_none() {
    runs(
        "subject-holds-no-view",
        "pub struct Shape {\n    pub ends: bool,\n    pub sized: bool,\n}\n\n\
         impl Shape {\n    pub fn meets(ref self, wanted: ref Shape) -> bool {\n        \
         return self.missing(wanted).len() == 0\n    }\n\n    \
         pub fn missing(ref self, wanted: ref Shape) -> Vec[String] {\n        \
         let mut words: Vec[String] = []\n        \
         if wanted.ends && !self.ends {\n            words.push(\"ends\")\n        }\n        \
         return words\n    }\n}\n\n\
         fn main() {\n    let a = Shape { ends: true, sized: false }\n    \
         let b = Shape { ends: false, sized: true }\n    \
         println(f\"{a.meets(ref b)} {b.meets(ref a)}\")\n}\n",
        "true false",
    );
}

/// **Owned text is not a pattern**: `starts_with`, `ends_with` and `contains`
/// take a view of text or a character, and text of its own handed to one
/// reached `rustc` as *`Pattern` is not implemented for `String`*. It is
/// `NK1102` with *write `ref`*, and each shape that is accepted runs.
#[test]
fn owned_text_handed_to_a_pattern_is_refused_and_a_view_runs() {
    for (argument, refused) in [
        ("f\"{w}[\"", true),
        ("w", true),
        ("ref w", false),
        ("ref f\"{w}\"", false),
        ("\"ab\"", false),
        ("'a'", false),
    ] {
        let source = format!(
            "fn f(line: ref String, w: String) -> bool {{\n    return line.starts_with({argument})\n}}\n\n\
             fn main() {{\n    let r = f(\"abc\", \"a\")\n    println(f\"{{r}}\")\n}}\n"
        );
        let found = findings(&source);
        let refusal = found.iter().find(|f| f.code == "NK1102");
        assert_eq!(refusal.is_some(), refused, "{argument}: {found:#?}");
        if let Some(refusal) = refusal {
            assert_eq!(
                refusal.help.as_deref(),
                Some("Write `ref` in front of it to pass a view of it."),
                "{argument}"
            );
        } else {
            let out = ran("pattern", &source, Build::default());
            assert!(out == "true" || out == "false", "{argument}: {out}");
        }
    }
}
