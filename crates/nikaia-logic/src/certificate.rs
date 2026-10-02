// crates/nikaia-logic/src/certificate.rs
//
// **The checker** (ADR-265 D5): what has to be trusted so that no solver has
// to be. It computes the query's cases itself and replays each refutation
// step by step - a few arithmetic operations per step, and no search.

use crate::lia::{Lin, query_cases};
use crate::{Budget, Certificate, Model, Query, Step, Unknown};

/// Why a certificate does not show what it claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// The query is outside what the checker reads.
    NotReadable,
    /// The query splits into more cases than the certificate refutes.
    TooFewCases { found: usize },
    /// The certificate has a refutation for a different number of cases than
    /// the query splits into.
    Cases { expected: usize, found: usize },
    /// A step names a bound that is not there, or a step that is not before it.
    Reference { case: usize, step: usize },
    /// A combination's multiplier is not positive.
    Multiplier { case: usize, step: usize },
    /// A coefficient left the range the checker computes in.
    Overflow { case: usize, step: usize },
    /// The last step's bound is not a contradiction.
    NotAContradiction { case: usize },
}

/// Whether `certificate` shows that the query's facts imply its goal.
pub fn verify(query: &Query<'_>, certificate: &Certificate) -> Result<(), Rejected> {
    // The cases are the checker's own, under a budget of exactly as many as
    // the certificate refutes: one more and the certificate missed it.
    let budget = Budget {
        cases: certificate.cases.len(),
        ..Budget::default()
    };
    let cases = query_cases(query, &budget).map_err(|why| match why {
        Unknown::TooManyCases => Rejected::TooFewCases {
            found: certificate.cases.len(),
        },
        _ => Rejected::NotReadable,
    })?;
    if cases.len() != certificate.cases.len() {
        return Err(Rejected::Cases {
            expected: cases.len(),
            found: certificate.cases.len(),
        });
    }
    for (case, (bounds, refutation)) in cases.iter().zip(&certificate.cases).enumerate() {
        let mut made: Vec<Lin> = Vec::with_capacity(refutation.steps.len());
        for (at, step) in refutation.steps.iter().enumerate() {
            let reference = Rejected::Reference { case, step: at };
            let earlier = |i: usize| made.get(i).filter(|_| i < at).ok_or(reference.clone());
            let bound = match *step {
                Step::Hypothesis(i) => bounds.get(i).cloned().ok_or(reference.clone())?,
                Step::Tighten(from) => earlier(from)?.clone().tightened(),
                Step::Combine {
                    left,
                    by_left,
                    right,
                    by_right,
                } => {
                    if by_left <= 0 || by_right <= 0 {
                        return Err(Rejected::Multiplier { case, step: at });
                    }
                    let overflow = Rejected::Overflow { case, step: at };
                    let l = earlier(left)?.scale(by_left).ok_or(overflow.clone())?;
                    let r = earlier(right)?.scale(by_right).ok_or(overflow.clone())?;
                    l.add(&r).ok_or(overflow)?
                }
            };
            made.push(bound);
        }
        match made.last() {
            Some(last) if last.terms.is_empty() && last.constant > 0 => {}
            _ => return Err(Rejected::NotAContradiction { case }),
        }
    }
    Ok(())
}

/// Whether `model` makes every fact of the query true and its goal false.
pub fn verify_model(query: &Query<'_>, model: &Model) -> bool {
    let arena = query.arena;
    query
        .facts
        .iter()
        .all(|f| arena.bool_value(*f, &model.values) == Some(true))
        && arena.bool_value(query.goal, &model.values) == Some(false)
}
