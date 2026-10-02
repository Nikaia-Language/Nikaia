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

use crate::{Answer, Arena, Budget, Query, Solver, Term, TermId, Unknown};

/// Fourier-Motzkin elimination with integer tightening.
#[derive(Debug, Default, Clone, Copy)]
pub struct FourierMotzkin;

impl Solver for FourierMotzkin {
    fn check(&self, query: &Query<'_>, budget: &Budget) -> Answer {
        let mut all = Vec::with_capacity(query.facts.len() + 1);
        for fact in query.facts {
            match formula(query.arena, *fact, true) {
                Ok(f) => all.push(f),
                Err(why) => return Answer::Unknown(why),
            }
        }
        match formula(query.arena, query.goal, false) {
            Ok(f) => all.push(f),
            Err(why) => return Answer::Unknown(why),
        }
        let Some(cases) = cases(&Formula::And(all), budget) else {
            return Answer::Unknown(Unknown::TooManyCases);
        };
        for case in cases {
            if let Err(why) = contradictory(case, budget) {
                return Answer::Unknown(why);
            }
        }
        Answer::Proved
    }
}

/// `Σ coefficient·name + constant`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lin {
    terms: BTreeMap<String, i128>,
    constant: i128,
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

    fn add(&self, other: &Lin) -> Option<Lin> {
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

    fn scale(&self, k: i128) -> Option<Lin> {
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
    fn tightened(mut self) -> Lin {
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

/// Fourier-Motzkin with integer tightening: `Ok` where the bounds, all
/// `<= 0`, have no integer solution, and why not where it cannot tell.
fn contradictory(bounds: Vec<Lin>, budget: &Budget) -> Result<(), Unknown> {
    let mut bounds: Vec<Lin> = bounds.into_iter().map(Lin::tightened).collect();
    loop {
        // A bound with no variables left is a verdict: `c <= 0`.
        if bounds.iter().any(|b| b.terms.is_empty() && b.constant > 0) {
            return Ok(());
        }
        bounds.retain(|b| !b.terms.is_empty());
        bounds.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        bounds.dedup();
        let names: BTreeSet<String> = bounds
            .iter()
            .flat_map(|b| b.terms.keys().cloned())
            .collect();
        // Eliminate the name whose elimination makes the fewest new bounds.
        let Some(name) = names.into_iter().min_by_key(|name| {
            let up = bounds
                .iter()
                .filter(|b| b.terms.get(name).is_some_and(|k| *k > 0))
                .count();
            let down = bounds
                .iter()
                .filter(|b| b.terms.get(name).is_some_and(|k| *k < 0))
                .count();
            up * down
        }) else {
            return Err(Unknown::NoContradiction);
        };
        let (with, without): (Vec<Lin>, Vec<Lin>) = bounds
            .into_iter()
            .partition(|b| b.terms.contains_key(&name));
        let (up, down): (Vec<Lin>, Vec<Lin>) = with.into_iter().partition(|b| b.terms[&name] > 0);
        let mut next = without;
        for u in &up {
            for d in &down {
                let a = u.terms[&name];
                let b = -d.terms[&name];
                // b·u + a·d cancels `name`; both are `<= 0`, so is the sum.
                let (Some(left), Some(right)) = (u.scale(b), d.scale(a)) else {
                    return Err(Unknown::Overflow);
                };
                let Some(sum) = left.add(&right) else {
                    return Err(Unknown::Overflow);
                };
                next.push(sum.tightened());
                if next.len() > budget.bounds {
                    return Err(Unknown::TooManyBounds);
                }
            }
        }
        bounds = next;
    }
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
