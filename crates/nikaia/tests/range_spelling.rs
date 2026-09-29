//! `a..b` includes its end and `a..<b` does not
//! ([ADR-137](../../../docs/specification/adr/adr-137.md) D3, D4, D5).
//!
//! **One meaning per spelling** is the whole of the argument. The alternative
//! the record turned down was `..` exclusive in an expression and inclusive in
//! a pattern, which is a rule a reader has to hold in their head and one the
//! compiler cannot help with: both forms parse in both places and mean
//! different things. Kotlin and Swift are the precedent and both read `..<` as
//! *up to, not including*.

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn emit(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("it lowers")
        .rust
}

/// **`..<` stops before its end and `..` includes it** (D3, D4), and the two
/// lower to Rust's two.
#[test]
fn the_two_spellings_are_the_two_ranges() {
    assert!(emit("fn main() { for i in 0..<5 { } }").contains("for i in 0..5 {"));
    assert!(emit("fn main() { for i in 0..5 { } }").contains("for i in 0..=5 {"));
}

/// **`..=` is withdrawn** (D5), and the refusal names the form it was. Keeping
/// it would be the two-spellings defect introduced by the record that argues
/// *one meaning per spelling*.
#[test]
fn the_old_inclusive_spelling_is_refused_by_name() {
    let source = "fn main() { for i in 0..=5 { } }";
    let refused = parse_to_ast(source).expect_err("`..=` goes");
    let finding =
        nikaia::diagnostics::refused_finding(&refused).expect("a parse error carries its finding");
    let message = nikaia::diagnostics::render_finding(finding, "app.nika", source);
    assert!(message.contains("Nikaia has no `..=`."), "{message}");
    assert!(message.contains("`0..n` already includes `n`"), "{message}");
    assert!(message.contains("0..n"), "{message}");
    assert!(message.contains("0..<n"), "{message}");
}

/// **Both chains take both**, which is what says the head of a `for` and an
/// ordinary expression read the same two operators: the grammar keeps a second
/// expression chain for the positions where a `{` is a body rather than a
/// struct literal, and a spelling added to one and not the other is a form
/// that works in half the language.
#[test]
fn the_head_chain_reads_the_same_two() {
    // A range kept in a name is a value of its own (ADR-212 D3), `span` for
    // `..<` and `through` for `..`; one written into the `for` is Rust's.
    assert!(emit("fn main() { let r = 0..<5\n for i in r { } }").contains("range::span(0, 5)"));
    assert!(emit("fn main() { if 3 > 2 { for i in 0..<5 { } } }").contains("0..5"));
    assert!(emit("fn main() { let r = 0..5\n for i in r { } }").contains("range::through(0, 5)"));
}

/// **A slice reads them too**, which is D4's *everywhere*: `..` is inclusive in
/// a `for`, in a slice and in a pattern alike.
#[test]
fn a_slice_reads_the_same_two() {
    let rust = emit("fn f(dna: ref String, k: i64) { let part = ref dna[0..<k] }");
    assert!(rust.contains("(0..k)"), "{rust}");
    let inclusive = emit("fn f(dna: ref String, k: i64) { let part = ref dna[0..k] }");
    assert!(inclusive.contains("(0..=k)"), "{inclusive}");
}

/// **It still binds looser than every operator in it**, which is the reading a
/// loop head wants: `0..<n - 1` ends below `n - 1` rather than being a range
/// with something subtracted from it.
#[test]
fn it_binds_looser_than_its_arithmetic() {
    let rust = emit("fn f(n: i32) { for i in 1..<n - 1 { } }");
    assert!(rust.contains("for i in 1..n - 1 {"), "{rust}");
}

/// **And the migration is exact**, which is why it was one change: the old
/// spelling keeps parsing and changes meaning, so nothing in the tree may be
/// left on it. No `.nika` file in the corpus writes a bare `..` range any more
/// except where the inclusive one is meant.
#[test]
fn no_nika_source_in_the_tree_was_left_on_the_old_meaning() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut left = Vec::new();
    let mut walk = vec![root.clone()];
    while let Some(dir) = walk.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if path.is_dir() {
                if !matches!(name.as_str(), "target" | ".git" | "vendor" | "node_modules") {
                    walk.push(path);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("nika") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (n, line) in text.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                // A `for` head is the shape the corpus writes; an inclusive one
                // would be deliberate, and none is.
                if code.contains(" in ") && code.contains("..") && !code.contains("..<") {
                    left.push(format!("{}:{}: {}", path.display(), n + 1, code));
                }
            }
        }
    }
    assert!(
        left.is_empty(),
        "left on the old spelling:\n{}",
        left.join("\n")
    );
}
