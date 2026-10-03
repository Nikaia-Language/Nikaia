//! What a pair of operations costs on the runtime that is already running.
//!
//! This is the measurement [ADR-038](../../../../docs/specification/adr/adr-038.md)
//! D4 predicts and [ADR-292](../../../../docs/specification/adr/adr-292.md)
//! wrote down as a prediction it could not time: an I/O thread that is already
//! running costs nothing per operation, where the vehicle §8.4 measured costs
//! a thread wake-up per pair - **46 µs, and no user-space vehicle removes it**.
//!
//! Three shapes over the same two files, so the difference between them is the
//! vehicle and nothing else:
//!
//! | shape | what it is |
//! |---|---|
//! | `seq` | one read, then the other - the baseline, and what a program that writes no `overlap` gets |
//! | `join` | `rayon::join`: §8.4's vehicle, which `std` no longer has |
//! | `both` | `nikaia_std::fs::read_both`: both in flight on the pre-started runtime |
//!
//! The number that answers D4 is **`join - seq` against `both - seq`**: what
//! each vehicle adds for overlapping a pair whose payload is small enough that
//! the payload is not the answer. Every shape reads the same bytes through the
//! same `std`, so nothing but the vehicle differs.
//!
//! ```sh
//! benches/overlap/runtime.sh              # the table, over five file sizes
//! cargo run -p overlap-bench --release --bin runtime -- 5000 7
//! ```
//!
//! The second argument is how many times the whole measurement is repeated;
//! every repeat is printed, because a single number without its spread is not
//! a measurement (`benches/overlap/README.md`).

use nikaia_std::{fs, rt};

const A: &str = "eins.txt";
const B: &str = "zwei.txt";

/// One read, then the other. What the emitter writes for two statements.
///
/// **Driven rather than called**, since
/// [ADR-055](../../../../docs/specification/adr/adr-055.md) §6 step 2: a read is
/// an `.await` in the emitted program and `block_on` is what drives it, so
/// measuring the call without the executor would measure a program this
/// compiler does not produce - which is the same rule the `match` above is kept
/// for.
fn sequential() -> usize {
    rt::exec::block_on(async {
        let a = fs::read(A, &fs::Root::Anywhere).await.unwrap_or_default();
        let b = fs::read(B, &fs::Root::Anywhere).await.unwrap_or_default();
        a.len() + b.len()
    })
}

/// ADR-292's vehicle: two closures on the pool, one wake-up per pair.
///
/// **`std` no longer has this**, and the number is why: a pool takes closures,
/// a closure cannot `.await`, and the wake-up it costs is what
/// [ADR-292](../../../../docs/specification/adr/adr-292.md) D2's vehicle had to
/// avoid. `rayon::join` is called here directly so the comparison ADR-038 D4
/// rests on stays reproducible after the vehicle went.
///
/// The reads are the runtime's **synchronous** surface, because a closure on a
/// pool cannot await: this shape is the one the pool serves, and a pool worker
/// that is going to block is what §8.4 priced.
fn joined() -> usize {
    let (a, b) = rayon::join(
        || rt::io::read(A.as_ref()).unwrap_or_default(),
        || rt::io::read(B.as_ref()).unwrap_or_default(),
    );
    a.len() + b.len()
}

/// ADR-038 D4's: both operations in flight on the runtime that was already
/// running, results collected in the order they were written.
fn both() -> usize {
    let (a, b) = fs::read_both(A, B);
    a.unwrap_or_default().len() + b.unwrap_or_default().len()
}

fn per_pair(n: usize, mut shape: impl FnMut() -> usize) -> (f64, usize) {
    let began = std::time::Instant::now();
    let mut acc = 0usize;
    for _ in 0..n {
        acc = acc.wrapping_add(shape());
    }
    (began.elapsed().as_secs_f64() * 1e6 / n as f64, acc)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let n: usize = args
        .next()
        .unwrap_or_else(|| "5000".into())
        .parse()
        .unwrap();
    let repeats: usize = args.next().unwrap_or_else(|| "7".into()).parse().unwrap();

    // The runtime is started here, once, exactly as the `fn main` the emitter
    // writes starts it (D4). Nothing below pays for starting it - which is the
    // whole of what is being measured.
    let runtime = nikaia_std::rt::start(nikaia_std::rt::UserCode::Concurrent);
    println!("runtime: {}", nikaia_std::rt::handle().describe());

    // A warm pool and a warm page cache for every shape, so the first repeat
    // is not measuring a cold one.
    for _ in 0..64 {
        sequential();
        joined();
        both();
    }

    println!("repeat  seq(µs)  join(µs)  both(µs)  join-seq  both-seq");
    for repeat in 1..=repeats {
        let (seq, one) = per_pair(n, sequential);
        let (join, two) = per_pair(n, joined);
        let (pair, three) = per_pair(n, both);
        assert_eq!(one, two, "the shapes must read the same bytes");
        assert_eq!(two, three, "the shapes must read the same bytes");
        println!(
            "{repeat:>6}  {seq:>7.2}  {join:>8.2}  {pair:>8.2}  {:>8.2}  {:>8.2}",
            join - seq,
            pair - seq
        );
    }

    runtime.finish();
}
