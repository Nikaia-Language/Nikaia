//! **Cleanup that can pause** — `impl Cleanup`
//! ([ADR-239](../../../docs/specification/adr/adr-239.md), refining
//! [ADR-006](../../../docs/specification/adr/adr-006.md)).
//!
//! A value whose teardown needs I/O — a buffered file flushing — cannot run it
//! in the language below's `Drop`, which cannot pause. So the work is split
//! where it has to be:
//!
//! * **When** a value dies is the language below's answer, exactly as for any
//!   other value: at the end of the scope that owns it last, wherever that is,
//!   and not at all where it was moved on. Nothing here, and nothing in the
//!   compiler, tracks moves.
//! * Dying **parks** the value's `cleanup` in the running task's queue
//!   ([`Cleaned`]'s `Drop`). Nothing runs yet.
//! * A **settle point** — which the compiler writes after a block where a value
//!   of such a type dies, and around the body of a function where one may —
//!   runs the parked work in the order the values died, and hands back what
//!   failed ([`settle`], [`settle_after`]).
//!
//! **A panic parks nothing**: the value is dropped as it is, which runs its
//! synchronous `drop` and nothing that waits (D5). A value whose task was
//! **cancelled**, or that died after a task's last settle point, stays parked
//! with nobody to settle it; the task hands it on as an orphan, and the runtime
//! finishes orphans before the program ends, under `cleanup-deadline`, and
//! names each one the deadline cut off ([`start_orphans`], [`unfinished`]).
//!
//! **Two queues**, because at `user_parallelism = yes` a task may move between
//! threads and what it parks has to be able to go with it: [`CleanedSend`]
//! parks work that is `Send`, [`Cleaned`] work that need not be. The compiler
//! writes the one its setting needs, and a settle point drains what that
//! setting has ([`Kind`]).

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;

/// Work that may stay on its thread.
type Work = Pin<Box<dyn Future<Output = Result<(), String>>>>;
/// Work that may move with its task.
type SendWork = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// A value's cleanup, parked: what it is, and the work.
pub struct Parked<W> {
    what: String,
    work: W,
}

impl<W> Parked<W> {
    /// What the parked value is, as a message names it.
    pub fn what(&self) -> &str {
        &self.what
    }
}

thread_local! {
    static LOCAL: RefCell<Vec<Parked<Work>>> = const { RefCell::new(Vec::new()) };
    static SENT: RefCell<Vec<Parked<SendWork>>> = const { RefCell::new(Vec::new()) };
    static LOCAL_ORPHANS: RefCell<Vec<Parked<Work>>> = const { RefCell::new(Vec::new()) };
}

static SENT_ORPHANS: std::sync::Mutex<Vec<Parked<SendWork>>> = std::sync::Mutex::new(Vec::new());

/// **A cleanup that can pause** (Part I 6.4): what `impl Cleanup for T` lowers
/// to, at `user_parallelism = no`.
pub trait Cleanup: 'static {
    /// The value's cleanup, which may pause and may fail. The failure is its
    /// message, since it is handed on without its type
    /// ([ADR-239](../../../docs/specification/adr/adr-239.md) D3).
    fn cleanup(&mut self) -> Pin<Box<dyn Future<Output = Result<(), String>> + '_>>;

    /// What the value is, as a message names it.
    fn describe(&self) -> String {
        format!("a `{}`", short_name::<Self>())
    }
}

/// The same, where the cleanup may move with its task, at
/// `user_parallelism = yes`.
pub trait CleanupSend: Send + 'static {
    /// See [`Cleanup::cleanup`].
    fn cleanup(&mut self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    /// See [`Cleanup::describe`].
    fn describe(&self) -> String {
        format!("a `{}`", short_name::<Self>())
    }
}

/// The last segment of a type's path, which is the name a program wrote.
fn short_name<T: ?Sized>() -> &'static str {
    let full = std::any::type_name::<T>();
    let head = full.split('<').next().unwrap_or(full);
    head.rsplit("::").next().unwrap_or(head)
}

/// **What a cleanup's own result becomes**: nothing, where it cannot fail, and
/// its message where it did.
pub trait Outcome {
    /// The result as a parked value reports it.
    fn reported(self) -> Result<(), String>;
}

impl Outcome for () {
    fn reported(self) -> Result<(), String> {
        Ok(())
    }
}

impl<E: std::fmt::Display> Outcome for Result<(), E> {
    fn reported(self) -> Result<(), String> {
        self.map_err(|error| error.to_string())
    }
}

/// **A value of a type with a cleanup that can pause**, at
/// `user_parallelism = no`: the value, and a `Drop` that parks its cleanup.
///
/// Reached through as the value itself (`Deref`), so a field, a method and a
/// view of it are what they would be without it.
pub struct Cleaned<T: Cleanup>(Option<T>);

/// The same at `user_parallelism = yes`, where the cleanup may move with its
/// task.
pub struct CleanedSend<T: CleanupSend>(Option<T>);

macro_rules! cleaned {
    ($cleaned:ident, $trait:ident, $queue:ident, $work:ty) => {
        impl<T: $trait> $cleaned<T> {
            /// A value, whose cleanup this now owes.
            pub fn new(value: T) -> Self {
                Self(Some(value))
            }

            /// **Close it now** (ADR-239 D3): run the cleanup here, hand its
            /// failure back as the call's, and owe nothing afterwards.
            pub async fn close(mut self) -> Result<(), Failure> {
                let mut value = self.0.take().expect("a value is closed once");
                let what = value.describe();
                let outcome = value.cleanup().await;
                drop(value);
                outcome.map_err(|message| Failure {
                    failed: vec![(what, message)],
                })
            }
        }

        impl<T: $trait> std::ops::Deref for $cleaned<T> {
            type Target = T;
            fn deref(&self) -> &T {
                self.0
                    .as_ref()
                    .expect("a value is reached before it is closed")
            }
        }

        impl<T: $trait> std::ops::DerefMut for $cleaned<T> {
            fn deref_mut(&mut self) -> &mut T {
                self.0
                    .as_mut()
                    .expect("a value is reached before it is closed")
            }
        }

        impl<T: $trait + std::fmt::Display> std::fmt::Display for $cleaned<T> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                (**self).fmt(f)
            }
        }

        impl<T: $trait + std::fmt::Debug> std::fmt::Debug for $cleaned<T> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                (**self).fmt(f)
            }
        }

        // **What the value derives, the wrapper has**: a struct that holds one
        // derives what its fields have, and a copy is a value of its own with
        // a cleanup of its own.
        impl<T: $trait + Clone> Clone for $cleaned<T> {
            fn clone(&self) -> Self {
                Self::new((**self).clone())
            }
        }

        impl<T: $trait + PartialEq> PartialEq for $cleaned<T> {
            fn eq(&self, other: &Self) -> bool {
                **self == **other
            }
        }

        impl<T: $trait + Eq> Eq for $cleaned<T> {}

        impl<T: $trait + PartialOrd> PartialOrd for $cleaned<T> {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                (**self).partial_cmp(&**other)
            }
        }

        impl<T: $trait + Ord> Ord for $cleaned<T> {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                (**self).cmp(&**other)
            }
        }

        impl<T: $trait + std::hash::Hash> std::hash::Hash for $cleaned<T> {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                (**self).hash(state)
            }
        }

        impl<T: $trait> Drop for $cleaned<T> {
            fn drop(&mut self) {
                let Some(mut value) = self.0.take() else {
                    return;
                };
                // **A panic parks nothing** (ADR-239 D5): the value may be
                // broken in the way that caused it, so only its synchronous
                // `drop` runs, which is dropping it here.
                if std::thread::panicking() {
                    return;
                }
                let what = value.describe();
                let work: $work = Box::pin(async move {
                    let outcome = value.cleanup().await;
                    drop(value);
                    outcome
                });
                // A thread whose queues are already gone - its own teardown -
                // parks nothing: the work, and the value with it, is dropped
                // here, which runs its `impl Drop`.
                let _ = $queue.try_with(|queue| queue.borrow_mut().push(Parked { what, work }));
            }
        }
    };
}

cleaned!(Cleaned, Cleanup, LOCAL, Work);
cleaned!(CleanedSend, CleanupSend, SENT, SendWork);

/// **What failed at a settle point**: each value's description and message, in
/// the order they died.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    failed: Vec<(String, String)>,
}

impl Failure {
    /// What failed, and how.
    pub fn failed(&self) -> &[(String, String)] {
        &self.failed
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (at, (what, message)) in self.failed.iter().enumerate() {
            if at > 0 {
                f.write_str("; ")?;
            }
            write!(f, "cleaning up {what} failed: {message}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Failure {}

/// **Which of the task's queues a settle point drains**, which is what decides
/// whether the settle point may cross threads. `Local` drains both: a value
/// that may cross threads is cleaned up as well on the thread it died on, and
/// its work is written as local work. `Sent` drains only the queue whose work
/// may cross, which is the one there is at `user_parallelism = yes`, where
/// every value with a cleanup is one that may cross and the task that settles
/// may run on the pool.
pub trait Kind {
    /// The work this kind runs.
    type Work: Future<Output = Result<(), String>> + Unpin;
    /// The queues a task or a branch of this kind keeps of its own.
    type Queues: Queue + Default;
    /// Everything parked in this task that this kind runs, in the order it
    /// died.
    fn taken() -> Vec<Parked<Self::Work>>;
}

/// Both queues, at `user_parallelism = no`.
pub struct Local;

/// The queue whose work may cross threads, at `user_parallelism = yes`.
pub struct Sent;

impl Kind for Local {
    type Work = Work;
    type Queues = Queues;

    fn taken() -> Vec<Parked<Work>> {
        let mut local = LOCAL
            .try_with(|queue| std::mem::take(&mut *queue.borrow_mut()))
            .unwrap_or_default();
        let sent = SENT
            .try_with(|queue| std::mem::take(&mut *queue.borrow_mut()))
            .unwrap_or_default();
        local.extend(sent.into_iter().map(|parked| Parked {
            what: parked.what,
            work: parked.work as Work,
        }));
        local
    }
}

impl Kind for Sent {
    type Work = SendWork;
    type Queues = SendQueues;

    fn taken() -> Vec<Parked<SendWork>> {
        SENT.try_with(|queue| std::mem::take(&mut *queue.borrow_mut()))
            .unwrap_or_default()
    }
}

/// **Run what this task parked**, in the order it died, and say what failed.
pub async fn settle<K: Kind>() -> Result<(), Failure> {
    let mut failed = Vec::new();
    for parked in K::taken() {
        if let Some(failure) = parked.run().await {
            failed.push(failure);
        }
    }
    match failed.is_empty() {
        true => Ok(()),
        false => Err(Failure { failed }),
    }
}

/// **A settle point where nothing that dies can fail**: the work runs, and
/// there is nothing to hand back.
pub async fn settle_quietly<K: Kind>() {
    let _ = settle::<K>().await;
}

/// **The settle point around a function's body**
/// ([ADR-239](../../../docs/specification/adr/adr-239.md) D2): the body is run
/// to its end - every `return` and every failing call inside it ends the body
/// and not the function - and then what died in it is settled. A failure while
/// the body already failed is the body's failure's **secondary** (D3); one
/// where the body succeeded is the function's failure.
pub async fn settle_after<K: Kind, T, E, F>(body: F) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
    E: From<Failure> + crate::error::Joined,
{
    let result = body.await;
    joined(result, settle::<K>().await)
}

/// The same around a function that cannot fail. A cleanup that failed there has
/// no caller to go to, so it is said on standard error, as an orphan's is.
pub async fn settle_with<K: Kind, T, F: Future<Output = T>>(body: F) -> T {
    let value = body.await;
    if let Err(failure) = settle::<K>().await {
        eprintln!("nikaia: {failure}");
    }
    value
}

/// **A task's own queues**, swapped in while it is polled so that what it
/// parks is its own and what it settles is its own.
/// **Queues of one's own** - a task's, or an `overlap` branch's - swapped in
/// while it is polled, so that what it parks and what it settles are its own.
pub trait Queue {
    /// Swap these in, and what was in back here.
    fn swap(&mut self);
    /// **What nobody will settle**, handed to the runtime: the task is over.
    fn orphan(&mut self);
    /// **What the enclosing task will settle**, handed to it: the branch is
    /// over, and what it could not settle itself goes to the settle point
    /// around it.
    fn hand_over(&mut self);
}

#[derive(Default)]
pub struct Queues {
    local: Vec<Parked<Work>>,
    sent: Vec<Parked<SendWork>>,
}

impl Queue for Queues {
    fn swap(&mut self) {
        let _ = LOCAL.try_with(|queue| std::mem::swap(&mut *queue.borrow_mut(), &mut self.local));
        let _ = SENT.try_with(|queue| std::mem::swap(&mut *queue.borrow_mut(), &mut self.sent));
    }

    fn orphan(&mut self) {
        let _ = LOCAL_ORPHANS.try_with(|orphans| orphans.borrow_mut().append(&mut self.local));
        SENT_ORPHANS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .append(&mut self.sent);
    }

    fn hand_over(&mut self) {
        let _ = LOCAL.try_with(|queue| queue.borrow_mut().append(&mut self.local));
        let _ = SENT.try_with(|queue| queue.borrow_mut().append(&mut self.sent));
    }
}

/// The same for a task that may move between threads: only the queue whose
/// work may move with it.
#[derive(Default)]
pub struct SendQueues {
    sent: Vec<Parked<SendWork>>,
}

impl Queue for SendQueues {
    fn swap(&mut self) {
        let _ = SENT.try_with(|queue| std::mem::swap(&mut *queue.borrow_mut(), &mut self.sent));
    }

    fn orphan(&mut self) {
        SENT_ORPHANS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .append(&mut self.sent);
    }

    fn hand_over(&mut self) {
        let _ = SENT.try_with(|queue| queue.borrow_mut().append(&mut self.sent));
    }
}

/// **A branch's own queues**, handed to the enclosing task when the branch is
/// dropped - at its end, or with the `overlap` around it - so nothing it parked
/// is lost.
struct Own<Q: Queue>(Q);

impl<Q: Queue> Drop for Own<Q> {
    fn drop(&mut self) {
        self.0.hand_over();
    }
}

/// A future polled with a branch's own queues in place.
struct Within<'a, F, Q: Queue> {
    body: Pin<&'a mut F>,
    queues: &'a mut Own<Q>,
}

impl<F: Future, Q: Queue> Future for Within<'_, F, Q> {
    type Output = F::Output;

    fn poll(
        self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<F::Output> {
        let me = self.get_mut();
        me.queues.0.swap();
        let polled = me.body.as_mut().poll(context);
        me.queues.0.swap();
        polled
    }
}

/// **An `overlap` branch with a queue of its own**
/// ([ADR-239](../../../docs/specification/adr/adr-239.md) D3): the branches
/// run at once in one task, and what one of them parks is not another's to
/// settle. What is still parked at its end goes to the settle point around the
/// `overlap`.
pub async fn branch<K: Kind, T, F: Future<Output = T>>(body: F) -> T {
    let mut queues = Own(K::Queues::default());
    let body = std::pin::pin!(body);
    Within {
        body,
        queues: &mut queues,
    }
    .await
}

/// **The same, settled at the branch's end**, where the branch's channel can
/// take the failure: a cleanup that fails while the branch is already failing
/// joins **that branch's** error (ADR-115 D3), one that fails where it
/// succeeded is its error.
pub async fn branch_after<K: Kind, T, E, F>(body: F) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
    E: From<Failure> + crate::error::Joined,
{
    let mut queues = Own(K::Queues::default());
    let result = Within {
        body: std::pin::pin!(body),
        queues: &mut queues,
    }
    .await;
    let settled = Within {
        body: std::pin::pin!(settle::<K>()),
        queues: &mut queues,
    }
    .await;
    joined(result, settled)
}

/// A body's result and its settle point's, as one (D3).
fn joined<T, E: From<Failure> + crate::error::Joined>(
    result: Result<T, E>,
    settled: Result<(), Failure>,
) -> Result<T, E> {
    match (result, settled) {
        (result, Ok(())) => result,
        (Ok(_), Err(failure)) => Err(E::from(failure)),
        (Err(mut error), Err(failure)) => {
            error.joined_by(E::from(failure));
            Err(error)
        }
    }
}

/// **Every cleanup nobody will settle**, taken: a cancelled task's, a finished
/// task's that died after its last settle point, and what the thread itself
/// still holds. What the runtime finishes before the program ends.
pub fn orphans() -> (Vec<Parked<Work>>, Vec<Parked<SendWork>>) {
    let mut local = LOCAL_ORPHANS
        .try_with(|orphans| std::mem::take(&mut *orphans.borrow_mut()))
        .unwrap_or_default();
    local.extend(
        LOCAL
            .try_with(|queue| std::mem::take(&mut *queue.borrow_mut()))
            .unwrap_or_default(),
    );
    let mut sent = std::mem::take(&mut *SENT_ORPHANS.lock().unwrap_or_else(|e| e.into_inner()));
    sent.extend(
        SENT.try_with(|queue| std::mem::take(&mut *queue.borrow_mut()))
            .unwrap_or_default(),
    );
    (local, sent)
}

impl<W: Future<Output = Result<(), String>> + Unpin> Parked<W> {
    /// **Run the parked work**, for the runtime's drain: what failed, if it
    /// did.
    pub async fn run(self) -> Option<(String, String)> {
        match self.work.await {
            Ok(()) => None,
            Err(message) => Some((self.what, message)),
        }
    }
}

/// **The orphans started and not finished**, by a number of their own and what
/// they clean up: what an expired `cleanup-deadline` names
/// ([ADR-239](../../../docs/specification/adr/adr-239.md) D5).
static RUNNING: std::sync::Mutex<Vec<(u64, String)>> = std::sync::Mutex::new(Vec::new());

static NUMBERED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn running() -> std::sync::MutexGuard<'static, Vec<(u64, String)>> {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Every orphan, numbered and recorded as running, in the order it died.
fn recorded<W>(parked: Vec<Parked<W>>) -> Vec<(u64, Parked<W>)> {
    let mut running = running();
    parked
        .into_iter()
        .map(|parked| {
            let number = NUMBERED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            running.push((number, parked.what.clone()));
            (number, parked)
        })
        .collect()
}

/// One after the other, in the order they died, as a task would have settled
/// them: a cleanup may rely on the one that died after it being done.
async fn drained<W: Future<Output = Result<(), String>> + Unpin>(orphans: Vec<(u64, Parked<W>)>) {
    for (number, parked) in orphans {
        let failed = parked.run().await;
        running().retain(|(running, _)| *running != number);
        said(failed);
    }
}

/// **Start every orphan on this thread**, where the drain at the end of `main`
/// waits for it as for any task nobody joined (ADR-239 D5). A failure has no
/// caller to go to, so it is said on standard error. How many were started.
pub fn start_orphans() -> usize {
    let (local, sent) = orphans();
    let started = local.len() + sent.len();
    if !local.is_empty() {
        crate::rt::exec::start(drained(recorded(local)));
    }
    if !sent.is_empty() {
        crate::rt::exec::start(drained(recorded(sent)));
    }
    started
}

/// **What the orphans still running clean up**, in the order they died: the
/// resources an expired deadline cut off.
pub fn unfinished() -> Vec<String> {
    running().iter().map(|(_, what)| what.clone()).collect()
}

fn said(failed: Option<(String, String)>) {
    if let Some((what, message)) = failed {
        eprintln!("nikaia: cleaning up {what} failed: {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Noted {
        name: &'static str,
        log: Arc<Mutex<Vec<String>>>,
        fails: bool,
    }

    impl Cleanup for Noted {
        fn cleanup(&mut self) -> Pin<Box<dyn Future<Output = Result<(), String>> + '_>> {
            Box::pin(async move {
                self.log
                    .lock()
                    .unwrap()
                    .push(format!("cleanup {}", self.name));
                match self.fails {
                    true => Err(format!("{} would not flush", self.name)),
                    false => Ok(()),
                }
            })
        }
    }

    impl Drop for Noted {
        fn drop(&mut self) {
            self.log.lock().unwrap().push(format!("drop {}", self.name));
        }
    }

    fn noted(name: &'static str, log: &Arc<Mutex<Vec<String>>>, fails: bool) -> Cleaned<Noted> {
        Cleaned::new(Noted {
            name,
            log: log.clone(),
            fails,
        })
    }

    fn run<T>(future: impl Future<Output = T>) -> T {
        crate::rt::exec::block_on(future)
    }

    /// **Dying parks; the settle point runs it, in the order they died, and the
    /// synchronous `drop` after each cleanup.**
    #[test]
    fn values_that_die_are_cleaned_up_at_the_settle_point() {
        let log = Arc::new(Mutex::new(Vec::new()));
        run(async {
            {
                let _a = noted("a", &log, false);
                let _b = noted("b", &log, false);
            }
            assert!(
                log.lock().unwrap().is_empty(),
                "nothing runs before the settle point"
            );
            settle::<Local>().await.expect("nothing failed");
        });
        assert_eq!(
            *log.lock().unwrap(),
            ["cleanup b", "drop b", "cleanup a", "drop a"]
        );
    }

    /// **What failed is said, and a failure while the body failed is its
    /// secondary.**
    #[test]
    fn a_failure_is_the_settle_points_and_joins_one_already_there() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let failed = run(async {
            drop(noted("a", &log, true));
            settle::<Local>().await
        })
        .expect_err("it failed");
        assert_eq!(failed.failed()[0].0, "a `Noted`");
        assert!(failed.to_string().contains("a would not flush"), "{failed}");

        let result: Result<(), Box<dyn std::error::Error>> =
            run(settle_after::<Local, _, _, _>(async {
                drop(noted("b", &log, true));
                Ok(())
            }));
        assert!(result.is_err());
    }

    /// **`close` runs the cleanup now and owes nothing after.**
    #[test]
    fn close_runs_it_now_and_nothing_is_parked() {
        let log = Arc::new(Mutex::new(Vec::new()));
        run(async {
            let value = noted("c", &log, false);
            value.close().await.expect("closed");
            settle::<Local>().await.expect("nothing parked");
        });
        assert_eq!(*log.lock().unwrap(), ["cleanup c", "drop c"]);
    }

    /// **A panic parks nothing**: only the synchronous `drop` runs.
    #[test]
    fn a_panic_runs_only_the_drop() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _value = noted("p", &log, false);
            panic!("a bug");
        }));
        assert!(caught.is_err());
        assert_eq!(*log.lock().unwrap(), ["drop p"]);
        assert!(LOCAL.with(|queue| queue.borrow().is_empty()));
    }
}
