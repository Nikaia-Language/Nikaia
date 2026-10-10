//! `std::supervisor`: a supervisor runs tasks and restarts them when they end
//! ([ADR-328](../../../docs/specification/adr/adr-328.md), Part II 12.8).
//!
//! ```nika
//! supervisor::run([
//!     supervisor::child(fn { refresh_loop(cache) }),
//!     supervisor::child(fn { serve(config) }; restart: supervisor::Kind::Transient),
//! ]; strategy: supervisor::Strategy::OneForOne)
//! ```
//!
//! **The list's order is the start order and the dependency** (D3): children
//! start in list order and stop in reverse. The strategy says which siblings
//! restart with a crashed child, the child's kind which ends are restarted,
//! and the policy when. `run` returns only when the supervisor gives up, and
//! throws [`Escalated`] then, after stopping every child.
//!
//! **When, a policy decides** (D5): a value of a type implementing
//! [`Restart`], `std`'s default [`Backoff`] where the child names none. The
//! trait and the types it speaks in are Nikaia, `supervisor.nika`, lowered to
//! `supervisor.rs` and included here.

use std::time::{Duration, Instant};

use crate::task::TaskHandle;

include!("supervisor.rs");

/// Which siblings restart with a child that crashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// The child alone.
    OneForOne,
    /// Every child.
    OneForAll,
    /// The child and every child after it in the list.
    RestForOne,
}

/// Which ends of a child are restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Every end, a normal one too.
    Permanent,
    /// A panic or a thrown error, not a normal end.
    Transient,
    /// None; a crash is reported.
    Temporary,
}

/// **What `run` throws when it gives up**: the policy of a child that crashed
/// said so. Every child was stopped first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Escalated {
    /// What the last crash said.
    pub message: String,
}

impl std::fmt::Display for Escalated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the supervisor gave up: {}", self.message)
    }
}

impl std::error::Error for Escalated {}

/// What a child's function ends with: nothing, or what it threw.
pub type Outcome = Result<(), Box<dyn std::error::Error>>;

/// A child at `user_parallelism = no`.
pub struct Child {
    body: crate::func::Kept<dyn Fn() -> crate::func::Boxed<Outcome>>,
    kind: Kind,
    /// Asked at every restart, and changed by the asking: `decide` takes
    /// `ref mut self`.
    policy: std::cell::RefCell<Box<dyn Restart>>,
}

/// A child where tasks run on threads.
pub struct PoolChild {
    body: crate::func::Kept<dyn Fn() -> crate::func::SendBoxed<Outcome> + Send + Sync>,
    kind: Kind,
    policy: std::sync::Mutex<Box<dyn Restart + Send>>,
}

/// `supervisor::child(fn { … }; restart: …, policy: …)`.
pub fn child(
    body: crate::func::Kept<dyn Fn() -> crate::func::Boxed<Outcome>>,
    restart: Kind,
    policy: impl Restart + 'static,
) -> Child {
    Child {
        body,
        kind: restart,
        policy: std::cell::RefCell::new(Box::new(policy)),
    }
}

/// [`child`] where tasks run on threads.
pub fn child_on_pool(
    body: crate::func::Kept<dyn Fn() -> crate::func::SendBoxed<Outcome> + Send + Sync>,
    restart: Kind,
    policy: impl Restart + Send + 'static,
) -> PoolChild {
    PoolChild {
        body,
        kind: restart,
        policy: std::sync::Mutex::new(Box::new(policy)),
    }
}

/// How one attempt of a child ended: normally, or with what it said.
type Ended = Option<String>;

/// The message a child's outcome carries, where it is a failure.
fn ended(outcome: Outcome) -> Ended {
    outcome.err().map(|error| error.to_string())
}

/// **Run the supervisor** until it gives up (ADR-328 D1-D4).
pub async fn run(children: &[Child], strategy: Strategy) -> Result<(), Escalated> {
    let kinds: Vec<Kind> = children.iter().map(|c| c.kind).collect();
    supervise(
        &kinds,
        strategy,
        |at| {
            let body = children[at].body.clone();
            TaskHandle::start(async move { ended(body().await) })
        },
        |at, history| children[at].policy.borrow_mut().decide(history),
    )
    .await
}

/// [`run`] where tasks run on threads.
pub async fn run_on_pool(children: &[PoolChild], strategy: Strategy) -> Result<(), Escalated> {
    let kinds: Vec<Kind> = children.iter().map(|c| c.kind).collect();
    supervise(
        &kinds,
        strategy,
        |at| {
            let body = children[at].body.clone();
            TaskHandle::start_on_pool(async move { ended(body().await) })
        },
        |at, history| {
            children[at]
                .policy
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .decide(history)
        },
    )
    .await
}

/// The supervisor itself, over a way to start child `at` and to ask child
/// `at`'s policy: written once for both executors, and for the tests' policies.
///
/// **The history is the supervisor's**, per child: how long the attempt that
/// just ended ran, how many failures the child has had, and how long since the
/// first (D5). Its cycle is the child's life; a policy that starts a new one
/// after a healthy run - [`Backoff`] - keeps where it began itself.
pub(crate) async fn supervise(
    kinds: &[Kind],
    strategy: Strategy,
    start: impl Fn(usize) -> TaskHandle<Ended>,
    decide: impl Fn(usize, Failures) -> Next,
) -> Result<(), Escalated> {
    let count = kinds.len();
    let mut running: Vec<Option<TaskHandle<Ended>>> =
        (0..count).map(|at| Some(start(at))).collect();
    let mut started: Vec<Instant> = vec![Instant::now(); count];
    let mut failures: Vec<i64> = vec![0; count];
    let mut first: Vec<Option<Instant>> = vec![None; count];
    loop {
        if running.iter().all(Option::is_none) {
            return Ok(());
        }
        let (at, outcome) = FirstEnded { running: &running }.await;
        running[at] = None;
        let crash = match outcome {
            Ok(None) => None,
            Ok(Some(thrown)) => Some(thrown),
            Err(crashed) => Some(crashed.message),
        };
        let restarted = match kinds[at] {
            Kind::Permanent => true,
            Kind::Transient => crash.is_some(),
            Kind::Temporary => {
                if let Some(said) = &crash {
                    eprintln!("supervisor: a temporary child crashed and is not restarted: {said}");
                }
                false
            }
        };
        if !restarted {
            continue;
        }
        let said = crash.unwrap_or_else(|| "the child ended".to_string());
        // **The policy of the child that crashed decides for the siblings it
        // takes along** (Part II 12.8).
        let now = Instant::now();
        failures[at] += 1;
        let history = Failures {
            last_uptime: now.duration_since(started[at]),
            consecutive: failures[at],
            since_first: now.duration_since(*first[at].get_or_insert(now)),
        };
        match decide(at, history) {
            Next::Escalate => {
                for handle in running.iter_mut().rev() {
                    if let Some(handle) = handle.take() {
                        handle.cancel();
                    }
                }
                return Err(Escalated { message: said });
            }
            Next::Delay(span) => crate::time::sleep(span).await,
            Next::Immediate => {}
        }
        let again: Vec<usize> = match strategy {
            Strategy::OneForOne => vec![at],
            Strategy::OneForAll => (0..count).collect(),
            Strategy::RestForOne => (at..count).collect(),
        };
        // Stopped in reverse order, started in list order.
        for &sibling in again.iter().rev() {
            if let Some(handle) = running[sibling].take() {
                handle.cancel();
            }
        }
        for &sibling in &again {
            running[sibling] = Some(start(sibling));
            started[sibling] = Instant::now();
        }
    }
}

/// The first of the running children to end, and how.
struct FirstEnded<'a> {
    running: &'a [Option<TaskHandle<Ended>>],
}

impl std::future::Future for FirstEnded<'_> {
    type Output = (usize, Result<Ended, crate::task::Crashed>);

    fn poll(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        for (at, handle) in self.running.iter().enumerate() {
            if let Some(handle) = handle
                && let std::task::Poll::Ready(outcome) = handle.poll_ended(context)
            {
                return std::task::Poll::Ready((at, outcome));
            }
        }
        std::task::Poll::Pending
    }
}

/// **`std`'s default policy** (ADR-328, Part II 12.8): the first failure is
/// restarted at once, then after a delay that grows, with jitter, up to a
/// ceiling; an attempt that ran long enough starts a new cycle; a cycle that
/// lasts too long gives up.
///
/// **The numbers are starting values**, not measured: what a service on one
/// machine tolerates. A program that needs others passes another policy.
///
/// **Its cycle is its own**: the supervisor's history counts the child's whole
/// life, and an attempt that ran [`Backoff::healthy`] long starts a new cycle
/// here, kept as where in that history it began.
#[derive(Debug, Clone)]
pub struct Backoff {
    /// The second failure's delay; each after it doubles.
    pub first_delay: Duration,
    /// No delay is longer.
    pub ceiling: Duration,
    /// An attempt that ran this long ends the cycle: it was healthy.
    pub healthy: Duration,
    /// A cycle that lasts longer than this gives up.
    pub give_up_after: Duration,
    /// The failures before this cycle, and how long after the first failure
    /// it began.
    cycle_from: (i64, Duration),
    seed: u64,
}

/// `supervisor::Backoff()`: `std`'s default policy, the value `policy:` has
/// where a child names none.
#[allow(non_snake_case)]
pub fn Backoff() -> Backoff {
    Backoff::default()
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            first_delay: Duration::from_millis(100),
            ceiling: Duration::from_secs(30),
            healthy: Duration::from_secs(60),
            give_up_after: Duration::from_secs(300),
            cycle_from: (0, Duration::ZERO),
            seed: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u64)
                .unwrap_or(1)
                | 1,
        }
    }
}

impl Restart for Backoff {
    /// One failure, from the supervisor's history of the child.
    fn decide(&mut self, history: Failures) -> Next {
        if history.last_uptime >= self.healthy {
            self.cycle_from = (history.consecutive - 1, history.since_first);
        }
        let failures = history.consecutive - self.cycle_from.0;
        let lasted = history.since_first.saturating_sub(self.cycle_from.1);
        if lasted > self.give_up_after {
            return Next::Escalate;
        }
        if failures <= 1 {
            return Next::Immediate;
        }
        let doubled = self
            .first_delay
            .saturating_mul(1u32 << (failures - 2).min(20) as u32);
        let delay = doubled.min(self.ceiling);
        // **Jitter**, so that children that crashed together do not restart
        // together: up to a quarter less, never more than the ceiling.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        let quarter = delay / 4;
        let off = match quarter.as_nanos() as u64 {
            0 => 0,
            n => self.seed % n,
        };
        Next::Delay(delay - Duration::from_nanos(off))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `n`th failure, after an attempt that ran `ran`, `since` after the
    /// first: the supervisor's history, written by hand.
    fn failure(n: i64, ran: Duration, since: Duration) -> Failures {
        Failures {
            last_uptime: ran,
            consecutive: n,
            since_first: since,
        }
    }

    /// The first failure restarts at once, then the delay grows, stays under
    /// the ceiling, and a long cycle gives up.
    #[test]
    fn the_default_policy_grows_and_gives_up() {
        let mut policy = Backoff::default();
        assert_eq!(
            policy.decide(failure(1, Duration::ZERO, Duration::ZERO)),
            Next::Immediate
        );
        let mut last = Duration::ZERO;
        for n in 0..12 {
            match policy.decide(failure(
                n + 2,
                Duration::ZERO,
                Duration::from_secs(n as u64),
            )) {
                Next::Delay(d) => {
                    assert!(d <= policy.ceiling, "{d:?}");
                    if n < 6 {
                        assert!(
                            d >= last.mul_f64(1.2) || d >= policy.ceiling.mul_f64(0.75),
                            "{d:?} after {last:?}"
                        );
                    }
                    last = d;
                }
                other => panic!("a delay, not {other:?}"),
            }
        }
        assert_eq!(
            policy.decide(failure(14, Duration::ZERO, Duration::from_secs(301))),
            Next::Escalate
        );
    }

    /// An attempt that ran long enough starts a new cycle.
    #[test]
    fn a_healthy_run_starts_a_new_cycle() {
        let mut policy = Backoff::default();
        assert_eq!(
            policy.decide(failure(1, Duration::ZERO, Duration::ZERO)),
            Next::Immediate
        );
        assert!(matches!(
            policy.decide(failure(2, Duration::ZERO, Duration::ZERO)),
            Next::Delay(_)
        ));
        assert_eq!(
            policy.decide(failure(
                3,
                Duration::from_secs(61),
                Duration::from_secs(400)
            )),
            Next::Immediate
        );
        assert!(matches!(
            policy.decide(failure(4, Duration::ZERO, Duration::from_secs(401))),
            Next::Delay(_)
        ));
    }

    /// One policy, asked for every child, as `run` asks each child's own.
    fn asking(policy: Backoff) -> impl Fn(usize, Failures) -> Next {
        let held = std::cell::RefCell::new(policy);
        move |_, history| held.borrow_mut().decide(history)
    }

    fn crashing(times: usize) -> impl Fn(usize) -> TaskHandle<Ended> {
        let left = std::rc::Rc::new(std::cell::Cell::new(times));
        move |_| {
            let left = left.clone();
            TaskHandle::start(async move {
                match left.get() {
                    0 => None,
                    n => {
                        left.set(n - 1);
                        Some("it broke".to_string())
                    }
                }
            })
        }
    }

    fn quick() -> Backoff {
        Backoff {
            first_delay: Duration::from_millis(1),
            ceiling: Duration::from_millis(2),
            ..Backoff::default()
        }
    }

    /// A transient child that fails twice and then ends normally is restarted
    /// twice, and the supervisor then has nothing left to run.
    #[test]
    fn a_transient_child_is_restarted_until_it_ends_normally() {
        let outcome = crate::rt::exec::block_on(supervise(
            &[Kind::Transient],
            Strategy::OneForOne,
            crashing(2),
            asking(quick()),
        ));
        assert_eq!(outcome, Ok(()));
    }

    /// A policy that gives up makes `run` throw what the last crash said.
    #[test]
    fn a_policy_that_gives_up_escalates() {
        let outcome = crate::rt::exec::block_on(supervise(
            &[Kind::Transient],
            Strategy::OneForOne,
            crashing(usize::MAX),
            asking(Backoff {
                give_up_after: Duration::ZERO,
                ..quick()
            }),
        ));
        assert_eq!(
            outcome,
            Err(Escalated {
                message: "it broke".to_string()
            })
        );
    }

    /// A temporary child is not restarted.
    #[test]
    fn a_temporary_child_is_not_restarted() {
        let outcome = crate::rt::exec::block_on(supervise(
            &[Kind::Temporary],
            Strategy::OneForOne,
            crashing(1),
            asking(quick()),
        ));
        assert_eq!(outcome, Ok(()));
    }
}
