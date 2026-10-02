// crates/nikaia-logic/src/alethe.rs
//
// **A proof as Alethe** (ADR-265 D5, D6): an export, so that a checker this
// crate does not trust - Carcara, cvc5's - can check what the reference
// solver claims. It is not on the prover's path; `verify` is.
//
// The proof has three parts.
//
// 1. **Clauses.** Each assertion is taken apart into clauses of comparison
//    literals with Alethe's Boolean rules: `and` and `not_or` for what holds
//    of every part, and a resolution with a tautology - `and_pos`, `and_neg`,
//    `or_pos`, `or_neg`, `not_not` - for what is nested inside a clause. A
//    negated equality is split by `la_disequality` into the two strict
//    inequalities it means.
// 2. **One refutation per choice.** Picking one literal from every clause
//    gives a conjunction, and every such conjunction contradicts itself,
//    because together the clauses do. The reference solver refutes each, and
//    its certificate becomes `la_generic` lemmas - a bound read off a literal,
//    a combination, an integer tightening, the final `c <= 0` with `c > 0` -
//    resolved into the clause `(cl ¬l₁ … ¬lₖ)`.
// 3. **A resolution tree.** The choices are folded back clause by clause:
//    the clause itself resolved with the choices of each of its literals
//    leaves the clause of what was chosen before it, until nothing is left -
//    `(cl)`. A choice that did not need the literal it picked is that clause
//    already.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::lia::{Lin, query_cases};
use crate::smtlib::{symbol, term};
use crate::{Answer, Arena, Budget, FourierMotzkin, Query, Solver, Step, Term, TermId};

/// How many choices a proof may enumerate before it is not written.
const CHOICES: usize = 256;

/// The query's proof in Alethe, to be checked against
/// [`crate::smtlib::write`]'s script of the same query: where the reference
/// solver proves it, its Boolean structure is one Alethe can take apart, and
/// it has no more than a fixed number of choices. `None` otherwise.
pub fn write(query: &Query<'_>) -> Option<String> {
    let mut arena = query.arena.clone();
    let mut proof = Proof::default();

    // 1. The assumptions, exactly as the script asserts them, and the
    //    clauses of literals they come to.
    let mut pending: Vec<Clause> = Vec::new();
    for fact in query.facts {
        let step = proof.assume(&term(&arena, *fact));
        pending.push(Clause {
            step,
            parts: vec![(*fact, true)],
        });
    }
    let step = proof.assume(&format!("(not {})", term(&arena, query.goal)));
    pending.push(Clause {
        step,
        parts: vec![(query.goal, false)],
    });
    let mut clauses: Vec<Clause> = Vec::new();
    while let Some(clause) = pending.pop() {
        match clause.parts.iter().position(|p| !is_literal(&arena, *p)) {
            None => clauses.push(clause),
            Some(at) => pending.extend(opened(&mut arena, &mut proof, clause, at)?),
        }
    }
    // An empty clause is the refutation already: its step concludes `(cl)`.
    if clauses.iter().any(|c| c.parts.is_empty()) {
        return Some(proof.text);
    }
    clauses.reverse();
    let choices = clauses
        .iter()
        .try_fold(1usize, |n, c| n.checked_mul(c.parts.len()))?;
    if choices > CHOICES {
        return None;
    }

    // 2 and 3. Each prefix of choices, from the last clause back to none.
    let mut refuted: BTreeMap<Vec<usize>, (String, Vec<String>)> = BTreeMap::new();
    for index in 0..choices {
        // The choice `index` names, the last clause fastest.
        let mut rest = index;
        let mut key = vec![0usize; clauses.len()];
        for at in (0..clauses.len()).rev() {
            let size = clauses[at].parts.len();
            key[at] = rest % size;
            rest /= size;
        }
        let chosen: Vec<(TermId, bool)> = key
            .iter()
            .zip(&clauses)
            .map(|(at, clause)| clause.parts[*at])
            .collect();
        let refutation = choice(&mut arena, &mut proof, &chosen)?;
        refuted.insert(key, refutation);
    }
    for depth in (0..clauses.len()).rev() {
        let clause = &clauses[depth];
        let mut folded: BTreeMap<Vec<usize>, (String, Vec<String>)> = BTreeMap::new();
        let prefixes: Vec<Vec<usize>> = refuted
            .keys()
            .map(|k| k[..depth].to_vec())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        for prefix in prefixes {
            let below: Vec<&(String, Vec<String>)> = (0..clause.parts.len())
                .map(|at| {
                    let mut key = prefix.clone();
                    key.push(at);
                    &refuted[&key]
                })
                .collect();
            // A choice that did not need its literal is the answer already.
            let unneeded = below
                .iter()
                .zip(&clause.parts)
                .find(|((_, lits), part)| !lits.contains(&negated(&literal(&arena, **part))));
            let result = match unneeded {
                Some(((step, lits), _)) => (step.clone(), lits.clone()),
                None => {
                    let mut lits: Vec<String> = Vec::new();
                    for ((_, below_lits), part) in below.iter().zip(&clause.parts) {
                        let pivot = negated(&literal(&arena, *part));
                        for lit in below_lits.iter() {
                            if *lit != pivot && !lits.contains(lit) {
                                lits.push(lit.clone());
                            }
                        }
                    }
                    let premises: Vec<&str> = std::iter::once(clause.step.as_str())
                        .chain(below.iter().map(|(step, _)| step.as_str()))
                        .collect();
                    let step = proof.step(&format!(
                        "(cl{}) :rule resolution :premises ({})",
                        spaced(&lits),
                        premises.join(" ")
                    ));
                    (step, lits)
                }
            };
            folded.insert(prefix, result);
        }
        refuted = folded;
    }
    let (_, last) = refuted.get(&Vec::new())?;
    last.is_empty().then_some(proof.text)
}

/// A clause and the step that proves it: each part a term and whether it
/// stands as itself or negated.
#[derive(Clone)]
struct Clause {
    step: String,
    parts: Vec<(TermId, bool)>,
}

/// Whether a part is a literal the solver reads as bounds: a comparison, or
/// an equality that holds. A negated equality is two inequalities first.
fn is_literal(arena: &Arena, (id, positive): (TermId, bool)) -> bool {
    match arena.get(id) {
        Term::Le(..) | Term::Lt(..) | Term::Ge(..) | Term::Gt(..) => true,
        Term::Eq(..) => positive,
        _ => false,
    }
}

/// A part as the clause writes it.
fn literal(arena: &Arena, (id, positive): (TermId, bool)) -> String {
    match positive {
        true => term(arena, id),
        false => format!("(not {})", term(arena, id)),
    }
}

/// The literal's negation as a clause writes it.
fn negated(literal: &str) -> String {
    match literal
        .strip_prefix("(not ")
        .and_then(|l| l.strip_suffix(')'))
    {
        Some(inner) => inner.to_string(),
        None => format!("(not {literal})"),
    }
}

fn spaced(lits: &[String]) -> String {
    lits.iter().map(|l| format!(" {l}")).collect()
}

/// The clause with its `at`th part taken apart: the clauses it comes to, each
/// with the resolution step that proves it. `None` for a part Alethe's rules
/// here do not take apart.
fn opened(arena: &mut Arena, proof: &mut Proof, clause: Clause, at: usize) -> Option<Vec<Clause>> {
    let (id, positive) = clause.parts[at];
    let rest: Vec<(TermId, bool)> = clause
        .parts
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != at)
        .map(|(_, p)| *p)
        .collect();
    let written = literal(arena, (id, positive));
    // Resolve the clause with a tautology on the part, leaving `rest` and
    // `instead`.
    let resolved = |arena: &Arena,
                    proof: &mut Proof,
                    tautology: String,
                    instead: Vec<(TermId, bool)>|
     -> Clause {
        let parts: Vec<(TermId, bool)> = rest.iter().copied().chain(instead).collect();
        let text: Vec<String> = parts.iter().map(|p| literal(arena, *p)).collect();
        let step = proof.step(&format!(
            "(cl{}) :rule resolution :premises ({} {tautology})",
            spaced(&text),
            clause.step
        ));
        Clause { step, parts }
    };
    let term_of = |arena: &Arena, id: TermId| term(arena, id);
    Some(match (arena.get(id).clone(), positive) {
        // `true`, or `not false`: the clause holds, and says nothing.
        (Term::Bool(b), p) if b == p => Vec::new(),
        (Term::Bool(_), true) => {
            let tautology = proof.step("(cl (not false)) :rule false");
            vec![resolved(arena, proof, tautology, Vec::new())]
        }
        (Term::Bool(_), false) => {
            let tautology = proof.step("(cl true) :rule true");
            vec![resolved(arena, proof, tautology, Vec::new())]
        }
        // `(not a)` standing as itself is `a` negated: the same text.
        (Term::Not(a), true) => {
            let mut parts = rest.clone();
            parts.insert(at.min(parts.len()), (a, false));
            vec![Clause {
                step: clause.step.clone(),
                parts,
            }]
        }
        (Term::Not(a), false) => {
            let tautology = proof.step(&format!(
                "(cl (not {written}) {}) :rule not_not",
                term_of(arena, a)
            ));
            vec![resolved(arena, proof, tautology, vec![(a, true)])]
        }
        (Term::And(parts), true) => {
            let mut out = Vec::new();
            for (i, part) in parts.iter().enumerate() {
                let tautology = proof.step(&format!(
                    "(cl (not {written}) {}) :rule and_pos :args ({i})",
                    term_of(arena, *part)
                ));
                out.push(resolved(arena, proof, tautology, vec![(*part, true)]));
            }
            out
        }
        (Term::And(parts), false) => {
            let negations: Vec<String> = parts
                .iter()
                .map(|p| format!("(not {})", term_of(arena, *p)))
                .collect();
            let tautology = proof.step(&format!(
                "(cl {}{}) :rule and_neg",
                term_of(arena, id),
                spaced(&negations)
            ));
            let instead = parts.iter().map(|p| (*p, false)).collect();
            vec![resolved(arena, proof, tautology, instead)]
        }
        (Term::Or(parts), true) => {
            let texts: Vec<String> = parts.iter().map(|p| term_of(arena, *p)).collect();
            let tautology = proof.step(&format!(
                "(cl (not {written}){}) :rule or_pos",
                spaced(&texts)
            ));
            let instead = parts.iter().map(|p| (*p, true)).collect();
            vec![resolved(arena, proof, tautology, instead)]
        }
        (Term::Or(parts), false) => {
            let mut out = Vec::new();
            for (i, part) in parts.iter().enumerate() {
                let tautology = proof.step(&format!(
                    "(cl {} (not {})) :rule or_neg :args ({i})",
                    term_of(arena, id),
                    term_of(arena, *part)
                ));
                out.push(resolved(arena, proof, tautology, vec![(*part, false)]));
            }
            out
        }
        // `a != b` is written `(not (= a b))`: the equality, negated.
        (Term::Ne(a, b), p) => {
            let equal = arena.eq(a, b);
            let mut parts = rest.clone();
            parts.insert(at.min(parts.len()), (equal, !p));
            vec![Clause {
                step: clause.step.clone(),
                parts,
            }]
        }
        // A negated equality is one of two strict inequalities.
        (Term::Eq(a, b), false) => {
            let (a_text, b_text) = (term_of(arena, a), term_of(arena, b));
            let total = proof.step(&format!(
                "(cl (or (= {a_text} {b_text}) (not (<= {a_text} {b_text})) (not (<= {b_text} \
                 {a_text})))) :rule la_disequality"
            ));
            let below = arena.le(a, b);
            let above = arena.le(b, a);
            let split = proof.step(&format!(
                "(cl (= {a_text} {b_text}) (not (<= {a_text} {b_text})) (not (<= {b_text} \
                 {a_text}))) :rule or :premises ({total})"
            ));
            vec![resolved(
                arena,
                proof,
                split,
                vec![(below, false), (above, false)],
            )]
        }
        _ => return None,
    })
}

/// **One choice refuted**: the reference solver's certificate for the
/// conjunction of `chosen`, as lemmas resolved into the clause of the
/// negations of the literals it used. Its step and that clause.
fn choice(
    arena: &mut Arena,
    proof: &mut Proof,
    chosen: &[(TermId, bool)],
) -> Option<(String, Vec<String>)> {
    let mut facts: Vec<TermId> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for part in chosen {
        let text = literal(arena, *part);
        if seen.contains(&text) {
            continue;
        }
        seen.push(text);
        facts.push(match part.1 {
            true => part.0,
            false => arena.not(part.0),
        });
    }
    let falsum = arena.bool(false);
    let query = Query {
        arena,
        facts: &facts,
        goal: falsum,
    };
    let Answer::Proved { certificate } = FourierMotzkin.check(&query, &Budget::default()) else {
        return None;
    };
    let one = Budget {
        cases: 1,
        ..Budget::default()
    };
    let cases = query_cases(&query, &one).ok()?;
    let ([bounds], [refutation]) = (cases.as_slice(), certificate.cases.as_slice()) else {
        return None;
    };
    // Each bound of the case: the literal it was read off, negated as the
    // lemma writes it, and the multiplier that turns one into the other.
    let mut sources: Vec<(String, i64)> = Vec::new();
    for fact in &facts {
        let (id, positive) = match arena.get(*fact) {
            Term::Not(inner) => (*inner, false),
            _ => (*fact, true),
        };
        let negation = negated(&literal(arena, (id, positive)));
        match (arena.get(id), positive) {
            (Term::Eq(..), true) => {
                sources.push((negation.clone(), -1));
                sources.push((negation, 1));
            }
            _ => sources.push((negation, 1)),
        }
    }
    if sources.len() != bounds.len() {
        return None;
    }

    let mut lemmas: Vec<String> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    let mut made: Vec<(Lin, String)> = Vec::with_capacity(refutation.steps.len());
    for step in &refutation.steps {
        let (bound, lemma) = match *step {
            Step::Hypothesis(i) => {
                let bound = bounds.get(i)?.clone();
                let (negation, weight) = sources.get(i)?;
                if !used.contains(negation) {
                    used.push(negation.clone());
                }
                let atom = atom(&bound);
                (
                    bound,
                    format!("(cl {negation} {atom}) :rule la_generic :args ({weight} 1)"),
                )
            }
            Step::Tighten(from) => {
                let (before, before_atom) = made.get(from)?.clone();
                let factor = before.terms.values().fold(0i64, |g, k| gcd(g, k.abs()));
                let bound = before.tightened();
                let atom = atom(&bound);
                (
                    bound,
                    format!("(cl (not {before_atom}) {atom}) :rule la_generic :args (1 {factor})"),
                )
            }
            Step::Combine {
                left,
                by_left,
                right,
                by_right,
            } => {
                let (l, l_atom) = made.get(left)?.clone();
                let (r, r_atom) = made.get(right)?.clone();
                let bound = l.scale(by_left)?.add(&r.scale(by_right)?)?;
                let atom = atom(&bound);
                (
                    bound,
                    format!(
                        "(cl (not {l_atom}) (not {r_atom}) {atom}) :rule la_generic :args \
                         ({by_left} {by_right} 1)"
                    ),
                )
            }
        };
        lemmas.push(proof.step(&lemma));
        let atom = atom(&bound);
        made.push((bound, atom));
    }
    let (last, last_atom) = made.last()?;
    if !last.terms.is_empty() || last.constant <= 0 {
        return None;
    }
    lemmas.push(proof.step(&format!(
        "(cl (not {last_atom})) :rule la_generic :args (1)"
    )));
    let step = proof.step(&format!(
        "(cl{}) :rule resolution :premises ({})",
        spaced(&used),
        lemmas.join(" ")
    ));
    Some((step, used))
}

#[derive(Default)]
struct Proof {
    text: String,
    count: usize,
}

impl Proof {
    fn assume(&mut self, term: &str) -> String {
        let name = format!("h{}", self.count);
        self.count += 1;
        let _ = writeln!(self.text, "(assume {name} {term})");
        name
    }

    fn step(&mut self, body: &str) -> String {
        let name = format!("t{}", self.count);
        self.count += 1;
        let _ = writeln!(self.text, "(step {name} {body})");
        name
    }
}

/// A bound `lin <= 0` as an SMT-LIB atom.
fn atom(lin: &Lin) -> String {
    let number = |n: i64| match n < 0 {
        true => format!("(- {})", n.unsigned_abs()),
        false => n.to_string(),
    };
    let mut items: Vec<String> = lin
        .terms
        .iter()
        .map(|(name, k)| match k {
            1 => symbol(name),
            _ => format!("(* {} {})", number(*k), symbol(name)),
        })
        .collect();
    if lin.constant != 0 || items.is_empty() {
        items.push(number(lin.constant));
    }
    match items.as_slice() {
        [one] => format!("(<= {one} 0)"),
        _ => format!("(<= (+ {}) 0)", items.join(" ")),
    }
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { gcd(b, a % b) }
}
