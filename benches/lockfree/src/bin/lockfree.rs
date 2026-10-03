//! What a **compare-and-swap loop** costs against the two lock shapes, for a
//! value that fits in a machine word.
//!
//! [ADR-281](../../../../docs/specification/adr/adr-281.md) leaves a door
//! open: `update` takes a pure function and is therefore repeatable, which is
//! the route to an implementation that retries instead of locking — the route
//! Clojure's `atom` and Haskell's `TVar` took. This measures whether walking
//! through it would pay, and **for which of the two shapes**.
//!
//! **The question is not whether lock-free wins under contention.** It does, and
//! nobody doubts it. The question is what it costs where nothing contends,
//! because that is the case a `user_parallelism = no` build is always in and
//! most values at `yes` are in as well.
//!
//! | row | what it is |
//! |---|---|
//! | `Local::update` | today's answer where nothing crosses a thread: a borrow flag, one non-atomic write |
//! | `Crossing::update` | today's answer where something may: an OS mutex with ADR-281 D9's owner check |
//! | compare-and-swap loop | the proposal: read, compute, swap, retry — **any** pure function on a word |
//! | `fetch_add` | the floor of what is possible, and what *recognising the operation* would buy over the loop |
//! | `Local::update` twice | the control. It must tie, and it bounds every difference above from below |
//!
//! The threaded rows are reported as a **sign and not a cost**
//! ([`docs/mutex-floor.md`](../../../../docs/mutex-floor.md) §4.2 is the standing
//! reason: on a shared 4-vCPU box the threaded row did not reproduce while every
//! single-threaded row did).
//!
//! ```sh
//! benches/lockfree/lockfree.sh                 # the table, 9 repeats
//! ```

use std::hint::black_box;
use std::sync::atomic::{AtomicI64, Ordering};

use nikaia_std::lock::{Crossing, Local};

/// Nanoseconds per iteration, and whatever the loop accumulated so nothing in
/// it can be optimised away.
fn per_op(n: usize, mut shape: impl FnMut() -> usize) -> (f64, usize) {
    let began = std::time::Instant::now();
    let mut acc = 0usize;
    for _ in 0..n {
        acc = acc.wrapping_add(shape());
    }
    (began.elapsed().as_secs_f64() * 1e9 / n as f64, acc)
}

fn stats(xs: &[f64]) -> (f64, f64, f64, f64) {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let var = xs.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n;
    let lo = xs.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    (mean, lo, hi, var.sqrt())
}

/// **The proposal**: `update`'s block run against a word until nobody got in
/// between.
///
/// It needs nothing of the block but that repeating it is unobservable — not
/// that it is an addition. That is the whole reason the door's shape matters:
/// the old value goes in and the new comes back, so this works for any pure
/// function and not only for the operations a compiler could recognise.
#[inline(always)]
fn swapping(cell: &AtomicI64, f: impl Fn(i64) -> i64) {
    let mut old = cell.load(Ordering::Relaxed);
    loop {
        let new = f(old);
        match cell.compare_exchange_weak(old, new, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => return,
            Err(seen) => old = seen,
        }
    }
}

/// **The crossing shape written by hand** — added after the first table, and
/// now a control rather than a proposal.
///
/// The first table said the shipped shape cost **63.5 ns** where the record
/// claims its owner check is +2.0; this row was what asked why. It was three
/// mutex acquisitions per door and a SipHash of the thread id on every one. The
/// shipped shape is this now, so the two must **tie** — and what they tie at is
/// what the fix was worth.
struct Cheaper<T> {
    inner: std::sync::Mutex<Option<T>>,
    held_by: std::sync::atomic::AtomicU64,
}

thread_local! {
    /// Who is asking, computed once per thread instead of on every door.
    static WHO: u64 = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::thread::current().id().hash(&mut hasher);
        hasher.finish() | 1
    };
}

impl<T> Cheaper<T> {
    fn new(value: T) -> Self {
        Self {
            inner: std::sync::Mutex::new(Some(value)),
            held_by: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn update(&self, f: impl FnOnce(T) -> T) {
        let me = WHO.with(|w| *w);
        assert!(self.held_by.load(Ordering::Relaxed) != me, "re-entered");
        self.held_by.store(me, Ordering::Relaxed);
        {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let old = guard.take().expect("a value");
            *guard = Some(f(old));
        }
        self.held_by.store(0, Ordering::Relaxed);
    }
}

struct Series {
    name: &'static str,
    ns: Vec<f64>,
}

impl Series {
    fn take(
        name: &'static str,
        n: usize,
        repeats: usize,
        mut shape: impl FnMut(usize) -> (f64, usize),
    ) -> Self {
        let _ = shape(n / 10 + 1);
        let mut ns = Vec::with_capacity(repeats);
        print!("  {name:<34}");
        for _ in 0..repeats {
            let (t, acc) = shape(n);
            black_box(acc);
            print!(" {t:7.3}");
            ns.push(t);
        }
        println!();
        Series { name, ns }
    }

    fn stats(&self) -> (f64, f64, f64, f64) {
        stats(&self.ns)
    }
}

fn compare(base: &Series, other: &Series) {
    let (bm, blo, bhi, bsd) = base.stats();
    let (om, olo, ohi, osd) = other.stats();
    println!(
        "  {:<34} {bm:7.3} [{blo:.3}, {bhi:.3}] sd {bsd:.3}",
        base.name
    );
    println!(
        "  {:<34} {om:7.3} [{olo:.3}, {ohi:.3}] sd {osd:.3}",
        other.name
    );
    println!(
        "  {:<34} {:+7.3} ns   ×{:.2}",
        "difference / ratio",
        om - bm,
        om / bm
    );
    println!();
}

/// Per-operation-per-thread time for `threads` threads on **one** value.
fn threaded(n: usize, threads: usize, one: impl Fn(usize) + Send + Sync + Copy) -> f64 {
    let began = std::time::Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(move || one(n));
        }
    });
    began.elapsed().as_secs_f64() * 1e9 / (n * threads) as f64
}

fn main() {
    let mut args = std::env::args().skip(1);
    let n: usize = args
        .next()
        .unwrap_or_else(|| "5000000".into())
        .parse()
        .unwrap();
    let repeats: usize = args.next().unwrap_or_else(|| "9".into()).parse().unwrap();
    let threads: usize = args
        .next()
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .unwrap()
                .get()
                .to_string()
        })
        .parse()
        .unwrap();

    println!("ns per operation, {repeats} repeats of {n} operations; every repeat printed");
    println!();

    let cheap = Local::new(0i64);
    let crossing = Crossing::new(0i64);
    let word = AtomicI64::new(0);

    // ---- the three shapes, uncontended, one thread. -----------------------
    println!("update fn(alt) {{ alt + 1 }} — uncontended, one thread");
    let cheap_series = Series::take("lock::Local<i64>", n, repeats, |n| {
        per_op(n, || {
            let c = black_box(&cheap);
            c.update(|v| *v += 1);
            1
        })
    });
    let crossing_series = Series::take("lock::Crossing<i64>", n, repeats, |n| {
        per_op(n, || {
            let c = black_box(&crossing);
            c.update(|v| *v += 1);
            1
        })
    });
    let swapping_series = Series::take("compare-and-swap loop", n, repeats, |n| {
        per_op(n, || {
            let c = black_box(&word);
            swapping(c, |v| v + 1);
            1
        })
    });
    let adding_series = Series::take("fetch_add", n, repeats, |n| {
        per_op(n, || {
            let c = black_box(&word);
            c.fetch_add(1, Ordering::AcqRel);
            1
        })
    });

    let cheaper = Cheaper::new(0i64);
    let cheaper_series = Series::take("the crossing shape, by hand", n, repeats, |n| {
        per_op(n, || {
            let c = black_box(&cheaper);
            c.update(|v| v + 1);
            1
        })
    });

    println!("against the cheap shape — what a `no` build pays today:");
    compare(&cheap_series, &swapping_series);
    println!("against the crossing shape — what a value that may cross pays today:");
    compare(&crossing_series, &swapping_series);
    println!("and what recognising the operation would buy over the loop:");
    compare(&swapping_series, &adding_series);
    println!("the shipped crossing shape against the same thing by hand (must tie now):");
    compare(&cheaper_series, &crossing_series);
    println!("the loop against the crossing shape:");
    compare(&cheaper_series, &swapping_series);

    // ---- the control. ------------------------------------------------------
    println!("the cheap shape twice over (the control — must tie)");
    let first = Series::take("lock::Local<i64>, run A", n, repeats, |n| {
        per_op(n, || {
            let c = black_box(&cheap);
            c.update(|v| *v += 1);
            1
        })
    });
    let second = Series::take("lock::Local<i64>, run B", n, repeats, |n| {
        per_op(n, || {
            let c = black_box(&cheap);
            c.update(|v| *v += 1);
            1
        })
    });
    compare(&first, &second);

    // ---- and contended: a sign, never a cost. ------------------------------
    let each = n / 10 + 1;
    println!("{threads} threads on one value — a SIGN and not a cost (mutex-floor.md §4.2)");
    let shared_lock = Crossing::new(0i64);
    let shared_word = AtomicI64::new(0);
    for round in 1..=2 {
        let locked = threaded(each, threads, |k| {
            for _ in 0..k {
                black_box(&shared_lock).update(|v| *v += 1);
            }
        });
        let swapped = threaded(each, threads, |k| {
            for _ in 0..k {
                swapping(black_box(&shared_word), |v| v + 1);
            }
        });
        println!(
            "  round {round}: crossing {locked:7.3}   swap {swapped:7.3}   ×{:.2}",
            swapped / locked
        );
    }
    println!();
    println!(
        "  the counts, so neither loop was elided: crossing {} swap {}",
        shared_lock.get(),
        shared_word.load(Ordering::Relaxed)
    );
}
