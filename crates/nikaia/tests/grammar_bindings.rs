//! **A grammar's bindings have the types the grammar gives them** (#500,
//! Part II 10.8's table): a rule's declared result, text for `until` and its
//! kind, a list for a repetition, a `T?` for `p?`.

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

fn refused_as(source: &str, ty: &str) -> bool {
    findings(source)
        .iter()
        .any(|f| f.code == "NK1103" && f.message.contains(&format!("`{ty}`")))
}

/// `t:term` is what `term` returns, and a list of `add_tail`s is a list of
/// what that returns.
#[test]
fn a_rule_binding_is_the_rule_s_result() {
    let fine = r#"
grammar Calc {
    entry rule expr -> i64 =
        head:term tail:add_tail* {
            let mut value = head
            for t in tail { value = value + t }
            value
        }
    rule add_tail -> i64 =
          "+" t:term { t }
        | "-" t:term { -t }
    rule term -> i64 = n:dec[i64](digit+) { n }
}
"#;
    assert!(findings(fine).is_empty(), "{:#?}", findings(fine));
    let wrong = r#"
grammar Calc {
    entry rule expr -> i64 =
        head:term tail:term* {
            let b: bool = head
            let c: bool = tail
            0
        }
    rule term -> i64 = n:dec[i64](digit+) { n }
}
"#;
    assert!(refused_as(wrong, "i64"), "{:#?}", findings(wrong));
    assert!(refused_as(wrong, "Vec[i64]"), "{:#?}", findings(wrong));
}

/// `s:until(";")` is a view of the input's text.
#[test]
fn until_binds_text() {
    let fine = r#"
grammar Fields {
    entry rule field -> ref String = s:until(";") { s.trim() }
}
"#;
    assert!(findings(fine).is_empty(), "{:#?}", findings(fine));
    let wrong = r#"
grammar Fields {
    entry rule field -> i64 = s:until(";") {
        let n: i64 = s
        n
    }
}
"#;
    assert!(
        findings(wrong).iter().any(|f| f.code == "NK1103"),
        "{:#?}",
        findings(wrong)
    );
}
