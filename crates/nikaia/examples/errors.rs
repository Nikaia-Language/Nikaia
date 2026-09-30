//! Regenerates `tests/errors/EXPECTED.txt`.
//!
//! ```text
//! cargo run -p nikaia --example errors > tests/errors/EXPECTED.txt
//! ```
//!
//! The sibling of `dump.rs`, and the same bargain: what is checked in is what
//! the compiler says *today*, wrong messages included, so that any change to
//! the expectation ranking is a diff somebody can read. The claims about what
//! each case *should* say live in `docs/error-corpus.md`.

use nikaia::parser::parse_to_ast;
use std::path::{Path, PathBuf};

/// Every case and its message, in one text.
///
/// Sorted by name, so the file is the same on every machine and a diff is only
/// ever a change in behaviour. `crates/nikaia/tests/errors.rs` holds a copy of
/// this walk - the same split `dump.rs` and `grammar_lowering.rs` already have,
/// an example being a binary a test cannot call into.
pub fn render_corpus(dir: &Path) -> String {
    let mut cases: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read errors dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("nika"))
        .collect();
    cases.sort();

    let mut out = String::new();
    for path in cases {
        let name = path.file_stem().expect("stem").to_string_lossy();
        let source = std::fs::read_to_string(&path).expect("read case");
        out.push_str(&format!("=== {name}\n"));
        match parse_to_ast(&source) {
            // A case that parses is not a hole in the corpus: it says the
            // grammar admits something, and that belongs in the diff too.
            // Two of them do, E2 and G2 - see docs/error-corpus.md.
            Ok(_) => out.push_str("(parses)\n"),
            Err(e) => {
                out.push_str(&e.to_string());
                out.push('\n');
            }
        }
        out.push('\n');
    }
    out
}

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/errors");
    print!("{}", render_corpus(&dir));
}
