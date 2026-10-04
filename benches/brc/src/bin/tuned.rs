//! What a 1BRC entry looks like before it is parallelised: `mmap`, raw bytes
//! with no UTF-8 validation, one `memchr` searcher over the whole file, the
//! separator found by walking back from the end of the line, integer
//! temperatures and a word-at-a-time hash. The program `examples/1brc.nika` is
//! measured against; see `benches/brc/README.md`.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("a measurements file is required");
    tuned(&path);
}

/// `rustc-hash`'s mixer, written out rather than depended on.
///
/// The station name is the key, it is short, and the default `SipHash` costs
/// about a quarter of the whole program when it is the only thing between a
/// line and its slot. A 1BRC entry replaces it; so does this.
#[derive(Default)]
struct Fx(u64);

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl Fx {
    #[inline]
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(SEED);
    }
}

impl Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, mut bytes: &[u8]) {
        while bytes.len() >= 8 {
            self.add(u64::from_ne_bytes(
                bytes[..8].try_into().expect("eight bytes"),
            ));
            bytes = &bytes[8..];
        }
        if !bytes.is_empty() {
            let mut buf = [0u8; 8];
            buf[..bytes.len()].copy_from_slice(bytes);
            self.add(u64::from_ne_bytes(buf));
        }
    }
}

/// What a 1BRC entry looks like before it is parallelised.
fn tuned(path: &str) {
    let file = std::fs::File::open(path).expect("open the measurements");
    // SAFETY: the mapping is read-only and private, and the one thing no type
    // system rules out - another process rewriting the file underneath it - is
    // the documented caveat of every memory map. `std::fs::map` accepts it on
    // the same terms (ADR-016).
    let map = unsafe { memmap2::Mmap::map(&file).expect("map the measurements") };
    let data: &[u8] = &map;

    let mut stats: HashMap<&[u8], [i64; 4], BuildHasherDefault<Fx>> = HashMap::default();

    // One searcher for the whole file rather than one per line. The separator
    // is then found by walking *back* from the end of the line, because the
    // format fixes the temperature at no more than five characters - the same
    // fact the example's grammar writes down as `"-"? digit{1,2} "." digit`.
    let mut start = 0usize;
    for end in memchr::memchr_iter(b'\n', data) {
        let line = &data[start..end];
        start = end + 1;

        let mut sep = line.len() - 1;
        while line[sep] != b';' {
            sep -= 1;
        }
        let name = &line[..sep];

        let mut t = &line[sep + 1..];
        let neg = t[0] == b'-';
        if neg {
            t = &t[1..];
        }
        // `dd.d` or `d.d`: the two widths the format allows, branchlessly
        // enough that the compiler keeps both in registers.
        let v: i64 = if t.len() == 4 {
            (t[0] - b'0') as i64 * 100 + (t[1] - b'0') as i64 * 10 + (t[3] - b'0') as i64
        } else {
            (t[0] - b'0') as i64 * 10 + (t[2] - b'0') as i64
        };
        let v = if neg { -v } else { v };

        let e = stats.entry(name).or_insert([i64::MAX, i64::MIN, 0, 0]);
        if v < e[0] {
            e[0] = v
        }
        if v > e[1] {
            e[1] = v
        }
        e[2] += v;
        e[3] += 1;
    }

    let mut names: Vec<&[u8]> = stats.keys().copied().collect();
    names.sort_unstable();
    report(names.into_iter().map(|n| {
        let s = stats[n];
        // The only place this half looks at the text as text, and the only
        // place it could fail on input `fs::map` would have rejected outright.
        let name = std::str::from_utf8(n).expect("a station name is text");
        (name, s[0], s[1], s[2], s[3])
    }));
}

/// The one line the benchmark asks for. Shared, so that a difference in the
/// output is never a difference in how it was printed.
fn report<'a>(rows: impl Iterator<Item = (&'a str, i64, i64, i64, i64)>) {
    let body: Vec<String> = rows
        .map(|(name, min, max, sum, count)| {
            format!(
                "{}={:.1}/{:.1}/{:.1}",
                name,
                min as f64 / 10.0,
                (sum as f64 / count as f64) / 10.0,
                max as f64 / 10.0
            )
        })
        .collect();
    println!("{{{}}}", body.join(", "));
}
