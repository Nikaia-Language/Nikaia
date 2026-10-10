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

    let hole = parsed.hole(7, "total + 1").expect("a hole");
    let again = parsed.hole(7, "total + 1").expect("a hole");
    let other = parsed.hole(7, "total * 2").expect("a hole");
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

    // The same text in another template is another hole: what is recorded
    // about one is not read at the other.
    let elsewhere = parsed.hole(8, "total + 1").expect("a hole");
    let apart = ids_in(&format!("{elsewhere:#?}"));
    assert!(
        apart.iter().all(|id| !ids.contains(id)),
        "{apart:?} / {ids:?}"
    );
    assert_eq!(hole, elsewhere, "and the trees are equal (D3)");
}

/// A program the text-tier pass rewrites: a value of either kind is handed
/// over as `value.into_either()` (ADR-282 D14).
const REWRITTEN: &str = r##"use std::fs

fn shout(s: String) -> String {
    return f"{s}!"
}

fn main() throws {
    let text = fs::read_to_string("extra.conf", fs::Root::Anywhere)
    let mut last: String = f"nothing"
    for line in text.lines() {
        last = line.trim()
    }
    println(f"{shout(last)} {shout(f"x")}")
}
"##;

#[test]
fn a_rewritten_tree_gives_every_node_an_id_of_its_own() {
    let parsed = parse_to_ast(REWRITTEN).expect("parse failed");
    let ids = ids_in(&format!("{:#?}", parsed.program));
    let built = ids
        .iter()
        .filter(|id| **id >= nikaia::ast::FIRST_BUILT)
        .count();
    assert!(built > 0, "the tier pass wrapped something: {ids:?}");
    assert!(
        ids.iter().any(|id| *id < nikaia::ast::FIRST_BUILT),
        "and the parser's nodes are still there"
    );
    let distinct: BTreeSet<u32> = ids.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "no two nodes share an id: {ids:?}"
    );
}

#[test]
fn a_wrapped_hole_keeps_its_ids_and_the_wrapper_takes_a_new_one() {
    let parsed = parse_to_ast("fn main() {}").expect("parse failed");
    let (mut hole, _) = parse_expression_from(&parsed.interner, "a + b", 0).expect("parse");
    let before = ids_in(&format!("{hole:#?}"));
    let node = nikaia::check::value_node(&hole);
    let wrap = parsed.interner.intern_string("into_either");
    nikaia::text_tiers::wrap_hole(&mut hole, &[(node, wrap)]);

    let after = ids_in(&format!("{hole:#?}"));
    assert_eq!(after.len(), before.len() + 1, "one node more: {after:?}");
    assert!(
        before.iter().all(|id| after.contains(id)),
        "the wrapped tree has the ids it had: {before:?} in {after:?}"
    );
    let wrapper: Vec<_> = after.iter().filter(|id| !before.contains(id)).collect();
    assert_eq!(wrapper.len(), 1);
    assert!(*wrapper[0] >= nikaia::ast::FIRST_BUILT);
}

#[test]
fn a_cloned_subtree_has_the_ids_of_the_original() {
    let parsed = parse_to_ast(A_FILE).expect("parse failed");
    let original = format!("{:#?}", main_body(&parsed));
    let copy = main_body(&parsed).clone();
    assert_eq!(ids_in(&original), ids_in(&format!("{copy:#?}")));
    assert!(!ids_in(&original).is_empty());
}

#[test]
fn a_node_the_parser_places_twice_is_two_nodes() {
    // `xs[a..]` is `xs[a..<xs.len()]`: `xs` stands as the base and as the
    // receiver of `len`, and what is recorded about one place is not the other's.
    let source = "fn main() { let xs = [1, 2, 3]\nlet rest = xs[1..] }";
    let parsed = parse_to_ast(source).expect("parse failed");
    let ids = ids_in(&format!("{:#?}", parsed.program));
    let distinct: BTreeSet<u32> = ids.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "no two nodes share an id: {ids:?}"
    );
    // And the copy is equal to the original, ids apart (D3).
    let Stmt::Let { value, .. } = &main_body(&parsed).stmts[1].node else {
        panic!("a let");
    };
    let Expr::Index { base, index, .. } = value else {
        panic!("an index");
    };
    let Expr::Range { end, .. } = &**index else {
        panic!("a range");
    };
    let Expr::MethodCall { receiver, .. } = &**end else {
        panic!("a call of len");
    };
    assert_eq!(base, receiver);
    assert_ne!(nikaia::ast::id_of(base), nikaia::ast::id_of(receiver));
}
