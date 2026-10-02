//! **What changed, said line by line** (`tools/diffs.nika`, #125): the lines
//! of a ledger a `--locked` build finds different, and the difference an
//! output test prints between what it expected and what it produced.

use nikaia_std::tools::diffs::{changed_lines, differing_lines};

/// A changed line is named under its entry, once; a new entry names itself.
#[test]
fn a_changed_ledger_line_is_named_under_its_entry() {
    let committed = "# header\n[fn.\"a\"]\nsync = true\n";
    let built = "# header\n[fn.\"a\"]\nsync = true\nthrows = [\"E\"]\n[fn.\"b\"]\n";
    assert_eq!(
        changed_lines(built, committed),
        vec![
            "  [fn.\"a\"]".to_string(),
            "      throws = [\"E\"]".to_string(),
            "  [fn.\"b\"]".to_string(),
        ]
    );
}

/// The lines that differ, with two of context and `…` for what lies between -
/// what was produced first where the two are as long as each other.
#[test]
fn an_output_difference_shows_the_lines_that_differ() {
    let expected = "1\n2\n3\n4\n5\n6\n7\n8\n";
    let produced = "1\n2\n3\n4\n5\n6\n7\nacht\n";
    assert_eq!(
        differing_lines(expected, produced),
        "  …\n  6\n  7\n+ acht\n- 8\n"
    );
    assert_eq!(
        differing_lines("a\nb", "a\nb\n"),
        "  the same lines; they differ in the line break at the end\n"
    );
}
