//! The table a `comptime` map crosses as
//! ([ADR-176](../../../docs/specification/adr/adr-176.md) D2, D3).
//!
//! **Two shapes under one type**, and which one a table gets was decided while
//! the program was built rather than by a branch in `std`:
//!
//! * **Under twelve keys**, no displacements. `Fixed::get` walks the keys with a
//!   length check, which [`docs/history/fixed-map-lookup.md`](../../../docs/history/fixed-map-lookup.md)
//!   §6 measured within 0 to 17 % of a generated `match` — past the point where
//!   the hash has already won, so the `match` this would otherwise have emitted
//!   buys nothing.
//! * **From twelve**, CHD: buckets by one part of the hash, a displacement pair
//!   per bucket, one slot per key. The lookup is one hash, one displacement, one
//!   slot and one compare.
//!
//! Twelve is where the two cross in that file, and the band is forgiving —
//! anywhere from eight to sixteen is within 30 % on the wrong side — which is
//! worth saying so nobody re-measures this to move it by two.

/// Where a scan stops being as good as a hash
/// ([ADR-176](../../../docs/specification/adr/adr-176.md) D3).
pub const HASHED_FROM: usize = nikaia_std::tools::fixed::HASHED_FROM as usize;

/// A table, ready to be written into a `const`.
pub struct Table {
    pub seed: u64,
    /// Empty for a small table, which is read by walking it.
    pub disps: Vec<(u32, u32)>,
    /// In slot order where there are displacements, in written order where
    /// there are not.
    pub keys: Vec<String>,
    /// Beside `keys`, element for element.
    pub order: Vec<usize>,
}

/// The same hash `nikaia_std::fixed` computes, over the same bytes.
///
/// **Written in Nikaia** (0.0.250): `nikaia-std/src/tools/fixed.nika`, the
/// program [ADR-248](../../../docs/specification/adr/adr-248.md) was decided
/// for. It is still two implementations of one function - that one and the
/// lookup `nikaia_std::fixed` runs - and what holds them together is not care
/// but `crates/nikaia/tests/fixed_map.rs`, which **runs** a program over both
/// table shapes.
pub fn fnv(key: &str, seed: u64) -> u64 {
    nikaia_std::tools::fixed::fnv(key, seed)
}

/// Build the table for these keys, in the order they were written - CHD from
/// twelve keys, the keys as written under that
/// (`nikaia-std/src/tools/fixed.nika`, which says how).
///
/// `None` where no seed worked, which is reported rather than looped on.
pub fn build(keys: &[String]) -> Option<Table> {
    let table = nikaia_std::tools::fixed::build(keys)?;
    Some(Table {
        seed: table.seed,
        disps: table.disps,
        keys: table.keys,
        order: table
            .order
            .into_iter()
            .map(|at| usize::try_from(at).expect("a slot holds a key's place"))
            .collect(),
    })
}
