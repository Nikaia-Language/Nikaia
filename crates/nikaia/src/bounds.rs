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
use nikaia_std::tools::bounds_walk::{self as walk, BoundsContext, Ensured};

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
    /// Of the `nonnegative` asked, the values proved `>= 0` where they stand
    /// (ADR-285 D32), by node.
    pub nonnegative: HashSet<usize>,
}

/// **What a pattern's bindings matched** (ADR-314 D1), through sequences and
/// groups: `x:dec[T](digit{m,n})` (or one `digit`) is a whole number in
/// `0..=10^n - 1`, `c:digit` a `char` whose code is `48..=57` (`hex_digit`
/// `48..=102`, a one-character literal its own code). A name the grammar
/// declares as a rule of its own is not a built-in, and says nothing.
fn matched(
    parsed: &Parsed,
    rules: &BTreeSet<&str>,
    pattern: &crate::ast::Pattern,
    given: &mut BTreeMap<String, nikaia_std::tools::bounds_interval::Range>,
    classes: &mut BTreeMap<String, nikaia_std::tools::bounds_interval::Range>,
) {
    use crate::ast::{Pattern, Repeat};
    use nikaia_std::tools::bounds_interval::Range;
    let range = |lo: i64, hi: i64| Range {
        lo: Some(lo),
        hi: Some(hi),
    };
    let class = |pattern: &Pattern| -> Option<Range> {
        match pattern {
            Pattern::Ref {
                name,
                generics,
                args,
            } if generics.is_empty() && args.is_empty() && !rules.contains(parsed.text(*name)) => {
                match parsed.text(*name) {
                    "digit" => Some(range(48, 57)),
                    "hex_digit" => Some(range(48, 102)),
                    _ => None,
                }
            }
            Pattern::Literal(text) if text.chars().count() == 1 => {
                let code = text.chars().next()? as i64;
                Some(range(code, code))
            }
            _ => None,
        }
    };
    match pattern {
        Pattern::Seq(parts) => {
            for part in parts {
                matched(parsed, rules, &part.node, given, classes);
            }
        }
        Pattern::Group(inner) => matched(parsed, rules, &inner.node, given, classes),
        Pattern::Bind { name, pat } => {
            let name = parsed.text(*name).to_string();
            if let Some(code) = class(&pat.node) {
                classes.insert(name, code);
                return;
            }
            let Pattern::Ref {
                name: called,
                generics,
                args,
            } = &pat.node
            else {
                return;
            };
            if parsed.text(*called) != "dec" || rules.contains("dec") || generics.len() != 1 {
                return;
            }
            let digits = match args.as_slice() {
                [only] => match &only.node {
                    Pattern::Repeat {
                        pat,
                        rep: Repeat::Exactly(n) | Repeat::Between(_, n),
                    } if class(&pat.node).is_some_and(|c| c.lo == Some(48) && c.hi == Some(57)) => {
                        *n
                    }
                    other if class(other).is_some_and(|c| c.lo == Some(48) && c.hi == Some(57)) => {
                        1
                    }
                    _ => return,
                },
                _ => return,
            };
            if digits > 18 {
                return;
            }
            given.insert(name, range(0, 10i64.pow(digits) - 1));
        }
        _ => {}
    }
}

/// **What each callee the ledgers describe ensures**
/// ([ADR-314](../../docs/specification/adr/adr-314.md) D3), read back over its
/// parameters and `result`, by the name a call writes. The first ledger that
/// has an entry answers; a condition that does not read back is left out,
/// which only ever proves less.
fn ensured(ledgers: &[&crate::contracts::Ledger]) -> BTreeMap<String, Ensured> {
    let mut out = BTreeMap::new();
    for ledger in ledgers {
        for (key, contract) in &ledger.functions {
            if contract.ensures.is_empty() || out.contains_key(key) {
                continue;
            }
            let Some(signature) = &contract.signature else {
                continue;
            };
            let params: Vec<String> = signature.params.iter().map(|(n, _)| n.clone()).collect();
            let chars = signature
                .params
                .iter()
                .map(|(_, ty)| matches!(ty, crate::contracts::ty::Ty::Named { name, .. } if name == "char"))
                .collect();
            let mut names: BTreeSet<String> = params.iter().cloned().collect();
            names.insert("result".to_string());
            let conditions: Vec<_> = contract
                .ensures
                .iter()
                .map(|text| crate::prove::condition_nodes(text, &names))
                .filter(|nodes| !nodes.is_empty())
                .collect();
            if conditions.is_empty() {
                continue;
            }
            out.insert(
                key.clone(),
                Ensured {
                    params,
                    chars,
                    conditions,
                },
            );
        }
    }
    out
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
    char_codes: &BTreeSet<usize>,
    ledgers: &[&crate::contracts::Ledger],
    arithmetic: &BTreeMap<usize, String>,
    nonnegative: &BTreeSet<usize>,
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
        char_codes: char_codes.iter().map(|n| *n as i64).collect(),
        ensured: ensured(ledgers),
        arithmetic: arithmetic
            .iter()
            .map(|(at, ty)| (*at as i64, ty.clone()))
            .collect(),
        nonnegative: nonnegative.iter().map(|n| *n as i64).collect(),
        given: BTreeMap::new(),
        classes: BTreeMap::new(),
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
        let Item::Fn {
            name, args, body, ..
        } = &item.node
        else {
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
        let function = name.map(|n| parsed.text(n).to_string()).unwrap_or_default();
        let walked = crate::proofs::in_function(&function, item.span.start as usize, || {
            walk::bounds_aggressive(
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
            )
        });
        out.indices
            .extend(walked.indices.into_iter().map(|n| n as usize));
        out.arithmetic
            .extend(walked.arithmetic.into_iter().map(|n| n as usize));
        out.nonnegative
            .extend(walked.nonnegative.into_iter().map(|n| n as usize));
    }
    // **A grammar's actions, walked like a function's body**
    // ([ADR-314](../../docs/specification/adr/adr-314.md) D1), from what each
    // binding matched.
    for item in &parsed.program.items {
        let Item::Grammar(grammar) = &item.node else {
            continue;
        };
        let rules: BTreeSet<&str> = grammar.rules.iter().map(|r| parsed.text(r.name)).collect();
        for rule in &grammar.rules {
            for alt in &rule.alts {
                let Some(action) = &alt.action else {
                    continue;
                };
                let mut given = BTreeMap::new();
                let mut classes = BTreeMap::new();
                matched(parsed, &rules, &alt.pattern.node, &mut given, &mut classes);
                let context = BoundsContext {
                    given,
                    classes,
                    ..context.clone()
                };
                let copy = std::cell::RefCell::new(SolverCopy::default());
                let walked = crate::proofs::in_function(
                    parsed.text(rule.name),
                    rule.span.start as usize,
                    || {
                        walk::bounds_aggressive(
                            &Vec::new(),
                            action,
                            crate::emit::nonnegative_names(action)
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
                        )
                    },
                );
                out.indices
                    .extend(walked.indices.into_iter().map(|n| n as usize));
                out.arithmetic
                    .extend(walked.arithmetic.into_iter().map(|n| n as usize));
                out.nonnegative
                    .extend(walked.nonnegative.into_iter().map(|n| n as usize));
            }
        }
    }
    out
}
