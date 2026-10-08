//! The executor: what drives a Nikaia program that can pause
//! ([ADR-055](../../../../docs/specification/adr/adr-055.md) D3).
//!
//! **Why this is ours and not a bound runtime.**
//! [ADR-303](../../../../docs/specification/adr/adr-303.md) D3 built the I/O —
//! completion for files, readiness for everything else — and D4 starts it before
//! the program's first statement. A `tokio` or a `smol` brings an executor *and*
//! an I/O layer, so binding one would put two event loops in one process, which
//! is the hazard D7 treats when a foreign crate causes it. What was missing is
//! the smaller half: a task queue and a waker. That is this file.
//!
//! **What a suspension point becomes.** Before this, a Nikaia function that
//! paused did it by blocking its thread, and Part II 11.2's *"at `no` the task is
//! interleaved on the same thread"* was unbuildable — two synchronous closures
//! cannot interleave, because neither yields. Here a pause is a `Poll::Pending`,
//! and the executor runs something else.
//!
//! **One thread, deliberately.** This is `user_parallelism = no`, which is the
//! default ([ADR-037](../../../../docs/specification/adr/adr-037.md) D2), and at
//! that setting *nothing the user wrote ever runs concurrently* — interleaved is
//! not concurrent. The `yes` executor is the next step and is not here.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// A task the executor owns: a future that has been started and whose result
/// nobody is holding.
///
/// `Output = ()` because a task's value leaves through its
/// [`TaskHandle`](crate::task::TaskHandle) rather than through the queue - the
/// queue only has to know how to poll it to the end.
type Task = Pin<Box<dyn Future<Output = ()>>>;

/// Whether a task is ready to be polled again.
///
/// **A flag and not a queue of wakers**, which is what keeps this sound with no
/// locking discipline to get wrong: waking a task sets a bit, and the executor
/// reads the bit. A wake that arrives while the task is already queued is
/// therefore free rather than a duplicate.
struct Alarm(AtomicBool);

impl Alarm {
    fn woken() -> Arc<Alarm> {
        Arc::new(Alarm(AtomicBool::new(true)))
    }

    fn ring(self: &Arc<Self>) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether it rang, and clear it.
    fn take(&self) -> bool {
        self.0.swap(false, Ordering::Acquire)
    }
}

/// A `Waker` over a [`Alarm`]: `std`'s own, through [`std::task::Wake`], so
/// the count on the `Arc` is `std`'s to keep.
fn waker_for(alarm: Arc<Alarm>) -> Waker {
    Waker::from(alarm)
}

impl std::task::Wake for Alarm {
    fn wake(self: Arc<Self>) {
        self.ring();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.ring();
    }
}

/// How long this thread waits on the bell before looking again, while the pool
/// is carrying tasks.
///
/// **A safety net rather than a schedule.** Everything that could make `main`
/// runnable rings the bell, so the usual wake has no delay; the bound is there
/// because `main` is not the thread driving the I/O in that case, and a wait
/// with no bound on a bell somebody else is responsible for ringing is one
/// mistake away from a hang.
const SLICE: std::time::Duration = std::time::Duration::from_millis(20);

/// One started task and the alarm that says it is ready.
struct Queued {
    task: Task,
    alarm: Arc<Alarm>,
}

// The tasks this thread has started but not finished.
//
// Thread-local because this executor is one thread's (ADR-055 D4 at `no`), and
// because that removes the question of what a `spawn` from inside a task means:
// it joins the same queue.
thread_local! {
    static STARTED: RefCell<VecDeque<Queued>> = const { RefCell::new(VecDeque::new()) };
}

// Whether what this thread is polling right now is the future `block_on` was
// handed - `main` - rather than one of the tasks queued beside it.
thread_local! {
    static POLLING_MAIN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// **`main` is running and nothing else on this thread is waiting**
/// ([ADR-263](../../../../docs/specification/adr/adr-263.md) D2, its half that
/// is about tasks).
///
/// `true` only inside `main`'s own poll, with no task of this thread's queue
/// started and unfinished. A task's poll answers `false`, and so does every
/// thread of the pool: neither is `main`. That is the conservative side by
/// construction - a task on the pool reads through the runtime as before,
/// which costs that read the hand-off and never costs anything its overlap.
pub(crate) fn main_alone() -> bool {
    POLLING_MAIN.with(std::cell::Cell::get)
        && STARTED.with(|started| started.try_borrow().is_ok_and(|queue| queue.is_empty()))
}

/// Sets [`POLLING_MAIN`] for one poll of `main`, and puts back what was there,
/// on the way out and on an unwind alike.
struct PollingMain(bool);

impl PollingMain {
    fn enter() -> PollingMain {
        PollingMain(POLLING_MAIN.with(|polling| polling.replace(true)))
    }
}

impl Drop for PollingMain {
    fn drop(&mut self) {
        POLLING_MAIN.with(|polling| polling.set(self.0));
    }
}

/// Start a task on **this** thread's queue. It runs whether or not anybody
/// joins it (D5).
///
/// `user_parallelism = no`'s half, and the one the emitter names there. A task
/// started here is polled by the `block_on` on this thread and by nothing else,
/// which is why it needs no `Send`: nothing it holds ever crosses.
pub fn start(future: impl Future<Output = ()> + 'static) {
    let queued = Queued {
        task: Box::pin(future),
        alarm: Alarm::woken(),
    };
    STARTED.with(|started| started.borrow_mut().push_back(queued));
}

/// Start a task on the **pool**, where another thread may pick it up
/// ([ADR-055](../../../../docs/specification/adr/adr-055.md) §6 step 1's `yes`
/// half).
///
/// The `Send` bound is §2 D6, and it is here rather than on [`start`] because
/// the two are different lowerings of one Nikaia line: the emitter writes this
/// one at `user_parallelism = yes` and that one at `no`, so a program at the
/// default is never asked for a property its setting does not need
/// ([ADR-312](../../../../docs/specification/adr/adr-312.md) D9 is the same
/// shape — at one user thread a `Shared` is a plain count, and it could not be
/// if every task had to be `Send`).
///
/// **A program built at `yes` that reaches here with no pool** is a program
/// whose runtime was started as `Sequential`, which a generated `main` cannot
/// do — the same switch writes both lines. A test harness can, so the task runs
/// on this thread rather than being dropped.
pub fn start_on_pool(future: impl Future<Output = ()> + Send + 'static) {
    match crate::rt::handle().user_pool() {
        Some(pool) => pool.start_task(future),
        None => start(future),
    }
}

/// **A call that cannot pause, driven where no `.await` can be written**
/// ([ADR-288](../../../../docs/specification/adr/adr-288.md) D31).
///
/// A function written `sync(f)` is lowered as the `async fn` its body is: `f`
/// may pause. A caller that is itself `sync`, and hands it a lambda that does
/// not pause, has been told by the checker that the call cannot pause - so the
/// future is ready the first time it is asked, and this asks once, with a waker
/// nothing will ever use. A future that is not ready is this compiler having
/// allowed a pause it said could not happen, and that is an abort with the
/// sentence that says so rather than a hang.
#[track_caller]
pub fn settled<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut context) {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!(
            "a call the compiler proved cannot pause paused (a `sync(f)` function handed \
             a lambda that does not pause) - this is a Nikaia compiler bug"
        ),
    }
}

/// Drive `future` to its value, running every started task in between.
///
/// **The one place a Nikaia program's `main` is driven**
/// ([ADR-303](../../../../docs/specification/adr/adr-303.md) D4 says where it is
/// called from). Nothing else blocks: a pause inside the program is a
/// `Poll::Pending` that comes back here.
///
/// A task that is not ready and a `main` that is not ready mean there is nothing
/// to do on this thread, and the work that will wake either of them is the
/// runtime's I/O - which is on a thread of its own and already running. So the
/// wait is a park, and the I/O worker's reply is what ends it.
pub fn block_on<T>(future: impl Future<Output = T>) -> T {
    let mut main = Box::pin(future);
    // **`main`'s value, held rather than returned** (ADR-055 D5).
    //
    // *"A task nobody joins still runs"* is what a program of one `spawn`
    // needs - Part I 8.2's own example keeps no handle - and returning here the
    // moment `main` is ready would have made that sentence false: the task
    // would be a future in a queue nobody polls again. So the value waits until
    // the queue is empty, and until then this loop is the tasks' turn.
    let mut outcome: Option<T> = None;
    // **The drain's deadline** ([ADR-297](../../../../docs/specification/adr/adr-297.md)
    // D5), started when `main`'s value arrives and not before: what it bounds is
    // the wait for the tasks nobody joined, and `main` itself may legitimately
    // run for as long as it likes.
    let deadline = crate::rt::handle().config().cleanup_deadline;
    let mut draining_since: Option<std::time::Instant> = None;
    let alarm = Alarm::woken();
    let waker = waker_for(alarm.clone());
    let mut context = Context::from_waker(&waker);
    // **The completion count everything here was last polled against.** An I/O
    // future stores no waker, so what says *poll everyone again* is this count
    // moving — which the park below says too, and could not say about a
    // completion that landed before the round it would have waited in.
    let mut polled_at = crate::rt::io::generation();

    loop {
        // **Read before polling, and that ordering is the correctness.** An I/O
        // completion that arrives between the poll below and the park at the
        // bottom has already moved this count, so the park returns at once
        // rather than waiting for a wake that has already happened
        // (`rt::io::generation`).
        let generation = crate::rt::io::generation();

        // **A completion that landed before this round is one everybody has to
        // hear about.** The park rings every alarm when it *waits* for one and
        // gets it; it cannot ring them for one that had already arrived, and a
        // task whose future stores no waker then has a clear alarm and is never
        // polled again — so `block_on`'s drain waits out the whole
        // `cleanup-deadline` and abandons a task whose answer was already in
        // its channel ([ADR-055](../../../../docs/specification/adr/adr-055.md)
        // D5 says it runs, and it did not).
        if generation != polled_at {
            polled_at = generation;
            alarm.ring();
            STARTED.with(|started| {
                for queued in started.borrow().iter() {
                    queued.alarm.ring();
                }
            });
        }

        if outcome.is_none() && alarm.take() {
            let polled = {
                let _main = PollingMain::enter();
                main.as_mut().poll(&mut context)
            };
            if let Poll::Ready(value) = polled {
                outcome = Some(value);
            }
        }

        // **Every task that says it is ready, once round.** Taking the queue
        // rather than iterating it is what lets a task start another one
        // without this loop seeing its own tail.
        let ready: Vec<Queued> = STARTED.with(|started| started.borrow_mut().drain(..).collect());
        let mut progressed = false;
        let mut again = VecDeque::new();
        for mut queued in ready {
            if !queued.alarm.take() {
                again.push_back(queued);
                continue;
            }
            progressed = true;
            let task_waker = waker_for(queued.alarm.clone());
            let mut task_context = Context::from_waker(&task_waker);
            if queued.task.as_mut().poll(&mut task_context).is_pending() {
                again.push_back(queued);
            }
        }
        STARTED.with(|started| {
            let mut started = started.borrow_mut();
            // The tasks a task started go after the ones that were already
            // waiting, so a `spawn` cannot starve what was queued before it.
            again.append(&mut started);
            *started = again;
        });

        // **`main` is done and so is every task it started.** The one place
        // this returns without a word.
        //
        // *Every* task: at `user_parallelism = yes` a `spawn` goes to the pool
        // rather than to this queue, and D5's *"a task nobody joins still
        // runs"* is the same promise there. So the drain below waits for both,
        // and the number it reports is both.
        let on_the_pool = crate::rt::handle()
            .user_pool()
            .map(|pool| pool.live())
            .unwrap_or(0);
        let waiting = STARTED.with(|started| started.borrow().len()) + on_the_pool;
        if outcome.is_some() {
            // **The cleanups nobody will settle go first**
            // ([ADR-297](../../../../docs/specification/adr/adr-297.md) D5):
            // a cancelled task's, and one that died after a task's last
            // settle point. They are started here as tasks of this thread,
            // so the drain below waits for them under the same deadline.
            if crate::cleanup::start_orphans() > 0 {
                continue;
            }
            if waiting == 0 {
                return outcome.take().expect("checked just above");
            }
            // **D5's drain, bounded.** `"0"` disables draining, which is that
            // decision's own word for it - so a program configured that way
            // leaves the moment `main` is done and its unjoined tasks do not
            // finish. Otherwise the clock starts here.
            if deadline.is_zero() {
                return outcome.take().expect("checked just above");
            }
            let since = *draining_since.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() >= deadline {
                // **A cleanup the deadline cut off is a failure of the
                // program**, and each is named
                // ([ADR-297](../../../../docs/specification/adr/adr-297.md)
                // D5, ADR-297 D7 and D8): exit 70, on the panic path.
                let cut = crate::cleanup::unfinished();
                if !cut.is_empty() {
                    crate::rt::cleanups_expired(&cut, deadline);
                }
                // D5: the remainder are abandoned, and the program says so
                // rather than exiting quietly. A task has no name to give, so
                // what is named is how many and what the bound was.
                eprintln!(
                    "nikaia: {waiting} background task(s) did not finish within the \
                     {}s cleanup deadline and were abandoned",
                    deadline.as_secs_f64()
                );
                return outcome.take().expect("checked just above");
            }
        }

        // Something moved, so `main` may be able to move with it: ring its
        // alarm and go round. A task that made progress is the only thing on
        // this thread that could have changed what `main` is waiting for.
        if progressed {
            alarm.ring();
            continue;
        }
        // Or `main` was woken while the tasks were being polled - by a task
        // filling the slot it is waiting on, which is exactly what a `.join()`
        // is. **Only while `main` is still running**: once its value is in,
        // nothing takes its alarm at the top of the round, and asking it here
        // would go round for ever without reaching the park below - where
        // the clock a sleeping task waits for is read (ADR-297 D5 found it: an
        // adopted cleanup that sleeps waited out the whole deadline).
        if outcome.is_none() && alarm.take() {
            alarm.ring();
            continue;
        }

        // **Nothing on this thread can move**, so what will move is the I/O -
        // the kernel's completion queue, or an I/O worker, which is a thread of
        // its own and already running
        // ([ADR-303](../../../../docs/specification/adr/adr-303.md) D4). This is
        // the **one place in the program that parks**, which is what makes a
        // `std` read a suspension point: the thread is given up here and
        // nowhere else, so every other task has already had its turn.
        //
        // `generation` was read at the top of this round, before anything was
        // polled - so a completion that arrived while the tasks were being
        // polled is already accounted for and this returns immediately.
        // While draining, the park takes what is left of the deadline with it:
        // a worker that never answers must not become a program that never
        // exits (D5).
        let left = draining_since.map(|since| deadline.saturating_sub(since.elapsed()));

        // **Something is waiting for the clock rather than for a completion**
        // ([ADR-150](../../../../docs/specification/adr/adr-150.md) D1): a
        // `sleep` leaves the time it wants in `rt::timer`, and the park below
        // has nothing to wait for on its own account — `park_for` would answer
        // *nothing is outstanding* and this loop would either spin or call a
        // future that behaved correctly a defect.
        //
        // So the wait is the timer's, bounded by the nearer of the two bounds
        // there are, and every alarm is rung afterwards for the reason the I/O
        // path rings them: which future the clock was for is not something the
        // executor knows.
        if let Some(when) = crate::rt::timer::taken() {
            let rest = when.saturating_duration_since(std::time::Instant::now());
            let bound = |rest: std::time::Duration| match left {
                Some(drain) => drain.min(rest),
                None => rest,
            };
            if !rest.is_zero() {
                // **In the I/O where there is I/O and this thread drives it**,
                // because a completion the kernel has already accepted is only
                // reaped by whoever is in the ring - and with tasks on the pool
                // that thread is the pilot rather than this one.
                let moved =
                    on_the_pool == 0 && crate::rt::io::park_for(generation, Some(bound(rest)));
                if !moved {
                    // Either there was nothing to park in, or the park gave up
                    // early. What is left of the span is waited out on the
                    // bell, which every wake rings.
                    let rest = when.saturating_duration_since(std::time::Instant::now());
                    if !rest.is_zero() {
                        crate::rt::io::wait_for_bell(generation, Some(bound(rest)));
                    }
                }
            }
            alarm.ring();
            STARTED.with(|started| {
                for queued in started.borrow().iter() {
                    queued.alarm.ring();
                }
            });
            continue;
        }
        // **At `yes`, with tasks on the pool, this thread waits on the bell and
        // not in the I/O**, because the pool's pilot has the I/O and exactly
        // one thread may. On the completion path that is not a preference: a
        // thread inside `io_uring_enter` is woken by the kernel alone, so a
        // task filling the slot `main` is joining would never reach it. Every
        // wake that matters rings the bell — a completion, a `Slot` filled, a
        // task finishing — so this is the same wait through one door.
        //
        // With **no** live pool task there is no pilot, and `main` is the only
        // thread that can drive the I/O, so it takes the park itself. Nothing
        // can add a task in between: at `yes` a `spawn` is written by `main`
        // or by a task, and here there is neither running.
        if on_the_pool > 0 {
            if crate::rt::io::wait_for_bell(generation, Some(left.unwrap_or(SLICE).min(SLICE))) {
                alarm.ring();
                STARTED.with(|started| {
                    for queued in started.borrow().iter() {
                        queued.alarm.ring();
                    }
                });
            }
            continue;
        }
        if crate::rt::io::park_for(generation, left) {
            // **The I/O moved, so everything gets another turn.** Which task
            // was waiting for *this* completion is not something the executor
            // knows: an I/O future stores no waker, because the executor is the
            // only thing on this thread that parks and it parks in the I/O
            // (`rt::io::Reading`). So the answer to "who should be polled now"
            // is everyone, which at one thread is a handful of futures and one
            // `main`.
            alarm.ring();
            STARTED.with(|started| {
                for queued in started.borrow().iter() {
                    queued.alarm.ring();
                }
            });
            continue;
        }

        // **There was no I/O to wait for either.** A future returned `Pending`
        // without arranging for its waker to be called, which is a defect in
        // `std` - and a hang is the worst way to report one.
        if waiting == 0 {
            // `main` alone, and nothing exists that could wake it. Poll once
            // more rather than park: the cheapest way to be wrong here is to
            // spin one extra round, and the most expensive is to hang.
            alarm.ring();
            continue;
        }
        panic!(
            "the runtime has {waiting} task(s) waiting, and neither the tasks nor the \
             I/O can move. This is a defect in nikaia-std: a future returned \
             `Pending` without arranging for its waker to be called (ADR-055 §6)."
        );
    }
}

/// The value a task hands back, and whether it is there yet.
///
/// **A slot with a waker in it**, which is what makes a `.join()` a suspension
/// point rather than a wait: the task fills it and wakes whoever asked.
pub struct Slot<T> {
    inner: Mutex<Inner<T>>,
}

struct Inner<T> {
    value: Option<T>,
    waiting: Option<Waker>,
    /// **What the task panicked with**, where it did
    /// ([ADR-326](../../../docs/specification/adr/adr-326.md) D4): there is no
    /// value coming, and a `join` says so.
    panicked: Option<String>,
}

impl<T> Slot<T> {
    pub fn empty() -> Arc<Slot<T>> {
        Arc::new(Slot {
            inner: Mutex::new(Inner {
                value: None,
                waiting: None,
                panicked: None,
            }),
        })
    }

    /// Put the value in and wake whoever is waiting for it.
    pub fn fill(&self, value: T) {
        let waiting = {
            let mut inner = self.inner.lock().expect("the slot's lock");
            inner.value = Some(value);
            inner.waiting.take()
        };
        // Outside the lock: a waker may poll, and polling may reach for this
        // slot again.
        if let Some(waker) = waiting {
            waker.wake();
        }
        // **And the bell, for the thread that is not on this one.** At
        // `user_parallelism = yes` a task fills its slot on a pool thread while
        // whoever joined it waits on the main one, and the waker above rings an
        // alarm nobody is reading — the wait is on the bell. Ringing it here is
        // what makes `.join()` a suspension point across a thread and not only
        // across a task.
        //
        // At `no` this is a counter and a `notify_all` nobody is waiting on,
        // which costs a lock the program was already going to take.
        crate::rt::ring_the_bell();
    }

    /// **The task panicked**, so no value is coming: whoever joins it is told
    /// (ADR-326 D4).
    pub fn fail(&self, said: String) {
        let waiting = {
            let mut inner = self.inner.lock().expect("the slot's lock");
            inner.panicked = Some(said);
            inner.waiting.take()
        };
        if let Some(waker) = waiting {
            waker.wake();
        }
        crate::rt::ring_the_bell();
    }

    fn take(&self, context: &mut Context<'_>) -> Poll<T> {
        let mut inner = self.inner.lock().expect("the slot's lock");
        // **A `join` of a task that panicked panics in the joiner**, naming
        // what the task said: the value it waits for does not exist. A `main`
        // that joins it ends the program, as a panic in `main` does.
        if let Some(said) = inner.panicked.take() {
            drop(inner);
            panic!("the task this joins stopped: {said}");
        }
        match inner.value.take() {
            Some(value) => Poll::Ready(value),
            None => {
                inner.waiting = Some(context.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// Awaiting a [`Slot`].
pub struct Waiting<T> {
    slot: Arc<Slot<T>>,
}

impl<T> Waiting<T> {
    pub fn on(slot: Arc<Slot<T>>) -> Waiting<T> {
        Waiting { slot }
    }
}

impl<T> Future for Waiting<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<T> {
        self.slot.take(context)
    }
}

/// Hand control back to the executor once, so something else may run.
///
/// What a suspension point is, with no I/O behind it. `std`'s pausing entries
/// will return futures of their own (ADR-055 §6 step 3); this is the smallest
/// one there is, and it is what a test can interleave on.
pub struct Yield(bool);

impl Yield {
    pub fn once() -> Yield {
        Yield(false)
    }
}

impl Future for Yield {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;
        // Wake immediately: this is not a wait, it is a place to be interrupted.
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    /// **`block_on` drives a future with nothing to wait for.**
    #[test]
    fn a_future_that_never_pauses_runs_straight_through() {
        assert_eq!(block_on(async { 7 }), 7);
    }

    /// **A suspension point is a place the executor may run something else** —
    /// which is Part II 11.2's *"interleaved on the same thread"*, and the whole
    /// reason this file exists (ADR-055 D4).
    ///
    /// Two tasks that yield between their two halves come out interleaved. With
    /// the synchronous lowering this was unreachable: neither closure had a
    /// point at which it gave the thread up.
    #[test]
    fn two_tasks_interleave_on_one_thread() {
        let order = Rc::new(RefCell::new(Vec::new()));

        let first = order.clone();
        let second = order.clone();
        let seen = order.clone();

        block_on(async move {
            start(async move {
                first.borrow_mut().push("a1");
                Yield::once().await;
                first.borrow_mut().push("a2");
            });
            start(async move {
                second.borrow_mut().push("b1");
                Yield::once().await;
                second.borrow_mut().push("b2");
            });
            // Let the tasks run: `main` yields until they are done.
            for _ in 0..4 {
                Yield::once().await;
            }
            assert_eq!(
                *seen.borrow(),
                vec!["a1", "b1", "a2", "b2"],
                "each task ran up to its suspension point before the other resumed"
            );
        });
    }

    /// **A task nobody joins still runs** (D5), which is what Part I 8.2's own
    /// example needs: it keeps no handle.
    #[test]
    fn a_task_nobody_joins_still_runs() {
        let ran = Rc::new(RefCell::new(false));
        let inside = ran.clone();
        let seen = ran.clone();

        block_on(async move {
            start(async move {
                *inside.borrow_mut() = true;
            });
            Yield::once().await;
            assert!(*seen.borrow(), "the task ran without being awaited");
        });
    }

    /// **A slot is how a value leaves a task**, and taking it is a suspension
    /// point rather than a wait.
    #[test]
    fn a_slot_wakes_whoever_is_waiting_for_it() {
        let slot = Slot::<i64>::empty();
        let filling = slot.clone();

        let answer = block_on(async move {
            start(async move {
                Yield::once().await;
                filling.fill(41);
            });
            Waiting::on(slot).await + 1
        });
        assert_eq!(answer, 42);
    }
}

#[cfg(test)]
mod io_tests {
    /// **Two file reads in flight at once, on one thread**
    /// ([ADR-055](../../../../docs/specification/adr/adr-055.md) §6 step 3).
    ///
    /// The claim step 3 adds to step 1's: a suspension point is no longer only
    /// `Yield`, it is a `std` read - so `task::overlap2` over two reads is
    /// Part II 11.2's sentence about something a program actually writes, and
    /// `overlap { … }` is how a program writes it
    /// ([ADR-292](../../../../docs/specification/adr/adr-292.md) D2).
    ///
    /// It reads two files and checks both answers. What makes it a test of the
    /// *interleaving* rather than of reading twice is that neither half is
    /// awaited before the other is started: both are polled by the same
    /// `poll_fn`, and the executor's park is what waits for either.
    #[test]
    fn two_reads_are_in_flight_at_once_on_one_thread() {
        let dir = std::env::temp_dir().join(format!("nikaia-exec-io-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let one = dir.join("eins.txt");
        let two = dir.join("zwei.txt");
        std::fs::write(&one, "eins").expect("write");
        std::fs::write(&two, "zweizwei").expect("write");

        let (a, b) = super::block_on(crate::task::overlap2(
            crate::fs::read_to_string(&one, &crate::fs::Root::Anywhere),
            crate::fs::read_to_string(&two, &crate::fs::Root::Anywhere),
        ));
        assert_eq!(a.expect("eins"), "eins");
        assert_eq!(b.expect("zwei"), "zweizwei");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A read that fails is a failure and not a hang.
    ///
    /// The park is the place a mistake here becomes a program that never
    /// finishes, so the path where there is nothing to wait for has its own
    /// test: the `open` fails before anything reaches the kernel, so the future
    /// is finished at its first poll and the executor never parks.
    #[test]
    fn a_read_of_a_file_that_is_not_there_fails_rather_than_hangs() {
        let missing = std::env::temp_dir().join(format!("nikaia-absent-{}", std::process::id()));
        let _ = std::fs::remove_file(&missing);
        let outcome = super::block_on(crate::fs::read_to_string(
            &missing,
            &crate::fs::Root::Anywhere,
        ));
        assert!(outcome.is_err(), "a missing file read as something");
    }

    /// **Many reads at once**, which is where a slot table gets its accounting
    /// wrong.
    ///
    /// A finished slot whose answer nobody had taken yet used to be released as
    /// free, and the next operation reused its buffer - so a read came back as
    /// another operation's bytes. `uring::Job::owned` is the fix and this is the
    /// shape that finds it: every file has different contents, and they are
    /// checked against the file they were asked for.
    #[test]
    fn eight_reads_at_once_each_get_their_own_bytes() {
        let dir = std::env::temp_dir().join(format!("nikaia-exec-many-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let paths: Vec<_> = (0..8)
            .map(|n| {
                let path = dir.join(format!("{n}.txt"));
                std::fs::write(&path, "x".repeat(n + 1)).expect("write");
                path
            })
            .collect();

        let read = super::block_on(async {
            let mut out = Vec::new();
            // Pinned so each one can be polled where it stands: what this is
            // about is eight slots held at once, so they are all started before
            // any answer is taken.
            let mut futures: Vec<_> = paths
                .iter()
                .map(|path| Box::pin(crate::fs::read_to_string(path, &crate::fs::Root::Anywhere)))
                .collect();
            for future in &mut futures {
                out.push(future.as_mut().await);
            }
            out
        });

        for (n, text) in read.into_iter().enumerate() {
            assert_eq!(
                text.expect("read"),
                "x".repeat(n + 1),
                "file {n} came back as somebody else's bytes"
            );
        }

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod drain_tests {
    use super::*;
    use std::rc::Rc;

    // **The bound itself is tested end to end**, in `crates/nikaia/tests/tasks.rs`:
    // it needs a `cleanup-deadline` short enough to wait out, the runtime is one
    // per process, and this suite shares a process with eighty other tests. What
    // is here is the half that must not break when the bound is added.

    /// **A task that does finish is waited for**, which is the case the bound
    /// must not break.
    #[test]
    fn a_task_that_finishes_is_still_waited_for() {
        let done = Rc::new(RefCell::new(false));
        let inside = done.clone();
        block_on(async move {
            start(async move {
                Yield::once().await;
                Yield::once().await;
                *inside.borrow_mut() = true;
            });
        });
        assert!(
            *done.borrow(),
            "the drain returned before the task finished"
        );
    }
}
