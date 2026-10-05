//! **Which index checks a proof shows are not needed**
//! ([ADR-306](../../docs/specification/adr/adr-306.md)).
//!
//! An index into a list stops the program where it is outside (Part III A.2).
//! `--optimization=remove-bounds-checks:<level>` drops that check where it is
//! **proved** that the position is inside, every time the index is reached -
//! never on a guess, so a program that passes the option means what it meant
//! without it (D2). Two levels:
//!
//! * **`basic`** (D3) reads one shape and asks no solver: `xs[k]` where `k` is
//!   the binding of an enclosing `for k in lo..<xs.len()`, `lo` is not
//!   negative, and nothing in the loop's body can change `xs`'s length.
//! * **`aggressive`** (D4) walks each body forward, keeping the linear facts
//!   that hold at each point - ranges, branch and loop conditions, the
//!   operands of `&&` and `||`, `let`s and assignments, the lengths a list is
//!   built or resized to - and asks the solver whether `0 <= i < xs.len()`
//!   follows. A check is dropped only where the solver's certificate passes
//!   the checker ([ADR-270](../../docs/specification/adr/adr-270.md) D5).
//!
//! What either finds is a set of index nodes, by the address of the indexed
//! expression ([`crate::check::value_node`] of the `base`), which the emitter
//! reads where it writes an index of a list.
//!
//! **`--optimization=remove-overflow-checks:aggressive`**
//! ([ADR-306](../../docs/specification/adr/adr-306.md) D6) asks the same walk
//! whether a `+`, `-` or `*` stays inside its type, and the walk knows more
//! than ADR-306 gave it: a bound on **every value a list holds**, the join of
//! what each write puts in, proved where it is written (D2); `x % n` (D3); and
//! the length a loop that pushes once per turn leaves (D4).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::ast::{Expr, Item, Spanned};
use crate::check::value_node;
use crate::parser::Parsed;
use crate::prove::{SolverCopy, asked_of};
use nikaia_std::tools::bounds_basic as nika;
use nikaia_std::tools::bounds_shape::Around;
use nikaia_std::tools::bounds_walk::{self as walk, BoundsContext};

/// What happens to an index check the walk proved (ADR-306 D14): `on` writes
/// it without its check, `off` (the default) keeps it. What is proved does not
/// depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
pub enum BoundsChecks {
    /// Every index is checked: the default.
    #[default]
    Kept,
    /// An index the walk proved inside is written without its check.
    Removed,
}

impl BoundsChecks {
    /// The word after `remove-bounds-checks:`.
    pub fn parse(word: &str) -> Option<BoundsChecks> {
        match word {
            "off" => Some(BoundsChecks::Kept),
            "on" => Some(BoundsChecks::Removed),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BoundsChecks::Kept => "off",
            BoundsChecks::Removed => "on",
        }
    }
}

/// What happens to an overflow check the walk proved (ADR-306 D14), as for
/// [`BoundsChecks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
pub enum OverflowChecks {
    /// Every `+`, `-` and `*` is checked: the default.
    #[default]
    Kept,
    /// One the walk proved stays inside its type is written without its check.
    Removed,
}

impl OverflowChecks {
    /// The word after `remove-overflow-checks:`.
    pub fn parse(word: &str) -> Option<OverflowChecks> {
        match word {
            "off" => Some(OverflowChecks::Kept),
            "on" => Some(OverflowChecks::Removed),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            OverflowChecks::Kept => "off",
            OverflowChecks::Removed => "on",
        }
    }
}

/// What the walk proved: index nodes ([`crate::check::value_node`] of the
/// indexed `base`) and arithmetic operators (by the byte their operator
/// starts at).
#[derive(Debug, Default)]
pub struct Proven {
    pub indices: HashSet<usize>,
    pub arithmetic: HashSet<usize>,
}

/// **Every index and every operation the walk proves**, whatever a build does
/// with them (ADR-306 D14): the shape D3 reads without a solver and every
/// linear fact D4 decides with one. What is written without its check is the
/// emitter's choice, by `remove-bounds-checks` and `remove-overflow-checks`.
///
/// `lengths` are the receivers of the `x.len()` calls that count a `std`
/// list, text or map ([`crate::check::Checked::std_lengths`]): only those are
/// numbers of a proof, because a `len()` a program writes for its own type
/// may say anything. `arithmetic` is every `+`, `-` and `*` whose two sides
/// are one whole-number type, with that type
/// ([`crate::check::Checked::arithmetic`]): only those may be proved.
pub fn proven(
    parsed: &Parsed,
    lengths: &BTreeSet<usize>,
    sized: &BTreeSet<usize>,
    arithmetic: &BTreeMap<usize, String>,
) -> Proven {
    let mut out = Proven::default();
    let mut functions: BTreeMap<String, Vec<bool>> = BTreeMap::new();
    // A method of this program that takes a parameter `mut` changes what it
    // is handed, whichever type it is called on.
    let mut changing_methods: BTreeSet<String> = BTreeSet::new();
    for item in &parsed.program.items {
        if let Item::Impl { methods, .. } = &item.node {
            for method in methods {
                if let Item::Fn {
                    name: Some(name),
                    args,
                    ..
                } = &method.node
                    && args.iter().any(|a| a.mutable)
                {
                    changing_methods.insert(parsed.text(*name).to_string());
                }
            }
        }
    }
    for item in &parsed.program.items {
        if let Item::Fn {
            name: Some(name),
            receiver: None,
            args,
            ..
        } = &item.node
        {
            functions.insert(
                parsed.text(*name).to_string(),
                args.iter().map(|a| a.mutable).collect(),
            );
        }
    }
    let around = Around {
        functions,
        changing_methods,
    };
    let context = BoundsContext {
        lengths: lengths.iter().map(|n| *n as i64).collect(),
        sized: sized.iter().map(|n| *n as i64).collect(),
        arithmetic: arithmetic
            .iter()
            .map(|(at, ty)| (*at as i64, ty.clone()))
            .collect(),
        around,
    };
    let mut bodies: Vec<&Spanned<Item>> = Vec::new();
    for item in &parsed.program.items {
        match &item.node {
            Item::Fn { .. } => bodies.push(item),
            Item::Impl { methods, .. } => bodies.extend(methods.iter()),
            _ => {}
        }
    }
    let node_of = |e: &Expr| value_node(e) as i64;
    for item in bodies {
        let Item::Fn { args, body, .. } = &item.node else {
            continue;
        };
        out.indices.extend(
            nika::basic_indices(body, &parsed.interner, &node_of)
                .into_iter()
                .map(|n| n as usize),
        );
        // **The aggressive walk** (`tools/bounds_walk.nika`), with the
        // solver behind a copy of its arena that grows with it.
        let copy = std::cell::RefCell::new(SolverCopy::default());
        let walked = walk::bounds_aggressive(
            args,
            body,
            crate::emit::nonnegative_names(body)
                .into_iter()
                .map(|s| parsed.text(s).to_string())
                .collect(),
            &context,
            &parsed.program,
            &parsed.interner,
            &node_of,
            &|arena, facts, goal| {
                asked_of(&copy, arena, facts, goal, crate::proofs::ask)
                    == crate::proofs::Asked::Proved
            },
        );
        out.indices
            .extend(walked.indices.into_iter().map(|n| n as usize));
        out.arithmetic
            .extend(walked.arithmetic.into_iter().map(|n| n as usize));
    }
    out
}
