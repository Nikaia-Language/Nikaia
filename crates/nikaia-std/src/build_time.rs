//! **What bounds code run while a program is built**
//! ([ADR-321](../../../docs/specification/adr/adr-321.md) D7, D10).
//!
//! The compiler lowers a `comptime`'s code with a count at the start of every
//! function and every turn of a loop, and the path of functions it is in. Past
//! the budget, or past the call depth, the run stops and says so on its last
//! line, which the compiler reads and turns into `NK1152`, naming the
//! `comptime` and the path. The count is the same on every machine: it is a
//! count, not a time.
//!
//! **Only build-time code calls this.** A program the compiler lowers for
//! running counts nothing.

use std::cell::RefCell;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

/// **How deep a build-time call may go.** The run's own thread has a stack
/// that holds this many frames of any body the lowering writes, many times
/// over.
pub const DEEPEST: usize = 10_000;

/// The stack the run is given, so that [`DEEPEST`] calls fit in a debug build.
pub const STACK: usize = 1 << 30;

/// What the compiler reads off the run's last line.
pub const SAID: &str = "nikaia-build-time:";

/// **The steps left**, counted down: one static and one subtraction a step,
/// because a step is taken at every turn of every loop. A run computes on one
/// thread, so the order is relaxed.
static LEFT: AtomicU64 = AtomicU64::new(u64::MAX);
static BUDGET: AtomicU64 = AtomicU64::new(u64::MAX);

thread_local! {
    static PATH: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// The budget this run may spend, set once before the value is computed.
pub fn start(budget: u64) {
    BUDGET.store(budget, Ordering::Relaxed);
    LEFT.store(budget, Ordering::Relaxed);
}

/// One counted step: a turn of a loop, or a call.
#[inline]
pub fn step() {
    if LEFT.fetch_sub(1, Ordering::Relaxed) == 0 {
        stop(&format!("steps {}", BUDGET.load(Ordering::Relaxed)));
    }
}

/// A function the run is in, for as long as it is.
pub struct Frame(());

/// Entering the function `name`: a step, and a place on the path.
#[inline]
pub fn enter(name: &'static str) -> Frame {
    step();
    let deep = PATH.with(|p| {
        let mut path = p.borrow_mut();
        path.push(name);
        path.len()
    });
    if deep > DEEPEST {
        stop(&format!("depth {DEEPEST}"));
    }
    Frame(())
}

impl Drop for Frame {
    fn drop(&mut self) {
        PATH.with(|p| {
            p.borrow_mut().pop();
        });
    }
}

/// **Stop, and say where**: what was exceeded, then the path. A path deeper
/// than a reader can take in is shown by its first and last calls.
fn stop(what: &str) -> ! {
    let path = PATH.with(|p| {
        let path = p.borrow();
        match path.len() > 12 {
            true => {
                let mut shown: Vec<&str> = path[..6].to_vec();
                shown.push("…");
                shown.extend(&path[path.len() - 6..]);
                shown.join(" > ")
            }
            false => path.join(" > "),
        }
    });
    let mut err = std::io::stderr();
    let _ = writeln!(err, "{SAID} {what} | {path}");
    std::process::exit(3);
}

/// **The run's memory, counted** ([ADR-321](../../../docs/specification/adr/adr-321.md)
/// D10): the bytes live at each request, against a bound. It counts what the
/// program asks for, not what the system grants, so the same program stops at
/// the same request on every machine of one architecture - and `std`'s Rust
/// half too, since every allocation passes through it.
///
/// **The allocator itself is generated**, in the library build-time code links
/// against, because a program linked against `std` dynamically uses the
/// allocator of the library it links and not one of its own. It hands each
/// request to the system and tells this what it took and gave back: an
/// allocator is `unsafe` to implement, and this crate has none (ADR-218).
#[derive(Debug)]
pub struct Counted {
    bound: std::sync::atomic::AtomicUsize,
    live: std::sync::atomic::AtomicUsize,
}

impl Default for Counted {
    fn default() -> Counted {
        Counted::new()
    }
}

impl Counted {
    /// Nothing bounds it until [`Counted::bound`] says what does.
    pub const fn new() -> Counted {
        Counted {
            bound: std::sync::atomic::AtomicUsize::new(usize::MAX),
            live: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// The bound this run holds to, set once before the value is computed.
    pub fn bound(&self, bytes: usize) {
        self.bound
            .store(bytes, std::sync::atomic::Ordering::Relaxed);
    }

    /// `bytes` more are live; past the bound the run stops.
    pub fn take(&self, bytes: usize) {
        use std::sync::atomic::Ordering;
        let now = self.live.fetch_add(bytes, Ordering::Relaxed) + bytes;
        let bound = self.bound.load(Ordering::Relaxed);
        if now > bound {
            over_memory(bound);
        }
    }

    /// `bytes` are given back.
    pub fn give(&self, bytes: usize) {
        self.live
            .fetch_sub(bytes, std::sync::atomic::Ordering::Relaxed);
    }
}

/// **Stop past the memory bound**, saying so without asking for memory: the
/// number is written into a buffer on the stack.
fn over_memory(bound: usize) -> ! {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    let mut n = bound;
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    let mut err = std::io::stderr();
    let _ = err.write_all(SAID.as_bytes());
    let _ = err.write_all(b" memory ");
    let _ = err.write_all(&digits[at..]);
    let _ = err.write_all(b" | \n");
    std::process::exit(3);
}

/// **A value the compiler cannot walk, taken apart into what its ledger's
/// `constant` constructor takes** ([ADR-318](../../../docs/specification/adr/adr-318.md)
/// D5): each part already in the form the compiler reads back, `(i 30)`.
pub trait Parts {
    fn parts(&self) -> Vec<String>;
}

/// `time::Duration::new(secs: u64, nanos: u32)`.
impl Parts for std::time::Duration {
    fn parts(&self) -> Vec<String> {
        vec![
            format!("(i {})", self.as_secs()),
            format!("(i {})", self.subsec_nanos()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_counts_and_a_frame_leaves_the_path() {
        start(100);
        {
            let _outer = enter("outer");
            let _inner = enter("inner");
            PATH.with(|p| assert_eq!(*p.borrow(), ["outer", "inner"]));
        }
        PATH.with(|p| assert!(p.borrow().is_empty()));
        assert_eq!(LEFT.load(Ordering::Relaxed), 98);
    }
}
