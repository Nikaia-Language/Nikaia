//! The fixed cost of each vehicle with no work in it at all.
//!
//! `strict` vs `join` says what an overlap costs *for these reads*; this says
//! what it costs for nothing, which is the floor no payload can undercut. The
//! two numbers together are what ADR-292 rests on: the floor is thread
//! wake-up latency, so no user-space vehicle removes it.

fn main() {
    let n: usize = std::env::args().nth(1).unwrap().parse().unwrap();
    let per = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e6 / (n as f64);

    let t = std::time::Instant::now();
    let mut acc = 0u64;
    for i in 0..n {
        let (a, b) = std::thread::scope(|s| {
            let x = s.spawn(|| i as u64);
            let y = s.spawn(|| i as u64 + 1);
            (x.join().unwrap(), y.join().unwrap())
        });
        acc = acc.wrapping_add(a + b);
    }
    println!("scope {:.1} µs/pair {acc}", per(t));

    rayon::join(|| 0u64, || 0u64);
    let t = std::time::Instant::now();
    let mut acc = 0u64;
    for i in 0..n {
        let (a, b) = rayon::join(|| i as u64, || i as u64 + 1);
        acc = acc.wrapping_add(a + b);
    }
    println!("join  {:.1} µs/pair {acc}", per(t));
}
