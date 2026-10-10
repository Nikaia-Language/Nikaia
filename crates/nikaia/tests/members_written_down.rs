//! **What is written down is what can be called**
//! ([ADR-286](../../../docs/specification/adr/adr-286.md) D36, #509): a member
//! a known type or module does not have is `NK1171`, also where Rust has one
//! of that name. Each of these was lowered as written and answered by `rustc`
//! about the generated file, or, for `as_str`, ran as Rust.

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn errors(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

#[test]
fn a_member_nothing_writes_down_is_refused() {
    for (what, source, message, help) in [
        (
            "a Rust associated function",
            "fn main() {\n    let t = String::from(\"x\")\n    println(t)\n}\n",
            "`String` has no function called `from`.",
            "Write the text itself, `\"x\"`, or `\"x\".clone()` for text of its own.",
        ),
        (
            "a method of text",
            "fn main() {\n    let s = \"x\".frobnicate()\n}\n",
            "`String` has no method called `frobnicate`.",
            "Only what is written down for `String` can be called; a Rust function or method of the same name is not reached.",
        ),
        (
            "a method of a list",
            "fn main() {\n    let s = [1, 2, 3].frobnicate()\n}\n",
            "`Vec` has no method called `frobnicate`.",
            "Only what is written down for `Vec` can be called; a Rust function or method of the same name is not reached.",
        ),
        (
            "a function of a std module",
            "use std::fs\n\nfn main() {\n    fs::frobnicate(\"x\")\n}\n",
            "`fs` has no function called `frobnicate`.",
            "Only what is written down for `fs` can be called; a Rust function or method of the same name is not reached.",
        ),
        (
            "a Rust method Nikaia does not offer",
            "fn main() {\n    let s = \"x\".as_str()\n    println(s)\n}\n",
            "`String` has no method called `as_str`.",
            "Leave it out: the text is already what is passed.",
        ),
        (
            "a method of the program's own struct",
            "struct P {\n    x: i64,\n}\n\nfn main() {\n    let p = P { x: 1 }\n    p.frobnicate()\n}\n",
            "`P` has no method called `frobnicate`.",
            "Only what is written down for `P` can be called; a Rust function or method of the same name is not reached.",
        ),
    ] {
        let found = errors(source);
        assert!(
            found.iter().any(|f| f.code == "NK1171"
                && f.message == message
                && f.help.as_deref() == Some(help)),
            "{what}: {found:#?}"
        );
    }
}

/// **A near name is offered**, and `is_some` on a `T?` (`NK1125`) is
/// answered with `null`.
#[test]
fn the_help_names_the_spelling_nikaia_has() {
    let found = errors("fn main() {\n    let s = \"abc\".lenn()\n}\n");
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK1171" && f.help.as_deref() == Some("Did you mean `.len()`?")),
        "{found:#?}"
    );
    let found = errors(
        "fn first(xs: ref Vec[i64]) -> bool {\n    let x: i64? = null\n    return x.is_some()\n}\n\nfn main() {\n    println(f\"{first([1])}\")\n}\n",
    );
    assert!(
        found.iter().any(|f| f.code == "NK1125"
            && f.help.as_deref() == Some("Compare with `null`: `value != null`.")),
        "{found:#?}"
    );
}

/// **What is written down still passes**: a method `std` describes, a
/// program's own method, and a function of a module.
#[test]
fn a_member_that_is_written_down_passes() {
    let found = errors(
        "use std::text\n\nstruct P {\n    x: i64,\n}\n\nimpl P {\n    fn twice(self) -> i64 {\n        return self.x * 2\n    }\n}\n\n\
         fn main() {\n    let p = P { x: 1 }\n    let n = text::parse_i64(\"3\") ?? 0\n    println(f\"{p.twice()} {\"ab\".len()} {n}\")\n}\n",
    );
    assert!(found.is_empty(), "{found:#?}");
}
