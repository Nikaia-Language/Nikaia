// crates/nikaia/src/ast/mod.rs
//
// **The syntax tree is declared in Nikaia** (ADR-294 D6):
// `nikaia-std/src/tools/ast.nika`, lowered to `ast.rs` beside it. This module
// re-exports it, so every reader keeps writing `crate::ast::Expr`, and adds
// what the compiler needs that the language does not say - a literal's number
// as an `i128`, which no Nikaia number holds.

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
    }
}
