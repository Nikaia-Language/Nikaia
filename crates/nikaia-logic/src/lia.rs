// crates/nikaia-logic/src/lia.rs
//
// **The reference solver: linear integer arithmetic** (ADR-270 D1, ADR-269
// D10). A goal is proved when every case of its negation, joined with the
// facts, has no integer solution. Fourier-Motzkin elimination over the
// rationals decides that from one side: where it finds a contradiction there
// is none over the rationals, so none over the integers; each derived bound is
// tightened to the integers on the way, which is what lets `x < 5` and `x > 4`
// contradict. It may fail to find a contradiction that exists, and never finds
// one that does not.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::{
    Answer, Arena, Budget, Certificate, Model, Query, Refutation, Solver, Step, Term, TermId,
    Unknown, verify_model,
};

/// Fourier-Motzkin elimination with integer tightening.
#[derive(Debug, Default, Clone, Copy)]
pub struct FourierMotzkin;

impl Solver for FourierMotzkin {
    fn check(&self, query: &Query<'_>, budget: &Budget) -> Answer {
        let indexed = match indexed(query) {
            Ok(indexed) => indexed,
            Err(why) => return Answer::Unknown(why),
        };
        let mut state = State::default();
        state.absorb(&indexed.top, &Rc::new(BTreeSet::new()));
        let mut work = 0;
        match search(&indexed, state, budget, &mut work) {
            Ok((certificate, _)) => Answer::Proved { certificate },
            Err(Open::Consistent(Some(values))) => refuted(query, values)
                .map_or(Answer::Unknown(Unknown::NoContradiction), |model| {
                    Answer::Refuted { model }
                }),
            Err(Open::Consistent(None)) => Answer::Unknown(Unknown::NoContradiction),
            Err(Open::Unknown(why)) => Answer::Unknown(why),
        }
    }
}

/// The disjunctions a case was chosen from: the ones whose alternative it
/// relies on.
type Choices = Rc<BTreeSet<usize>>;

/// What one branch of the search holds: the atoms that hold in it, in the
/// order they arrived, and the disjunctions not yet split - each with the
/// choices that brought it in.
#[derive(Debug, Clone, Default)]
struct State {
    held: Vec<(usize, Choices)>,
    open: Vec<(usize, Choices)>,
}

impl State {
    fn absorb(&mut self, items: &[Item], why: &Choices) {
        for item in items {
            match *item {
                Item::Atom(a) => {
                    if !self.held.iter().any(|(b, _)| *b == a) {
                        self.held.push((a, why.clone()));
                    }
                }
                Item::Or(o) => self.open.push((o, why.clone())),
            }
        }
    }
}

/// **Splitting on demand** (ADR-270 D5): refute what the branch holds; where
/// it is consistent, split the open disjunction with the fewest alternatives
/// and refute each. A case that did not rely on the alternative it was given
/// refutes the branch as it stands, so the disjunction's other alternatives
/// are not looked at - the search jumps back to the last choice it relied
/// on. The certificate is the tree that is left, and the choices it relies
/// on come with it.
fn search(
    indexed: &Indexed,
    state: State,
    budget: &Budget,
    work: &mut usize,
) -> Result<(Certificate, Choices), Open> {
    *work += 1;
    if *work > budget.cases {
        return Err(Open::Unknown(Unknown::TooManyCases));
    }
    let bounds: Vec<Lin> = state
        .held
        .iter()
        .map(|(a, _)| indexed.atoms[*a].clone())
        .collect();
    match contradictory(bounds, budget) {
        Ok(mut refutation) => {
            let mut relies: BTreeSet<usize> = BTreeSet::new();
            for step in &mut refutation.steps {
                if let Step::Hypothesis(i) = step {
                    let (atom, why) = &state.held[*i];
                    relies.extend(why.iter().copied());
                    *i = *atom;
                }
            }
            return Ok((Certificate::Refuted(refutation), Rc::new(relies)));
        }
        Err(Open::Consistent(values)) if state.open.is_empty() => {
            return Err(Open::Consistent(values));
        }
        Err(Open::Consistent(_)) => {}
        Err(open) => return Err(open),
    }
    let at = (0..state.open.len())
        .min_by_key(|i| {
            let (o, _) = state.open[*i];
            (indexed.ors[o].len(), o)
        })
        .expect("an open disjunction");
    let mut rest = state;
    let (disjunction, brought) = rest.open.remove(at);
    let mut why = (*brought).clone();
    why.insert(disjunction);
    let why = Rc::new(why);
    let mut cases = Vec::with_capacity(indexed.ors[disjunction].len());
    let mut relies: BTreeSet<usize> = brought.iter().copied().collect();
    for alternative in &indexed.ors[disjunction] {
        let mut branch = rest.clone();
        branch.absorb(alternative, &why);
        let (certificate, needs) = search(indexed, branch, budget, work)?;
        if !needs.contains(&disjunction) {
            return Ok((certificate, needs));
        }
        relies.extend(needs.iter().copied().filter(|o| *o != disjunction));
        cases.push(certificate);
    }
    Ok((Certificate::Split { disjunction, cases }, Rc::new(relies)))
}

/// A case's values made a model of the whole query: every variable the query
/// reads is given - zero where the case did not mention it - and the model is
/// checked by evaluating the query, so a fault in the back-substitution is an
/// *unknown*, never a wrong model.
fn refuted(query: &Query<'_>, mut values: BTreeMap<String, i64>) -> Option<Model> {
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
    Consistent(Option<BTreeMap<String, i64>>),
    /// The solver could not tell.
    Unknown(Unknown),
}

/// The query's facts and its goal's negation, every atom and every
/// disjunction numbered in the order they are written. The solver and the
/// checker both start here, so a certificate names the same atoms and
/// disjunctions the solver saw.
pub(crate) fn indexed(query: &Query<'_>) -> Result<Indexed, Unknown> {
    let mut all = Vec::with_capacity(query.facts.len() + 1);
    for fact in query.facts {
        all.push(formula(query.arena, *fact, true)?);
    }
    all.push(formula(query.arena, query.goal, false)?);
    let mut indexed = Indexed::default();
    indexed.top = indexed.items(&Formula::And(all));
    Ok(indexed)
}

/// A query's formula with its atoms and disjunctions numbered: what holds at
/// the top, and for each disjunction what each alternative adds.
#[derive(Debug, Default)]
pub(crate) struct Indexed {
    pub(crate) atoms: Vec<Lin>,
    pub(crate) ors: Vec<Vec<Vec<Item>>>,
    pub(crate) top: Vec<Item>,
}

/// One thing a branch holds: an atom `lin <= 0`, or a disjunction still to
/// be split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Item {
    Atom(usize),
    Or(usize),
}

impl Indexed {
    fn atom(&mut self, lin: Lin) -> Vec<Item> {
        self.atoms.push(lin);
        vec![Item::Atom(self.atoms.len() - 1)]
    }

    /// **What every alternative says**: a bound `form + c <= 0` that each
    /// alternative holds an atom of, with its own `c`, holds whichever is
    /// taken - with the weakest of them. `z = y + 1 || z = y + 2` gives
    /// `y - z + 1 <= 0` without a split. Only atoms an alternative holds
    /// itself are read, never one inside a further disjunction.
    fn hull(&self, alternatives: &[Vec<Item>]) -> Vec<Lin> {
        let strongest = |alternative: &Vec<Item>| {
            let mut by_form: BTreeMap<&BTreeMap<String, i64>, i64> = BTreeMap::new();
            for item in alternative {
                if let Item::Atom(a) = item {
                    let atom = &self.atoms[*a];
                    let c = by_form.entry(&atom.terms).or_insert(atom.constant);
                    *c = (*c).max(atom.constant);
                }
            }
            by_form
        };
        let mut each = alternatives.iter().map(strongest);
        let Some(mut common) = each.next() else {
            return Vec::new();
        };
        for other in each {
            common = common
                .into_iter()
                .filter_map(|(form, c)| Some((form, c.min(*other.get(form)?))))
                .collect();
        }
        common
            .into_iter()
            .filter(|(form, _)| !form.is_empty())
            .map(|(form, constant)| Lin {
                terms: form.clone(),
                constant,
            })
            .collect()
    }

    /// What `formula` adds to a branch. `false` is the atom `1 <= 0`; a
    /// disjunction with an alternative that adds nothing holds already, and
    /// one with a single alternative is that alternative.
    fn items(&mut self, formula: &Formula) -> Vec<Item> {
        match formula {
            Formula::True => Vec::new(),
            Formula::False => self.atom(Lin::constant(1)),
            Formula::Le(lin) => self.atom(lin.clone()),
            Formula::And(parts) => parts.iter().flat_map(|p| self.items(p)).collect(),
            Formula::Or(parts) => {
                let at = self.ors.len();
                self.ors.push(Vec::new());
                let alternatives: Vec<Vec<Item>> = parts.iter().map(|p| self.items(p)).collect();
                match alternatives.as_slice() {
                    [] => self.atom(Lin::constant(1)),
                    [one] => one.clone(),
                    _ if alternatives.iter().any(Vec::is_empty) => Vec::new(),
                    _ => {
                        let mut items = vec![Item::Or(at)];
                        for hull in self.hull(&alternatives) {
                            items.extend(self.atom(hull));
                        }
                        self.ors[at] = alternatives;
                        items
                    }
                }
            }
        }
    }
}

/// `Σ coefficient·name + constant`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Lin {
    pub(crate) terms: BTreeMap<String, i64>,
    pub(crate) constant: i64,
}

impl Lin {
    fn constant(c: i64) -> Lin {
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

    pub(crate) fn scale(&self, k: i64) -> Option<Lin> {
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

    fn add_const(&self, c: i64) -> Lin {
        let mut out = self.clone();
        out.constant = out.constant.saturating_add(c);
        out
    }

    /// Divide through by the coefficients' common factor and round the bound
    /// towards the integers: `2x + 3 <= 0` is `x + 2 <= 0` over the integers.
    pub(crate) fn tightened(mut self) -> Lin {
        let g = self.terms.values().fold(0i64, |g, k| gcd(g, k.abs()));
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

fn gcd(a: i64, b: i64) -> i64 {
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

/// A bound the elimination holds, and the step that made it.
type Held = (Lin, usize);

/// Fourier-Motzkin with integer tightening: where the bounds, all `<= 0`,
/// have no integer solution, the derivation of `c <= 0` with `c > 0` that
/// shows it (ADR-270 D5); why not where it cannot tell.
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
        // Of the bounds on one form only the strongest counts: `form + c <= 0`
        // with the largest `c`.
        bounds.sort_by(|(a, _), (b, _)| a.terms.cmp(&b.terms).then(b.constant.cmp(&a.constant)));
        bounds.dedup_by(|later, earlier| later.0.terms == earlier.0.terms);
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
fn back_substituted(rounds: &[(String, Vec<Lin>)]) -> Option<BTreeMap<String, i64>> {
    let mut values: BTreeMap<String, i64> = BTreeMap::new();
    for (name, bounds) in rounds.iter().rev() {
        let (mut low, mut high): (Option<i64>, Option<i64>) = (None, None);
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

    fn le(terms: &[(&str, i64)], constant: i64) -> Lin {
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
