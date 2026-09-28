//! What a constant integer expression comes to, for the two passes that ask.
//!
//! The arithmetic used to live in the checker, where it answers *"does this fit
//! the type beside it?"* ([`check`](crate::check)'s `NK1116` and `NK1118`). The
//! emitter now asks a second question of the same expression - *"is the first
//! type that holds this an `i64`?"*
//! ([ADR-063](../../../docs/specification/adr/adr-063.md)) - and two constant
//! folds in one compiler is one fold and one liability.
//!
//! **What separates the two callers is not the arithmetic, it is the lookup.**
//! The checker knows what a name is worth; the emitter has no scope and no
//! types ([ADR-028](../../../docs/specification/adr/adr-028.md)). So the lookup
//! is the parameter, and the emitter passes [`nothing_is_known`] - which gives
//! it exactly the subset of expressions that can be answered without looking
//! anything up.
//!
//! **Folded in an `i128`**, so a sum that cannot fit an `i64` is a number the
//! caller can name rather than one this wrapped: the fold must not do quietly
//! what it exists to report.
//!
//! **Every step is `checked_`, and `None` means nothing is claimed.** A name the
//! lookup cannot evaluate, an operator this does not fold, a division by a
//! constant zero, a fold that leaves the `i128` - each one stops the whole
//! expression. An expression that does not fold is never refused, which is the
//! polarity both callers are held to (Part III, C.4): the compiler may fail to
//! refuse a program `rustc` will, and may never refuse one that is right.

use winnow_grammar::Symbol;

use crate::ast::{BinaryOp, Expr, UnaryOp};

/// What a constant integer expression came to, and the type an operand's
/// declaration pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constant {
    /// Folded in an `i128` so a sum that cannot fit an `i64` is still a number
    /// the caller can name rather than one that wrapped.
    pub value: i128,
    /// The integer type an operand's *declaration* fixed, where one did. **A
    /// literal pins nothing**: `3000000000` is an `i64` wherever a use asks for
    /// one (Part I 2.4), which is why a literal standing alone may not be
    /// refused - and why, with nothing pinned and nothing beside it, it is free
    /// to take the wider type ([ADR-060](../../../docs/specification/adr/adr-060.md)).
    pub pinned: Option<String>,
}

/// What a name is worth, where the caller knows anything about it.
///
/// `None` for a name the caller cannot evaluate - which, for a caller that has
/// no scope at all, is every name ([`nothing_is_known`]).
pub type Lookup<'a> = &'a dyn Fn(Symbol) -> Option<Constant>;

/// The lookup of a pass that has no scope to look in.
///
/// It is a function and not an absent parameter so that the difference is
/// visible at the call: the emitter is not *skipping* the lookup, it has none,
/// and every name in an expression it folds stops that fold.
pub fn nothing_is_known(_: Symbol) -> Option<Constant> {
    None
}

/// The value a constant integer expression comes to, or `None` where nothing is
/// claimed about it.
pub fn constant_of(expr: &Expr, name_is: Lookup<'_>) -> Option<Constant> {
    match expr {
        Expr::LitInt(value) => Some(Constant {
            value: *value,
            pinned: None,
        }),
        Expr::Variable(name) => name_is(*name),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
        } => {
            let inner = constant_of(expr, name_is)?;
            Some(Constant {
                value: inner.value.checked_neg()?,
                pinned: inner.pinned,
            })
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let lhs = constant_of(lhs, name_is)?;
            let rhs = constant_of(rhs, name_is)?;
            // Two operands that pin different types are a mismatch the type
            // check reports on its own; folding them would be arithmetic in a
            // type neither of them has.
            let pinned = match (&lhs.pinned, &rhs.pinned) {
                (Some(a), Some(b)) if a != b => return None,
                (Some(a), _) => Some(a.clone()),
                (_, pinned) => pinned.clone(),
            };
            let value = match op {
                BinaryOp::Add => lhs.value.checked_add(rhs.value)?,
                BinaryOp::Sub => lhs.value.checked_sub(rhs.value)?,
                BinaryOp::Mul => lhs.value.checked_mul(rhs.value)?,
                BinaryOp::Div => lhs.value.checked_div(rhs.value)?,
                BinaryOp::Rem => lhs.value.checked_rem(rhs.value)?,
                _ => return None,
            };
            Some(Constant { value, pinned })
        }
        _ => None,
    }
}

/// **Does this constant need the wider type, and may it have it?**
/// ([ADR-063](../../../docs/specification/adr/adr-063.md) D1.)
///
/// Three things have to hold, and each is a different half of the rule:
///
/// * **nothing pinned it.** `let a: i32 = 2` then `a + a` is arithmetic in an
///   `i32` because a declaration said so, and widening it would be this
///   compiler quietly choosing a type over the one written down;
/// * **an `i32` does not hold it**, so there is a reason to move at all - a
///   value that fits stays untouched, which is what keeps every program that
///   compiles today compiling (Part I 2.4);
/// * **an `i64` does hold it.** A value past that has no second answer either,
///   and it is refused rather than widened - by `NK1116`, in this language's
///   words.
pub fn wants_widening(folded: &Constant) -> bool {
    folded.pinned.is_none()
        && i32::try_from(folded.value).is_err()
        && i64::try_from(folded.value).is_ok()
}
