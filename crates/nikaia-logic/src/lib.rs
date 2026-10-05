// crates/nikaia-logic/src/lib.rs
//
// **The logic layer of the prover** (ADR-270): terms, queries, answers and
// solvers, knowing nothing of Nikaia. The compiler's frontend (`prove.rs`)
// walks a program and asks questions here; another language or tool may ask
// the same questions in the same form.
//
// * A **query** is self-contained (D3): its facts and its goal, as terms in an
//   arena it reads and does not change. There is no assertion stack, and no
//   state is left behind for the next query.
// * The reference solver's **answer** depends on the query and a budget
//   counted in units of work (D14). A search may be freer than that; what a
//   build uses is the committed proof file (D4).
// * The first solver is the **reference**: Fourier-Motzkin elimination with
//   integer tightening, over linear integer arithmetic, splitting a
//   disjunction only where a branch needs it.
// * A *proved* answer carries a **certificate** that [`verify`] checks without
//   trusting the solver that made it (D5).
// * A query can be written as **SMT-LIB 2**, and read back from it, so that
//   another solver can be asked the same question, and a proof as
//   **Alethe**, so that another checker can check it (D6).

pub mod alethe;
mod certificate;
mod lia;
mod normal;
pub mod smtlib;
mod term;

pub use certificate::{Rejected, verify, verify_model};
pub use lia::FourierMotzkin;
pub use normal::{Normal, certificate_of, certificate_text, model_of, model_text};
pub use term::{Arena, Term, TermId};

/// One question: do the facts rule out every way the goal could be false?
#[derive(Debug, Clone, Copy)]
pub struct Query<'a> {
    pub arena: &'a Arena,
    pub facts: &'a [TermId],
    pub goal: TermId,
}

/// What a solver answers (ADR-270 D3).
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
    /// The search looked at more branches than the budget allows.
    TooManyCases,
    /// An elimination held more bounds than the budget allows.
    TooManyBounds,
    /// The solver searched within its budget and found no contradiction; the
    /// goal may hold, but this solver does not show it.
    NoContradiction,
}

/// How much work a solver may do on one query, counted in its own units
/// (ADR-270 D14): the reference solver answers the same for the same budget
/// anywhere, which keeps its tests stable. What a build uses is the committed
/// proof file, not this answer (D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// How many branches the search may look at, each one elimination.
    pub cases: usize,
    /// How many bounds one elimination may hold.
    pub bounds: usize,
}

impl Default for Budget {
    fn default() -> Budget {
        Budget {
            cases: 1024,
            bounds: 4096,
        }
    }
}

/// A decision procedure (ADR-270 D1).
pub trait Solver {
    fn check(&self, query: &Query<'_>, budget: &Budget) -> Answer;
}

/// **Why a goal holds, in a form a small checker can follow** (ADR-270 D5).
///
/// The facts and the goal's negation are atoms `lin <= 0` and disjunctions,
/// numbered in the order the query writes them. A certificate is a tree: a
/// [`Refutation`] of the atoms a branch holds, or a split of one disjunction
/// the branch has not split yet, with a certificate for each alternative.
/// The atoms and disjunctions are not in the certificate; [`verify`] computes
/// them from the query, so a split cannot leave an alternative out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Certificate {
    Refuted(Refutation),
    Split {
        disjunction: usize,
        cases: Vec<Certificate>,
    },
}

/// One branch's contradiction: steps, each making a bound `lin <= 0` from the
/// atoms the branch holds or from earlier steps. The last step's bound has no
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
    /// The query's `n`th atom, which the branch holds.
    Hypothesis(usize),
    /// An earlier step's bound tightened to the integers.
    Tighten(usize),
    /// `by_left · left + by_right · right`, both multipliers positive.
    Combine {
        left: usize,
        by_left: i64,
        right: usize,
        by_right: i64,
    },
}

/// Values that make a query's facts true and its goal false (ADR-270 D3).
/// [`verify_model`] checks one by evaluating the query, so it need not be
/// trusted either. Among the models a solver could give it gives the same one
/// on every run: the first branch of the search that has one, and in it each variable as near
/// to zero as its bounds allow (D3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub values: std::collections::BTreeMap<String, i64>,
}
