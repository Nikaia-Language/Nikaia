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
// * A *proved* answer carries a **certificate** that [`verify`] checks without
//   trusting the solver that made it (D5).
// * A query can be written as **SMT-LIB 2**, and read back from it, so that
//   another solver can be asked the same question (D6).

mod certificate;
mod lia;
pub mod smtlib;
mod term;

pub use certificate::{Rejected, verify, verify_model};
pub use lia::FourierMotzkin;
pub use term::{Arena, Term, TermId};

/// One question: do the facts rule out every way the goal could be false?
#[derive(Debug, Clone, Copy)]
pub struct Query<'a> {
    pub arena: &'a Arena,
    pub facts: &'a [TermId],
    pub goal: TermId,
}

/// What a solver answers (ADR-265 D3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The facts imply the goal, and the certificate shows why.
    Proved { certificate: Certificate },
    /// The facts do not imply the goal: here are values for every variable of
    /// the query under which every fact holds and the goal does not.
    Refuted { model: Model },
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

/// **Why a goal holds, in a form a small checker can follow** (ADR-265 D5).
///
/// The facts and the goal's negation split into cases - every way the goal
/// could be false - and each case gets a [`Refutation`]: a derivation of a
/// bound `c <= 0` with `c > 0` from the case's own bounds. The cases are not
/// in the certificate; [`verify`] computes them from the query, so a
/// certificate cannot leave one out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    pub cases: Vec<Refutation>,
}

/// One case's contradiction: steps, each making a bound `lin <= 0` from the
/// case's bounds or from earlier steps. The last step's bound has no
/// variables and a positive constant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refutation {
    pub steps: Vec<Step>,
}

/// One step of a [`Refutation`]. Each is sound over the integers: a
/// non-negative combination of bounds `<= 0` is one, and so is a bound divided
/// by its coefficients' common factor with the constant rounded up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The case's `n`th bound.
    Hypothesis(usize),
    /// An earlier step's bound tightened to the integers.
    Tighten(usize),
    /// `by_left · left + by_right · right`, both multipliers positive.
    Combine {
        left: usize,
        by_left: i128,
        right: usize,
        by_right: i128,
    },
}

/// Values that make a query's facts true and its goal false (ADR-265 D3).
/// [`verify_model`] checks one by evaluating the query, so it need not be
/// trusted either. Among the models a solver could give it gives the same one
/// on every run: the first case that has one, and in it each variable as near
/// to zero as its bounds allow (D4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub values: std::collections::BTreeMap<String, i128>,
}
