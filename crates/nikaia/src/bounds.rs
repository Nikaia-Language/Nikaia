//! **Which index checks a proof shows are not needed**
//! ([ADR-271](../../docs/specification/adr/adr-271.md)).
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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use nikaia_logic::{Answer, Arena, Budget, FourierMotzkin, Query, Solver, TermId, verify};

use crate::ast::{BinaryOp, Block, Expr, FPart, Item, Span, Spanned, Stmt, UnaryOp};
use crate::check::value_node;
use crate::parser::Parsed;

/// How hard the compiler works to drop an index check (ADR-271 D1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
pub enum BoundsChecks {
    /// Every index is checked: the default.
    #[default]
    Kept,
    /// The loop over a list's own length (D3).
    Basic,
    /// Every linear fact the walk keeps, decided by the solver (D4).
    Aggressive,
}

impl BoundsChecks {
    /// The word after `remove-bounds-checks:`.
    pub fn parse(word: &str) -> Option<BoundsChecks> {
        match word {
            "off" => Some(BoundsChecks::Kept),
            "basic" => Some(BoundsChecks::Basic),
            "aggressive" => Some(BoundsChecks::Aggressive),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BoundsChecks::Kept => "off",
            BoundsChecks::Basic => "basic",
            BoundsChecks::Aggressive => "aggressive",
        }
    }
}

/// The index nodes whose check is proved unnecessary at `level`.
///
/// `lengths` are the receivers of the `x.len()` calls that count a `std`
/// list, text or map ([`crate::check::Checked::std_lengths`]): only those are
/// numbers of a proof, because a `len()` a program writes for its own type
/// may say anything.
pub fn proven(parsed: &Parsed, level: BoundsChecks, lengths: &BTreeSet<usize>) -> HashSet<usize> {
    let mut out = HashSet::new();
    if level == BoundsChecks::Kept {
        return out;
    }
    let mut functions: HashMap<String, Vec<bool>> = HashMap::new();
    // A method of this program that takes a parameter `mut` changes what it
    // is handed, whichever type it is called on.
    let mut changing_methods: HashSet<String> = HashSet::new();
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
    let mut bodies: Vec<&Spanned<Item>> = Vec::new();
    for item in &parsed.program.items {
        match &item.node {
            Item::Fn { .. } => bodies.push(item),
            Item::Impl { methods, .. } => bodies.extend(methods.iter()),
            _ => {}
        }
    }
    for item in bodies {
        let Item::Fn { args, body, .. } = &item.node else {
            continue;
        };
        basic_block(parsed, body, &mut Vec::new(), &mut out);
        if level == BoundsChecks::Aggressive {
            let mut walk = Walk {
                parsed,
                lengths,
                functions: &functions,
                changing_methods: &changing_methods,
                arena: Arena::new(),
                proven: &mut out,
                pinned: pinned_in(parsed, body),
                nonnegative: HashSet::new(),
            };
            let mut facts = Facts::default();
            for arg in args {
                let name = parsed.text(arg.name).to_string();
                let ty = parsed.text(arg.ty.name);
                if arg.ty.generics.is_empty() && is_whole_number(ty) {
                    facts.ints.insert(name.clone());
                    if ty.starts_with('u') {
                        walk.at_least_zero(&mut facts, &name);
                    }
                }
                if is_a_list(ty) {
                    let length = length_of(&name);
                    walk.at_least_zero(&mut facts, &length);
                }
            }
            walk.nonnegative = crate::emit::nonnegative_names(body)
                .into_iter()
                .map(|s| parsed.text(s).to_string())
                .collect();
            walk.block(body, &mut facts);
        }
    }
    out
}

/// The variable of the proof that stands for `name.len()`.
fn length_of(name: &str) -> String {
    format!("{name}.len()")
}

fn is_whole_number(ty: &str) -> bool {
    matches!(
        ty,
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize" | "isize"
    )
}

fn is_a_list(ty: &str) -> bool {
    matches!(ty, "Vec" | "List" | "Array")
}

/// **Methods that leave a list's length as it was.** A method not on this list
/// and not on [`changes_length`]'s is taken to change it, so a method the list
/// does not know costs a proof and never makes a wrong one.
fn keeps_length(method: &str) -> bool {
    matches!(
        method,
        "len"
            | "is_empty"
            | "contains"
            | "iter"
            | "first"
            | "last"
            | "get"
            | "clone"
            | "to_vec"
            | "index_of"
            | "starts_with"
            | "ends_with"
            | "join"
            | "sum"
            | "min"
            | "max"
            | "sort"
            | "sort_by"
            | "sort_by_key"
            | "reverse"
            | "swap"
            | "fill"
            | "map"
            | "filter"
            | "any"
            | "all"
            | "count"
            | "find"
            | "position"
            | "enumerate"
            | "zip"
            | "windows"
            | "chunks"
            | "rev"
            | "fold"
            | "binary_search"
            | "to_string"
    )
}

// --- basic (D3) ---------------------------------------------------------------

/// A loop `for k in lo..<xs.len()` whose body leaves `xs`'s length alone.
struct Over {
    binding: String,
    list: String,
}

fn basic_block(parsed: &Parsed, block: &Block, over: &mut Vec<Over>, out: &mut HashSet<usize>) {
    for stmt in &block.stmts {
        basic_stmt(parsed, &stmt.node, over, out);
    }
}

fn basic_stmt(parsed: &Parsed, stmt: &Stmt, over: &mut Vec<Over>, out: &mut HashSet<usize>) {
    match stmt {
        Stmt::For {
            bindings,
            iter,
            body,
        } => {
            basic_expr(parsed, iter, over, out);
            let loop_over = match (bindings.as_slice(), iter) {
                (
                    [binding],
                    Expr::Range {
                        start,
                        end,
                        inclusive: false,
                    },
                ) => match &**end {
                    Expr::MethodCall {
                        receiver,
                        method,
                        args,
                        ..
                    } if args.is_empty() && parsed.text(*method) == "len" => match &**receiver {
                        Expr::Variable(list)
                            if not_negative_start(parsed, start, over)
                                && length_stays(parsed, body, parsed.text(*list))
                                && !rebinds(parsed, body, parsed.text(*binding)) =>
                        {
                            Some(Over {
                                binding: parsed.text(*binding).to_string(),
                                list: parsed.text(*list).to_string(),
                            })
                        }
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            };
            // A binding of this loop hides an outer loop's of the same name.
            let hidden: Vec<usize> = bindings
                .iter()
                .flat_map(|b| {
                    let b = parsed.text(*b);
                    over.iter()
                        .enumerate()
                        .filter(move |(_, o)| o.binding == b || o.list == b)
                        .map(|(at, _)| at)
                })
                .collect();
            let mut inner: Vec<Over> = over
                .iter()
                .enumerate()
                .filter(|(at, _)| !hidden.contains(at))
                .map(|(_, o)| Over {
                    binding: o.binding.clone(),
                    list: o.list.clone(),
                })
                .collect();
            inner.extend(loop_over);
            basic_block(parsed, body, &mut inner, out);
        }
        Stmt::Let { names, value, .. } => {
            basic_expr(parsed, value, over, out);
            // From here on in this block the name is another binding.
            for name in names {
                let name = parsed.text(*name);
                over.retain(|o| o.binding != name && o.list != name);
            }
        }
        Stmt::Comptime { name, value, .. } => {
            basic_expr(parsed, value, over, out);
            let name = parsed.text(*name);
            over.retain(|o| o.binding != name && o.list != name);
        }
        Stmt::Assign { target, value, .. } => {
            basic_expr(parsed, target, over, out);
            basic_expr(parsed, value, over, out);
        }
        Stmt::While { cond, body } => {
            basic_expr(parsed, cond, over, out);
            let mut inner = clone_over(over);
            basic_block(parsed, body, &mut inner, out);
        }
        Stmt::Return(Some(value)) | Stmt::Expr(value) => basic_expr(parsed, value, over, out),
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
    }
}

fn clone_over(over: &[Over]) -> Vec<Over> {
    over.iter()
        .map(|o| Over {
            binding: o.binding.clone(),
            list: o.list.clone(),
        })
        .collect()
}

fn basic_expr(parsed: &Parsed, expr: &Expr, over: &mut Vec<Over>, out: &mut HashSet<usize>) {
    if let Expr::Index { base, index } = expr
        && let (Expr::Variable(list), Expr::Variable(at)) = (&**base, &**index)
    {
        let (list, at) = (parsed.text(*list), parsed.text(*at));
        if over.iter().any(|o| o.list == list && o.binding == at) {
            out.insert(value_node(base));
        }
    }
    match expr {
        // A lambda may run after the loop, when the list is another length.
        Expr::Closure { .. } | Expr::Spawn { .. } => {}
        _ => {
            for_each_child(expr, &mut |child| basic_expr(parsed, child, over, out));
            for_each_block(expr, &mut |block| {
                let mut inner = clone_over(over);
                basic_block(parsed, block, &mut inner, out);
            });
        }
    }
}

/// A literal that is not negative, or the binding of a loop D3 already holds.
fn not_negative_start(parsed: &Parsed, start: &Expr, over: &[Over]) -> bool {
    match start {
        Expr::LitInt { negative, .. } => !negative,
        Expr::Variable(name) => over.iter().any(|o| o.binding == parsed.text(*name)),
        _ => false,
    }
}

/// **Nothing in `body` can change `list`'s length**: it is not assigned, not
/// bound again, not handed to a call, not captured by a lambda, and every
/// method called on it is one [`keeps_length`] knows.
fn length_stays(parsed: &Parsed, body: &Block, list: &str) -> bool {
    let mut stays = true;
    visit_stmts(body, &mut |stmt| match stmt {
        Stmt::Assign {
            target: Expr::Variable(name),
            ..
        } if parsed.text(*name) == list => stays = false,
        Stmt::Let { names, .. } if names.iter().any(|n| parsed.text(*n) == list) => stays = false,
        Stmt::For { bindings, .. } if bindings.iter().any(|n| parsed.text(*n) == list) => {
            stays = false
        }
        _ => {}
    });
    visit_exprs(body, &mut |expr| match expr {
        Expr::MethodCall {
            receiver, method, ..
        }
        | Expr::SafeMethod {
            receiver, method, ..
        } if matches!(&**receiver, Expr::Variable(n) if parsed.text(*n) == list)
            && !keeps_length(parsed.text(*method)) =>
        {
            stays = false
        }
        Expr::Call { args, .. } | Expr::MethodCall { args, .. } | Expr::SafeMethod { args, .. }
            if args
                .iter()
                .any(|a| matches!(a, Expr::Variable(n) if parsed.text(*n) == list)) =>
        {
            stays = false
        }
        Expr::Closure { body, .. } if mentions(parsed, body, list) => stays = false,
        Expr::Spawn { body, .. } if mentions_expr(parsed, body, list) => stays = false,
        _ => {}
    });
    stays
}

/// Whether `body` binds `name` again anywhere.
fn rebinds(parsed: &Parsed, body: &Block, name: &str) -> bool {
    let mut found = false;
    visit_stmts(body, &mut |stmt| match stmt {
        Stmt::Let { names, .. } if names.iter().any(|n| parsed.text(*n) == name) => found = true,
        Stmt::Comptime { name: n, .. } if parsed.text(*n) == name => found = true,
        Stmt::For { bindings, .. } if bindings.iter().any(|n| parsed.text(*n) == name) => {
            found = true
        }
        Stmt::Assign {
            target: Expr::Variable(n),
            ..
        } if parsed.text(*n) == name => found = true,
        _ => {}
    });
    visit_exprs(body, &mut |expr| {
        if let Expr::Closure { params, .. } = expr
            && params.iter().any(|p| parsed.text(*p) == name)
        {
            found = true;
        }
    });
    found
}

fn mentions(parsed: &Parsed, block: &Block, name: &str) -> bool {
    let mut found = false;
    visit_exprs(block, &mut |e| {
        if matches!(e, Expr::Variable(n) if parsed.text(*n) == name) {
            found = true;
        }
    });
    visit_stmts(block, &mut |s| {
        if let Stmt::Assign {
            target: Expr::Variable(n),
            ..
        } = s
            && parsed.text(*n) == name
        {
            found = true;
        }
    });
    found
}

fn mentions_expr(parsed: &Parsed, expr: &Expr, name: &str) -> bool {
    let block = Block {
        stmts: vec![Spanned::new(Stmt::Expr(expr.clone()), Span::nowhere())],
    };
    mentions(parsed, &block, name)
}

// --- aggressive (D4) ------------------------------------------------------------

/// What holds at one point of a body.
#[derive(Clone, Default)]
struct Facts {
    /// Names that are whole numbers here.
    ints: BTreeSet<String>,
    facts: Vec<TermId>,
}

/// How many facts a point keeps: the newest, so that a long body costs the
/// solver what a short one does.
const MOST_FACTS: usize = 48;

struct Walk<'a> {
    parsed: &'a Parsed,
    /// [`proven`]'s `lengths`.
    lengths: &'a BTreeSet<usize>,
    functions: &'a HashMap<String, Vec<bool>>,
    /// Methods of this program with a `mut` parameter.
    changing_methods: &'a HashSet<String>,
    arena: Arena,
    proven: &'a mut HashSet<usize>,
    /// Names a lambda, a task or an `overlap` branch changes: they may change
    /// whenever one runs, so no fact is kept about them anywhere.
    pinned: HashSet<String>,
    /// Whole numbers that are never negative anywhere in the body
    /// (`crate::emit::nonnegative_names`).
    nonnegative: HashSet<String>,
}

impl Walk<'_> {
    fn text(&self, sym: winnow_grammar::Symbol) -> &str {
        self.parsed.text(sym)
    }

    fn push(&self, facts: &mut Facts, fact: TermId) {
        let mut read = BTreeSet::new();
        self.arena.variables(fact, &mut read);
        if read.iter().any(|name| self.pinned.contains(base_of(name))) {
            return;
        }
        facts.facts.push(fact);
        if facts.facts.len() > MOST_FACTS {
            facts.facts.remove(0);
        }
    }

    fn at_least_zero(&mut self, facts: &mut Facts, name: &str) {
        let (n, zero) = (self.arena.var(name), self.arena.int(0));
        let fact = self.arena.ge(n, zero);
        self.push(facts, fact);
    }

    /// Nothing known about `name` or its length holds any more.
    fn forget(&self, facts: &mut Facts, name: &str) {
        let length = length_of(name);
        facts
            .facts
            .retain(|f| !self.arena.mentions(*f, name) && !self.arena.mentions(*f, &length));
    }

    fn forget_length(&self, facts: &mut Facts, name: &str) {
        let length = length_of(name);
        facts.facts.retain(|f| !self.arena.mentions(*f, &length));
    }

    /// Every fact `name` reads, rewritten for `name` now being `old + by`:
    /// what held of the old value holds of `name - by`.
    fn shift(&mut self, facts: &mut Facts, name: &str, by: TermId) {
        let now = self.arena.var(name);
        let old = self.arena.sub(now, by);
        let with = BTreeMap::from([(name.to_string(), old)]);
        for fact in facts.facts.iter_mut() {
            if self.arena.mentions(*fact, name) {
                *fact = self.arena.substitute(*fact, &with);
            }
        }
    }

    /// What holds again after a loop's body or a branch changed `changed`:
    /// a name that is never negative still is not.
    fn after_change(&mut self, facts: &mut Facts, changed: &Changed) {
        for name in &changed.values {
            self.forget(facts, name);
            if self.nonnegative.contains(name) && facts.ints.contains(name) {
                self.at_least_zero(facts, name);
            }
        }
        for name in &changed.lengths {
            self.forget_length(facts, name);
        }
    }

    fn lin(&mut self, expr: &Expr, facts: &Facts) -> Option<TermId> {
        match expr {
            Expr::LitInt { value, negative } => {
                let n = i64::try_from(crate::ast::int_value(*value, *negative)).ok()?;
                Some(self.arena.int(n))
            }
            Expr::Variable(name) => {
                let name = self.text(*name).to_string();
                facts.ints.contains(&name).then(|| self.arena.var(&name))
            }
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } if args.is_empty() && self.text(*method) == "len" => match &**receiver {
                Expr::Variable(name) if self.lengths.contains(&value_node(receiver)) => {
                    let length = length_of(self.text(*name));
                    Some(self.arena.var(&length))
                }
                _ => None,
            },
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
            } => {
                let a = self.lin(expr, facts)?;
                Some(self.arena.neg(a))
            }
            // **A conversion keeps the value or stops the program** (Part III
            // A.2: one that does not fit is checked at every build), so where
            // the program goes on, the number is the one converted.
            Expr::Cast { expr, ty }
                if ty.generics.is_empty() && is_whole_number(self.text(ty.name)) =>
            {
                self.lin(expr, facts)
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                let l = self.lin(lhs, facts)?;
                let r = self.lin(rhs, facts)?;
                match op {
                    BinaryOp::Add => Some(self.arena.add(l, r)),
                    BinaryOp::Sub => Some(self.arena.sub(l, r)),
                    BinaryOp::Mul
                        if self.arena.constant(l).is_some() || self.arena.constant(r).is_some() =>
                    {
                        Some(self.arena.mul(l, r))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// **What holds where `expr` is true**: a formula `expr` implies, which
    /// may say less than `expr` does - of `a && b`, what is read of either
    /// half - and never more.
    fn claim(&mut self, expr: &Expr, facts: &Facts) -> Option<TermId> {
        match expr {
            Expr::LitBool(b) => Some(self.arena.bool(*b)),
            Expr::Unary {
                op: UnaryOp::Not,
                expr,
            } => self.denial(expr, facts),
            Expr::Binary {
                op: BinaryOp::And,
                lhs,
                rhs,
                ..
            } => match (self.claim(lhs, facts), self.claim(rhs, facts)) {
                (Some(l), Some(r)) => Some(self.arena.and(vec![l, r])),
                (Some(one), None) | (None, Some(one)) => Some(one),
                (None, None) => None,
            },
            // A disjunction says something only where both halves do.
            Expr::Binary {
                op: BinaryOp::Or,
                lhs,
                rhs,
                ..
            } => {
                let l = self.claim(lhs, facts)?;
                let r = self.claim(rhs, facts)?;
                Some(self.arena.or(vec![l, r]))
            }
            _ => self.comparison(expr, facts),
        }
    }

    /// **What holds where `expr` is false**: a formula `!expr` implies, by
    /// the same rule as [`Self::claim`] - so `a || b` false says what is read
    /// of either half false, and `a && b` false says something only where
    /// both halves can be denied.
    fn denial(&mut self, expr: &Expr, facts: &Facts) -> Option<TermId> {
        match expr {
            Expr::LitBool(b) => Some(self.arena.bool(!*b)),
            Expr::Unary {
                op: UnaryOp::Not,
                expr,
            } => self.claim(expr, facts),
            Expr::Binary {
                op: BinaryOp::Or,
                lhs,
                rhs,
                ..
            } => match (self.denial(lhs, facts), self.denial(rhs, facts)) {
                (Some(l), Some(r)) => Some(self.arena.and(vec![l, r])),
                (Some(one), None) | (None, Some(one)) => Some(one),
                (None, None) => None,
            },
            Expr::Binary {
                op: BinaryOp::And,
                lhs,
                rhs,
                ..
            } => {
                let l = self.denial(lhs, facts)?;
                let r = self.denial(rhs, facts)?;
                Some(self.arena.or(vec![l, r]))
            }
            _ => {
                let atom = self.comparison(expr, facts)?;
                Some(self.arena.not(atom))
            }
        }
    }

    /// A comparison of two linear terms, exactly.
    fn comparison(&mut self, expr: &Expr, facts: &Facts) -> Option<TermId> {
        let Expr::Binary { op, lhs, rhs, .. } = expr else {
            return None;
        };
        if !matches!(
            op,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne
        ) {
            return None;
        }
        let a = self.lin(lhs, facts)?;
        let b = self.lin(rhs, facts)?;
        Some(match op {
            BinaryOp::Lt => self.arena.lt(a, b),
            BinaryOp::Le => self.arena.le(a, b),
            BinaryOp::Gt => self.arena.gt(a, b),
            BinaryOp::Ge => self.arena.ge(a, b),
            BinaryOp::Eq => self.arena.eq(a, b),
            _ => self.arena.ne(a, b),
        })
    }

    fn assume(&mut self, facts: &mut Facts, cond: &Expr) {
        if let Some(holds) = self.claim(cond, facts) {
            self.push(facts, holds);
        }
    }

    fn assume_not(&mut self, facts: &mut Facts, cond: &Expr) {
        if let Some(holds) = self.denial(cond, facts) {
            self.push(facts, holds);
        }
    }

    /// Whether `0 <= index < list.len()` follows from what holds.
    fn inside(&mut self, list: &str, index: &Expr, facts: &Facts) -> bool {
        if self.pinned.contains(list) {
            return false;
        }
        let Some(at) = self.lin(index, facts) else {
            return false;
        };
        let length = length_of(list);
        let (len, zero) = (self.arena.var(&length), self.arena.int(0));
        let low = self.arena.ge(at, zero);
        let high = self.arena.lt(at, len);
        let goal = self.arena.and(vec![low, high]);
        let query = Query {
            arena: &self.arena,
            facts: &facts.facts,
            goal,
        };
        match FourierMotzkin.check(&query, &Budget::default()) {
            Answer::Proved { certificate } => verify(&query, &certificate).is_ok(),
            _ => false,
        }
    }

    /// A block; whether control never goes past its end.
    fn block(&mut self, block: &Block, facts: &mut Facts) -> bool {
        let before = facts.ints.clone();
        let mut bound: Vec<String> = Vec::new();
        let mut leaves = false;
        for stmt in &block.stmts {
            if let Stmt::Let { names, .. } = &stmt.node {
                bound.extend(names.iter().map(|n| self.text(*n).to_string()));
            }
            if let Stmt::Comptime { name, .. } = &stmt.node {
                bound.push(self.text(*name).to_string());
            }
            if self.stmt(&stmt.node, facts) {
                leaves = true;
                break;
            }
        }
        // The block's own names leave with it; a name it hid is the outer
        // one again, and what was known of that was forgotten where it was
        // hidden.
        for name in &bound {
            self.forget(facts, name);
            facts.ints.remove(name);
            if before.contains(name) {
                facts.ints.insert(name.clone());
            }
        }
        leaves
    }

    fn stmt(&mut self, stmt: &Stmt, facts: &mut Facts) -> bool {
        match stmt {
            Stmt::Let {
                names, ty, value, ..
            } => {
                self.expr(value, facts);
                let value_lin = self.lin(value, facts);
                let whole = match ty {
                    Some(t) => t.generics.is_empty() && is_whole_number(self.text(t.name)),
                    None => value_lin.is_some(),
                };
                for name in names {
                    let name = self.text(*name).to_string();
                    self.forget(facts, &name);
                    facts.ints.remove(&name);
                }
                if let [only] = names.as_slice() {
                    let name = self.text(*only).to_string();
                    if whole {
                        facts.ints.insert(name.clone());
                        if let Some(v) = value_lin
                            && !self.arena.mentions(v, &name)
                        {
                            let n = self.arena.var(&name);
                            let fact = self.arena.eq(n, v);
                            self.push(facts, fact);
                        }
                        if ty
                            .as_ref()
                            .is_some_and(|t| self.text(t.name).starts_with('u'))
                        {
                            self.at_least_zero(facts, &name);
                        }
                    }
                    self.built(facts, &name, value);
                }
                false
            }
            Stmt::Comptime { name, value, .. } => {
                self.expr(value, facts);
                let name = self.text(*name).to_string();
                self.forget(facts, &name);
                facts.ints.remove(&name);
                false
            }
            // **The value first, and then the write**, in the order the
            // lowering runs them (ADR-114 D2): a value that shrinks the list
            // is reached before the index it is written at.
            Stmt::Assign { target, op, value } => {
                self.expr(value, facts);
                self.place(target, facts);
                if let Expr::Variable(name) = target {
                    let name = self.text(*name).to_string();
                    self.assign(facts, &name, *op, value);
                }
                false
            }
            Stmt::For {
                bindings,
                iter,
                body,
            } => {
                self.expr(iter, facts);
                let changed = self.changed(body);
                let range = match (bindings.as_slice(), iter) {
                    (
                        [only],
                        Expr::Range {
                            start,
                            end,
                            inclusive,
                        },
                    ) => {
                        let low = self.lin(start, facts);
                        let high = self.lin(end, facts);
                        Some((self.text(*only).to_string(), low, high, *inclusive))
                    }
                    _ => None,
                };
                let mut inner = facts.clone();
                self.after_change(&mut inner, &changed);
                for binding in bindings {
                    let name = self.text(*binding).to_string();
                    self.forget(&mut inner, &name);
                    inner.ints.remove(&name);
                }
                if let Some((name, low, high, inclusive)) = range {
                    inner.ints.insert(name.clone());
                    let n = self.arena.var(&name);
                    // A bound is read once, before the first turn: it holds
                    // of the binding while nothing it reads has changed.
                    let stable = |walk: &Self, t: TermId| {
                        changed.values.iter().all(|c| !walk.arena.mentions(t, c))
                            && changed
                                .lengths
                                .iter()
                                .all(|c| !walk.arena.mentions(t, &length_of(c)))
                            && !walk.arena.mentions(t, &name)
                    };
                    if let Some(low) = low
                        && stable(self, low)
                    {
                        let fact = self.arena.le(low, n);
                        self.push(&mut inner, fact);
                    }
                    if let Some(high) = high
                        && stable(self, high)
                    {
                        let fact = match inclusive {
                            true => self.arena.le(n, high),
                            false => self.arena.lt(n, high),
                        };
                        self.push(&mut inner, fact);
                    }
                }
                self.block(body, &mut inner);
                self.after_change(facts, &changed);
                false
            }
            Stmt::While { cond, body } => {
                let mut changed = self.changed(body);
                changed.extend(self.changed_in_expr(cond));
                self.after_change(facts, &changed);
                self.expr(cond, facts);
                let mut inner = facts.clone();
                self.assume(&mut inner, cond);
                self.block(body, &mut inner);
                if !breaks(body) {
                    self.assume_not(facts, cond);
                }
                false
            }
            Stmt::Return(value) => {
                if let Some(value) = value {
                    self.expr(value, facts);
                }
                true
            }
            Stmt::Break | Stmt::Continue => true,
            Stmt::Expr(expr) => self.expr_leaves(expr, facts),
        }
    }

    /// `name = value`, or `name op= value`.
    fn assign(&mut self, facts: &mut Facts, name: &str, op: Option<BinaryOp>, value: &Expr) {
        if !facts.ints.contains(name) {
            self.forget(facts, name);
            self.built(facts, name, value);
            return;
        }
        let value_lin = self.lin(value, facts);
        let now = self.arena.var(name);
        let by = match (op, value_lin) {
            (Some(BinaryOp::Add), Some(v)) if !self.arena.mentions(v, name) => Some(v),
            (Some(BinaryOp::Sub), Some(v)) if !self.arena.mentions(v, name) => {
                Some(self.arena.neg(v))
            }
            // `n = n + d`, with `d` not reading `n`.
            (None, Some(v)) if self.arena.mentions(v, name) => {
                let d = self.arena.sub(v, now);
                let zero = BTreeMap::from([(name.to_string(), self.arena.int(0))]);
                let d0 = self.arena.substitute(d, &zero);
                // Linear in `n` with coefficient one exactly where setting
                // `n` to zero and to one gives the same difference.
                let one = BTreeMap::from([(name.to_string(), self.arena.int(1))]);
                let d1 = self.arena.substitute(d, &one);
                let same = self.arena.eq(d0, d1);
                let query = Query {
                    arena: &self.arena,
                    facts: &[],
                    goal: same,
                };
                let linear = matches!(
                    FourierMotzkin.check(&query, &Budget::default()),
                    Answer::Proved { certificate } if verify(&query, &certificate).is_ok()
                );
                linear.then_some(d0)
            }
            _ => None,
        };
        match (op, by, value_lin) {
            (_, Some(by), _) => self.shift(facts, name, by),
            (None, None, Some(v)) => {
                self.forget(facts, name);
                let n = self.arena.var(name);
                let fact = self.arena.eq(n, v);
                self.push(facts, fact);
            }
            _ => {
                self.forget(facts, name);
                if self.nonnegative.contains(name) {
                    self.at_least_zero(facts, name);
                }
            }
        }
    }

    /// What is known of `name`'s length from the value it was given.
    fn built(&mut self, facts: &mut Facts, name: &str, value: &Expr) {
        if let Expr::ListLit { items, .. } = value
            && let Ok(n) = i64::try_from(items.len())
        {
            let length = length_of(name);
            let (len, n) = (self.arena.var(&length), self.arena.int(n));
            let fact = self.arena.eq(len, n);
            self.push(facts, fact);
        }
    }

    /// The target of an assignment: an index there is a write, proved as a
    /// read is.
    fn place(&mut self, target: &Expr, facts: &mut Facts) {
        match target {
            Expr::Index { base, index } => {
                self.place(base, facts);
                self.expr(index, facts);
                if let Expr::Variable(list) = &**base {
                    let list = self.text(*list).to_string();
                    if self.inside(&list, index, facts) {
                        self.proven.insert(value_node(base));
                    }
                }
            }
            Expr::Field { base, .. } => self.place(base, facts),
            _ => {}
        }
    }

    /// An expression as a statement; whether control never goes past it.
    fn expr_leaves(&mut self, expr: &Expr, facts: &mut Facts) -> bool {
        match expr {
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(cond, facts);
                let mut then_facts = facts.clone();
                self.assume(&mut then_facts, cond);
                let then_leaves = self.block(then_branch, &mut then_facts);
                let mut else_facts = facts.clone();
                self.assume_not(&mut else_facts, cond);
                let else_leaves = match else_branch {
                    Some(block) => self.block(block, &mut else_facts),
                    None => false,
                };
                match (then_leaves, else_leaves) {
                    (true, true) => return true,
                    (true, false) => *facts = else_facts,
                    (false, true) => *facts = then_facts,
                    (false, false) => {
                        let mut changed = self.changed(then_branch);
                        if let Some(block) = else_branch {
                            changed.extend(self.changed(block));
                        }
                        self.after_change(facts, &changed);
                    }
                }
                false
            }
            Expr::Return(value) => {
                if let Some(value) = &**value {
                    self.expr(value, facts);
                }
                true
            }
            Expr::Break | Expr::Continue => true,
            Expr::Throw(value) => {
                self.expr(value, facts);
                true
            }
            // `panic(…)` does not come back.
            Expr::Call { func, args, .. } if matches!(&**func, Expr::Variable(n) if self.text(*n) == "panic") =>
            {
                for arg in args {
                    self.expr(arg, facts);
                }
                true
            }
            _ => {
                self.expr(expr, facts);
                false
            }
        }
    }

    /// Walk an expression: prove the indices in it with what holds, and keep
    /// what it changes.
    fn expr(&mut self, expr: &Expr, facts: &mut Facts) {
        match expr {
            Expr::Index { base, index } => {
                self.expr(base, facts);
                self.expr(index, facts);
                if let Expr::Variable(list) = &**base {
                    let list = self.text(*list).to_string();
                    if self.inside(&list, index, facts) {
                        self.proven.insert(value_node(base));
                    }
                }
            }
            // The right side runs only where the left one decided it does.
            Expr::Binary {
                op: op @ (BinaryOp::And | BinaryOp::Or),
                lhs,
                rhs,
                ..
            } => {
                self.expr(lhs, facts);
                let mut right = facts.clone();
                match op {
                    BinaryOp::And => self.assume(&mut right, lhs),
                    _ => self.assume_not(&mut right, lhs),
                }
                self.expr(rhs, &mut right);
                let changed = self.changed_in_expr(rhs);
                self.after_change(facts, &changed);
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(cond, facts);
                let mut then_facts = facts.clone();
                self.assume(&mut then_facts, cond);
                self.block(then_branch, &mut then_facts);
                let mut changed = self.changed(then_branch);
                if let Some(block) = else_branch {
                    let mut else_facts = facts.clone();
                    self.assume_not(&mut else_facts, cond);
                    self.block(block, &mut else_facts);
                    changed.extend(self.changed(block));
                }
                self.after_change(facts, &changed);
            }
            Expr::Block(block) | Expr::Unsafe(block) => {
                let mut inner = facts.clone();
                self.block(block, &mut inner);
                let changed = self.changed(block);
                self.after_change(facts, &changed);
            }
            Expr::Match { value, arms } => {
                self.expr(value, facts);
                let mut changed = Changed::default();
                for arm in arms {
                    // A pattern may bind a name that hides an outer one, so
                    // nothing known outside is carried into an arm.
                    let mut inner = Facts::default();
                    if let Some(guard) = &arm.guard {
                        self.expr(guard, &mut inner);
                    }
                    self.expr(&arm.body, &mut inner);
                    changed.extend(self.changed_in_expr(&arm.body));
                }
                self.after_change(facts, &changed);
            }
            // Not entered: a lambda or a task may run at any later moment.
            Expr::Closure { .. } | Expr::Spawn { .. } | Expr::Overlap(_) | Expr::Select(_) => {}
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            }
            | Expr::SafeMethod {
                receiver,
                method,
                args,
                ..
            } => {
                self.expr(receiver, facts);
                for arg in args {
                    self.expr(arg, facts);
                }
                let method = self.text(*method).to_string();
                if let Expr::Variable(name) = &**receiver {
                    let name = self.text(*name).to_string();
                    self.method(facts, &name, &method, args);
                }
                let changes_its_arguments = self.changing_methods.contains(&method);
                for arg in args {
                    if let Expr::Variable(name) = arg {
                        let name = self.text(*name).to_string();
                        if changes_its_arguments {
                            self.forget(facts, &name);
                        } else if !facts.ints.contains(&name) {
                            self.forget_length(facts, &name);
                        }
                    }
                }
            }
            Expr::Call { func, args, config } => {
                self.expr(func, facts);
                for arg in args {
                    self.expr(arg, facts);
                }
                for c in config {
                    self.expr(&c.value, facts);
                }
                let callee = match &**func {
                    Expr::Variable(name) => self.functions.get(self.text(*name)).cloned(),
                    _ => None,
                };
                for (at, arg) in args.iter().enumerate() {
                    if let Expr::Variable(name) = arg {
                        let changes = callee
                            .as_ref()
                            .is_none_or(|params| params.get(at).copied().unwrap_or(true));
                        if changes {
                            let name = self.text(*name).to_string();
                            self.forget(facts, &name);
                        }
                    }
                }
            }
            _ => {
                let mut children: Vec<&Expr> = Vec::new();
                for_each_child(expr, &mut |child| children.push(child));
                for child in children {
                    self.expr(child, facts);
                }
                let mut blocks: Vec<&Block> = Vec::new();
                for_each_block_ref(expr, &mut blocks);
                for block in blocks {
                    let mut inner = facts.clone();
                    self.block(block, &mut inner);
                    let changed = self.changed(block);
                    self.after_change(facts, &changed);
                }
            }
        }
    }

    /// A method on a name: what it does to the name's length.
    fn method(&mut self, facts: &mut Facts, name: &str, method: &str, args: &[Expr]) {
        if keeps_length(method) || facts.ints.contains(name) {
            return;
        }
        let length = length_of(name);
        match (method, args) {
            ("push", [_]) | ("insert", [_, _]) => {
                let one = self.arena.int(1);
                self.shift(facts, &length, one);
            }
            ("remove", [_]) | ("swap_remove", [_]) => {
                let one = self.arena.int(-1);
                self.shift(facts, &length, one);
            }
            ("clear", []) => {
                self.forget_length(facts, name);
                let (len, zero) = (self.arena.var(&length), self.arena.int(0));
                let fact = self.arena.eq(len, zero);
                self.push(facts, fact);
            }
            ("resize", [to, _]) => {
                let to = self.lin(to, facts);
                self.forget_length(facts, name);
                if let Some(to) = to {
                    let len = self.arena.var(&length);
                    let fact = self.arena.eq(len, to);
                    self.push(facts, fact);
                }
            }
            _ => self.forget_length(facts, name),
        }
    }

    /// The names whose value, and the lists whose length, `block` may change.
    fn changed(&self, block: &Block) -> Changed {
        let mut out = Changed::default();
        // A `let` in a nested block hides a name only there; the walk
        // forgets it where it is bound. One in a loop's body is bound again
        // each turn, which the walk sees the same way.
        visit_stmts(block, &mut |stmt| {
            if let Stmt::Assign {
                target: Expr::Variable(name),
                ..
            } = stmt
            {
                let name = self.text(*name).to_string();
                out.lengths.insert(name.clone());
                out.values.insert(name);
            }
        });
        visit_exprs(block, &mut |expr| self.changed_by(expr, &mut out));
        out
    }

    fn changed_in_expr(&self, expr: &Expr) -> Changed {
        let block = Block {
            stmts: vec![Spanned::new(Stmt::Expr(expr.clone()), Span::nowhere())],
        };
        self.changed(&block)
    }

    fn changed_by(&self, expr: &Expr, out: &mut Changed) {
        match expr {
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            }
            | Expr::SafeMethod {
                receiver,
                method,
                args,
                ..
            } => {
                if let Expr::Variable(name) = &**receiver
                    && !keeps_length(self.text(*method))
                {
                    out.lengths.insert(self.text(*name).to_string());
                }
                let changes_its_arguments = self.changing_methods.contains(self.text(*method));
                for arg in args {
                    if let Expr::Variable(name) = arg {
                        let name = self.text(*name).to_string();
                        if changes_its_arguments {
                            out.values.insert(name.clone());
                        }
                        out.lengths.insert(name);
                    }
                }
            }
            Expr::Call { func, args, .. } => {
                let callee = match &**func {
                    Expr::Variable(name) => self.functions.get(self.text(*name)),
                    _ => None,
                };
                for (at, arg) in args.iter().enumerate() {
                    if let Expr::Variable(name) = arg
                        && callee.is_none_or(|params| params.get(at).copied().unwrap_or(true))
                    {
                        let name = self.text(*name).to_string();
                        out.lengths.insert(name.clone());
                        out.values.insert(name);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The names a statement or expression may change.
#[derive(Default)]
struct Changed {
    values: BTreeSet<String>,
    lengths: BTreeSet<String>,
}

impl Changed {
    fn extend(&mut self, other: Changed) {
        self.values.extend(other.values);
        self.lengths.extend(other.lengths);
    }
}

/// `xs.len()` is about `xs`.
fn base_of(name: &str) -> &str {
    name.strip_suffix(".len()").unwrap_or(name)
}

/// Whether a `break` leaves this loop from its body (not from a loop inside it).
fn breaks(body: &Block) -> bool {
    fn block(b: &Block) -> bool {
        b.stmts.iter().any(|s| match &s.node {
            Stmt::Break => true,
            Stmt::For { .. } | Stmt::While { .. } => false,
            Stmt::Expr(e) | Stmt::Return(Some(e)) => expr(e),
            Stmt::Let { value, .. } | Stmt::Comptime { value, .. } => expr(value),
            Stmt::Assign { value, .. } => expr(value),
            _ => false,
        })
    }
    fn expr(e: &Expr) -> bool {
        if matches!(e, Expr::Break) {
            return true;
        }
        if matches!(e, Expr::Closure { .. } | Expr::Spawn { .. }) {
            return false;
        }
        let mut found = false;
        for_each_child(e, &mut |c| found |= expr(c));
        for_each_block(e, &mut |b| found |= block(b));
        found
    }
    block(body)
}

/// The names a lambda, a task, an `overlap` branch or a `select` arm in
/// `body` changes or hands on.
fn pinned_in(parsed: &Parsed, body: &Block) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut deferred: Vec<Block> = Vec::new();
    visit_exprs(body, &mut |expr| match expr {
        Expr::Closure { body, .. } | Expr::Overlap(body) => deferred.push(body.clone()),
        Expr::Spawn { body, .. } => deferred.push(Block {
            stmts: vec![Spanned::new(Stmt::Expr((**body).clone()), Span::nowhere())],
        }),
        Expr::Select(arms) => deferred.extend(arms.iter().map(|a| a.body.clone())),
        _ => {}
    });
    for block in &deferred {
        visit_stmts(block, &mut |stmt| {
            if let Stmt::Assign { target, .. } = stmt {
                let mut root = target;
                while let Expr::Index { base, .. } | Expr::Field { base, .. } = root {
                    root = base;
                }
                if let Expr::Variable(name) = root {
                    out.insert(parsed.text(*name).to_string());
                }
            }
        });
        visit_exprs(block, &mut |expr| match expr {
            Expr::MethodCall { receiver, args, .. } | Expr::SafeMethod { receiver, args, .. } => {
                if let Expr::Variable(name) = &**receiver {
                    out.insert(parsed.text(*name).to_string());
                }
                for arg in args {
                    if let Expr::Variable(name) = arg {
                        out.insert(parsed.text(*name).to_string());
                    }
                }
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    if let Expr::Variable(name) = arg {
                        out.insert(parsed.text(*name).to_string());
                    }
                }
            }
            _ => {}
        });
    }
    out
}

// --- walking the tree ---------------------------------------------------------

/// Each expression directly inside `expr`, not inside one of its blocks.
fn for_each_child<'e>(expr: &'e Expr, f: &mut dyn FnMut(&'e Expr)) {
    match expr {
        Expr::Match { value, arms } => {
            f(value);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    f(guard);
                }
                f(&arm.body);
            }
        }
        Expr::Range { start, end, .. } => {
            f(start);
            f(end);
        }
        Expr::Tuple(items) | Expr::ListLit { items, .. } => items.iter().for_each(f),
        Expr::LitInterpolated { parts } => {
            for part in parts {
                if let FPart::Hole { expr, .. } = part {
                    f(expr);
                }
            }
        }
        Expr::If { cond, .. } => f(cond),
        Expr::Call { func, args, config } => {
            f(func);
            args.iter().for_each(&mut *f);
            for c in config {
                f(&c.value);
            }
        }
        Expr::Spawn { body, .. } => f(body),
        Expr::MethodCall {
            receiver,
            args,
            config,
            ..
        }
        | Expr::SafeMethod {
            receiver,
            args,
            config,
            ..
        } => {
            f(receiver);
            args.iter().for_each(&mut *f);
            for c in config {
                f(&c.value);
            }
        }
        Expr::Field { base, .. } | Expr::SafeField { base, .. } => f(base),
        Expr::StructLit { fields, .. } => {
            for field in fields {
                if let Some(value) = &field.value {
                    f(value);
                }
            }
        }
        Expr::With { base, fields, .. } => {
            f(base);
            for field in fields {
                if let Some(value) = &field.value {
                    f(value);
                }
            }
        }
        Expr::Unary { expr, .. }
        | Expr::Try(expr)
        | Expr::Throw(expr)
        | Expr::Cast { expr, .. }
        | Expr::TryCatch { expr, .. } => f(expr),
        Expr::Binary { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        Expr::Return(value) => {
            if let Some(value) = &**value {
                f(value);
            }
        }
        Expr::Index { base, index } => {
            f(base);
            f(index);
        }
        Expr::Coalesce { value, fallback } => {
            f(value);
            f(fallback);
        }
        Expr::Select(arms) => {
            for arm in arms {
                f(&arm.value);
            }
        }
        _ => {}
    }
}

/// Each block directly inside `expr`.
fn for_each_block<'e>(expr: &'e Expr, f: &mut dyn FnMut(&'e Block)) {
    match expr {
        Expr::Block(b) | Expr::Overlap(b) | Expr::Unsafe(b) => f(b),
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => {
            f(then_branch);
            if let Some(b) = else_branch {
                f(b);
            }
        }
        Expr::Closure { body, .. } => f(body),
        Expr::TryCatch { handler, .. } => f(handler),
        Expr::Select(arms) => {
            for arm in arms {
                f(&arm.body);
            }
        }
        _ => {}
    }
}

fn for_each_block_ref<'e>(expr: &'e Expr, out: &mut Vec<&'e Block>) {
    for_each_block(expr, &mut |b| out.push(b));
}

/// Every statement in `block` and the blocks inside it, lambdas included.
fn visit_stmts<'b>(block: &'b Block, f: &mut dyn FnMut(&'b Stmt)) {
    for stmt in &block.stmts {
        f(&stmt.node);
        match &stmt.node {
            Stmt::For { iter, body, .. } => {
                visit_stmts_expr(iter, f);
                visit_stmts(body, f);
            }
            Stmt::While { cond, body } => {
                visit_stmts_expr(cond, f);
                visit_stmts(body, f);
            }
            Stmt::Let { value, .. } | Stmt::Comptime { value, .. } | Stmt::Expr(value) => {
                visit_stmts_expr(value, f)
            }
            Stmt::Assign { target, value, .. } => {
                visit_stmts_expr(target, f);
                visit_stmts_expr(value, f);
            }
            Stmt::Return(Some(value)) => visit_stmts_expr(value, f),
            _ => {}
        }
    }
}

fn visit_stmts_expr<'b>(expr: &'b Expr, f: &mut dyn FnMut(&'b Stmt)) {
    for_each_child(expr, &mut |c| visit_stmts_expr(c, f));
    for_each_block(expr, &mut |b| visit_stmts(b, f));
}

/// Every expression in `block` and the blocks inside it, lambdas included.
fn visit_exprs<'b>(block: &'b Block, f: &mut dyn FnMut(&'b Expr)) {
    for stmt in &block.stmts {
        match &stmt.node {
            Stmt::For { iter, body, .. } => {
                visit_expr(iter, f);
                visit_exprs(body, f);
            }
            Stmt::While { cond, body } => {
                visit_expr(cond, f);
                visit_exprs(body, f);
            }
            Stmt::Let { value, .. } | Stmt::Comptime { value, .. } | Stmt::Expr(value) => {
                visit_expr(value, f)
            }
            Stmt::Assign { target, value, .. } => {
                visit_expr(target, f);
                visit_expr(value, f);
            }
            Stmt::Return(Some(value)) => visit_expr(value, f),
            _ => {}
        }
    }
}

fn visit_expr<'b>(expr: &'b Expr, f: &mut dyn FnMut(&'b Expr)) {
    f(expr);
    for_each_child(expr, &mut |c| visit_expr(c, f));
    for_each_block(expr, &mut |b| visit_exprs(b, f));
}
