//! **Queries through the interface** (ADR-265 D3, D4): facts and a goal as
//! terms, and the reference solver's answer. Nothing here knows Nikaia.

use nikaia_logic::{
    Answer, Arena, Budget, Certificate, FourierMotzkin, Model, Query, Refutation, Rejected, Solver,
    Step, TermId, Unknown, smtlib, verify, verify_model,
};

/// The solver's answer, with a *proved* one's certificate checked: every proof
/// in this file goes through the checker too (ADR-265 D5).
fn check(arena: &Arena, facts: &[TermId], goal: TermId) -> Answer {
    let query = Query { arena, facts, goal };
    let answer = FourierMotzkin.check(&query, &Budget::default());
    match &answer {
        Answer::Proved { certificate } => {
            assert_eq!(verify(&query, certificate), Ok(()), "{certificate:#?}");
        }
        Answer::Refuted { model } => assert!(verify_model(&query, model), "{model:#?}"),
        Answer::Unknown(_) => {}
    }
    answer
}

fn refuted(values: &[(&str, i64)]) -> Answer {
    Answer::Refuted {
        model: Model {
            values: values.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        },
    }
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

    // x = 5 does not prove x > 5, and the model says why.
    let too_much = a.gt(x, five);
    assert_eq!(check(&a, &[fact], too_much), refuted(&[("x", 5)]));
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
    // Without the guard nothing shows it: n = 0 is the value nearest zero
    // that makes `n - 1 > 1` false.
    assert_eq!(check(&a, &[], goal), refuted(&[("n", 0)]));
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

/// **The budget bounds the work** (D4): a goal whose proof needs more
/// branches than allowed is unknown, the same at every run. Ten numbers, each
/// `-1` or `1`, never add up to `1` - but only every one of the 1024 ways of
/// choosing them shows it.
#[test]
fn the_budget_bounds_the_work() {
    let mut a = Arena::new();
    let (zero, one, minus) = (a.int(0), a.int(1), a.int(-1));
    let mut facts = Vec::new();
    let mut sum = zero;
    for k in 0..10 {
        let x = a.var(&format!("x{k}"));
        let (low, high, nonzero) = (a.ge(x, minus), a.le(x, one), a.ne(x, zero));
        facts.push(a.and(vec![low, high, nonzero]));
        sum = a.add(sum, x);
    }
    let goal = a.ne(sum, one);
    let tight = Budget {
        cases: 64,
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

fn steps(certificate: &mut Certificate) -> &mut Vec<Step> {
    match certificate {
        Certificate::Refuted(refutation) => &mut refutation.steps,
        Certificate::Split { .. } => panic!("no split was expected"),
    }
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
    let empty = Certificate::Refuted(Refutation { steps: Vec::new() });
    assert!(matches!(
        verify(&query, &empty),
        Err(Rejected::NotAContradiction { case: 0 })
    ));

    // An atom the branch does not hold.
    let mut missing = certificate.clone();
    steps(&mut missing)[0] = Step::Hypothesis(99);
    assert!(matches!(
        verify(&query, &missing),
        Err(Rejected::Reference { .. })
    ));

    // A multiplier that flips a bound.
    let mut flipped = certificate.clone();
    for step in steps(&mut flipped) {
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
    steps(&mut short).retain(|s| matches!(s, Step::Hypothesis(_)));
    assert!(matches!(
        verify(&query, &short),
        Err(Rejected::NotAContradiction { .. })
    ));

    // A split of a disjunction the query does not have.
    let split = Certificate::Split {
        disjunction: 0,
        cases: vec![certificate.clone(), certificate.clone()],
    };
    assert_eq!(
        verify(&query, &split),
        Err(Rejected::NotOpen { disjunction: 0 })
    );

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

/// **A model is the nearest to zero the bounds allow, and it is checked**
/// (ADR-265 D3, D4): back-substitution through two eliminations, a value
/// pushed off zero by a bound, and one forced by a chain.
#[test]
fn a_model_is_found_through_the_eliminations() {
    let mut a = Arena::new();
    let (x, y, three, ten) = (a.var("x"), a.var("y"), a.int(3), a.int(10));
    // x >= 3, y >= x + 3; the goal y > 10 is false at x = 3, y = 6.
    let low = a.ge(x, three);
    let step = a.add(x, three);
    let above = a.ge(y, step);
    let goal = a.gt(y, ten);
    assert_eq!(
        check(&a, &[low, above], goal),
        refuted(&[("x", 3), ("y", 6)])
    );

    // Integers matter: 2x = 2y + 1 has no integer solution, so nothing is
    // false - the facts prove anything.
    let (two, one) = (a.int(2), a.int(1));
    let twice_x = a.mul(two, x);
    let twice_y = a.mul(two, y);
    let odd = a.add(twice_y, one);
    let parity = a.eq(twice_x, odd);
    let falsum = a.bool(false);
    assert!(proved(&check(&a, &[parity], falsum)));
}

/// Write the query's Alethe proof, and where `NIKAIA_CARCARA` names the
/// Carcara binary, have it check the proof against the query's own SMT-LIB
/// script.
fn alethe_checked(query: &Query<'_>) -> String {
    let proof = nikaia_logic::alethe::write(query).expect("a proof");
    if let Some(carcara) = std::env::var_os("NIKAIA_CARCARA") {
        let dir = std::env::temp_dir().join(format!(
            "nikaia-alethe-{}-{}",
            std::process::id(),
            proof.len()
        ));
        std::fs::create_dir_all(&dir).expect("a directory");
        std::fs::write(dir.join("q.smt2"), smtlib::write(query)).expect("the problem");
        std::fs::write(dir.join("q.alethe"), &proof).expect("the proof");
        let out = std::process::Command::new(carcara)
            .arg("check")
            .arg(dir.join("q.alethe"))
            .arg(dir.join("q.smt2"))
            .output()
            .expect("carcara runs");
        let said = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            said.trim(),
            "valid",
            "{proof}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        std::fs::remove_dir_all(&dir).ok();
    }
    proof
}

/// **A proof as Alethe** (ADR-265 D6), written for another checker to check:
/// a conjunction, whose refutation is one choice - an equality's two bounds,
/// a combination, an integer tightening - and one that splits, through a
/// disjunction and a `!=`, into choices folded back by resolution.
#[test]
fn a_proof_is_written_as_alethe() {
    let mut a = Arena::new();
    let (x, five, one) = (a.var("x"), a.int(5), a.int(1));
    let (two, y) = (a.int(2), a.var("y"));
    let fact = a.eq(x, five);
    let twice = a.mul(two, y);
    let odd = a.add(twice, one);
    let parity = a.le(odd, x);
    let above = a.ge(odd, x);
    let goal = a.gt(x, one);
    let facts = [fact, parity, above];
    let query = Query {
        arena: &a,
        facts: &facts,
        goal,
    };
    let proof = alethe_checked(&query);
    assert!(proof.starts_with("(assume h0 (= x 5))\n"), "{proof}");
    assert!(proof.contains(":rule la_generic"), "{proof}");
    assert!(proof.contains("(cl) :rule resolution"), "{proof}");

    // `mode == 1 → x - 1 > 0` with `x = 5`, and `y != 3` with `y >= 3`:
    // the goal `y > 3 && (mode != 1 || x > 1)` splits into choices.
    let (mode, three) = (a.var("mode"), a.int(3));
    let is_one = a.eq(mode, one);
    let taken = a.not(is_one);
    let less = a.sub(x, one);
    let zero = a.int(0);
    let shifted = a.gt(less, zero);
    let implied = a.or(vec![taken, shifted]);
    let not_three = a.ne(y, three);
    let at_least = a.ge(y, three);
    let past = a.gt(y, three);
    let bigger = a.gt(x, one);
    let either = a.or(vec![taken, bigger]);
    let goal = a.and(vec![past, either]);
    let facts = [fact, implied, not_three, at_least];
    let query = Query {
        arena: &a,
        facts: &facts,
        goal,
    };
    let proof = alethe_checked(&query);
    for rule in ["or_pos", "and_neg", "la_disequality", "resolution"] {
        assert!(proof.contains(rule), "{rule}\n{proof}");
    }
}

/// **A number is an `i64`** (ADR-265 D2): a combination whose coefficients
/// leave it answers *unknown* for its reason, never a wrong answer. Here the
/// facts hold at `x = 1, y = 1`, and eliminating either name multiplies a
/// coefficient near `2^62` by `5` or `7`.
#[test]
fn a_step_that_leaves_i64_is_unknown_and_never_wrong() {
    let mut a = Arena::new();
    let (x, y) = (a.var("x"), a.var("y"));
    let (big, near, five, seven) = (
        a.int(i64::MAX / 2),
        a.int(i64::MAX / 2 - 1),
        a.int(5),
        a.int(7),
    );
    let (one, two) = (a.int(1), a.int(2));
    let (bx, ty) = (a.mul(big, x), a.mul(near, y));
    let left = a.sub(bx, ty);
    let first = a.ge(left, one);
    let (fx, sy) = (a.mul(five, x), a.mul(seven, y));
    let right = a.sub(fx, sy);
    let second = a.le(right, two);
    let falsum = a.bool(false);
    let answer = check(&a, &[first, second], falsum);
    assert!(!proved(&answer), "{answer:?}");
    assert_eq!(answer, Answer::Unknown(Unknown::Overflow), "{answer:?}");
}

/// How many refutations a certificate holds, and whether it splits at all.
fn leaves(certificate: &Certificate) -> usize {
    match certificate {
        Certificate::Refuted(_) => 1,
        Certificate::Split { cases, .. } => cases.iter().map(leaves).sum(),
    }
}

fn certificate(answer: Answer) -> Certificate {
    match answer {
        Answer::Proved { certificate } => certificate,
        other => panic!("not proved: {other:?}"),
    }
}

/// **A disjunction is split where a branch needs it** (D4): thirty-two
/// guards `return … if x == k` leave `x != 0`, …, `x != 31`, which with
/// `0 <= x <= 32` prove `x == 32`. Taken apart up front that is 2^33 cases;
/// split on demand, each split has one alternative refuted at once.
#[test]
fn guards_are_split_on_demand() {
    let mut a = Arena::new();
    let x = a.var("x");
    let (zero, top) = (a.int(0), a.int(32));
    let mut facts = vec![a.ge(x, zero), a.le(x, top)];
    for k in 0..32 {
        let k = a.int(k);
        facts.push(a.ne(x, k));
    }
    let goal = a.eq(x, top);
    let proof = certificate(check(&a, &facts, goal));
    assert!(leaves(&proof) <= 2 * 33 + 2, "{}", leaves(&proof));
    // The Alethe proof is searched the same way.
    let query = Query {
        arena: &a,
        facts: &facts,
        goal,
    };
    alethe_checked(&query);

    // Without the guard on 31, `x` may be 31: the model says so.
    let mut fewer = facts.clone();
    fewer.remove(2 + 31);
    assert_eq!(check(&a, &fewer, goal), refuted(&[("x", 31)]));
}

/// **A split that was not needed is dropped** (D4): `y != 0` for twenty
/// names says nothing about `x`, and the proof of `x > 3` from `x > 5` does
/// not split one of them.
#[test]
fn a_disjunction_the_proof_does_not_need_is_not_split() {
    let mut a = Arena::new();
    let (x, zero, three, five) = (a.var("x"), a.int(0), a.int(3), a.int(5));
    let mut facts = Vec::new();
    for k in 0..20 {
        let y = a.var(&format!("y{k}"));
        facts.push(a.ne(y, zero));
    }
    facts.push(a.gt(x, five));
    let goal = a.gt(x, three);
    let proof = certificate(check(&a, &facts, goal));
    assert!(matches!(proof, Certificate::Refuted(_)), "{proof:?}");
}

/// **What every alternative says holds without a split**: a path of twenty
/// steps, each `+1` or `+2`, ends at least twenty past where it began. Each
/// step's two alternatives agree on `z' >= z + 1`, and that is the proof - one
/// refutation, where the cases would be 2^20.
#[test]
fn what_every_alternative_says_needs_no_split() {
    let mut a = Arena::new();
    let (one, two) = (a.int(1), a.int(2));
    let mut z = a.var("z0");
    let zero = a.int(0);
    let mut facts = vec![a.eq(z, zero)];
    for k in 1..=20 {
        let next = a.var(&format!("z{k}"));
        let (by_one, by_two) = (a.add(z, one), a.add(z, two));
        let (step_one, step_two) = (a.eq(next, by_one), a.eq(next, by_two));
        facts.push(a.or(vec![step_one, step_two]));
        z = next;
    }
    let twenty = a.int(20);
    let goal = a.ge(z, twenty);
    let proof = certificate(check(&a, &facts, goal));
    assert_eq!(leaves(&proof), 1, "{proof:?}");
    // In Alethe each step's agreement is a lemma per alternative, resolved.
    let query = Query {
        arena: &a,
        facts: &facts,
        goal,
    };
    alethe_checked(&query);

    // Forty is not reached on every path, and the model takes `+1` each time.
    let forty = a.int(40);
    let far = a.ge(z, forty);
    assert!(matches!(check(&a, &facts, far), Answer::Refuted { .. }));
}

/// **A split must answer every alternative** (D5): a certificate that drops
/// one is rejected, and so is one that splits a disjunction twice.
#[test]
fn a_split_that_leaves_an_alternative_out_is_rejected() {
    let mut a = Arena::new();
    let (x, zero, one, minus) = (a.var("x"), a.int(0), a.int(1), a.int(-1));
    let facts = [a.ne(x, zero), a.ge(x, minus), a.le(x, one)];
    let (low, high) = (a.eq(x, minus), a.eq(x, one));
    let goal = a.or(vec![low, high]);
    let query = Query {
        arena: &a,
        facts: &facts,
        goal,
    };
    let proof = certificate(check(&a, &facts, goal));
    let Certificate::Split { disjunction, cases } = &proof else {
        panic!("`x != 0` is split: {proof:?}");
    };
    assert_eq!(verify(&query, &proof), Ok(()));

    let short = Certificate::Split {
        disjunction: *disjunction,
        cases: cases[..1].to_vec(),
    };
    assert_eq!(
        verify(&query, &short),
        Err(Rejected::Cases {
            expected: 2,
            found: 1
        })
    );
    let twice = Certificate::Split {
        disjunction: *disjunction,
        cases: vec![proof.clone(), proof.clone()],
    };
    assert_eq!(
        verify(&query, &twice),
        Err(Rejected::NotOpen {
            disjunction: *disjunction
        })
    );
}
