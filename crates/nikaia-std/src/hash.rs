//! The hash a map gets when nobody outside the program chose its keys.
//!
//! [ADR-010](../../../docs/specification/adr/adr-010.md) D5: provenance selects
//! a hash function and nothing else. Untrusted keys get the keyed, randomly
//! seeded hash that `std` gives every map by default; trusted keys get this one
//! - fast, fixed seed, not cryptographic and not trying to be.
//!
//! Two shapes of key reach it by two paths, and each has the function that was
//! measured best for it ([ADR-316](../../../docs/specification/adr/adr-316.md)
//! D5). A number, a `char`, a `bool` arrive through `write_u64` and its
//! siblings and are one Fx step each: the hash `rustc` used on its own tables
//! for years. Text arrives through `write`, at any length, and is first folded
//! into one word by the byte hash of `rustc-hash` 2 (`hash_bytes` below), which
//! reads a short key as two overlapping words instead of branching on its
//! length piece by piece. On 1BRC's names that halved the mispredictions a row
//! (3.42 to 1.62) and took 19 instructions off it; on `k-nucleotide`'s keys of
//! one length it cut 7 % of the instructions.
//!
//! It is here rather than behind a dependency for two reasons: it sits on the
//! hot path of every Nikaia program that aggregates anything, and a program
//! whose *hash function* comes from a crate it did not choose is a program
//! whose iteration order and worst case can change under it.
//!
//! What it is not: a defence against chosen keys. That is exactly why the
//! compiler picks between the two rather than making this the default, and why
//! a barrier in the provenance analysis widens to untrusted rather than to
//! here.

use std::hash::{BuildHasherDefault, Hasher};

/// A `HashMap` for keys the operator chose.
pub type TrustedMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<FxHasher>>;

/// A `HashSet` for keys the operator chose.
pub type TrustedSet<K> = std::collections::HashSet<K, BuildHasherDefault<FxHasher>>;

/// The constant `rustc`'s hash multiplies by: the 64-bit odd number closest to
/// `2^64 / φ`, so that the multiply spreads a change in any bit across the
/// whole word.
const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

/// A fast, non-cryptographic hash with a fixed seed.
///
/// One multiply and one rotate per number written, and one more for text once
/// `hash_bytes` has made a word of it; no allocation, no state beyond the
/// accumulator. Every write folds into the same accumulator, so the order of
/// the parts of a compound key matters - which is what a hash of a tuple has to
/// promise.
#[derive(Default, Clone)]
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.add(hash_bytes(bytes));
    }

    #[inline]
    fn write_u8(&mut self, n: u8) {
        self.add(n as u64);
    }

    #[inline]
    fn write_u16(&mut self, n: u16) {
        self.add(n as u64);
    }

    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.add(n as u64);
    }

    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

// `hash_bytes` and what it calls are `rustc-hash` 2.1.1's (`src/lib.rs`),
// MIT or Apache-2.0, by Orson Peters; the 64-bit half only, since `nikaia-std`
// targets 64-bit. Copied rather than depended on, for the reasons at the top.

// Digits of pi.
const SEED1: u64 = 0x243f_6a88_85a3_08d3;
const SEED2: u64 = 0x1319_8a2e_0370_7344;
const PREVENT_TRIVIAL_ZERO_COLLAPSE: u64 = 0xa409_3822_299f_31d0;

/// The full 64 x 64 -> 128 product, its halves xor-ed: the middle bits, which
/// move most with a small change in either input, end up everywhere.
#[inline]
fn multiply_mix(x: u64, y: u64) -> u64 {
    let full = (x as u128) * (y as u128);
    (full as u64) ^ ((full >> 64) as u64)
}

#[inline]
fn word(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes[..8].try_into().expect("eight"))
}

#[inline]
fn half(bytes: &[u8]) -> u64 {
    u32::from_le_bytes(bytes[..4].try_into().expect("four")) as u64
}

/// Any number of bytes as one word.
///
/// Up to 16 bytes, the key is read as its first and its last 8 (or 4) bytes,
/// which overlap where the key is shorter than both: one branch on the length's
/// class rather than one per piece of the tail. Longer keys are read 16 bytes a
/// step, and the last 16 overlap the step before. The length is xor-ed in at
/// the end, so two lengths of one overlapping pattern do not collide.
#[inline]
fn hash_bytes(bytes: &[u8]) -> u64 {
    let len = bytes.len();
    let mut s0 = SEED1;
    let mut s1 = SEED2;
    if len <= 16 {
        if len >= 8 {
            s0 ^= word(bytes);
            s1 ^= word(&bytes[len - 8..]);
        } else if len >= 4 {
            s0 ^= half(bytes);
            s1 ^= half(&bytes[len - 4..]);
        } else if len > 0 {
            s0 ^= bytes[0] as u64;
            s1 ^= ((bytes[len - 1] as u64) << 8) | bytes[len / 2] as u64;
        }
    } else {
        let mut off = 0;
        while off < len - 16 {
            let x = word(&bytes[off..]);
            let y = word(&bytes[off + 8..]);
            // Two independent streams, s0 and s1, so the loop unrolls; the
            // constant keeps a run of zeros from collapsing the state.
            let t = multiply_mix(s0 ^ x, PREVENT_TRIVIAL_ZERO_COLLAPSE ^ y);
            s0 = s1;
            s1 = t;
            off += 16;
        }
        let suffix = &bytes[len - 16..];
        s0 ^= word(suffix);
        s1 ^= word(&suffix[8..]);
    }
    multiply_mix(s0, s1) ^ len as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn hash_of(bytes: &[u8]) -> u64 {
        let mut h = FxHasher::default();
        h.write(bytes);
        h.finish()
    }

    /// A hash function is only useful if the same key hashes the same way every
    /// time, in the same process and the next one. That is what "fixed seed"
    /// means and it is the whole difference from the map `std` gives by
    /// default.
    #[test]
    fn the_same_key_hashes_the_same_way() {
        assert_eq!(hash_of(b"Hamburg"), hash_of(b"Hamburg"));
        assert_ne!(hash_of(b"Hamburg"), hash_of(b"Bulawayo"));
    }

    /// Order matters inside a key: `ab` is not `ba`, and a compound key's parts
    /// do not commute.
    #[test]
    fn the_order_of_the_bytes_matters() {
        assert_ne!(hash_of(b"ab"), hash_of(b"ba"));

        let mut first = FxHasher::default();
        first.write_u32(1);
        first.write_u32(2);
        let mut second = FxHasher::default();
        second.write_u32(2);
        second.write_u32(1);
        assert_ne!(first.finish(), second.finish());
    }

    /// Every length is reached: a key one byte longer hashes differently,
    /// across the short reads (1-3, 4-7, 8-16 bytes) and the 16-byte steps.
    /// The empty key and the single zero byte differ too, since the length is
    /// mixed in. Not injectivity - a 64-bit non-cryptographic hash does not
    /// promise that, and a map compares keys in full (ADR-010 D5).
    #[test]
    fn every_length_is_covered() {
        let mut seen = std::collections::HashSet::new();
        for len in 0..80usize {
            let key: Vec<u8> = (0..len).map(|i| (i % 251 + 1) as u8).collect();
            assert!(seen.insert(hash_of(&key)), "collision at length {len}");
        }
        assert_ne!(hash_of(b""), hash_of(&[0]));
    }

    /// The overlapping reads see one pattern at two lengths, and still tell
    /// them apart: a run of one byte, every length up to 40.
    #[test]
    fn a_repeated_byte_at_every_length_differs() {
        let mut seen = std::collections::HashSet::new();
        for len in 0..40usize {
            assert!(seen.insert(hash_of(&vec![b'a'; len])), "length {len}");
        }
    }

    /// The copy answers as `rustc-hash` 2 does, read back out of its hasher:
    /// from a zero state, one `write` leaves `hash_bytes(b) * K` rotated left
    /// by 26, so undoing the rotation and the multiply gives `hash_bytes(b)`.
    #[test]
    fn hash_bytes_is_the_one_copied() {
        const K: u64 = 0xf135_7aea_2e62_a9c5;
        // K is odd, so it has an inverse mod 2^64: Newton's iteration doubles
        // the correct bits each step, from the 3 that `K` itself gets right.
        let mut inverse = K;
        for _ in 0..5 {
            inverse = inverse.wrapping_mul(2u64.wrapping_sub(K.wrapping_mul(inverse)));
        }
        assert_eq!(K.wrapping_mul(inverse), 1);
        for len in 0..80usize {
            let key: Vec<u8> = (0..len).map(|i| (i * 37 % 256) as u8).collect();
            let mut theirs = rustc_hash::FxHasher::default();
            theirs.write(&key);
            let back = theirs.finish().rotate_right(26).wrapping_mul(inverse);
            assert_eq!(back, hash_bytes(&key), "length {len}");
        }
    }

    /// What it is for: a table that answers, over the keys a data format
    /// actually has.
    #[test]
    fn it_works_as_a_map_hasher() {
        let mut map: TrustedMap<&str, i32> = TrustedMap::default();
        for (i, name) in ["Hamburg", "Bulawayo", "Palembang", "St. John's", "東京"]
            .iter()
            .enumerate()
        {
            map.insert(name, i as i32);
        }
        assert_eq!(map.len(), 5);
        assert_eq!(map["東京"], 4);
        assert_eq!(map.get("nowhere"), None);

        // …and it agrees with the map it stands in for.
        let plain: HashMap<&str, i32> = map.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(plain.len(), map.len());
    }

    /// The keys of a real aggregation, spread over a table of the size one
    /// would have: no bucket may hold a tenth of them.
    #[test]
    fn it_spreads_the_keys_of_an_aggregation() {
        let keys: Vec<String> = (0..413).map(|i| format!("Station{i:03}")).collect();
        let buckets = 1024u64;
        let mut counts = vec![0usize; buckets as usize];
        for key in &keys {
            counts[(hash_of(key.as_bytes()) % buckets) as usize] += 1;
        }
        let worst = counts.iter().copied().max().expect("non-empty");
        assert!(worst < keys.len() / 10, "worst bucket held {worst}");
    }
}
