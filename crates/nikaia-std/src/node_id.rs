//! **The id every `Expr` node carries** ([ADR-340](../../../docs/specification/adr/adr-340.md),
//! #558): what the compiler's tables are keyed on where Rust once keyed them on
//! an expression's address.
//!
//! A `u32` the parser gives when it makes the node (D1), below
//! [`FIRST_BUILT`]; a node the compiler builds after parsing takes one from
//! [`NodeId::fresh`], from [`FIRST_BUILT`] up, so no two nodes of any tree
//! share one (D2). A clone keeps its id.
//!
//! **Two ids are always equal** (D3): the id is identity, not value, so two
//! trees that differ only in their ids compare equal and `Expr` keeps its
//! derived `PartialEq`. A table is keyed on [`NodeId::get`], the number, never
//! on the `NodeId`, whose `==` says nothing about which node it is.

use std::sync::atomic::{AtomicU32, Ordering};

/// The first id a node the compiler builds takes. The parser gives the ids
/// below it, file by file.
pub const FIRST_BUILT: u32 = 1 << 31;

static BUILT: AtomicU32 = AtomicU32::new(FIRST_BUILT);

/// One expression node's id.
#[derive(Clone, Copy)]
pub struct NodeId(u32);

impl NodeId {
    /// The id with this number: what the parser gives, in the order it makes
    /// its nodes.
    pub fn numbered(number: u32) -> NodeId {
        NodeId(number)
    }

    /// An id no other node has, for a node made after parsing.
    pub fn fresh() -> NodeId {
        NodeId(BUILT.fetch_add(1, Ordering::Relaxed))
    }

    /// The number a table is keyed on.
    pub fn get(self) -> u32 {
        self.0
    }
}

/// D3: the id is identity, not value.
impl PartialEq for NodeId {
    fn eq(&self, _: &NodeId) -> bool {
        true
    }
}

impl Eq for NodeId {}

/// **Printed as `NodeId`, and with its number only in the pretty form**
/// (`{:#?}`): the compiler tells two arguments apart by their `{:?}` text
/// (`check::argument_shape`), and a number in it would make two copies of one
/// expression differ, which is D3 turned around.
impl std::fmt::Debug for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, "NodeId({})", self.0)
        } else {
            f.write_str("NodeId")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_ids_are_equal_and_their_numbers_are_not() {
        assert_eq!(NodeId::numbered(1), NodeId::numbered(2));
        assert_ne!(NodeId::numbered(1).get(), NodeId::numbered(2).get());
    }

    #[test]
    fn a_fresh_id_is_new_every_time_and_above_the_parser() {
        let first = NodeId::fresh().get();
        let second = NodeId::fresh().get();
        assert!(first >= FIRST_BUILT);
        assert_ne!(first, second);
    }
}
