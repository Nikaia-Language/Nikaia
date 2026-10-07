//! **Views kept past a call** (Part I 6.6): where a destination already names
//! the buffer a view parameter points into, the program is lowered.
//!
//! The programs that are lowered, and what they compute, are
//! `tests/language/src/views_kept.nika`; this file keeps what is refused.

use nikaia::contracts::LedgerOps;
use nikaia::parser::parse_to_ast;

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

/// **Owned text is not a pattern**: `starts_with`, `ends_with` and `contains`
/// take a view of text or a character, and text of its own handed to one
/// reached `rustc` as *`Pattern` is not implemented for `String`*. It is
/// `NK1102` with *write `ref`*; each shape that is accepted runs in
/// `views_kept.nika`.
#[test]
fn owned_text_handed_to_a_pattern_is_refused() {
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
        }
    }
}
