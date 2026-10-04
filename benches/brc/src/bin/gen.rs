//! The 1BRC input: `gen <rows> <file>`. A fixed seed, so the same rows make the
//! same file.

fn main() {
    let mut args = std::env::args().skip(1);
    let rows: usize = args
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(1_000_000);
    let path = args.next().expect("a file to write is required");
    generate(rows, &path);
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
        "gen: wrote {rows} rows over {} stations to {path}",
        names.len()
    );
}
