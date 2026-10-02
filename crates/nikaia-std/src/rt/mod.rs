//! `std`'s runtime: started before `main`, and the one surface I/O goes
//! through.
//!
//! This module is
//! [ADR-038](../../../../docs/specification/adr/adr-038.md) D3, D4 and D5, and
//! it is deliberately *here* rather than in the compiler. ADR-033 §8.4 gave
//! the reason when it put the statement overlap behind
//! [`crate::task::both`]: the next change of mechanism must be a `std` change
//! and not a compiler change. Nothing about `io_uring`, `epoll`, a thread
//! count or a configuration key appears in a `.nika` file or in the emitted
//! Rust - a program says `fs::read_to_string(p)` and what happens underneath
//! is this file's business (Part I 8.1, Part III 15.x).
//!
//! ## D4 — it is running before the first statement the user wrote
//!
//! An I/O thread that is already running costs nothing per operation; one
//! started per pair of operations costs a wake-up per pair, which is the whole
//! of ADR-033 §8.4's finding. So the emitted `fn main` starts this and then
//! calls the program:
//!
//! ```rust,ignore
//! fn main() {
//!     let runtime = nikaia_std::rt::start(nikaia_std::rt::UserCode::Sequential);
//!     let outcome = nikaia_main();
//!     runtime.finish();
//!     outcome
//! }
//! ```
//!
//! What starts is what [ADR-037](../../../../docs/specification/adr/adr-037.md)
//! D2 allows. At `user_parallelism = no`: the I/O workers, and everything the
//! user wrote on the main thread. At `yes`: a pool for user code as well. The
//! asymmetry is D2's own - the compiler's threads were never bounded by that
//! switch, and the I/O thread is one of the compiler's ([ADR-016](../../../../docs/specification/adr/adr-016.md) D3).
//!
//! ## D3 — the two halves of I/O, behind one surface
//!
//! * **Files complete.** [`uring`] hands the read to the kernel and collects
//!   it; no thread performs it and none is woken for it.
//! * **Sockets signal readiness.** [`worker`]'s poller says when a descriptor
//!   can be read or written without blocking, and the caller does the transfer.
//! * **The fallback is the blocking path on an I/O worker**, for the machine
//!   that has no completion queue: an older kernel, a sandbox that forbids the
//!   syscalls, a target that never had them.
//!
//! **Which one is chosen is decided when the program starts, never when it is
//! compiled.** A binary built on a machine with `io_uring` runs on one without
//! it. The only compile-time gate is on `io_uring`'s *bindings*, which do not
//! exist off Linux at all; on Linux the choice is [`Method::Auto`] probing the
//! kernel, or an operator pinning it (D5).

pub mod config;
/// The executor (ADR-055 D3): what drives a program that can pause.
pub mod exec;
pub mod pool;
pub mod timer;
pub mod worker;

#[cfg(target_os = "linux")]
use file_ring as uring;

pub use config::{Config, Method};
pub use worker::Interest;

pub(crate) mod readiness;

use std::path::Path;
use std::sync::{Condvar, Mutex, OnceLock};

/// Whether the program's own code may run concurrently
/// ([ADR-037](../../../../docs/specification/adr/adr-037.md) D2), as the
/// runtime sees it.
///
/// The emitted `main` says which, because the *compiler* is what knows: it is
/// a build switch, not an operating property, and it is deliberately not one
/// of D5's four settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserCode {
    /// `user_parallelism = no`. Nothing the user wrote ever runs concurrently,
    /// so no pool for user code starts. The I/O workers still do: they are the
    /// compiler's.
    #[default]
    Sequential,
    /// `user_parallelism = yes`. A pool for user code starts with the runtime.
    Concurrent,
}

/// Which mechanism this process actually got, after detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Files {
    /// The kernel completes the read (D3's first half).
    Completion,
    /// The blocking path on an I/O worker (D3's fallback).
    Blocking,
}

impl Files {
    pub fn as_str(self) -> &'static str {
        match self {
            Files::Completion => "completion",
            Files::Blocking => "blocking",
        }
    }
}

/// The runtime. One per process, started before `main`.
pub struct Runtime {
    config: Config,
    user_code: UserCode,
    files: Files,
    workers: worker::Workers,
    #[cfg(target_os = "linux")]
    ring: Option<Mutex<uring::Ring>>,
    /// **The bell a worker rings to wake a park on `ring`**
    /// ([ADR-121](../../../docs/specification/adr/adr-121.md) D1), or `None`
    /// where this runtime has no ring.
    ///
    /// Beside the lock and not inside it, because the thread that rings cannot
    /// take the lock the thread it is waking is holding - `uring::ring_the_bell`
    /// says that at length. Per runtime and not per process: a test that builds
    /// a second `Runtime` gets a second ring with a bell of its own, and
    /// publishing that one as *the* bell left the process's own park deaf. Found
    /// by `the_compilers_pair_answers_the_same_either_way` hanging.
    #[cfg(target_os = "linux")]
    bell: Option<uring::Bell>,
    /// The executor for user tasks, at `user_parallelism = yes` and not
    /// otherwise.
    ///
    /// `None` at `Sequential` is not an optimisation. It is ADR-037 D2 as a
    /// thread count: there is no vehicle, so nothing the user wrote *can* run
    /// concurrently, whatever a later mistake in the emitter asks for.
    ///
    /// **Not `rayon`'s pool**, which is what stood here while the vehicle was a
    /// closure pair: a pool of closures cannot hold a future that pauses
    /// ([`pool`] says why at length). The worker count is the same one.
    user_pool: Option<std::sync::Arc<pool::Pool>>,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// **The bell an I/O worker rings when it has finished something**, and the
/// count of rings so far.
///
/// The completion path has the ring itself to wait on - one
/// `io_uring_enter` covers every operation in flight. The fallback has one
/// private reply channel per operation and no way at all to wait for
/// *whichever finishes first*, which is what an executor's park hook needs.
///
/// So a worker bumps the count and wakes whoever is parked, and the hook waits
/// for the count to move past the value the caller read **before** it polled.
/// That ordering is the whole of the correctness: a worker that finishes
/// between the poll and the park has already bumped the count, so the park
/// returns at once instead of waiting for a ring that will never come again.
///
/// It is a generation and not a queue: each future reads its own reply channel,
/// and all this has to say is *"something changed, poll again"*.
static FINISHED: Mutex<u64> = Mutex::new(0);
static BELL: Condvar = Condvar::new();

/// Rung by an I/O worker, after the operation and before it takes the next one.
///
/// **Both bells, always**
/// ([ADR-121](../../../docs/specification/adr/adr-121.md) D1). The condvar
/// above is what a thread parked off the ring waits on, and the `eventfd` on
/// the ring is what a thread parked *in* `io_uring_enter` waits on - a process
/// can have waiters of both kinds at the same instant, and which park a given
/// thread took is not a question this can ask. The second costs one write to a
/// descriptor nobody may be polling, which is the price of never having to ask.
pub(crate) fn ring_the_bell() {
    {
        let mut count = FINISHED.lock().unwrap_or_else(|e| e.into_inner());
        *count += 1;
        // Every parked thread, because any of them may be the one waiting for
        // this operation - and at `user_parallelism = no` there is exactly one.
        BELL.notify_all();
    }
    // **After the count**, and outside the lock it is under. Both orderings
    // matter: a park woken by this looks at a world the worker has already
    // finished changing, and `Ring::park`'s own check of the count is what
    // makes a ring that arrives between a poll and a park impossible to lose.
    //
    // `get` and not `handle`: this runs on a thread the runtime started, so the
    // runtime exists - and a worker that somehow rings while the `OnceLock` is
    // still being filled has nothing parked on a ring to wake.
    #[cfg(target_os = "linux")]
    if let Some(runtime) = RUNTIME.get()
        && let Some(bell) = &runtime.bell
    {
        bell.ring();
    }
}

/// Start the runtime, and hand back the guard whose `finish` drains it.
///
/// Called by the emitted `fn main` before the program's first statement (D4).
/// Calling it twice is not an error and does not start a second runtime - a
/// library's test harness and a generated `main` may both reach for it, and
/// the process has one runtime either way.
pub fn start(user_code: UserCode) -> Started {
    let _ =
        RUNTIME.get_or_init(|| Runtime::build(user_code, Config::load().unwrap_or_else(report)));
    Started { _private: () }
}

/// The runtime, starting it if `main` was not one this compiler emitted.
///
/// A `std` function must work in a unit test and in a `build.rs`, neither of
/// which has a generated `main`. That is a *fallback* and not the design: the
/// first operation in such a process pays the start, which is exactly the cost
/// D4 exists to remove.
pub fn handle() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        let user_code = ON_DEMAND.get().copied().unwrap_or_default();
        Runtime::build(user_code, Config::load().unwrap_or_else(report))
    })
}

/// What [`on_demand`] recorded, for the start [`handle`] does when something
/// first asks for the runtime.
static ON_DEMAND: OnceLock<UserCode> = OnceLock::new();

/// **The runtime of a `main` that cannot pause, started when it is first
/// needed** ([ADR-270](../../../docs/specification/adr/adr-270.md) D8 step 1).
///
/// ADR-038 D4 starts the runtime before the first statement so that an
/// operation inside the program costs no thread start - which is worth it for a
/// program that pauses. One whose `main` cannot pause has no operation that
/// needs the I/O workers, and starting them anyway put a second thread in the
/// process: the allocator then takes its locks on every allocation, which cost
/// the solver's kernels two per cent against the same loops in Rust
/// (`docs/solver-workload.md` §8). Here nothing starts until [`handle`] is
/// asked, which such a program normally never does.
pub fn on_demand(user_code: UserCode) -> Started {
    let _ = ON_DEMAND.set(user_code);
    Started { _private: () }
}

/// A configuration file that does not parse is the operator's to fix, and a
/// program that silently ran on the defaults instead would hide it. The
/// program still starts - refusing to start over a tuning file would be worse
/// - and the reason is on standard error, once.
fn report(why: String) -> Config {
    eprintln!("nikaia: {why}");
    eprintln!("nikaia: the runtime is starting on its default settings");
    Config::default()
}

/// What [`start`] hands back: the drain, as a value a generated `main` holds.
///
/// Not a `Drop` guard, deliberately. ADR-006 D5's drain has an outcome - it
/// names every resource that did not finish - and a destructor is the wrong
/// place for something that reports.
#[must_use = "the runtime is drained by calling `finish`"]
pub struct Started {
    _private: (),
}

impl Started {
    /// Drain, bounded by the configured deadline, and warn about what did not
    /// finish (ADR-006 D5).
    pub fn finish(self) {
        // A runtime that was never started has nothing to drain (`on_demand`).
        let Some(runtime) = RUNTIME.get() else {
            return;
        };
        let deadline = runtime.config.cleanup_deadline;
        if deadline.is_zero() {
            // "0" disables draining, which is ADR-006 D5's own word for it.
            return;
        }
        let left = runtime.workers.drain(deadline);
        // **And the readiness registrations**, which the worker drain used to
        // cover because a readiness wait *was* a worker operation. What is
        // waited for here is a wait that may never answer — a socket nobody
        // writes to — and the deadline is exactly what bounds it (ADR-006 D5).
        // Counted, not waited for a second deadline's worth: what is left of
        // the first one is what is left.
        let left = left + readiness::registry().drain(deadline);
        if left > 0 {
            expired(left, deadline);
        }
    }
}

/// **A cleanup that did not finish is a failure of the program**
/// ([ADR-112](../../../docs/specification/adr/adr-112.md) D1, D2).
///
/// **Exit 70**, which is `EX_SOFTWARE` from `sysexits.h` — the Unix convention
/// for *the program could not complete its work correctly*, which is what
/// happened. It is fixed, documented, and distinct from a panic's status (an
/// abort is `134` on Linux) so that a script can tell the two apart. A program
/// whose cleanups all finished exits as it would have anyway, and one built
/// with `cleanup-deadline = "0"` expires nothing (D3).
///
/// **Through the panic path**, which is D2 and is why this raises one rather
/// than printing: the message has to reach the program's **panic hook**, so
/// that whatever collects crash reports collects this. Standard output is
/// never written to — it belongs to the program's output and may be the very
/// file somebody is waiting for.
///
/// **What `panic = "abort"` costs is the status and not the message.** There
/// the process is gone before this returns, with the abort's own status; the
/// hook has already run and said what happened. The exit code is the one thing
/// a build for that profile cannot have, and it is named here rather than left
/// to be discovered.
///
/// **A cleanup is named** where one was cut off
/// ([ADR-239](../../../docs/specification/adr/adr-239.md) D5,
/// [`cleanups_expired`]); what is left after every cleanup finished is I/O
/// nobody waits for by name, and that is counted.
fn expired(left: usize, deadline: std::time::Duration) {
    ended_by_the_deadline(format!(
        "nikaia: {left} pending I/O operation(s) did not finish within the {}s cleanup \
         deadline and were abandoned",
        deadline.as_secs_f64()
    ));
}

/// **The cleanups an expired deadline cut off, each by name** (ADR-239 D5,
/// ADR-112 D2): the same status and the same path as [`expired`].
pub(crate) fn cleanups_expired(cut: &[String], deadline: std::time::Duration) {
    let named: Vec<String> = cut
        .iter()
        .map(|what| {
            format!(
                "nikaia: the cleanup of {what} did not finish within the {}s cleanup \
                 deadline and was abandoned",
                deadline.as_secs_f64()
            )
        })
        .collect();
    ended_by_the_deadline(named.join("\n"));
}

fn ended_by_the_deadline(said: String) {
    // The hook runs on the way out of this, which is the whole reason for the
    // panic: it is the program's own, and a crash-report collector's if one is
    // installed.
    let _ = std::panic::catch_unwind(move || panic!("{said}"));
    std::process::exit(EXIT_CLEANUP_EXPIRED);
}

/// `EX_SOFTWARE` from `sysexits.h`
/// ([ADR-112](../../../docs/specification/adr/adr-112.md) D1).
pub const EXIT_CLEANUP_EXPIRED: i32 = 70;

impl Runtime {
    fn build(user_code: UserCode, config: Config) -> Runtime {
        let workers = worker::Workers::start(config.io_workers);

        #[cfg(target_os = "linux")]
        let ring = match config.io_method {
            // **Run-time detection.** `IoUring::new` is the probe: an older
            // kernel answers `ENOSYS` and a sandbox that forbids the syscall
            // answers `EPERM`, and either way the program runs on the
            // blocking path rather than failing to start.
            Method::Auto => uring::Ring::open().ok().map(Mutex::new),
            Method::Uring => match uring::Ring::open() {
                Ok(ring) => Some(Mutex::new(ring)),
                // Pinned, and not available: a failure to start. The whole
                // point of pinning a mechanism is to find out whether it is
                // there, so falling back silently would defeat it (D5).
                Err(e) => panic!(
                    "nikaia: `io-method = \"uring\"` was pinned in {} and this machine \
                     has no io_uring ({e}); `auto` falls back, `uring` does not",
                    config::FILE
                ),
            },
            Method::Blocking => None,
        };

        #[cfg(target_os = "linux")]
        let files = if ring.is_some() {
            Files::Completion
        } else {
            Files::Blocking
        };
        #[cfg(not(target_os = "linux"))]
        let files = Files::Blocking;

        let user_pool = match user_code {
            UserCode::Sequential => None,
            UserCode::Concurrent => Some(pool::Pool::start(config.user_pool)),
        };

        // Read out of the ring before the lock closes over it: a `write` may
        // not wait for a park (D1), so the number lives beside the lock.
        #[cfg(target_os = "linux")]
        let bell = ring
            .as_ref()
            .map(|ring| ring.lock().unwrap_or_else(|e| e.into_inner()).bell());

        Runtime {
            config,
            user_code,
            files,
            workers,
            #[cfg(target_os = "linux")]
            ring,
            #[cfg(target_os = "linux")]
            bell,
            user_pool,
        }
    }

    /// The four settings this process started on.
    pub fn config(&self) -> Config {
        self.config
    }

    /// Which mechanism files actually got, after detection.
    pub fn files(&self) -> Files {
        self.files
    }

    /// What `user_parallelism` said.
    pub fn user_code(&self) -> UserCode {
        self.user_code
    }

    /// The executor for user tasks, which exists only at
    /// `user_parallelism = yes`.
    pub fn user_pool(&self) -> Option<&std::sync::Arc<pool::Pool>> {
        self.user_pool.as_ref()
    }

    /// How many I/O operations are queued and unanswered.
    pub fn pending(&self) -> usize {
        // **And the readiness registrations**, which used to be worker
        // operations and are counted here for the reason they were counted
        // there: the park asks *is anything outstanding* before it sleeps, and
        // ADR-006 D5's drain asks it before it lets a program go. What changed
        // in 0.0.165 is the mechanism and not the answer ([`readiness`]).
        self.workers.pending() + readiness::registry().outstanding()
    }

    /// One line naming what started and how, for `--runtime` and for a bug
    /// report that has to say which path it was on.
    pub fn describe(&self) -> String {
        format!(
            "io-workers={} io-method={} (chose {}) user-pool={} cleanup-deadline={:?} user-code={}",
            self.config.io_workers,
            self.config.io_method.as_str(),
            self.files.as_str(),
            match self.user_pool.as_ref() {
                Some(pool) => pool.threads().to_string(),
                None => "none".to_string(),
            },
            self.config.cleanup_deadline,
            match self.user_code {
                UserCode::Sequential => "sequential",
                UserCode::Concurrent => "concurrent",
            }
        )
    }

    #[cfg(target_os = "linux")]
    fn with_ring<T>(&self, f: impl FnOnce(&mut uring::Ring) -> T) -> Option<T> {
        let ring = self.ring.as_ref()?;
        // A panic inside a file operation poisons this lock and must not lose
        // the ring: the buffers the kernel holds live in it, and dropping them
        // is the one thing `uring`'s soundness rule forbids. Recovering is
        // sound because the *next* operation reconciles the ring before it
        // submits anything.
        let mut held = ring.lock().unwrap_or_else(|e| e.into_inner());
        let before = held.answered();
        let out = f(&mut held);
        // **An answer this call reaped may be another thread's**, and that
        // thread learns of it here or not at all. The ring is shared: a poll
        // or a park on one thread takes every completion off the queue, so a
        // thread whose read finished while somebody else held the ring finds
        // its slot answered and nothing else changed - no count moved, no bell
        // rang - and a park it enters next waits for a completion that has
        // already come. Where its read needs another turn, which only its own
        // poll submits, that wait is for ever while any other operation is
        // outstanding, with the ring's lock held and every thread behind it.
        // Found as the arm64 CI job hanging in
        // `many_threads_reading_and_writing_at_once_each_get_their_own_answer`
        // (0.0.259).
        //
        // **Counted while the ring is still held**, which is what makes it
        // impossible to miss: [`io::park_for`] reads the count under this same
        // lock before it waits, so a park that follows this call sees it moved
        // and returns, and one that came before it is the one that reaped.
        if held.answered() != before {
            let mut count = FINISHED.lock().unwrap_or_else(|e| e.into_inner());
            *count += 1;
            BELL.notify_all();
        }
        Some(out)
    }
}

/// I/O, as the one surface D3 asks for.
///
/// Every function here is what a Nikaia program reaches through `std::fs` and
/// `std::net`. Which mechanism serves it is decided inside, at startup, and a
/// caller cannot tell - which is what makes a change of mechanism a `std`
/// change.
pub mod io {
    use super::{Files, Interest, Path, handle, worker};
    use std::io::{Error, Result};

    /// The invariant ADR-037 D2 rests on, checked rather than assumed.
    ///
    /// The I/O thread is the compiler's, so `std`'s user-facing functions are
    /// never *on* it: one of them being called from a worker would mean a
    /// worker had been handed something other than an [`worker::Op`], which
    /// the type system already forbids. A `debug_assert` is the belt for the
    /// braces.
    fn off_the_io_thread() {
        debug_assert!(
            !worker::on_io_worker(),
            "a `std` I/O call reached an I/O worker; the worker's inbox takes \
             operations, never code (ADR-037 D2)"
        );
    }

    /// A whole file, as bytes.
    pub fn read(path: &Path) -> Result<Vec<u8>> {
        off_the_io_thread();
        let mut both = read_all(&[path]);
        both.pop().expect("one path, one answer")
    }

    /// Two files, **both in flight at once**, answered in the order they were
    /// asked for.
    ///
    /// This is the shape ADR-033 §8.5 predicted and could not measure: two
    /// operations that meet on nothing, neither waiting for the other, and no
    /// thread started or woken for the pair. `benches/overlap/src/bin/runtime.rs`
    /// is what puts the number on it.
    ///
    /// It is `std`'s own function over `std`'s own operations, which is why it
    /// exists at `user_parallelism = no`: nothing the *user* wrote runs
    /// concurrently here, and the two reads are `std`'s
    /// ([ADR-037](../../../../docs/specification/adr/adr-037.md) D2).
    pub fn read_both(a: &Path, b: &Path) -> (Result<Vec<u8>>, Result<Vec<u8>>) {
        off_the_io_thread();
        let mut both = read_all(&[a, b]);
        let second = both.pop().expect("two paths, two answers");
        let first = both.pop().expect("two paths, two answers");
        (first, second)
    }

    /// Every path, all of them in flight at once. The one place the mechanism
    /// is chosen.
    fn read_all(paths: &[&Path]) -> Vec<Result<Vec<u8>>> {
        let runtime = handle();
        match runtime.files() {
            #[cfg(target_os = "linux")]
            Files::Completion => runtime
                .with_ring(|ring| ring.read_many(paths))
                .expect("`Files::Completion` means there is a ring"),
            #[cfg(not(target_os = "linux"))]
            Files::Completion => unreachable!("no completion queue off Linux"),
            Files::Blocking => blocking_read_all(paths),
        }
    }

    /// D3's fallback, for the machine with no completion queue.
    ///
    /// **The last operation of a batch runs on the calling thread**, and the
    /// rest go to the I/O workers, which are already running - so a pair of
    /// reads is *one* message rather than two, and a lone read is none at all.
    ///
    /// That split is the measurement, not a micro-optimisation. A caller that
    /// is going to block until the answer arrives gains nothing from handing
    /// its own work to another thread and waiting for it: it pays a wake-up
    /// and buys no overlap. Where a *pair* is asked for, the second operation
    /// has something to overlap with and the wake-up buys something - and that
    /// wake-up is exactly the cost ADR-033 §8.4 measured and could not remove,
    /// which is why D3 does not put files here when the kernel can complete
    /// them instead.
    fn blocking_read_all(paths: &[&Path]) -> Vec<Result<Vec<u8>>> {
        let runtime = handle();
        let (queued, here) = paths.split_at(paths.len().saturating_sub(1));

        let mut waiting = Vec::with_capacity(queued.len());
        for path in queued {
            let (reply, answer) = std::sync::mpsc::channel();
            let sent = runtime.workers.send(worker::Op::Read {
                path: path.to_path_buf(),
                reply,
            });
            waiting.push(sent.then_some(answer));
        }

        // Started last and finished first: the operations already in flight
        // are the ones being overlapped with.
        let mine = here.first().map(std::fs::read);

        let mut out: Vec<Result<Vec<u8>>> = waiting
            .into_iter()
            .map(|answer| match answer {
                Some(answer) => answer
                    .recv()
                    .unwrap_or_else(|_| Err(Error::other("the runtime's I/O worker went away"))),
                None => Err(Error::other("the runtime has already been drained")),
            })
            .collect();
        out.extend(mine);
        out
    }

    /// A whole file, written.
    ///
    /// **The copy happens where the kernel needs it and nowhere else.** On the
    /// completion path the buffer handed over has to outlive the submission, so
    /// `std` gives the ring a `Vec` it owns outright - that is `rt::uring`'s
    /// soundness rule and not a convenience, and there is no borrowed buffer to
    /// keep alive because there is no borrowed buffer. On the blocking path the
    /// caller is blocked for the whole of the write, so the bytes are written
    /// straight out of the caller's own slice and nothing is copied.
    pub fn write(path: &Path, bytes: &[u8], append: bool, create: bool) -> Result<()> {
        off_the_io_thread();
        let runtime = handle();
        match runtime.files() {
            #[cfg(target_os = "linux")]
            Files::Completion => runtime
                .with_ring(|ring| ring.write(path, bytes.to_vec(), append, create))
                .expect("`Files::Completion` means there is a ring"),
            #[cfg(not(target_os = "linux"))]
            Files::Completion => unreachable!("no completion queue off Linux"),
            // One operation, and the caller is going to wait for it: a worker
            // would cost a wake-up and buy no overlap.
            Files::Blocking => worker::blocking_write(path, bytes, append, create),
        }
    }

    /// **A file operation in flight**, whichever mechanism is serving it.
    ///
    /// D3's one surface, as a value: a caller polls this and cannot tell
    /// whether the kernel is doing the read or an I/O worker is. That is the
    /// same claim `read` above makes for a caller that waits, and it is the
    /// claim [ADR-055](../../../../docs/specification/adr/adr-055.md) §6 step 3
    /// needs for one that suspends instead.
    pub enum InFlight {
        /// A slot on the ring.
        #[cfg(target_os = "linux")]
        Ring(usize),
        /// An I/O worker's reply, on its own channel.
        Worker(std::sync::mpsc::Receiver<Result<Vec<u8>>>),
        /// Finished before it was ever polled - a path that does not exist
        /// fails at the `open`, and a write on the fallback runs on the calling
        /// thread.
        ///
        /// `None` once it has been taken, which is what makes polling it twice
        /// an error rather than a second answer.
        Done(Option<Result<Vec<u8>>>),
    }

    impl Drop for InFlight {
        /// **A slot given back without its answer being taken.**
        ///
        /// A future dropped before it finished - a `catch` that diverted, a
        /// task nobody polled again - still holds a ring slot, and the ring
        /// cannot tell that from one somebody is about to come back for. So the
        /// handle says so itself: `abandon` stops the slot being anybody's, and
        /// the next operation reclaims it once the kernel's completions for its
        /// buffer are in (`uring::Job::owned`, rule 2).
        ///
        /// The buffer is *not* freed here, and that is the soundness rule
        /// rather than an omission.
        fn drop(&mut self) {
            #[cfg(target_os = "linux")]
            if let InFlight::Ring(slot) = self {
                let slot = *slot;
                handle().with_ring(|ring| ring.abandon(slot));
            }
        }
    }

    /// Start reading `path` whole, and hand back the operation.
    ///
    /// Nothing here waits. The mechanism is chosen exactly where `read_all`
    /// chooses it, and for the same reason: the choice is `std`'s, once, at
    /// startup.
    pub fn begin_read(path: &Path) -> InFlight {
        off_the_io_thread();
        let runtime = handle();
        match runtime.files() {
            #[cfg(target_os = "linux")]
            Files::Completion => {
                match runtime
                    .with_ring(|ring| ring.begin_read(path))
                    .expect("`Files::Completion` means there is a ring")
                {
                    Ok(slot) => InFlight::Ring(slot),
                    // The `open` or the `stat` failed, which is a path walk and
                    // not a transfer: there was never anything on the ring.
                    Err(e) => InFlight::Done(Some(Err(e))),
                }
            }
            #[cfg(not(target_os = "linux"))]
            Files::Completion => unreachable!("no completion queue off Linux"),
            Files::Blocking => {
                let (reply, answer) = std::sync::mpsc::channel();
                match runtime.workers.send(worker::Op::Read {
                    path: path.to_path_buf(),
                    reply,
                }) {
                    true => InFlight::Worker(answer),
                    false => InFlight::Done(Some(Err(Error::other(
                        "the runtime has already been drained",
                    )))),
                }
            }
        }
    }

    /// Start writing `bytes` to `path`, and hand back the operation.
    ///
    /// **On the fallback this is not in flight at all**, and that is D3's own
    /// split rather than a shortcut: `worker::blocking_write`'s comment says a
    /// caller that is going to wait for its own write gains nothing from
    /// handing it to another thread, and there is no `Op` for one. So on that
    /// path the write happens here and the operation is already `Done` - which
    /// a caller cannot tell apart from a very fast completion.
    pub fn begin_write(path: &Path, bytes: &[u8], append: bool, create: bool) -> InFlight {
        off_the_io_thread();
        let runtime = handle();
        match runtime.files() {
            #[cfg(target_os = "linux")]
            Files::Completion => {
                match runtime
                    .with_ring(|ring| ring.begin_write(path, bytes.to_vec(), append, create))
                    .expect("`Files::Completion` means there is a ring")
                {
                    Ok(slot) => InFlight::Ring(slot),
                    Err(e) => InFlight::Done(Some(Err(e))),
                }
            }
            #[cfg(not(target_os = "linux"))]
            Files::Completion => unreachable!("no completion queue off Linux"),
            Files::Blocking => InFlight::Done(Some(
                worker::blocking_write(path, bytes, append, create).map(|()| Vec::new()),
            )),
        }
    }

    /// Whether the operation has finished, and what it came to if it has.
    ///
    /// **Never blocks.** A future polls this and returns `Pending` on a `None`,
    /// having first read [`generation`] so that a completion arriving between
    /// the two cannot be missed.
    pub fn poll(operation: &mut InFlight) -> Option<Result<Vec<u8>>> {
        match operation {
            #[cfg(target_os = "linux")]
            InFlight::Ring(slot) => {
                let slot = *slot;
                let done = handle()
                    .with_ring(|ring| ring.poll_slot(slot))
                    .expect("`Files::Completion` means there is a ring");
                if done.is_some() {
                    // **The slot is not ours any more**, so `Drop` must not
                    // give it back: `poll_slot` cleared it when it handed the
                    // answer over, and by the time this value is dropped the
                    // index may already belong to another operation - giving
                    // *that* one's slot away is the defect `Job::owned` exists
                    // to prevent.
                    //
                    // **Replaced without being dropped.** A plain assignment
                    // drops the old `Ring(slot)`, which is exactly the `Drop`
                    // this comment forbids, run after the lock was released:
                    // another thread had taken the slot in between, lost it
                    // to `abandon`, and was answered *the runtime lost a file
                    // operation's slot*
                    // (`fs::tests::many_threads_reading_and_writing_at_once_each_get_their_own_answer`).
                    // The old value is an index and nothing else, so
                    // forgetting it leaks nothing.
                    std::mem::forget(std::mem::replace(operation, InFlight::Done(None)));
                }
                done
            }
            InFlight::Worker(answer) => match answer.try_recv() {
                Ok(done) => Some(done),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err(Error::other("the runtime's I/O worker went away")))
                }
            },
            InFlight::Done(done) => Some(done.take().unwrap_or_else(|| {
                Err(Error::other(
                    "a file operation was asked for its answer twice",
                ))
            })),
        }
    }

    /// **A file operation, as something that can be awaited.**
    ///
    /// The whole of what makes a `std` read a suspension point: it polls the
    /// operation, and where the operation has not finished it returns
    /// `Pending` **with the waker recorded by the executor rather than here**.
    /// That is not a shortcut - the executor is the only thing on this thread
    /// that parks, and it parks in the I/O (`exec::block_on`), so it is already
    /// awake when the answer arrives and re-arms what it is holding. A waker
    /// stored here would be a second mechanism for the same wake.
    pub struct Reading {
        operation: InFlight,
    }

    impl std::future::Future for Reading {
        type Output = Result<Vec<u8>>;

        fn poll(
            self: std::pin::Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            match poll(&mut self.get_mut().operation) {
                Some(done) => std::task::Poll::Ready(done),
                None => {
                    // **No waker is stored and none is rung**, which is the one
                    // place this executor differs from a general one. What this
                    // waits for is the I/O, the executor is the only thing on
                    // this thread that parks, and it parks *in* the I/O
                    // (`exec::block_on`) - so when the answer arrives the
                    // executor is already awake and re-arms everything it is
                    // holding. Ringing the waker here instead would be a spin:
                    // a ready task with an unfinished read, polled round after
                    // round, and never a park to wait in.
                    let _ = context;
                    std::task::Poll::Pending
                }
            }
        }
    }

    /// Read `path` whole, as a future.
    pub fn reading(path: &Path) -> Reading {
        Reading {
            operation: begin_read(path),
        }
    }

    /// Write `bytes` to `path`, as a future.
    pub fn writing(path: &Path, bytes: &[u8], append: bool, create: bool) -> Reading {
        Reading {
            operation: begin_write(path, bytes, append, create),
        }
    }

    /// The count of I/O completions the runtime has seen, read **before** a
    /// poll so that [`park`] cannot wait for a ring that has already been rung.
    pub fn generation() -> u64 {
        *super::FINISHED.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **Wait for the bell**, which is what a thread that is *not* driving the
    /// I/O has to wait on.
    ///
    /// At `user_parallelism = yes` the pool's pilot owns the I/O park
    /// ([`crate::rt::pool`] says why exactly one thread may), so the main
    /// thread cannot take it — and on the completion path it must not try:
    /// a thread inside `io_uring_enter` is woken by the kernel and by nothing
    /// else, so a task on another thread filling the slot `main` is joining
    /// would never reach it.
    ///
    /// What everything that could wake `main` has in common is that it rings
    /// this bell: an I/O worker after an operation, the pilot after a
    /// completion, a `Slot` being filled, a task finishing. So this is the one
    /// wait, and `false` means the bound ran out with nothing having moved.
    pub fn wait_for_bell(since: u64, limit: Option<std::time::Duration>) -> bool {
        let mut count = super::FINISHED.lock().unwrap_or_else(|e| e.into_inner());
        match limit {
            None => {
                while *count <= since {
                    count = super::BELL.wait(count).unwrap_or_else(|e| e.into_inner());
                }
                true
            }
            Some(limit) => {
                let until = std::time::Instant::now() + limit;
                while *count <= since {
                    let left = until.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() {
                        return false;
                    }
                    let (held, timed_out) = super::BELL
                        .wait_timeout(count, left)
                        .unwrap_or_else(|e| e.into_inner());
                    count = held;
                    if timed_out.timed_out() && *count <= since {
                        return false;
                    }
                }
                true
            }
        }
    }

    /// **Wait for the I/O to move**, and nothing else. The executor's park hook.
    ///
    /// `since` is what [`generation`] answered before the poll that found
    /// nothing to do. `false` means there is nothing outstanding to wait for,
    /// which tells the executor that waiting would be waiting forever - and a
    /// hang is the worst way to report a defect (§6 step 1).
    pub fn park(since: u64) -> bool {
        park_for(since, None)
    }

    /// The same, giving up after `limit` if one is given.
    ///
    /// **The drain at the end of a program is what wants a bound**
    /// ([ADR-006](../../../../docs/specification/adr/adr-006.md) D5): the
    /// executor waits for the tasks nobody joined, and a task that never
    /// finishes must not become a program that never exits.
    ///
    /// **Honest limit, and it is D5's own.** The timer here is checked *between*
    /// parks rather than driven independently, because a single-threaded
    /// executor has no second thread to drive one from — so a park already
    /// entered runs to its own end. On the fallback that end is bounded, because
    /// the wait takes the remaining time; on the completion path it is the
    /// kernel posting the completion for work it has already accepted. What D5
    /// names as the thing a deadline cannot cover — FFI that blocks the thread
    /// rather than pausing — is unchanged by this.
    pub fn park_for(since: u64, limit: Option<std::time::Duration>) -> bool {
        let runtime = handle();
        match runtime.files() {
            #[cfg(target_os = "linux")]
            // **A worker operation is something to wait for here too**
            // ([ADR-121](../../../docs/specification/adr/adr-121.md) D1): the
            // bell is armed on the ring, so a worker's reply posts a completion
            // and the park returns. What the ring cannot know is that there is
            // one coming, because D2 keeps the bell out of its job count - so
            // the worker count is read here and handed down.
            Files::Completion => runtime
                .with_ring(|ring| {
                    ring.park(since, runtime.pending() > 0, limit, super::io::generation)
                })
                .expect("`Files::Completion` means there is a ring"),
            #[cfg(not(target_os = "linux"))]
            Files::Completion => unreachable!("no completion queue off Linux"),
            Files::Blocking => {
                let mut count = super::FINISHED.lock().unwrap_or_else(|e| e.into_inner());
                // **The count is read before `pending()`, and that ordering is
                // the correctness** — the same sentence `exec::block_on` writes
                // above its own read, one layer down, and the layer where it
                // was missing.
                //
                // An operation answered *between* the caller's poll and this
                // call has already left `pending`, so asking *"is anything
                // outstanding?"* first answers **no** about a reply that is
                // sitting in a channel nobody has looked at since. The caller
                // then hears *nothing can move* and either spins out its
                // `cleanup-deadline` (`block_on`'s drain: *1 background task(s)
                // did not finish*) or panics about a waker nobody arranged —
                // both about a task whose answer had already arrived.
                //
                // Measured before this: `a_future_fed_from_a_worker_finishes_under_block_on`
                // went red about two runs in five of its binary.
                if *count > since {
                    return true;
                }
                if runtime.pending() == 0 {
                    return false;
                }
                match limit {
                    None => {
                        while *count <= since {
                            count = super::BELL.wait(count).unwrap_or_else(|e| e.into_inner());
                        }
                        true
                    }
                    Some(limit) => {
                        let until = std::time::Instant::now() + limit;
                        while *count <= since {
                            let left = until.saturating_duration_since(std::time::Instant::now());
                            if left.is_zero() {
                                return false;
                            }
                            let (held, timed_out) = super::BELL
                                .wait_timeout(count, left)
                                .unwrap_or_else(|e| e.into_inner());
                            count = held;
                            if timed_out.timed_out() && *count <= since {
                                return false;
                            }
                        }
                        true
                    }
                }
            }
        }
    }

    /// Wait until a socket can be read or written without blocking.
    ///
    /// D3's other half: a socket is *not* a file. `epoll` answers a socket
    /// exactly and answers a file uselessly, which is why the two halves have
    /// different mechanisms behind this one surface. `Ok(false)` is the
    /// timeout expiring, which a deadline has to be able to tell from a
    /// failure.
    ///
    /// The wait happens on an I/O worker, which is already running - so
    /// waiting costs a message rather than a thread.
    pub fn wait(
        socket: &impl std::os::fd::AsFd,
        interest: Interest,
        timeout: Option<std::time::Duration>,
    ) -> Result<bool> {
        off_the_io_thread();
        super::readiness::arm(socket, interest, timeout).blocking()
    }

    /// **The same wait, as something that can be awaited**
    /// ([ADR-121](../../../docs/specification/adr/adr-121.md) D4).
    ///
    /// This is what the record is for. The reply arrives on an I/O worker, and a
    /// worker rings a bell that - since D1 - *both* parks hear, so a `Pending`
    /// from here is one the executor can sleep on whichever mechanism the
    /// process got. Before D1 it could not: on the completion path the park
    /// answered off a count of ring jobs, a worker's reply was not one, and a
    /// future fed from one either spun or met `exec::block_on`'s panic about a
    /// waker nobody arranged.
    ///
    /// **No waker is stored**, for the reason [`Reading`] gives at length: the
    /// executor is the only thing on this thread that parks, and it parks in the
    /// I/O.
    ///
    /// The descriptor the worker polls is a **duplicate** of this one
    /// ([`worker::Op::Readiness`] says why), so a `Waiting` dropped before its
    /// answer arrives leaves the worker with a descriptor of its own rather than
    /// with a number the caller may since have closed.
    pub fn waiting(
        socket: &impl std::os::fd::AsFd,
        interest: Interest,
        timeout: Option<std::time::Duration>,
    ) -> Waiting {
        off_the_io_thread();
        super::readiness::arm(socket, interest, timeout)
    }

    /// **All of standard input, as a future**
    /// ([ADR-121](../../../docs/specification/adr/adr-121.md) D4).
    ///
    /// The read itself is the blocking one standard input always had, moved onto
    /// an I/O worker: D4 asks for *no second read shape and no ring path for a
    /// stream that has no size to `stat`*. What is new is not the read but the
    /// **park**. A worker's reply wakes the executor on either mechanism now, so
    /// the thread the program runs on is given up here instead of held.
    pub fn stdin_whole() -> Replied<Vec<u8>> {
        off_the_io_thread();
        let runtime = handle();
        let (reply, answer) = std::sync::mpsc::channel();
        let queued = runtime.workers.send(worker::Op::Stdin { reply });
        Replied {
            answer: match queued {
                true => Ok(answer),
                false => Err(Error::other("the runtime has already been drained")),
            },
        }
    }

    /// **Blocking work on a thread of its own, as a future**
    /// ([ADR-243](../../../docs/specification/adr/adr-243.md) D3): what
    /// `process::run` waits on. The park is [`stdin_whole`]'s, and the thread
    /// is not an I/O worker's, so a child that runs for a minute holds up no
    /// read.
    pub fn beside<T: Send + 'static>(
        work: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Replied<T> {
        off_the_io_thread();
        let runtime = handle();
        Replied {
            answer: runtime
                .workers
                .beside(work)
                .ok_or_else(|| Error::other("no thread could be started for the wait")),
        }
    }

    /// **The next chunk of standard input, as something that can be awaited**
    /// ([ADR-172](../../../docs/specification/adr/adr-172.md) D4).
    ///
    /// The same shape as [`stdin_whole`] with a size on it: a stream a program
    /// walks a line at a time is read a buffer at a time, and the line endings
    /// are found by the caller.
    pub fn stdin_chunk(want: usize) -> Replied<Vec<u8>> {
        off_the_io_thread();
        let runtime = handle();
        let (reply, answer) = std::sync::mpsc::channel();
        let queued = runtime.workers.send(worker::Op::StdinChunk { want, reply });
        Replied {
            answer: match queued {
                true => Ok(answer),
                false => Err(Error::other("the runtime has already been drained")),
            },
        }
    }

    /// What [`waiting`] hands back.
    ///
    /// **A registration and not a worker's reply since 0.0.165**
    /// ([`super::readiness`]): a readiness wait used to occupy an I/O worker
    /// for its whole duration, and `io-workers` is `1` by default — so a wait
    /// that had not answered blocked every other wait in the process.
    pub type Waiting = super::readiness::Armed;

    /// **A worker's reply, as something that can be awaited**
    /// ([ADR-121](../../../docs/specification/adr/adr-121.md) D4).
    ///
    /// One shape for every worker operation, because they differ only in what
    /// comes back: the reply is a channel, the poll is a `try_recv`, and the
    /// wake is the bell the worker rings when it is done.
    pub struct Replied<T> {
        /// The reply channel, or the reason there is none - a runtime already
        /// drained, which is an answer and not a wait.
        answer: Result<std::sync::mpsc::Receiver<Result<T>>>,
    }

    impl<T> std::future::Future for Replied<T> {
        type Output = Result<T>;

        fn poll(
            self: std::pin::Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            use std::task::Poll;

            let _ = context;
            let answer = match self.get_mut().answer.as_mut() {
                Ok(answer) => answer,
                // The `Err` is taken by value on the first poll and the channel
                // that is not there cannot be polled again - a second poll of a
                // finished future is the caller's mistake either way, and this
                // is the message it gets rather than a hang.
                Err(e) => return Poll::Ready(Err(Error::other(e.to_string()))),
            };
            match answer.try_recv() {
                Ok(done) => Poll::Ready(done),
                Err(std::sync::mpsc::TryRecvError::Empty) => Poll::Pending,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Poll::Ready(Err(Error::other("the runtime's I/O worker went away")))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A worker operation wakes the executor on either path**
    /// ([ADR-121](../../../docs/specification/adr/adr-121.md) D1 and D3).
    ///
    /// **This test used to assert the opposite**, and that is why it is here.
    /// The bell was the **fallback's**: one private reply channel per operation
    /// and no way to wait for *whichever finishes first*, so a worker bumped a
    /// count and the park hook watched it. On the completion path the hook
    /// waited on the **ring** instead, and `Ring::park` answered off `unreaped`,
    /// which counts ring jobs — a worker operation is not one, so the hook said
    /// *there is nothing to wait for* while something was plainly in flight. No
    /// future could be fed from a worker reply: `exec::block_on` either spun
    /// (`main` alone) or panicked about a waker nobody arranged.
    ///
    /// D1 puts an `eventfd` on the ring with a poll always armed and has the
    /// bell write to it, so a worker's reply completes a ring job and the park
    /// returns. D3 keeps the test and inverts the claim, because *a hang is the
    /// failure this may not have*: what went red the day the defect existed goes
    /// red the day it comes back.
    #[test]
    fn a_worker_operation_wakes_the_park_on_either_path() {
        let runtime = handle();
        // A pipe with nothing in it: the readiness wait will not finish, so the
        // operation is still in flight while this thread asks about the park.
        let (reader, _writer) = std::io::pipe().expect("a pipe");
        let waiting = std::thread::spawn(move || {
            let _ = io::wait(
                &reader,
                Interest::Readable,
                Some(std::time::Duration::from_secs(2)),
            );
        });
        // Until the worker has taken it, there is nothing to say.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while runtime.pending() == 0 && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        // **At least one and not exactly one.** `pending` is the *process's*
        // count and the harness runs these tests side by side, so another
        // test's read may be in flight at the same instant — and what this
        // asserts is that **something** is, which is the whole of what the park
        // is then asked about. An exact count was a claim about the other
        // tests' scheduling and went red the day one was added.
        assert!(runtime.pending() >= 1, "the readiness wait is queued");

        // **The claim, and it is about the park hook rather than about time.**
        // `park_for` answers `false` for *there is nothing outstanding to wait
        // for*, which is what the ring park used to say about a worker
        // operation. The readiness wait above takes two seconds to time out, so
        // this park is entered with the operation in flight on either path, and
        // what is asserted is that the hook did **not** claim there was nothing
        // to wait for.
        //
        // **`false` is two answers, so the time is what tells them apart.**
        // *Nothing to wait for* is the early return and comes back at once;
        // *the bound ran out and nothing moved* comes back after the 50 ms -
        // which is what a park with a two-second wait in flight should do. The
        // test used to read `true` only, which it got when some other test's
        // operation happened to finish inside the window: alone, it failed
        // every time, and beside others it failed on a slower runner.
        let started = std::time::Instant::now();
        let moved = io::park_for(io::generation(), Some(std::time::Duration::from_millis(50)));
        let waited = started.elapsed() >= std::time::Duration::from_millis(40);
        assert!(
            moved || waited,
            "the park said there was nothing to wait for while a worker \
             operation was in flight on the {} path; ADR-121 D1's eventfd is \
             what makes a worker's reply something the ring park can wait for",
            runtime.files().as_str()
        );
        let _ = waiting.join();
    }

    /// **A future fed from a worker finishes under `block_on`, with other tasks
    /// alive** ([ADR-121](../../../docs/specification/adr/adr-121.md) D3).
    ///
    /// The other half of the same claim, through the executor rather than
    /// through the hook — and the half that could not be written at all before
    /// D1. `io::waiting` is a worker operation as a future: it polls a reply
    /// channel and stores no waker, so the only thing that can wake it is the
    /// park, and the park is what D1 changed.
    ///
    /// *With other tasks alive*, because `main` alone is the case the defect
    /// **hid**: with nothing else to poll, `block_on` spun round its loop and
    /// eventually got the answer. A started task is what makes the executor park
    /// properly, and a park that could not hear the worker is a program that
    /// never ends.
    ///
    /// **Which is why `block_on` is driven on a thread of its own and joined
    /// with a bound.** Its park is deliberately unbounded while `main` is still
    /// running - an unbounded wait is what a program asked for - so a bell that
    /// is not heard is a hang rather than a panic, and a test that hung would
    /// report this defect by never finishing. The bound is the harness this test
    /// brings with it.
    ///
    /// **The bound is generous on purpose**, and what it is generous about is not
    /// the mechanism. `io_workers` is **one** by default and this harness runs the
    /// suite in parallel, so every in-process readiness wait queues behind the
    /// others on one thread, and a wait ahead of these two holds it for as long as
    /// its own timeout. A hang is unbounded, so a bound of a minute tells the two
    /// apart while a bound of ten seconds went red under a loaded whole-workspace
    /// run and green on its own.
    ///
    /// **And it runs twenty times**, which is not belt and braces: what it
    /// catches is a **race**, and a race caught once in two and a half runs is
    /// a test that reports *no defect* three times out of five. Twenty rounds
    /// of a coin that lands red two times in five come up green by luck once in
    /// twenty-five thousand runs. The defect it is pinning is the completion
    /// that landed *before* the round that would have waited for it: the park
    /// then has nothing to wait for and rings nobody's alarm, and a task whose
    /// future stores no waker is never polled again (`exec::block_on`).
    #[test]
    fn a_future_fed_from_a_worker_finishes_under_block_on() {
        for _ in 0..20 {
            a_future_fed_from_a_worker_finishes_under_block_on_once();
        }
    }

    fn a_future_fed_from_a_worker_finishes_under_block_on_once() {
        use std::io::Write;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let (reader, mut writer) = std::io::pipe().expect("a pipe");
        // **A task that is alive and waiting on another worker operation**, which
        // is what makes the executor **park** rather than spin round a ready
        // queue - `Yield` would have kept it ready and hidden the very defect
        // this is about.
        //
        // *A worker operation and not a `Slot`*, and the difference is the whole
        // reason this reads the way it does: a `Slot` filled from an ordinary
        // thread is outside the runtime's accounting, so `pending()` can be zero
        // while a task waits for one - and `exec::block_on` then panics about a
        // waker nobody arranged, correctly, because nothing it can see will move.
        // The first version of this test did that and failed under a loaded
        // whole-workspace run while passing alone.
        let (second, mut poking) = std::io::pipe().expect("a second pipe");
        let ran = Arc::new(AtomicBool::new(false));

        // Both arrive after the executor has had time to park, so what wakes it
        // is a worker's reply and not a poll that was going to be ready anyway.
        let writing = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            writer.write_all(b"x").expect("the peer writes");
            poking.write_all(b"y").expect("and the second");
        });

        let (told, heard) = std::sync::mpsc::channel();
        let finished = Arc::clone(&ran);
        std::thread::Builder::new()
            .name("adr-121-block-on".to_string())
            .spawn(move || {
                exec::start(async move {
                    let _ = io::waiting(
                        &second,
                        Interest::Readable,
                        Some(std::time::Duration::from_secs(2)),
                    )
                    .await;
                    finished.store(true, Ordering::SeqCst);
                });
                let ready = exec::block_on(async {
                    io::waiting(
                        &reader,
                        Interest::Readable,
                        Some(std::time::Duration::from_secs(2)),
                    )
                    .await
                });
                let _ = told.send(ready.map_err(|e| e.to_string()));
            })
            .expect("the driving thread starts");

        let ready = heard
            .recv_timeout(std::time::Duration::from_secs(60))
            .unwrap_or_else(|_| {
                panic!(
                    "`block_on` never came back: a future fed from a worker did not wake \
                     the executor, which is the hang ADR-121 D3 exists to keep out"
                )
            });
        assert!(
            ready.expect("the readiness wait was answered"),
            "a pipe with a byte in it is readable"
        );
        assert!(
            ran.load(Ordering::SeqCst),
            "the task nobody joined still ran to its end (ADR-055 D5)"
        );
        let _ = writing.join();
    }

    /// **An answer another thread reaps moves the generation** (0.0.259).
    ///
    /// The ring is one per process and every thread's poll and park takes all
    /// of its completions off the queue, so a read's answer is often reaped by a
    /// thread that is not waiting for it. The reader's park then has only the
    /// generation to learn it by - it waits for a completion that has already
    /// come otherwise, and where the read needs a second turn that only its own
    /// poll submits, it waits for ever with the ring's lock held: the arm64 CI
    /// job hung six hours in
    /// `fs::many_threads_reading_and_writing_at_once_each_get_their_own_answer`.
    ///
    /// So: one read submitted and left alone, another read done start to end
    /// on a second thread - which reaps both - and the count has to have moved.
    /// On the fallback path there is no ring and nothing to hold.
    #[test]
    fn an_answer_another_thread_reaps_moves_the_generation() {
        if handle().files() != Files::Completion {
            return;
        }
        let dir = std::env::temp_dir().join(format!("nikaia-reaped-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let (mine, theirs) = (dir.join("mine"), dir.join("theirs"));
        std::fs::write(&mine, "mine").expect("a file to read");
        std::fs::write(&theirs, "theirs").expect("another");

        let left_alone = io::reading(&mine);
        // Long enough for the kernel to have answered it, so the second
        // thread's reap is the one that takes it off the queue.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let since = io::generation();
        let read = std::thread::spawn(move || exec::block_on(io::reading(&theirs)))
            .join()
            .expect("the second thread reads");
        assert_eq!(read.expect("its read"), b"theirs");
        assert!(
            io::generation() > since,
            "a completion reaped on another thread left the generation where it \
             was, so a park waiting for it cannot learn it came"
        );
        assert_eq!(exec::block_on(left_alone).expect("the first read"), b"mine");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// **An expired `cleanup-deadline` is exit 70, said on the panic path**
    /// (ADR-112 D1 and D2).
    ///
    /// It has to be a *process*. The decision is about what the thing that
    /// started the program reads — systemd, cron, a script under `set -e` —
    /// and a status is only a status once the process is over; `exit` cannot
    /// be watched from inside the process that calls it. So this runs the test
    /// binary again, with a configuration file whose deadline is short and one
    /// operation that will never finish, and reads what came back.
    ///
    /// Both halves are read, because either alone would pass while the
    /// decision was half built: the **status** is D1, and the **message on
    /// standard error** is D2, which is the default panic hook's doing and
    /// therefore any hook's.
    #[test]
    fn an_expired_cleanup_deadline_is_exit_70_on_the_panic_path() {
        const NAME: &str = "an_expired_cleanup_deadline_is_exit_70_on_the_panic_path";
        const CHILD: &str = "NIKAIA_EXPIRED_DEADLINE_CHILD";

        if std::env::var_os(CHILD).is_some() {
            // The child. A pipe nobody writes to: the readiness wait is still
            // in flight when the drain begins, so the deadline is the thing
            // that ends it.
            let (reader, _writer) = std::io::pipe().expect("a pipe");
            std::thread::spawn(move || {
                let _ = io::wait(&reader, Interest::Readable, None);
            });
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while handle().pending() == 0 && std::time::Instant::now() < until {
                std::thread::yield_now();
            }
            assert_eq!(handle().pending(), 1, "the readiness wait is queued");
            // The real path, not a call to `expired`: `finish` is what a
            // generated `main` calls, and it is where the status is decided.
            start(UserCode::Sequential).finish();
            unreachable!("`finish` ended the program");
        }

        let dir = std::env::temp_dir().join(format!("nikaia-adr-112-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory for the child's config");
        let config = dir.join(config::FILE);
        std::fs::write(&config, "cleanup-deadline = \"50ms\"\n").expect("write the config");

        // The name is a filter and not a path, so that moving the test
        // between modules does not quietly leave the child running nothing;
        // `--nocapture`, because the panic hook writes where the harness would
        // otherwise be capturing, and the message is half of what is read.
        let ran = std::process::Command::new(std::env::current_exe().expect("this binary"))
            .args([NAME, "--nocapture"])
            .env(CHILD, "1")
            .env(config::PATH_VAR, &config)
            .output()
            .expect("the child ran");
        std::fs::remove_dir_all(&dir).ok();

        let said = String::from_utf8_lossy(&ran.stderr).into_owned();
        assert_eq!(
            ran.status.code(),
            Some(EXIT_CLEANUP_EXPIRED),
            "D1: an expired deadline is 70, not 0.\nstderr:\n{said}"
        );
        assert!(
            said.contains("did not finish within the"),
            "D2: the message goes to standard error.\nstderr:\n{said}"
        );
        // Not *empty* - the harness itself writes there - but without the
        // message, which is what D2 says: standard output belongs to the
        // program and may be the very file somebody is waiting for.
        assert!(
            !String::from_utf8_lossy(&ran.stdout).contains("did not finish within the"),
            "D2: the message is never on standard output"
        );
    }

    /// D4, from the only angle a test can see it: the runtime a `std` call
    /// finds is already there, and it is the same one every call finds.
    #[test]
    fn there_is_one_runtime_and_it_is_already_running() {
        let first = handle();
        let second = handle();
        assert!(std::ptr::eq(first, second), "two runtimes started");
        assert!(first.config().io_workers >= 1, "D4: one I/O thread always");
    }

    /// ADR-037 D2 as a thread count. Nothing the user wrote can run
    /// concurrently at `Sequential`, because there is nothing to run it on.
    #[test]
    fn a_sequential_build_starts_no_pool_for_user_code() {
        let runtime = Runtime::build(UserCode::Sequential, Config::default());
        assert!(runtime.user_pool().is_none());
        // **The workers' count and not `pending()`**, which since 0.0.165 also
        // carries the *process's* readiness registrations — and the harness
        // runs these tests side by side, so another test's socket may be armed
        // at this instant. The same sentence
        // `a_worker_operation_wakes_the_park_on_either_path` writes about its
        // own count, one question over.
        assert_eq!(runtime.workers.pending(), 0);

        // …and the I/O worker started anyway, because it is the compiler's
        // thread and was never bounded by that switch.
        let path = std::env::temp_dir().join(format!("nikaia-rt-seq-{}", std::process::id()));
        std::fs::write(&path, "eins").expect("write");
        assert!(runtime.describe().contains("user-code=sequential"));
        runtime.workers.drain(std::time::Duration::from_secs(5));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_concurrent_build_starts_one() {
        let runtime = Runtime::build(UserCode::Concurrent, Config::default());
        assert!(runtime.user_pool().is_some());
        assert!(runtime.describe().contains("user-code=concurrent"));
        runtime.workers.drain(std::time::Duration::from_secs(5));
    }

    /// The fallback is not a hypothetical: pinning `blocking` produces a
    /// runtime that never touches a ring, and it answers the same bytes.
    #[test]
    fn the_blocking_fallback_reads_what_the_completion_path_reads() {
        let path = std::env::temp_dir().join(format!("nikaia-rt-both-{}", std::process::id()));
        let text = "Hamburg;12.0\n".repeat(10_000);
        std::fs::write(&path, &text).expect("write");

        let blocking = Runtime::build(
            UserCode::Sequential,
            Config {
                io_method: Method::Blocking,
                ..Config::default()
            },
        );
        assert_eq!(blocking.files(), Files::Blocking);

        let auto = Runtime::build(UserCode::Sequential, Config::default());
        let one = read_through(&blocking, &path);
        let other = read_through(&auto, &path);
        assert_eq!(one, text.as_bytes(), "the fallback read the file");
        assert_eq!(other, one, "and the chosen mechanism read the same bytes");

        blocking.workers.drain(std::time::Duration::from_secs(5));
        auto.workers.drain(std::time::Duration::from_secs(5));
        std::fs::remove_file(&path).ok();
    }

    /// A pair through a named runtime, so the test can compare two mechanisms
    /// rather than only whichever one this machine chose.
    fn read_through(runtime: &Runtime, path: &Path) -> Vec<u8> {
        match runtime.files() {
            #[cfg(target_os = "linux")]
            Files::Completion => runtime
                .with_ring(|ring| ring.read_many(&[path]))
                .expect("a ring")
                .pop()
                .expect("one answer")
                .expect("read"),
            #[cfg(not(target_os = "linux"))]
            Files::Completion => unreachable!(),
            Files::Blocking => {
                let (reply, answer) = std::sync::mpsc::channel();
                assert!(runtime.workers.send(worker::Op::Read {
                    path: path.to_path_buf(),
                    reply
                }));
                answer.recv().expect("answered").expect("read")
            }
        }
    }

    /// Both mechanisms answer a pair in the order it was asked for, which is
    /// what makes the surface one surface.
    #[test]
    fn a_pair_is_answered_in_order_on_either_mechanism() {
        let dir = std::env::temp_dir().join(format!("nikaia-rt-pair-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let (a, b) = (dir.join("eins"), dir.join("zwei"));
        std::fs::write(&a, "eins").expect("write");
        std::fs::write(&b, "zwei").expect("write");

        for method in [Method::Auto, Method::Blocking] {
            let runtime = Runtime::build(
                UserCode::Sequential,
                Config {
                    io_method: method,
                    io_workers: 2,
                    ..Config::default()
                },
            );
            assert_eq!(read_through(&runtime, &a), b"eins", "{method:?}");
            assert_eq!(read_through(&runtime, &b), b"zwei", "{method:?}");
            runtime.workers.drain(std::time::Duration::from_secs(5));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The pair the **compiler** lowers onto answers the same bytes in the same
    /// order on either mechanism (ADR-033 D10).
    ///
    /// What it costs differs - the completion path overlaps and the fallback
    /// reads in written order - and what it *means* may not, because the
    /// program that chose it did not ask for an overlap and must not be able to
    /// tell that it got one.
    #[test]
    fn the_compilers_pair_answers_the_same_either_way() {
        let dir = std::env::temp_dir().join(format!("nikaia-rt-d10-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let (a, b) = (dir.join("eins"), dir.join("zwei"));
        std::fs::write(&a, "eins").expect("write");
        std::fs::write(&b, "zwei zwei").expect("write");

        for method in [Method::Auto, Method::Blocking] {
            let runtime = Runtime::build(
                UserCode::Sequential,
                Config {
                    io_method: method,
                    io_workers: 2,
                    ..Config::default()
                },
            );
            // Through the public surface, which is what a lowered program
            // reaches: the runtime it finds is the process's, and this asserts
            // the answer rather than which runtime answered.
            let (first, second) = io::read_both(&a, &b);
            assert_eq!(first.expect("the first read"), b"eins", "{method:?}");
            assert_eq!(second.expect("the second read"), b"zwei zwei", "{method:?}");
            // …and a missing file fails in its own half, not in the other's.
            let (missing, there) = io::read_both(&dir.join("nicht-da"), &a);
            assert!(missing.is_err(), "{method:?}");
            assert_eq!(
                there.expect("the file that is there"),
                b"eins",
                "{method:?}"
            );
            runtime.workers.drain(std::time::Duration::from_secs(5));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// D3's other half, through the surface rather than through the mechanism:
    /// a socket says when it is readable, and the wait happens on an I/O
    /// worker that was already running.
    ///
    /// `epoll` answers a socket exactly and answers a file uselessly, which is
    /// the whole reason the two halves have different mechanisms behind one
    /// `std` surface - so this is the test that the socket half is reachable
    /// from where a program would reach it.
    #[test]
    fn a_socket_reports_readiness_through_the_std_surface() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;

        let (here, there) = UnixStream::pair().expect("a socket pair");

        // Nothing sent: not readable, and a timeout is `Ok(false)` rather than
        // a failure - a deadline has to be able to tell those apart.
        assert!(
            !io::wait(
                &here,
                Interest::Readable,
                Some(std::time::Duration::from_millis(20))
            )
            .expect("polled")
        );

        // A byte from the peer, and the same call says so.
        (&there).write_all(b"x").expect("the peer writes");
        assert!(
            io::wait(
                &here,
                Interest::Readable,
                Some(std::time::Duration::from_secs(5))
            )
            .expect("polled")
        );

        // …and the transfer is the caller's, which is what "readiness" means:
        // the runtime said when, the program says what.
        let mut byte = [0u8; 1];
        (&here).read_exact(&mut byte).expect("read");
        assert_eq!(byte, [b'x']);

        // A socket with room is writable straight away, so both interests are
        // reachable rather than only the one a test happened to need.
        assert!(io::wait(&here, Interest::Writable, None).expect("polled"));
    }

    /// What the `--runtime` line has to be able to say.
    #[test]
    fn the_description_names_the_mechanism_that_was_chosen() {
        let runtime = Runtime::build(UserCode::Sequential, Config::default());
        let said = runtime.describe();
        assert!(said.contains("io-method=auto"), "{said}");
        assert!(
            said.contains("chose completion") || said.contains("chose blocking"),
            "the report has to name which one detection picked: {said}"
        );
        runtime.workers.drain(std::time::Duration::from_secs(5));
    }
}
