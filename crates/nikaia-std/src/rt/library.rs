//! **The threads a library's `_async` calls run on**
//! ([ADR-284](../../../../docs/specification/adr/adr-284.md) D9): kept and
//! handed the next call rather than started and ended for each one.
//!
//! A thread that has finished a call waits for the next; one that has waited
//! [`IDLE`] for nothing ends. No call waits for a thread: where none is idle, a
//! new one starts - a call may pause for as long as its work does, and a pool
//! of fixed size would make the next call wait for it.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// One call, as the library hands it over.
pub type Job = Box<dyn FnOnce() + Send>;

/// How long a thread with nothing to do waits for something before it ends.
pub const IDLE: Duration = Duration::from_secs(60);

/// The threads waiting for a call, each by a number of its own.
fn idle() -> &'static Mutex<Vec<(u64, Sender<Job>)>> {
    static IDLE_THREADS: OnceLock<Mutex<Vec<(u64, Sender<Job>)>>> = OnceLock::new();
    IDLE_THREADS.get_or_init(|| Mutex::new(Vec::new()))
}

/// **Run `job` on a library thread**: an idle one, or a new one.
pub fn run(job: Job) {
    let mut job = job;
    loop {
        let taken = idle().lock().unwrap_or_else(|held| held.into_inner()).pop();
        let Some((_, waiting)) = taken else {
            break;
        };
        match waiting.send(job) {
            Ok(()) => return,
            // It ended between being listed and being asked: the next one.
            Err(returned) => job = returned.0,
        }
    }
    start(job);
}

fn start(first: Job) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    let (handed, waits): (Sender<Job>, Receiver<Job>) = channel();
    std::thread::Builder::new()
        .name("nikaia-library".to_string())
        .spawn(move || {
            first();
            loop {
                idle()
                    .lock()
                    .unwrap_or_else(|held| held.into_inner())
                    .push((number, handed.clone()));
                match waits.recv_timeout(IDLE) {
                    Ok(job) => job(),
                    Err(RecvTimeoutError::Timeout) => {
                        let mut list = idle().lock().unwrap_or_else(|held| held.into_inner());
                        match list.iter().position(|(listed, _)| *listed == number) {
                            // Nobody took it: it ends, listed nowhere.
                            Some(at) => {
                                list.remove(at);
                                return;
                            }
                            // Taken while it gave up: a call is on its way.
                            None => {
                                drop(list);
                                if let Ok(job) = waits.recv() {
                                    job();
                                }
                            }
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .expect("a library thread starts");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A thread is used again**: two calls one after the other run on one
    /// thread, and two at once on two.
    #[test]
    fn a_thread_is_kept_for_the_next_call() {
        let (said, heard) = channel();
        let first = said.clone();
        run(Box::new(move || {
            first.send(std::thread::current().id()).unwrap()
        }));
        let one = heard.recv().unwrap();
        // Until the thread has listed itself as idle again.
        std::thread::sleep(Duration::from_millis(50));
        let second = said.clone();
        run(Box::new(move || {
            second.send(std::thread::current().id()).unwrap()
        }));
        assert_eq!(heard.recv().unwrap(), one, "the same thread");

        let (gate, opened) = channel::<()>();
        let blocked = said.clone();
        run(Box::new(move || {
            blocked.send(std::thread::current().id()).unwrap();
            let _ = opened.recv();
        }));
        let busy = heard.recv().unwrap();
        let other = said.clone();
        run(Box::new(move || {
            other.send(std::thread::current().id()).unwrap()
        }));
        assert_ne!(
            heard.recv().unwrap(),
            busy,
            "a busy thread is not waited for"
        );
        drop(gate);
    }
}
