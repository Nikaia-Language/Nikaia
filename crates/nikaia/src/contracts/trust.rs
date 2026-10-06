// crates/nikaia/src/contracts/trust.rs
//
// Where a program's bytes came from (ADR-010).
//
// **The adapter of `nikaia-std/src/tools/trust.nika`** (ADR-294 D1, D2): the
// module's decisions - which written root is a way around the root check, and
// what `--trust` says - are Nikaia. What stays here is what it reads, which is
// the compiler's: the call walk and the ledger, handed over as names, `bool`s
// and line numbers.
//
// `Trusted ⊑ Untrusted`, joined in the safe direction: one untrusted input
// makes the result untrusted. The sources are `std`'s, stated in its ledger
// (ADR-020) - a file the operator named, a pipe they connected, the arguments
// they typed - and a program's own trust is the join over the ones it calls.
//
// **The program's join, and each map's.** `analyse` joins over every source
// the program reads: where none is untrusted, every map keeps the fast hash.
// Where one is, a map is fast only where `trusted_maps` follows everything it
// is keyed by to a source that is trusted (#431, ADR-010 D1: trust is a
// container's). A program holds several inputs - files, sockets, arguments -
// and one untrusted among them no longer slows the maps the others fill.
//
// **A barrier widens to untrusted, never the other way** (ADR-010 D1). A call
// this compiler cannot resolve could be anything, so a program that reaches one
// whose result it then treats as input has no proof of who chose those bytes.
// What Stage 0 can reach is `std`, which states all of its sources, so today
// there is no such call - and `Reason::Unresolved` is what will carry it when
// there is.

use crate::ast::Span;
use crate::contracts::LedgerOps;
use crate::parser::Parsed;

use super::{Ledger, Provenance};
use crate::contracts::SignatureOps;

/// What the analysis concluded, and what it concluded it from.
#[derive(Debug, Clone)]
pub struct Trust {
    pub provenance: Provenance,
    /// Every source the program calls, in the order they were found, with what
    /// each contributed. This is what `--trust` prints: the choice is visible,
    /// never a mystery (ADR-010 D7).
    pub reasons: Vec<Reason>,
    /// Every path call whose root is a **word** rather than a directory
    /// ([ADR-108](../../../docs/specification/adr/adr-108.md) D4).
    ///
    /// `Anywhere` is the one way around the root check, and the security review
    /// of a program's file access is this list. A `Dir` whose root is the literal
    /// `"/"` is in it too, because that is the same thing in another spelling.
    pub roots: Vec<Root>,
}

#[derive(Debug, Clone)]
pub struct Reason {
    /// The source, as the ledger names it.
    pub source: String,
    pub provenance: Provenance,
}

/// One call that wrote a root the review wants to see
/// ([ADR-108](../../../docs/specification/adr/adr-108.md) D4).
#[derive(Debug, Clone)]
pub struct Root {
    /// The entry, as the ledger names it.
    pub entry: String,
    pub wrote: Wrote,
    /// The statement the call stands in, which is the granularity every other
    /// report here uses.
    pub span: Span,
}

/// Which of the two spellings D4 lists a root is, declared in Nikaia
/// (`nikaia-std/src/tools/trust.nika`).
pub use nikaia_std::tools::trust::Wrote;

/// What `--trust` names where a method call could not be resolved, so the
/// program has no proof of who chose the bytes it read.
pub const UNRESOLVED: &str = "a method call this compiler could not resolve";

/// The provenance of this program's input.
pub fn analyse(parsed: &Parsed, library: &Ledger) -> Trust {
    let mut reasons: Vec<Reason> = Vec::new();
    let mut roots: Vec<Root> = Vec::new();

    // **The walk is Nikaia** (`tools/foreign.nika`, #125): every call by name
    // a body makes, with what it hands a parameter called `root` where the
    // callee has one.
    let root_at = |name: &str| -> i64 {
        library
            .lookup(name)
            .and_then(|(_, contract)| contract.signature.as_ref())
            .and_then(|s| s.arguments().iter().position(|(param, _)| param == "root"))
            .map_or(-1, |at| at as i64)
    };
    for seen in crate::foreign::seen(parsed, &root_at) {
        let crate::foreign::Seen::Call { name, wrote, span } = seen else {
            continue;
        };
        let Some((key, contract)) = library.lookup(&name) else {
            continue;
        };
        if let Some(provenance) = contract.provenance
            && !reasons.iter().any(|r| r.source == key)
        {
            reasons.push(Reason {
                source: key.clone(),
                provenance,
            });
        }
        if let Some(wrote) = wrote {
            roots.push(Root {
                entry: key,
                wrote,
                span,
            });
        }
    }

    // **A source read through a method** (#472, ADR-288): `c.read()` is
    // `net::Connection::read` once the receiver's type is known, which only
    // the checker knows. The ledger's own pass asks it and records each
    // function's resolved keys.
    //
    // **And one it could not resolve counts as untrusted** (ADR-010 D1: a
    // barrier widens, never the other way). A map that gets the keyed hash
    // needlessly is slower; the opposite is an attack.
    let (_, checked) = Ledger::infer_package_checked(&[parsed], library);
    let mut unresolved = false;
    for calls in checked.iter().flat_map(|c| c.methods.values()) {
        unresolved |= calls.unresolved;
        for key in &calls.resolved {
            let Some((key, contract)) = library.lookup(key) else {
                continue;
            };
            if let Some(provenance) = contract.provenance
                && !reasons.iter().any(|r| r.source == key)
            {
                reasons.push(Reason {
                    source: key,
                    provenance,
                });
            }
        }
    }
    if unresolved {
        reasons.push(Reason {
            source: UNRESOLVED.to_string(),
            provenance: Provenance::Untrusted,
        });
    }

    reasons.sort_by(|a, b| a.source.cmp(&b.source));
    roots.sort_by_key(|r| r.span.at());

    // A program that reads nothing has no input to distrust. Its maps are keyed
    // by what it wrote itself, which is the compiled-in case ADR-010 D2 calls
    // trusted.
    let provenance = reasons
        .iter()
        .map(|r| r.provenance)
        .fold(Provenance::Trusted, Provenance::join);

    Trust {
        provenance,
        reasons,
        roots,
    }
}

/// What `nikaia --explain --trust` prints
/// ([ADR-010](../../../docs/specification/adr/adr-010.md) D7, and
/// [ADR-108](../../../docs/specification/adr/adr-108.md) D4's listing).
///
/// **The file and its text, because a site is a line.** Provenance is a
/// property of the program and wanted neither; a root is written somewhere, and
/// *the security review of a program's file access is that list* — which a list
/// without line numbers is not.
pub fn render(trust: &Trust, path: &str, source: &str) -> String {
    use nikaia_std::tools::trust as nika;
    let reasons: Vec<nika::Reason> = trust
        .reasons
        .iter()
        .map(|r| nika::Reason {
            source: r.source.clone(),
            untrusted: r.provenance == Provenance::Untrusted,
        })
        .collect();
    // **A site is a line and a column**, counted here where the source is:
    // what crosses into Nikaia is two numbers.
    let places: Vec<nika::Place> = trust
        .roots
        .iter()
        .map(|root| {
            let (line, column) = winnow_grammar::span::line_column(source, root.span.at());
            nika::Place {
                line: line as i64,
                column: column as i64,
                entry: root.entry.clone(),
                wrote: root.wrote,
            }
        })
        .collect();
    nika::render(
        trust.provenance == Provenance::Untrusted,
        &reasons,
        &places,
        path,
    )
}

/// **The maps that keep the fast hash in a program that reads untrusted input**
/// ([ADR-010](../../../docs/specification/adr/adr-010.md) D1, #431): trust is
/// a container's, the join of what goes into it, and not the program's.
///
/// By the `let` that makes each one, by the byte its statement starts at. A map
/// is one only where all of this is seen:
///
/// * it is made by that `let` (`let m = collections::HashMap()`) and its name
///   is bound nowhere else in the function;
/// * it never leaves the function: every use of the name is a method call on
///   it other than `clone`, an index into it, or the iterable of a `for` - so
///   its type is nowhere another function or a field names;
/// * every argument of every method call on it, and every index into it, is
///   made only of what the program wrote itself and of what a trusted source
///   handed back, each name followed to the binding it reads.
///
/// **Everything not seen is untrusted** (D1, a barrier widens): a parameter, a
/// pattern's binding, a function of the program's own, a method that is a
/// source somewhere in `std` by its name, and any shape this does not read. A
/// map that gets the keyed hash needlessly is slower; the opposite is an
/// attack.
pub fn trusted_maps(parsed: &Parsed, library: &Ledger) -> std::collections::BTreeSet<usize> {
    use crate::ast::Item;
    use std::collections::BTreeSet;

    // A method is a source where `std` names one by that name, whatever it is
    // called on.
    let sources: BTreeSet<String> = library
        .functions
        .iter()
        .filter(|(_, c)| c.provenance == Some(Provenance::Untrusted))
        .filter_map(|(key, _)| key.rsplit("::").next().map(str::to_string))
        .collect();
    let mut own: BTreeSet<String> = BTreeSet::new();
    let mut bodies = Vec::new();
    for item in &parsed.program.items {
        match &item.node {
            Item::Fn {
                name: Some(name),
                args,
                body,
                ..
            } => {
                own.insert(parsed.text(*name).to_string());
                bodies.push((args, body));
            }
            Item::Impl { methods, .. } => {
                for method in methods {
                    if let Item::Fn {
                        name: Some(name),
                        args,
                        body,
                        ..
                    } = &method.node
                    {
                        own.insert(parsed.text(*name).to_string());
                        bodies.push((args, body));
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    for (args, body) in bodies {
        let mut walk = MapWalk {
            parsed,
            library,
            sources: &sources,
            own: &own,
            scopes: vec![
                args.iter()
                    .map(|a| (parsed.text(a.name).to_string(), usize::MAX))
                    .collect(),
            ],
            given: Vec::new(),
            made: Vec::new(),
            kept: Vec::new(),
            escapes: BTreeSet::new(),
            bound: std::collections::BTreeMap::new(),
            unread: false,
            walked: BTreeSet::new(),
        };
        walk.block(body);
        if walk.made.is_empty() || walk.unread {
            continue;
        }
        let trusted = walk.trusted();
        for (at, name, map) in &walk.made {
            // Bound once, so every use of the name in the body is this map.
            if walk.bound.get(name).copied() != Some(1) {
                continue;
            }
            // **Any other use is the map leaving**: handed on, bound again,
            // returned, assigned.
            if walk.escapes.contains(map) {
                continue;
            }
            let on_it: Vec<&Option<Vec<usize>>> = walk
                .kept
                .iter()
                .filter(|(m, _)| m == map)
                .map(|(_, deps)| deps)
                .collect();
            let keys_trusted = on_it.iter().all(|deps| match deps {
                Some(deps) => deps.iter().all(|d| trusted.contains(d)),
                None => false,
            });
            if keys_trusted {
                out.insert(*at);
            }
        }
    }
    out
}

/// One function's bindings, followed lexically ([`trusted_maps`]).
struct MapWalk<'a> {
    parsed: &'a Parsed,
    library: &'a Ledger,
    sources: &'a std::collections::BTreeSet<String>,
    own: &'a std::collections::BTreeSet<String>,
    /// Each block's names, innermost last, to the binding they read;
    /// `usize::MAX` is one never trusted (a parameter, a pattern's binding).
    scopes: Vec<std::collections::BTreeMap<String, usize>>,
    /// What each binding is given: the bindings each value reads, or nothing
    /// for a value that is never trusted.
    given: Vec<Vec<Option<Vec<usize>>>>,
    /// The `let`s that make a map: the statement, the name, the binding.
    made: Vec<(usize, String, usize)>,
    /// Each use of a map this reads, by its binding, with what the use hands
    /// it (one entry per use; the iterable of a `for` hands it nothing).
    kept: Vec<(usize, Option<Vec<usize>>)>,
    /// The bindings whose name stands somewhere else: an argument, a value
    /// bound or returned, an assignment.
    escapes: std::collections::BTreeSet<usize>,
    /// How often each name is bound in the body.
    bound: std::collections::BTreeMap<String, usize>,
    unread: bool,
    walked: std::collections::BTreeSet<usize>,
}

impl<'a> MapWalk<'a> {
    fn bind(&mut self, name: &str, given: Option<Vec<usize>>) -> usize {
        let id = self.given.len();
        self.given.push(vec![given]);
        *self.bound.entry(name.to_string()).or_default() += 1;
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), id);
        }
        id
    }

    fn opaque(&mut self, name: &str) {
        *self.bound.entry(name.to_string()).or_default() += 1;
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), usize::MAX);
        }
    }

    fn resolve(&self, name: &str) -> Option<usize> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn block(&mut self, block: &'a crate::ast::Block) {
        use crate::ast::{Expr, Stmt};
        if !self
            .walked
            .insert(block as *const crate::ast::Block as usize)
        {
            return;
        }
        self.scopes.push(Default::default());
        for stmt in &block.stmts {
            match &stmt.node {
                Stmt::Let { names, value, .. } => {
                    self.expr(value);
                    let given = self.deps(value);
                    match names.as_slice() {
                        [one] => {
                            let name = self.parsed.text(*one).to_string();
                            let id = self.bind(&name, given);
                            if is_a_map_made(self.parsed, value) {
                                self.made.push((stmt.span.at(), name, id));
                            }
                        }
                        many => {
                            for name in many {
                                self.opaque(self.parsed.text(*name));
                            }
                        }
                    }
                }
                Stmt::Assign { target, value, .. } => {
                    self.expr(target);
                    self.expr(value);
                    if let Expr::Variable(name) = target {
                        let given = self.deps(value);
                        match self.resolve(self.parsed.text(*name)) {
                            Some(id) if id != usize::MAX => self.given[id].push(given),
                            _ => {}
                        }
                    }
                }
                Stmt::For {
                    bindings,
                    iter,
                    body,
                } => {
                    match iter {
                        Expr::Variable(name) => {
                            if let Some(id) = self.resolve(self.parsed.text(*name))
                                && id != usize::MAX
                            {
                                self.kept.push((id, Some(Vec::new())));
                            }
                        }
                        _ => self.expr(iter),
                    }
                    let given = self.deps(iter);
                    self.scopes.push(Default::default());
                    for name in bindings {
                        self.bind(self.parsed.text(*name), given.clone());
                    }
                    self.block(body);
                    self.scopes.pop();
                }
                Stmt::While { cond, body } => {
                    self.expr(cond);
                    self.block(body);
                }
                Stmt::Expr(value) => self.expr(value),
                Stmt::Return(value) => {
                    if let Some(value) = value {
                        self.expr(value);
                    }
                }
                _ => self.unread = true,
            }
        }
        self.scopes.pop();
    }

    /// An expression where it stands: the uses of a map in it, read with the
    /// names bound here, and the blocks inside it walked with their own.
    fn expr(&mut self, expr: &'a crate::ast::Expr) {
        use crate::ast::{Expr, FPart};
        match expr {
            Expr::Block(b) | Expr::Unsafe(b) => self.block(b),
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(cond);
                self.block(then_branch);
                if let Some(b) = else_branch {
                    self.block(b);
                }
            }
            Expr::MethodCall {
                receiver,
                method,
                args,
                config,
            } => {
                if let Expr::Variable(name) = receiver.as_ref()
                    && self.parsed.text(*method) != "clone"
                    && let Some(id) = self.resolve(self.parsed.text(*name))
                    && id != usize::MAX
                {
                    let mut all = Some(Vec::new());
                    for a in args {
                        all = join(all, self.deps(a));
                    }
                    self.kept.push((id, all));
                } else {
                    self.expr(receiver);
                }
                args.iter().for_each(|a| self.expr(a));
                config.iter().for_each(|c| self.expr(&c.value));
            }
            Expr::Index { base, index } => {
                if let Expr::Variable(name) = base.as_ref()
                    && let Some(id) = self.resolve(self.parsed.text(*name))
                    && id != usize::MAX
                {
                    let deps = self.deps(index);
                    self.kept.push((id, deps));
                } else {
                    self.expr(base);
                }
                self.expr(index);
            }
            Expr::Call { func, args, config } => {
                self.expr(func);
                args.iter().for_each(|a| self.expr(a));
                config.iter().for_each(|c| self.expr(&c.value));
            }
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::Unary { expr, .. }
            | Expr::Cast { expr, .. }
            | Expr::Try(expr)
            | Expr::Throw(expr) => self.expr(expr),
            Expr::Field { base, .. } | Expr::SafeField { base, .. } => self.expr(base),
            Expr::Coalesce { value, fallback } => {
                self.expr(value);
                self.expr(fallback);
            }
            Expr::Tuple(items) => items.iter().for_each(|i| self.expr(i)),
            Expr::ListLit { items, .. } => items.iter().for_each(|i| self.expr(i)),
            Expr::LitInterpolated { parts, .. } => {
                for part in parts {
                    if let FPart::Hole { expr, .. } = part {
                        self.expr(expr);
                    }
                }
            }
            Expr::Return(value) => {
                if let Some(value) = value.as_ref() {
                    self.expr(value);
                }
            }
            Expr::Variable(name) => {
                if let Some(id) = self.resolve(self.parsed.text(*name))
                    && id != usize::MAX
                {
                    self.escapes.insert(id);
                }
            }
            Expr::Path(_)
            | Expr::LitInt { .. }
            | Expr::LitFloat(_)
            | Expr::LitStr { .. }
            | Expr::LitChar(_)
            | Expr::LitBool(_)
            | Expr::LitNull
            | Expr::Break
            | Expr::Continue => {}
            // A lambda, a `match`, a `catch`, a task: names bound some way this
            // does not follow.
            _ => self.unread = true,
        }
    }

    /// The bindings a value is made of, or nothing where something in it is
    /// never trusted.
    fn deps(&self, value: &crate::ast::Expr) -> Option<Vec<usize>> {
        use crate::ast::{Expr, FPart};
        let all = |xs: &[Expr]| {
            xs.iter()
                .try_fold(Vec::new(), |acc, x| join(Some(acc), self.deps(x)))
        };
        match value {
            Expr::LitInt { .. }
            | Expr::LitFloat(_)
            | Expr::LitStr { .. }
            | Expr::LitChar(_)
            | Expr::LitBool(_)
            | Expr::LitNull => Some(Vec::new()),
            Expr::Variable(name) => match self.resolve(self.parsed.text(*name)) {
                Some(id) if id != usize::MAX => Some(vec![id]),
                _ => None,
            },
            Expr::LitInterpolated { parts, .. } => {
                parts.iter().try_fold(Vec::new(), |acc, p| match p {
                    FPart::Text(_) => Some(acc),
                    FPart::Hole { expr, .. } => join(Some(acc), self.deps(expr)),
                })
            }
            Expr::Tuple(items) => all(items),
            Expr::ListLit { items, .. } => all(items),
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => self.deps(expr),
            Expr::Binary { lhs, rhs, .. } => join(self.deps(lhs), self.deps(rhs)),
            Expr::Field { base, .. } => self.deps(base),
            Expr::Index { base, index } => join(self.deps(base), self.deps(index)),
            Expr::Coalesce { value, fallback } => join(self.deps(value), self.deps(fallback)),
            Expr::Call { func, args, config } => {
                let name = match func.as_ref() {
                    Expr::Path(segments) => segments
                        .iter()
                        .map(|s| self.parsed.text(*s))
                        .collect::<Vec<_>>()
                        .join("::"),
                    Expr::Variable(name) => self.parsed.text(*name).to_string(),
                    _ => return None,
                };
                if self.own.contains(&name) {
                    return None;
                }
                let (_, contract) = self.library.lookup(&name)?;
                match contract.provenance {
                    Some(Provenance::Trusted) => Some(Vec::new()),
                    Some(Provenance::Untrusted) => None,
                    None => config
                        .iter()
                        .fold(all(args), |acc, c| join(acc, self.deps(&c.value))),
                }
            }
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                let name = self.parsed.text(*method);
                if self.sources.contains(name) || self.own.contains(name) {
                    return None;
                }
                join(self.deps(receiver), all(args))
            }
            _ => None,
        }
    }

    /// Which bindings are trusted: the greatest answer that holds, so a name
    /// that feeds itself (`x = x + 1`) is as trusted as what else it is given.
    fn trusted(&self) -> std::collections::BTreeSet<usize> {
        let mut trusted: std::collections::BTreeSet<usize> = (0..self.given.len())
            .filter(|id| self.given[*id].iter().all(Option::is_some))
            .collect();
        loop {
            let now = trusted.clone();
            trusted.retain(|id| {
                self.given[*id].iter().all(|g| {
                    g.as_ref()
                        .is_some_and(|deps| deps.iter().all(|d| now.contains(d)))
                })
            });
            if trusted.len() == now.len() {
                return trusted;
            }
        }
    }
}

fn join(a: Option<Vec<usize>>, b: Option<Vec<usize>>) -> Option<Vec<usize>> {
    let mut a = a?;
    a.extend(b?);
    Some(a)
}

fn is_a_map_made(parsed: &Parsed, value: &crate::ast::Expr) -> bool {
    use crate::ast::Expr;
    let Expr::Call { func, args, .. } = value else {
        return false;
    };
    let last = match func.as_ref() {
        Expr::Path(segments) => segments.last().map(|s| parsed.text(*s)),
        Expr::Variable(name) => Some(parsed.text(*name)),
        _ => None,
    };
    args.is_empty() && matches!(last, Some("HashMap" | "HashSet"))
}
