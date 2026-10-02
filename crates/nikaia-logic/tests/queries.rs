//! **Queries through the interface** (ADR-265 D3, D4): facts and a goal as
//! terms, and the reference solver's answer. Nothing here knows Nikaia.

use nikaia_logic::{
    Answer, Arena, Budget, Certificate, FourierMotzkin, Query, Rejected, Solver, Step, TermId,
    Unknown, smtlib, verify,
};

/// The solver's answer, with a *proved* one's certificate checked: every proof
/// in this file goes through the checker too (ADR-265 D5).
fn check(arena: &Arena, facts: &[TermId], goal: TermId) -> Answer {
    let query = Query { arena, facts, goal };
    let answer = FourierMotzkin.check(&query, &Budget::default());
    if let Answer::Proved { certificate } = &answer {
        assert_eq!(verify(&query, certificate), Ok(()), "{certificate:#?}");
    }
    answer
}

fn proved(answer: &Answer) -> bool {
    matches!(answer, Answer::Proved { .. })
}

/// `f(5)` against `assert(x > 1)`: the argument in place of the parameter.
#[test]
fn a_constant_argument_proves_a_bound() {
    let mut a = Arena::new();
    let (x, five, one) = (a.var("x"), a.int(5), a.int(1));
    let fact = a.eq(x, five);
    let goal = a.gt(x, one);
    assert!(proved(&check(&a, &[fact], goal)));

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
    assert!(proved(&check(&a, &[guard], goal)));
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
    assert!(proved(&check(&a, &[fact], goal)));
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

/// **The checker trusts nothing it is handed** (ADR-265 D5): a certificate
/// that leaves a case out, names a bound that is not there, uses a negative
/// multiplier or ends on something that is not a contradiction is rejected.
#[test]
fn a_forged_certificate_is_rejected() {
    let mut a = Arena::new();
    let (x, five, one) = (a.var("x"), a.int(5), a.int(1));
    let fact = a.eq(x, five);
    let goal = a.gt(x, one);
    let facts = [fact];
    let query = Query {
        arena: &a,
        facts: &facts,
        goal,
    };
    let Answer::Proved { certificate } = FourierMotzkin.check(&query, &Budget::default()) else {
        panic!("x = 5 proves x > 1");
    };
    assert_eq!(verify(&query, &certificate), Ok(()));

    // No refutation at all.
    let empty = Certificate { cases: Vec::new() };
    assert!(matches!(
        verify(&query, &empty),
        Err(Rejected::TooFewCases { found: 0 })
    ));

    // A bound the case does not have.
    let mut missing = certificate.clone();
    missing.cases[0].steps[0] = Step::Hypothesis(99);
    assert!(matches!(
        verify(&query, &missing),
        Err(Rejected::Reference { .. })
    ));

    // A multiplier that flips a bound.
    let mut flipped = certificate.clone();
    for step in &mut flipped.cases[0].steps {
        if let Step::Combine { by_left, .. } = step {
            *by_left = -*by_left;
        }
    }
    assert!(matches!(
        verify(&query, &flipped),
        Err(Rejected::Multiplier { .. })
    ));

    // Only the hypotheses: no contradiction is derived.
    let mut short = certificate.clone();
    short.cases[0]
        .steps
        .retain(|s| matches!(s, Step::Hypothesis(_)));
    assert!(matches!(
        verify(&query, &short),
        Err(Rejected::NotAContradiction { .. })
    ));

    // A true certificate for another query does not carry over.
    let too_much = a.gt(x, five);
    let other = Query {
        arena: &a,
        facts: &facts,
        goal: too_much,
    };
    assert!(verify(&other, &certificate).is_err());
}

/// **SMT-LIB 2 at the edge** (ADR-265 D6): a query written out reads the
/// way another solver expects, and read back it gets the same answer.
#[test]
fn a_query_round_trips_through_smtlib() {
    let mut a = Arena::new();
    let (n, len, three, one) = (a.var("n"), a.var("xs.len()"), a.int(3), a.int(1));
    let small = a.lt(n, three);
    let guard = a.not(small);
    let longer = a.ge(len, n);
    let less = a.sub(len, one);
    let goal = a.gt(less, one);
    let facts = [guard, longer];
    let query = Query {
        arena: &a,
        facts: &facts,
        goal,
    };
    let text = smtlib::write(&query);
    assert_eq!(
        text,
        "(set-logic QF_LIA)\n\
         (declare-const n Int)\n\
         (declare-const |xs.len()| Int)\n\
         (assert (not (< n 3)))\n\
         (assert (>= |xs.len()| n))\n\
         (assert (not (> (- |xs.len()| 1) 1)))\n\
         (check-sat)\n"
    );

    let script = smtlib::read(&text).expect("it reads back");
    let falsum = {
        let mut arena = script.arena;
        let f = arena.bool(false);
        (arena, f)
    };
    let (arena, f) = falsum;
    assert!(proved(&check(&arena, &script.assertions, f)));
}

/// A script in the wider language: n-ary operators, implication, comments
/// and `declare-fun` of no arguments; and what is outside the sorts is said.
#[test]
fn a_benchmark_style_script_is_read() {
    let script = smtlib::read(
        "; a comment\n\
         (set-info :status unsat)\n\
         (set-logic QF_LIA)\n\
         (declare-fun a () Int)\n\
         (declare-const b Int)\n\
         (assert (<= 0 a b 10))\n\
         (assert (=> (> a 5) (< b 3)))\n\
         (assert (> a 7))\n\
         (check-sat)\n\
         (exit)\n",
    )
    .expect("it reads");
    let mut arena = script.arena;
    let f = arena.bool(false);
    // a > 7 forces b < 3, and b >= a > 7: unsat.
    assert!(proved(&check(&arena, &script.assertions, f)));

    assert!(matches!(
        smtlib::read("(declare-const r Real)"),
        Err(smtlib::ReadError::Unsupported(_))
    ));
    assert!(matches!(
        smtlib::read("(assert (> z 0))"),
        Err(smtlib::ReadError::Undeclared(z)) if z == "z"
    ));
    assert!(matches!(
        smtlib::read("(assert (> 1 0)"),
        Err(smtlib::ReadError::Syntax(_))
    ));
}
