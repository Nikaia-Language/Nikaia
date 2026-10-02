// crates/nikaia-logic/src/certificate.rs
//
// **The checker** (ADR-270 D5): what has to be trusted so that no solver has
// to be. It numbers the query's atoms and disjunctions itself, follows the
// certificate's splits - every alternative of a disjunction the branch has
// not split yet - and replays each refutation step by step: a few arithmetic
// operations per step, and no search.

use std::collections::BTreeSet;

use crate::lia::{Indexed, Item, Lin, indexed};
use crate::{Certificate, Model, Query, Refutation, Step};

/// Why a certificate does not show what it claims. `case` counts the
/// refutations in the order the checker reaches them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// The query is outside what the checker reads.
    NotReadable,
    /// A split names a disjunction the branch does not have open.
    NotOpen { disjunction: usize },
    /// A split has a different number of cases than its disjunction has
    /// alternatives.
    Cases { expected: usize, found: usize },
    /// A step names an atom the branch does not hold, or a step that is not
    /// before it.
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
    let indexed = indexed(query).map_err(|_| Rejected::NotReadable)?;
    let mut branch = Branch::default();
    branch.absorb(&indexed.top);
    let mut case = 0;
    follow(&indexed, branch, certificate, &mut case)
}

/// The atoms a branch holds and the disjunctions it has not split.
#[derive(Debug, Clone, Default)]
struct Branch {
    held: BTreeSet<usize>,
    open: BTreeSet<usize>,
}

impl Branch {
    fn absorb(&mut self, items: &[Item]) {
        for item in items {
            match *item {
                Item::Atom(a) => self.held.insert(a),
                Item::Or(o) => self.open.insert(o),
            };
        }
    }
}

fn follow(
    indexed: &Indexed,
    branch: Branch,
    certificate: &Certificate,
    case: &mut usize,
) -> Result<(), Rejected> {
    match certificate {
        Certificate::Refuted(refutation) => {
            let at = *case;
            *case += 1;
            replay(indexed, &branch, refutation, at)
        }
        Certificate::Split { disjunction, cases } => {
            if !branch.open.contains(disjunction) {
                return Err(Rejected::NotOpen {
                    disjunction: *disjunction,
                });
            }
            let alternatives = &indexed.ors[*disjunction];
            if alternatives.len() != cases.len() {
                return Err(Rejected::Cases {
                    expected: alternatives.len(),
                    found: cases.len(),
                });
            }
            for (alternative, below) in alternatives.iter().zip(cases) {
                let mut next = branch.clone();
                next.open.remove(disjunction);
                next.absorb(alternative);
                follow(indexed, next, below, case)?;
            }
            Ok(())
        }
    }
}

fn replay(
    indexed: &Indexed,
    branch: &Branch,
    refutation: &Refutation,
    case: usize,
) -> Result<(), Rejected> {
    let mut made: Vec<Lin> = Vec::with_capacity(refutation.steps.len());
    for (at, step) in refutation.steps.iter().enumerate() {
        let reference = Rejected::Reference { case, step: at };
        let earlier = |i: usize| made.get(i).filter(|_| i < at).ok_or(reference.clone());
        let bound = match *step {
            Step::Hypothesis(a) if branch.held.contains(&a) => indexed.atoms[a].clone(),
            Step::Hypothesis(_) => return Err(reference),
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
        Some(last) if last.terms.is_empty() && last.constant > 0 => Ok(()),
        _ => Err(Rejected::NotAContradiction { case }),
    }
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
