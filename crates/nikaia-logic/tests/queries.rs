//! **Queries through the interface** (ADR-265 D3, D4): facts and a goal as
//! terms, and the reference solver's answer. Nothing here knows Nikaia.

use nikaia_logic::{Answer, Arena, Budget, FourierMotzkin, Query, Solver, TermId, Unknown};

fn check(arena: &Arena, facts: &[TermId], goal: TermId) -> Answer {
    FourierMotzkin.check(&Query { arena, facts, goal }, &Budget::default())
}

/// `f(5)` against `assert(x > 1)`: the argument in place of the parameter.
#[test]
fn a_constant_argument_proves_a_bound() {
    let mut a = Arena::new();
    let (x, five, one) = (a.var("x"), a.int(5), a.int(1));
    let fact = a.eq(x, five);
    let goal = a.gt(x, one);
    assert_eq!(check(&a, &[fact], goal), Answer::Proved);

    let too_much = a.gt(x, five);
    assert_eq!(
        check(&a, &[fact], too_much),
        Answer::Unknown(Unknown::NoContradiction)
    );
}

/// A guard's fact - `n >= 3` after `return 0 if n < 3` - proves `n - 1 > 1`.
#[test]
fn a_guard_proves_what_follows_it() {
    let mut a = Arena::new();
    let (n, three, one) = (a.var("n"), a.int(3), a.int(1));
    let small = a.lt(n, three);
    let guard = a.not(small);
    let less = a.sub(n, one);
    let goal = a.gt(less, one);
    assert_eq!(check(&a, &[guard], goal), Answer::Proved);
    // Without the guard nothing shows it.
    assert_eq!(
        check(&a, &[], goal),
        Answer::Unknown(Unknown::NoContradiction)
    );
}

/// Integers, not rationals: `2x = 1` has no solution, so it proves anything.
#[test]
fn integer_tightening_is_applied() {
    let mut a = Arena::new();
    let (x, two, one) = (a.var("x"), a.int(2), a.int(1));
    let twice = a.mul(two, x);
    let fact = a.eq(twice, one);
    let goal = a.bool(false);
    assert_eq!(check(&a, &[fact], goal), Answer::Proved);
}

/// A product of two variables is outside the theory, and said so.
#[test]
fn a_product_of_variables_is_outside_the_theory() {
    let mut a = Arena::new();
    let (x, y, zero) = (a.var("x"), a.var("y"), a.int(0));
    let product = a.mul(x, y);
    let goal = a.ge(product, zero);
    assert_eq!(
        check(&a, &[], goal),
        Answer::Unknown(Unknown::OutsideTheTheory)
    );
}

/// **The budget is counted in cases, and the answer depends on it alone**
/// (D4): a goal that splits into more cases than allowed is unknown, the
/// same at every run.
#[test]
fn the_budget_bounds_the_work() {
    let mut a = Arena::new();
    let zero = a.int(0);
    let mut facts = Vec::new();
    for k in 0..10 {
        let x = a.var(&format!("x{k}"));
        facts.push(a.ne(x, zero));
    }
    let goal = a.bool(true);
    let tight = Budget {
        cases: 16,
        ..Budget::default()
    };
    let query = Query {
        arena: &a,
        facts: &facts,
        goal,
    };
    assert_eq!(
        FourierMotzkin.check(&query, &tight),
        Answer::Unknown(Unknown::TooManyCases)
    );
    assert_eq!(
        FourierMotzkin.check(&query, &tight),
        FourierMotzkin.check(&query, &tight)
    );
}
