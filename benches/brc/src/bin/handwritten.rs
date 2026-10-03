//! The hand-written ceiling for `examples/1brc.nika`.
//!
//! The example claims that a grammar which describes a whole file, lowered by
//! this compiler, is worth writing instead of the loop you would write by
//! hand. That claim is only worth as much as the hand-written loop it is
//! measured against, so here are two of them — and the second is the one that
//! matters, because beating a straw man proves nothing.
//!
//! | shape | what it is |
//! |---|---|
//! | `naive` | what a competent Rust programmer writes first: `read_to_string`, `str::lines`, `split_once`, `HashMap<&str, _>` with the default hasher |
//! | `tuned` | what a 1BRC entry looks like before it is parallelised: `mmap`, raw bytes with no UTF-8 validation, one `memchr` searcher over the whole file, the separator found by walking *back* from the end of the line, integer temperatures, a word-at-a-time hash |
//!
//! Both are a **pair** with `examples/1brc.nika` in the sense the other
//! benches here use: everything differs except the aggregation and the output,
//! and the output is compared byte for byte, so a difference in speed is never
//! a difference in what was computed.
//!
//! `tuned` is allowed one thing the compiler is not: it never validates UTF-8.
//! `std::fs::map` must (ADR-016 D1) — the `&str` views a grammar cuts out of a
//! mapping depend on it — and that difference is a real part of the gap rather
//! than an unfairness to be corrected.
//!
//! The binary is `handwritten`, not `brc`: `examples/1brc.nika` is built as a
//! project called `brc`, and two binaries of that name in one comparison is
//! one too many.
//!
//! ```sh
//! benches/brc/brc.sh                                      # the table
//! cargo run -p brc-bench --release --bin handwritten -- gen 8000000 m.txt
//! cargo run -p brc-bench --release --bin handwritten -- naive m.txt
//! cargo run -p brc-bench --release --bin handwritten -- tuned m.txt
//! ```
//!
//! The absolute numbers do not travel — `docs/history/runtime-cost.md` §6.3
//! has this box moving by 1.4–1.9× from one day to the next — so the script
//! prints the machine with the table and the README quotes ratios.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    match mode.as_str() {
        "naive" => naive(&path(args.next())),
        "tuned" => tuned(&path(args.next())),
        "gen" => {
            let rows: usize = args
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(8_000_000);
            generate(rows, &path(args.next()));
        }
        _ => {
            eprintln!("usage: handwritten (naive|tuned) <file> | handwritten gen <rows> <file>");
            std::process::exit(2);
        }
    }
}

fn path(arg: Option<String>) -> String {
    arg.unwrap_or_else(|| {
        eprintln!("handwritten: a measurements file is required");
        std::process::exit(2);
    })
}

// --- naive -----------------------------------------------------------------

/// What you write first: the whole file as a `String`, `lines`, `split_once`,
/// and the map `std` gives you.
///
/// It is not a straw man. The temperature is already parsed as an integer
/// rather than through `f64::from_str`, because anyone who has looked at the
/// format does that much — and it is the same arithmetic the example's
/// `TENTHS` rule performs, which keeps the two comparable.
fn naive(path: &str) {
    let data = std::fs::read_to_string(path).expect("read the measurements");
    // min, max, sum, count.
    let mut stats: HashMap<&str, (i32, i32, i64, i64)> = HashMap::new();

    for line in data.lines() {
        let (name, temp) = line.split_once(';').expect("a separator");
        let (neg, rest) = match temp.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, temp),
        };
        let (whole, frac) = rest.split_once('.').expect("a decimal point");

        let mut v: i32 = 0;
        for b in whole.bytes() {
            v = v * 10 + (b - b'0') as i32;
        }
        v = v * 10 + (frac.as_bytes()[0] - b'0') as i32;
        if neg {
            v = -v;
        }

        let e = stats.entry(name).or_insert((v, v, 0, 0));
        e.0 = e.0.min(v);
        e.1 = e.1.max(v);
        e.2 += v as i64;
        e.3 += 1;
    }

    let mut names: Vec<&&str> = stats.keys().collect();
    names.sort_unstable();
    report(names.into_iter().map(|n| {
        let (min, max, sum, count) = stats[*n];
        (*n, min as i64, max as i64, sum, count)
    }));
}

// --- tuned -----------------------------------------------------------------

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

// --- shared ----------------------------------------------------------------

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

/// An input, because the benchmark's own 13 GB file is not something to keep
/// in a repository and a measurement without one is not reproducible.
///
/// 413 stations, which is the count the real file has and the number that
/// matters: at fifteen the table lives in L1 and every hash looks free. Some
/// names carry non-ASCII characters, as the real ones do, because an all-ASCII
/// file lets a UTF-8 validator run eight bytes at a time and the compiler's
/// side of the comparison would look better than it is (ADR-016 §3).
fn generate(rows: usize, path: &str) {
    use std::io::Write;

    let syllables = [
        "ba", "ka", "lo", "mi", "ru", "zen", "tor", "vik", "sa", "na", "dor", "el", "gua", "hai",
        "ing", "jos", "kro", "lun", "mer", "nov", "opo", "pri", "qua", "ros", "sur", "tal", "urb",
        "vas", "wro", "xan", "yor", "zut",
    ];
    let accents = [
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        " São",
        " Zürich",
        " Ürümqi",
    ];

    // A fixed seed, so two runs of the generator are the same file and a
    // measurement can be repeated next month.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let mut names: Vec<String> = Vec::new();
    while names.len() < 413 {
        let parts = 2 + (next() % 3) as usize;
        let mut name = String::new();
        for _ in 0..parts {
            name.push_str(syllables[(next() % syllables.len() as u64) as usize]);
        }
        name[..1].make_ascii_uppercase();
        name.push_str(accents[(next() % accents.len() as u64) as usize]);
        if !names.contains(&name) {
            names.push(name);
        }
    }

    let file = std::fs::File::create(path).expect("create the measurements");
    let mut out = std::io::BufWriter::with_capacity(1 << 20, file);
    for _ in 0..rows {
        let name = &names[(next() % 413) as usize];
        // -40.0 ..= 50.0, the range the benchmark generates.
        let tenths = (next() % 901) as i64 - 400;
        writeln!(out, "{name};{}.{}", tenths / 10, (tenths % 10).abs())
            .expect("write a measurement");
    }
    out.flush().expect("flush the measurements");
    eprintln!(
        "handwritten: wrote {rows} rows over {} stations to {path}",
        names.len()
    );
}
