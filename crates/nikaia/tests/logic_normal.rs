//! **A query in normal form, and an answer as text** (ADR-270 D19): what a
//! `nikaia.proofs` entry is keyed by and holds.

#[path = "common/nikaia_logic.rs"]
mod nikaia_logic;

use nikaia_logic::{
    Answer, Arena, Budget, FourierMotzkin, Normal, Query, Solver, TermId, certificate_of,
    certificate_text, model_of, model_text, verify, verify_model,
};

/// `0 <= i`, `i < n`, `n <= len` ⊢ `i < len`, with the names and the order of
/// the facts given.
fn bounded(names: [&str; 3], reversed: bool) -> (Arena, Vec<TermId>, TermId) {
    let [i, n, len] = names;
    let mut a = Arena::new();
    let (i, n, len, zero) = (a.var(i), a.var(n), a.var(len), a.int(0));
    let mut facts = vec![a.ge(i, zero), a.lt(i, n), a.le(n, len)];
    if reversed {
        facts.reverse();
    }
    let goal = a.lt(i, len);
    (a, facts, goal)
}

fn normal_text(arena: &Arena, facts: &[TermId], goal: TermId) -> String {
    Normal::of(&Query { arena, facts, goal }).text()
}

/// The program's names and the order its facts were written in are not part
/// of the question.
#[test]
fn renamed_and_reordered_queries_share_one_normal_form() {
    let (a, facts, goal) = bounded(["i", "n", "len"], false);
    let (b, other_facts, other_goal) = bounded(["at", "count", "size"], true);
    assert_eq!(
        normal_text(&a, &facts, goal),
        normal_text(&b, &other_facts, other_goal)
    );
}

/// `a >= b` and `b <= a` are one question, and a fact said twice is said once.
#[test]
fn a_turned_comparison_and_a_repeated_fact_change_nothing() {
    let mut a = Arena::new();
    let (x, y) = (a.var("x"), a.var("y"));
    let facts = vec![a.ge(x, y)];
    let goal = a.le(y, x);

    let mut b = Arena::new();
    let (x2, y2) = (b.var("x"), b.var("y"));
    let first = b.le(y2, x2);
    let again = b.le(y2, x2);
    let other_facts = vec![first, again];
    let other_goal = b.ge(x2, y2);
    assert_eq!(
        normal_text(&a, &facts, goal),
        normal_text(&b, &other_facts, other_goal)
    );
}

/// A different question has a different normal form.
#[test]
fn a_different_bound_is_a_different_question() {
    let (a, facts, goal) = bounded(["i", "n", "len"], false);
    let mut b = Arena::new();
    let (i, n, len, one) = (b.var("i"), b.var("n"), b.var("len"), b.int(1));
    let other_facts = vec![b.ge(i, one), b.lt(i, n), b.le(n, len)];
    let other_goal = b.lt(i, len);
    assert_ne!(
        normal_text(&a, &facts, goal),
        normal_text(&b, &other_facts, other_goal)
    );
}

/// The certificate is found on the normal form, checks against it, and
/// survives being written out and read back.
#[test]
fn a_certificate_of_the_normal_form_round_trips_and_checks() {
    let (a, facts, goal) = bounded(["i", "n", "len"], true);
    let normal = Normal::of(&Query {
        arena: &a,
        facts: &facts,
        goal,
    });
    let Answer::Proved { certificate } = FourierMotzkin.check(&normal.query(), &Budget::default())
    else {
        panic!("the bound is proved");
    };
    let text = certificate_text(&certificate);
    assert!(!text.contains(' '), "one word: {text}");
    let read = certificate_of(&text).expect("the text reads back");
    assert_eq!(read, certificate);
    assert_eq!(verify(&normal.query(), &read), Ok(()));
    assert_eq!(certificate_of(&format!("{text}x")), None);
}

/// A split certificate round-trips too: `x = 0 or x = 1` ⊢ `x <= 1`.
#[test]
fn a_split_certificate_round_trips() {
    let mut a = Arena::new();
    let (x, zero, one) = (a.var("x"), a.int(0), a.int(1));
    let (is_zero, is_one) = (a.eq(x, zero), a.eq(x, one));
    let facts = vec![a.or(vec![is_zero, is_one])];
    let goal = a.le(x, one);
    let normal = Normal::of(&Query {
        arena: &a,
        facts: &facts,
        goal,
    });
    let Answer::Proved { certificate } = FourierMotzkin.check(&normal.query(), &Budget::default())
    else {
        panic!("the bound is proved");
    };
    let read = certificate_of(&certificate_text(&certificate)).expect("reads back");
    assert_eq!(read, certificate);
    assert_eq!(verify(&normal.query(), &read), Ok(()));
}

/// A model of the normal form round-trips, checks against it, and is given
/// back in the program's names.
#[test]
fn a_model_round_trips_and_is_named_back() {
    // at >= 3, size >= at + 3; `size > 10` is false at at = 3, size = 6.
    let mut a = Arena::new();
    let (at, size, three, ten) = (a.var("at"), a.var("size"), a.int(3), a.int(10));
    let step = a.add(at, three);
    let facts = vec![a.ge(at, three), a.ge(size, step)];
    let goal = a.gt(size, ten);
    let normal = Normal::of(&Query {
        arena: &a,
        facts: &facts,
        goal,
    });
    let Answer::Refuted { model } = FourierMotzkin.check(&normal.query(), &Budget::default())
    else {
        panic!("`size` may be 6");
    };
    let read = model_of(&model_text(&model)).expect("reads back");
    assert_eq!(read, model);
    assert!(verify_model(&normal.query(), &read));
    let named = normal.named(&model);
    assert_eq!(named.values.get("at"), Some(&3));
    assert_eq!(named.values.get("size"), Some(&6));
    assert_eq!(model_of("-").map(|m| m.values.len()), Some(0));
    assert_eq!(model_of("v0=x"), None);
}
