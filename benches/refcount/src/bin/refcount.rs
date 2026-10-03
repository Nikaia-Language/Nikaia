//! What an atomic reference count costs, against a plain one.
//!
//! [ADR-037](../../../../docs/specification/adr/adr-037.md) D3 expands `Shared`
//! from `user_parallelism`: `no` gives `Rc`, `yes` gives `Arc`. Whether that
//! choice could be made **per value** instead is left open there, and the
//! question is only worth machinery if the atomic is worth avoiding. So this
//! measures the thing the inference would buy, before the inference exists.
//!
//! Six shapes, all on one thread except the last two, each run as a pair so the
//! only difference inside a pair is the reference count:
//!
//! | shape | what it is |
//! |---|---|
//! | `clone+drop` | a handle cloned and dropped — one increment and one decrement, and nothing else |
//! | `read` | the value read through the handle, no handle made — the control: no count is touched, so the pair must be a tie |
//! | `clone+work(k)` | the same clone and drop with `k` multiply-adds between them — what the atomic is as a *share* of a loop that does something |
//! | `counter` | Part II 12.2's `Shared[Locked[i32]]` increment: `Rc<RefCell<i32>>` against `Arc<Mutex<i32>>`. Both halves of the expansion move together here, which is the finding this shape exists for |
//! | `clone+drop, N threads, own handle` | the same atomic with no sharing: N threads each cloning a handle of their own |
//! | `clone+drop, N threads, one handle` | N threads on **one** count — the cache line that makes an atomic expensive rather than merely atomic |
//!
//! `read` is the one that says whether the harness is measuring anything: `Rc`
//! and `Arc` deref identically, so a difference there is noise and bounds every
//! other difference from below.
//!
//! ```sh
//! benches/refcount/refcount.sh                 # the table, 9 repeats
//! cargo run -p refcount-bench --release --bin refcount -- 20000000 9
//! ```
//!
//! Every repeat is printed, because a single number without its spread is not a
//! measurement (`benches/overlap/README.md`), and
//! [`docs/history/runtime-cost.md`](../../../../docs/history/runtime-cost.md) §6.3 is why only
//! the ratios here are load-bearing.

use std::cell::RefCell;
use std::hint::black_box;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

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

/// Mean, minimum, maximum and standard deviation over the repeats.
fn stats(xs: &[f64]) -> (f64, f64, f64, f64) {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let var = xs.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n;
    let lo = xs.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    (mean, lo, hi, var.sqrt())
}

/// `k` multiply-adds, which is a loop body that is not a reference count.
///
/// `seed` varies with the iteration in every caller, because a loop-invariant
/// argument is hoisted out of the loop and then nothing is being measured - the
/// first run of this file printed 0.000 ns for exactly that reason
/// (`docs/history/rc-or-arc.md` §8).
#[inline(never)]
fn work(k: usize, seed: usize) -> usize {
    let mut acc = seed;
    for i in 0..k {
        acc = acc.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(i);
    }
    acc
}

/// Take a handle and drop it, in a body the optimiser cannot see into.
///
/// A `clone` and the `drop` that pairs with it are removable where both are
/// visible and the count is not atomic, so measuring the pair inline would
/// measure `Rc` doing nothing and `Arc` doing something. Handing the handle to
/// an opaque call is also the shape the emitter would write for a `Shared`
/// passed to a function, which is the case the inference is about.
#[inline(never)]
fn consume_rc(h: Rc<usize>) -> usize {
    *h
}

#[inline(never)]
fn consume_arc(h: Arc<usize>) -> usize {
    *h
}

/// One shape's nine numbers, printed as they are taken.
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
        // Warm-up: a tenth of a run, so no repeat pays for a cold branch
        // predictor or the first allocation.
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

/// A pair of series differing in one thing, and what the difference is.
fn compare(plain: &Series, atomic: &Series) {
    let (pm, plo, phi, psd) = plain.stats();
    let (am, alo, ahi, asd) = atomic.stats();
    println!(
        "  {:<34} {pm:7.3} [{plo:.3}, {phi:.3}] sd {psd:.3}",
        plain.name
    );
    println!(
        "  {:<34} {am:7.3} [{alo:.3}, {ahi:.3}] sd {asd:.3}",
        atomic.name
    );
    println!(
        "  {:<34} {:+7.3} ns   ×{:.2}",
        "difference / ratio",
        am - pm,
        am / pm
    );
    println!();
}

fn main() {
    let mut args = std::env::args().skip(1);
    let n: usize = args
        .next()
        .unwrap_or_else(|| "20000000".into())
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

    // ---- clone + drop: one increment, one decrement, nothing else. --------
    println!("clone+drop — the count and nothing else");
    let rc = Rc::new(1_234_567usize);
    let plain = Series::take("Rc<usize>", n, repeats, |n| {
        per_op(n, || consume_rc(Rc::clone(&rc)))
    });
    let arc = Arc::new(1_234_567usize);
    let atomic = Series::take("Arc<usize>", n, repeats, |n| {
        per_op(n, || consume_arc(Arc::clone(&arc)))
    });
    compare(&plain, &atomic);

    // ---- read only: no count is touched, so this pair must tie. -----------
    println!("read through the handle — no count touched (the control)");
    let plain = Series::take("Rc<usize>", n, repeats, |n| {
        per_op(n, || **black_box(&rc) + black_box(0))
    });
    let atomic = Series::take("Arc<usize>", n, repeats, |n| {
        per_op(n, || **black_box(&arc) + black_box(0))
    });
    compare(&plain, &atomic);

    // ---- clone + k units of work: the atomic as a share of a loop. --------
    for k in [4usize, 40, 400] {
        println!("clone+drop with {k} multiply-adds between them");
        let each = (n / (1 + k)).max(100_000);
        let plain = Series::take("Rc<usize>", each, repeats, |n| {
            let mut i = 0usize;
            per_op(n, || {
                i = i.wrapping_add(1);
                let h = Rc::clone(&rc);
                work(k, i).wrapping_add(consume_rc(h))
            })
        });
        let atomic = Series::take("Arc<usize>", each, repeats, |n| {
            let mut i = 0usize;
            per_op(n, || {
                i = i.wrapping_add(1);
                let h = Arc::clone(&arc);
                work(k, i).wrapping_add(consume_arc(h))
            })
        });
        compare(&plain, &atomic);
    }

    // ---- Part II 12.2's counter: both halves of the expansion move. -------
    println!("Shared[Locked[i32]] counter increment (Part II 12.2)");
    let cell = Rc::new(RefCell::new(0i32));
    let plain = Series::take("Rc<RefCell<i32>>", n, repeats, |n| {
        per_op(n, || {
            let h = black_box(Rc::clone(&cell));
            *h.borrow_mut() += 1;
            1
        })
    });
    let lock = Arc::new(Mutex::new(0i32));
    let atomic = Series::take("Arc<Mutex<i32>>", n, repeats, |n| {
        per_op(n, || {
            let h = black_box(Arc::clone(&lock));
            *h.lock().unwrap() += 1;
            1
        })
    });
    compare(&plain, &atomic);

    // ---- the same atomic on several threads, with and without sharing. ----
    // `Rc` has no entry here: this is what only the atomic can do, and the gap
    // between the two rows is what makes an atomic expensive rather than merely
    // atomic.
    println!("clone+drop on {threads} threads — ns per operation per thread");
    let each = n / 4;
    let own = Series::take("Arc, a handle each", each, repeats, |n| {
        let began = std::time::Instant::now();
        let acc: usize = std::thread::scope(|s| {
            let hs: Vec<_> = (0..threads)
                .map(|_| {
                    let mine = Arc::new(1_234_567usize);
                    s.spawn(move || {
                        let mut acc = 0usize;
                        for _ in 0..n {
                            let h = Arc::clone(&mine);
                            acc = acc.wrapping_add(*h);
                        }
                        acc
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).sum()
        });
        let total = threads * n;
        (began.elapsed().as_secs_f64() * 1e9 / total as f64, acc)
    });
    let shared = Series::take("Arc, one handle between them", each, repeats, |n| {
        let one = Arc::new(1_234_567usize);
        let began = std::time::Instant::now();
        let acc: usize = std::thread::scope(|s| {
            let hs: Vec<_> = (0..threads)
                .map(|_| {
                    let mine = Arc::clone(&one);
                    s.spawn(move || {
                        let mut acc = 0usize;
                        for _ in 0..n {
                            let h = Arc::clone(&mine);
                            acc = acc.wrapping_add(*h);
                        }
                        acc
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).sum()
        });
        let total = threads * n;
        (began.elapsed().as_secs_f64() * 1e9 / total as f64, acc)
    });
    compare(&own, &shared);
}

/// Whether the two differ in anything a program can observe other than speed
/// and thread-crossing.
///
/// The experiment was set to check this **first**, because the reason
/// [ADR-037](../../../../docs/specification/adr/adr-037.md) D3 gives for
/// `Shared` being written by hand is that "sharing changes **when a value is
/// cleaned up**, and that is observable" ([ADR-297](../../../../docs/specification/adr/adr-297.md)).
/// If `Rc` and `Arc` differed in anything of that kind, the language's own rule
/// against inferring cleanup timing would forbid inferring between them and the
/// question would be closed. They do not, and these are the assertions that say
/// so rather than the argument that says so.
///
/// The one difference that is real and is **not** in this list is named in
/// `docs/history/rc-or-arc.md` §2: the *thread a destructor runs on*. With an atomic
/// count the last handle may be dropped on another thread, so a cleanup runs
/// there - which is observable, and which can only happen to a value that
/// crosses, where the analysis has no freedom anyway.
#[cfg(test)]
mod observable {
    use std::rc::{Rc, Weak as RcWeak};
    use std::sync::{Arc, Mutex, Weak as ArcWeak};

    /// Both are one pointer, and both put two counts in front of the value, so
    /// a program's memory does not tell them apart.
    #[test]
    fn a_handle_is_the_same_size_either_way() {
        assert_eq!(
            std::mem::size_of::<Rc<u64>>(),
            std::mem::size_of::<Arc<u64>>()
        );
        assert_eq!(std::mem::size_of::<Rc<u64>>(), std::mem::size_of::<usize>());
        assert_eq!(
            std::mem::size_of::<RcWeak<u64>>(),
            std::mem::size_of::<ArcWeak<u64>>()
        );
    }

    /// The count a program could read is the same count at every step.
    ///
    /// Part I 6.2 calls `Shared` "a count of the value's owners". If the
    /// language ever lets a program read it, this is the assertion that says the
    /// two answer alike.
    #[test]
    fn the_owner_count_is_the_same_at_every_step() {
        let rc = Rc::new(1u64);
        let arc = Arc::new(1u64);
        let mut plain = vec![Rc::strong_count(&rc)];
        let mut atomic = vec![Arc::strong_count(&arc)];

        let mut rcs = Vec::new();
        let mut arcs = Vec::new();
        for _ in 0..4 {
            rcs.push(Rc::clone(&rc));
            arcs.push(Arc::clone(&arc));
            plain.push(Rc::strong_count(&rc));
            atomic.push(Arc::strong_count(&arc));
        }
        while rcs.pop().is_some() {
            arcs.pop();
            plain.push(Rc::strong_count(&rc));
            atomic.push(Arc::strong_count(&arc));
        }
        assert_eq!(plain, atomic);
    }

    /// The value is cleaned up at the same point in the program.
    ///
    /// The one D3 cares about. A handle is cloned, passed around and dropped in
    /// a fixed order, and the log says when the destructor ran; the two logs are
    /// compared step for step.
    #[test]
    fn cleanup_happens_at_the_same_point() {
        // The log is behind a `Mutex` so that the logging type is `Send` and
        // `Sync` and the `Arc` half of the pair is an `Arc` a program would
        // really write. Nothing below is threaded; the lock is uncontended and
        // the same lock is taken the same number of times in both halves.
        struct Log(&'static str, Arc<Mutex<Vec<String>>>);
        impl Drop for Log {
            fn drop(&mut self) {
                self.1.lock().unwrap().push(format!("dropped {}", self.0));
            }
        }
        fn say(log: &Arc<Mutex<Vec<String>>>, what: &str) {
            log.lock().unwrap().push(what.to_string());
        }

        // The same script, run twice: the only difference is the count.
        let plain = {
            let log = Arc::new(Mutex::new(Vec::new()));
            let value = Rc::new(Log("value", Arc::clone(&log)));
            say(&log, "built");
            let second = Rc::clone(&value);
            say(&log, "cloned");
            drop(value);
            say(&log, "first handle gone");
            drop(second);
            say(&log, "second handle gone");
            let script = log.lock().unwrap().clone();
            script
        };
        let atomic = {
            let log = Arc::new(Mutex::new(Vec::new()));
            let value = Arc::new(Log("value", Arc::clone(&log)));
            say(&log, "built");
            let second = Arc::clone(&value);
            say(&log, "cloned");
            drop(value);
            say(&log, "first handle gone");
            drop(second);
            say(&log, "second handle gone");
            let script = log.lock().unwrap().clone();
            script
        };
        assert_eq!(plain, atomic);
        assert_eq!(
            plain,
            vec![
                "built",
                "cloned",
                "first handle gone",
                "dropped value",
                "second handle gone",
            ]
        );
    }

    /// A weak handle goes stale at the same point too.
    #[test]
    fn a_weak_handle_goes_stale_at_the_same_point() {
        let rc = Rc::new(7u64);
        let arc = Arc::new(7u64);
        let (weak_plain, weak_atomic) = (Rc::downgrade(&rc), Arc::downgrade(&arc));
        assert_eq!(
            weak_plain.upgrade().is_some(),
            weak_atomic.upgrade().is_some()
        );
        let (kept_plain, kept_atomic) = (Rc::clone(&rc), Arc::clone(&arc));
        drop(rc);
        drop(arc);
        assert_eq!(
            weak_plain.upgrade().is_some(),
            weak_atomic.upgrade().is_some()
        );
        drop(kept_plain);
        drop(kept_atomic);
        assert_eq!(
            weak_plain.upgrade().is_none(),
            weak_atomic.upgrade().is_none()
        );
    }

    /// Exclusive access and unwrapping succeed and fail at the same points,
    /// which is what a copy-on-write surface would be built on.
    #[test]
    fn exclusive_access_and_unwrapping_agree() {
        let mut rc = Rc::new(1u64);
        let mut arc = Arc::new(1u64);
        assert_eq!(
            Rc::get_mut(&mut rc).is_some(),
            Arc::get_mut(&mut arc).is_some()
        );

        let (kept_plain, kept_atomic) = (Rc::clone(&rc), Arc::clone(&arc));
        assert_eq!(
            Rc::get_mut(&mut rc).is_some(),
            Arc::get_mut(&mut arc).is_some()
        );
        assert_eq!(Rc::try_unwrap(rc).is_err(), Arc::try_unwrap(arc).is_err());
        assert_eq!(
            Rc::try_unwrap(kept_plain).is_ok(),
            Arc::try_unwrap(kept_atomic).is_ok()
        );
    }

    /// `Arc` is `Send` where the value is and `Rc` never is, which is the whole
    /// of the difference the language would be inferring about.
    ///
    /// Asserted by compiling: the bounds below hold for one and not for the
    /// other, and there is no way to write the second half of this test.
    #[test]
    fn the_difference_is_send_and_nothing_else_here() {
        fn needs_send<T: Send>(_: T) {}
        needs_send(Arc::new(1u64));
        // needs_send(Rc::new(1u64));  // does not compile, and that is the point
    }
}
