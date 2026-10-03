//! The `yes` executor: a task may run on a thread of its own
//! ([ADR-055](../../../../docs/specification/adr/adr-055.md) §6 step 1's second
//! half, [ADR-037](../../../../docs/specification/adr/adr-037.md) D2).
//!
//! **A second executor and not a use of the pool that was already there.**
//! `rayon`'s pool is a work-stealing pool for **closures**: `join` and
//! `par_iter` hand it a `FnOnce`, it runs it to the end, and there is no point
//! at which a closure gives its thread up. A task can pause, so what a pool
//! would have to hold for one is a future to be polled again later, which is
//! not what a closure pool is for. So this is a queue of futures over the same
//! worker count (`user-pool`), and `rayon` keeps the work it is good at:
//! `fs::map`'s UTF-8 check and the parallel piece driver.
//!
//! **What is different from [`super::exec`]'s one thread** is all one
//! difference — a task may be polled on a thread that did not start it — and
//! three things follow from it:
//!
//! * a task's future must be `Send` (§2 D6), which the compiler refuses before
//!   this can;
//! * a waker has to reach the queue from any thread, so a task is an
//!   `Arc<Cell>` a waker pushes back rather than a flag one loop reads;
//! * and **exactly one thread at a time may wait in the I/O**, because the ring
//!   is behind a lock and a thread inside `io_uring_enter` is woken by the
//!   kernel and by nothing else. That thread is the **pilot**; every other
//!   worker waits on this pool's own bell.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Waker};

/// A task the pool owns. `Send`, which is the whole of what `yes` asks of a
/// task that `no` does not.
type Task = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Where a task is, as one atomic.
///
/// The states exist to answer one question: **what happens to a wake that
/// arrives while the task is being polled?** Without [`RUNNING_WOKEN`] it would
/// be dropped — the waker would see `RUNNING` and decline to queue, correctly,
/// since the future is out of the cell and a second worker must not poll it —
/// and then nothing would put it back. So a wake during a poll is *recorded*,
/// and the worker that finishes the poll re-queues instead of going idle.
const IDLE: u8 = 0;
const QUEUED: u8 = 1;
const RUNNING: u8 = 2;
const RUNNING_WOKEN: u8 = 3;

/// One task: its future, where it is, and the pool to push it back onto.
///
/// The future lives in a `Mutex<Option<…>>` rather than beside the queue,
/// because a waker is a thing anybody may hold — an I/O reply, another task's
/// `Slot`, a thread this pool never heard of — and all it has to be able to do
/// is say *"this one is ready again"* without owning what it names.
struct Cell {
    id: u64,
    state: AtomicU8,
    future: Mutex<Option<Task>>,
    pool: std::sync::Weak<Pool>,
}

impl Cell {
    /// Say this task is ready, from any thread.
    fn wake(self: &Arc<Self>) {
        loop {
            match self.state.load(Ordering::Acquire) {
                IDLE => {
                    if self
                        .state
                        .compare_exchange(IDLE, QUEUED, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        if let Some(pool) = self.pool.upgrade() {
                            pool.push(self.clone());
                        }
                        return;
                    }
                }
                RUNNING => {
                    if self
                        .state
                        .compare_exchange(
                            RUNNING,
                            RUNNING_WOKEN,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return;
                    }
                }
                // Already queued, or already recorded as woken while running.
                // Either way somebody is going to poll it.
                _ => return,
            }
        }
    }
}

/// A `Waker` over a [`Cell`]: `std`'s own, through [`std::task::Wake`], so
/// the count on the `Arc` is `std`'s to keep.
fn waker_for(cell: Arc<Cell>) -> Waker {
    Waker::from(cell)
}

impl std::task::Wake for Cell {
    fn wake(self: Arc<Self>) {
        Cell::wake(&self);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        Cell::wake(self);
    }
}

/// How long a worker that is not piloting waits on the bell before looking
/// again.
///
/// **Not a poll interval, and nothing depends on it for correctness.**
/// Everything that makes a task ready pushes it and rings this bell, so a
/// waiting worker is woken with no delay. The bound is there because the
/// *pilot* is inside the kernel's wait, where this bell cannot reach it: if the
/// pilot is the thread that will notice the completion, the others should look
/// again rather than sleep on a bell nobody is going to ring.
const SLICE: std::time::Duration = std::time::Duration::from_millis(20);

/// The pool: every live task, the ready ones, and the threads that drain them.
pub struct Pool {
    /// **Every task started and not finished.** The pool owns them, the way
    /// `exec`'s queue owns its own: a task that returned `Pending` is held by
    /// its wakers alone otherwise, and an I/O future stores no waker (D3's
    /// design, `rt::io::Reading`) — so without this a task waiting on a read
    /// would be dropped rather than polled again.
    live: Mutex<BTreeMap<u64, Arc<Cell>>>,
    /// The ones that say they are ready now.
    ready: Mutex<VecDeque<Arc<Cell>>>,
    /// Rung on every push, when a task finishes, and when the pilot comes back.
    bell: Condvar,
    next: AtomicU64,
    /// Held by whichever worker is waiting in the I/O. Exactly one: a second
    /// thread would block on the ring's lock rather than take work.
    piloting: AtomicBool,
    closing: AtomicBool,
    /// How many workers actually started — counted rather than asked for, so
    /// `--runtime` reports what this process got.
    threads: std::sync::atomic::AtomicUsize,
}

impl Pool {
    /// Start `threads` workers. `0` is `user-pool`'s documented default — as
    /// many as the machine has — and not a count somebody typed.
    pub fn start(threads: usize) -> Arc<Pool> {
        let threads = match threads {
            0 => std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            n => n,
        };
        let pool = Arc::new(Pool {
            live: Mutex::new(BTreeMap::new()),
            ready: Mutex::new(VecDeque::new()),
            bell: Condvar::new(),
            next: AtomicU64::new(0),
            piloting: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            threads: std::sync::atomic::AtomicUsize::new(0),
        });
        for n in 0..threads {
            let mine = pool.clone();
            // **Detached, and no handle is kept.** A worker leaves when
            // `close` is called, and ADR-297 D6's drain is about the *tasks*
            // rather than about the threads — `block_on` has already waited for
            // the work by the time anything closes this.
            let started = std::thread::Builder::new()
                .name(format!("nikaia-task-{n}"))
                .spawn(move || mine.work());
            if started.is_err() {
                // A machine that will not give us a thread is a machine this
                // program runs on with fewer of them. Refusing to start would
                // turn a resource limit into a failure to run at all.
                break;
            }
            pool.threads.fetch_add(1, Ordering::AcqRel);
        }
        pool
    }

    /// Start a task. It runs whether or not anybody joins it (ADR-055 D5).
    pub fn start_task(self: &Arc<Pool>, future: impl Future<Output = ()> + Send + 'static) {
        let id = self.next.fetch_add(1, Ordering::AcqRel);
        let cell = Arc::new(Cell {
            id,
            state: AtomicU8::new(QUEUED),
            future: Mutex::new(Some(Box::pin(future))),
            pool: Arc::downgrade(self),
        });
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, cell.clone());
        self.push(cell);
    }

    /// How many worker threads this pool actually got, which is what
    /// `--runtime` reports: a machine that would not give us one is a machine
    /// this program runs on with fewer of them.
    pub fn threads(&self) -> usize {
        self.threads.load(Ordering::Acquire)
    }

    /// How many tasks are started and unfinished — what the drain at the end of
    /// `main` asks about (ADR-297 D6).
    pub fn live(&self) -> usize {
        self.live.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Stop the workers.
    ///
    /// Whether anything is left to run is decided **before** this, by
    /// `exec::block_on`'s drain; by the time the runtime closes, the answer has
    /// been acted on and said out loud.
    pub fn close(&self) {
        self.closing.store(true, Ordering::Release);
        self.bell.notify_all();
    }

    fn push(&self, cell: Arc<Cell>) {
        self.ready
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(cell);
        self.bell.notify_one();
    }

    /// One worker's whole life.
    fn work(self: Arc<Pool>) {
        loop {
            let next = self
                .ready
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front();
            if let Some(cell) = next {
                self.run(cell);
                continue;
            }
            if self.closing.load(Ordering::Acquire) {
                return;
            }
            self.idle();
        }
    }

    /// Poll one task once.
    fn run(&self, cell: Arc<Cell>) {
        cell.state.store(RUNNING, Ordering::Release);
        // **Taken out of the cell for the poll**, which is what makes it
        // impossible for two workers to poll one future: the cell is empty
        // while it runs, so a second pop of the same `Arc` finds nothing and
        // drops it. The state machine is what stops a wake being lost with it.
        let taken = cell.future.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(mut future) = taken else {
            return;
        };
        let waker = waker_for(cell.clone());
        let mut context = Context::from_waker(&waker);
        if future.as_mut().poll(&mut context).is_ready() {
            self.live
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&cell.id);
            // **A task finishing is a cross-thread wake.** Whoever joined it is
            // on another thread — at `yes` that is usually `main` — and the
            // runtime's bell is what `main` waits on there.
            self.bell.notify_all();
            crate::rt::ring_the_bell();
            return;
        }
        *cell.future.lock().unwrap_or_else(|e| e.into_inner()) = Some(future);
        // Back to idle, unless a wake arrived while it was running — in which
        // case this worker is the one that has to put it back, because the
        // waker saw `RUNNING` and declined to.
        if cell
            .state
            .compare_exchange(RUNNING, IDLE, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            cell.state.store(QUEUED, Ordering::Release);
            self.push(cell);
        }
    }

    /// Nothing was ready. Wait for something, in the one way that is allowed.
    fn idle(self: &Arc<Pool>) {
        let piloting = self
            .piloting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if !piloting {
            let ready = self.ready.lock().unwrap_or_else(|e| e.into_inner());
            if !ready.is_empty() || self.closing.load(Ordering::Acquire) {
                return;
            }
            let _ = self
                .bell
                .wait_timeout(ready, SLICE)
                .unwrap_or_else(|e| e.into_inner());
            return;
        }

        // **The generation is read before the queue is checked**, which is the
        // ordering `exec::block_on` keeps for the same reason: a completion
        // arriving between the two has already moved the count, so the park
        // returns at once rather than waiting for a wake that has happened.
        let generation = crate::rt::io::generation();
        let empty = self
            .ready
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty();
        if empty && !self.closing.load(Ordering::Acquire) {
            crate::rt::io::park_for(generation, Some(SLICE));
            // **Everybody gets another turn.** Which task was waiting for this
            // completion is not something the pool knows: an I/O future stores
            // no waker, because the executor is the only thing that parks. So
            // the answer to "who should be polled now" is everyone.
            self.rearm();
            // …and whoever is on the main thread, which at `yes` waits on the
            // runtime's bell rather than in the I/O.
            crate::rt::ring_the_bell();
        }
        self.piloting.store(false, Ordering::Release);
        self.bell.notify_all();
    }

    /// Put every idle task back on the queue.
    ///
    /// `wake` decides what that means per task: one already queued stays where
    /// it is, and one being polled is marked instead, so this never queues the
    /// same future twice.
    fn rearm(&self) {
        let live: Vec<Arc<Cell>> = self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for cell in live {
            cell.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Two tasks on two threads, and the program sees both answers.**
    ///
    /// The `no` executor's own test is that two tasks *interleave* on one
    /// thread; this one is the other half of ADR-037 D2 — they do not have to
    /// take turns, because there is more than one thread for them to be on.
    #[test]
    fn tasks_run_and_their_answers_come_back() {
        let pool = Pool::start(2);
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for _ in 0..8 {
            let mine = done.clone();
            pool.start_task(async move {
                mine.fetch_add(1, Ordering::AcqRel);
            });
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.live() > 0 && std::time::Instant::now() < until {
            std::thread::yield_now();
        }
        assert_eq!(done.load(Ordering::Acquire), 8, "every task ran");
        assert_eq!(pool.live(), 0, "and none is left behind");
        pool.close();
    }

    /// **Four tasks on four threads take about as long as one**, which is the
    /// whole of what `user_parallelism = yes` buys and the one thing the `no`
    /// executor cannot do however long it is given.
    #[test]
    fn tasks_run_at_the_same_time_and_not_in_turn() {
        fn spin(until: std::time::Instant) {
            while std::time::Instant::now() < until {
                std::hint::spin_loop();
            }
        }
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        if threads < 4 {
            return;
        }
        let pool = Pool::start(4);
        let slice = std::time::Duration::from_millis(300);
        let began = std::time::Instant::now();
        for _ in 0..4 {
            pool.start_task(async move { spin(std::time::Instant::now() + slice) });
        }
        let until = began + std::time::Duration::from_secs(10);
        while pool.live() > 0 && std::time::Instant::now() < until {
            std::thread::yield_now();
        }
        let took = began.elapsed();
        assert_eq!(pool.live(), 0, "every task finished");
        assert!(
            took < slice * 3,
            "four 300ms tasks on four threads took {took:?}: they ran in turn"
        );
        pool.close();
    }

    /// **A task that pauses is put back and polled again**, which is the whole
    /// difference between this and a pool of closures: a `Pending` is a place
    /// to be interrupted rather than a thread held.
    #[test]
    fn a_task_that_pauses_is_polled_again() {
        let pool = Pool::start(1);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mine = seen.clone();
        pool.start_task(async move {
            mine.lock().expect("the log").push("before");
            crate::rt::exec::Yield::once().await;
            mine.lock().expect("the log").push("after");
        });
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.live() > 0 && std::time::Instant::now() < until {
            std::thread::yield_now();
        }
        assert_eq!(
            *seen.lock().expect("the log"),
            vec!["before", "after"],
            "both halves ran, and the pause was not the end of it"
        );
        pool.close();
    }

    /// **A wake that arrives while the task is being polled is not lost.**
    ///
    /// This is what the four states are for. The future wakes *itself* from
    /// inside its own poll, so the waker sees `RUNNING` and declines to queue;
    /// what has to happen then is that the worker finishing the poll puts it
    /// back. If it did not, the task would sit idle for ever and this would
    /// hang rather than fail.
    #[test]
    fn a_wake_during_a_poll_is_not_lost() {
        let pool = Pool::start(1);
        let rounds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mine = rounds.clone();
        pool.start_task(async move {
            std::future::poll_fn(move |context| {
                let seen = mine.fetch_add(1, Ordering::AcqRel);
                if seen >= 2 {
                    return std::task::Poll::Ready(());
                }
                // Woken from inside the poll: the state is `RUNNING`, so the
                // waker records it rather than queueing.
                context.waker().wake_by_ref();
                std::task::Poll::Pending
            })
            .await
        });
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.live() > 0 && std::time::Instant::now() < until {
            std::thread::yield_now();
        }
        assert_eq!(pool.live(), 0, "the task finished rather than sitting idle");
        assert!(rounds.load(Ordering::Acquire) >= 3, "it was polled again");
        pool.close();
    }
}
