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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use nikaia_logic::{Answer, Arena, Budget, FourierMotzkin, Query, Solver, TermId, verify};

use crate::ast::{BinaryOp, Block, Expr, FPart, Item, Span, Spanned, Stmt, UnaryOp};
use crate::check::value_node;
use crate::parser::Parsed;

/// How hard the compiler works to drop an index check (ADR-306 D1).
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

/// How hard the compiler works to drop an overflow check (ADR-306 D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
pub enum OverflowChecks {
    /// Every `+`, `-` and `*` is checked: the default.
    #[default]
    Kept,
    /// Every one the walk proves stays inside its type is not.
    Aggressive,
}

impl OverflowChecks {
    /// The word after `remove-overflow-checks:`. There is no `basic`: the one
    /// shape a walk without a solver would prove is the one LLVM proves.
    pub fn parse(word: &str) -> Option<OverflowChecks> {
        match word {
            "off" => Some(OverflowChecks::Kept),
            "aggressive" => Some(OverflowChecks::Aggressive),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            OverflowChecks::Kept => "off",
            OverflowChecks::Aggressive => "aggressive",
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

/// The index nodes whose check is proved unnecessary at `level`, and the
/// arithmetic whose overflow check is at `overflow`.
///
/// `lengths` are the receivers of the `x.len()` calls that count a `std`
/// list, text or map ([`crate::check::Checked::std_lengths`]): only those are
/// numbers of a proof, because a `len()` a program writes for its own type
/// may say anything. `arithmetic` is every `+`, `-` and `*` whose two sides
/// are one whole-number type, with that type
/// ([`crate::check::Checked::arithmetic`]): only those may be proved.
pub fn proven(
    parsed: &Parsed,
    level: BoundsChecks,
    overflow: OverflowChecks,
    lengths: &BTreeSet<usize>,
    arithmetic: &BTreeMap<usize, String>,
) -> Proven {
    let mut out = Proven::default();
    if level == BoundsChecks::Kept && overflow == OverflowChecks::Kept {
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
        if level >= BoundsChecks::Basic {
            basic_block(parsed, body, &mut Vec::new(), &mut out.indices);
        }
        if level < BoundsChecks::Aggressive && overflow == OverflowChecks::Kept {
            continue;
        }
        let pinned = pinned_in(parsed, body);
        let shape = Shape::of(parsed, args, body, &pinned, &functions, &changing_methods);
        let mut walk = Walk {
            parsed,
            lengths,
            functions: &functions,
            changing_methods: &changing_methods,
            arena: Arena::new(),
            proven: Proven::default(),
            pinned,
            nonnegative: crate::emit::nonnegative_names(body)
                .into_iter()
                .map(|s| parsed.text(s).to_string())
                .collect(),
            arithmetic,
            shape,
            assumed: HashMap::new(),
            written: HashMap::new(),
            poisoned: HashSet::new(),
            collecting: false,
        };
        // **A bound on a list's values is an invariant, found and then
        // checked** (ADR-306 D7): each pass reads the bounds the last one
        // found, and the last pass keeps only those every write is proved
        // to stay within, assuming all of them. Reads before the writes they
        // depend on, and writes that depend on each other, are why one pass
        // is not enough; a bound that only holds by assuming itself larger
        // is dropped.
        //
        // The passes that only collect prove nothing. The pass that checks
        // proves too, and when it keeps every bound it assumed, what it proved
        // stands.
        let mut settled = false;
        if !walk.shape.roots.is_empty() {
            walk.collecting = true;
            for _ in 0..2 {
                walk.pass(args, body);
                walk.assumed = walk.found();
            }
            walk.collecting = false;
            for _ in 0..4 {
                walk.proven = Proven::default();
                walk.pass(args, body);
                let found = walk.found();
                let before = walk.assumed.len();
                walk.assumed
                    .retain(|key, bound| found.get(key).is_some_and(|f| bound.holds(f)));
                if walk.assumed.len() == before {
                    settled = true;
                    break;
                }
            }
        }
        if !settled {
            if !walk.shape.roots.is_empty() {
                walk.assumed.clear();
            }
            walk.proven = Proven::default();
            walk.pass(args, body);
        }
        if level == BoundsChecks::Aggressive {
            out.indices.extend(walk.proven.indices.iter().copied());
        }
        if overflow == OverflowChecks::Aggressive {
            out.arithmetic
                .extend(walk.proven.arithmetic.iter().copied());
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
    proven: Proven,
    /// Names a lambda, a task or an `overlap` branch changes: they may change
    /// whenever one runs, so no fact is kept about them anywhere.
    pinned: HashSet<String>,
    /// Whole numbers that are never negative anywhere in the body
    /// (`crate::emit::nonnegative_names`).
    nonnegative: HashSet<String>,
    /// [`proven`]'s `arithmetic`.
    arithmetic: &'a BTreeMap<usize, String>,
    /// What the body is, read once before any pass.
    shape: Shape,
    /// The bounds on lists' values this pass may read (ADR-306 D7), by key:
    /// a list's name for its values, `name[]` for the values of the lists it
    /// holds.
    assumed: HashMap<String, Range>,
    /// The join of what this pass saw written, by key.
    written: HashMap<String, Range>,
    /// Keys a write this pass put something unbounded in.
    poisoned: HashSet<String>,
    /// A pass that only finds bounds and proves nothing.
    collecting: bool,
}

/// **A closed interval of whole numbers**, either end open where nothing is
/// known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Range {
    lo: Option<i64>,
    hi: Option<i64>,
}

impl Range {
    fn exactly(n: i64) -> Range {
        Range {
            lo: Some(n),
            hi: Some(n),
        }
    }

    fn of_type(ty: &str) -> Option<Range> {
        let (lo, hi): (i128, i128) = match ty {
            "i8" => (i8::MIN.into(), i8::MAX.into()),
            "i16" => (i16::MIN.into(), i16::MAX.into()),
            "i32" => (i32::MIN.into(), i32::MAX.into()),
            "i64" | "isize" => (i64::MIN.into(), i64::MAX.into()),
            "u8" => (0, u8::MAX.into()),
            "u16" => (0, u16::MAX.into()),
            "u32" => (0, u32::MAX.into()),
            "u64" | "usize" => (0, u64::MAX.into()),
            _ => return None,
        };
        // A bound past what an `i64` holds is no bound the proof can state;
        // the nearer one is stronger, so claiming it is never wrong.
        Some(Range {
            lo: Some(lo.max(i64::MIN.into()) as i64),
            hi: Some(hi.min(i64::MAX.into()) as i64),
        })
    }

    /// Whether every number this one admits is one `other` admits too:
    /// `self` holds what `other` claims.
    fn holds(&self, other: &Range) -> bool {
        let lo = match (self.lo, other.lo) {
            (_, None) => self.lo.is_none(),
            (None, Some(_)) => true,
            (Some(a), Some(b)) => a <= b,
        };
        let hi = match (self.hi, other.hi) {
            (_, None) => self.hi.is_none(),
            (None, Some(_)) => true,
            (Some(a), Some(b)) => a >= b,
        };
        lo && hi
    }

    fn join(&self, other: &Range) -> Range {
        Range {
            lo: self.lo.zip(other.lo).map(|(a, b)| a.min(b)),
            hi: self.hi.zip(other.hi).map(|(a, b)| a.max(b)),
        }
    }

    fn meet(&self, other: &Range) -> Range {
        Range {
            lo: match (self.lo, other.lo) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            },
            hi: match (self.hi, other.hi) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
        }
    }

    fn is_bounded(&self) -> bool {
        self.lo.is_some() || self.hi.is_some()
    }

    fn add(&self, other: &Range) -> Range {
        Range {
            lo: self.lo.zip(other.lo).and_then(|(a, b)| a.checked_add(b)),
            hi: self.hi.zip(other.hi).and_then(|(a, b)| a.checked_add(b)),
        }
    }

    fn neg(&self) -> Range {
        Range {
            lo: self.hi.and_then(i64::checked_neg),
            hi: self.lo.and_then(i64::checked_neg),
        }
    }

    fn mul(&self, other: &Range) -> Range {
        let (Some(a), Some(b), Some(c), Some(d)) = (self.lo, self.hi, other.lo, other.hi) else {
            return Range { lo: None, hi: None };
        };
        let products = [
            a.checked_mul(c),
            a.checked_mul(d),
            b.checked_mul(c),
            b.checked_mul(d),
        ];
        if products.iter().any(Option::is_none) {
            return Range { lo: None, hi: None };
        }
        let products = products.map(|p| p.unwrap_or_default());
        Range {
            lo: products.iter().min().copied(),
            hi: products.iter().max().copied(),
        }
    }
}

impl Walk<'_> {
    fn text(&self, sym: winnow_grammar::Symbol) -> &str {
        self.parsed.text(sym)
    }

    /// One pass over the body: proofs into [`Self::proven`], what is
    /// written into [`Self::written`].
    fn pass(&mut self, args: &[crate::ast::FnArg], body: &Block) {
        self.written.clear();
        self.poisoned.clear();
        let mut facts = Facts::default();
        for arg in args {
            let name = self.text(arg.name).to_string();
            let ty = self.text(arg.ty.name).to_string();
            if arg.ty.generics.is_empty() && is_whole_number(&ty) {
                facts.ints.insert(name.clone());
                if ty.starts_with('u') {
                    self.at_least_zero(&mut facts, &name);
                }
            }
            if is_a_list(&ty) {
                let length = length_of(&name);
                self.at_least_zero(&mut facts, &length);
            }
        }
        self.block(body, &mut facts);
    }

    /// The bounds this pass found: every key written, none written
    /// something unbounded.
    fn found(&self) -> HashMap<String, Range> {
        self.written
            .iter()
            .filter(|(key, bound)| bound.is_bounded() && !self.is_poisoned(key))
            .map(|(key, bound)| (key.clone(), *bound))
            .collect()
    }

    /// A key is out where it, or a list it is part of, was written something
    /// unbounded: `xs.push(ys)` with `ys` unknown says nothing of `xs[]`.
    fn is_poisoned(&self, key: &str) -> bool {
        let mut at = key;
        loop {
            if self.poisoned.contains(at) {
                return true;
            }
            match at.strip_suffix("[]") {
                Some(shorter) => at = shorter,
                None => return false,
            }
        }
    }

    /// **`value` is written into the list whose elements are `key`**
    /// (ADR-306 D7): a number joins the key's bound, a list's items join
    /// `key[]`'s, a list the walk knows joins its bound into `key[]`.
    fn write(&mut self, key: &str, value: &Expr, facts: &Facts) {
        if !self.shape.tracks(key) {
            return;
        }
        let inner = format!("{key}[]");
        if let Expr::ListLit { items, .. } = value {
            for item in items {
                self.write(&inner, item, facts);
            }
            return;
        }
        if let Some(range) = self.range(value, facts) {
            self.join(key, range);
            return;
        }
        if let Some(elements) = self.shape.elements_key(self.parsed, value) {
            match self.assumed.get(&elements).copied() {
                Some(bound) => self.join(&inner, bound),
                None => {
                    self.poisoned.insert(inner);
                }
            }
            return;
        }
        self.poisoned.insert(key.to_string());
    }

    fn join(&mut self, key: &str, range: Range) {
        if !range.is_bounded() {
            self.poisoned.insert(key.to_string());
            return;
        }
        let joined = match self.written.get(key) {
            Some(was) => was.join(&range),
            None => range,
        };
        self.written.insert(key.to_string(), joined);
    }

    /// Whether `goal` follows from what holds, on a certificate the checker
    /// accepts.
    fn proves(&self, facts: &Facts, goal: TermId) -> bool {
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

    /// **What is known of a linear term, as constants**: a candidate from
    /// interval propagation over the facts, as Wuffs bounds a value, each end
    /// then confirmed by the solver on a certificate the checker accepts. An
    /// end the solver does not confirm is open.
    fn bounds_of(&mut self, term: TermId, facts: &Facts) -> Range {
        if let Some(n) = self.arena.constant(term) {
            return Range::exactly(n);
        }
        let Some(form) = linear_form(&self.arena, term) else {
            return Range { lo: None, hi: None };
        };
        let known = propagate(&self.arena, &facts.facts);
        let candidate = evaluate(&form, &known);
        let mut confirmed = Range { lo: None, hi: None };
        if let Some(hi) = candidate.hi {
            let k = self.arena.int(hi);
            let goal = self.arena.le(term, k);
            if self.proves(facts, goal) {
                confirmed.hi = Some(hi);
            }
        }
        if let Some(lo) = candidate.lo {
            let k = self.arena.int(lo);
            let goal = self.arena.ge(term, k);
            if self.proves(facts, goal) {
                confirmed.lo = Some(lo);
            }
        }
        confirmed
    }

    /// **The constants a whole number lies between**, where the walk can
    /// tell: a linear term by the solver, and around it `%` (ADR-306 D8),
    /// `>>`, `&`, a conversion, a truncation, and a value read out of a list
    /// whose values are bounded (D2). `None` is "not a number the walk
    /// knows", which is also what anything else is.
    fn range(&mut self, expr: &Expr, facts: &Facts) -> Option<Range> {
        if let Some(term) = self.lin(expr, facts) {
            return Some(self.bounds_of(term, facts));
        }
        match expr {
            Expr::Binary { op, lhs, rhs, .. } => match op {
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => {
                    let a = self.range(lhs, facts)?;
                    let b = self.range(rhs, facts)?;
                    Some(match op {
                        BinaryOp::Add => a.add(&b),
                        BinaryOp::Sub => a.add(&b.neg()),
                        _ => a.mul(&b),
                    })
                }
                // **`0 <= x % n < n`** where `x` is not negative and `n` is
                // positive; `|x % n| < n` where only `n` is known.
                BinaryOp::Rem => {
                    let n = self.range(rhs, facts)?;
                    let (Some(low), Some(high)) = (n.lo, n.hi) else {
                        return None;
                    };
                    if low < 1 {
                        return None;
                    }
                    let x = self.range(lhs, facts);
                    Some(match x {
                        Some(x) if x.lo.is_some_and(|l| l >= 0) => Range {
                            lo: Some(0),
                            hi: Some(x.hi.map_or(high - 1, |h| h.min(high - 1))),
                        },
                        _ => Range {
                            lo: Some(-(high - 1)),
                            hi: Some(high - 1),
                        },
                    })
                }
                BinaryOp::Shr => {
                    let k = self.range(rhs, facts)?;
                    let (Some(k), Some(same)) = (k.lo, k.hi) else {
                        return None;
                    };
                    if k != same || !(0..64).contains(&k) {
                        return None;
                    }
                    let x = self.range(lhs, facts)?;
                    x.lo.filter(|l| *l >= 0)?;
                    Some(Range {
                        lo: x.lo.map(|l| l >> k),
                        hi: x.hi.map(|h| h >> k),
                    })
                }
                BinaryOp::BitAnd => {
                    let mut mask = None;
                    for side in [lhs, rhs] {
                        if let Some(t) = self.lin(side, facts)
                            && let Some(c) = self.arena.constant(t)
                            && c >= 0
                        {
                            mask = Some(c);
                        }
                    }
                    let mask = mask?;
                    Some(Range {
                        lo: Some(0),
                        hi: Some(mask),
                    })
                }
                _ => None,
            },
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
            } => self.range(expr, facts).map(|r| r.neg()),
            // A conversion keeps its value or stops the program.
            Expr::Cast { expr, ty } if ty.generics.is_empty() => {
                let into = Range::of_type(self.text(ty.name))?;
                Some(match self.range(expr, facts) {
                    Some(r) => r.meet(&into),
                    None => into,
                })
            }
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } if args.is_empty() => {
                let method = self.text(*method).to_string();
                if method == "len" {
                    return Some(Range {
                        lo: Some(0),
                        hi: None,
                    });
                }
                let into = Range::of_type(method.strip_prefix("truncating_")?)?;
                Some(match self.range(receiver, facts) {
                    Some(r) if into.holds(&r) => r,
                    _ => into,
                })
            }
            Expr::Index { base, .. } => {
                let key = self.shape.elements_key(self.parsed, base)?;
                self.assumed.get(&key).copied()
            }
            _ => None,
        }
    }

    /// Whether `expr`, an operation of type `ty`, stays inside it.
    fn fits(&mut self, expr: &Expr, ty: &str, facts: &Facts) -> bool {
        let Some(bounds) = Range::of_type(ty) else {
            return false;
        };
        let (Some(lo), Some(hi)) = (bounds.lo, bounds.hi) else {
            return false;
        };
        if let Some(term) = self.lin(expr, facts) {
            let (low, high) = (self.arena.int(lo), self.arena.int(hi));
            let above = self.arena.ge(term, low);
            let below = self.arena.le(term, high);
            let goal = self.arena.and(vec![above, below]);
            return self.proves(facts, goal);
        }
        self.range(expr, facts).is_some_and(|r| bounds.holds(&r))
    }

    /// `name` lies within `range`, where it is a whole number.
    fn bounded(&mut self, facts: &mut Facts, name: &str, range: Range) {
        let n = self.arena.var(name);
        if let Some(lo) = range.lo {
            let lo = self.arena.int(lo);
            let fact = self.arena.ge(n, lo);
            self.push(facts, fact);
        }
        if let Some(hi) = range.hi {
            let hi = self.arena.int(hi);
            let fact = self.arena.le(n, hi);
            self.push(facts, fact);
        }
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
            if (self.nonnegative.contains(name) || self.shape.unsigned.contains(name))
                && facts.ints.contains(name)
            {
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
                if let Some(n) = self.shape.constants.get(&name) {
                    return Some(self.arena.int(*n));
                }
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
        if self.collecting || self.pinned.contains(list) {
            return false;
        }
        let length = length_of(list);
        let len = self.arena.var(&length);
        let Some(at) = self.lin(index, facts) else {
            // A position read out of a list whose values are bounded
            // (ADR-306 D7), or one `%` keeps small (D3).
            let Some(Range {
                lo: Some(lo),
                hi: Some(hi),
            }) = self.range(index, facts)
            else {
                return false;
            };
            if lo < 0 {
                return false;
            }
            let hi = self.arena.int(hi);
            let goal = self.arena.lt(hi, len);
            return self.proves(facts, goal);
        };
        let zero = self.arena.int(0);
        let low = self.arena.ge(at, zero);
        let high = self.arena.lt(at, len);
        let goal = self.arena.and(vec![low, high]);
        self.proves(facts, goal)
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
                // **A number the walk bounds without a linear term**: a value
                // read out of a list, a `%` (ADR-306 D7, D8).
                let value_range = match value_lin {
                    Some(_) => None,
                    None => self.range(value, facts).filter(Range::is_bounded),
                };
                let whole = match ty {
                    Some(t) => t.generics.is_empty() && is_whole_number(self.text(t.name)),
                    None => value_lin.is_some() || value_range.is_some(),
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
                        if let Some(range) = value_range {
                            self.bounded(facts, &name, range);
                        }
                    }
                    self.built(facts, &name, value);
                    if self.shape.roots.contains(&name)
                        && let Expr::ListLit { items, .. } = value
                    {
                        for item in items {
                            self.write(&name, item, facts);
                        }
                    }
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
            // lowering runs them (ADR-293 D2): a value that shrinks the list
            // is reached before the index it is written at.
            Stmt::Assign { target, op, value } => {
                self.expr(value, facts);
                self.place(target, facts);
                if op.is_none()
                    && let Expr::Index { base, .. } = target
                    && let Some(key) = self.shape.elements_key(self.parsed, base)
                {
                    self.write(&key, value, facts);
                }
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
                // `for x in xs`, where what `xs` holds is bounded (ADR-306 D7).
                if let [only] = bindings.as_slice()
                    && !matches!(iter, Expr::Range { .. })
                    && let Some(key) = self.shape.elements_key(self.parsed, iter)
                    && let Some(bound) = self.assumed.get(&key).copied()
                {
                    let name = self.text(*only).to_string();
                    inner.ints.insert(name.clone());
                    self.bounded(&mut inner, &name, bound);
                }
                if let Some((name, low, high, inclusive)) = range.clone() {
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
                // **A loop that pushes once per turn adds its count to the
                // length** (ADR-306 D9).
                let filled = match &range {
                    Some((_, Some(low), Some(high), inclusive)) => {
                        let stable = changed
                            .values
                            .iter()
                            .chain(changed.lengths.iter())
                            .all(|c| {
                                !self.arena.mentions(*low, c)
                                    && !self.arena.mentions(*high, c)
                                    && !self.arena.mentions(*low, &length_of(c))
                                    && !self.arena.mentions(*high, &length_of(c))
                            });
                        let lists = match stable {
                            true => filled_once(self.parsed, body, &self.pinned),
                            false => Vec::new(),
                        };
                        let not_empty = self.arena.le(*low, *high);
                        match !lists.is_empty() && self.proves(facts, not_empty) {
                            true => {
                                let one = self.arena.int(i64::from(*inclusive));
                                let span = self.arena.sub(*high, *low);
                                let count = self.arena.add(span, one);
                                lists.into_iter().map(|l| (l, count)).collect()
                            }
                            false => Vec::new(),
                        }
                    }
                    _ => Vec::new(),
                };
                let mut outer = Changed {
                    values: changed.values.clone(),
                    lengths: changed.lengths.clone(),
                };
                for (list, _) in &filled {
                    outer.lengths.remove(list);
                }
                self.after_change(facts, &outer);
                for (list, count) in filled {
                    self.shift(facts, &length_of(&list), count);
                }
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
                let range = match (op, value_lin) {
                    (None, None) => self.range(value, facts),
                    _ => None,
                };
                self.forget(facts, name);
                if self.nonnegative.contains(name) || self.shape.unsigned.contains(name) {
                    self.at_least_zero(facts, name);
                }
                if let Some(range) = range {
                    self.bounded(facts, name, range);
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
                        self.proven.indices.insert(value_node(base));
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
                        self.proven.indices.insert(value_node(base));
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
            // **An operation that stays inside its type** (ADR-306 D6): its
            // operands first, in the order they run.
            Expr::Binary {
                op: BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul,
                lhs,
                rhs,
                span,
                ..
            } => {
                self.expr(lhs, facts);
                self.expr(rhs, facts);
                if !self.collecting
                    && let Some(ty) = self.arithmetic.get(&span.at())
                    && self.fits(expr, ty, facts)
                {
                    self.proven.arithmetic.insert(span.at());
                }
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
                if let Some(at) = writes_a_value(&method, args.len())
                    && let Some(key) = self.shape.elements_key(self.parsed, receiver)
                {
                    self.write(&key, &args[at], facts);
                }
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

/// **The lists a loop's body pushes onto exactly once per turn** (ADR-306
/// D4): one `xs.push(v)` among the body's own statements, nothing else that
/// changes `xs`'s length or hands it on, and no way out of a turn before its
/// end - no `break`, `continue`, `return`, `throw` or `?` anywhere in it.
fn filled_once(parsed: &Parsed, body: &Block, pinned: &HashSet<String>) -> Vec<String> {
    let mut leaves = false;
    visit_stmts(body, &mut |stmt| {
        if matches!(stmt, Stmt::Break | Stmt::Continue | Stmt::Return(_)) {
            leaves = true;
        }
    });
    visit_exprs(body, &mut |expr| {
        if matches!(
            expr,
            Expr::Break | Expr::Continue | Expr::Return(_) | Expr::Throw(_) | Expr::Try(_)
        ) {
            leaves = true;
        }
    });
    if leaves {
        return Vec::new();
    }
    let mut pushed: HashMap<String, usize> = HashMap::new();
    for stmt in &body.stmts {
        if let Stmt::Expr(Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        }) = &stmt.node
            && let (Expr::Variable(list), "push", [_]) =
                (&**receiver, parsed.text(*method), args.as_slice())
        {
            *pushed.entry(parsed.text(*list).to_string()).or_default() += 1;
        }
    }
    pushed
        .into_iter()
        .filter(|(list, count)| {
            *count == 1 && !pinned.contains(list) && {
                let mut changes = 0;
                visit_exprs(body, &mut |expr| match expr {
                    Expr::MethodCall {
                        receiver, method, ..
                    }
                    | Expr::SafeMethod {
                        receiver, method, ..
                    } if matches!(&**receiver, Expr::Variable(n) if parsed.text(*n) == list)
                        && !keeps_length(parsed.text(*method)) =>
                    {
                        changes += 1
                    }
                    Expr::Call { args, .. }
                    | Expr::MethodCall { args, .. }
                    | Expr::SafeMethod { args, .. }
                        if args
                            .iter()
                            .any(|a| matches!(a, Expr::Variable(n) if parsed.text(*n) == list)) =>
                    {
                        changes += 2
                    }
                    _ => {}
                });
                changes == 1 && !rebinds(parsed, body, list)
            }
        })
        .map(|(list, _)| list)
        .collect()
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

// --- interval propagation (ADR-306) ---------------------------------------------

/// A linear term as coefficients by variable and a constant.
type Form = (BTreeMap<String, i128>, i128);

/// What propagation knows of each variable: its least and greatest value.
type Known = HashMap<String, (Option<i128>, Option<i128>)>;

/// `term` as a sum of variables times constants plus a constant, where it is
/// one.
fn linear_form(arena: &Arena, term: TermId) -> Option<Form> {
    use nikaia_logic::Term;
    Some(match arena.get(term) {
        Term::Int(n) => (BTreeMap::new(), i128::from(*n)),
        Term::Var(name) => (BTreeMap::from([(name.clone(), 1)]), 0),
        Term::Add(a, b) | Term::Sub(a, b) => {
            let sign = match arena.get(term) {
                Term::Add(..) => 1,
                _ => -1,
            };
            let (mut vars, k) = linear_form(arena, *a)?;
            let (other, l) = linear_form(arena, *b)?;
            for (name, c) in other {
                *vars.entry(name).or_default() += sign * c;
            }
            (vars, k + sign * l)
        }
        Term::Neg(a) => negated(linear_form(arena, *a)?),
        Term::Mul(a, b) => {
            let (va, ka) = linear_form(arena, *a)?;
            let (vb, kb) = linear_form(arena, *b)?;
            match (va.is_empty(), vb.is_empty()) {
                (true, _) => (vb.into_iter().map(|(n, c)| (n, c * ka)).collect(), ka * kb),
                (_, true) => (va.into_iter().map(|(n, c)| (n, c * kb)).collect(), ka * kb),
                _ => return None,
            }
        }
        _ => return None,
    })
}

fn negated((vars, k): Form) -> Form {
    (vars.into_iter().map(|(n, c)| (n, -c)).collect(), -k)
}

/// The comparisons a fact asserts, each as `form <= 0`.
fn atoms(arena: &Arena, fact: TermId, out: &mut Vec<Form>) {
    use nikaia_logic::Term;
    let diff = |a: TermId, b: TermId| -> Option<Form> {
        let (mut va, ka) = linear_form(arena, a)?;
        let (vb, kb) = linear_form(arena, b)?;
        for (n, c) in vb {
            *va.entry(n).or_default() -= c;
        }
        Some((va, ka - kb))
    };
    match arena.get(fact) {
        Term::And(parts) => parts.iter().for_each(|p| atoms(arena, *p, out)),
        Term::Le(a, b) => out.extend(diff(*a, *b)),
        // Over whole numbers `a < b` is `a - b + 1 <= 0`.
        Term::Lt(a, b) => out.extend(diff(*a, *b).map(|(v, k)| (v, k + 1))),
        Term::Ge(a, b) => out.extend(diff(*a, *b).map(negated)),
        Term::Gt(a, b) => out.extend(diff(*a, *b).map(negated).map(|(v, k)| (v, k + 1))),
        Term::Eq(a, b) => {
            if let Some(d) = diff(*a, *b) {
                out.push(negated(d.clone()));
                out.push(d);
            }
        }
        _ => {}
    }
}

/// **Bounds on each variable the facts imply**, by a few rounds of
/// `c·x <= -k - (the least the other terms can be)`. Sound and not complete;
/// the solver confirms each bound that is used.
fn propagate(arena: &Arena, facts: &[TermId]) -> Known {
    let mut all = Vec::new();
    for fact in facts {
        atoms(arena, *fact, &mut all);
    }
    let mut known = Known::new();
    for _ in 0..4 {
        let mut changed = false;
        for (vars, k) in &all {
            for (x, cx) in vars {
                if *cx == 0 {
                    continue;
                }
                let mut rest = Some(*k);
                for (y, cy) in vars {
                    if y == x {
                        continue;
                    }
                    let (lo, hi) = known.get(y).copied().unwrap_or((None, None));
                    let least = match *cy > 0 {
                        true => lo.map(|l| cy * l),
                        false => hi.map(|h| cy * h),
                    };
                    rest = rest.zip(least).and_then(|(r, l)| r.checked_add(l));
                }
                let Some(rest) = rest else { continue };
                // cx·x <= -rest
                let entry = known.entry(x.clone()).or_insert((None, None));
                if *cx > 0 {
                    let bound = (-rest).div_euclid(*cx);
                    if entry.1.is_none_or(|h| bound < h) {
                        entry.1 = Some(bound);
                        changed = true;
                    }
                } else {
                    // x >= rest / -cx, rounded up.
                    let bound = -((-rest).div_euclid(-cx));
                    if entry.0.is_none_or(|l| bound > l) {
                        entry.0 = Some(bound);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    known
}

/// The interval a linear form takes over what is known of its variables.
fn evaluate(form: &Form, known: &Known) -> Range {
    let (vars, k) = form;
    let (mut lo, mut hi) = (Some(*k), Some(*k));
    for (x, c) in vars {
        let (l, h) = known.get(x).copied().unwrap_or((None, None));
        let (least, most) = match *c >= 0 {
            true => (l.map(|l| c * l), h.map(|h| c * h)),
            false => (h.map(|h| c * h), l.map(|l| c * l)),
        };
        lo = lo.zip(least).and_then(|(a, b)| a.checked_add(b));
        hi = hi.zip(most).and_then(|(a, b)| a.checked_add(b));
    }
    Range {
        lo: lo.and_then(|v| i64::try_from(v).ok()),
        hi: hi.and_then(|v| i64::try_from(v).ok()),
    }
}

// --- what a body is (ADR-306) ------------------------------------------------------

/// **What the walk reads off a body once, before any pass.**
#[derive(Default)]
struct Shape {
    /// Names bound once, to a literal, and never changed: their value.
    constants: HashMap<String, i64>,
    /// Names bound once with an unsigned type: never negative, whatever is
    /// assigned to them, because an assignment that would make one negative
    /// stops the program.
    unsigned: HashSet<String>,
    /// **Lists whose every write the walk sees** (ADR-306 D7): bound once in
    /// the body to a literal, never assigned, handed on or captured, and
    /// changed only by a method that writes what the walk can read
    /// ([`writes_a_value`]) or that only keeps or removes values.
    roots: HashSet<String>,
    /// Names bound once, not `mut`, to a list of [`Self::roots`] or to an
    /// element of one: the key of its elements.
    aliases: HashMap<String, String>,
}

/// Methods that put one value into a list, and which argument it is.
fn writes_a_value(method: &str, args: usize) -> Option<usize> {
    match (method, args) {
        ("push", 1) | ("fill", 1) => Some(0),
        ("insert", 2) | ("resize", 2) => Some(1),
        _ => None,
    }
}

/// Methods that change a list without putting anything new in it.
fn only_removes(method: &str) -> bool {
    matches!(
        method,
        "pop" | "remove" | "swap_remove" | "clear" | "truncate" | "retain" | "dedup"
    )
}

impl Shape {
    fn of(
        parsed: &Parsed,
        args: &[crate::ast::FnArg],
        body: &Block,
        pinned: &HashSet<String>,
        functions: &HashMap<String, Vec<bool>>,
        changing_methods: &HashSet<String>,
    ) -> Shape {
        let text = |s: winnow_grammar::Symbol| parsed.text(s).to_string();
        // How often each name is bound anywhere in the body, lambdas and
        // patterns included: a name bound twice is not one thing.
        let mut bound: HashMap<String, usize> = HashMap::new();
        let mut bind = |name: String| *bound.entry(name).or_default() += 1;
        for arg in args {
            bind(text(arg.name));
        }
        visit_stmts(body, &mut |stmt| match stmt {
            Stmt::Let { names, .. } => names.iter().for_each(|n| bind(text(*n))),
            Stmt::Comptime { name, .. } => bind(text(*name)),
            Stmt::For { bindings, .. } => bindings.iter().for_each(|n| bind(text(*n))),
            _ => {}
        });
        visit_exprs(body, &mut |expr| match expr {
            Expr::Closure { params, .. } => params.iter().for_each(|n| bind(text(*n))),
            Expr::Match { arms, .. } => {
                for arm in arms {
                    pattern_symbols(&arm.pattern, &mut |n| bind(text(n)));
                }
            }
            Expr::Select(arms) => {
                for arm in arms {
                    if let Some(n) = arm.binding {
                        bind(text(n));
                    }
                }
            }
            // A `catch` block names the failure `error`.
            Expr::TryCatch { .. } => bind("error".to_string()),
            _ => {}
        });
        let once = |name: &str| bound.get(name) == Some(&1);

        let mut shape = Shape::default();
        for arg in args {
            let name = text(arg.name);
            if once(&name) && arg.ty.generics.is_empty() && is_unsigned(parsed.text(arg.ty.name)) {
                shape.unsigned.insert(name);
            }
        }
        visit_stmts(body, &mut |stmt| {
            if let Stmt::Let {
                names,
                mutable,
                ty,
                value,
            } = stmt
                && let [only] = names.as_slice()
            {
                let name = text(*only);
                if !once(&name) {
                    return;
                }
                let whole = ty
                    .as_ref()
                    .is_none_or(|t| t.generics.is_empty() && is_whole_number(parsed.text(t.name)));
                if let Some(t) = ty
                    && t.generics.is_empty()
                    && is_unsigned(parsed.text(t.name))
                {
                    shape.unsigned.insert(name.clone());
                }
                if !mutable
                    && whole
                    && let Expr::LitInt { value, negative } = value
                    && let Ok(n) = i64::try_from(crate::ast::int_value(*value, *negative))
                {
                    shape.constants.insert(name.clone(), n);
                }
                if matches!(value, Expr::ListLit { .. }) {
                    shape.roots.insert(name);
                }
            }
        });
        // Aliases, in two rounds: one may name another.
        for _ in 0..2 {
            visit_stmts(body, &mut |stmt| {
                if let Stmt::Let {
                    names,
                    mutable: false,
                    value,
                    ..
                } = stmt
                    && let [only] = names.as_slice()
                    && once(&text(*only))
                    && !shape.roots.contains(&text(*only))
                    && let Some(key) = shape.elements_key(parsed, value)
                {
                    shape.aliases.insert(text(*only), key);
                }
            });
        }

        // **What takes a list out of the walk's sight.**
        let mut lost: HashSet<String> = HashSet::new();
        for name in pinned {
            lost.insert(name.clone());
        }
        // Anything a lambda, a task, an `overlap` branch or a `select` arm
        // names: it may run whenever.
        visit_exprs(body, &mut |expr| {
            let deferred: Vec<Block> = match expr {
                Expr::Closure { body, .. } | Expr::Overlap(body) => vec![body.clone()],
                Expr::Spawn { body, .. } => vec![Block {
                    stmts: vec![Spanned::new(Stmt::Expr((**body).clone()), Span::nowhere())],
                }],
                Expr::Select(arms) => arms.iter().map(|a| a.body.clone()).collect(),
                _ => Vec::new(),
            };
            for block in &deferred {
                visit_exprs(block, &mut |e| {
                    if let Expr::Variable(n) = e {
                        lost.insert(text(*n));
                    }
                });
            }
        });
        visit_stmts(body, &mut |stmt| match stmt {
            Stmt::Assign { target, op, .. } => {
                // `xs = …` replaces the list, `xs.f = …` changes it, and
                // `xs[i] op= v` writes a value the walk does not compute.
                if (op.is_some() || !matches!(target, Expr::Index { .. }))
                    && let Some(root) = root_name(parsed, target)
                {
                    lost.insert(root);
                }
            }
            // A `mut` binding of a list, or of a part of it, may be changed
            // where the walk does not look for it.
            Stmt::Let {
                mutable: true,
                value,
                ..
            } => {
                if let Some(root) = root_name(parsed, value) {
                    lost.insert(root);
                }
            }
            _ => {}
        });
        visit_exprs(body, &mut |expr| match expr {
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
                let method = parsed.text(*method);
                if let Some(root) = root_name(parsed, receiver)
                    && writes_a_value(method, args.len()).is_none()
                    && !only_removes(method)
                    && !keeps_length(method)
                {
                    lost.insert(root);
                }
                for arg in args {
                    if let Some(root) = root_name(parsed, arg)
                        && (changing_methods.contains(method) || !keeps_length(method))
                        && writes_a_value(method, args.len()).is_none()
                    {
                        lost.insert(root);
                    }
                }
            }
            Expr::Call { func, args, .. } => {
                let callee = match &**func {
                    Expr::Variable(name) => functions.get(parsed.text(*name)),
                    _ => None,
                };
                for (at, arg) in args.iter().enumerate() {
                    if let Some(root) = root_name(parsed, arg)
                        && callee.is_none_or(|params| params.get(at).copied().unwrap_or(true))
                    {
                        lost.insert(root);
                    }
                }
            }
            _ => {}
        });
        // A name lost through an alias loses the list it names.
        let through_aliases: Vec<String> = lost
            .iter()
            .filter_map(|n| shape.aliases.get(n))
            .map(|key| key_root(key).to_string())
            .collect();
        lost.extend(through_aliases);
        shape.roots.retain(|r| !lost.contains(r));
        shape
    }

    /// The key of the elements of `expr`, a list: a root's name, an alias's
    /// key, and `key[]` for an element of a list of lists.
    fn elements_key(&self, parsed: &Parsed, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Variable(n) => {
                let n = parsed.text(*n);
                match self.roots.contains(n) {
                    true => Some(n.to_string()),
                    false => self.aliases.get(n).cloned(),
                }
            }
            Expr::Index { base, .. } => self.elements_key(parsed, base).map(|k| format!("{k}[]")),
            _ => None,
        }
    }

    fn tracks(&self, key: &str) -> bool {
        self.roots.contains(key_root(key))
    }
}

/// `watched[][]` is about `watched`.
fn key_root(key: &str) -> &str {
    key.split('[').next().unwrap_or(key)
}

/// The variable an index, a field or a name is rooted in.
fn root_name(parsed: &Parsed, expr: &Expr) -> Option<String> {
    match expr {
        Expr::Variable(n) => Some(parsed.text(*n).to_string()),
        Expr::Index { base, .. } | Expr::Field { base, .. } => root_name(parsed, base),
        _ => None,
    }
}

/// Every symbol a pattern names, bound or not: counting too many makes a
/// name look bound twice, which only costs a proof.
fn pattern_symbols(pattern: &crate::ast::MatchPattern, f: &mut dyn FnMut(winnow_grammar::Symbol)) {
    use crate::ast::MatchPattern;
    match pattern {
        MatchPattern::Path(segments) => segments.iter().for_each(|s| f(*s)),
        MatchPattern::Tuple { parts, .. } => parts.iter().for_each(|p| pattern_symbols(p, f)),
        MatchPattern::Named { bindings, .. } => bindings.iter().for_each(|s| f(*s)),
        MatchPattern::Or(alternatives) => alternatives.iter().for_each(|p| pattern_symbols(p, f)),
        _ => {}
    }
}

fn is_unsigned(ty: &str) -> bool {
    matches!(ty, "u8" | "u16" | "u32" | "u64" | "usize")
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
