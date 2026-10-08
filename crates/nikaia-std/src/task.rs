//! The one place a Nikaia program runs two things at once.
//!
//! The emitter names what is in this module and nothing else, so which vehicle
//! carries an overlap is this file's decision rather than a shape baked into
//! every generated program (ADR-292).
//!
//! **Two things a program can ask for, and they are different questions:**
//!
//! * [`TaskHandle`] is a `spawn` - one piece of work the executor owns, joined
//!   later or not at all ([ADR-055](../../../docs/specification/adr/adr-055.md)
//!   D5).
//! * `overlap2`..`overlap8` are an `overlap { … }` block - every branch in
//!   flight in one pass, one function per arity
//!   ([ADR-292](../../../docs/specification/adr/adr-292.md) D2).
//! * `race2`..`race8` are a `select { … }` block - the same branches in flight,
//!   and the **first** to finish is the one that is kept
//!   ([ADR-292](../../../docs/specification/adr/adr-292.md) D12). The pair D4
//!   names, one module apart from nothing.
//!
//! Both take **futures**, because a branch **is** one: an `async` block that
//! borrows what is around it and is started once, where a closure would add a
//! call and take nothing away. *Not* because the language below has no `async`
//! closure - it has one, measured
//! ([ADR-277](../../../docs/specification/adr/adr-277.md) D13), and this line
//! used to say otherwise. What ran on the pool
//! instead — `both`, the closure pair ADR-292 D6 chose for a group the
//! *compiler* put together — went with the automatic grouping itself
//! (ADR-292 D1).

/// **What a `spawn` hands back** (Part I 8.2,
/// [ADR-055](../../../docs/specification/adr/adr-055.md) D5).
///
/// ```nika
/// let handle = spawn fn { process(path) }
/// let result = handle.join()
/// ```
///
/// `.join()` is where two tasks meet again, and it is an `.await`: the task
/// fills a slot and wakes whoever is waiting on it, so joining gives the thread
/// up rather than holding it. That is what makes Part II 11.2's *"uniform API"*
/// true - the same two lines mean the same thing at either setting of
/// `user_parallelism`, and the only difference is how many threads the executor
/// has.
///
/// **A task nobody joins still runs** (D5), which Part I 8.2's own example
/// needs: the executor owns the task, so dropping the handle drops the handle
/// and not the work.
pub struct TaskHandle<T> {
    slot: std::sync::Arc<crate::rt::exec::Slot<T>>,
    asked: std::sync::Arc<Cancelled>,
}

/// **The request a [`TaskHandle::cancel`] makes**
/// ([ADR-292](../../../docs/specification/adr/adr-292.md) D14).
///
/// A flag and the waker of whoever is running the task, which is what makes the
/// request *prompt* rather than eventual: without the waker a task parked on
/// I/O would sit in the queue until that I/O answered, and only then notice it
/// had been cancelled.
struct Cancelled {
    asked: std::sync::atomic::AtomicBool,
    waking: std::sync::Mutex<Option<std::task::Waker>>,
}

impl Cancelled {
    fn nobody_asked() -> std::sync::Arc<Cancelled> {
        std::sync::Arc::new(Cancelled {
            asked: std::sync::atomic::AtomicBool::new(false),
            waking: std::sync::Mutex::new(None),
        })
    }
}

/// A task's body, wrapped in what makes it cancellable.
///
/// **The body is held in an `Option` so that it can be dropped early**, and
/// dropping it is the whole of D2: a future dropped at its suspension point
/// tears its values down, a `cleanup` that pauses is adopted by the runtime and
/// bounded by the `cleanup-deadline`
/// ([ADR-297](../../../docs/specification/adr/adr-297.md) D5), and nobody waits
/// for any of it.
struct Cancellable<F, T, Q: crate::cleanup::Queue> {
    body: Option<std::pin::Pin<Box<F>>>,
    slot: std::sync::Arc<crate::rt::exec::Slot<T>>,
    asked: std::sync::Arc<Cancelled>,
    /// **The cleanups this task parked**
    /// ([ADR-297](../../../docs/specification/adr/adr-297.md) D1): swapped in
    /// while it is polled, so that what it parks and what it settles are its
    /// own, and handed to the runtime as orphans when it is over.
    queues: Q,
}

impl<F: std::future::Future<Output = T>, T, Q: crate::cleanup::Queue + Unpin> std::future::Future
    for Cancellable<F, T, Q>
{
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        let me = self.get_mut();
        me.queues.swap();
        let polled = me.step(context);
        me.queues.swap();
        // **What is still parked when the task is over is nobody's to settle**:
        // a cancelled task's values, and one that died after the last settle
        // point. The runtime finishes them before the program ends (ADR-297
        // D5).
        if polled.is_ready() {
            me.queues.orphan();
        }
        polled
    }
}

/// **A task dropped before it was over** - the runtime abandoning it at the
/// deadline, or a queue that goes with its thread - tears its values down in
/// its own queues too, so what they park is an orphan like a cancelled task's
/// and not a stranger's to settle (ADR-297 D5).
impl<F, T, Q: crate::cleanup::Queue> Drop for Cancellable<F, T, Q> {
    fn drop(&mut self) {
        if self.body.is_some() {
            self.queues.swap();
            self.body = None;
            self.queues.swap();
        }
        self.queues.orphan();
    }
}

impl<F: std::future::Future<Output = T>, T, Q: crate::cleanup::Queue> Cancellable<F, T, Q> {
    /// One poll of the body, with the task's own queues in place.
    fn step(&mut self, context: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if self.asked.asked.load(std::sync::atomic::Ordering::Acquire) {
            // **The teardown is the drop**, and it happens here rather than in
            // `cancel` because this is the task's own thread and its pause
            // point.
            self.body = None;
            return std::task::Poll::Ready(());
        }
        let Some(body) = self.body.as_mut() else {
            return std::task::Poll::Ready(());
        };
        // **A panic ends this task and nothing else**
        // ([ADR-326](../../../docs/specification/adr/adr-326.md) D4), at both
        // settings: caught at the task's edge, after the hook has said where it
        // happened. The body is dropped, so its values are torn down and its
        // cleanups run as a cancelled task's do, and whoever joins it is told.
        let polled = {
            let _inside = InATask::enter();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body.as_mut().poll(context)))
        };
        let polled = match polled {
            Ok(polled) => polled,
            Err(payload) => {
                self.body = None;
                self.slot.fail(Crashed {
                    message: panic_message(payload.as_ref()),
                    site: SITE.with(|held| std::mem::take(&mut *held.borrow_mut())),
                });
                return std::task::Poll::Ready(());
            }
        };
        match polled {
            std::task::Poll::Ready(value) => {
                self.body = None;
                self.slot.fill(value);
                std::task::Poll::Ready(())
            }
            std::task::Poll::Pending => {
                *self.asked.waking.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(context.waker().clone());
                std::task::Poll::Pending
            }
        }
    }
}

/// **What `join` throws for a task that panicked**
/// ([ADR-328](../../../docs/specification/adr/adr-328.md) D8): the crash
/// happened in another task, so the joiner's own state is whole and the crash
/// is an error it may catch, as any other is.
///
/// `message` is what the panic said; `site` is where, as the hook named it
/// (`src/main.nika:4`), or empty where no `.nika` line is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crashed {
    pub message: String,
    pub site: String,
}

impl std::fmt::Display for Crashed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.site.is_empty() {
            true => write!(f, "the task crashed: {}", self.message),
            false => write!(f, "the task crashed at {}: {}", self.site, self.message),
        }
    }
}

impl std::error::Error for Crashed {}

thread_local! {
    /// **Where the last panic on this thread happened**, as the hook named it:
    /// the catch at the task's edge has the payload and not the location.
    static SITE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// The hook's half of [`Crashed::site`]: where a task's panic happened.
pub(crate) fn panicked_at(site: String) {
    SITE.with(|held| *held.borrow_mut() = site);
}

/// What a panic said, in the two shapes a payload comes in.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "it panicked".to_string())
}

thread_local! {
    /// How many task bodies this thread is polling right now: the panic hook
    /// asks it, to say a task stopped rather than the program.
    static IN_A_TASK: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// **Whether the code running on this thread is a task's** rather than
/// `main`'s ([ADR-326](../../../docs/specification/adr/adr-326.md) D4): a panic
/// there ends the task, and the program goes on.
pub fn in_a_task() -> bool {
    IN_A_TASK.with(|depth| depth.get() > 0)
}

/// One poll of a task's body, counted for [`in_a_task`] while it lasts.
struct InATask;

impl InATask {
    fn enter() -> InATask {
        IN_A_TASK.with(|depth| depth.set(depth.get() + 1));
        InATask
    }
}

impl Drop for InATask {
    fn drop(&mut self) {
        IN_A_TASK.with(|depth| depth.set(depth.get() - 1));
    }
}

impl<T: 'static> TaskHandle<T> {
    /// Start `body` as a task, and hand back the handle to its value.
    ///
    /// **The captures have already moved**, because the emitter writes the body
    /// as an `async move` block - which is Part I 8.3's implicit move and Rust's
    /// `move` meeting at the same place, with `NK2101` in front of it for the
    /// data the parent still wanted.
    pub fn start(body: impl std::future::Future<Output = T> + 'static) -> TaskHandle<T> {
        let slot = crate::rt::exec::Slot::empty();
        let asked = Cancelled::nobody_asked();
        crate::rt::exec::start(Cancellable {
            body: Some(Box::pin(body)),
            slot: slot.clone(),
            asked: asked.clone(),
            queues: crate::cleanup::Queues::default(),
        });
        TaskHandle { slot, asked }
    }
}

impl<T> TaskHandle<T> {
    /// Whether the task has ended, without taking the handle: what a
    /// supervisor asks of every child at once (ADR-328).
    pub(crate) fn poll_ended(
        &self,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<T, Crashed>> {
        self.slot.take(context)
    }

    /// **Stop the task** ([ADR-292](../../../docs/specification/adr/adr-292.md)
    /// D3), with exactly the semantics losing a `select` has.
    ///
    /// The task stops at its current pause point, its values are torn down, and
    /// a `cleanup` that pauses is adopted by the runtime and finished in the
    /// background. The caller does not wait for any of that, because it should
    /// not pay for it (D2).
    ///
    /// **It takes the handle**, the way `join` does, and that is what makes
    /// §4's open question — *can a cancelled task be observed to have been
    /// cancelled?* — one no program can ask: after this there is no handle to
    /// ask with. It also means a cancelled task is never joined, so nothing
    /// waits for a value that is not coming.
    pub fn cancel(self) {
        self.asked
            .asked
            .store(true, std::sync::atomic::Ordering::Release);
        // **Wake it so that it notices now.** A task parked on I/O has a clear
        // alarm, and the executor polls what its alarm says is ready.
        let waking = self
            .asked
            .waking
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(waker) = waking {
            waker.wake();
        }
        // And the thread that is not this one, at `user_parallelism = yes`.
        crate::rt::ring_the_bell();
    }
}

impl<T: Send + 'static> TaskHandle<T> {
    /// The same, on the **pool**, where another thread may pick the task up
    /// ([ADR-055](../../../docs/specification/adr/adr-055.md) §6 step 1's `yes`
    /// half, [ADR-037](../../../docs/specification/adr/adr-037.md) D2).
    ///
    /// **Two functions and not one with a bound**, because the bound is the
    /// difference and it belongs to the build rather than to the language. The
    /// emitter writes this line at `user_parallelism = yes` and [`start`] at
    /// `no`, from one Nikaia `spawn` — so a program at the default is never
    /// asked for a `Send` its setting does not need, which is what keeps
    /// [ADR-312](../../../docs/specification/adr/adr-312.md) D9's plain count
    /// reachable from inside a task.
    ///
    /// [`start`]: TaskHandle::start
    pub fn start_on_pool(
        body: impl std::future::Future<Output = T> + Send + 'static,
    ) -> TaskHandle<T> {
        let slot = crate::rt::exec::Slot::empty();
        let asked = Cancelled::nobody_asked();
        crate::rt::exec::start_on_pool(Cancellable {
            body: Some(Box::pin(body)),
            slot: slot.clone(),
            asked: asked.clone(),
            queues: crate::cleanup::SendQueues::default(),
        });
        TaskHandle { slot, asked }
    }

    /// The task's value, once it has one. **A suspension point**, not a wait.
    /// A task that panicked has none, and this throws [`Crashed`] (ADR-328 D8).
    pub async fn join(self) -> Result<T, Crashed> {
        crate::rt::exec::Waiting::on(self.slot).await
    }
}

thread_local! {
    /// How many `overlap` and `select` blocks this thread is polling a branch
    /// of right now.
    static OPEN_BLOCKS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// **A branch of an `overlap` or a `select` is being polled on this thread**
/// ([ADR-263](../../../docs/specification/adr/adr-263.md) D2).
///
/// A block's branches are no tasks: they are polled one after another on the
/// block's own thread (ADR-292 D4). Its first branch starts its read before the
/// second branch has started anything, so the runtime has nothing in flight at
/// that moment - and a read made on the calling thread there would run to its
/// end before the second branch began, which is the sum ADR-292 D6 refuses.
/// So a file operation asks this before it takes the calling thread.
///
/// **Counted per poll, not per block**, because a block in a task on the pool
/// may be polled by one worker and then another: a count held across polls
/// would be raised on one thread and lowered on a different one.
pub(crate) fn inside_a_block() -> bool {
    OPEN_BLOCKS.with(|open| open.get() > 0)
}

/// One poll of a block's branches, counted for [`inside_a_block`] while it
/// lasts.
struct OpenBlock;

impl OpenBlock {
    fn enter() -> OpenBlock {
        OPEN_BLOCKS.with(|open| open.set(open.get() + 1));
        OpenBlock
    }
}

impl Drop for OpenBlock {
    fn drop(&mut self) {
        OPEN_BLOCKS.with(|open| open.set(open.get() - 1));
    }
}

/// **Every branch of an `overlap { … }`, all of them in flight**
/// (Part I 8.1.2, [ADR-292](../../../docs/specification/adr/adr-292.md) D2).
///
/// One arity per macro expansion, because the branches have different types and
/// a tuple of futures is what that means in the language below. The emitter
/// writes `task::overlap3(a, b, c).await` and the arity is how many statements
/// the block had.
///
/// **Flat and not nested, which is D6.** A pair joined with a pair -
/// `two(a, two(b, c))` - polls `a` to completion before `b` is ever started, so
/// a block holding a computation and two reads would cost their sum: the naive
/// order D6 names and refuses. Polling every branch in one pass costs `max` instead: a branch that
/// suspends returns `Pending` at its first suspension point and the next branch
/// is started at once.
///
/// **The order it polls in is the order it is given**, and the emitter hands the
/// branches over with the ones that can pause first — which is D6's rule, read
/// off the ledger's `sync` column. The results go back into written order at the
/// call, so this never has to know about it.
///
/// **A failure is the caller's**, not this function's: a branch that can fail
/// hands back a `Result` like any other value, and D5's "the first in written
/// order wins" is the emitter's `?` on the tuple rather than a rule here.
macro_rules! overlapping {
    ($name:ident, $($branch:ident : $result:ident),+) => {
        // One parameter per branch is what an arity *is*, so the argument count
        // is the point rather than a smell: `overlap8` takes eight branches
        // because a block of eight has eight.
        #[allow(non_snake_case, clippy::too_many_arguments)]
        pub async fn $name<$($branch, $result),+>($($branch: $branch),+) -> ($($result),+)
        where
            $($branch: std::future::Future<Output = $result>),+
        {
            $(let mut $branch = Box::pin($branch);)+
            $(let mut $result: Option<$result> = None;)+

            std::future::poll_fn(move |context| {
                let _open = OpenBlock::enter();
                $(
                    if $result.is_none() {
                        if let std::task::Poll::Ready(value) = $branch.as_mut().poll(context) {
                            $result = Some(value);
                        }
                    }
                )+
                if $($result.is_some())&&+ {
                    return std::task::Poll::Ready((
                        $($result.take().expect("checked just above")),+
                    ));
                }
                // No waker is rung, for the reason `interleave` gives: what a
                // branch waits for is the I/O, and the executor is the only
                // thing on this thread that parks.
                std::task::Poll::Pending
            })
            .await
        }
    };
}

/// **A block's one outcome, out of its branches'**
/// ([ADR-292](../../../docs/specification/adr/adr-292.md) D5,
/// [ADR-292](../../../docs/specification/adr/adr-292.md) D18).
///
/// The arguments are the branches' results **in written order**, which is the
/// only order the source has and therefore the only one that makes the outcome
/// reproducible. `?` left to right is that rule, written as the language
/// below's own control flow rather than as a comparison this makes.
///
/// **Why it is a function and not a `?` per branch at the call.** An
/// `overlap { … } catch { … }` hands the block's outcome to a handler, and a
/// handler needs the `Result` rather than the value - a `?` in the middle of
/// the block would leave the function instead. One place that turns *n*
/// outcomes into one is also the place [ADR-292](../../../docs/specification/adr/adr-292.md)
/// puts the `secondary` list the day the later failures stop being dropped:
/// this function is the only code that sees them all.
macro_rules! combining {
    ($name:ident, $($branch:ident : $value:ident),+) => {
        #[allow(non_snake_case, clippy::too_many_arguments)]
        pub fn $name<$($value),+, E: crate::error::Joined>(
            $($branch: Result<$value, E>),+
        ) -> Result<($($value),+), E> {
            // **Every failure, and the first in written order is the block's**
            // ([ADR-292](../../../docs/specification/adr/adr-292.md) D5,
            // [ADR-292](../../../docs/specification/adr/adr-292.md) D9). The
            // block waited for every branch, so when this runs every outcome is
            // known and the list is a fact rather than a race - which is the
            // reason D2 gives for not cancelling the rest at the first failure.
            let mut failed: Vec<E> = Vec::new();
            $(
                let $branch = match $branch {
                    Ok(value) => Some(value),
                    Err(e) => {
                        failed.push(e);
                        None
                    }
                };
            )+
            let mut failed = failed.into_iter();
            if let Some(mut first) = failed.next() {
                for later in failed {
                    first.joined_by(later);
                }
                return Err(first);
            }
            Ok(($($branch.expect("no branch failed")),+))
        }
    };
}

combining!(combine2, A: RA, B: RB);
combining!(combine3, A: RA, B: RB, C: RC);
combining!(combine4, A: RA, B: RB, C: RC, D: RD);
combining!(combine5, A: RA, B: RB, C: RC, D: RD, E2: RE);
combining!(combine6, A: RA, B: RB, C: RC, D: RD, E2: RE, F: RF);
combining!(combine7, A: RA, B: RB, C: RC, D: RD, E2: RE, F: RF, G: RG);
combining!(combine8, A: RA, B: RB, C: RC, D: RD, E2: RE, F: RF, G: RG, H: RH);

overlapping!(overlap2, A: RA, B: RB);
overlapping!(overlap3, A: RA, B: RB, C: RC);
overlapping!(overlap4, A: RA, B: RB, C: RC, D: RD);
overlapping!(overlap5, A: RA, B: RB, C: RC, D: RD, E: RE);
overlapping!(overlap6, A: RA, B: RB, C: RC, D: RD, E: RE, F: RF);
overlapping!(overlap7, A: RA, B: RB, C: RC, D: RD, E: RE, F: RF, G: RG);
overlapping!(overlap8, A: RA, B: RB, C: RC, D: RD, E: RE, F: RF, G: RG, H: RH);

/// **Which arm of a `select { … }` won** (Part II 12.4,
/// [ADR-292](../../../docs/specification/adr/adr-292.md) D12).
///
/// One enum per arity, for the reason `overlap` has one function per arity: the
/// arms have different types, and in the language below that is what a sum of
/// them means. The variants are **ordinals** and not letters, so the generated
/// `match` reads as the source does — `Race2::Second(_)` is the second arm of
/// the block (Part III C.1).
///
/// **The losers are dropped**, and that is D2 rather than an implementation
/// detail: a future dropped at a suspension point tears its values down, a
/// `cleanup` that pauses is adopted by the runtime and bounded by the
/// `cleanup-deadline` ([ADR-297](../../../docs/specification/adr/adr-297.md)
/// D3), and the winner does not wait for any of it. The language below drops
/// the losing futures when `race<n>` returns, so the mechanism is the one this
/// runtime already had.
macro_rules! racing {
    ($name:ident, $won:ident, $($branch:ident : $result:ident : $variant:ident),+) => {
        #[derive(Debug)]
        pub enum $won<$($result),+> {
            $($variant($result)),+
        }

        // One parameter per arm is what an arity *is*, the same way an
        // `overlap`'s is.
        #[allow(non_snake_case, clippy::too_many_arguments)]
        pub async fn $name<$($branch, $result),+>($($branch: $branch),+) -> $won<$($result),+>
        where
            $($branch: std::future::Future<Output = $result>),+
        {
            $(let mut $branch = Box::pin($branch);)+

            std::future::poll_fn(move |context| {
                let _open = OpenBlock::enter();
                // **In written order, and the first that is ready wins.** Two
                // arms ready in the same pass is a tie, and the written order
                // is what breaks it - which is the same rule
                // [ADR-292](../../../docs/specification/adr/adr-292.md) D5 uses
                // for an `overlap`'s failures, said about a value instead.
                $(
                    if let std::task::Poll::Ready(value) = $branch.as_mut().poll(context) {
                        return std::task::Poll::Ready($won::$variant(value));
                    }
                )+
                // No waker is rung, for the reason `overlap` gives: what a
                // branch waits for is the I/O or the clock, and the executor is
                // the only thing on this thread that parks.
                std::task::Poll::Pending
            })
            .await
        }
    };
}

racing!(race2, Race2, A: RA: First, B: RB: Second);
racing!(race3, Race3, A: RA: First, B: RB: Second, C: RC: Third);
racing!(race4, Race4, A: RA: First, B: RB: Second, C: RC: Third, D: RD: Fourth);
racing!(
    race5, Race5, A: RA: First, B: RB: Second, C: RC: Third, D: RD: Fourth, E: RE: Fifth
);
racing!(
    race6, Race6, A: RA: First, B: RB: Second, C: RC: Third, D: RD: Fourth, E: RE: Fifth,
    F: RF: Sixth
);
racing!(
    race7, Race7, A: RA: First, B: RB: Second, C: RC: Third, D: RD: Fourth, E: RE: Fifth,
    F: RF: Sixth, G: RG: Seventh
);
racing!(
    race8, Race8, A: RA: First, B: RB: Second, C: RC: Third, D: RD: Fourth, E: RE: Fifth,
    F: RF: Sixth, G: RG: Seventh, H: RH: Eighth
);

/// One half of a [`crate::fs::read_both`], finished as `fs::read_to_string`
/// would have finished it.
///
/// `fs::read_to_string` is a read and then a UTF-8 check, and the pair performs
/// the read - so this is the check, on exactly the bytes that came back,
/// reporting exactly the failure the sequential program would have reported. It
/// is `std`'s own function and not a line written twice into every program that
/// reads two files at once.
///
/// **`what` is the path**, because since
/// [ADR-280](../../../docs/specification/adr/adr-280.md) D5 the failure names
/// what it was about, and the pair's own read does not carry it. Without it
/// the claim above stops being true: `read_to_string` would say which file and
/// this would not.
pub fn as_text(
    bytes: Result<Vec<u8>, std::io::Error>,
    what: &str,
) -> Result<String, crate::io::IoError> {
    match bytes {
        Ok(bytes) => crate::fs::text(bytes, what),
        Err(error) => Err(crate::io::IoError::of(error, what)),
    }
}

/// **A scope's tasks** (Part II 12.7, ADR-328 D8): they borrow what the
/// function around the scope holds, and the scope waits for every one of them.
///
/// At `user_parallelism = no` everything runs on one thread, so a task that
/// never pauses runs where it is started, to its end. A task that panicked
/// cancels the ones not yet started, and the scope throws [`Crashed`] where it
/// ends. `'env` is what the tasks borrow, which outlives the scope, as
/// `std::thread::scope` has it.
pub struct Scope<'scope, 'env: 'scope> {
    crashed: std::cell::RefCell<Option<Crashed>>,
    scope: std::marker::PhantomData<&'scope mut &'scope ()>,
    env: std::marker::PhantomData<&'env mut &'env ()>,
}

impl<'scope> Scope<'scope, '_> {
    /// Start a task that never pauses: it runs now, unless a task before it
    /// crashed, which cancelled it.
    pub fn spawn(&self, body: impl FnOnce() + 'scope) {
        if self.crashed.borrow().is_some() {
            return;
        }
        if let Err(crash) = caught(body) {
            *self.crashed.borrow_mut() = Some(crash);
        }
    }
}

/// **Run `body` with a scope, and wait for its tasks** (Part II 12.7): their
/// first crash is what this throws, after `body` and every task have ended.
pub fn scope<'env, T>(
    body: impl for<'scope> FnOnce(&'scope Scope<'scope, 'env>) -> T,
) -> Result<T, Crashed> {
    let scope = Scope {
        crashed: std::cell::RefCell::new(None),
        scope: std::marker::PhantomData,
        env: std::marker::PhantomData,
    };
    let value = body(&scope);
    let crash = scope.crashed.borrow_mut().take();
    match crash {
        Some(crash) => Err(crash),
        None => Ok(value),
    }
}

/// **The same scope where tasks run on every core** (`user_parallelism = yes`):
/// each task is the pool's, and the scope waits for all of them. A task here
/// never pauses (Part II 12.7, `NK2102`), so one that started runs to its end;
/// a crash cancels only the tasks not yet started.
pub struct PoolScope<'pool, 'scope> {
    pool: &'pool rayon::Scope<'scope>,
    crashed: std::sync::Arc<std::sync::Mutex<Option<Crashed>>>,
}

impl<'scope> PoolScope<'_, 'scope> {
    /// Start a task on the pool.
    pub fn spawn(&self, body: impl FnOnce() + Send + 'scope) {
        let crashed = self.crashed.clone();
        self.pool.spawn(move |_| {
            if crashed.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
                return;
            }
            if let Err(crash) = caught(body) {
                crashed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_insert(crash);
            }
        });
    }
}

/// [`scope`] at `user_parallelism = yes`.
pub fn scope_on_pool<'scope, T: Send>(
    body: impl FnOnce(&PoolScope<'_, 'scope>) -> T + Send,
) -> Result<T, Crashed> {
    let crashed = std::sync::Arc::new(std::sync::Mutex::new(None));
    let value = rayon::scope(|pool| {
        body(&PoolScope {
            pool,
            crashed: crashed.clone(),
        })
    });
    let crash = crashed.lock().unwrap_or_else(|e| e.into_inner()).take();
    match crash {
        Some(crash) => Err(crash),
        None => Ok(value),
    }
}

/// One task of a scope, its panic caught at its edge as a task's is (ADR-326 D4).
fn caught(body: impl FnOnce()) -> Result<(), Crashed> {
    let _inside = InATask::enter();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).map_err(|payload| Crashed {
        message: panic_message(payload.as_ref()),
        site: SITE.with(|held| std::mem::take(&mut *held.borrow_mut())),
    })
}

#[cfg(test)]
mod scoped {
    use super::*;

    #[test]
    fn a_scope_lends_to_its_tasks_and_waits_for_them() {
        let data = [1, 2, 3];
        let sum = std::cell::Cell::new(0);
        let made = scope(|s| {
            s.spawn(|| sum.set(sum.get() + data.iter().sum::<i32>()));
            s.spawn(|| sum.set(sum.get() + data.len() as i32));
            7
        });
        assert_eq!((made, sum.get(), data.len()), (Ok(7), 9, 3));
    }

    #[test]
    fn a_crash_cancels_the_tasks_after_it_and_is_thrown_at_the_end() {
        let ran = std::cell::Cell::new(0);
        let made = scope(|s| {
            s.spawn(|| ran.set(ran.get() + 1));
            s.spawn(|| panic!("boom"));
            s.spawn(|| ran.set(ran.get() + 10));
        });
        assert_eq!(made.map_err(|crash| crash.message), Err("boom".to_string()));
        assert_eq!(ran.get(), 1);
    }

    #[test]
    fn a_pool_scope_runs_its_tasks_on_the_pool_and_waits() {
        let data: Vec<i64> = (1..=100).collect();
        let total = std::sync::atomic::AtomicI64::new(0);
        let made = scope_on_pool(|s| {
            for half in data.chunks(50) {
                let total = &total;
                s.spawn(move || {
                    total.fetch_add(half.iter().sum(), std::sync::atomic::Ordering::SeqCst);
                });
            }
        });
        assert!(made.is_ok());
        assert_eq!(total.into_inner(), 5050);
        let crashed = scope_on_pool(|s| s.spawn(|| panic!("on the pool")));
        assert_eq!(
            crashed.map_err(|crash| crash.message),
            Err("on the pool".to_string())
        );
    }
}
