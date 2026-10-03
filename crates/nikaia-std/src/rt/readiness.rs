//! **One poller, a registration each, and no worker in between**
//! ([ADR-303](../../../../docs/specification/adr/adr-303.md) D3's readiness
//! half, as [ADR-289](../../../../docs/specification/adr/adr-289.md) D6's socket
//! needs it).
//!
//! ## What this replaces, and why it is not about speed
//!
//! A readiness wait used to be an [`super::worker::Op`]: the descriptor was
//! duplicated, handed to an I/O worker over a channel, and the worker **blocked
//! in `poll_one` for the whole of the wait**. `io-workers` is `1` by default
//! ([ADR-303](../../../../docs/specification/adr/adr-303.md) D4 — *one I/O
//! thread always runs*), so a wait that had not answered blocked every other
//! wait in the process. A server waiting on `accept` while a connection waited
//! on `read` was a server that stalled, and that is the shape an HTTP server
//! has.
//!
//! Measured on the way in, and the numbers chose this shape
//! ([ADR-296](../../../../docs/specification/adr/adr-296.md) D12):
//!
//! | | per wait |
//! | :--- | ---: |
//! | a poller built per wait, which is what the worker did | 7.4 µs |
//! | one poller, `add` and `delete` around each wait | 2.5 µs |
//! | a registration kept and re-armed | 1.2 µs |
//! | **the whole round trip through a worker** | **35 µs** |
//!
//! So the poller was never the cost: **28 of the 35 µs was the hop**, and the
//! occupancy was not a cost at all but a ceiling.
//!
//! ## The shape
//!
//! One [`polled::Poller`] for the process and one thread inside its `wait`.
//! Arming is an `epoll_ctl` on the **calling** thread — it does not block, so
//! there is nothing to hand to anybody. When the kernel answers, the thread
//! fills the slot, wakes the waker, and rings the bell
//! ([ADR-121](../../../../docs/specification/adr/adr-121.md) D1), because a
//! parked executor is asleep in the ring or on the condvar and a `Waker` alone
//! does not reach either.

use std::collections::BTreeMap;
use std::io::{Error, Result};
use std::os::fd::AsFd;
use std::sync::{Condvar, Mutex, OnceLock};
use std::task::Waker;
use std::time::{Duration, Instant};

use polled::{Event, Events, Poller};

use super::Interest;

/// The process's one poller and the slots waiting on it.
pub(crate) struct Registry {
    poller: Poller,
    slots: Mutex<Slots>,
    /// What a **blocking** wait waits on. The future path uses the waker; a
    /// caller that is not in an executor — a `build.rs`, a unit test, `std`'s
    /// own [`super::io::wait`] — has no waker to store.
    answered: Condvar,
}

#[derive(Default)]
struct Slots {
    next: usize,
    open: BTreeMap<usize, Slot>,
}

/// A registration's state. The descriptor itself - a **duplicate** of the
/// caller's, so the registration holds the kernel object open whatever the
/// caller does with its own copy - is held by the poller under the slot's key,
/// which is what keeps it open for exactly as long as it is registered.
struct Slot {
    deadline: Option<Instant>,
    waker: Option<Waker>,
    /// `Some` once the answer is in: `true` ready, `false` the deadline.
    answer: Option<Result<bool>>,
}

static REGISTRY: OnceLock<Registry> = OnceLock::new();

/// The registry, starting its thread the first time anything waits.
///
/// Lazily and not in [`super::Runtime::build`], because a readiness wait is not
/// something every program does and a thread that nothing uses is a thread
/// nobody asked for.
pub(crate) fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| Registry {
        poller: Poller::new().expect("a poller for the process's readiness waits"),
        slots: Mutex::new(Slots::default()),
        answered: Condvar::new(),
    })
}

/// Start the thread that answers, once.
fn answering() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::thread::Builder::new()
            .name("nikaia-readiness".to_string())
            .spawn(|| registry().answer_forever())
            .expect("the readiness thread starts");
    });
}

impl Registry {
    /// Register `socket` and hand back the key to wait on.
    fn arm(
        &'static self,
        socket: &impl AsFd,
        interest: Interest,
        timeout: Option<Duration>,
    ) -> Result<usize> {
        answering();
        let fd = socket.as_fd().try_clone_to_owned()?;
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        slots.next += 1;
        let key = slots.next;
        let event = match interest {
            Interest::Readable => Event::readable(key),
            Interest::Writable => Event::writable(key),
        };
        self.poller.add(fd, event)?;
        slots.open.insert(
            key,
            Slot {
                deadline: timeout.map(|limit| Instant::now() + limit),
                waker: None,
                answer: None,
            },
        );
        drop(slots);
        // The thread may be inside a `wait` whose bound is later than this
        // slot's deadline, or inside one with no bound at all.
        let _ = self.poller.notify();
        Ok(key)
    }

    /// Take the answer where there is one, and leave the waker where there is
    /// not.
    fn taken(&self, key: usize, waker: Option<&Waker>) -> Option<Result<bool>> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let slot = slots.open.get_mut(&key)?;
        if slot.answer.is_some() {
            let slot = slots.open.remove(&key).expect("looked at just above");
            self.deregister(key);
            return slot.answer;
        }
        if let Some(waker) = waker {
            slot.waker = Some(waker.clone());
        }
        None
    }

    /// Wait for one key on this thread, for a caller that has no executor.
    fn blocked_on(&self, key: usize) -> Result<bool> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match slots.open.get(&key) {
                None => return Err(Error::other("the readiness registration went away")),
                Some(slot) if slot.answer.is_some() => {
                    let slot = slots.open.remove(&key).expect("looked at just above");
                    self.deregister(key);
                    return slot.answer.expect("checked just above");
                }
                Some(_) => {
                    slots = self.answered.wait(slots).unwrap_or_else(|e| e.into_inner());
                }
            }
        }
    }

    /// Drop a registration nobody is waiting on any more — a future that was
    /// cancelled, which for a socket is a connection the program stopped
    /// caring about.
    fn forget(&self, key: usize) {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if slots.open.remove(&key).is_some() {
            self.deregister(key);
        }
    }

    /// Take the descriptor out of the poller, which closes the duplicate once
    /// it is no longer registered.
    fn deregister(&self, key: usize) {
        drop(self.poller.remove(key));
        // **And tell whoever is waiting for the set to empty.** A slot leaving
        // is as much a change as an answer arriving, and [`Self::drain`] is
        // asleep on exactly that question. Without this a drain slept out its
        // whole deadline over a registration that had already gone — measured
        // as three socket tests taking 30 seconds together and no time at all
        // apart.
        self.answered.notify_all();
    }

    /// How many registrations are open.
    ///
    /// **Counted into [`super::Runtime::pending`]**, and that is what keeps
    /// every promise the worker path made: the park asks *is anything
    /// outstanding* before it sleeps, and the drain
    /// ([ADR-297](../../../../docs/specification/adr/adr-297.md) D6) asks it
    /// before it lets a program go. A readiness wait was a worker operation and
    /// answered both; what changed is the mechanism and not the answer.
    pub(crate) fn outstanding(&self) -> usize {
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .open
            .len()
    }

    /// Wait out the registrations, bounded, and hand back how many are left.
    ///
    /// [ADR-297](../../../../docs/specification/adr/adr-297.md) D6's drain, for
    /// the half that used to be the worker's: a wait on a socket nobody writes
    /// to may never answer, and the deadline is what bounds it.
    pub(crate) fn drain(&self, deadline: Duration) -> usize {
        let until = Instant::now() + deadline;
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        while !slots.open.is_empty() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let (held, _) = self
                .answered
                .wait_timeout(slots, left)
                .unwrap_or_else(|e| e.into_inner());
            slots = held;
        }
        slots.open.len()
    }

    /// The thread. One `wait`, bounded by the nearest deadline there is.
    fn answer_forever(&self) {
        let mut events = Events::new();
        loop {
            let bound = {
                let slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
                slots
                    .open
                    .values()
                    .filter_map(|slot| slot.deadline)
                    .min()
                    .map(|at| at.saturating_duration_since(Instant::now()))
            };
            events.clear();
            if self.poller.wait(&mut events, bound).is_err() {
                // A poller that cannot wait is a runtime that cannot answer,
                // and spinning on it would be worse than pausing.
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }

            let mut woken = Vec::new();
            // **Whether an answer landed, and not whether a waker did.** A
            // caller that blocks stores none — it is on the condvar below —
            // and a thread that only woke the wakers it found would leave such
            // a caller asleep on an answer that is already in its slot. Met by
            // `a_socket_reports_readiness_through_the_std_surface` hanging on a
            // **timeout**, where there is no event either.
            let mut changed = false;
            {
                let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
                for event in events.iter() {
                    if let Some(slot) = slots.open.get_mut(&event.key) {
                        if slot.answer.is_none() {
                            slot.answer = Some(Ok(true));
                            changed = true;
                        }
                        woken.extend(slot.waker.take());
                    }
                }
                // **And the deadlines**, which the bound above woke this for.
                let now = Instant::now();
                for slot in slots.open.values_mut() {
                    if slot.answer.is_none() && slot.deadline.is_some_and(|at| at <= now) {
                        // `Ok(false)` and never an error: a deadline has to be
                        // something a caller can tell from a failure.
                        slot.answer = Some(Ok(false));
                        changed = true;
                        woken.extend(slot.waker.take());
                    }
                }
            }

            if changed {
                self.answered.notify_all();
                for waker in woken {
                    waker.wake();
                }
                // **The bell, because a `Waker` does not reach a parked
                // executor** ([ADR-121](../../../../docs/specification/adr/adr-121.md)
                // D1): that thread is asleep in `io_uring_enter` or on the
                // condvar, and neither watches an alarm.
                super::ring_the_bell();
            }
        }
    }
}

/// A registration, and the future over it.
///
/// Dropping one before it answers deletes the registration, which is what makes
/// a cancelled wait cost nothing.
pub struct Armed {
    key: Option<usize>,
    /// The error from arming, where there was one: a future that is already an
    /// answer.
    failed: Option<Error>,
}

impl Armed {
    /// Wait on this thread, for a caller with no executor.
    pub(crate) fn blocking(mut self) -> Result<bool> {
        if let Some(error) = self.failed.take() {
            return Err(error);
        }
        let key = self.key.take().expect("armed or failed");
        registry().blocked_on(key)
    }
}

impl std::future::Future for Armed {
    type Output = Result<bool>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        use std::task::Poll;

        let this = self.get_mut();
        if let Some(error) = this.failed.take() {
            return Poll::Ready(Err(error));
        }
        let Some(key) = this.key else {
            return Poll::Ready(Err(Error::other(
                "a readiness future was polled after it finished",
            )));
        };
        match registry().taken(key, Some(context.waker())) {
            Some(answer) => {
                this.key = None;
                Poll::Ready(answer)
            }
            None => Poll::Pending,
        }
    }
}

impl Drop for Armed {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            registry().forget(key);
        }
    }
}

/// Register `socket` and hand back the future.
pub(crate) fn arm(socket: &impl AsFd, interest: Interest, timeout: Option<Duration>) -> Armed {
    match registry().arm(socket, interest, timeout) {
        Ok(key) => Armed {
            key: Some(key),
            failed: None,
        },
        Err(error) => Armed {
            key: None,
            failed: Some(error),
        },
    }
}
