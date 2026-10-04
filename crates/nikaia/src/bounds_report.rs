// crates/nikaia/src/bounds_report.rs
//
// `--bounds`: every index into a list and every `+`, `-` and `*` the
// optimizations of ADR-306 may drop the check of, and what became of each
// (#389).
//
// An author could see what `remove-bounds-checks` and `remove-overflow-checks`
// proved only by reading the lowered Rust for `proven::read` and
// `<T>::wrapping_*`. `--asserts` already answers the same question for written
// claims (ADR-269 D7); this is that report for the checks the compiler writes
// itself. It explains a decision and changes none, as `--trust` and `--tethers`
// do.
//
// **The reason a check stayed is the shape that stopped the walk, read off the
// site**, not a trace of the solver: a field, a list of lists, a product of two
// unknowns, a call's result, or none of those - a value the walk has no bound
// for, which is a counter, a loop or a length. Those are `solver-workload.md`
// §8.4's categories, each with the issue that would supply the fact. It names
// where the next fact pays off; it does not prove that the fact would be
// enough.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::ast::{BinaryOp, Block, Expr, Item, Span, Stmt};
use crate::bounds::{BoundsChecks, OverflowChecks};
use crate::check::value_node;
use crate::contracts::Ledger;
use crate::contracts::sync::{visit_stmt, visit_stmt_blocks};
use crate::parser::Parsed;

/// One site a check stands at.
struct Site {
    line: usize,
    /// The site as written, near enough (`check::written`).
    text: String,
    index: bool,
    proved: bool,
    /// Why it was not, where it was not.
    blocked: Option<&'static str>,
}

/// The report, for one file, at the levels the build asked for - or at
/// `aggressive` where it asked for none, said in the first line.
pub fn report(
    parsed: &Parsed,
    source: &str,
    path: &str,
    own: &Ledger,
    library: &Ledger,
    bounds: BoundsChecks,
    overflow: OverflowChecks,
) -> String {
    let off = bounds == BoundsChecks::Kept && overflow == OverflowChecks::Kept;
    let (bounds, overflow) = match off {
        true => (BoundsChecks::Aggressive, OverflowChecks::Aggressive),
        false => (bounds, overflow),
    };
    let checked = crate::check::check(parsed, own, library);
    let proven = crate::bounds::proven(
        parsed,
        bounds,
        overflow,
        &checked.std_lengths,
        &checked.sized_lengths,
        &checked.arithmetic,
    );
    let mut walk = Walk {
        parsed,
        source,
        lists: &checked.list_indices,
        arithmetic: &checked.arithmetic,
        proven: &proven,
        bounds,
        overflow,
        seen: HashSet::new(),
        sites: Vec::new(),
    };
    for item in &parsed.program.items {
        match &item.node {
            Item::Fn { body, .. } => walk.block(body),
            Item::Impl { methods, .. } => {
                for method in methods {
                    if let Item::Fn { body, .. } = &method.node {
                        walk.block(body);
                    }
                }
            }
            _ => {}
        }
    }
    let mut sites = walk.sites;
    sites.sort_by_key(|s| s.line);
    let indexes = sites.iter().filter(|s| s.index).count();
    let operations = sites.len() - indexes;
    let proved = |index: bool| {
        sites
            .iter()
            .filter(|s| s.index == index && s.proved)
            .count()
    };
    let mut out = format!(
        "bounds in {path}: {} indexes, {} without their check; {} operations, {} without \
         their check{}\n",
        indexes,
        proved(true),
        operations,
        proved(false),
        match off {
            true => " - at `aggressive`, which this build does not ask for",
            false => "",
        }
    );
    let width = sites
        .iter()
        .map(|s| s.text.len())
        .max()
        .unwrap_or(0)
        .min(40);
    for site in &sites {
        let how = match (site.proved, site.blocked) {
            (true, _) => "proved".to_string(),
            (false, Some(why)) => format!("checked: {why}"),
            (false, None) => "checked".to_string(),
        };
        out.push_str(&format!(
            "  {:>4}  {:<width$}  {how}\n",
            site.line,
            site.text,
            width = width
        ));
    }
    out
}

struct Walk<'a> {
    parsed: &'a Parsed,
    source: &'a str,
    lists: &'a BTreeSet<usize>,
    arithmetic: &'a BTreeMap<usize, String>,
    proven: &'a crate::bounds::Proven,
    bounds: BoundsChecks,
    overflow: OverflowChecks,
    /// Index nodes already listed: an index read in a statement is listed once.
    seen: HashSet<usize>,
    sites: Vec<Site>,
}

impl Walk<'_> {
    fn line(&self, at: usize) -> usize {
        self.source[..at.min(self.source.len())]
            .matches('\n')
            .count()
            + 1
    }

    fn block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.stmt(&stmt.node, &stmt.span);
            visit_stmt_blocks(&stmt.node, &mut |inner| self.block(inner));
        }
    }

    fn stmt(&mut self, stmt: &Stmt, span: &Span) {
        let line = self.line(span.at());
        // **A compound assignment is its operation** (ADR-306 D5a): keyed by
        // the statement's first byte, where no operator of an expression stands.
        if let Stmt::Assign {
            target,
            op: Some(op @ (BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul)),
            value,
        } = stmt
            && self.arithmetic.contains_key(&span.at())
        {
            let proved = self.overflow != OverflowChecks::Kept
                && self.proven.arithmetic.contains(&span.at());
            let text = format!(
                "{} {}= {}",
                crate::check::written(self.parsed, target),
                symbol(*op),
                crate::check::written(self.parsed, value)
            );
            self.sites.push(Site {
                line,
                text,
                index: false,
                proved,
                blocked: (!proved).then(|| blocked_operation(*op, target, value)),
            });
        }
        let mut found: Vec<Site> = Vec::new();
        visit_stmt(self.parsed, stmt, &mut |expr| match expr {
            Expr::Index { base, index } if self.lists.contains(&value_node(base)) => {
                if !self.seen.insert(value_node(base)) {
                    return;
                }
                let proved = self.bounds != BoundsChecks::Kept
                    && self.proven.indices.contains(&value_node(base));
                found.push(Site {
                    line,
                    text: crate::check::written(self.parsed, expr),
                    index: true,
                    proved,
                    blocked: (!proved).then(|| blocked_index(base, index)),
                });
            }
            Expr::Binary {
                op: op @ (BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul),
                lhs,
                rhs,
                span,
                ..
            } if self.arithmetic.contains_key(&span.at()) => {
                let proved = self.overflow != OverflowChecks::Kept
                    && self.proven.arithmetic.contains(&span.at());
                found.push(Site {
                    line: self.line(span.at()),
                    text: crate::check::written(self.parsed, expr),
                    index: false,
                    proved,
                    blocked: (!proved).then(|| blocked_operation(*op, lhs, rhs)),
                });
            }
            _ => {}
        });
        self.sites.extend(found);
    }
}

fn symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        _ => "*",
    }
}

/// The shape that kept an index's check, read off the site.
fn blocked_index(base: &Expr, index: &Expr) -> &'static str {
    if matches!(base, Expr::Index { .. }) {
        return LIST_OF_LISTS;
    }
    shape_of([base, index])
}

/// The shape that kept an operation's check, read off the site.
///
/// A side the walk has no fact for at all - a field, a list of lists, a call -
/// is named before the product: it would block the sum of the two as well.
fn blocked_operation(op: BinaryOp, lhs: &Expr, rhs: &Expr) -> &'static str {
    let shape = shape_of([lhs, rhs]);
    if shape == UNBOUNDED && matches!(op, BinaryOp::Mul) && !is_literal(lhs) && !is_literal(rhs) {
        return PRODUCT;
    }
    shape
}

fn shape_of(parts: [&Expr; 2]) -> &'static str {
    let mut field = false;
    let mut call = false;
    let mut nested = false;
    for part in parts {
        reads(part, &mut |expr| match expr {
            Expr::Field { .. } => field = true,
            Expr::Call { .. } => call = true,
            // `xs.len()` is a number of a proof, not a call's result; a call
            // with arguments, or a `?.`, is one.
            Expr::MethodCall { args, .. } if !args.is_empty() => call = true,
            Expr::SafeMethod { .. } => call = true,
            Expr::Index { base, .. } if matches!(**base, Expr::Index { .. }) => nested = true,
            _ => {}
        });
    }
    match (field, nested, call) {
        (true, _, _) => FIELD,
        (_, true, _) => LIST_OF_LISTS,
        (_, _, true) => CALL,
        _ => UNBOUNDED,
    }
}

/// Every expression inside `expr`, itself included.
fn reads(expr: &Expr, f: &mut impl FnMut(&Expr)) {
    f(expr);
    match expr {
        Expr::Binary { lhs, rhs, .. } => {
            reads(lhs, f);
            reads(rhs, f);
        }
        Expr::Unary { expr, .. } => reads(expr, f),
        Expr::Index { base, index } => {
            reads(base, f);
            reads(index, f);
        }
        Expr::Field { base, .. } => reads(base, f),
        Expr::Call { args, .. } => args.iter().for_each(|a| reads(a, f)),
        Expr::MethodCall { receiver, args, .. } | Expr::SafeMethod { receiver, args, .. } => {
            reads(receiver, f);
            args.iter().for_each(|a| reads(a, f));
        }
        _ => {}
    }
}

fn is_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::LitInt { .. })
}

const FIELD: &str = "it reads a field, of which the walk keeps no facts (#384)";
const LIST_OF_LISTS: &str = "a list of lists, whose inner lengths the walk does not know (#385)";
const PRODUCT: &str = "a product of two unknowns (#386)";
const CALL: &str = "a call's result, which the walk knows nothing of (#382)";
const UNBOUNDED: &str = "no fact bounds what it reads - a counter, a loop, a length, or one list \
     as long as another (#432, #383, #388)";
