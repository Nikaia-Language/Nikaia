//! **What an envelope costs a library's error** — the measurement
//! issue #199 asked for before the list may travel to a caller with a
//! bare channel of its own, and on which
//! [ADR-280](../../../../docs/specification/adr/adr-280.md) decided.
//!
//! A library's error travels **bare** ([ADR-280](../../../../docs/specification/adr/adr-280.md)
//! D2): `Result<T, io::IoError>`. Where a body joins, it travels in an envelope
//! ([ADR-280](../../../../docs/specification/adr/adr-280.md) D13):
//! `Result<T, Thrown<io::IoError>>`, which is the error and one word - the site,
//! or a pointer to the site, trace and list. The question is what that word
//! costs where nothing fails, and where something does.
//!
//! | row | what it is |
//! |---|---|
//! | bare, success | three calls deep, each `?`-propagating a `Result<i64, IoError>` that is `Ok` |
//! | envelope, success | the same with `Thrown<IoError>` |
//! | bare, failure | the innermost call fails every time; the error is built and propagated |
//! | envelope, failure | the same, the envelope put on where the error is made |
//! | bare, success, twice | the control. It must tie with the first row |
//!
//! ```sh
//! cargo run -p envelope-bench --release --bin envelope
//! ```

use std::hint::black_box;
use std::time::Instant;

use nikaia_std::error::Thrown;
use nikaia_std::io::IoError;

#[inline(never)]
fn bare_leaf(i: i64, fail: bool) -> Result<i64, IoError> {
    match fail && black_box(true) {
        true => Err(IoError::NotFound(Default::default())),
        false => Ok(black_box(i).wrapping_mul(3)),
    }
}

#[inline(never)]
fn bare_mid(i: i64, fail: bool) -> Result<i64, IoError> {
    Ok(bare_leaf(i, fail)?.wrapping_add(1))
}

#[inline(never)]
fn bare_top(i: i64, fail: bool) -> Result<i64, IoError> {
    Ok(bare_mid(i, fail)?.wrapping_sub(1))
}

#[inline(never)]
fn wrapped_leaf(i: i64, fail: bool) -> Result<i64, Thrown<IoError>> {
    match fail && black_box(true) {
        true => Err(Thrown::from(IoError::NotFound(Default::default()))),
        false => Ok(black_box(i).wrapping_mul(3)),
    }
}

#[inline(never)]
fn wrapped_mid(i: i64, fail: bool) -> Result<i64, Thrown<IoError>> {
    Ok(wrapped_leaf(i, fail)?.wrapping_add(1))
}

#[inline(never)]
fn wrapped_top(i: i64, fail: bool) -> Result<i64, Thrown<IoError>> {
    Ok(wrapped_mid(i, fail)?.wrapping_sub(1))
}

fn run_bare(n: i64, fail: bool) -> i64 {
    let mut total: i64 = 0;
    for i in 0..n {
        total = match bare_top(i, fail) {
            Ok(v) => total.wrapping_add(v),
            Err(e) => total.wrapping_add(black_box(&e) as *const _ as i64 & 1),
        };
    }
    total
}

fn run_wrapped(n: i64, fail: bool) -> i64 {
    let mut total: i64 = 0;
    for i in 0..n {
        total = match wrapped_top(i, fail) {
            Ok(v) => total.wrapping_add(v),
            Err(e) => total.wrapping_add(black_box(&e) as *const _ as i64 & 1),
        };
    }
    total
}

fn nanos_each(took: std::time::Duration, n: i64) -> f64 {
    took.as_secs_f64() * 1e9 / n as f64
}

fn main() {
    const N: i64 = 5_000_000;
    const REPEATS: usize = 7;

    let mut bare_ok = f64::MAX;
    let mut wrapped_ok = f64::MAX;
    let mut bare_err = f64::MAX;
    let mut wrapped_err = f64::MAX;
    let mut control = f64::MAX;

    for _ in 0..REPEATS {
        let began = Instant::now();
        black_box(run_bare(N, false));
        bare_ok = bare_ok.min(nanos_each(began.elapsed(), N));

        let began = Instant::now();
        black_box(run_wrapped(N, false));
        wrapped_ok = wrapped_ok.min(nanos_each(began.elapsed(), N));

        let began = Instant::now();
        black_box(run_bare(N, true));
        bare_err = bare_err.min(nanos_each(began.elapsed(), N));

        let began = Instant::now();
        black_box(run_wrapped(N, true));
        wrapped_err = wrapped_err.min(nanos_each(began.elapsed(), N));

        let began = Instant::now();
        black_box(run_bare(N, false));
        control = control.min(nanos_each(began.elapsed(), N));
    }

    println!(
        "size   Result<i64, IoError> {} bytes, Result<i64, Thrown<IoError>> {} bytes",
        std::mem::size_of::<Result<i64, IoError>>(),
        std::mem::size_of::<Result<i64, Thrown<IoError>>>()
    );
    println!(
        "size   Result<String, IoError> {} bytes, Result<String, Thrown<IoError>> {} bytes",
        std::mem::size_of::<Result<String, IoError>>(),
        std::mem::size_of::<Result<String, Thrown<IoError>>>()
    );
    println!("bare, success          {bare_ok:6.2} ns/call");
    println!(
        "envelope, success      {wrapped_ok:6.2} ns/call   ×{:.2}",
        wrapped_ok / bare_ok
    );
    println!("bare, failure          {bare_err:6.2} ns/call");
    println!(
        "envelope, failure      {wrapped_err:6.2} ns/call   ×{:.2}",
        wrapped_err / bare_err
    );
    println!("bare, success, twice   {control:6.2} ns/call   (the control: it ties)");
}
