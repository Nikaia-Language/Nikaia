//! Message passing: a **bounded** queue between a sender and a receiver
//! ([ADR-149](../../../docs/specification/adr/adr-149.md), Part II 12.5).
//!
//! **Nothing here is a language construct.** Two values and two methods say
//! everything the page's example says, and the tuple `let` that binds them was
//! already built ([ADR-291](../../../docs/specification/adr/adr-291.md)).
//!
//! **And there is no `unbounded()`** (D5). A capacity is a promise about
//! memory, and this language makes programs say their promises; an unbounded
//! channel is a memory leak with a name and a `send` that never pauses, which
//! is the one thing a program cannot reason about under load. A program that
//! wants *unbounded* writes a large number — and has said it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

/// What the two ends share.
///
/// **A lock and two wakers**, and not one waker each per waiting value: at most
/// one end of this channel waits at a time for any one reason — the sender
/// waits for room, the receiver waits for a value — so what has to be
/// remembered is who to wake rather than a queue of who.
struct Shared<T> {
    inner: Mutex<Waiting<T>>,
    capacity: usize,
    senders: AtomicUsize,
}

struct Waiting<T> {
    queue: VecDeque<T>,
    /// Whoever is waiting for room.
    sending: Option<Waker>,
    /// Whoever is waiting for a value.
    receiving: Option<Waker>,
}

/// **The sending end.** Handing it into a `spawn` is what
/// [ADR-123](../../../docs/specification/adr/adr-123.md)'s crossing check asks
/// about, and it asks about it there rather than at `send`, because a move is
/// where a value changes threads (D4).
pub struct Sender<T> {
    shared: Arc<Shared<T>>,
}

/// **The receiving end.** `recv` hands back a `T?`, and `null` means every
/// sender is gone (D3).
pub struct Receiver<T> {
    shared: Arc<Shared<T>>,
}

/// **A channel of `capacity` values** (D1, D5).
///
/// A capacity below one is refused rather than quietly turned into something
/// else: `bounded(0)` is a rendezvous channel, which is a different promise and
/// which no page writes (§4).
pub fn bounded<T>(capacity: i64) -> (Sender<T>, Receiver<T>) {
    let room = usize::try_from(capacity).unwrap_or(0);
    assert!(
        room > 0,
        "a channel's capacity is at least one; `bounded(0)` is a rendezvous \
         channel, which this language does not have (ADR-149 §4)"
    );
    let shared = Arc::new(Shared {
        inner: Mutex::new(Waiting {
            queue: VecDeque::new(),
            sending: None,
            receiving: None,
        }),
        capacity: room,
        senders: AtomicUsize::new(1),
    });
    (
        Sender {
            shared: shared.clone(),
        },
        Receiver { shared },
    )
}

impl<T> Sender<T> {
    /// **Put a value in, waiting for room where there is none** (D2).
    ///
    /// A suspension point, which the ledger says by *not* saying `sync` — so a
    /// `sync` body cannot send on a channel and the compiler names the promise
    /// in the way, without a rule about channels. That is what a **bounded**
    /// channel is for: back-pressure is a pause, and a pause is something this
    /// language's ledger already talks about.
    pub async fn send(&self, value: T) {
        let mut value = Some(value);
        std::future::poll_fn(move |context| {
            let mut inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.queue.len() >= self.shared.capacity {
                inner.sending = Some(context.waker().clone());
                return Poll::Pending;
            }
            inner
                .queue
                .push_back(value.take().expect("a value is sent once"));
            let receiving = inner.receiving.take();
            drop(inner);
            // Outside the lock: waking may poll, and polling reaches for this
            // channel again.
            if let Some(waker) = receiving {
                waker.wake();
            }
            crate::rt::ring_the_bell();
            Poll::Ready(())
        })
        .await
    }
}

impl<T> Drop for Sender<T> {
    /// **The last sender going away is what closes the channel** (D3), and it
    /// has to wake whoever is waiting — otherwise a `recv` that will never be
    /// answered is a program that never ends.
    fn drop(&mut self) {
        if self.shared.senders.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        let receiving = self
            .shared
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .receiving
            .take();
        if let Some(waker) = receiving {
            waker.wake();
        }
        crate::rt::ring_the_bell();
    }
}

impl<T> Receiver<T> {
    /// **The next value, or `null` where every sender is gone** (D3).
    ///
    /// Not a failure: a closed channel is the ordinary end of a stream, and
    /// `??` is what reads it (Part I 3.5).
    pub async fn recv(&self) -> Option<T> {
        std::future::poll_fn(move |context| {
            let mut inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(value) = inner.queue.pop_front() {
                let sending = inner.sending.take();
                drop(inner);
                if let Some(waker) = sending {
                    waker.wake();
                }
                crate::rt::ring_the_bell();
                return Poll::Ready(Some(value));
            }
            // **The queue is read before the sender count**, and that ordering
            // is the correctness: a sender that pushed a value and then went
            // away has already filled the queue, so asking *"is anyone left?"*
            // first would answer **no** about a value that is sitting right
            // there. The same sentence `rt::io::park_for` writes above its own
            // read, one layer up.
            if self.shared.senders.load(Ordering::Acquire) == 0 {
                return Poll::Ready(None);
            }
            inner.receiving = Some(context.waker().clone());
            Poll::Pending
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive<T>(future: impl std::future::Future<Output = T>) -> T {
        crate::rt::exec::block_on(future)
    }

    #[test]
    fn a_value_sent_is_the_value_received() {
        let _runtime = crate::rt::start(crate::rt::UserCode::Sequential);
        drive(async {
            let (tx, rx) = bounded::<i64>(4);
            tx.send(7).await;
            assert_eq!(rx.recv().await, Some(7));
        });
    }

    #[test]
    fn a_channel_with_no_senders_left_hands_back_nothing() {
        let _runtime = crate::rt::start(crate::rt::UserCode::Sequential);
        drive(async {
            let (tx, rx) = bounded::<i64>(4);
            tx.send(1).await;
            drop(tx);
            assert_eq!(rx.recv().await, Some(1));
            assert_eq!(rx.recv().await, None);
        });
    }

    /// The order a queue keeps is the order it was given.
    #[test]
    fn the_order_is_the_order_it_was_sent_in() {
        let _runtime = crate::rt::start(crate::rt::UserCode::Sequential);
        drive(async {
            let (tx, rx) = bounded::<i64>(4);
            for n in 1..=3 {
                tx.send(n).await;
            }
            drop(tx);
            let mut seen = Vec::new();
            while let Some(n) = rx.recv().await {
                seen.push(n);
            }
            assert_eq!(seen, vec![1, 2, 3]);
        });
    }

    #[test]
    #[should_panic(expected = "at least one")]
    fn a_capacity_of_nothing_is_refused() {
        let _ = bounded::<i64>(0);
    }
}
