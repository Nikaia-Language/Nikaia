//! **A jump may carry its condition after it**
//! ([ADR-276](../../../docs/specification/adr/adr-276.md)).
//!
//! `return 250 if speed > 250` is `if speed > 250 { return 250 }`, and the
//! same for `throw`, `break` and `continue`. What the form promises is where
//! control goes, so the programs that show it are
//! `tests/language/src/jump_guards.nika`; this file keeps what the parser
//! builds.

use nikaia::ast::{Expr, Item, Stmt};
use nikaia::parser::parse_to_ast;

/// The statements of the program's `main`, as the parser built them.
fn main_body(source: &str) -> Vec<Stmt> {
    let parsed = parse_to_ast(source).expect("the source parses");
    parsed
        .program
        .items
        .iter()
        .find_map(|item| match &item.node {
            Item::Fn {
                name: Some(name),
                body,
                ..
            } if parsed.text(*name) == "main" => {
                Some(body.stmts.iter().map(|s| s.node.clone()).collect())
            }
            _ => None,
        })
        .expect("a main")
}

/// **D3: an `if` that begins a line is a statement of its own**, never the
/// condition of the jump above it.
#[test]
fn an_if_on_the_next_line_is_not_the_jumps_condition() {
    let body = main_body(
        "fn main() {\n\
         \x20   for n in 0..<3 {\n\
         \x20       break\n\
         \x20       if n > 1 { println(\"never\") }\n\
         \x20   }\n\
         }\n",
    );
    let Stmt::For { body, .. } = &body[0] else {
        panic!("a for: {body:#?}");
    };
    assert!(matches!(body.stmts[0].node, Stmt::Break), "{body:#?}");
    assert!(
        matches!(body.stmts[1].node, Stmt::Expr(Expr::If { .. })),
        "{body:#?}"
    );
}

/// **D1: the tree is the `if` it stands for**, so nothing after the parser
/// needed to learn the form.
#[test]
fn the_guard_is_an_if_around_the_jump() {
    let body = main_body("fn main() {\n    for n in 0..<3 {\n        break if n > 1\n    }\n}\n");
    let Stmt::For { body, .. } = &body[0] else {
        panic!("a for: {body:#?}");
    };
    let Stmt::Expr(Expr::If {
        then_branch,
        else_branch: None,
        ..
    }) = &body.stmts[0].node
    else {
        panic!("an if: {body:#?}");
    };
    assert!(matches!(then_branch.stmts[0].node, Stmt::Break));
}
