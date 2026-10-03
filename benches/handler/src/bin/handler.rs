//! What the **boxed future** costs a handler that does not pause
//! ([ADR-277](../../../../docs/specification/adr/adr-277.md) D12).
//!
//! D1 made a parameter whose type may pause lower to a closure returning a
//! boxed future — `impl Fn(A) -> Pin<Box<dyn Future<Output = R>>>` — whether the
//! callee runs it or keeps it, and D3 said that number is measured rather than
//! assumed. **The *whether* is gone since
//! [ADR-277](../../../../docs/specification/adr/adr-277.md) D11**: a **run**
//! parameter takes `impl AsyncFn(A) -> R` and only a **kept** one keeps the
//! box, because `AsyncFn` is a bound and a field needs a type. What the rows
//! below measure is therefore what each of the two shapes costs, rather than a
//! cost one case was paying for the other.
//!
//! | row | what it is |
//! |---|---|
//! | plain closure | what `fn(i64) -> i64 sync` lowers to: `impl Fn(i64) -> i64` |
//! | boxed future | what `fn(i64) -> i64` lowers to, awaited by the same executor a program uses |
//! | async closure | what `fn(i64) -> i64` lowers to at a **run** parameter: `impl AsyncFn(i64) -> i64` |
//! | plain closure, twice | the control. It must tie, and it bounds the difference above from below |
//!
//! **The third row is why this bench outlived its record**
//! ([ADR-277](../../../../docs/specification/adr/adr-277.md) D13,
//! [ADR-277](../../../../docs/specification/adr/adr-277.md) D11 chose the
//! boxed future because *"Rust has no stable `async` closure"*, and that is
//! false on this toolchain and was false when it was written. The row was added
//! so the alternative would be a number rather than an argument; it is now what
//! a run parameter costs, and the **second** row is what a kept one does.
//!
//! ```sh
//! cargo run -p handler-bench --release --bin handler
//! ```
//!
//! **What it is not.** It is not a claim about a handler that *does* pause:
//! there the plain closure is not a shape at all, so the floor is a reference
//! point and not an alternative. What the three rows compare is the two shapes
//! a pausing parameter can take against the one a `sync` parameter takes.

use std::future::Future;
use std::hint::black_box;
use std::pin::Pin;
use std::time::Instant;

/// The shape `fn(i64) -> i64 sync` lowers to.
fn run_plain(n: i64, f: impl Fn(i64) -> i64) -> i64 {
    let mut total: i64 = 0;
    for i in 0..n {
        total = total.wrapping_add(f(i));
    }
    total
}

/// The shape `fn(i64) -> i64` lowers to, and the call the callee writes for it.
async fn run_future(n: i64, f: impl Fn(i64) -> Pin<Box<dyn Future<Output = i64>>>) -> i64 {
    let mut total: i64 = 0;
    for i in 0..n {
        total = total.wrapping_add(f(i).await);
    }
    total
}

/// The same callee, taking the shape D1 said the language below did not have.
///
/// The body is the one above with the type changed, which is the point: what
/// separates the two rows is the box and the dynamic call, not the loop.
async fn run_async_closure(n: i64, f: impl AsyncFn(i64) -> i64) -> i64 {
    let mut total: i64 = 0;
    for i in 0..n {
        total = total.wrapping_add(f(i).await);
    }
    total
}

fn nanos_each(took: std::time::Duration, n: i64) -> f64 {
    took.as_secs_f64() * 1e9 / n as f64
}

fn main() {
    // Large enough that the loop dominates the timing call, small enough to
    // stay in cache: the difference being measured is an allocation per call.
    const N: i64 = 2_000_000;
    const REPEATS: usize = 5;

    let mut plain = f64::MAX;
    let mut future = f64::MAX;
    let mut closure = f64::MAX;
    let mut control = f64::MAX;

    for _ in 0..REPEATS {
        let began = Instant::now();
        let got = run_plain(N, |i| black_box(i).wrapping_mul(3));
        black_box(got);
        plain = plain.min(nanos_each(began.elapsed(), N));

        let began = Instant::now();
        let got = nikaia_std::rt::exec::block_on(run_future(N, |i| {
            Box::pin(async move { black_box(i).wrapping_mul(3) })
        }));
        black_box(got);
        future = future.min(nanos_each(began.elapsed(), N));

        let began = Instant::now();
        let got = nikaia_std::rt::exec::block_on(run_async_closure(N, async |i: i64| {
            black_box(i).wrapping_mul(3)
        }));
        black_box(got);
        closure = closure.min(nanos_each(began.elapsed(), N));

        let began = Instant::now();
        let got = run_plain(N, |i| black_box(i).wrapping_mul(3));
        black_box(got);
        control = control.min(nanos_each(began.elapsed(), N));
    }

    // The best of the repeats, which is the row least disturbed by whatever
    // else the machine was doing - the same choice `benches/lockfree` makes.
    println!("plain closure          {plain:6.2} ns/call");
    println!(
        "boxed future           {future:6.2} ns/call   ×{:.2}",
        future / plain
    );
    println!(
        "async closure          {closure:6.2} ns/call   ×{:.2}",
        closure / plain
    );
    println!("plain closure, twice   {control:6.2} ns/call   (the control: it ties)");
}
