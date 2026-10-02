// crates/nikaia/src/types.rs
//
// Part I 2.2 and Part III C.1: **a type nothing declares is refused here rather
// than lowered** ([ADR-096](../../../docs/specification/adr/adr-096.md)).
//
// A *value* nothing declares has had `NK1117` since
// [ADR-051](../../../docs/specification/adr/adr-051.md) — *"nothing declares
// `q`"*. A type had nothing, so `let x: Widgit = 3` lowered verbatim and came
// back as `rustc`'s *"cannot find type `Widgit` in this scope"*, about a file
// nobody wrote. Part III C.1's rule held for one half of this language's names
// and not the other.
//
// **A walk of its own**, beside `dsl::check`, `views::check` and
// `traits::check`, and for the reason each of those is separate: it asks a
// question about a *written name* rather than about a value's type, so it needs
// the item tree and neither the scope stack nor the inference.

use crate::ast::Item;
use crate::check::Finding;
use crate::contracts::Ledger;
use crate::parser::Parsed;

/// Every name declared twice (`NK1148`), every bound naming no trait and
/// every written type nothing declares (`NK1135`, `NK1158`, `NK1182`).
/// **Written in Nikaia** (`tools/types.nika`, #125): what the file declares
/// as types and what an `impl`'s target makes a type parameter are
/// `contracts`' answers, handed in.
pub fn check(parsed: &Parsed, own: &Ledger, library: &Ledger) -> Vec<Finding> {
    let declared = crate::contracts::declared_types(parsed);
    let impl_parameters: Vec<Vec<String>> = parsed
        .program
        .items
        .iter()
        .map(|item| match &item.node {
            Item::Impl { target, .. } => {
                crate::contracts::impl_parameters(parsed, target, &declared)
            }
            _ => Vec::new(),
        })
        .collect();
    nikaia_std::tools::types::types_checked(
        &parsed.interner,
        &parsed.program.items,
        own,
        library,
        &declared,
        &impl_parameters,
    )
    .into_iter()
    .map(crate::traits::from_nikaia)
    .collect()
}

/// **The two bounds that ask what a type *is*** — Part II 10.3 and
/// [ADR-088](../../../docs/specification/adr/adr-088.md) D2.
///
/// `[T: Struct]` and `[T: Enum]` are not traits anybody declares and no
/// `impl` answers them: what answers is the **declaration**, which is the whole
/// of D2's *the bound is what makes the shape reachable*. They are language
/// words, like `sync` and `throws`, and they live in one constant because the
/// checker, the bound check and the emitter each have to know the same two
/// names — the emitter because Rust has no such trait, so a bound that reached
/// it would come back as *cannot find trait `Struct` in this scope*, about a
/// file nobody wrote ([Part III
/// C.1](../../../docs/specification/30-nikaia-tooling.md)).
///
/// **A program that declares one wins.** `trait Struct { … }` beside this is
/// an ordinary trait and every check reads the declaration first, so nothing
/// here takes a name away from a program that wanted it.
pub const SHAPE_BOUNDS: [&str; 2] = ["Struct", "Enum"];
