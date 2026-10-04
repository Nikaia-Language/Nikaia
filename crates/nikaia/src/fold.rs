//! What a constant integer expression comes to, for the two passes that ask.
//!
//! The arithmetic used to live in the checker, where it answers *"does this fit
//! the type beside it?"* ([`check`](crate::check)'s `NK1116` and `NK1118`). The
//! emitter now asks a second question of the same expression - *"is the first
//! type that holds this an `i64`?"*
//! ([ADR-285](../../../docs/specification/adr/adr-285.md)) - and two constant
//! folds in one compiler is one fold and one liability.
//!
//! **What separates the two callers is not the arithmetic, it is the lookup.**
//! The checker knows what a name is worth; the emitter has no scope and no
//! types ([ADR-288](../../../docs/specification/adr/adr-288.md)). So the lookup
//! is the parameter, and the emitter passes [`nothing_is_known`] - which gives
//! it exactly the subset of expressions that can be answered without looking
//! anything up.
//!
//! **The fold is Nikaia** (`nikaia-std/src/tools/fold.nika`,
//! [ADR-294](../../../docs/specification/adr/adr-294.md) D11): a magnitude and
//! a sign, 65 bits, held at every step to the type a declared operand pinned.
//! This module is its adapter: the checker keeps a constant in an `i128`, so
//! the answer is turned into one here, and a fold that left the 65 bits is a
//! number one past `u64::MAX` - which no type holds, and which the checker
//! says as *more than* the widest.
//!
//! **Every step is `checked_`, and `None` means nothing is claimed.** A name the
//! lookup cannot evaluate, an operator this does not fold, a division by a
//! constant zero, a fold that leaves the `i128` - each one stops the whole
//! expression. An expression that does not fold is never refused, which is the
//! polarity both callers are held to (Part III, C.4): the compiler may fail to
//! refuse a program `rustc` will, and may never refuse one that is right.

use winnow_grammar::Symbol;

use crate::ast::Expr;
use nikaia_std::tools::fold as nika;

/// What a constant integer expression came to, and the type an operand's
/// declaration pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constant {
    /// A number the checker can compare against a type's range. A fold that
    /// left the 65 bits every literal fits in is one past `u64::MAX`, with its
    /// sign.
    pub value: i128,
    /// The integer type an operand's *declaration* fixed, where one did. **A
    /// literal pins nothing**: `3000000000` is an `i64` wherever a use asks for
    /// one (Part I 2.4), which is why a literal standing alone may not be
    /// refused - and why, with nothing pinned and nothing beside it, it is free
    /// to take the wider type ([ADR-285](../../../docs/specification/adr/adr-285.md)).
    pub pinned: Option<String>,
}

/// The magnitude of a number no integer type holds: one past `u64::MAX`.
pub const BEYOND: i128 = u64::MAX as i128 + 1;

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
///
/// A step that left the type a declared operand pinned is answered with that
/// step's number, so the caller refuses it where the language below would.
pub fn constant_of(expr: &Expr, name_is: Lookup<'_>) -> Option<Constant> {
    let lookup = |name: Symbol| match name_is(name).as_ref().and_then(to_nikaia) {
        Some(known) => nika::Folded::Value(known),
        None => nika::Folded::Nothing,
    };
    match nika::constant_of(expr, &lookup) {
        nika::Folded::Value(c) | nika::Folded::Overflows(c) => Some(from_nikaia(c)),
        nika::Folded::TooLarge(negative) => Some(Constant {
            value: if negative { -BEYOND } else { BEYOND },
            pinned: None,
        }),
        nika::Folded::Nothing => None,
    }
}

/// **Does this constant need the wider type, and may it have it?**
/// ([ADR-285](../../../docs/specification/adr/adr-285.md) D22.)
///
/// Nothing pinned it, an `i32` does not hold it, and an `i64` does - the fold's
/// own answer, asked of the number the checker keeps.
pub fn wants_widening(folded: &Constant) -> bool {
    to_nikaia(folded).is_some_and(|c| nika::wants_widening(&nika::Folded::Value(c)))
}

/// An `i128` as the fold counts: `None` past the 65 bits.
fn to_nikaia(c: &Constant) -> Option<nika::Constant> {
    Some(nika::Constant {
        value: nikaia_std::tools::integers::integer(
            u64::try_from(c.value.unsigned_abs()).ok()?,
            c.value < 0,
        ),
        pinned: c.pinned.clone().unwrap_or_default(),
    })
}

fn from_nikaia(c: nika::Constant) -> Constant {
    Constant {
        value: crate::build_time::integer_value(&c.value),
        pinned: (!c.pinned.is_empty()).then_some(c.pinned),
    }
}
