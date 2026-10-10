// crates/nikaia/src/ast/mod.rs
//
// **The syntax tree is declared in Nikaia** (ADR-294 D6):
// `nikaia-std/src/tools/ast.nika`, lowered to `ast.rs` beside it. This module
// re-exports it, so every reader keeps writing `crate::ast::Expr`, and adds
// what the compiler needs that the language does not say - a literal's number
// as an `i128`, which no Nikaia number holds.

pub use nikaia_std::node_id::{FIRST_BUILT, NodeId};
pub use nikaia_std::tools::ast::*;

/// The number an integer literal says, wide enough for every one of them.
pub fn int_value(value: u64, negative: bool) -> i128 {
    if negative {
        -i128::from(value)
    } else {
        i128::from(value)
    }
}

/// The literal that says `n`. The parser reads only numbers ADR-285 D19
/// allows, from `-9223372036854775808` to `18446744073709551615`, so the
/// magnitude fits a `u64`.
pub fn int_literal(n: i128) -> Expr {
    Expr::LitInt {
        value: u64::try_from(n.unsigned_abs()).expect("a literal is as large as a u64 holds"),
        negative: n < 0,
        id: crate::ast::NodeId::fresh(),
    }
}

/// Gives the node a new id: how a clone placed a second time in one tree stops
/// being the same node as the first ([ADR-340](../../../docs/specification/adr/adr-340.md) D2).
pub fn set_id(expr: &mut Expr, id: NodeId) {
    match expr {
        Expr::LitInt { id: slot, .. }
        | Expr::Match { id: slot, .. }
        | Expr::Range { id: slot, .. }
        | Expr::ListLit { id: slot, .. }
        | Expr::LitStr { id: slot, .. }
        | Expr::LitInterpolated { id: slot, .. }
        | Expr::If { id: slot, .. }
        | Expr::Call { id: slot, .. }
        | Expr::Spawn { id: slot, .. }
        | Expr::Dsl { id: slot, .. }
        | Expr::MethodCall { id: slot, .. }
        | Expr::Field { id: slot, .. }
        | Expr::StructLit { id: slot, .. }
        | Expr::With { id: slot, .. }
        | Expr::Closure { id: slot, .. }
        | Expr::Unary { id: slot, .. }
        | Expr::Binary { id: slot, .. }
        | Expr::SafeField { id: slot, .. }
        | Expr::SafeMethod { id: slot, .. }
        | Expr::Index { id: slot, .. }
        | Expr::Cast { id: slot, .. }
        | Expr::Coalesce { id: slot, .. }
        | Expr::Asm { id: slot, .. }
        | Expr::TryCatch { id: slot, .. }
        | Expr::Tuple(_, slot)
        | Expr::LitChar(_, slot)
        | Expr::LitBool(_, slot)
        | Expr::Variable(_, slot)
        | Expr::Block(_, slot)
        | Expr::Overlap(_, slot)
        | Expr::Select(_, slot)
        | Expr::Path(_, slot)
        | Expr::Try(_, slot)
        | Expr::Throw(_, slot)
        | Expr::Return(_, slot)
        | Expr::LitFloat(_, slot)
        | Expr::Unsafe(_, slot)
        | Expr::LitNull(slot)
        | Expr::Break(slot)
        | Expr::Continue(slot) => *slot = id,
    }
}
