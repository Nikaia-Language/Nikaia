//! `loop`, `const`, `macro` and `quote` are names
//! ([ADR-298](../../../docs/specification/adr/adr-298.md)).
//!
//! **Reserving a word buys exactly one thing**, and it is the sentence a reader
//! who writes it gets. The four were reserved *against* the possibility of a
//! construct rather than for one, which is the ground
//! [ADR-298](../../../docs/specification/adr/adr-298.md) D1 does not accept —
//! and `NK1117` can say that sentence about an ordinary name, so they were
//! paying for nothing.
//!
//! That an escaped one still runs is in `tests/language/src/four_words_freed.nika`.

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

const FREED: [&str; 4] = ["loop", "const", "macro", "quote"];

fn lowered(source: &str) -> Result<String, String> {
    let parsed = parse_to_ast(source).map_err(|e| format!("{e:#}"))?;
    emit_program(&parsed, Build::default())
        .map(|it| it.rust)
        .map_err(|e| format!("{e:#}"))
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

/// **None of the four is on the reserved list** (D1), which is the claim the
/// rest of this file rests on.
#[test]
fn the_four_left_the_list() {
    for word in FREED {
        assert!(
            !nikaia::parser::RESERVED_WORDS.contains(&word),
            "`{word}` is a name now"
        );
    }
    // And `with` did not leave (D3): it is reserved **for** a construct, which
    // is the one ground that holds.
    assert!(nikaia::parser::RESERVED_WORDS.contains(&"with"));
}

/// **Each is a name in every position that declares one** (D1).
#[test]
fn each_is_an_ordinary_name() {
    for word in FREED {
        let source = format!(
            "struct Row {{\n\
             \x20   {word}: i64,\n\
             }}\n\
             fn takes({word}: i64) -> i64 {{ return {word} }}\n\
             fn main() {{\n\
             \x20   let {word} = 1\n\
             \x20   let r = Row {{ {word}: {word} }}\n\
             \x20   println(f\"{{takes(r.{word})}}\")\n\
             }}"
        );
        let found = findings(&source);
        assert!(found.is_empty(), "`{word}` is a name: {found:#?}");
    }
}

/// **The language below is answered as it is for `type`** (D1): the emitter
/// escapes the three Rust reserves, and leaves the one it does not.
///
/// `quote` is a keyword in neither language and needs nothing, which is what
/// makes it the control in this test rather than a fourth case.
#[test]
fn the_three_the_backend_reserves_are_escaped() {
    for (word, escaped) in [
        ("loop", true),
        ("const", true),
        ("macro", true),
        ("quote", false),
    ] {
        let rust = lowered(&format!(
            "fn main() {{ let mut {word} = 1 {word} = {word} + 1 println(f\"{{{word}}}\") }}"
        ))
        .unwrap_or_else(|e| panic!("`{word}` lowers: {e}"));
        assert_eq!(
            rust.contains(&format!("r#{word}")),
            escaped,
            "`{word}`: escaped should be {escaped}\n{rust}"
        );
    }
}

/// **What each used to be told a reserved word, `NK1117` tells a stray one**
/// (D2).
///
/// Three sentences and four words, because `macro` and `quote` are one answer:
/// this language has no macros, and what a reader reaching for either wants is
/// `comptime` and a bound, or a grammar.
#[test]
fn a_stray_word_is_told_what_to_write_instead() {
    for (word, expected) in [
        ("loop", "while true"),
        ("const", "comptime"),
        ("macro", "no macros"),
        ("quote", "no macros"),
    ] {
        // `{word} x` is two statements to a scannerless grammar, which is the
        // misparse `NK1117` exists for - so the one about the word is picked out
        // rather than the pair being counted.
        let found: Vec<_> = findings(&format!("fn main() {{\n    {word} x\n}}"))
            .into_iter()
            .filter(|f| f.code == "NK1117" && f.message.contains(&format!("`{word}`")))
            .collect();
        assert_eq!(found.len(), 1, "`{word}` is one refusal: {found:#?}");
        let help = found[0].help.as_deref().expect("a way out");
        assert!(
            help.contains(expected),
            "`{word}` is answered with `{expected}`: {help}"
        );
        // And the sentence says the word is a name otherwise, so a reader who
        // *did* mean a name is not sent looking for a keyword.
        assert!(help.contains("ordinary name"), "`{word}`: {help}");
    }
}

/// **Where the word is declared there is nothing to say, and nothing is said**
/// (D2's last line).
#[test]
fn a_declared_word_gets_no_sentence() {
    for word in FREED {
        let found = findings(&format!(
            "fn main() {{\n    let {word} = 1\n    println(f\"{{{word}}}\")\n}}"
        ));
        assert!(found.is_empty(), "`{word}` is declared here: {found:#?}");
    }
}

/// **A name that is not one of the four keeps the general sentence**, which is
/// what says this added a case rather than replacing the help.
#[test]
fn every_other_name_is_answered_as_before() {
    let found: Vec<_> = findings("fn main() {\n    widgit x\n}")
        .into_iter()
        .filter(|f| f.code == "NK1117" && f.message.contains("`widgit`"))
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    let help = found[0].help.as_deref().expect("a way out");
    assert!(help.contains("declare it with `let`"), "{help}");
    assert!(
        help.contains("Nikaia doesn't have one called `widgit`"),
        "{help}"
    );
    // **And it stopped explaining `1_000`**
    // ([ADR-285](../../../docs/specification/adr/adr-285.md)): that form is a
    // number now, so the clause that named it as a misparse was wrong.
    assert!(!help.contains("`1_000`"), "{help}");
}
