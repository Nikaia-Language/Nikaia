// crates/nikaia-logic/src/lia.rs
//
// **The reference solver: linear integer arithmetic** (ADR-265 D1, ADR-264
// D10). A goal is proved when every case of its negation, joined with the
// facts, has no integer solution. Fourier-Motzkin elimination over the
// rationals decides that from one side: where it finds a contradiction there
// is none over the rationals, so none over the integers; each derived bound is
// tightened to the integers on the way, which is what lets `x < 5` and `x > 4`
// contradict. It may fail to find a contradiction that exists, and never finds
// one that does not.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Answer, Arena, Budget, Certificate, Model, Query, Refutation, Solver, Step, Term, TermId,
    Unknown, verify_model,
};

/// Fourier-Motzkin elimination with integer tightening.
#[derive(Debug, Default, Clone, Copy)]
pub struct FourierMotzkin;

impl Solver for FourierMotzkin {
    fn check(&self, query: &Query<'_>, budget: &Budget) -> Answer {
        let cases = match query_cases(query, budget) {
            Ok(cases) => cases,
            Err(why) => return Answer::Unknown(why),
        };
        let mut refutations = Vec::with_capacity(cases.len());
        for case in cases {
            match contradictory(case, budget) {
                Ok(refutation) => refutations.push(refutation),
                Err(Open::Consistent(Some(values))) => {
                    return refuted(query, values)
                        .map_or(Answer::Unknown(Unknown::NoContradiction), |model| {
                            Answer::Refuted { model }
                        });
                }
                Err(Open::Consistent(None)) => {
                    return Answer::Unknown(Unknown::NoContradiction);
                }
                Err(Open::Unknown(why)) => return Answer::Unknown(why),
            }
        }
        Answer::Proved {
            certificate: Certificate { cases: refutations },
        }
    }
}

/// A case's values made a model of the whole query: every variable the query
/// reads is given - zero where the case did not mention it - and the model is
/// checked by evaluating the query, so a fault in the back-substitution is an
/// *unknown*, never a wrong model.
fn refuted(query: &Query<'_>, mut values: BTreeMap<String, i128>) -> Option<Model> {
    let mut names = BTreeSet::new();
    for id in query.facts.iter().chain([&query.goal]) {
        query.arena.variables(*id, &mut names);
    }
    for name in names {
        values.entry(name).or_insert(0);
    }
    let model = Model { values };
    verify_model(query, &model).then_some(model)
}

/// How a case ended that is not a contradiction.
#[derive(Debug)]
enum Open {
    /// The elimination ran out of variables without one: the bounds are
    /// consistent over the rationals, with integer values where back-
    /// substitution found them.
    Consistent(Option<BTreeMap<String, i128>>),
    /// The solver could not tell.
    Unknown(Unknown),
}

/// The query's facts and its goal's negation, as a disjunction of conjunctions
/// of bounds: every way the goal could be false. The solver and the checker
/// both start here, so a certificate is about the same cases the solver saw.
pub(crate) fn query_cases(query: &Query<'_>, budget: &Budget) -> Result<Vec<Vec<Lin>>, Unknown> {
    let mut all = Vec::with_capacity(query.facts.len() + 1);
    for fact in query.facts {
        all.push(formula(query.arena, *fact, true)?);
    }
    all.push(formula(query.arena, query.goal, false)?);
    cases(&Formula::And(all), budget).ok_or(Unknown::TooManyCases)
}

/// `Σ coefficient·name + constant`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lin {
    pub(crate) terms: BTreeMap<String, i128>,
    pub(crate) constant: i128,
}

impl Lin {
    fn constant(c: i128) -> Lin {
        Lin {
            terms: BTreeMap::new(),
            constant: c,
        }
    }

    fn var(name: &str) -> Lin {
        Lin {
            terms: BTreeMap::from([(name.to_string(), 1)]),
            constant: 0,
        }
    }

    pub(crate) fn add(&self, other: &Lin) -> Option<Lin> {
        let mut out = self.clone();
        for (name, k) in &other.terms {
            let entry = out.terms.entry(name.clone()).or_insert(0);
            *entry = entry.checked_add(*k)?;
            if *entry == 0 {
                out.terms.remove(name);
            }
        }
        out.constant = out.constant.checked_add(other.constant)?;
        Some(out)
    }

    pub(crate) fn scale(&self, k: i128) -> Option<Lin> {
        if k == 0 {
            return Some(Lin::constant(0));
        }
        let mut terms = BTreeMap::new();
        for (name, c) in &self.terms {
            terms.insert(name.clone(), c.checked_mul(k)?);
        }
        Some(Lin {
            terms,
            constant: self.constant.checked_mul(k)?,
        })
    }

    fn sub(&self, other: &Lin) -> Option<Lin> {
        self.add(&other.scale(-1)?)
    }

    fn add_const(&self, c: i128) -> Lin {
        let mut out = self.clone();
        out.constant = out.constant.saturating_add(c);
        out
    }

    /// Divide through by the coefficients' common factor and round the bound
    /// towards the integers: `2x + 3 <= 0` is `x + 2 <= 0` over the integers.
    pub(crate) fn tightened(mut self) -> Lin {
        let g = self.terms.values().fold(0i128, |g, k| gcd(g, k.abs()));
        if g > 1 {
            for k in self.terms.values_mut() {
                *k /= g;
            }
            // ceil(constant / g)
            self.constant = self.constant.div_euclid(g)
                + if self.constant.rem_euclid(g) == 0 {
                    0
                } else {
                    1
                };
        }
        self
    }
}

fn gcd(a: i128, b: i128) -> i128 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// A formula over `lin <= 0` atoms, with negation already pushed inward.
#[derive(Debug, Clone)]
enum Formula {
    True,
    False,
    /// `lin <= 0`.
    Le(Lin),
    And(Vec<Formula>),
    Or(Vec<Formula>),
}

/// An integer term as a linear expression.
fn linear(arena: &Arena, id: TermId) -> Result<Lin, Unknown> {
    let overflow = Unknown::Overflow;
    match arena.get(id) {
        Term::Int(n) => Ok(Lin::constant(*n)),
        Term::Var(name) => Ok(Lin::var(name)),
        Term::Add(a, b) => linear(arena, *a)?.add(&linear(arena, *b)?).ok_or(overflow),
        Term::Sub(a, b) => linear(arena, *a)?.sub(&linear(arena, *b)?).ok_or(overflow),
        Term::Neg(a) => linear(arena, *a)?.scale(-1).ok_or(overflow),
        Term::Mul(a, b) => {
            let (l, r) = (linear(arena, *a)?, linear(arena, *b)?);
            if l.terms.is_empty() {
                r.scale(l.constant).ok_or(overflow)
            } else if r.terms.is_empty() {
                l.scale(r.constant).ok_or(overflow)
            } else {
                Err(Unknown::OutsideTheTheory)
            }
        }
        _ => Err(Unknown::OutsideTheTheory),
    }
}

/// A Boolean term as a formula, or its negation where `holds` is false.
fn formula(arena: &Arena, id: TermId, holds: bool) -> Result<Formula, Unknown> {
    let overflow = Unknown::Overflow;
    let compare = |a: TermId, b: TermId, op: Compare| -> Result<Formula, Unknown> {
        let a = linear(arena, a)?;
        let b = linear(arena, b)?;
        let op = if holds { op } else { op.negated() };
        let a_minus_b = a.sub(&b).ok_or(overflow)?;
        let b_minus_a = b.sub(&a).ok_or(overflow)?;
        Ok(match op {
            // a < b  is  a - b + 1 <= 0 over the integers.
            Compare::Lt => Formula::Le(a_minus_b.add_const(1)),
            Compare::Le => Formula::Le(a_minus_b),
            Compare::Gt => Formula::Le(b_minus_a.add_const(1)),
            Compare::Ge => Formula::Le(b_minus_a),
            Compare::Eq => Formula::And(vec![Formula::Le(a_minus_b), Formula::Le(b_minus_a)]),
            Compare::Ne => Formula::Or(vec![
                Formula::Le(a_minus_b.add_const(1)),
                Formula::Le(b_minus_a.add_const(1)),
            ]),
        })
    };
    match arena.get(id) {
        Term::Bool(b) => Ok(if *b == holds {
            Formula::True
        } else {
            Formula::False
        }),
        Term::Not(a) => formula(arena, *a, !holds),
        Term::And(parts) | Term::Or(parts) => {
            let conjunction = matches!(arena.get(id), Term::And(_)) == holds;
            let parts = parts
                .iter()
                .map(|p| formula(arena, *p, holds))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(if conjunction {
                Formula::And(parts)
            } else {
                Formula::Or(parts)
            })
        }
        Term::Le(a, b) => compare(*a, *b, Compare::Le),
        Term::Lt(a, b) => compare(*a, *b, Compare::Lt),
        Term::Ge(a, b) => compare(*a, *b, Compare::Ge),
        Term::Gt(a, b) => compare(*a, *b, Compare::Gt),
        Term::Eq(a, b) => compare(*a, *b, Compare::Eq),
        Term::Ne(a, b) => compare(*a, *b, Compare::Ne),
        _ => Err(Unknown::OutsideTheTheory),
    }
}

#[derive(Debug, Clone, Copy)]
enum Compare {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl Compare {
    fn negated(self) -> Compare {
        match self {
            Compare::Lt => Compare::Ge,
            Compare::Le => Compare::Gt,
            Compare::Gt => Compare::Le,
            Compare::Ge => Compare::Lt,
            Compare::Eq => Compare::Ne,
            Compare::Ne => Compare::Eq,
        }
    }
}

/// The formula as a disjunction of conjunctions of bounds, or `None` past the
/// budget's cases.
fn cases(formula: &Formula, budget: &Budget) -> Option<Vec<Vec<Lin>>> {
    match formula {
        Formula::True => Some(vec![Vec::new()]),
        Formula::False => Some(Vec::new()),
        Formula::Le(lin) => Some(vec![vec![lin.clone()]]),
        Formula::Or(parts) => {
            let mut out = Vec::new();
            for part in parts {
                out.extend(cases(part, budget)?);
                if out.len() > budget.cases {
                    return None;
                }
            }
            Some(out)
        }
        Formula::And(parts) => {
            let mut out: Vec<Vec<Lin>> = vec![Vec::new()];
            for part in parts {
                let each = cases(part, budget)?;
                let mut next = Vec::new();
                for left in &out {
                    for right in &each {
                        let mut both = left.clone();
                        both.extend(right.iter().cloned());
                        next.push(both);
                        if next.len() > budget.cases {
                            return None;
                        }
                    }
                }
                out = next;
            }
            Some(out)
        }
    }
}

/// A bound the elimination holds, and the step that made it.
type Held = (Lin, usize);

/// Fourier-Motzkin with integer tightening: where the bounds, all `<= 0`,
/// have no integer solution, the derivation of `c <= 0` with `c > 0` that
/// shows it (ADR-265 D5); why not where it cannot tell.
///
/// Every bound the elimination holds remembers the step that made it, so the
/// contradiction it ends on can be traced back to the case's own bounds, and
/// only the steps on that trace are kept.
fn contradictory(bounds: Vec<Lin>, budget: &Budget) -> Result<Refutation, Open> {
    let mut steps: Vec<Step> = Vec::new();
    // Each elimination's name and the bounds that held it, for values later.
    let mut rounds: Vec<(String, Vec<Lin>)> = Vec::new();
    let mut bounds: Vec<Held> = bounds
        .into_iter()
        .enumerate()
        .map(|(i, bound)| {
            steps.push(Step::Hypothesis(i));
            tightened(bound, &mut steps)
        })
        .collect();
    loop {
        // A bound with no variables left is a verdict: `c <= 0`.
        if let Some((_, step)) = bounds
            .iter()
            .find(|(b, _)| b.terms.is_empty() && b.constant > 0)
        {
            return Ok(traced(steps, *step));
        }
        bounds.retain(|(b, _)| !b.terms.is_empty());
        bounds.sort_by(|a, b| format!("{:?}", a.0).cmp(&format!("{:?}", b.0)));
        bounds.dedup_by(|later, earlier| later.0 == earlier.0);
        let names: BTreeSet<String> = bounds
            .iter()
            .flat_map(|(b, _)| b.terms.keys().cloned())
            .collect();
        // Eliminate the name whose elimination makes the fewest new bounds.
        let Some(name) = names.into_iter().min_by_key(|name| {
            let up = bounds
                .iter()
                .filter(|(b, _)| b.terms.get(name).is_some_and(|k| *k > 0))
                .count();
            let down = bounds
                .iter()
                .filter(|(b, _)| b.terms.get(name).is_some_and(|k| *k < 0))
                .count();
            up * down
        }) else {
            return Err(Open::Consistent(back_substituted(&rounds)));
        };
        let (with, without): (Vec<Held>, Vec<Held>) = bounds
            .into_iter()
            .partition(|(b, _)| b.terms.contains_key(&name));
        rounds.push((name.clone(), with.iter().map(|(b, _)| b.clone()).collect()));
        let (up, down): (Vec<Held>, Vec<Held>) =
            with.into_iter().partition(|(b, _)| b.terms[&name] > 0);
        let mut next = without;
        for (u, u_step) in &up {
            for (d, d_step) in &down {
                let a = u.terms[&name];
                let b = -d.terms[&name];
                // b·u + a·d cancels `name`; both are `<= 0`, so is the sum.
                let (Some(left), Some(right)) = (u.scale(b), d.scale(a)) else {
                    return Err(Open::Unknown(Unknown::Overflow));
                };
                let Some(sum) = left.add(&right) else {
                    return Err(Open::Unknown(Unknown::Overflow));
                };
                steps.push(Step::Combine {
                    left: *u_step,
                    by_left: b,
                    right: *d_step,
                    by_right: a,
                });
                next.push(tightened(sum, &mut steps));
                if next.len() > budget.bounds {
                    return Err(Open::Unknown(Unknown::TooManyBounds));
                }
            }
        }
        bounds = next;
    }
}

/// Integer values for the eliminated names, last eliminated first: each one
/// as near to zero as the bounds that held it allow, given the values of the
/// names eliminated after it. `None` where a name's bounds leave no integer.
fn back_substituted(rounds: &[(String, Vec<Lin>)]) -> Option<BTreeMap<String, i128>> {
    let mut values: BTreeMap<String, i128> = BTreeMap::new();
    for (name, bounds) in rounds.iter().rev() {
        let (mut low, mut high): (Option<i128>, Option<i128>) = (None, None);
        for bound in bounds {
            // k·name + rest <= 0
            let k = bound.terms[name];
            let mut rest = bound.constant;
            for (other, c) in &bound.terms {
                if other != name {
                    rest = rest.checked_add(c.checked_mul(*values.get(other)?)?)?;
                }
            }
            if k > 0 {
                // name <= floor(-rest / k)
                let limit = rest.checked_neg()?.div_euclid(k);
                high = Some(high.map_or(limit, |h| h.min(limit)));
            } else {
                // name >= ceil(rest / -k)
                let limit = rest
                    .checked_neg()?
                    .div_euclid(k.checked_neg()?)
                    .checked_neg()?;
                low = Some(low.map_or(limit, |l| l.max(limit)));
            }
        }
        let value = match (low, high) {
            (Some(l), Some(h)) if l > h => return None,
            (Some(l), _) if l > 0 => l,
            (_, Some(h)) if h < 0 => h,
            _ => 0,
        };
        values.insert(name.clone(), value);
    }
    Some(values)
}

/// The bound the last step made, tightened to the integers - with a step of
/// its own where tightening changed it.
fn tightened(bound: Lin, steps: &mut Vec<Step>) -> Held {
    let made = steps.len() - 1;
    let tight = bound.clone().tightened();
    if tight == bound {
        (bound, made)
    } else {
        steps.push(Step::Tighten(made));
        (tight, steps.len() - 1)
    }
}

/// Only the steps `last` was derived from, in their order, renumbered.
fn traced(steps: Vec<Step>, last: usize) -> Refutation {
    let mut needed = vec![false; steps.len()];
    needed[last] = true;
    for at in (0..=last).rev() {
        if !needed[at] {
            continue;
        }
        match steps[at] {
            Step::Hypothesis(_) => {}
            Step::Tighten(from) => needed[from] = true,
            Step::Combine { left, right, .. } => {
                needed[left] = true;
                needed[right] = true;
            }
        }
    }
    let mut renumbered = vec![usize::MAX; steps.len()];
    let mut kept = Vec::new();
    for (at, step) in steps.into_iter().enumerate().take(last + 1) {
        if !needed[at] {
            continue;
        }
        renumbered[at] = kept.len();
        kept.push(match step {
            Step::Hypothesis(i) => Step::Hypothesis(i),
            Step::Tighten(from) => Step::Tighten(renumbered[from]),
            Step::Combine {
                left,
                by_left,
                right,
                by_right,
            } => Step::Combine {
                left: renumbered[left],
                by_left,
                right: renumbered[right],
                by_right,
            },
        });
    }
    Refutation { steps: kept }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le(terms: &[(&str, i128)], constant: i128) -> Lin {
        Lin {
            terms: terms.iter().map(|(n, k)| (n.to_string(), *k)).collect(),
            constant,
        }
    }

    fn contradicts(bounds: Vec<Lin>) -> bool {
        contradictory(bounds, &Budget::default()).is_ok()
    }

    #[test]
    fn a_refutation_keeps_only_the_steps_it_needs() {
        // x <= 5, an unrelated y <= 0, and x >= 6: the y bound is not traced.
        let refutation = contradictory(
            vec![le(&[("x", 1)], -5), le(&[("y", 1)], 0), le(&[("x", -1)], 6)],
            &Budget::default(),
        )
        .expect("contradictory");
        assert_eq!(
            refutation.steps,
            [
                Step::Hypothesis(0),
                Step::Hypothesis(2),
                Step::Combine {
                    left: 0,
                    by_left: 1,
                    right: 1,
                    by_right: 1
                },
            ]
        );
    }

    #[test]
    fn a_bound_and_its_opposite_contradict() {
        // x - 5 <= 0 and 6 - x <= 0: x <= 5 and x >= 6.
        assert!(contradicts(vec![le(&[("x", 1)], -5), le(&[("x", -1)], 6)]));
        // x <= 5 and x >= 5 is x = 5: no contradiction.
        assert!(!contradicts(vec![le(&[("x", 1)], -5), le(&[("x", -1)], 5)]));
    }

    #[test]
    fn integer_tightening_finds_what_the_rationals_miss() {
        // 2x <= 1 and 2x >= 1 has x = 1/2 over the rationals and nothing over
        // the integers.
        assert!(contradicts(vec![le(&[("x", 2)], -1), le(&[("x", -2)], 1)]));
    }

    #[test]
    fn a_chain_of_bounds_is_followed() {
        // a <= b, b <= c, c < a.
        assert!(contradicts(vec![
            le(&[("a", 1), ("b", -1)], 0),
            le(&[("b", 1), ("c", -1)], 0),
            le(&[("c", 1), ("a", -1)], 1),
        ]));
    }
}
