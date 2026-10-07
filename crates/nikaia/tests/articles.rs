//! **`a` or `an` by the word that follows** (#511): a message that names a
//! type says `an `io::IoError``, `an `i64``, `an `f64`` and `a `u8``.

mod common;

use nikaia::check::Finding;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = common::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    common::checked(&parsed, &own, &library).findings
}

fn said(found: &[Finding]) -> String {
    found
        .iter()
        .map(|f| {
            format!(
                "{}\n{}\n{}",
                f.message,
                f.notes.join("\n"),
                f.help.clone().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_article_follows_the_word() {
    assert_eq!(nikaia_std::tools::check_words::article("`i64`"), "an");
    assert_eq!(nikaia_std::tools::check_words::article("`f64`"), "an");
    assert_eq!(nikaia_std::tools::check_words::article("`u8`"), "a");
    assert_eq!(nikaia_std::tools::check_words::article("`usize`"), "a");
    assert_eq!(
        nikaia_std::tools::check_words::article("`io::IoError`"),
        "an"
    );
    assert_eq!(nikaia_std::tools::check_words::article("`String`"), "a");
    assert_eq!(nikaia_std::tools::check_words::article("`Error`"), "an");
}

/// `NK2105` on a caught `io::IoError`, and `NK1199` on an `f64` and an `i64`.
#[test]
fn a_message_says_an_before_a_vowel() {
    let source = r#"
use std::fs

fn main() {
    let t = fs::read_to_string("nope.txt", fs::Root::Anywhere) catch {
        let kept = error
        println(f"{error}")
        ""
    }
    let px = 0.0
    let n: i64 = 3
    let sum = px + n
    println(f"{t} {sum}")
}
"#;
    let text = said(&findings(source));
    assert!(text.contains("is an `io::IoError`"), "{text}");
    assert!(text.contains("mixing an `f64` and an `i64`"), "{text}");
    assert!(
        !text.contains("a `io::IoError`") && !text.contains("a `f64`"),
        "{text}"
    );
}
