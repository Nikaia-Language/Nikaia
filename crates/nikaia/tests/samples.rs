//! Every file under `tests/samples/` must parse.
//!
//! These samples were previously only exercised by hand, which is how
//! `let_assignment.nika` came to contain a top-level `let` that the grammar
//! never accepted. A test keeps them honest.

use nikaia::parser::parse_to_ast;
use std::path::{Path, PathBuf};

fn samples_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/nikaia; the samples live at the repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/samples")
        .canonicalize()
        .expect("samples directory")
}

#[test]
fn all_samples_parse() {
    let dir = samples_dir();
    let mut checked = 0;

    for entry in std::fs::read_dir(&dir).expect("read samples dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("nika") {
            continue;
        }

        let source = std::fs::read_to_string(&path).expect("read sample");
        if let Err(e) = parse_to_ast(&source) {
            panic!("{} failed to parse:\n{e}", path.display());
        }
        checked += 1;
    }

    assert!(checked > 0, "no .nika samples found in {}", dir.display());
}

#[test]
fn trailing_input_is_rejected() {
    // Input the grammar cannot consume must be an error, not a silently
    // truncated parse.
    // `%`, because `@` starts an attribute since ADR-331 and is read as one.
    let err = parse_to_ast("fn main() {}\n%%%").expect_err("should reject trailing garbage");
    let msg = err.to_string();
    assert!(
        msg.contains("Didn't expect `%` here."),
        "unexpected error: {msg}"
    );
    assert!(
        msg.contains("(line 2, column 1)"),
        "unexpected error: {msg}"
    );
}

#[test]
fn trailing_whitespace_is_accepted() {
    // The entry rule ends with `skip_ws`, so a trailing newline is fine.
    parse_to_ast("fn main() {}\n\n  \n").expect("trailing whitespace should parse");
}
