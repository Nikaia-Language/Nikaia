//! When the executor should look again, where what it is waiting for is a time.
//!
//! **Every other `Pending` in this runtime is waiting for a completion**, and
//! the park is how the thread waits for one. A `sleep`
//! ([ADR-150](../../../../docs/specification/adr/adr-150.md)) is waiting for
//! nothing at all — the clock is not something a worker posts — so the park
//! would answer *there is nothing outstanding* and the executor would spin or
//! call it a defect.
//!
//! So a sleeping future leaves the time it wants here, and the park takes the
//! nearest of them as its bound. One cell and not a queue: what the executor
//! needs is the **earliest**, every pending future is polled once a round, and
//! a future that is still pending leaves its time here again — so the cell is
//! rebuilt each round rather than kept in step with a set of registrations
//! nobody would remove from.

use std::cell::Cell;
use std::time::Instant;

thread_local! {
    /// The earliest time anything on this thread asked to be looked at again,
    /// since the executor last read it.
    static EARLIEST: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// *Poll me again no later than this.*
///
/// Called from a future's `poll`, which is always on the thread whose executor
/// will read it — so a thread-local is the whole of the bookkeeping.
pub fn wake_at(when: Instant) {
    EARLIEST.with(|earliest| {
        let soonest = match earliest.get() {
            Some(already) if already <= when => already,
            _ => when,
        };
        earliest.set(Some(soonest));
    });
}

/// The earliest time asked for since this was last called, and clear it.
///
/// **Take rather than read**, because the answer is only good for the round it
/// was collected in: a future that is still sleeping asks again on its next
/// poll, and one that has finished should not keep the executor awake.
pub fn taken() -> Option<Instant> {
    EARLIEST.with(|earliest| earliest.take())
}

/// Whether anything on this thread has asked for a time since the executor
/// last read it - an armed timer, for
/// [ADR-263](../../../../docs/specification/adr/adr-263.md) D2.
pub fn armed() -> bool {
    EARLIEST.with(|earliest| earliest.get().is_some())
}
