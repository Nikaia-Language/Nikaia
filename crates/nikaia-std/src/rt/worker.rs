//! The I/O threads, and the boundary that keeps user code off them.
//!
//! [ADR-303](../../../../docs/specification/adr/adr-303.md) D4 starts one of
//! these before the first statement the user wrote, so that an operation costs
//! no thread wake-up. [ADR-037](../../../../docs/specification/adr/adr-037.md)
//! D2 says whose thread it is: *the compiler's*, not the user's - which is the
//! only reason it may exist at `user_parallelism = no` at all.
//!
//! ## Where that boundary is, in the code
//!
//! It is [`Op`]. A worker's inbox is a channel of `Op`, `Op` is a **closed
//! enum private to `crate::rt`**, and no variant of it carries a closure, a
//! `Box<dyn FnOnce>` or anything else a program could put its own code into.
//! So "nothing the user wrote runs on the I/O thread" is not a convention that
//! review has to keep: outside this module there is no way to name a value the
//! channel accepts.
//!
//! The other direction is checked at run time, cheaply: [`on_io_worker`] is
//! true only inside a worker's loop, and every public entry point of
//! [`crate::rt::io`] asserts it is false. A `std` function that ended up being
//! called from a worker would be a `std` bug, and it says so rather than
//! deadlocking.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// What a worker may be asked to do.
///
/// Not `pub`: see the module comment. This enum *is* the user-code boundary,
/// and it stays a closed list of operations `std` performs on the program's
/// behalf.
///
/// **Readiness used to be one of these and is not any more** (0.0.165): a
/// worker **blocked** in the poller for the whole of a wait, and `io-workers`
/// is `1` by default, so a wait that had not answered blocked every other wait
/// in the process. It is a registration on one shared poller now
/// ([`super::readiness`]), and nothing here occupies a thread while it waits
/// for something that has not happened.
pub(super) enum Op {
    /// A whole file, read on the worker thread with ordinary blocking calls -
    /// D3's fallback, for the machine that has no completion queue.
    Read {
        path: PathBuf,
        reply: Sender<io::Result<Vec<u8>>>,
    },
    /// All of standard input, read on the worker thread
    /// ([ADR-121](../../../../docs/specification/adr/adr-121.md) D4).
    ///
    /// **Not a second read shape**, which is that decision's own words: a stream
    /// has no size to `stat` and nothing to put on the ring, so this is the
    /// blocking read standard input always had - moved off the thread the
    /// program runs on, now that a worker's reply can wake a park on either
    /// mechanism. The lock `StdinLock` takes is the process's, which is what
    /// makes one reader at a time true here as it is anywhere else.
    Stdin { reply: Sender<io::Result<Vec<u8>>> },
    /// **The next chunk of standard input**
    /// ([ADR-172](../../../../docs/specification/adr/adr-172.md) D4), for a
    /// stream a program walks a line at a time.
    ///
    /// **A chunk and not a line**, which is the whole of why `io::lines` can
    /// pause without becoming slower than the blocking reader it replaces: a
    /// hop to a worker and back costs a wake-up, and a program that reads a
    /// million lines would pay a million of them. The caller finds its line
    /// endings in what comes back and asks again when the buffer runs out, so
    /// the hops are one per buffer.
    ///
    /// An empty answer is the end of the stream, which is what a read of zero
    /// bytes means everywhere.
    StdinChunk {
        want: usize,
        reply: Sender<io::Result<Vec<u8>>>,
    },
    /// Stop after everything already queued. What shutdown sends (ADR-297 D6).
    Stop,
}

/// Which way a socket has to be ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interest {
    /// There is something to read, or the peer has gone.
    Readable,
    /// A write will not block.
    Writable,
}

thread_local! {
    /// Whether this thread is one of the runtime's I/O workers.
    static IS_IO_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the calling thread is an I/O worker.
///
/// The assertion behind ADR-037 D2's line: the I/O thread is the compiler's,
/// so `std`'s user-facing functions are never *on* it.
pub fn on_io_worker() -> bool {
    IS_IO_WORKER.with(std::cell::Cell::get)
}

/// The worker threads, and the queue they share.
pub(super) struct Workers {
    /// `mpsc::Sender` is `Send` but not `Sync`, and the runtime is a
    /// `&'static` every thread reaches - so the handle is behind a lock. At
    /// `user_parallelism = no` there is one user thread and it is uncontended
    /// by construction (ADR-037 D2).
    inbox: Mutex<Sender<Op>>,
    /// Operations queued and not yet answered. Shutdown's deadline watches
    /// this, and it is what makes the drain bounded rather than hopeful
    /// (ADR-297 D6).
    pending: Arc<AtomicUsize>,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl Workers {
    /// Start `count` of them. At least one, always (D4).
    pub(super) fn start(count: usize) -> Workers {
        let (inbox, outbox) = channel::<Op>();
        let outbox = Arc::new(Mutex::new(outbox));
        let pending = Arc::new(AtomicUsize::new(0));

        let mut threads = Vec::with_capacity(count.max(1));
        for n in 0..count.max(1) {
            let outbox = Arc::clone(&outbox);
            let pending = Arc::clone(&pending);
            let thread = std::thread::Builder::new()
                .name(format!("nikaia-io-{n}"))
                .spawn(move || run(&outbox, &pending))
                .expect("the runtime's I/O thread starts");
            threads.push(thread);
        }

        Workers {
            inbox: Mutex::new(inbox),
            pending,
            threads: Mutex::new(threads),
        }
    }

    /// Queue one operation. `false` if the workers have already stopped, which
    /// is what a caller after shutdown sees.
    pub(super) fn send(&self, op: Op) -> bool {
        self.pending.fetch_add(1, Ordering::SeqCst);
        let inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        if inbox.send(op).is_err() {
            self.pending.fetch_sub(1, Ordering::SeqCst);
            return false;
        }
        true
    }

    /// **Work that waits on something outside this process, on a thread of
    /// its own** ([ADR-243](../../../../docs/specification/adr/adr-243.md) D3).
    ///
    /// A child process runs for as long as it runs, and an I/O worker that
    /// waited on one would hold up every read queued behind it - `io-workers`
    /// is `1` by default, which is the ceiling
    /// [ADR-303](../../../../docs/specification/adr/adr-303.md) took away from
    /// readiness waits. So the wait gets a thread, and everything a worker's
    /// reply does after the operation this does too: the reply goes in its
    /// channel, **both bells** ring, and the operation counts in `pending` from
    /// before the thread starts until after the bell, so a caller never sees an
    /// answered operation as one nobody is working on.
    pub(super) fn beside<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<Receiver<T>> {
        let (reply, answer) = channel();
        self.pending.fetch_add(1, Ordering::SeqCst);
        let pending = Arc::clone(&self.pending);
        let started = std::thread::Builder::new()
            .name("nikaia-child".to_string())
            .spawn(move || {
                let _ = reply.send(work());
                super::ring_the_bell();
                pending.fetch_sub(1, Ordering::SeqCst);
            });
        match started {
            Ok(_) => Some(answer),
            Err(_) => {
                self.pending.fetch_sub(1, Ordering::SeqCst);
                None
            }
        }
    }

    pub(super) fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    /// Drain, bounded by `deadline`, then stop (ADR-297 D6).
    ///
    /// Returns how many operations had not finished when the deadline expired,
    /// which is what the warning names. This cannot hang: the timer is the
    /// caller's own clock and does not depend on any operation making
    /// progress.
    pub(super) fn drain(&self, deadline: std::time::Duration) -> usize {
        let until = std::time::Instant::now() + deadline;
        while self.pending() > 0 && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let left = self.pending();

        let threads = {
            let mut held = self.threads.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *held)
        };
        {
            let inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
            for _ in 0..threads.len() {
                self.pending.fetch_add(1, Ordering::SeqCst);
                let _ = inbox.send(Op::Stop);
            }
        }
        if left == 0 {
            for thread in threads {
                let _ = thread.join();
            }
        }
        // With work still outstanding the threads are left to the process's
        // exit rather than joined: joining them is exactly the wait the
        // deadline has already expired on.
        left
    }
}

/// One worker's whole life.
fn run(outbox: &Mutex<Receiver<Op>>, pending: &AtomicUsize) {
    IS_IO_WORKER.with(|flag| flag.set(true));
    loop {
        let op = {
            let held = match outbox.lock() {
                Ok(held) => held,
                Err(poisoned) => poisoned.into_inner(),
            };
            held.recv()
        };
        let Ok(op) = op else { return };
        if matches!(op, Op::Stop) {
            pending.fetch_sub(1, Ordering::SeqCst);
            return;
        }
        perform(op);
        // **The bell first and the count second**, which is the opposite of
        // what this did and the ordering the caller needs.
        //
        // The reply is in its channel by now; the bell says *an answer landed*
        // and `pending` says *something is still outstanding*. A caller that
        // finds neither concludes **nothing can move** — `exec::block_on`
        // panics about a waker nobody arranged, or spins out its
        // `cleanup-deadline` and abandons the task. Dropping the count first
        // opened exactly that window: for the few instructions between the two
        // writes, an operation that had already answered was invisible in both.
        //
        // The other order's argument was that a woken caller should see the
        // finished count rather than the one before it. It costs nothing to be
        // wrong that way round: the caller re-polls, finds the reply, and a
        // `pending` that is briefly one too high only ever makes it **wait**
        // instead of declaring the runtime stuck. Measured before this:
        // `a_future_fed_from_a_worker_finishes_under_block_on` went red about
        // two runs in five of its binary.
        super::ring_the_bell();
        pending.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What a worker actually does. Every arm is `std`'s own code, which is the
/// whole point of `Op` being closed.
fn perform(op: Op) {
    match op {
        Op::Read { path, reply } => {
            let _ = reply.send(std::fs::read(&path));
        }
        Op::Stdin { reply } => {
            use std::io::Read;

            let mut bytes = Vec::new();
            let read = std::io::stdin()
                .lock()
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = reply.send(read);
        }
        Op::StdinChunk { want, reply } => {
            use std::io::Read;

            // **One read and not a loop to fill the buffer.** Short is not the
            // end of a stream, and a caller that asked for a chunk wants
            // whatever is there now - a pipe that produces a line a second is
            // a program that prints a line a second, and filling 64 KiB first
            // would make it a program that prints nothing for eighteen hours.
            let mut bytes = vec![0_u8; want];
            let read = std::io::stdin().lock().read(&mut bytes).map(|n| {
                bytes.truncate(n);
                bytes
            });
            let _ = reply.send(read);
        }
        Op::Stop => {}
    }
}

/// The blocking write: D3's fallback for a file that leaves the program.
///
/// It runs on the **calling** thread, and there is no `Op` for it. A caller
/// that is going to wait for its own write gains nothing from handing it to
/// another thread: it pays a wake-up and buys no overlap. What would earn the
/// worker is a *batch* of writes, the way [`Op::Read`] earns it for a batch of
/// reads - and no lowering asks for one, so the variant that would carry it is
/// not written until something needs it.
pub(super) fn blocking_write(
    path: &std::path::Path,
    bytes: &[u8],
    append: bool,
    create: bool,
) -> io::Result<()> {
    use std::io::Write;

    if !append && create {
        return std::fs::write(path, bytes);
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .append(append)
        .truncate(!append)
        .create(create)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boundary, from the only side a test can see it from: the calling
    /// thread is never an I/O worker, and a worker's own loop always is.
    #[test]
    fn the_calling_thread_is_never_an_io_worker() {
        assert!(!on_io_worker());
        let workers = Workers::start(1);

        // A worker answers, so it ran; and the answer arrived on this thread,
        // which is not one.
        let (reply, answer) = channel();
        let path = std::env::temp_dir().join(format!("nikaia-worker-{}", std::process::id()));
        std::fs::write(&path, "eins").expect("write");
        assert!(workers.send(Op::Read {
            path: path.clone(),
            reply
        }));
        assert_eq!(answer.recv().expect("answered").expect("read"), b"eins");
        assert!(!on_io_worker());

        assert_eq!(workers.drain(std::time::Duration::from_secs(5)), 0);
        std::fs::remove_file(&path).ok();
    }

    /// The drain is bounded by the clock and not by the work, which is the
    /// half of ADR-297 D6 that cannot hang.
    ///
    /// **A read of a FIFO nobody writes to**, which used to be a readiness wait
    /// on a socket nobody writes to. Readiness is not a worker operation any
    /// more ([`super::readiness`]) and this test is about the **drain**, so
    /// what it needs is any operation that does not finish — and opening a FIFO
    /// blocks until a writer appears, which is a property of the thing rather
    /// than a sleep this had to choose a length for.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_drain_is_bounded_by_its_deadline() {
        let workers = Workers::start(1);
        let (reply, answer) = channel();
        let path = std::env::temp_dir().join(format!("nikaia-drain-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let made = std::process::Command::new("mkfifo")
            .arg("-m")
            .arg("600")
            .arg(&path)
            .status()
            .expect("mkfifo runs");
        assert!(made.success(), "mkfifo");
        assert!(workers.send(Op::Read {
            path: path.clone(),
            reply,
        }));

        let began = std::time::Instant::now();
        let left = workers.drain(std::time::Duration::from_millis(50));
        assert!(began.elapsed() < std::time::Duration::from_secs(2), "hung");
        assert_eq!(left, 1, "the deadline expired with the operation pending");
        drop(answer);
        // The worker is still blocked in `open`; a writer lets it go, and the
        // entry is this test's to remove.
        let _ = std::fs::OpenOptions::new().write(true).open(&path);
        let _ = std::fs::remove_file(&path);
    }
}
