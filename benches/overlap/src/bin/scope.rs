#![allow(clippy::manual_unwrap_or_default)]
//! (The `match` is what the emitter writes for `catch`. Collapsing it to
//! `unwrap_or_default` would measure a program this compiler does not produce.)

//! The overlap's *first* lowering, kept as the number `join` had to beat.
//!
//! The emitter no longer writes this - see `join.rs` - but it is what ADR-292
//! §8.2's table was measured on, and dropping it would make that table
//! unreproducible.

use std::fs;
fn once() -> usize {
    let (a, b) = std::thread::scope(|scope| {
        let first = scope.spawn(|| match fs::read_to_string("eins.txt") {
            Ok(v) => v,
            Err(_) => String::new(),
        });
        let second = scope.spawn(|| match fs::read_to_string("zwei.txt") {
            Ok(v) => v,
            Err(_) => String::new(),
        });
        (
            first
                .join()
                .unwrap_or_else(|p| std::panic::resume_unwind(p)),
            second
                .join()
                .unwrap_or_else(|p| std::panic::resume_unwind(p)),
        )
    });
    a.len() + b.len()
}
fn main() {
    let n: usize = std::env::args().nth(1).unwrap().parse().unwrap();
    let t = std::time::Instant::now();
    let mut acc = 0usize;
    for _ in 0..n {
        acc = acc.wrapping_add(once());
    }
    println!("scope {:.3} {acc}", t.elapsed().as_secs_f64() * 1000.0);
}
