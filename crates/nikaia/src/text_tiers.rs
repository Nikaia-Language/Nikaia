//! **What a declared `String` is below, decided by what flows into it**
//! ([ADR-222](../../../docs/specification/adr/adr-222.md),
//! [ADR-223](../../../docs/specification/adr/adr-223.md)).
//!
//! [ADR-107](../../../docs/specification/adr/adr-107.md) D1 made `String` the
//! one text type and its state - borrowed, tethered, owned - the compiler's,
//! *the cheapest state that works, chosen per use*. Below, a declaration has
//! one representation, so for a declared `String` "per use" means: by every
//! value any line of the package puts there. A **position** is every place a
//! program declares `String`: a struct's field, a function's result, a
//! function's parameter, an annotated `let` - and inside each, the element of
//! a list or a set and the key or value of a map (`Vec[String]`,
//! `HashMap[String, i64]`). Three tiers, and each pays only for itself:
//!
//! * **only text of its own** flows in - the ordinary case: it stays `String`,
//!   and nothing about it changes;
//! * **only views** (and literals, which are views of text that lives as long
//!   as the program): it becomes `ref String`, exactly as if the program had
//!   written that, and [ADR-209](../../../docs/specification/adr/adr-209.md)
//!   decides where each buffer lives - in the caller's frame for nothing, with
//!   a handle only where no frame outlives it;
//! * **both**: it becomes `ref String` marked [`crate::ast::Type::either`],
//!   which is `nikaia_std::either_text::EitherText` below. A view is borrowed
//!   and text of its own is owned, **each at its own line**, so no line pays for
//!   another line's kind of text.
//!
//! A literal alone moves nothing: a position only literals and owned text flow
//! into stays `String`, and the literal is built where it stands as before
//! ([ADR-207](../../../docs/specification/adr/adr-207.md) D2).
//!
//! **What is published may become a view but never both kinds**: a `pub` field
//! of a `pub` struct, and a `pub` function's parameters and result. Their
//! representation leaves with the package, before the programs that use it
//! exist; a view the package itself puts there is in its ledger as
//! `ref String`, and a mixed one would need a type no other package can know
//! to build. There the view is refused as before, saying why
//! ([ADR-208](../../../docs/specification/adr/adr-208.md) D2).
//!
//! Run once, right after parsing, so that the checker, every derived column,
//! the ledger and the emitter all read one answer off the types.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Block, Expr, Item, Stmt, Type};
use crate::parser::Parsed;

/// What a value of text is, as far as where it lives goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    /// Text of its own: `f"…"`, `.clone()`, a `String` handed over.
    Owned,
    /// A view into text something else owns.
    View,
    /// A literal, or a view of one: lives as long as the program.
    Static,
    /// What a mixed position hands back: either of the first two, per value.
    Either,
}

type Kinds = BTreeSet<Kind>;

fn one(kind: Kind) -> Kinds {
    BTreeSet::from([kind])
}

/// What a set of kinds makes of a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Owned,
    View,
    Mixed,
}

fn tier(kinds: &Kinds) -> Tier {
    let viewed = kinds.contains(&Kind::View);
    let owned = kinds.contains(&Kind::Owned);
    match (kinds.contains(&Kind::Either), viewed, owned) {
        (true, _, _) | (false, true, true) => Tier::Mixed,
        (false, true, false) => Tier::View,
        _ => Tier::Owned,
    }
}

/// Methods whose result is text of its own, whatever they are called on.
const OWNS: &[&str] = &[
    "clone",
    "to_owned",
    "to_string",
    "clone_text",
    "to_uppercase",
    "to_lowercase",
    "repeat",
    "replace",
    "join",
    "concat",
];

/// Methods of text whose result is a view **of the text they are called on**.
const VIEWS: &[&str] = &[
    "trim",
    "trim_start",
    "trim_end",
    "trim_matches",
    "trim_start_matches",
    "trim_end_matches",
    "strip_prefix",
    "strip_suffix",
    "lines",
    "split",
    "split_whitespace",
    "splitn",
    "rsplit",
    "rsplitn",
    "split_terminator",
    "matches",
    "as_str",
];

/// Methods whose result is an element of what they are called on, so it is
/// the same kind of text as the elements are.
const ELEMENTS: &[&str] = &[
    "collect", "next", "unwrap", "first", "last", "get", "iter", "nth", "peek", "pop", "remove",
    "values", "keys",
];

/// Containers whose element is a position: a run or a set of one, and a map
/// whose key and value are two.
const RUNS: &[&str] = &["Vec", "List", "VecDeque", "Deque"];
const SETS: &[&str] = &["HashSet", "BTreeSet", "Set"];
const MAPS: &[&str] = &["HashMap", "BTreeMap", "Map"];

/// Methods that put their argument into a run or a set; `insert` on a map puts
/// a key and a value.
const PUTS: &[&str] = &[
    "push",
    "push_back",
    "push_front",
    "insert",
    "extend",
    "append",
];

/// Who declares a position.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Owner {
    Field(String, String),
    Result(String),
    Param(String, usize),
    /// An annotated `let`, by the byte its statement starts at.
    Let(usize),
}

/// A declared `String`: who declares it, and where inside the declared type -
/// the argument indices from the outside in (`HashMap[String, i64]`'s key is
/// `[0]`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    owner: Owner,
    path: Vec<usize>,
}

/// A declared `String`, or `String?`: a value that may be absent is a position
/// as well, and `null` puts no text of either kind into it (ADR-224 D1).
fn plain_string(parsed: &Parsed, ty: &Type) -> bool {
    parsed.text(ty.name) == "String"
        && ty.generics.is_empty()
        && !ty.is_view
        && !ty.is_tuple
        && !ty.is_slice
        && ty.code.is_none()
}

fn container(parsed: &Parsed, ty: &Type) -> Option<&'static str> {
    if ty.is_view || ty.is_nullable || ty.is_tuple || ty.code.is_some() {
        return None;
    }
    let name = parsed.text(ty.name);
    let name = name.rsplit("::").next().unwrap_or(name);
    [RUNS, SETS, MAPS]
        .iter()
        .flat_map(|list| list.iter())
        .find(|n| **n == name)
        .copied()
}

/// A function as this pass needs it.
#[derive(Debug, Clone)]
struct FnDecl {
    params: Vec<Type>,
    result: Option<Type>,
    published: bool,
}

/// What the program declares, read once.
struct Declared<'p> {
    parsed: &'p Parsed,
    /// Every field of every struct, and whether it is published.
    fields: BTreeMap<(String, String), (Type, bool)>,
    /// Every function and method, by key.
    fns: BTreeMap<String, FnDecl>,
    /// Method names declared in this program, to the keys that declare them.
    methods: BTreeMap<String, Vec<String>>,
}

impl<'p> Declared<'p> {
    fn of(parsed: &'p Parsed) -> Declared<'p> {
        let mut declared = Declared {
            parsed,
            fields: BTreeMap::new(),
            fns: BTreeMap::new(),
            methods: BTreeMap::new(),
        };
        for item in &parsed.program.items {
            match &item.node {
                Item::Struct {
                    name,
                    fields,
                    is_public,
                    ..
                } => {
                    let owner = parsed.text(*name).to_string();
                    for field in fields {
                        declared.fields.insert(
                            (owner.clone(), parsed.text(field.name).to_string()),
                            (field.ty.clone(), *is_public && field.is_public),
                        );
                    }
                }
                Item::Fn { .. } => declared.function(&item.node, None),
                Item::Impl {
                    target, methods, ..
                } => {
                    let target = parsed.text(target.name).to_string();
                    for method in methods {
                        declared.function(&method.node, Some(&target));
                    }
                }
                _ => {}
            }
        }
        declared
    }

    fn function(&mut self, item: &Item, target: Option<&str>) {
        let Item::Fn {
            name,
            args,
            ret_type,
            is_public,
            ..
        } = item
        else {
            return;
        };
        let own = name
            .map(|n| self.parsed.text(n).to_string())
            .unwrap_or_else(|| "new".to_string());
        let key = key(target, &own);
        if target.is_some() {
            self.methods.entry(own).or_default().push(key.clone());
        }
        self.fns.insert(
            key,
            FnDecl {
                params: args.iter().map(|a| a.ty.clone()).collect(),
                result: ret_type.clone(),
                published: *is_public,
            },
        );
    }

    /// Whether what an owner declares is published.
    fn published(&self, owner: &Owner) -> bool {
        match owner {
            Owner::Field(s, f) => self
                .fields
                .get(&(s.clone(), f.clone()))
                .is_some_and(|(_, p)| *p),
            Owner::Result(key) | Owner::Param(key, _) => {
                self.fns.get(key).is_some_and(|f| f.published)
            }
            Owner::Let(_) => false,
        }
    }

    /// A written `String`, to declare a binding with.
    fn string_type(&self) -> Option<Type> {
        let string = |ty: &Type| plain_string(self.parsed, ty).then(|| ty.clone());
        self.fields
            .values()
            .find_map(|(t, _)| string(t))
            .or_else(|| {
                self.fns
                    .values()
                    .flat_map(|f| f.params.iter().chain(f.result.iter()))
                    .find_map(string)
            })
    }
}

fn key(target: Option<&str>, own: &str) -> String {
    match target {
        Some(target) => format!("{target}::{own}"),
        None => own.to_string(),
    }
}

/// The tiers decided so far, by position.
type Tiers = BTreeMap<Position, Tier>;

/// What one walk found.
#[derive(Debug, Default)]
struct Flows {
    positions: BTreeMap<Position, Kinds>,
    /// A `let` binding a name to a literal, by the byte it stands at, and the
    /// positions that name is kept in (ADR-222 D4).
    literal_lets: BTreeMap<usize, BTreeSet<Position>>,
    /// Values that go into a mixed position at a line whose lowering does not
    /// wrap them itself, by address, and how each is handed over.
    wraps: BTreeMap<usize, Hand>,
    /// The same inside an `f"…"` hole, by the hole's text and the value's
    /// shape (ADR-229 D1).
    hole_wraps: BTreeMap<String, BTreeSet<(String, Hand)>>,
    /// Two positions a whole value goes between - a list handed back, handed
    /// over, or bound - which are therefore one representation below, and get
    /// one tier (ADR-224 D3).
    links: BTreeSet<(Position, Position)>,
    /// Positions a value goes into from inside an `f"…"` hole, where it
    /// cannot be wrapped: the hole is parsed out of the literal again by every
    /// reader, so this pass has no expression there to change. Such a
    /// position is never mixed - it stays text of its own, and the view is
    /// refused with its explanation rather than handed to `rustc` unwrapped.
    unwrappable: BTreeSet<Position>,
}

/// How a value is handed into a mixed position: the method this pass writes
/// after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Hand {
    /// `value.into_either()`: into a `String`.
    Either,
    /// `value.into_either_maybe()`: into a `String?`, present or absent.
    Maybe,
    /// `iterator.either_items()`: before a `collect` into a list whose
    /// element is mixed, each item handed over as it is.
    Items,
    /// `pairs.either_keys()`, `either_values()`, `either_pairs()`: before a
    /// `collect` into a map whose key, value or both are mixed
    /// ([ADR-227](../../docs/specification/adr/adr-227.md) D2).
    Keys,
    Values,
    Pairs,
}

/// One local, as far as this walk needs it.
#[derive(Debug, Clone, Default)]
struct Local {
    kinds: Kinds,
    /// The struct it is a value of, where that can be read off the program.
    of: Option<String>,
    /// Bound by a `let` to a literal and nothing else: the byte the `let`
    /// stands at.
    literal: Option<usize>,
    /// The position it is declared as, and the declared type.
    declared: Option<(Owner, Type)>,
}

struct Walk<'d, 'p> {
    declared: &'d Declared<'p>,
    tiers: &'d Tiers,
    flows: &'d mut Flows,
    scopes: Vec<BTreeMap<String, Local>>,
    /// The function being walked, where its result is a position.
    result: Option<String>,
    /// Inside an `f"…"` hole (see [`Flows::unwrappable`]).
    in_hole: bool,
    /// The text of the hole being walked, where it is one this pass can hand
    /// a value over in (ADR-229 D1).
    hole_text: Option<String>,
}

impl Walk<'_, '_> {
    fn text(&self, sym: winnow_grammar::Symbol) -> &str {
        self.declared.parsed.text(sym)
    }

    fn bind(&mut self, name: String, local: Local) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, local);
        }
    }

    fn local(&self, name: &str) -> Option<&Local> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn tier_of(&self, position: &Position) -> Tier {
        self.tiers.get(position).copied().unwrap_or(Tier::Owned)
    }

    /// Record that values of these kinds go into every declared `String` of
    /// `ty`: the type itself, or an element of it.
    fn flow_type(&mut self, owner: &Owner, ty: &Type, path: &mut Vec<usize>, kinds: &Kinds) {
        let parsed = self.declared.parsed;
        if plain_string(parsed, ty) {
            let position = Position {
                owner: owner.clone(),
                path: path.clone(),
            };
            self.flows
                .positions
                .entry(position)
                .or_default()
                .extend(kinds.iter().copied());
            return;
        }
        if container(parsed, ty).is_some() {
            for (i, arg) in ty.generics.iter().enumerate() {
                path.push(i);
                self.flow_type(owner, arg, path, kinds);
                path.pop();
            }
        }
    }

    /// A value going into one argument of a declared type, which is a
    /// position if it is `String`: recorded, and - where that position is
    /// mixed and `wrap` says the line is this pass's to wrap - marked for
    /// `.into_either()`.
    fn flow_value(
        &mut self,
        owner: &Owner,
        ty: &Type,
        path: Vec<usize>,
        value: Option<&Expr>,
        kinds: &Kinds,
        wrap: bool,
    ) {
        let mut path = path;
        // A list that comes whole from a position is linked to it below
        // (`flow_whole`), and the link is all that flows: what it reads as
        // is only what this walk has decided so far, and linked positions
        // share what flows into either, so an early reading would stay.
        let linked = container(self.declared.parsed, ty).is_some()
            && value.is_some_and(|v| {
                let collect = matches!(v, Expr::MethodCall { method, args, .. }
                    if args.is_empty() && self.text(*method) == "collect");
                !collect && !matches!(v, Expr::ListLit { .. }) && self.source(v).is_some()
            });
        if !linked {
            self.flow_type(owner, ty, &mut path, kinds);
        }
        if self.in_hole
            && self.hole_text.is_none()
            && wrap
            && plain_string(self.declared.parsed, ty)
        {
            self.flows.unwrappable.insert(Position {
                owner: owner.clone(),
                path: path.clone(),
            });
            return;
        }
        if let Some(value) = value {
            // `null` is no text of either kind, and goes in as it is.
            if plain_string(self.declared.parsed, ty)
                && wrap
                && !matches!(value, Expr::LitNull)
                && self.tier_of(&Position {
                    owner: owner.clone(),
                    path: path.clone(),
                }) == Tier::Mixed
            {
                let hand = match ty.is_nullable {
                    true => Hand::Maybe,
                    false => Hand::Either,
                };
                self.hand_over(value, hand);
            }
            self.kept_literal(value, owner, &path, ty);
            if container(self.declared.parsed, ty).is_some() {
                self.flow_whole(owner, ty, path, value);
            }
        }
    }

    /// **A container going in whole** (ADR-224 D3): a literal's items one by
    /// one, a `collect` item by item, and anything that already has a
    /// position - a list handed back, handed over or bound - linked to it, so
    /// that the two are one representation below and nothing is converted.
    fn flow_whole(&mut self, owner: &Owner, ty: &Type, path: Vec<usize>, value: &Expr) {
        let parsed = self.declared.parsed;
        let element = match container(parsed, ty) {
            Some(name) if !MAPS.contains(&name) => ty.generics.first(),
            _ => None,
        };
        match value {
            Expr::ListLit { items, .. } => {
                let Some(element) = element else { return };
                for item in items {
                    let kinds = self.expr(item);
                    let mut at = path.clone();
                    at.push(0);
                    self.flow_value(owner, element, at, Some(item), &kinds, true);
                }
            }
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } if self.text(*method) == "collect" && args.is_empty() => {
                // A map is collected from pairs: its key and its value are two
                // positions, and each is handed over as it is where it is mixed.
                if container(parsed, ty).is_some_and(|name| MAPS.contains(&name)) {
                    let mixed = |walk: &mut Self, at: usize| {
                        let Some(part) = ty.generics.get(at) else {
                            return false;
                        };
                        if !plain_string(parsed, part) {
                            return false;
                        }
                        let mut path = path.clone();
                        path.push(at);
                        let position = Position {
                            owner: owner.clone(),
                            path,
                        };
                        if walk.in_hole && walk.hole_text.is_none() {
                            walk.flows.unwrappable.insert(position);
                            return false;
                        }
                        walk.tier_of(&position) == Tier::Mixed
                    };
                    let hand = match (mixed(self, 0), mixed(self, 1)) {
                        (true, true) => Hand::Pairs,
                        (true, false) => Hand::Keys,
                        (false, true) => Hand::Values,
                        (false, false) => return,
                    };
                    self.hand_over(receiver, hand);
                    return;
                }
                let Some(element) = element else { return };
                let mut at = path.clone();
                at.push(0);
                let position = Position {
                    owner: owner.clone(),
                    path: at,
                };
                if !plain_string(parsed, element) {
                    return;
                }
                if self.in_hole && self.hole_text.is_none() {
                    self.flows.unwrappable.insert(position);
                } else if self.tier_of(&position) == Tier::Mixed {
                    self.hand_over(receiver, Hand::Items);
                }
            }
            _ => {
                if let Some((from, from_ty)) = self.source(value) {
                    self.link(
                        &from,
                        &from_ty,
                        &mut Vec::new(),
                        owner,
                        ty,
                        &mut path.clone(),
                    );
                }
            }
        }
    }

    /// The declared position a whole value comes from: a name declared with
    /// one, a field, what a function of this program hands back.
    fn source(&self, value: &Expr) -> Option<(Owner, Type)> {
        match value {
            Expr::Variable(_) | Expr::Field { .. } => self.place(value),
            Expr::Call { func, .. } => {
                let callee = crate::contracts::keep::callee_name(self.declared.parsed, func)?;
                let result = self.declared.fns.get(&callee)?.result.clone()?;
                Some((Owner::Result(callee), result))
            }
            Expr::MethodCall { method, .. } => {
                let keys = self.declared.methods.get(self.text(*method))?;
                let [key] = keys.as_slice() else { return None };
                let result = self.declared.fns.get(key)?.result.clone()?;
                Some((Owner::Result(key.clone()), result))
            }
            Expr::Try(inner) => self.source(inner),
            _ => None,
        }
    }

    /// Link every `String` inside two types of one shape, position to position.
    fn link(
        &mut self,
        from: &Owner,
        from_ty: &Type,
        from_path: &mut Vec<usize>,
        to: &Owner,
        to_ty: &Type,
        to_path: &mut Vec<usize>,
    ) {
        let parsed = self.declared.parsed;
        if plain_string(parsed, from_ty) && plain_string(parsed, to_ty) {
            let a = Position {
                owner: from.clone(),
                path: from_path.clone(),
            };
            let b = Position {
                owner: to.clone(),
                path: to_path.clone(),
            };
            if a != b {
                self.flows.links.insert((a, b));
            }
            return;
        }
        if container(parsed, from_ty).is_none()
            || container(parsed, to_ty).is_none()
            || from_ty.generics.len() != to_ty.generics.len()
        {
            return;
        }
        for (i, (f, t)) in from_ty.generics.iter().zip(&to_ty.generics).enumerate() {
            from_path.push(i);
            to_path.push(i);
            self.link(from, f, from_path, to, t, to_path);
            from_path.pop();
            to_path.pop();
        }
    }

    /// Mark a value to be handed over as `hand` says: by address where this
    /// pass can rewrite the program, by the hole's text and the value's shape
    /// inside an `f"…"` hole (ADR-229 D1).
    fn hand_over(&mut self, value: &Expr, hand: Hand) {
        match &self.hole_text {
            Some(text) if self.in_hole => {
                self.flows
                    .hole_wraps
                    .entry(text.clone())
                    .or_default()
                    .insert((crate::check::argument_shape(value), hand));
            }
            _ => {
                self.flows.wraps.insert(value as *const Expr as usize, hand);
            }
        }
    }

    /// What reading a value of a declared type gives: the tier of each
    /// `String` in it.
    fn kinds_of(&self, owner: &Owner, ty: &Type, path: &mut Vec<usize>) -> Kinds {
        let parsed = self.declared.parsed;
        if plain_string(parsed, ty) {
            return match self.tier_of(&Position {
                owner: owner.clone(),
                path: path.clone(),
            }) {
                Tier::Owned => one(Kind::Owned),
                Tier::View => one(Kind::View),
                Tier::Mixed => one(Kind::Either),
            };
        }
        if ty.is_view && parsed.text(ty.name) == "String" {
            return one(Kind::View);
        }
        if container(parsed, ty).is_some() {
            let mut out = Kinds::new();
            for (i, arg) in ty.generics.iter().enumerate() {
                path.push(i);
                out.extend(self.kinds_of(owner, arg, path));
                path.pop();
            }
            if !out.is_empty() {
                return out;
            }
        }
        one(Kind::Owned)
    }

    /// The element a read of a container hands out: a map's value, anything
    /// else's element; `keys` a map's key.
    fn element_kinds(&self, owner: &Owner, ty: &Type, keys: bool) -> Kinds {
        let parsed = self.declared.parsed;
        let at = match container(parsed, ty) {
            Some(name) if MAPS.contains(&name) && !keys => 1,
            Some(_) => 0,
            None => return self.kinds_of(owner, ty, &mut Vec::new()),
        };
        match ty.generics.get(at) {
            Some(arg) => self.kinds_of(owner, arg, &mut vec![at]),
            None => one(Kind::Owned),
        }
    }

    /// The declared position an expression names as a place: a local
    /// declared with a type, a field of a struct this walk can name.
    fn place(&self, expr: &Expr) -> Option<(Owner, Type)> {
        match expr {
            Expr::Variable(name) => self.local(self.text(*name))?.declared.clone(),
            Expr::Field { base, name } => {
                let owner = self.struct_of(base)?;
                let field = self.text(*name).to_string();
                let (ty, _) = self.declared.fields.get(&(owner.clone(), field.clone()))?;
                Some((Owner::Field(owner, field), ty.clone()))
            }
            _ => None,
        }
    }

    fn block(&mut self, block: &Block) -> Kinds {
        self.scopes.push(BTreeMap::new());
        let mut tail = Kinds::new();
        for (i, stmt) in block.stmts.iter().enumerate() {
            let kinds = self.stmt(&stmt.node, stmt.span.start);
            if i + 1 == block.stmts.len() {
                tail = kinds;
            }
        }
        self.scopes.pop();
        tail
    }

    /// Walk a statement; what it is worth as a block's last value.
    fn stmt(&mut self, stmt: &Stmt, at: usize) -> Kinds {
        match stmt {
            Stmt::Let {
                names, value, ty, ..
            } => {
                let kinds = self.expr(value);
                let of = ty
                    .as_ref()
                    .map(|t| self.text(t.name).to_string())
                    .or_else(|| self.struct_of(value));
                let literal = (ty.is_none() && matches!(value, Expr::LitStr { .. })).then_some(at);
                // **A name bound to a list without a type is that list**
                // (ADR-224 D3): what is put into it goes where the list came
                // from, whose representation it shares below.
                let declared = match ty {
                    Some(t) => Some((Owner::Let(at), t.clone())),
                    None => self
                        .source(value)
                        .filter(|(_, t)| container(self.declared.parsed, t).is_some()),
                };
                let aliased = ty.is_none() && declared.is_some();
                let kinds = match &declared {
                    Some((owner, ty)) if aliased => self.kinds_of(owner, ty, &mut Vec::new()),
                    Some((owner, ty)) => {
                        self.flow_value(owner, ty, Vec::new(), Some(value), &kinds, true);
                        self.kinds_of(owner, ty, &mut Vec::new())
                    }
                    None => kinds,
                };
                if let [name] = names.as_slice() {
                    let name = self.text(*name).to_string();
                    self.bind(
                        name,
                        Local {
                            kinds,
                            of,
                            literal,
                            declared,
                        },
                    );
                }
                Kinds::new()
            }
            Stmt::Assign { target, value, .. } => {
                let kinds = self.expr(value);
                match target {
                    // `m[k] = v`: a map's key and value, a run's element.
                    Expr::Index { base, index } => {
                        let key = self.expr(index);
                        if let Some((owner, ty)) = self.place(base) {
                            match container(self.declared.parsed, &ty) {
                                Some(name) if MAPS.contains(&name) => {
                                    if let [k, v] = ty.generics.as_slice() {
                                        self.flow_value(
                                            &owner,
                                            k,
                                            vec![0],
                                            Some(index),
                                            &key,
                                            true,
                                        );
                                        self.flow_value(
                                            &owner,
                                            v,
                                            vec![1],
                                            Some(value),
                                            &kinds,
                                            true,
                                        );
                                    }
                                }
                                Some(_) => {
                                    if let Some(element) = ty.generics.first() {
                                        self.flow_value(
                                            &owner,
                                            element,
                                            vec![0],
                                            Some(value),
                                            &kinds,
                                            true,
                                        );
                                    }
                                }
                                None => {}
                            }
                        }
                    }
                    _ => {
                        if let Some((owner, ty)) = self.place(target) {
                            self.flow_value(&owner, &ty, Vec::new(), Some(value), &kinds, true);
                        }
                        if let Expr::Variable(name) = target {
                            let name = self.text(*name).to_string();
                            if let Some(local) =
                                self.scopes.iter_mut().rev().find_map(|s| s.get_mut(&name))
                            {
                                if local.declared.is_none() {
                                    local.kinds.extend(kinds);
                                }
                                // Assigned again: no longer the literal it was
                                // bound to.
                                local.literal = None;
                            }
                        }
                    }
                }
                Kinds::new()
            }
            Stmt::For {
                bindings,
                iter,
                body,
            } => {
                let kinds = self.expr(iter);
                self.scopes.push(BTreeMap::new());
                for binding in bindings {
                    let name = self.text(*binding).to_string();
                    self.bind(
                        name,
                        Local {
                            kinds: kinds.clone(),
                            ..Local::default()
                        },
                    );
                }
                self.block(body);
                self.scopes.pop();
                Kinds::new()
            }
            Stmt::While { cond, body } => {
                self.expr(cond);
                self.block(body);
                Kinds::new()
            }
            Stmt::Return(Some(value)) => {
                let kinds = self.expr(value);
                self.flow_result(value, &kinds);
                Kinds::new()
            }
            Stmt::Expr(expr) => self.expr(expr),
            _ => Kinds::new(),
        }
    }

    /// What a function hands back: its result's position, wrapped by the
    /// emitter where it is mixed (ADR-222 D3), so not marked here.
    fn flow_result(&mut self, value: &Expr, kinds: &Kinds) {
        let Some(key) = self.result.clone() else {
            return;
        };
        let Some(ty) = self.declared.fns.get(&key).and_then(|f| f.result.clone()) else {
            return;
        };
        self.flow_value(
            &Owner::Result(key),
            &ty,
            Vec::new(),
            Some(value),
            kinds,
            false,
        );
    }

    /// A name bound to a literal, kept in a position: remembered with it, so
    /// that where the position stays `String` the binding is declared `String`
    /// (ADR-222 D4).
    fn kept_literal(&mut self, value: &Expr, owner: &Owner, path: &[usize], ty: &Type) {
        if !plain_string(self.declared.parsed, ty) {
            return;
        }
        let Expr::Variable(name) = value else {
            return;
        };
        let Some(at) = self.local(self.text(*name)).and_then(|l| l.literal) else {
            return;
        };
        self.flows
            .literal_lets
            .entry(at)
            .or_default()
            .insert(Position {
                owner: owner.clone(),
                path: path.to_vec(),
            });
    }

    /// The struct a value is of, where the program says so without types: a
    /// literal of it, a name bound to one, `self` in its `impl`.
    fn struct_of(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::StructLit { name, .. } => Some(self.text(*name).to_string()),
            Expr::Variable(name) => self.local(self.text(*name)).and_then(|l| l.of.clone()),
            Expr::Unary { expr, .. } | Expr::Try(expr) => self.struct_of(expr),
            _ => None,
        }
    }

    /// Arguments handed to a function of this program: each goes into its
    /// parameter's position.
    fn flow_args(&mut self, key: &str, args: &[Expr], kinds: &[Kinds]) {
        let Some(decl) = self.declared.fns.get(key).cloned() else {
            return;
        };
        for (at, (arg, kinds)) in args.iter().zip(kinds).enumerate() {
            if let Some(ty) = decl.params.get(at) {
                // The emitter wraps an argument to a mixed parameter itself
                // (`Emitter::either_param`), in a hole as anywhere else.
                let owner = Owner::Param(key.to_string(), at);
                self.flow_value(&owner, ty, Vec::new(), Some(arg), kinds, false);
            }
        }
    }

    /// Walk an expression, recording what flows into the positions inside it;
    /// what kind of text its value is.
    fn expr(&mut self, expr: &Expr) -> Kinds {
        match expr {
            Expr::LitStr { .. } => one(Kind::Static),
            // `null`, a number, a truth value and a character are no text.
            Expr::LitNull | Expr::LitInt(_) | Expr::LitFloat(_) | Expr::LitBool(_) => Kinds::new(),
            // **A hole is code** (ADR-032): what it passes to a function of
            // this program is a flow like any other.
            //
            // **And a value handed over inside one is handed over there**
            // ([ADR-229](../../docs/specification/adr/adr-229.md) D1): each hole
            // is walked with its text, which is what the wrap is recorded by.
            Expr::LitInterpolated(literal) => {
                let outer = std::mem::replace(&mut self.in_hole, true);
                let holes = crate::emit::interpolation(literal)
                    .map(|(_, holes)| holes)
                    .unwrap_or_default();
                for text in holes {
                    let Ok(hole) =
                        crate::parser::parse_expression(&self.declared.parsed.interner, &text)
                    else {
                        continue;
                    };
                    let held = self.hole_text.replace(text);
                    self.expr(&hole);
                    self.hole_text = held;
                }
                self.in_hole = outer;
                one(Kind::Owned)
            }
            Expr::Variable(name) => self
                .local(self.text(*name))
                .map(|l| l.kinds.clone())
                .unwrap_or_else(|| one(Kind::Owned)),
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
                let received = self.expr(receiver);
                let given: Vec<Kinds> = args.iter().map(|a| self.expr(a)).collect();
                let method = self.text(*method).to_string();
                // **A method this program declares answers for itself**, before
                // any name in the lists below.
                if let Some(keys) = self.declared.methods.get(&method).cloned() {
                    for key in &keys {
                        self.flow_args(key, args, &given);
                    }
                    return keys.iter().flat_map(|key| self.result_kinds(key)).collect();
                }
                // **Into a container that declares its element `String`**.
                if let Some((owner, ty)) = self.place(receiver) {
                    let kind = container(self.declared.parsed, &ty);
                    if PUTS.contains(&method.as_str()) && kind.is_some() {
                        let is_map = kind.is_some_and(|k| MAPS.contains(&k));
                        match (is_map, args.as_slice(), given.as_slice()) {
                            (true, [k, v], [kk, vk]) => {
                                if let [kt, vt] = ty.generics.as_slice() {
                                    self.flow_value(&owner, kt, vec![0], Some(k), kk, true);
                                    self.flow_value(&owner, vt, vec![1], Some(v), vk, true);
                                }
                            }
                            (false, _, _) => {
                                // `insert(i, v)` on a run: the value is last.
                                if let (Some(element), Some(value), Some(kinds)) =
                                    (ty.generics.first(), args.last(), given.last())
                                {
                                    let wrap = method != "extend" && method != "append";
                                    self.flow_value(
                                        &owner,
                                        element,
                                        vec![0],
                                        Some(value),
                                        kinds,
                                        wrap,
                                    );
                                }
                            }
                            _ => {}
                        }
                        return one(Kind::Owned);
                    }
                    if ELEMENTS.contains(&method.as_str()) {
                        return self.element_kinds(&owner, &ty, method == "keys");
                    }
                }
                // **What `map` makes is what its lambda hands back**, with the
                // lambda's parameter bound to what came in (ADR-227 D2):
                // `lines.map(fn(l) { (l, 1) })` is pairs keyed by views.
                if method == "map"
                    && let [Expr::Closure { params, body, .. }] = args.as_slice()
                {
                    let result = self.result.take();
                    self.scopes.push(BTreeMap::new());
                    let names: Vec<String> = match params.as_slice() {
                        [] => vec!["a".to_string()],
                        named => named.iter().map(|p| self.text(*p).to_string()).collect(),
                    };
                    for name in names {
                        self.bind(
                            name,
                            Local {
                                kinds: received.clone(),
                                ..Local::default()
                            },
                        );
                    }
                    let made = self.block(body);
                    self.scopes.pop();
                    self.result = result;
                    return made;
                }
                if matches!(
                    method.as_str(),
                    "filter" | "rev" | "skip" | "take" | "step_by"
                ) {
                    return received;
                }
                if OWNS.contains(&method.as_str()) {
                    return one(Kind::Owned);
                }
                if VIEWS.contains(&method.as_str()) {
                    return viewed(&received);
                }
                if ELEMENTS.contains(&method.as_str()) {
                    return received;
                }
                one(Kind::Owned)
            }
            Expr::Call { func, args, .. } => {
                let given: Vec<Kinds> = args.iter().map(|a| self.expr(a)).collect();
                let callee = crate::contracts::keep::callee_name(self.declared.parsed, func);
                match callee {
                    Some(callee) if self.declared.fns.contains_key(&callee) => {
                        self.flow_args(&callee, args, &given);
                        self.result_kinds(&callee)
                    }
                    // `Vec()`, `HashMap()`: empty, so no text of any kind.
                    Some(callee) if args.is_empty() && is_a_container_name(&callee) => Kinds::new(),
                    Some(callee) => std_result(&callee),
                    None => one(Kind::Owned),
                }
            }
            Expr::Field { base, .. } | Expr::SafeField { base, .. } => {
                self.expr(base);
                match self.place(expr) {
                    Some((owner, ty)) => self.kinds_of(&owner, &ty, &mut Vec::new()),
                    None => one(Kind::Owned),
                }
            }
            Expr::Index { base, index } => {
                let received = self.expr(base);
                self.expr(index);
                if matches!(index.as_ref(), Expr::Range { .. }) {
                    return viewed(&received);
                }
                match self.place(base) {
                    Some((owner, ty)) => self.element_kinds(&owner, &ty, false),
                    None => received,
                }
            }
            Expr::StructLit { name, fields } => {
                let owner = self.text(*name).to_string();
                for field in fields {
                    let shorthand = Expr::Variable(field.name);
                    let value = field.value.as_ref().unwrap_or(&shorthand);
                    let kinds = self.expr(value);
                    let name = self.text(field.name).to_string();
                    let key = (owner.clone(), name.clone());
                    if let Some((ty, _)) = self.declared.fields.get(&key).cloned() {
                        // The emitter wraps what goes into a mixed field
                        // itself (ADR-222 D3).
                        let owner = Owner::Field(owner.clone(), name);
                        let value = field.value.as_ref();
                        self.flow_value(&owner, &ty, Vec::new(), value, &kinds, false);
                        if value.is_none() {
                            self.kept_literal(&shorthand, &owner, &[], &ty);
                        }
                    }
                }
                one(Kind::Owned)
            }
            Expr::With { base, fields, .. } => {
                self.expr(base);
                let owner = self.struct_of(base);
                for field in fields {
                    let Some(value) = &field.value else { continue };
                    let kinds = self.expr(value);
                    if let Some(owner) = &owner {
                        let name = self.text(field.name).to_string();
                        if let Some((ty, _)) = self
                            .declared
                            .fields
                            .get(&(owner.clone(), name.clone()))
                            .cloned()
                        {
                            let owner = Owner::Field(owner.clone(), name);
                            self.flow_value(&owner, &ty, Vec::new(), Some(value), &kinds, false);
                        }
                    }
                }
                one(Kind::Owned)
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(cond);
                let mut out = self.block(then_branch);
                if let Some(block) = else_branch {
                    out.extend(self.block(block));
                }
                out
            }
            Expr::Match { value, arms } => {
                self.expr(value);
                let mut out = Kinds::new();
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.expr(guard);
                    }
                    out.extend(self.expr(&arm.body));
                }
                out
            }
            Expr::Block(block) | Expr::Unsafe(block) | Expr::Overlap(block) => self.block(block),
            Expr::Closure { body, .. } => {
                // A lambda's `return` is the lambda's, not the function's.
                let result = self.result.take();
                self.block(body);
                self.result = result;
                one(Kind::Owned)
            }
            Expr::Spawn { body, .. } => {
                let result = self.result.take();
                self.expr(body);
                self.result = result;
                one(Kind::Owned)
            }
            Expr::Coalesce { value, fallback } => {
                let mut out = self.expr(value);
                out.extend(self.expr(fallback));
                out
            }
            Expr::TryCatch { expr, handler } => {
                let mut out = self.expr(expr);
                out.extend(self.block(handler));
                out
            }
            Expr::Try(inner) | Expr::Unary { expr: inner, .. } => self.expr(inner),
            Expr::Throw(inner) => {
                self.expr(inner);
                Kinds::new()
            }
            Expr::Return(value) => {
                if let Some(value) = value {
                    let kinds = self.expr(value);
                    self.flow_result(value, &kinds);
                }
                Kinds::new()
            }
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
                one(Kind::Owned)
            }
            Expr::Cast { expr, .. } => {
                self.expr(expr);
                one(Kind::Owned)
            }
            Expr::Tuple(items) | Expr::ListLit { items, .. } => {
                let mut out = Kinds::new();
                for item in items {
                    out.extend(self.expr(item));
                }
                out
            }
            Expr::Range { start, end, .. } => {
                self.expr(start);
                self.expr(end);
                one(Kind::Owned)
            }
            _ => one(Kind::Owned),
        }
    }

    /// What a function of this program hands back.
    fn result_kinds(&self, key: &str) -> Kinds {
        match self.declared.fns.get(key).and_then(|f| f.result.as_ref()) {
            Some(ty) => self.kinds_of(&Owner::Result(key.to_string()), ty, &mut Vec::new()),
            None => one(Kind::Owned),
        }
    }
}

fn is_a_container_name(callee: &str) -> bool {
    let name = callee.rsplit("::").next().unwrap_or(callee);
    [RUNS, SETS, MAPS].iter().any(|list| list.contains(&name))
}

/// A view of text of these kinds: a view of a literal is as lasting as the
/// literal, and a view of anything else is a view.
fn viewed(received: &Kinds) -> Kinds {
    match received.iter().all(|k| *k == Kind::Static) && !received.is_empty() {
        true => one(Kind::Static),
        false => one(Kind::View),
    }
}

/// What a function of `std` hands back, off its ledger entry.
fn std_result(callee: &str) -> Kinds {
    static STD: std::sync::OnceLock<Option<crate::contracts::Ledger>> = std::sync::OnceLock::new();
    let ledger = STD.get_or_init(|| crate::contracts::Ledger::parse(crate::contracts::STD).ok());
    let view = ledger
        .as_ref()
        .and_then(|l| l.lookup(callee))
        .and_then(|(_, c)| c.signature.as_ref())
        .and_then(|s| s.result.as_ref())
        .is_some_and(|r| r.text() == "ref String");
    match view {
        true => one(Kind::View),
        false => one(Kind::Owned),
    }
}

/// Walk every function of the program once against the tiers found so far.
fn walk(declared: &Declared<'_>, tiers: &Tiers) -> Flows {
    let parsed = declared.parsed;
    let mut flows = Flows::default();
    let function = |item: &Item, target: Option<&str>, flows: &mut Flows| {
        let Item::Fn {
            name,
            receiver,
            args,
            body,
            ..
        } = item
        else {
            return;
        };
        let own = name
            .map(|n| parsed.text(n).to_string())
            .unwrap_or_else(|| "new".to_string());
        let key = key(target, &own);
        let mut walk = Walk {
            declared,
            tiers,
            flows,
            scopes: vec![BTreeMap::new()],
            result: Some(key.clone()),
            in_hole: false,
            hole_text: None,
        };
        for (at, arg) in args.iter().enumerate() {
            let owner = Owner::Param(key.clone(), at);
            let kinds = walk.kinds_of(&owner, &arg.ty, &mut Vec::new());
            let local = Local {
                kinds,
                of: Some(parsed.text(arg.ty.name).to_string()),
                literal: None,
                declared: Some((owner, arg.ty.clone())),
            };
            walk.bind(parsed.text(arg.name).to_string(), local);
        }
        if receiver.is_some()
            && let Some(target) = target
        {
            walk.bind(
                "self".to_string(),
                Local {
                    kinds: one(Kind::Owned),
                    of: Some(target.to_string()),
                    ..Local::default()
                },
            );
        }
        let tail = walk.block(body);
        // The body's last value is what it hands back.
        if let Some(Stmt::Expr(last)) = body.stmts.last().map(|s| &s.node) {
            walk.flow_result(last, &tail);
        }
    };
    for item in &parsed.program.items {
        match &item.node {
            Item::Fn { .. } => function(&item.node, None, &mut flows),
            Item::Impl {
                target, methods, ..
            } => {
                let target = parsed.text(target.name).to_string();
                for method in methods {
                    function(&method.node, Some(&target), &mut flows);
                }
            }
            _ => {}
        }
    }
    flows
}

fn decide(declared: &Declared<'_>, flows: &Flows) -> Tiers {
    // Linked positions are one representation: each gets what flows into any
    // of them, and none is mixed where one of them cannot be.
    let mut positions = flows.positions.clone();
    let mut fixed: BTreeSet<Position> = positions
        .keys()
        .filter(|p| declared.published(&p.owner) || flows.unwrappable.contains(p))
        .cloned()
        .collect();
    let mut changed = true;
    while changed {
        changed = false;
        for (a, b) in &flows.links {
            let mut both = positions.get(a).cloned().unwrap_or_default();
            both.extend(positions.get(b).cloned().unwrap_or_default());
            for p in [a, b] {
                let kinds = positions.entry(p.clone()).or_default();
                if *kinds != both {
                    *kinds = both.clone();
                    changed = true;
                }
            }
            if fixed.contains(a) != fixed.contains(b) {
                fixed.insert(a.clone());
                fixed.insert(b.clone());
                changed = true;
            }
        }
    }
    positions
        .iter()
        .map(|(position, kinds)| {
            let fixed = fixed.contains(position);
            let tier = match (tier(kinds), fixed) {
                (Tier::Mixed, true) => Tier::Owned,
                (tier, _) => tier,
            };
            (position.clone(), tier)
        })
        .filter(|(_, tier)| *tier != Tier::Owned)
        .collect()
}

/// **Decide the tiers and write them into the types**, so that every later
/// reader of the program reads one answer.
pub fn refine(parsed: &mut Parsed) {
    let declared = Declared::of(parsed);
    // A fixpoint: a read or a call hands on the tier found for its position,
    // and tiers only ever move up (owned, view, mixed), so it ends. The last
    // walk is against the final tiers, which is what the wraps are read off.
    let mut tiers = Tiers::new();
    let mut flows = Flows::default();
    for _ in 0..16 {
        flows = walk(&declared, &tiers);
        let next = decide(&declared, &flows);
        if next == tiers {
            break;
        }
        tiers = next;
    }
    if tiers.is_empty() && flows.literal_lets.is_empty() {
        return;
    }
    // **A name bound to a literal and kept where text of its own is wanted is
    // declared `String`** (ADR-222 D4): the literal is built into text once,
    // where it is bound, which is what the program would have written.
    let lets: BTreeSet<usize> = flows
        .literal_lets
        .iter()
        .filter(|(_, positions)| positions.iter().any(|p| !tiers.contains_key(p)))
        .map(|(at, _)| *at)
        .collect();
    let text = declared.string_type();
    let said = said(&tiers);
    let into = [
        parsed.interner.intern_string("into_either"),
        parsed.interner.intern_string("into_either_maybe"),
        parsed.interner.intern_string("either_items"),
        parsed.interner.intern_string("either_keys"),
        parsed.interner.intern_string("either_values"),
        parsed.interner.intern_string("either_pairs"),
    ];
    parsed.hole_wraps = flows
        .hole_wraps
        .iter()
        .map(|(text, wraps)| {
            let wraps = wraps
                .iter()
                .map(|(shape, hand)| (shape.clone(), into[*hand as usize]))
                .collect();
            (text.clone(), wraps)
        })
        .collect();
    rewrite(parsed, &tiers, &lets, text, &flows.wraps, into);
    parsed.text_tiers = said;
}

fn mark(ty: &mut Type, path: &[usize], tier: Tier) {
    let Some((first, rest)) = path.split_first() else {
        match tier {
            Tier::Owned => {}
            Tier::View => ty.is_view = true,
            Tier::Mixed => {
                ty.is_view = true;
                ty.either = true;
            }
        }
        return;
    };
    if let Some(arg) = ty.generics.get_mut(*first) {
        mark(arg, rest, tier);
    }
}

fn rewrite(
    parsed: &mut Parsed,
    tiers: &Tiers,
    lets: &BTreeSet<usize>,
    text_type: Option<Type>,
    wraps: &BTreeMap<usize, Hand>,
    into: [winnow_grammar::Symbol; 6],
) {
    // Values first: a value's address is where the walk saw it, and nothing
    // has moved yet.
    let names: Vec<(usize, Option<String>, Option<String>)> = parsed
        .program
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| match &item.node {
            Item::Struct { name, .. } => (i, Some(parsed.text(*name).to_string()), None),
            Item::Fn { name, .. } => (
                i,
                None,
                Some(
                    name.map(|n| parsed.text(n).to_string())
                        .unwrap_or_else(|| "new".to_string()),
                ),
            ),
            Item::Impl { target, .. } => (i, Some(parsed.text(target.name).to_string()), None),
            _ => (i, None, None),
        })
        .collect();
    let field_names: BTreeMap<usize, Vec<String>> = parsed
        .program
        .items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match &item.node {
            Item::Struct { fields, .. } => Some((
                i,
                fields
                    .iter()
                    .map(|f| parsed.text(f.name).to_string())
                    .collect(),
            )),
            _ => None,
        })
        .collect();
    let method_names: BTreeMap<usize, Vec<String>> = parsed
        .program
        .items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match &item.node {
            Item::Impl { methods, .. } => Some((
                i,
                methods
                    .iter()
                    .map(|m| match &m.node {
                        Item::Fn { name, .. } => name
                            .map(|n| parsed.text(n).to_string())
                            .unwrap_or_else(|| "new".to_string()),
                        _ => String::new(),
                    })
                    .collect(),
            )),
            _ => None,
        })
        .collect();
    let program = &mut parsed.program;
    for item in &mut program.items {
        wrap_item(&mut item.node, wraps, into);
        if let Some(text_type) = &text_type {
            annotate_item(&mut item.node, lets, text_type);
        }
    }
    for (position, tier) in tiers {
        match &position.owner {
            Owner::Field(owner, field) => {
                for (i, name, _) in &names {
                    if name.as_deref() != Some(owner.as_str()) {
                        continue;
                    }
                    let Some(j) = field_names
                        .get(i)
                        .and_then(|fs| fs.iter().position(|f| f == field))
                    else {
                        continue;
                    };
                    if let Item::Struct { fields, .. } = &mut program.items[*i].node {
                        mark(&mut fields[j].ty, &position.path, *tier);
                    }
                }
            }
            Owner::Result(key) | Owner::Param(key, _) => {
                let (target, own) = match key.rsplit_once("::") {
                    Some((t, o)) => (Some(t), o),
                    None => (None, key.as_str()),
                };
                for (i, name, fname) in &names {
                    let item = match target {
                        None if fname.as_deref() == Some(own) => &mut program.items[*i].node,
                        Some(t) if name.as_deref() == Some(t) => {
                            let Some(j) = method_names
                                .get(i)
                                .and_then(|ms| ms.iter().position(|m| m == own))
                            else {
                                continue;
                            };
                            match &mut program.items[*i].node {
                                Item::Impl { methods, .. } => &mut methods[j].node,
                                _ => continue,
                            }
                        }
                        _ => continue,
                    };
                    let Item::Fn { args, ret_type, .. } = item else {
                        continue;
                    };
                    match &position.owner {
                        Owner::Result(_) => {
                            if let Some(ty) = ret_type {
                                mark(ty, &position.path, *tier);
                            }
                        }
                        Owner::Param(_, at) => {
                            if let Some(arg) = args.get_mut(*at) {
                                mark(&mut arg.ty, &position.path, *tier);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Owner::Let(at) => {
                for item in &mut program.items {
                    mark_let_item(&mut item.node, *at, &position.path, *tier);
                }
            }
        }
    }
}

// --- Walking the program mutably ------------------------------------------

fn each_block(item: &mut Item, f: &mut dyn FnMut(&mut Block)) {
    match item {
        Item::Fn { body, .. } => f(body),
        Item::Impl { methods, .. } => {
            for method in methods {
                each_block(&mut method.node, f);
            }
        }
        _ => {}
    }
}

/// Every expression and block below a block, children before parents.
fn visit_block_mut(
    block: &mut Block,
    f: &mut dyn FnMut(&mut Expr),
    s: &mut dyn FnMut(&mut Stmt, usize),
) {
    for stmt in &mut block.stmts {
        let at = stmt.span.start;
        match &mut stmt.node {
            Stmt::Let { value, .. } => visit_expr_mut(value, f, s),
            Stmt::Assign { target, value, .. } => {
                visit_expr_mut(target, f, s);
                visit_expr_mut(value, f, s);
            }
            Stmt::For { iter, body, .. } => {
                visit_expr_mut(iter, f, s);
                visit_block_mut(body, f, s);
            }
            Stmt::While { cond, body } => {
                visit_expr_mut(cond, f, s);
                visit_block_mut(body, f, s);
            }
            Stmt::Return(Some(value)) | Stmt::Expr(value) => visit_expr_mut(value, f, s),
            _ => {}
        }
        s(&mut stmt.node, at);
    }
}

fn visit_expr_mut(
    expr: &mut Expr,
    f: &mut dyn FnMut(&mut Expr),
    s: &mut dyn FnMut(&mut Stmt, usize),
) {
    match expr {
        Expr::Call { func, args, config } => {
            visit_expr_mut(func, f, s);
            args.iter_mut().for_each(|a| visit_expr_mut(a, f, s));
            config
                .iter_mut()
                .for_each(|c| visit_expr_mut(&mut c.value, f, s));
        }
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
            visit_expr_mut(receiver, f, s);
            args.iter_mut().for_each(|a| visit_expr_mut(a, f, s));
            config
                .iter_mut()
                .for_each(|c| visit_expr_mut(&mut c.value, f, s));
        }
        Expr::Binary { lhs, rhs, .. } => {
            visit_expr_mut(lhs, f, s);
            visit_expr_mut(rhs, f, s);
        }
        Expr::Unary { expr, .. }
        | Expr::Try(expr)
        | Expr::Throw(expr)
        | Expr::Cast { expr, .. } => visit_expr_mut(expr, f, s),
        Expr::Return(Some(value)) => visit_expr_mut(value, f, s),
        Expr::Field { base, .. } | Expr::SafeField { base, .. } => visit_expr_mut(base, f, s),
        Expr::Index { base, index } => {
            visit_expr_mut(base, f, s);
            visit_expr_mut(index, f, s);
        }
        Expr::Range { start, end, .. } => {
            visit_expr_mut(start, f, s);
            visit_expr_mut(end, f, s);
        }
        Expr::Tuple(items) | Expr::ListLit { items, .. } => {
            items.iter_mut().for_each(|i| visit_expr_mut(i, f, s))
        }
        Expr::Coalesce { value, fallback } => {
            visit_expr_mut(value, f, s);
            visit_expr_mut(fallback, f, s);
        }
        Expr::TryCatch { expr, handler } => {
            visit_expr_mut(expr, f, s);
            visit_block_mut(handler, f, s);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            visit_expr_mut(cond, f, s);
            visit_block_mut(then_branch, f, s);
            if let Some(block) = else_branch {
                visit_block_mut(block, f, s);
            }
        }
        Expr::Match { value, arms } => {
            visit_expr_mut(value, f, s);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    visit_expr_mut(guard, f, s);
                }
                visit_expr_mut(&mut arm.body, f, s);
            }
        }
        Expr::StructLit { fields, .. } => {
            for field in fields {
                if let Some(value) = &mut field.value {
                    visit_expr_mut(value, f, s);
                }
            }
        }
        Expr::With { base, fields, .. } => {
            visit_expr_mut(base, f, s);
            for field in fields {
                if let Some(value) = &mut field.value {
                    visit_expr_mut(value, f, s);
                }
            }
        }
        Expr::Block(block) | Expr::Unsafe(block) | Expr::Overlap(block) => {
            visit_block_mut(block, f, s)
        }
        Expr::Closure { body, .. } => visit_block_mut(body, f, s),
        Expr::Spawn { body, .. } => visit_expr_mut(body, f, s),
        _ => {}
    }
    f(expr);
}

/// **A value going into a mixed position is handed over as `value.into_either()`**
/// (ADR-223 D2): `EitherText` borrows a view and moves text of its own in,
/// and which it is the language below reads off the value's type.
fn wrap_item(item: &mut Item, wraps: &BTreeMap<usize, Hand>, into: [winnow_grammar::Symbol; 6]) {
    if wraps.is_empty() {
        return;
    }
    // Every address is compared before any expression is replaced, so the
    // replacement - which moves the value into a new box - cannot shift an
    // address another comparison is still waiting for.
    let mut marked: Vec<(*mut Expr, Hand)> = Vec::new();
    each_block(item, &mut |block| {
        visit_block_mut(
            block,
            &mut |expr| {
                if let Some(hand) = wraps.get(&(expr as *const Expr as usize)) {
                    marked.push((expr as *mut Expr, *hand));
                }
            },
            &mut |_, _| {},
        )
    });
    // Replaced from the innermost out - children were marked before parents -
    // so wrapping one never moves another that is still to be found: a
    // replacement writes into the slot it found, and only what it wraps
    // moves, into a box of its own.
    for (at, hand) in marked {
        each_block(item, &mut |block| {
            visit_block_mut(
                block,
                &mut |expr| {
                    if std::ptr::eq(expr as *const Expr, at as *const Expr) {
                        let value = std::mem::replace(expr, Expr::Break);
                        *expr = Expr::MethodCall {
                            receiver: Box::new(value),
                            method: into[hand as usize],
                            args: Vec::new(),
                            config: Vec::new(),
                        };
                    }
                },
                &mut |_, _| {},
            )
        });
    }
}

fn annotate_item(item: &mut Item, lets: &BTreeSet<usize>, text: &Type) {
    if lets.is_empty() {
        return;
    }
    each_block(item, &mut |block| {
        visit_block_mut(block, &mut |_| {}, &mut |stmt, at| {
            if let Stmt::Let { ty, .. } = stmt
                && ty.is_none()
                && lets.contains(&at)
            {
                *ty = Some(text.clone());
            }
        })
    });
}

fn mark_let_item(item: &mut Item, wanted: usize, path: &[usize], tier: Tier) {
    each_block(item, &mut |block| {
        visit_block_mut(block, &mut |_| {}, &mut |stmt, at| {
            if at == wanted
                && let Stmt::Let { ty: Some(ty), .. } = stmt
            {
                mark(ty, path, tier);
            }
        })
    });
}

/// The positions that are not text of its own, in the words `--tethers` uses.
fn said(tiers: &Tiers) -> Vec<String> {
    tiers
        .iter()
        .filter_map(|(position, tier)| {
            let what = match tier {
                Tier::View => "a view: only views and literals flow into it",
                Tier::Mixed => {
                    "a view or text of its own, per value: both flow into it, and each is kept \
                     as it is"
                }
                Tier::Owned => return None,
            };
            let place = match &position.owner {
                Owner::Field(owner, field) => format!("`{owner}.{field}`"),
                Owner::Result(key) => format!("what `{key}` hands back"),
                Owner::Param(key, at) => format!("parameter {} of `{key}`", at + 1),
                Owner::Let(_) => "a `let`".to_string(),
            };
            let inside = match position.path.as_slice() {
                [] => String::new(),
                _ => " (its elements)".to_string(),
            };
            Some(format!("{place}{inside} is {what}"))
        })
        .collect()
}

/// **Apply a hole's wraps to it as it is parsed** (ADR-229 D1): each value
/// whose shape was recorded becomes `value.<method>()`, innermost first, as
/// [`wrap_item`] does to the program.
pub fn wrap_hole(hole: &mut Expr, wraps: &[(String, winnow_grammar::Symbol)]) {
    let mut marked: Vec<(*mut Expr, winnow_grammar::Symbol)> = Vec::new();
    let mut mark = |expr: &mut Expr| {
        let shape = crate::check::argument_shape(expr);
        if let Some((_, method)) = wraps.iter().find(|(s, _)| *s == shape) {
            marked.push((expr as *mut Expr, *method));
        }
    };
    visit_expr_mut(hole, &mut mark, &mut |_, _| {});
    for (at, method) in marked {
        visit_expr_mut(
            hole,
            &mut |expr| {
                if std::ptr::eq(expr as *const Expr, at as *const Expr) {
                    let value = std::mem::replace(expr, Expr::Break);
                    *expr = Expr::MethodCall {
                        receiver: Box::new(value),
                        method,
                        args: Vec::new(),
                        config: Vec::new(),
                    };
                }
            },
            &mut |_, _| {},
        );
    }
}
