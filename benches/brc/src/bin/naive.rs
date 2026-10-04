//! What a competent Rust programmer writes first for the One Billion Row
//! Challenge: `read_to_string`, `str::lines`, `split_once`, and a `HashMap`
//! with the default hasher. The baseline `examples/1brc.nika` is compared with
//! beside `tuned`; see `benches/brc/README.md`.

use std::collections::HashMap;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("a measurements file is required");
    naive(&path);
}

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
