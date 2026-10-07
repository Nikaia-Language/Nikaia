//! **`u64`, `u32` and the bit operators**
//! ([ADR-285](../../../docs/specification/adr/adr-285.md)): the types a program
//! asked for, `&`, `|`, `^`, `<<`, `>>` and `!` on integers, a literal as wide
//! as a `u64`, and a text's bytes. The program that asked is a hash; it and what
//! each operator computes are `tests/language/src/bits.nika`, and here is what
//! is refused.

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

/// **The refusals, each in this language's words** (D1, D3, D4): a bit
/// operator beside a comparison without parentheses, with the parenthesised
/// line handed over; `&` on two `bool`s, pointed at `&&`; and a `u64` added to
/// an `i64`.
#[test]
fn what_is_refused_is_refused_with_the_line_to_write() {
    let beside = findings(
        "fn main() {\n\
         \x20   let a: u64 = 5\n\
         \x20   let zero = a & 1 == 0\n\
         \x20   let fine = (a & 1) == 0\n\
         }\n",
    );
    assert!(
        beside
            .iter()
            .any(|f| f.code == "NK1197" && f.help.as_deref() == Some("Write `(a & 1) == 0`.")),
        "{beside:#?}"
    );
    assert_eq!(
        beside.iter().filter(|f| f.code == "NK1197").count(),
        1,
        "the parenthesised line is not refused: {beside:#?}"
    );

    let booleans = findings("fn main() {\n    let both = true & false\n}\n");
    assert!(
        booleans
            .iter()
            .any(|f| f.code == "NK1198" && f.help.as_deref() == Some("`&&` joins two `bool`s")),
        "{booleans:#?}"
    );

    let mixed = findings(
        "fn main() {\n\
         \x20   let a: u64 = 5\n\
         \x20   let b: i64 = 6\n\
         \x20   let c = a + b\n\
         }\n",
    );
    assert!(
        mixed.iter().any(|f| f.code == "NK1199"
            && f.message == "You're mixing a `u64` and an `i64` in one operation."),
        "{mixed:#?}"
    );
}

/// **A literal as wide as a `u64` holds** (D2, run in `bits.nika`), and no
/// wider: above an `i64` it is a `u64`'s, and refused against `i64` where no
/// type stands beside it.
#[test]
fn a_literal_above_an_i64_is_a_u64s() {
    let bare = findings("fn main() {\n    let big = 18446744073709551615\n}\n");
    assert!(
        bare.iter().any(|f| f.code == "NK1116"
            && f.help
                .as_deref()
                .is_some_and(|h| h.contains("`let x: u64 = …`"))),
        "{bare:#?}"
    );
    assert!(parse_to_ast("fn main() {\n    let x: u64 = 18446744073709551616\n}\n").is_err());
}

/// **An unannotated constant takes its type from its uses**
/// ([ADR-285](../../../docs/specification/adr/adr-285.md)): with nothing
/// pinning `a`, the three names are one number, no use asks, and what they are
/// given does not fit an `i32` - so the sum is an `i64` (run in `bits.nika`).
/// Beside an `i32` `a`, `b` is asked to be an `i32` by `a + b`, and `3000000000` does not fit one:
/// `NK1116` at `b`, naming the use, where it was `rustc`'s *cannot add `i64` to
/// `i32`*.
#[test]
fn a_large_constant_is_an_i64_and_mixes_with_nothing_else() {
    let pinned = findings(
        "fn main() {\n\
         \x20   let a: i32 = 1\n\
         \x20   let b = 3000000000\n\
         \x20   let c = a + b\n\
         }\n",
    );
    assert!(
        pinned.iter().any(|f| f.code == "NK1116"
            && f.message == "`3000000000` doesn't fit in an `i32`."
            && f.notes
                .iter()
                .any(|n| n.contains("`b` is an `i32` because of how it is used"))),
        "{pinned:#?}"
    );
    assert_eq!(pinned.len(), 1, "one cause, one refusal: {pinned:#?}");
}
