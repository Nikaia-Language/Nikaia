//! What `Locked[T]` is in the machine — two shapes behind one surface
//! ([ADR-281](../../../docs/specification/adr/adr-281.md)).
//!
//! **The shape is decided per value and not per build setting** (D1–D3): where
//! `contracts::sharing` proves that nothing crosses a thread with a value, its
//! lock is [`Local`]; where it does not, and at every value that does cross, it
//! is [`Crossing`]. The floor is the one that is safe, exactly as the reference
//! count's floor is the atomic one.
//!
//! **Both say the same thing when a program re-enters one lock** (D4), and that
//! is what the crossing shape pays its owner check for. A borrow flag says so by
//! itself; a bare `Mutex` would hang for ever without saying anything, and at one
//! user thread a hang is the whole program plus
//! [ADR-297](../../../docs/specification/adr/adr-297.md) D6's shutdown deadline.
//! Two behaviours for one written type, picked by an analysis the reader cannot
//! see, is the thing D4 refuses.
//!
//! **Neither is a pause.** `access` takes a lambda that runs straight through
//! ([ADR-005](../../../docs/specification/adr/adr-005.md) D6), so nothing is held
//! across a suspension point and a wait here is bounded by somebody else's
//! compute rather than by somebody else's waiting.

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// The one message, so re-entering reads the same whichever shape a value got.
///
/// **There used to be a second one beside this**, `emptied`, for a lock whose
/// `update` had failed and left nothing behind. It is gone with the state it
/// named: since [ADR-281](../../../docs/specification/adr/adr-281.md) D12 the
/// block is handed the **address** in the lock, so nothing is ever moved out
/// and no slot is ever empty. A panic in the block leaves the value changed as
/// far as the block got, which is D4 rather than a state to report.
///
/// `#[track_caller]` out to the `access` that called it, so the location the
/// panic hook is handed is the generated line that wrote the access and not a
/// line of this file — without it
/// [ADR-300](../../../docs/specification/adr/adr-300.md) D9's table has nothing
/// to look up.
#[cold]
#[inline(never)]
#[track_caller]
fn reentered() -> ! {
    panic!("this lock is already held by the same task: `access` cannot be re-entered")
}

/// **A door to a value a panicking task held** ([ADR-327](../../../docs/specification/adr/adr-327.md)
/// D1, D2): what it holds may be changed half-way, so it is not read. The
/// panic ends the task that opened the door (ADR-326 D4), as the first one's
/// ended the task that held it.
#[cold]
#[inline(never)]
#[track_caller]
fn poisoned() -> ! {
    panic!(
        "a task panicked while it held this lock, so the value may be half-changed: it is never read again"
    )
}

/// **How a supervised value comes back from its poison**
/// ([ADR-327](../../../docs/specification/adr/adr-327.md) D3,
/// [ADR-328](../../../docs/specification/adr/adr-328.md) D9): the restart
/// policy a supervisor's child has, asked by the door that finds the poison.
pub struct Phase {
    policy: Box<dyn crate::supervisor::Restart + Send>,
    /// When the value was last built, for how long the attempt ran.
    built: std::time::Instant,
    /// The history the policy decides from, as a supervisor keeps a child's:
    /// how many times it was poisoned, and when first.
    failures: i64,
    first: Option<std::time::Instant>,
    /// A delay the policy asked for: a door before it panics.
    waiting_until: Option<std::time::Instant>,
    /// The policy gave up: the value stays poisoned.
    given_up: bool,
}

impl std::fmt::Debug for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Phase")
            .field("failures", &self.failures)
            .field("given_up", &self.given_up)
            .finish_non_exhaustive()
    }
}

impl Phase {
    fn new(policy: impl crate::supervisor::Restart + Send + 'static) -> std::sync::Mutex<Phase> {
        std::sync::Mutex::new(Phase {
            policy: Box::new(policy),
            built: std::time::Instant::now(),
            failures: 0,
            first: None,
            waiting_until: None,
            given_up: false,
        })
    }

    /// Whether the door that found the poison rebuilds now; it panics where
    /// the policy waits or gave up.
    #[track_caller]
    fn recover(phase: &std::sync::Mutex<Phase>) {
        let mut phase = phase.lock().unwrap_or_else(|held| held.into_inner());
        if phase.given_up {
            gave_up();
        }
        let now = std::time::Instant::now();
        if phase.waiting_until.is_none() {
            phase.failures += 1;
            let history = crate::supervisor::Failures {
                last_uptime: now.duration_since(phase.built),
                consecutive: phase.failures,
                since_first: now.duration_since(*phase.first.get_or_insert(now)),
            };
            match phase.policy.decide(history) {
                crate::supervisor::Next::Immediate => {}
                crate::supervisor::Next::Delay(span) => phase.waiting_until = Some(now + span),
                crate::supervisor::Next::Escalate => {
                    phase.given_up = true;
                    gave_up();
                }
            }
        }
        if let Some(until) = phase.waiting_until {
            if now < until {
                being_rebuilt();
            }
            phase.waiting_until = None;
        }
        phase.built = now;
    }
}

/// A door to a supervised value during its policy's delay (ADR-328 D9).
#[cold]
#[inline(never)]
#[track_caller]
fn being_rebuilt() -> ! {
    panic!(
        "this value is being rebuilt after a task panicked while it held it: try again after the delay"
    )
}

/// A door to a supervised value whose policy gave up (ADR-328 D9).
#[cold]
#[inline(never)]
#[track_caller]
fn gave_up() -> ! {
    panic!("this value was rebuilt too often and its policy gave up: it stays poisoned")
}

/// The builder of a supervised value, which `Debug` cannot show.
pub struct Builder<F: ?Sized>(Box<F>);

impl<F: ?Sized> std::fmt::Debug for Builder<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Builder")
    }
}

/// **What a `set(neu; after: seen)` failed with**
/// ([ADR-281](../../../docs/specification/adr/adr-281.md) D26): the lock no
/// longer holds the value that was seen.
///
/// An error like any other — caught, declared, retried, or handed to the
/// caller, which in a server is the honest 409. It carries **nothing**: the
/// value that is in the lock now is not in it, because reading it would be a
/// second acquisition and a caller that wants it takes the door again and gets
/// a fresh stamp. What the name has to say is *somebody got there first*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overtaken;

impl std::fmt::Display for Overtaken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the lock no longer holds the value that was seen")
    }
}

impl std::error::Error for Overtaken {}

/// A lock for a value the analysis proved never crosses a thread.
///
/// A borrow flag, which is one non-atomic write and a branch. It is what every
/// lock is at `user_parallelism = no` (D2), because nothing a user writes can
/// cross there and the mutex would charge exactly what that switch exists to
/// save.
#[derive(Debug, Default)]
pub struct Local<T> {
    /// **The value itself, and no `Option` around it**
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D12).
    ///
    /// It used to be an `Option` so that `update` could move the value out and
    /// back without a `Default` bound and without unsafe code, and a lambda
    /// that failed left the slot `None` — a state every other door then had to
    /// report. D1 takes the cause away rather than the symptom: `update` is
    /// handed the **address**, so nothing is moved out and no slot is ever
    /// empty. What a panicking block leaves behind is D4's, and for this shape
    /// that is a value changed as far as the block got.
    inner: RefCell<T>,
    /// **Set when a block panicked while it held the value** (ADR-327 D1):
    /// every door after that panics rather than read it.
    poisoned: std::cell::Cell<bool>,
    /// How to build it again, for `SharedMut::supervised` (ADR-327 D3).
    #[allow(clippy::type_complexity)]
    rebuild: Option<(Builder<dyn Fn() -> T>, std::sync::Mutex<Phase>)>,
}

impl<T> Local<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: RefCell::new(value),
            poisoned: std::cell::Cell::new(false),
            rebuild: None,
        }
    }

    /// `SharedMut::supervised(fn { … })` as the compiler writes it: the kept
    /// function a program hands over.
    pub fn supervised_kept<F: Fn() -> T + ?Sized + 'static>(
        build: crate::func::Kept<F>,
        policy: impl crate::supervisor::Restart + Send + 'static,
    ) -> Self {
        Self::supervised(move || (build.0)(), policy)
    }

    /// **A value that is built again when a panic poisons it**
    /// ([ADR-327](../../../docs/specification/adr/adr-327.md) D3,
    /// [ADR-328](../../../docs/specification/adr/adr-328.md) D9), when
    /// `policy` says so - as a supervisor's child is restarted.
    pub fn supervised(
        build: impl Fn() -> T + 'static,
        policy: impl crate::supervisor::Restart + Send + 'static,
    ) -> Self {
        Self {
            inner: RefCell::new(build()),
            poisoned: std::cell::Cell::new(false),
            rebuild: Some((Builder(Box::new(build)), Phase::new(policy))),
        }
    }

    /// The borrow, for a door: refused when the value is held already
    /// (re-entered) or was left by a panic (poisoned), and marked poisoned if
    /// the block run under it panics.
    #[track_caller]
    #[inline]
    fn held<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        // **Re-entry first**: a door inside the block finds the value held,
        // and that is what it is told, not that the value is poisoned.
        let Ok(mut held) = self.inner.try_borrow_mut() else {
            reentered()
        };
        if self.poisoned.get() {
            match &self.rebuild {
                Some((build, phase)) => {
                    Phase::recover(phase);
                    *held = (build.0)();
                }
                None => poisoned(),
            }
        }
        // **Set for as long as the block runs**, and cleared only when it
        // returns: a block that panics never returns, so the mark stays.
        // Counted with callgrind, a release `update` is 5 instructions with
        // the mark and without it; a guard asking `thread::panicking` on the
        // way out made it 26.
        self.poisoned.set(true);
        let result = f(&mut held);
        self.poisoned.set(false);
        result
    }

    /// Part II 12.2's first door: a copy out. No code of the user's runs while
    /// the lock is open, which is why this needs no condition at all.
    #[track_caller]
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.read(|value| value.clone())
    }

    /// …and the second: replacing. The new value is computed outside, so the
    /// lock is open for the duration of one store.
    #[track_caller]
    pub fn set(&self, value: T) {
        self.held(|held| *held = value)
    }

    /// **Reading in place** ([ADR-281](../../../docs/specification/adr/adr-281.md)
    /// D1): the lambda is handed the value where it lies and may not change it,
    /// so nothing is copied to answer a question about a large value.
    #[track_caller]
    pub fn access<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.read(f)
    }

    /// **Where locked data changes**
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D12): the block is
    /// handed the **address** in the lock, changes the value in place, and
    /// returns nothing.
    ///
    /// Nothing is moved out and no slot is ever empty, which is what took the
    /// `Option` away. D2's other row — a copy and a compare-and-swap, for a
    /// value that fits a machine word — is a **speed** choice on top of this
    /// one and is not built; the block runs exactly once here, which D3 allows
    /// (*may* run more than once is a licence, not a requirement).
    #[track_caller]
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        self.held(f)
    }

    /// **The one door for a stamped value**
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D26).
    ///
    /// `kasse.set(neu; after: stand)` is, by definition,
    /// `update fn(mut v) { if v == stand { v = neu } else { throw Overtaken } }`
    /// — and this is that block, written once. The witness is the value
    /// itself: *store `neu` if the lock still holds what I saw*. If it does,
    /// every decision taken on what was seen still holds; if it does not, the
    /// caller is told rather than overwriting somebody else's work.
    ///
    /// **The comparison is of the whole value**, which is what `PartialEq`
    /// buys and what makes this honest for a large one: a ten-thousand-entry
    /// list is compared entry by entry, and a program that minds writes the
    /// `update` block with a version field of its own
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D12).
    /// [ADR-281](../../../docs/specification/adr/adr-281.md) D13's
    /// compare-and-swap is the same operation for a word-sized value and is
    /// the speed row that is not built.
    ///
    /// **One lock acquisition and not two.** A `get` followed by a `set` is
    /// the mistake D4 refuses; the compare and the store happen while the lock
    /// is open, which is the whole of what this door is for.
    ///
    /// **The witness is a view and the value is not**, which is the difference
    /// between them: the value is stored and the witness is only read. A
    /// witness taken by value would be *moved* out of the caller, and reading
    /// what was seen after asking whether it still holds is an ordinary thing
    /// to write — so the ledger says `seen: &$T` and the caller keeps it.
    #[track_caller]
    pub fn set_after(&self, value: T, seen: &T) -> Result<(), Overtaken>
    where
        T: PartialEq,
    {
        self.held(|held| match *held == *seen {
            true => {
                *held = value;
                Ok(())
            }
            false => Err(Overtaken),
        })
    }

    /// **Exclusive even to read**, and that is not an oversight.
    ///
    /// Two reads of one lock do not conflict, so a shared borrow would let a
    /// nested `access` through. The crossing shape cannot: a mutex is exclusive
    /// and its owner check fires. One written type may not behave two ways
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D9), and of the two
    /// ways to agree, refusing is the one that keeps Part II 12.2's rule whole -
    /// re-entering a lock is a defect in code that is not well-formed, whichever
    /// door it happens through.
    #[track_caller]
    fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.held(|held| f(held))
    }
}

/// A lock for a value that crosses a thread, or that the analysis could not
/// prove does not.
///
/// An OS mutex **with an owner check**, and the check is D4's price rather than
/// an extra: without it this shape hangs where [`Local`] reports, and one written
/// type would mean two behaviours. Measured at +2.0 ns, ×1.15 against a bare
/// `Mutex` (`docs/mutex-floor.md` §4.3).
#[derive(Debug, Default)]
pub struct Crossing<T> {
    /// The value itself, for the reason [`Local`]'s is
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D12).
    inner: Mutex<T>,
    /// Which task holds it, or `0` for nobody.
    ///
    /// **An atomic and not a second `Mutex`**, which is a correctness point
    /// before it is a cost: the mark is written *under* the guard, so it names
    /// the task that actually holds the lock rather than one that hopes to. It is
    /// **read** before the acquisition, because once that blocks there is nothing
    /// left to report to - and a read that races tells us nothing about *our own*
    /// re-entry, which is the only thing this answers, because only we ever write
    /// our own id here.
    ///
    /// Measured: as a `Mutex<Option<u64>>` written before the acquisition, one
    /// door cost three mutex acquisitions and 63.5 ns; this is 17.2
    /// (`benches/lockfree`).
    held_by: AtomicU64,
    /// How to build it again, for `SharedMut::supervised` (ADR-327 D3).
    #[allow(clippy::type_complexity)]
    rebuild: Option<(
        Builder<dyn Fn() -> T + Send + Sync>,
        std::sync::Mutex<Phase>,
    )>,
}

impl<T> Crossing<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Mutex::new(value),
            held_by: AtomicU64::new(NOBODY),
            rebuild: None,
        }
    }

    /// `SharedMut::supervised(fn { … })` where the value crosses threads.
    pub fn supervised_kept<F: Fn() -> T + ?Sized + Send + Sync + 'static>(
        build: crate::func::Kept<F>,
        policy: impl crate::supervisor::Restart + Send + 'static,
    ) -> Self {
        Self::supervised(move || (build.0)(), policy)
    }

    /// [`Local::supervised`], where the value crosses threads.
    pub fn supervised(
        build: impl Fn() -> T + Send + Sync + 'static,
        policy: impl crate::supervisor::Restart + Send + 'static,
    ) -> Self {
        Self {
            inner: Mutex::new(build()),
            held_by: AtomicU64::new(NOBODY),
            rebuild: Some((Builder(Box::new(build)), Phase::new(policy))),
        }
    }

    /// Part II 12.2's first two doors, which run no code of the user's.
    #[track_caller]
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.access(|value| value.clone())
    }

    #[track_caller]
    pub fn set(&self, value: T) {
        self.hold(|slot| *slot = value);
    }

    /// **Reading in place** ([ADR-281](../../../docs/specification/adr/adr-281.md)
    /// D1).
    #[track_caller]
    pub fn access<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.hold(|slot| f(slot))
    }

    /// **Where locked data changes**
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D12): the block is
    /// handed the address under the guard, changes the value in place, and
    /// returns nothing. The lock is held across the whole of it, so nothing
    /// sees a gap — and there is no gap to see, because nothing is moved out.
    #[track_caller]
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        self.hold(f);
    }

    /// **The one door for a stamped value**
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D26), the
    /// crossing shape. [`Local::set_after`] carries the reasoning; what is
    /// different here is only that the lock is a mutex, so the compare and the
    /// store happen under the guard the owner check already took.
    #[track_caller]
    pub fn set_after(&self, value: T, seen: &T) -> Result<(), Overtaken>
    where
        T: PartialEq,
    {
        self.hold(|slot| match *slot == *seen {
            true => {
                *slot = value;
                Ok(())
            }
            false => Err(Overtaken),
        })
    }

    /// The owner check and the guard, around whatever the door does with the
    /// slot — written once, because every door needs both and a door that
    /// forgot one would be the hang this shape carries the check to prevent.
    #[track_caller]
    fn hold<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let me = current_task();
        // **Read before the acquisition**, because once that blocks there is
        // nothing left to report to - which is the whole reason this shape
        // carries a mark at all.
        //
        // **And it is a build option** ([ADR-281](../../../docs/specification/adr/adr-281.md)
        // D8, Part I 1.2): under D2 a re-entrant acquisition cannot happen in a
        // correct compiler, so this is self-control of that rule rather than
        // error handling, and it is a guarantee that may be declined. For every
        // program that obeys the nesting rule both builds behave identically —
        // the switch decides only whether a violation is **noticed**.
        //
        // **It reaches this shape and not [`Local`]'s**, which is the honest
        // scope rather than an omission: the mark below is this compiler's, and
        // declining it costs an atomic load and store per acquisition
        // ([ADR-281](../../../docs/specification/adr/adr-281.md) D6 makes that
        // free at one thread and D3 charges it only on values that cross).
        // `Local`'s check is a `RefCell`'s own borrow flag — the language
        // below's, which no switch of ours declines.
        #[cfg(feature = "reentrancy-check")]
        if self.held_by.load(Ordering::Relaxed) == me {
            reentered();
        }

        // **A poisoned value is never read** (ADR-327 D1, D2): a task that
        // panicked while it held the lock may have left it half-changed, and
        // the mutex says so. The door panics in the task that opens it.
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(left) => match &self.rebuild {
                Some((build, phase)) => {
                    Phase::recover(phase);
                    let mut guard = left.into_inner();
                    *guard = (build.0)();
                    self.inner.clear_poison();
                    guard
                }
                None => poisoned(),
            },
        };
        // **Written under the guard**, so the mark names the holder rather than
        // a hopeful: before, it was set before the acquisition, and while one
        // task waited the mark said *its* name although another held the lock.
        self.held_by.store(me, Ordering::Relaxed);
        // **Cleared on the way out, a panic's way too**: a mark left behind
        // would make the next door read as a re-entry rather than as the
        // poisoned value it is.
        let _released = Released(&self.held_by);
        f(&mut guard)
    }
}

/// Clears [`Crossing`]'s owner mark when the block ends, however it ends.
struct Released<'a>(&'a AtomicU64);

impl Drop for Released<'_> {
    fn drop(&mut self) {
        self.0.store(NOBODY, Ordering::Relaxed);
    }
}

/// Who is asking, for the owner check.
///
/// The thread is the unit today, because a task does not move between threads
/// while it holds a lock: `access` takes a lambda that runs straight through, so
/// there is no suspension point between the two lines above for a task to be
/// moved at ([ADR-005](../../../docs/specification/adr/adr-005.md) D6). When a
/// task identity exists this is the one line that changes.
fn current_task() -> u64 {
    WHO.with(|who| *who)
}

/// Nobody holds it. `WHO` never answers this, so the two can never be confused.
const NOBODY: u64 = 0;

thread_local! {
    /// **Computed once per thread**, because it used to be a SipHash of the
    /// thread id on every single door - measured at a large share of what a door
    /// cost (`benches/lockfree`).
    ///
    /// `| 1` so it is never [`NOBODY`]: the hash is opaque and one value of it
    /// would otherwise mean "unheld".
    static WHO: u64 = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::thread::current().id().hash(&mut hasher);
        hasher.finish() | 1
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Poisons a lock by panicking inside its door, quietly.
    fn poison(door: impl FnOnce() + std::panic::UnwindSafe) {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let _ = std::panic::catch_unwind(door);
        std::panic::set_hook(hook);
    }

    /// Whether a door panics, quietly.
    fn panics<R>(door: impl FnOnce() -> R + std::panic::UnwindSafe) -> bool {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let failed = std::panic::catch_unwind(door).is_err();
        std::panic::set_hook(hook);
        failed
    }

    /// **A supervised value is rebuilt** (ADR-327 D3, ADR-328 D9): the first
    /// poison at once; the second after the policy's delay, a door during it
    /// panicking.
    #[test]
    fn a_supervised_value_is_rebuilt_by_its_policy() {
        let builds = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = builds.clone();
        let lock = std::panic::AssertUnwindSafe(Local::supervised(
            move || {
                counted.set(counted.get() + 1);
                10_i64
            },
            crate::supervisor::Backoff(),
        ));
        lock.update(|v| *v = 11);
        poison(|| lock.update(|_| panic!("boom")));
        assert_eq!(lock.get(), 10, "rebuilt at once");
        assert_eq!(builds.get(), 2);
        poison(|| lock.update(|_| panic!("boom")));
        assert!(panics(|| lock.get()), "a door during the delay panics");
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert_eq!(lock.get(), 10, "rebuilt after the delay");
        assert_eq!(builds.get(), 3);
    }

    /// **A policy of the program's** (ADR-328 D9): one that gives up at once
    /// leaves the value poisoned after the first panic.
    #[test]
    fn a_supervised_value_whose_policy_gives_up_stays_poisoned() {
        struct Never;
        impl crate::supervisor::Restart for Never {
            fn decide(&mut self, _: crate::supervisor::Failures) -> crate::supervisor::Next {
                crate::supervisor::Next::Escalate
            }
        }
        let lock = std::panic::AssertUnwindSafe(Local::supervised(|| 1_i64, Never));
        poison(|| lock.update(|_| panic!("boom")));
        assert!(panics(|| lock.get()), "the policy gave up");
        assert!(panics(|| lock.get()), "and the value stays poisoned");
    }

    #[test]
    fn a_supervised_crossing_value_is_rebuilt() {
        let lock =
            std::sync::Arc::new(Crossing::supervised(|| 7_i64, crate::supervisor::Backoff()));
        lock.update(|v| *v = 8);
        let held = lock.clone();
        let _ = std::thread::spawn(move || {
            std::panic::set_hook(Box::new(|_| {}));
            held.update(|_| panic!("boom"))
        })
        .join();
        let _ = std::panic::take_hook();
        assert_eq!(lock.get(), 7);
    }

    /// **Re-entering the crossing shape is noticed**, where the build carries
    /// the check ([ADR-281](../../../docs/specification/adr/adr-281.md) D36).
    ///
    /// Under D2 this cannot happen in a correct compiler — `NK2203` refuses the
    /// nesting when the program is compiled — so the only way to reach it is to
    /// write the nesting in Rust, here. That is what makes it *self-control of
    /// D2's rule* rather than error handling: no Nikaia program can get here.
    #[test]
    #[cfg(feature = "reentrancy-check")]
    fn re_entering_the_crossing_shape_is_noticed() {
        let lock = Crossing::new(1_i64);
        // The hook is taken off around it: the panic is the answer being
        // tested, and a test that prints a backtrace for its own expectation
        // reads like a failure.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let again = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lock.access(|_| lock.get());
        }));
        std::panic::set_hook(hook);

        let said = again.expect_err("the second acquisition is noticed");
        // `panic!` with no arguments carries a `&'static str`; with any, a
        // `String`. Both, because the message is one literal today and need not
        // stay one.
        let said = said
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| said.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        assert!(said.contains("cannot be re-entered"), "{said}");
    }

    /// **And `Local`'s check is not ours to decline.** It is a `RefCell`'s own
    /// borrow flag, so it is there at both settings — which is more noticing
    /// than the switch promises and costs a program that obeys the rule
    /// nothing.
    #[test]
    fn the_local_shapes_check_is_the_language_belows() {
        let lock = Local::new(1_i64);
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let again = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lock.access(|_| lock.set(2));
        }));
        std::panic::set_hook(hook);
        assert!(again.is_err(), "a `RefCell` notices either way");
    }

    /// The message a door panicked with.
    fn panicked_with(run: impl FnOnce()) -> String {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let said = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
        std::panic::set_hook(hook);
        let said = said.expect_err("the door panics");
        said.downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| said.downcast_ref::<String>().cloned())
            .unwrap_or_default()
    }

    /// **A value a panicking block held is never read again**
    /// ([ADR-327](../../../docs/specification/adr/adr-327.md) D1, D2), on both
    /// shapes alike: the next door panics, and a value nobody broke is
    /// untouched.
    #[test]
    fn a_poisoned_value_is_never_read() {
        let local = Local::new(1_i64);
        let untouched = Local::new(7_i64);
        panicked_with(|| {
            local.update(|n| {
                *n = 99;
                panic!("half-way");
            })
        });
        assert!(
            panicked_with(|| {
                local.get();
            })
            .contains("never read again")
        );
        assert!(panicked_with(|| local.set(2)).contains("never read again"));
        assert_eq!(untouched.get(), 7);

        let crossing = Crossing::new(1_i64);
        panicked_with(|| {
            crossing.update(|n| {
                *n = 99;
                panic!("half-way");
            })
        });
        assert!(
            panicked_with(|| {
                crossing.get();
            })
            .contains("never read again")
        );
    }

    /// The four doors, on both shapes
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md)).
    #[test]
    fn both_shapes_carry_all_four_doors() {
        let local = Local::new(1_i64);
        assert_eq!(local.get(), 1);
        local.set(5);
        local.update(|old| *old += 100);
        assert_eq!(local.access(|n| *n), 105);

        let crossing = Crossing::new(1_i64);
        assert_eq!(crossing.get(), 1);
        crossing.set(5);
        crossing.update(|old| *old += 100);
        assert_eq!(crossing.access(|n| *n), 105);
    }

    /// **`update` moves, it does not copy**, which is the finding that made
    /// `access` stop needing to write (D2). The list's buffer is the same
    /// allocation after an append, so nothing was copied to add one entry.
    #[test]
    fn update_appends_without_copying_the_value() {
        // Room to spare, so a `push` does not grow the buffer: what is being
        // asked is whether `update` copies, not whether `Vec` reallocates.
        let mut start = Vec::with_capacity(8);
        start.extend([1_i64, 2, 3]);
        let local = Local::new(start);
        let before = local.access(|v| v.as_ptr());
        local.update(|v| v.push(4));
        assert_eq!(local.access(|v| v.len()), 4);
        assert_eq!(
            local.access(|v| v.as_ptr()),
            before,
            "the same buffer: an append is a move and not a copy"
        );
    }

    /// And reading in place answers a question about a large value without
    /// taking a copy of it (D1) — the one thing neither `get` nor `update` does.
    #[test]
    fn access_reads_a_large_value_in_place() {
        let local = Local::new((0..10_000_i64).collect::<Vec<_>>());
        let before = local.access(|v| v.as_ptr());
        assert_eq!(local.access(|v| v.len()), 10_000);
        assert_eq!(local.access(|v| v.as_ptr()), before, "nothing was copied");
    }

    /// D4, which is the whole reason the crossing shape carries an owner check:
    /// **one behaviour, two prices.** A bare `Mutex` hangs here.
    #[test]
    fn re_entering_says_the_same_thing_in_both_shapes() {
        let local = Local::new(1_i64);
        let said = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            local.access(|_| local.access(|n| *n))
        }))
        .expect_err("the cheap shape reports");
        assert!(message(&said).contains("already held by the same task"));

        let crossing = Crossing::new(1_i64);
        let said = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crossing.access(|_| crossing.access(|n| *n))
        }))
        .expect_err("and so does the crossing one, rather than hanging");
        assert!(message(&said).contains("already held by the same task"));
    }

    /// And the crossing shape really does cross, which is the capability the
    /// cheap one cannot have: `Arc<Local<T>>` is not `Send` and this is.
    #[test]
    fn the_crossing_shape_crosses() {
        let value = std::sync::Arc::new(Crossing::new(0_i64));
        let mut threads = Vec::new();
        for _ in 0..4 {
            let value = std::sync::Arc::clone(&value);
            threads.push(std::thread::spawn(move || {
                for _ in 0..1000 {
                    value.update(|n| *n += 1);
                }
            }));
        }
        for thread in threads {
            thread.join().expect("no task failed");
        }
        assert_eq!(value.access(|n| *n), 4000);
    }

    fn message(said: &Box<dyn std::any::Any + Send>) -> &str {
        said.downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| said.downcast_ref::<&str>().copied())
            .unwrap_or("")
    }
}

/// **A value that fits a machine word, as the word itself**
/// ([ADR-281](../../../docs/specification/adr/adr-281.md) D13, D14;
/// [ADR-281](../../../docs/specification/adr/adr-281.md)).
///
/// The third shape, and the one [`Crossing`] gives way to where the value is a
/// number, a truth value or a character and nothing takes it in a door over
/// several locks. There is no lock: `update` runs the block on a **copy**,
/// compare-and-swaps the result in, and on a collision runs it again on a
/// fresh copy — which ADR-281 D14 permits, because a block is `sync`, touches
/// no lock and changes nothing outside `v`, so the run that lost leaves no
/// trace. A panic in the block leaves the word as it was (D4).
///
/// Measured against the crossing shape at ×0.63–×0.67 uncontended and several
/// times less under contention (`docs/lock-free.md` §2). It never replaces
/// [`Local`], which a compare-and-swap loses to by ×6.5.
///
/// **Not a [`Door`]**, deliberately: a door over two locks holds both at once,
/// and a retry cannot be held. The compiler gives such a value the crossing
/// shape; were it to get this one, the program would not compile rather than
/// race.
pub struct Word<T> {
    bits: AtomicU64,
    of: std::marker::PhantomData<T>,
}

/// **What fits a word**: a value that is its own bits, and back.
pub trait Bits: Copy {
    /// The value as 64 bits.
    fn to_bits(self) -> u64;
    /// The value those bits are.
    fn from_bits(bits: u64) -> Self;
}

macro_rules! bits_by_cast {
    ($($t:ty),*) => {$(
        impl Bits for $t {
            fn to_bits(self) -> u64 {
                self as u64
            }
            fn from_bits(bits: u64) -> Self {
                bits as $t
            }
        }
    )*};
}
bits_by_cast!(i8, i16, i32, i64, u8, u16, u32, u64, isize, usize);

impl Bits for f64 {
    fn to_bits(self) -> u64 {
        f64::to_bits(self)
    }
    fn from_bits(bits: u64) -> Self {
        f64::from_bits(bits)
    }
}

impl Bits for f32 {
    fn to_bits(self) -> u64 {
        u64::from(f32::to_bits(self))
    }
    fn from_bits(bits: u64) -> Self {
        f32::from_bits(bits as u32)
    }
}

impl Bits for bool {
    fn to_bits(self) -> u64 {
        u64::from(self)
    }
    fn from_bits(bits: u64) -> Self {
        bits != 0
    }
}

impl Bits for char {
    fn to_bits(self) -> u64 {
        u64::from(u32::from(self))
    }
    fn from_bits(bits: u64) -> Self {
        // Only a `char` is ever stored, so the bits are always one.
        char::from_u32(bits as u32).unwrap_or('\0')
    }
}

impl<T: Bits> Word<T> {
    pub fn new(value: T) -> Self {
        Self {
            bits: AtomicU64::new(value.to_bits()),
            of: std::marker::PhantomData,
        }
    }

    /// The value, read.
    pub fn get(&self) -> T {
        T::from_bits(self.bits.load(Ordering::Acquire))
    }

    /// The value, written.
    pub fn set(&self, value: T) {
        self.bits.store(value.to_bits(), Ordering::Release);
    }

    /// **Reading in place**, which for a word is reading a copy: nothing can
    /// change what the block was handed.
    pub fn access<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.get())
    }

    /// **The block on a copy, swapped in; again on a collision**
    /// (ADR-281 D13, D14).
    pub fn update(&self, mut f: impl FnMut(&mut T)) {
        let mut seen = self.bits.load(Ordering::Acquire);
        loop {
            let mut value = T::from_bits(seen);
            f(&mut value);
            match self.bits.compare_exchange_weak(
                seen,
                value.to_bits(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(now) => seen = now,
            }
        }
    }

    /// **The one door for a stamped value**
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D26), as one
    /// compare-and-swap: the value goes in where the word still holds what
    /// was seen. Compared as values, so a number equal to the one seen is the
    /// one seen, as the other shapes compare it.
    pub fn set_after(&self, value: T, seen: &T) -> Result<(), Overtaken>
    where
        T: PartialEq,
    {
        let mut now = self.bits.load(Ordering::Acquire);
        loop {
            if T::from_bits(now) != *seen {
                return Err(Overtaken);
            }
            match self.bits.compare_exchange_weak(
                now,
                value.to_bits(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(moved) => now = moved,
            }
        }
    }
}

impl<T: Bits + std::fmt::Debug> std::fmt::Debug for Word<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Word").field(&self.get()).finish()
    }
}

impl<T: Bits + Default> Default for Word<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

/// One lock, whichever shape it got — what a door over **several** of them needs
/// ([ADR-281](../../../docs/specification/adr/adr-281.md)).
///
/// A door over two locks cannot be written against `Local` or `Crossing` by name:
/// which shape a value gets is decided per value, so one program can hold both,
/// and the two values a transfer names need not have got the same answer. What
/// the door needs of each is the same three things whichever shape it is, and
/// this says which three.
pub trait Door {
    /// What the lock holds.
    type Held;

    /// **Where this lock stands in the one order everybody takes them in.**
    ///
    /// Its address. Two threads that both want A and B take them in the same
    /// order because they compute the same two numbers, which is what makes a
    /// cycle impossible rather than unlikely (Part II, 12.3).
    fn ordering(&self) -> usize;

    /// Read in place, the way `access` does.
    fn reading<R>(&self, f: impl FnOnce(&Self::Held) -> R) -> R;

    /// Change the value in place, the way `update` does
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D12) — and hand a
    /// result outward, which is what lets two of these nest.
    ///
    /// It used to take the value **out** and put one back, which is what made
    /// an empty slot a state every other door had to report. D1 takes the cause
    /// away: the block is handed the address, so nothing is moved and no slot
    /// is ever empty.
    fn changing<R>(&self, f: impl FnOnce(&mut Self::Held) -> R) -> R;
}

impl<T> Door for Local<T> {
    type Held = T;

    fn ordering(&self) -> usize {
        self as *const Self as usize
    }

    #[track_caller]
    fn reading<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.read(f)
    }

    #[track_caller]
    fn changing<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        self.held(f)
    }
}

impl<T> Door for Crossing<T> {
    type Held = T;

    fn ordering(&self) -> usize {
        self as *const Self as usize
    }

    #[track_caller]
    fn reading<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.access(f)
    }

    #[track_caller]
    fn changing<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        self.hold(f)
    }
}

/// **A handle on a lock is a lock**, so a door takes one without being told which
/// it was given ([ADR-281](../../../docs/specification/adr/adr-281.md) D17).
///
/// `SharedMut[T]` is a count around a lock and `Locked[T]` is the lock in a
/// field; the door is written once and both reach it. The **order** delegates to
/// the lock inside rather than to the handle, because two handles on one
/// allocation must compute the same number or the order they promise is not one.
impl<D: Door> Door for std::rc::Rc<D> {
    type Held = D::Held;

    fn ordering(&self) -> usize {
        (**self).ordering()
    }

    #[track_caller]
    fn reading<R>(&self, f: impl FnOnce(&Self::Held) -> R) -> R {
        (**self).reading(f)
    }

    #[track_caller]
    fn changing<R>(&self, f: impl FnOnce(&mut Self::Held) -> R) -> R {
        (**self).changing(f)
    }
}

impl<D: Door> Door for std::sync::Arc<D> {
    type Held = D::Held;

    fn ordering(&self) -> usize {
        (**self).ordering()
    }

    #[track_caller]
    fn reading<R>(&self, f: impl FnOnce(&Self::Held) -> R) -> R {
        (**self).reading(f)
    }

    #[track_caller]
    fn changing<R>(&self, f: impl FnOnce(&mut Self::Held) -> R) -> R {
        (**self).changing(f)
    }
}

/// **Both locks read, both held at once, and taken in one global order**
/// (Part II 12.3, [ADR-281](../../../docs/specification/adr/adr-281.md) D18).
///
/// The order is by address and not by the order they are written, which is the
/// whole point: `access_all(a, b)` in one task and `access_all(b, a)` in another
/// take them the same way round, so there is no cycle to deadlock in. Nesting
/// them by hand is what the language refuses (Part II, 12.3).
#[track_caller]
pub fn access_all<A, B, R>(a: &A, b: &B, f: impl FnOnce(&A::Held, &B::Held) -> R) -> R
where
    A: Door,
    B: Door,
{
    match a.ordering() <= b.ordering() {
        true => a.reading(|x| b.reading(|y| f(x, y))),
        false => b.reading(|y| a.reading(|x| f(x, y))),
    }
}

/// **Both locks written, changed in place**
/// ([ADR-281](../../../docs/specification/adr/adr-281.md) D18).
///
/// D12's one form, widened: **one `mut` per lock and nothing returned**. Both
/// are held for the whole of the block, so nothing sees either between the two
/// changes, and the same address order as above is what makes a cycle
/// impossible.
///
/// It used to take the old values by value and hand back a pair. That is what
/// made an empty slot a state — a block that panicked between the two left one
/// lock without a value — and D18 takes the cause away rather than the symptom.
#[track_caller]
pub fn update_all<A, B>(a: &A, b: &B, f: impl FnOnce(&mut A::Held, &mut B::Held))
where
    A: Door,
    B: Door,
{
    match a.ordering() <= b.ordering() {
        true => a.changing(|x| b.changing(|y| f(x, y))),
        false => b.changing(|y| a.changing(|x| f(x, y))),
    }
}

#[cfg(test)]
mod doors {
    use super::*;

    /// Chapter 12's own transfer, which the language could not write before
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md)).
    #[test]
    fn a_transfer_moves_between_two_locks() {
        let a = Local::new(100i64);
        let b = Local::new(5i64);
        update_all(&a, &b, |from, to| {
            *from -= 30;
            *to += 30;
        });
        assert_eq!(a.get(), 70);
        assert_eq!(b.get(), 35);
    }

    /// **The order is the addresses', not the arguments'**, which is what makes
    /// two tasks that name them the other way round safe.
    #[test]
    fn the_two_orders_are_the_same_order() {
        let a = Crossing::new(1i64);
        let b = Crossing::new(2i64);
        assert_eq!(access_all(&a, &b, |x, y| *x * 10 + *y), 12);
        assert_eq!(access_all(&b, &a, |y, x| *x * 10 + *y), 12);
    }

    /// And two threads doing the transfer both ways round neither deadlock nor
    /// lose a penny, which is the property `access_all` exists for.
    ///
    /// **Deliberately short.** A deadlock is a hang whatever the count, and the
    /// total is exact at any of them - so the number is chosen to prove the
    /// property without loading the machine, because two timing-sensitive I/O
    /// tests share this binary and a stress test beside them is a flake they did
    /// not have.
    #[test]
    fn two_threads_transferring_both_ways_keep_the_total() {
        let a = std::sync::Arc::new(Crossing::new(1_000i64));
        let b = std::sync::Arc::new(Crossing::new(1_000i64));
        let (one, two) = (std::sync::Arc::clone(&a), std::sync::Arc::clone(&b));
        let left = std::thread::spawn(move || {
            for _ in 0..400 {
                update_all(&*one, &*two, |x, y| {
                    *x -= 1;
                    *y += 1;
                });
            }
        });
        let (one, two) = (std::sync::Arc::clone(&a), std::sync::Arc::clone(&b));
        let right = std::thread::spawn(move || {
            for _ in 0..400 {
                update_all(&*two, &*one, |y, x| {
                    *y -= 1;
                    *x += 1;
                });
            }
        });
        left.join().expect("the left task");
        right.join().expect("the right task");
        assert_eq!(a.get() + b.get(), 2_000, "nothing is created or lost");
    }

    /// **The witness door stores when nothing moved and refuses when
    /// something did** ([ADR-281](../../../docs/specification/adr/adr-281.md)
    /// D5), in both shapes — the two are one surface
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D9), so a door
    /// that behaved differently in one of them would be the thing D4 refuses.
    #[test]
    fn the_witness_door_compares_before_it_stores() {
        let local = Local::new(100i64);
        let seen = local.get();
        assert_eq!(local.set_after(seen + 23, &seen), Ok(()));
        assert_eq!(local.get(), 123);
        // The witness is stale now, and the value stays what somebody else
        // made it rather than being overwritten.
        assert_eq!(local.set_after(0, &seen), Err(Overtaken));
        assert_eq!(local.get(), 123);

        let crossing = Crossing::new(100i64);
        let seen = crossing.get();
        assert_eq!(crossing.set_after(seen + 23, &seen), Ok(()));
        assert_eq!(crossing.get(), 123);
        assert_eq!(crossing.set_after(0, &seen), Err(Overtaken));
        assert_eq!(crossing.get(), 123);
    }

    /// **The comparison is of the whole value**, which is what makes the door
    /// honest for something larger than a word — and what the message in
    /// [ADR-281](../../../docs/specification/adr/adr-281.md) D26 says a program
    /// that minds should write an `update` block with a version field for.
    #[test]
    fn a_large_value_is_compared_entry_by_entry() {
        let list = Local::new(vec![1i64, 2, 3]);
        let seen = list.get();
        assert_eq!(list.set_after(vec![1, 2, 3, 4], &seen), Ok(()));
        assert_eq!(list.set_after(vec![9], &seen), Err(Overtaken));
        assert_eq!(list.get(), vec![1, 2, 3, 4]);
    }

    /// **Two threads, one witness**: exactly one of them stores, which is the
    /// property the whole door exists for. A `get` and a `set` here would let
    /// both through and lose one of the two updates.
    #[test]
    fn only_one_of_two_racing_writers_gets_through() {
        let kasse = std::sync::Arc::new(Crossing::new(0i64));
        let seen = kasse.get();
        let one = std::sync::Arc::clone(&kasse);
        let left = std::thread::spawn(move || one.set_after(1, &seen));
        let two = std::sync::Arc::clone(&kasse);
        let right = std::thread::spawn(move || two.set_after(2, &seen));
        let outcomes = [left.join().expect("left"), right.join().expect("right")];
        assert_eq!(
            outcomes.iter().filter(|o| o.is_ok()).count(),
            1,
            "one stored and one was overtaken: {outcomes:?}"
        );
        assert!(kasse.get() == 1 || kasse.get() == 2);
    }

    /// A lock of one type beside a lock of another, which is the case a door
    /// written against one shape could not have.
    #[test]
    fn the_two_locks_need_not_hold_the_same_type() {
        let name = Local::new("kasse".to_string());
        let count = Local::new(3i64);
        let said = access_all(&name, &count, |n, c| format!("{n} {c}"));
        assert_eq!(said, "kasse 3");
    }
}

#[cfg(test)]
mod word {
    use super::*;

    #[test]
    fn a_word_is_changed_by_its_block_and_read_as_a_copy() {
        let n = Word::new(40_i64);
        n.update(|v| *v += 2);
        assert_eq!(n.get(), 42);
        assert_eq!(n.access(|v| *v * 2), 84);
        n.set(-1);
        assert_eq!(n.get(), -1);
        let flag = Word::new(false);
        flag.update(|v| *v = !*v);
        assert!(flag.get());
        let x = Word::new(1.5_f64);
        x.update(|v| *v *= 2.0);
        assert_eq!(x.get(), 3.0);
        let c = Word::new('a');
        c.update(|v| *v = 'z');
        assert_eq!(c.get(), 'z');
    }

    /// **A collision runs the block again, and nothing is lost** (ADR-281 D14):
    /// four threads adding to one word end at the sum.
    #[test]
    fn no_update_is_lost_under_contention() {
        let n = std::sync::Arc::new(Word::new(0_i64));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let n = std::sync::Arc::clone(&n);
                std::thread::spawn(move || {
                    for _ in 0..10_000 {
                        n.update(|v| *v += 1);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("the thread ends");
        }
        assert_eq!(n.get(), 40_000);
    }

    #[test]
    fn set_after_is_one_compare_and_swap() {
        let n = Word::new(5_i64);
        assert_eq!(n.set_after(6, &4), Err(Overtaken));
        assert_eq!(n.get(), 5);
        assert_eq!(n.set_after(6, &5), Ok(()));
        assert_eq!(n.get(), 6);
    }

    /// **A panic in the block leaves the word as it was** (ADR-281 D15).
    #[test]
    fn a_panic_in_the_block_leaves_the_word_untouched() {
        let n = Word::new(7_i64);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            n.update(|v| {
                *v = 100;
                panic!("in the block");
            })
        }));
        assert!(caught.is_err());
        assert_eq!(n.get(), 7);
    }
}
