//! **Every `Expr` node carries an id the parser gave it**
//! ([ADR-340](../../../docs/specification/adr/adr-340.md), #558).
//!
//! The ids are read from the tree's pretty `Debug` (`{:#?}`), which prints each as `NodeId(n)`:
//! a walk of the tree would have to be kept in step with the variants, and this
//! one cannot miss a node.

use nikaia::ast::{Expr, Item, Stmt};
use nikaia::parser::{parse_expression, parse_expression_from, parse_to_ast};
use std::collections::BTreeSet;

/// Every id in the printed form of a tree, in the order printed.
fn ids_in(printed: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut rest = printed;
    while let Some(at) = rest.find("NodeId(") {
        rest = &rest[at + "NodeId(".len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        out.push(digits.parse().expect("an id is a number"));
    }
    out
}

const A_FILE: &str = r#"
fn add(a: i64, b: i64) -> i64 {
    return a + b * 2
}

fn main() {
    let xs = [1, 2, 3]
    let total = add(xs[0], xs[1]) + xs.len()
    if total > 3 {
        println(f"total {total} of {xs.len()}")
    }
    for x in 0..3 {
        print("{x}")
    }
    let named = match total {
        1 => "one",
        else => "many",
    }
    let maybe: String? = null
    let shown = maybe ?? "none"
    println(shown)
}
"#;

/// The body of `fn main` of a one-function source.
fn main_body(parsed: &nikaia::parser::Parsed) -> &nikaia::ast::Block {
    parsed
        .program
        .items
        .iter()
        .find_map(|item| match &item.node {
            Item::Fn { body, .. } => Some(body),
            _ => None,
        })
        .expect("a function")
}

#[test]
fn ids_are_unique_within_a_file() {
    let parsed = parse_to_ast(A_FILE).expect("parse failed");
    let ids = ids_in(&format!("{:#?}", parsed.program));
    assert!(ids.len() > 40, "the sample holds many nodes: {}", ids.len());
    let distinct: BTreeSet<u32> = ids.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "two nodes of one file share an id: {ids:?}"
    );
}

#[test]
fn ids_follow_parse_order() {
    let source = "fn main() { let x = a + b * c }";
    let first = parse_to_ast(source).expect("parse failed");
    let second = parse_to_ast(source).expect("parse failed");
    assert_eq!(
        ids_in(&format!("{:#?}", first.program)),
        ids_in(&format!("{:#?}", second.program)),
        "the same source gives the same ids"
    );

    // `a`, `b`, `c`, then `b * c`, then `a + (b * c)`: a node is made after
    // what it holds, and left to right.
    let Stmt::Let { value, .. } = &main_body(&first).stmts[0].node else {
        panic!("a let");
    };
    let Expr::Binary {
        lhs, rhs, id: sum, ..
    } = value
    else {
        panic!("a sum");
    };
    let Expr::Variable(_, a) = &**lhs else {
        panic!("`a`");
    };
    let Expr::Binary {
        lhs: b,
        rhs: c,
        id: product,
        ..
    } = &**rhs
    else {
        panic!("a product");
    };
    let (Expr::Variable(_, b), Expr::Variable(_, c)) = (&**b, &**c) else {
        panic!("`b` and `c`");
    };
    let numbers = [a.get(), b.get(), c.get(), product.get(), sum.get()];
    assert!(
        numbers.windows(2).all(|pair| pair[0] < pair[1]),
        "a, b, c, b * c, a + b * c: {numbers:?}"
    );
}

#[test]
fn two_trees_that_differ_only_in_ids_are_equal() {
    let parsed = parse_to_ast("fn main() {}").expect("parse failed");
    let (low, _) = parse_expression_from(&parsed.interner, "f(a + 1, b)", 0).expect("parse");
    let (high, _) = parse_expression_from(&parsed.interner, "f(a + 1, b)", 500).expect("parse");
    assert_ne!(
        ids_in(&format!("{low:#?}")),
        ids_in(&format!("{high:#?}")),
        "the ids differ"
    );
    assert_eq!(low, high, "and the trees are equal");

    let other = parse_expression(&parsed.interner, "f(a + 2, b)").expect("parse");
    assert_ne!(low, other, "a tree that differs in a value is not equal");
}

#[test]
fn a_hole_takes_ids_the_file_has_not_given() {
    let parsed = parse_to_ast(A_FILE).expect("parse failed");
    let given: BTreeSet<u32> = ids_in(&format!("{:#?}", parsed.program))
        .into_iter()
        .collect();

    let hole = parsed.hole("total + 1").expect("a hole");
    let again = parsed.hole("total + 1").expect("a hole");
    let other = parsed.hole("total * 2").expect("a hole");
    let ids = ids_in(&format!("{hole:#?}"));
    assert!(!ids.is_empty());
    assert!(
        ids.iter().all(|id| !given.contains(id)),
        "a hole's ids are new to the file: {ids:?}"
    );
    assert_eq!(
        ids,
        ids_in(&format!("{again:#?}")),
        "one hole, one set of ids"
    );
    let others = ids_in(&format!("{other:#?}"));
    assert!(
        others.iter().all(|id| !ids.contains(id)),
        "two holes do not share one"
    );
}
