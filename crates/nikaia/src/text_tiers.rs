//! **What a declared `String` is below, decided by what flows into it**
//! ([ADR-282](../../../docs/specification/adr/adr-282.md),
//! [ADR-282](../../../docs/specification/adr/adr-282.md)).
//!
//! [ADR-282](../../../docs/specification/adr/adr-282.md) D1 made `String` the
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
//!   written that, and [ADR-283](../../../docs/specification/adr/adr-283.md)
//!   decides where each buffer lives - in the caller's frame for nothing, with
//!   a handle only where no frame outlives it;
//! * **both**: it becomes `ref String` marked [`crate::ast::Type::either`],
//!   which is `nikaia_std::either_text::EitherText` below. A view is borrowed
//!   and text of its own is owned, **each at its own line**, so no line pays for
//!   another line's kind of text.
//!
//! A literal alone moves nothing: a position only literals and owned text flow
//! into stays `String`, and the literal is built where it stands as before
//! ([ADR-282](../../../docs/specification/adr/adr-282.md) D4).
//!
//! **What is published may become a view but never both kinds**: a `pub` field
//! of a `pub` struct, and a `pub` function's parameters and result. Their
//! representation leaves with the package, before the programs that use it
//! exist; a view the package itself puts there is in its ledger as
//! `ref String`, and a mixed one would need a type no other package can know
//! to build. There the view is refused as before, saying why
//! ([ADR-282](../../../docs/specification/adr/adr-282.md) D25).
//!
//! Run once, right after parsing, so that the checker, every derived column,
//! the ledger and the emitter all read one answer off the types.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Block, Expr, Item, Stmt, Type};
use crate::parser::Parsed;

/// What a set of kinds makes of a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Owned,
    View,
    Mixed,
}

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

/// The tiers decided, by position.
type Tiers = BTreeMap<Position, Tier>;

fn position_from(position: nikaia_std::tools::text_tiers::TextPosition) -> Position {
    use nikaia_std::tools::text_tiers::TextOwner;
    let owner = match position.owner {
        TextOwner::Field(of, field) => Owner::Field(of, field),
        TextOwner::Returned(key) => Owner::Result(key),
        TextOwner::Param(key, at) => Owner::Param(key, at as usize),
        TextOwner::Let(at) => Owner::Let(at as usize),
    };
    Position {
        owner,
        path: position.path.into_iter().map(|at| at as usize).collect(),
    }
}

fn tier_from(tier: nikaia_std::tools::text_tiers::Tier) -> Tier {
    use nikaia_std::tools::text_tiers::Tier as Nika;
    match tier {
        Nika::Owned => Tier::Owned,
        Nika::View => Tier::View,
        Nika::Mixed => Tier::Mixed,
    }
}

/// **No two nodes of the tree share an id** ([ADR-340](../../../docs/specification/adr/adr-340.md)
/// D2): a node the parser placed twice - `xs[a..]` holds `xs` as the base and
/// again as the receiver of `len` - keeps its id where it is first met, and
/// the second place takes a new one, so what is recorded about one place is not
/// read at the other.
fn separate_repeated_ids(parsed: &mut Parsed) {
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    for item in &mut parsed.program.items {
        each_block(&mut item.node, &mut |block| {
            visit_block_mut(
                block,
                &mut |expr| {
                    if !seen.insert(crate::ast::id_of(expr)) {
                        crate::ast::set_id(expr, crate::ast::NodeId::fresh());
                    }
                },
                &mut |_, _| {},
            )
        });
    }
}

/// **Decide the tiers and write them into the types**, so that every later
/// reader of the program reads one answer.
///
/// **The walk and the decision are Nikaia** (`tools/text_tiers.nika`,
/// ADR-294, #125): every value's kind of text, the positions it flows into,
/// the fixpoint over them, and where a value is handed into a mixed one. What
/// stays here is writing that answer into the syntax tree.
pub fn refine(parsed: &mut Parsed) {
    separate_repeated_ids(parsed);
    use nikaia_std::tools::text_tiers::{TierAsk, hand_number, text_tiers};
    let library = crate::contracts::std_ledger();
    let ask = TierAsk {
        names: &parsed.interner,
        aliases: &parsed.aliases,
        library,
    };
    let decided = text_tiers(&parsed.program, &ask);
    let tiers: Tiers = decided
        .tiers
        .into_iter()
        .map(|(position, tier)| (position_from(position), tier_from(tier)))
        .collect();
    // **A name bound to a literal and kept where text of its own is wanted is
    // declared `String`** (ADR-282 D17): the literal is built into text once,
    // where it is bound, which is what the program would have written.
    let lets: BTreeSet<usize> = decided.lets.iter().map(|at| *at as usize).collect();
    if tiers.is_empty() && lets.is_empty() {
        return;
    }
    let wraps: BTreeMap<usize, usize> = decided
        .wraps
        .iter()
        .map(|(at, hand)| (*at as usize, hand_number(hand) as usize))
        .collect();
    let said = said(&tiers);
    let into = [
        parsed.interner.intern_string("into_either"),
        parsed.interner.intern_string("into_either_maybe"),
        parsed.interner.intern_string("either_items"),
        parsed.interner.intern_string("either_keys"),
        parsed.interner.intern_string("either_values"),
        parsed.interner.intern_string("either_pairs"),
    ];
    parsed.hole_wraps = decided
        .hole_wraps
        .iter()
        .map(|(at, wraps)| {
            // By the value's id, which is what the hole is matched by wherever
            // a reader takes a copy of it (ADR-340).
            let nodes: BTreeSet<(usize, usize)> = wraps
                .iter()
                .map(|wrap| {
                    (
                        crate::check::value_node(&wrap.value),
                        hand_number(&wrap.hand) as usize,
                    )
                })
                .collect();
            let wraps = nodes
                .into_iter()
                .map(|(node, hand)| (node, into[hand]))
                .collect();
            (*at as u32, wraps)
        })
        .collect();
    // **The `String` a binding is declared with does not have to be written
    // anywhere else** (#441): `text` is the type node of a declared field,
    // parameter or result, and a program with none of them - a `main` that
    // pushes `let n = "ada"` into a `Vec[String]` - had no node, so D17's
    // binding stayed a view and the push was refused.
    let text = decided.text.or_else(|| {
        (!lets.is_empty()).then(|| crate::ast::Type {
            name: parsed.interner.intern_string("String"),
            generics: Vec::new(),
            is_view: false,
            is_tuple: false,
            is_nullable: false,
            code: Box::new(None),
            count: None,
            is_mut: false,
            is_slice: false,
            either: false,
        })
    });
    rewrite(parsed, &tiers, &lets, text, &wraps, into);
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
    wraps: &BTreeMap<usize, usize>,
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
        let at = stmt.span.at();
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

pub(crate) fn visit_expr_mut(
    expr: &mut Expr,
    f: &mut dyn FnMut(&mut Expr),
    s: &mut dyn FnMut(&mut Stmt, usize),
) {
    match expr {
        Expr::Call {
            func, args, config, ..
        } => {
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
        | Expr::Try(expr, _)
        | Expr::Throw(expr, _)
        | Expr::Cast { expr, .. } => visit_expr_mut(expr, f, s),
        Expr::Return(value, _) => {
            if let Some(value) = &mut **value {
                visit_expr_mut(value, f, s)
            }
        }
        Expr::Field { base, .. } | Expr::SafeField { base, .. } => visit_expr_mut(base, f, s),
        Expr::Index { base, index, .. } => {
            visit_expr_mut(base, f, s);
            visit_expr_mut(index, f, s);
        }
        Expr::Range { start, end, .. } => {
            visit_expr_mut(start, f, s);
            visit_expr_mut(end, f, s);
        }
        Expr::Tuple(items, _) | Expr::ListLit { items, .. } => {
            items.iter_mut().for_each(|i| visit_expr_mut(i, f, s))
        }
        Expr::Coalesce {
            value, fallback, ..
        } => {
            visit_expr_mut(value, f, s);
            visit_expr_mut(fallback, f, s);
        }
        Expr::TryCatch { expr, handler, .. } => {
            visit_expr_mut(expr, f, s);
            visit_block_mut(handler, f, s);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            visit_expr_mut(cond, f, s);
            visit_block_mut(then_branch, f, s);
            if let Some(block) = else_branch {
                visit_block_mut(block, f, s);
            }
        }
        Expr::Match { value, arms, .. } => {
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
        Expr::Block(block, _) | Expr::Unsafe(block, _) | Expr::Overlap(block, _) => {
            visit_block_mut(block, f, s)
        }
        Expr::Closure { body, .. } => visit_block_mut(body, f, s),
        Expr::Spawn { body, .. } => visit_expr_mut(body, f, s),
        _ => {}
    }
    f(expr);
}

/// **A value going into a mixed position is handed over as `value.into_either()`**
/// (ADR-282 D14): `EitherText` borrows a view and moves text of its own in,
/// and which it is the language below reads off the value's type.
fn wrap_item(item: &mut Item, wraps: &BTreeMap<usize, usize>, into: [winnow_grammar::Symbol; 6]) {
    if wraps.is_empty() {
        return;
    }
    // Found by the id the parser gave it (ADR-340 D4), and replaced where the
    // walk meets it: children are visited before their parent, so wrapping one
    // moves nothing another comparison is still waiting for, and the wrapper
    // takes an id of its own.
    each_block(item, &mut |block| {
        visit_block_mut(
            block,
            &mut |expr| {
                if let Some(hand) = wraps.get(&crate::check::value_node(expr)) {
                    wrapped(expr, into[*hand]);
                }
            },
            &mut |_, _| {},
        )
    });
}

/// `expr` becomes `expr.<method>()`: the value moves into the receiver's box
/// and the call is a node of its own.
fn wrapped(expr: &mut Expr, method: winnow_grammar::Symbol) {
    let value = std::mem::replace(expr, Expr::LitNull(crate::ast::NodeId::fresh()));
    *expr = Expr::MethodCall {
        receiver: Box::new(value),
        method,
        args: Vec::new(),
        config: Vec::new(),
        id: crate::ast::NodeId::fresh(),
    };
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

/// **Apply a hole's wraps to it** (ADR-229 D1): each value whose id was
/// recorded becomes `value.<method>()`, as [`wrap_item`] does to the program.
/// The wrapper takes an id of its own and what it wraps keeps its own, so a
/// reader of the copy finds what the checker recorded about the value.
pub fn wrap_hole(hole: &mut Expr, wraps: &[(usize, winnow_grammar::Symbol)]) {
    visit_expr_mut(
        hole,
        &mut |expr| {
            let id = crate::check::value_node(expr);
            if let Some((_, method)) = wraps.iter().find(|(wanted, _)| *wanted == id) {
                wrapped(expr, *method);
            }
        },
        &mut |_, _| {},
    );
}
