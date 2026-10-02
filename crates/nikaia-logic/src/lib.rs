// crates/nikaia-logic/src/lib.rs
//
// **The logic layer of the prover** (ADR-265): terms, queries, answers and
// solvers, knowing nothing of Nikaia. The compiler's frontend (`prove.rs`)
// walks a program and asks questions here; another language or tool may ask
// the same questions in the same form.
//
// * A **query** is self-contained (D3): its facts and its goal, as terms in an
//   arena it reads and does not change. There is no assertion stack, and no
//   state is left behind for the next query.
// * An **answer** depends on the query and a budget counted in units of work,
//   never on time or on how many threads ran it (D4).
// * The first solver is the **reference**: Fourier-Motzkin elimination with
//   integer tightening, over linear integer arithmetic.

mod lia;
mod term;

pub use lia::FourierMotzkin;
pub use term::{Arena, Term, TermId};

/// One question: do the facts rule out every way the goal could be false?
#[derive(Debug, Clone, Copy)]
pub struct Query<'a> {
    pub arena: &'a Arena,
    pub facts: &'a [TermId],
    pub goal: TermId,
}

/// What a solver answers (ADR-265 D3). A *refuted* answer, with the values
/// that make the goal false, comes with ADR-264 D8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The facts imply the goal.
    Proved,
    /// The solver could not tell, and why.
    Unknown(Unknown),
}

/// Why a solver could not tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unknown {
    /// The query is outside the solver's theory: a product of two variables,
    /// a sort it does not read.
    OutsideTheTheory,
    /// A coefficient left the range the solver computes in.
    Overflow,
    /// The query split into more cases than the budget allows.
    TooManyCases,
    /// An elimination held more bounds than the budget allows.
    TooManyBounds,
    /// The solver searched within its budget and found no contradiction; the
    /// goal may hold, but this solver does not show it.
    NoContradiction,
}

/// How much work a solver may do on one query, counted in its own units
/// (ADR-265 D4): never seconds, so that an answer is the same on any machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// How many cases the query may split into.
    pub cases: usize,
    /// How many bounds one elimination may hold.
    pub bounds: usize,
}

impl Default for Budget {
    fn default() -> Budget {
        Budget {
            cases: 256,
            bounds: 4096,
        }
    }
}

/// A decision procedure (ADR-265 D1).
pub trait Solver {
    fn check(&self, query: &Query<'_>, budget: &Budget) -> Answer;
}
