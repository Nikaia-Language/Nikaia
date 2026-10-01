//! Where a buffer lives once views of it outlive the scope that made it
//! ([ADR-209](../../../docs/specification/adr/adr-209.md), Part I 6.6).
//!
//! The crate `tether` (`crates/unsafe/tether`) holds it, with every `unsafe`
//! it takes and the argument for each ([ADR-218](../../../docs/specification/adr/adr-218.md)).
//! Generated code reaches it here, under the name it has always had.

pub use ::tether::{Dangling, Held, Hold, Holding, Keep, Keeps, Rebase, Viewed, Views};
