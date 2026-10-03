//! Files read and written by the kernel through `io_uring`, and the bell that
//! wakes a thread parked on the ring. Linux only.
//!
//! Every `unsafe` in this crate, and the argument for it, is listed in
//! `README.md`; the soundness rule for the buffers is below.
//!
//! ## Files, completed by the kernel
//! (Nikaia ADR-038 D3).
//!
//! `epoll` cannot read a file at all: a regular file is always "ready" and the
//! read blocks in the kernel anyway, which is why every readiness-based
//! runtime serves file I/O from a thread pool - and that pool *is*
//! Nikaia ADR-292's 46 µs.
//! `io_uring` removes it, not by being a faster pool but because no thread is
//! involved: the kernel performs the read and reports when it is done.
//!
//! ## The soundness rule, and how it is met
//!
//! A buffer handed to the kernel must stay alive and unmoved until the
//! completion for it arrives. The rule this file follows is one sentence:
//!
//! > **A slot, and the buffer in it, is freed only by the code that has
//! > reaped its completion.**
//!
//! Three things make that hold rather than hope for it.
//!
//! 1. **The buffers are not on any caller's stack.** A [`Job`] lives in
//!    `Ring::jobs`, and the `Ring` lives in the process-global runtime
//!    (the runtime that owns the `Ring`), which outlives every frame in the program. An unwind
//!    through `read_many` therefore cannot drop a buffer the kernel holds -
//!    there is nothing on that stack to drop.
//! 2. **A slot is released on a count, not on a return.** `Job::outstanding`
//!    is the number of submissions for that slot whose completion has not been
//!    reaped. The slot is cleared when it reaches zero and at no other moment,
//!    so an operation abandoned half-way leaves its buffer where the kernel
//!    left it.
//! 3. **The next operation reconciles before it submits.** [`Ring::reconcile`]
//!    reaps whatever an abandoned call left behind, which is what makes a slot
//!    reusable *after* its completion rather than after its caller gave up.
//!
//! What the language contributes is that there is never a borrowed buffer to
//! hand over in the first place. `fs::read` and `fs::read_to_string` return
//! **owned** values, so `std` allocates the buffer, moves it into the slot,
//! and moves the finished bytes out again - and `fs::map`, the one function
//! here that hands back memory it does not own, is a mapping and never
//! reaches the ring. Ownership is what
//! Nikaia ADR-005 and
//! Nikaia ADR-283's tether lattice
//! infer, and it is what the kernel is handed.
//!
//! ## What is *not* completed here
//!
//! `open` and `stat` are ordinary blocking syscalls. They are a path walk in
//! the kernel with no data transfer, they are what the measurement's baseline
//! pays too, and putting them on the ring would buy a syscall rather than a
//! thread. The *read* is what ADR-292 priced, and the read is what
//! completes.

#![cfg(target_os = "linux")]
#![deny(unsafe_op_in_unsafe_fn)]

use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::Arc;

use io_uring::{IoUring, opcode, types};

/// How many submissions the ring holds. Two is the pair ADR-292 is about; this
/// leaves room for a handful of jobs in flight without a resubmit.
const ENTRIES: u32 = 64;

/// **The user data the bell's poll carries**
/// (Nikaia ADR-121 D2).
///
/// Every other submission carries a slot index, and a slot index is a position
/// in [`Ring::jobs`] - so `u64::MAX` is a number no operation can wear, on a
/// machine that could not hold that many slots if it wanted to. D2 asks for the
/// bell's completion to be distinguishable from an operation's, and this is
/// how it is told apart.
const BELL: u64 = u64::MAX;

/// **The bell the ring's park is listening to**: an `eventfd`
/// (Nikaia ADR-121 D1), shared between the [`Ring`] that polls it and whoever
/// rings it.
///
/// **Rung without the ring's lock**, and that is the whole shape of it. The
/// thread that parks is inside `io_uring_enter` holding the ring's own lock,
/// and the thread that rings is an I/O worker: a worker that had to take that
/// lock to ring could not ring at all, because it would be waiting for the very
/// thread it is trying to wake. A `write` to a descriptor takes no lock, so the
/// runtime keeps a clone of the bell beside the ring and rings it directly.
#[derive(Clone, Debug)]
pub struct Bell(Arc<File>);

impl Bell {
    /// Ring it. Nothing is reported: a short write cannot happen for eight
    /// bytes, and `EAGAIN` means the counter is at its maximum, which is
    /// 2^64-2 rings nobody has read. The bell says *something moved*; it is
    /// not a queue and has nothing to lose.
    pub fn ring(&self) {
        let _ = (&*self.0).write(&1u64.to_ne_bytes());
    }

    /// Take the counter back to zero, so the next ring is a new poll. The
    /// descriptor is `EFD_NONBLOCK`, so a counter already at zero is `EAGAIN`
    /// and not a wait - and a zero counter means somebody else got here first.
    fn drain(&self) {
        let mut seen = [0u8; 8];
        let _ = (&*self.0).read(&mut seen);
    }

    fn fd(&self) -> std::os::fd::RawFd {
        self.0.as_raw_fd()
    }
}

/// How much a read grows its buffer by when the file turned out longer than
/// `stat` said - a file being appended to while it is read.
const GROW: usize = 64 * 1024;

/// One file operation, and the buffer the kernel is writing into or reading
/// out of.
///
/// The buffer is owned here and nowhere else. See the module comment for why
/// that is the whole of the soundness argument.
struct Job {
    /// Kept open for as long as the kernel may touch it, for the same reason
    /// the buffer is: a closed descriptor with a submission outstanding is the
    /// other half of the same bug.
    file: File,
    buf: Vec<u8>,
    /// How much of `buf` the kernel has filled (a read) or consumed (a write).
    at: usize,
    /// Submissions for this slot whose completion has not been reaped. The
    /// slot may be cleared when, and only when, this is zero.
    outstanding: usize,
    /// `None` while the job is running.
    outcome: Option<io::Result<()>>,
    kind: Kind,
    /// **A handle holds this slot and will come back for its answer**, so
    /// nothing else may release it.
    ///
    /// Rule 3 - *"the next operation reconciles before it submits"* - is what
    /// reclaims a slot an abandoned call left behind, and it reads "finished
    /// and nobody waiting" as "free". That was true when every caller finished
    /// its operation inside the call that started it. A future does not: it is
    /// started at its first poll and its answer is taken at a later one, and in
    /// between the slot is finished and nobody is *in* a call for it.
    ///
    /// So freeing it there dropped the buffer the kernel had already filled and
    /// the read came back as somebody else's bytes. Found by
    /// a test that compiled and ran a group of four operations, which printed
    /// `ccc leer` for `zwei vier`.
    ///
    /// [`Ring::abandon`] is what clears it, from the handle's own `Drop` - so a
    /// future dropped before it finished still gives its slot back.
    owned: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Read,
    Write,
}

/// The ring, and everything in flight on it.
///
/// One per runtime, behind the runtime's own lock. Two threads submitting into
/// one submission queue is not sound and no lock-free trick here would be
/// worth the reasoning - and at `user_parallelism = no` there is exactly one
/// user thread, so the lock is uncontended by construction
/// (Nikaia ADR-037 D2).
pub struct Ring {
    ring: IoUring,
    /// Slot index is the `user_data` the kernel reports back.
    jobs: Vec<Option<Job>>,
    /// Completions submitted and not yet reaped, over all slots.
    ///
    /// **The bell is not one of them**
    /// (Nikaia ADR-121 D2): a park
    /// with only the bell armed has nothing to wait for and still sleeps, which
    /// is what keeps D1 from turning every park into a wait for a bell nobody
    /// is going to ring.
    unreaped: usize,
    /// The bell's `eventfd` (D1). [`Bell`] says why a clone of it is also kept
    /// beside the lock.
    bell: Bell,
    /// Whether the bell's poll is on the ring. Re-armed by [`Ring::reap`] the
    /// moment it completes, which is D2's *answered by re-arming it*.
    bell_armed: bool,
    /// Every completion this ring has ever reaped, the bell's among them.
    ///
    /// What [`Ring::park`] answers off: *something moved* is a number that went
    /// up, and a bounded park that gives up with this unchanged answers `false`
    /// the way the fallback park does.
    moved: u64,
    /// Every **operation's** completion this ring has reaped - `moved` without
    /// the bell's.
    ///
    /// What a caller that shares the ring between threads needs from it
    /// (Nikaia's `rt::with_ring`): a completion reaped by one thread's
    /// [`Ring::poll_slot`] or [`Ring::park`] may be another thread's answer,
    /// and that thread cannot learn it from the ring, which it no longer
    /// holds. The caller reads this before and after each call and says
    /// *something moved* where the waiting thread will look.
    answered: u64,
}

impl Ring {
    /// A ring, or the reason there is none.
    ///
    /// **Feature detection, at run time and never at compile time.** A binary
    /// built on a machine with `io_uring` must still run on one without: an
    /// older kernel answers `ENOSYS`, a sandbox that forbids the syscalls
    /// answers `EPERM`, and both arrive here as an ordinary `Err` that the
    /// caller turns into the blocking path (D3).
    pub fn open() -> io::Result<Ring> {
        let ring = IoUring::new(ENTRIES)?;
        // SAFETY: `eventfd` takes a starting count and a flag word and hands
        // back a descriptor or `-1`. Nothing is borrowed and nothing outlives
        // the call. `EFD_NONBLOCK` is what makes [`Ring::drain_bell`] a read
        // that cannot wait, and `EFD_CLOEXEC` keeps the runtime's own
        // descriptor out of a program's children.
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut ring = Ring {
            ring,
            jobs: Vec::new(),
            unreaped: 0,
            // SAFETY: a descriptor `eventfd` just made, which nothing else in
            // this process holds or will close.
            bell: Bell(Arc::new(File::from(unsafe { OwnedFd::from_raw_fd(fd) }))),
            bell_armed: false,
            moved: 0,
            answered: 0,
        };
        ring.arm_bell();
        Ok(ring)
    }

    /// The bell, for the runtime to keep beside the lock. It belongs to
    /// **this** ring: a second ring has a bell of its own.
    pub fn bell(&self) -> Bell {
        self.bell.clone()
    }

    /// Read every one of `paths` whole, with all of them in flight at once.
    ///
    /// This is the shape ADR-292 predicted and could not measure: several
    /// operations in flight, their results collected in the order they were
    /// written, and **no thread started or woken for the pair** - the kernel
    /// does the reads and one `io_uring_enter` collects them.
    pub fn read_many(&mut self, paths: &[&Path]) -> Vec<io::Result<Vec<u8>>> {
        self.reconcile();

        // Open and size first, so that a path that does not exist fails
        // without a submission - and so that every buffer exists before
        // anything is handed to the kernel.
        let mut slots: Vec<Result<usize, io::Error>> = Vec::with_capacity(paths.len());
        for path in paths {
            slots.push(self.begin(path, Kind::Read, Vec::new()));
        }

        let mine: Vec<usize> = slots
            .iter()
            .filter_map(|s| s.as_ref().ok().copied())
            .collect();
        self.drive(&mine);

        slots
            .into_iter()
            .map(|slot| match slot {
                Err(e) => Err(e),
                Ok(slot) => self.finish(slot).map(|(buf, _)| buf),
            })
            .collect()
    }

    /// Write `bytes` to `path`, the kernel doing the transfer.
    ///
    /// `bytes` arrives by value, which is not a convenience: it is the
    /// soundness rule. There is no borrowed buffer to keep alive because there
    /// is no borrowed buffer.
    pub fn write(
        &mut self,
        path: &Path,
        bytes: Vec<u8>,
        append: bool,
        create: bool,
    ) -> io::Result<()> {
        self.reconcile();
        let slot = self.open_for_write(path, bytes, append, create)?;
        self.drive(&[slot]);
        self.finish(slot).map(|_| ())
    }

    /// Put a read on the ring and hand back its slot, **without waiting**.
    ///
    /// The slot is the handle: [`Ring::poll_slot`] asks whether it has
    /// finished, and nothing on this path blocks. That is what makes a file
    /// read a suspension point rather than a wait
    /// (Nikaia ADR-055 §6 step 3) -
    /// `read_many` above is the same operation for a caller that is going to
    /// wait for it anyway, and both are here because the ring is the same ring.
    pub fn begin_read(&mut self, path: &Path) -> io::Result<usize> {
        self.release_finished();
        let slot = self.begin(path, Kind::Read, Vec::new())?;
        self.claim(slot);
        self.submit_one(slot);
        // Handed to the kernel now rather than at the next poll: a submission
        // sitting in the queue is an operation that has not started.
        let _ = self.ring.submit();
        Ok(slot)
    }

    /// The same for a write.
    ///
    /// `bytes` arrives by value for the reason [`Ring::write`] gives: it is the
    /// soundness rule, not a convenience.
    pub fn begin_write(
        &mut self,
        path: &Path,
        bytes: Vec<u8>,
        append: bool,
        create: bool,
    ) -> io::Result<usize> {
        self.release_finished();
        let slot = self.open_for_write(path, bytes, append, create)?;
        self.claim(slot);
        self.submit_one(slot);
        let _ = self.ring.submit();
        Ok(slot)
    }

    /// Whether `slot` has finished, and what it came to if it has.
    ///
    /// **Never blocks**, which is the whole of its contract: a future polls
    /// this and returns `Pending` on a `None`.
    ///
    /// A read may take several turns, because `stat`'s answer is a hint and not
    /// a contract - so a slot whose turn finished with the file not yet ended
    /// is **resubmitted here**. A poll that submits is still a poll that does
    /// not wait.
    pub fn poll_slot(&mut self, slot: usize) -> Option<io::Result<Vec<u8>>> {
        self.reap();
        match self.jobs.get(slot).and_then(Option::as_ref) {
            // Still running, and nothing outstanding: the last turn moved bytes
            // and the file had more, so the next turn goes on the ring.
            Some(job) if job.outcome.is_none() => {
                if job.outstanding == 0 {
                    self.submit_one(slot);
                    let _ = self.ring.submit();
                }
                None
            }
            // Finished, but the kernel still holds a submission for the buffer.
            // Rule 2: the slot is released by whoever reaps its last
            // completion, so this waits for that rather than freeing it.
            Some(job) if job.outstanding > 0 => None,
            Some(_) => Some(self.finish(slot).map(|(buf, _)| buf)),
            // A slot nobody holds any more. Reported rather than waited on: a
            // `None` here would be a future that never finishes.
            None => Some(Err(io::Error::other(
                "the runtime lost a file operation's slot",
            ))),
        }
    }

    /// Wait until the kernel has posted at least one completion.
    ///
    /// The executor's park hook: it is called when no task can make progress,
    /// and it is the only place on this path that blocks. `false` means there
    /// was nothing outstanding to wait for, which tells the executor that
    /// waiting would be waiting forever.
    ///
    /// **`elsewhere` is a worker operation in flight**
    /// (Nikaia ADR-121 D1). The bell
    /// is armed on this ring, so a worker's reply *does* post a completion here
    /// and a park entered for one returns - but `unreaped` deliberately does
    /// not count the bell (D2), so without this the hook would answer *nothing
    /// to wait for* about the very operation it can now wait for. The caller
    /// knows the worker count; the ring does not.
    ///
    /// **Nothing is lost between the poll and the park.** A worker that rang
    /// before this call left the `eventfd`'s counter above zero, so the poll
    /// already on the ring is readable and `submit_and_wait` returns at once.
    /// That is the same ordering the fallback's generation gives, bought with a
    /// counter the kernel keeps instead of one this program does.
    ///
    /// `generation` is the caller's count of finished operations - read here,
    /// inside the lock, which is what keeps a ring between a poll and a park
    /// from being lost.
    pub fn park(
        &mut self,
        since: u64,
        elsewhere: bool,
        limit: Option<std::time::Duration>,
        generation: impl Fn() -> u64,
    ) -> bool {
        self.arm_bell();
        // **A bell already answered is not waited for**, which is D3's *a hang
        // is the failure this may not have* as one line of code.
        //
        // The bell coalesces - one descriptor for every worker - so a ring that
        // another thread's [`Ring::reap`] has already drained leaves nothing on
        // this ring to wait for, and a park entered after it would wait for the
        // next operation rather than for the one that finished. `since` is what
        // `generation` answered **before** the poll that found nothing to do,
        // and this is the fallback park's own ordering.
        //
        // **Inside the lock, which is the whole of why it cannot be lost.**
        // Draining the bell needs this lock, and `ring_the_bell` bumps the
        // count before it writes to the descriptor - so any ring that could
        // have been drained already shows up here as a count that has moved,
        // and any ring after this line finds the poll armed and the counter
        // above zero.
        //
        // **And it is asked before the two counts below**, which is where this
        // was wrong: an operation answered between the caller's poll and this
        // call has already left `pending`, so `elsewhere` is `false` and
        // `unreaped` is zero, and the early return said *nothing to wait for*
        // about a reply that was already in a channel. The caller then either
        // panicked about a waker nobody arranged or spun out its
        // `cleanup-deadline` and abandoned the task. The count is the thing
        // that remembers; it has to be read first.
        if generation() > since {
            self.reap();
            return true;
        }
        if self.unreaped == 0 && !elsewhere {
            return false;
        }
        let before = self.moved;
        match limit {
            None => {
                let _ = self.ring.submit_and_wait(1);
            }
            // **A bounded park gives up rather than waiting**, which is D3's
            // *a hang is the failure this may not have* on the path that used to
            // answer `false` for a worker operation and now waits for one.
            //
            // `IORING_ENTER_EXT_ARG` is a bound the kernel keeps for the length
            // of one `io_uring_enter`, so there is no timeout submission to
            // cancel afterwards and no timespec the kernel holds past the call.
            // `ETIME` is the bound expiring and anything else is a kernel that
            // has no `EXT_ARG` (before 5.11) or a submission it refused: all
            // three are *nothing moved*, which is what a caller with a deadline
            // asked to be told. Waiting unbounded instead would be the hang.
            Some(limit) => {
                let bound: types::Timespec = limit.into();
                let args = types::SubmitArgs::new().timespec(&bound);
                let _ = self.ring.submitter().submit_with_args(1, &args);
            }
        }
        self.reap();
        self.moved > before
    }

    /// How many operations' completions this ring has reaped, ever.
    ///
    /// A count and not a list: what it tells a caller sharing the ring is that
    /// a call it just made may have taken somebody else's answer off the queue,
    /// and the answer itself is in that somebody's slot.
    pub fn answered(&self) -> u64 {
        self.answered
    }

    /// Put the bell's poll on the ring, if it is not already there (D1).
    ///
    /// **A poll and not a read**, which is what keeps the module's soundness
    /// rule out of this entirely: `IORING_OP_POLL_ADD` hands the kernel a
    /// descriptor and no buffer, so there is nothing to keep alive and nothing
    /// to move. The eight bytes are taken afterwards, by an ordinary read on a
    /// descriptor the poll has just said is readable.
    ///
    /// A submission queue with no room leaves the bell unarmed and says so by
    /// leaving the flag false; the next [`Ring::park`] arms it, and until then
    /// the queue is full of work whose completions wake the park anyway.
    fn arm_bell(&mut self) {
        if self.bell_armed {
            return;
        }
        let entry = opcode::PollAdd::new(types::Fd(self.bell.fd()), libc::POLLIN as u32)
            .build()
            .user_data(BELL);
        // SAFETY: a poll submission borrows nothing. The descriptor belongs to
        // this `Ring`, which lives in the process-global runtime and is never
        // dropped, so it cannot be closed while the kernel holds the poll.
        if unsafe { self.ring.submission().push(&entry) }.is_ok() {
            self.bell_armed = true;
            let _ = self.ring.submit();
        }
    }

    /// Take the bell's counter back to zero, so the next ring is a new poll.
    fn drain_bell(&self) {
        self.bell.drain();
    }

    /// Say that a handle holds `slot` and will take its answer ([`Job::owned`]).
    fn claim(&mut self, slot: usize) {
        if let Some(job) = self.jobs.get_mut(slot).and_then(Option::as_mut) {
            job.owned = true;
        }
    }

    /// Give `slot` back without taking its answer.
    ///
    /// Called from the handle's `Drop` for a future that never finished. The
    /// slot is *not* freed here - rule 2 still holds, and the kernel may have a
    /// submission for the buffer - it only stops being somebody's, so the next
    /// [`Ring::release_finished`] may reclaim it once its completions are in.
    pub fn abandon(&mut self, slot: usize) {
        if let Some(job) = self.jobs.get_mut(slot).and_then(Option::as_mut) {
            job.owned = false;
        }
    }

    /// Release the slots whose last completion has arrived.
    ///
    /// [`Ring::reconcile`] without the wait - the half that is safe to run on a
    /// path that may not block. A slot not released here is released by the
    /// next call: what rule 2 forbids is freeing one early, never late.
    fn release_finished(&mut self) {
        self.reap();
        for slot in 0..self.jobs.len() {
            let free = matches!(
                self.jobs[slot].as_ref(),
                Some(job) if job.outstanding == 0 && job.outcome.is_some() && !job.owned
            );
            if free {
                self.jobs[slot] = None;
            }
        }
    }

    /// Open `path`, size it, and take a slot with a buffer the kernel can use.
    fn begin(&mut self, path: &Path, kind: Kind, bytes: Vec<u8>) -> io::Result<usize> {
        let file = File::open(path)?;
        let size = file.metadata()?.len() as usize;
        let buf = match kind {
            // `stat`'s answer is a hint and not a contract - `/proc` reports
            // zero and a growing file reports the old length - so the loop in
            // `drive` is what decides where the file ends. One extra byte
            // makes an exactly-`size` file take one more turn and report EOF,
            // rather than being silently truncated at the hint.
            //
            // **Capacity, not zeroes** (Nikaia ADR-263 D3): the kernel writes
            // into the spare capacity and `reap` grows the length by what it
            // wrote. Zeroing the buffer first was a `memset` of the whole file
            // the kernel then overwrote.
            Kind::Read => Vec::with_capacity(size + 1),
            Kind::Write => bytes,
        };
        Ok(self.take_slot(Job {
            file,
            buf,
            at: 0,
            outstanding: 0,
            outcome: None,
            kind,
            owned: false,
        }))
    }

    fn open_for_write(
        &mut self,
        path: &Path,
        bytes: Vec<u8>,
        append: bool,
        create: bool,
    ) -> io::Result<usize> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .append(append)
            .truncate(!append)
            .create(create)
            .open(path)?;
        Ok(self.take_slot(Job {
            file,
            buf: bytes,
            at: 0,
            outstanding: 0,
            outcome: None,
            kind: Kind::Write,
            owned: false,
        }))
    }

    /// The first slot that is free. A slot with a submission outstanding is
    /// **not** free, however long ago its caller gave up on it - that is rule
    /// 2 of the module comment, and it is the line between this being sound
    /// and being usually sound.
    fn take_slot(&mut self, job: Job) -> usize {
        if let Some(at) = self.jobs.iter().position(Option::is_none) {
            self.jobs[at] = Some(job);
            return at;
        }
        self.jobs.push(Some(job));
        self.jobs.len() - 1
    }

    /// Push one submission for `slot`, if it has more to do.
    ///
    /// Returns whether anything was submitted.
    fn submit_one(&mut self, slot: usize) -> bool {
        // **Never a second submission beside one still in flight**: it would
        // cover the same `at..len` of the same buffer - a read the kernel
        // writes twice at once, a write whose bytes reach the file twice.
        let job = match self.jobs.get_mut(slot).and_then(Option::as_mut) {
            Some(job) if job.outcome.is_none() && job.outstanding == 0 => job,
            _ => return false,
        };

        // A read's buffer is filled up to its length and offered up to its
        // capacity; when the kernel has filled all of it, it grows.
        if job.kind == Kind::Read && job.at == job.buf.capacity() {
            job.buf.reserve(GROW);
        }
        if job.kind == Kind::Write && job.at == job.buf.len() {
            job.outcome = Some(Ok(()));
            return false;
        }

        let fd = types::Fd(job.file.as_raw_fd());
        let at = job.at;
        let rest = match job.kind {
            Kind::Read => job.buf.capacity() - at,
            Kind::Write => job.buf.len() - at,
        } as u32;
        // SAFETY: the pointer is into `job.buf`, which lives in `self.jobs`
        // and is released only by `finish` once `outstanding` reaches zero -
        // so the kernel's view of it outlives every submission made for it.
        // The slice is disjoint from every other in-flight submission because
        // each slot has its own buffer and each submission covers `at..len`
        // of exactly one slot - `at..capacity` for a read, which is the
        // buffer's own allocation, and is not reallocated while a submission
        // is out (`reserve` runs only when `outstanding` is zero, above).
        let entry = unsafe {
            match job.kind {
                Kind::Read => opcode::Read::new(fd, job.buf.as_mut_ptr().add(at), rest)
                    .offset(at as u64)
                    .build()
                    .user_data(slot as u64),
                Kind::Write => opcode::Write::new(fd, job.buf.as_ptr().add(at), rest)
                    // `u64::MAX` is "wherever the descriptor is", which is
                    // what `O_APPEND` needs and what a truncating write wants
                    // after the first turn.
                    .offset(u64::MAX)
                    .build()
                    .user_data(slot as u64),
            }
        };

        loop {
            // SAFETY: `entry` names a buffer that outlives the submission, per
            // the comment above; that is the whole of this call's contract.
            if unsafe { self.ring.submission().push(&entry) }.is_ok() {
                break;
            }
            // A full submission queue: hand what is there to the kernel and
            // try again. `submit` cannot fail for lack of room.
            if let Err(e) = self.ring.submit() {
                if let Some(job) = self.jobs[slot].as_mut() {
                    job.outcome = Some(Err(e));
                }
                return false;
            }
        }
        self.jobs[slot]
            .as_mut()
            .expect("a slot just submitted for")
            .outstanding += 1;
        self.unreaped += 1;
        true
    }

    /// Run `mine` to their end, keeping all of them in flight.
    ///
    /// **Only `mine`.** A slot another caller holds - a future's read, begun
    /// and not yet polled - has a submission in flight already and is taken
    /// further by its own [`Ring::poll_slot`]; submitting it here as well was
    /// a second submission over the same bytes, and a write reached its file
    /// twice (`fs::tests::many_threads_reading_and_writing_at_once_each_get_their_own_answer`).
    fn drive(&mut self, mine: &[usize]) {
        loop {
            for slot in mine {
                self.submit_one(*slot);
            }
            let waiting = mine.iter().any(|slot| {
                self.jobs
                    .get(*slot)
                    .and_then(Option::as_ref)
                    .is_some_and(|job| job.outcome.is_none() || job.outstanding > 0)
            });
            if !waiting {
                return;
            }
            if let Err(e) = self.ring.submit_and_wait(1) {
                // Nothing was completed, so nothing may be freed. Every
                // outstanding count stays as it is and `reconcile` will find
                // them; the jobs are failed so their callers stop waiting.
                self.fail_running(e);
                return;
            }
            self.reap();
        }
    }

    /// Take every completion the kernel has posted, and account for it.
    fn reap(&mut self) {
        let mut done = Vec::new();
        for cqe in self.ring.completion() {
            done.push((cqe.user_data(), cqe.result()));
        }
        let mut rang = false;
        for (data, result) in done {
            // **The bell's completion is not an operation's**
            // (Nikaia ADR-121 D2): it
            // is answered by re-arming the poll below, it is never handed to a
            // future as a result, and it is not subtracted from `unreaped`
            // because it was never added to it.
            self.moved += 1;
            if data == BELL {
                self.bell_armed = false;
                rang = true;
                continue;
            }
            let slot = data as usize;
            self.unreaped -= 1;
            self.answered += 1;
            let Some(job) = self.jobs.get_mut(slot).and_then(Option::as_mut) else {
                // A completion for a slot nobody is waiting on: the buffer was
                // already released because `outstanding` had reached zero.
                // Nothing to do, and nothing to free.
                continue;
            };
            job.outstanding -= 1;
            match result {
                n if n < 0 => job.outcome = Some(Err(io::Error::from_raw_os_error(-n))),
                0 => {
                    // End of file for a read; a write that moved nothing is
                    // also finished, and both mean "no more turns".
                    job.buf.truncate(job.at);
                    job.outcome.get_or_insert(Ok(()));
                }
                n => {
                    job.at += n as usize;
                    match job.kind {
                        // SAFETY: the kernel reported `n` bytes written into
                        // `at..at + n` of this buffer's spare capacity, which
                        // the submission offered and no more (`rest`), so the
                        // first `at` bytes are initialised and within capacity.
                        Kind::Read => unsafe { job.buf.set_len(job.at) },
                        Kind::Write if job.at == job.buf.len() => {
                            job.outcome.get_or_insert(Ok(()));
                        }
                        Kind::Write => {}
                    }
                }
            }
        }
        // Once, after the whole batch: several rings between two reaps are one
        // readable descriptor, and one poll is what answers all of them.
        if rang {
            self.drain_bell();
            self.arm_bell();
        }
    }

    /// Mark every running job failed, without touching a single outstanding
    /// count. Used when the kernel refused the submission itself.
    fn fail_running(&mut self, error: io::Error) {
        for job in self.jobs.iter_mut().flatten() {
            if job.outcome.is_none() {
                job.outcome = Some(Err(io::Error::from(error.kind())));
            }
        }
    }

    /// The bytes, and the slot released - if and only if its completions have
    /// all arrived.
    fn finish(&mut self, slot: usize) -> io::Result<(Vec<u8>, usize)> {
        let Some(job) = self.jobs.get(slot).and_then(Option::as_ref) else {
            return Err(io::Error::other("the runtime lost a file operation's slot"));
        };
        let outcome = match &job.outcome {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(io::Error::from(e.kind())),
            None => Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "the file operation did not complete",
            )),
        };
        if job.outstanding > 0 {
            // Rule 2. The buffer stays exactly where the kernel left it, until
            // `reconcile` has seen the last completion for it - so there are no
            // bytes to hand back here, and a truncated success would be a
            // silent wrong answer. It is a failure instead.
            outcome?;
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "the file operation still has a submission with the kernel",
            ));
        }
        let job = self.jobs[slot].take().expect("checked just above");
        let at = job.at;
        outcome.map(|()| (job.buf, at))
    }

    /// Reap what an abandoned call left behind, and release the slots whose
    /// last completion has now arrived.
    ///
    /// This is what makes a slot reusable after its *completion* rather than
    /// after its caller returned, and it is why a panic between submit and
    /// reap cannot hand a freed buffer to the kernel.
    fn reconcile(&mut self) {
        if self.unreaped > 0 {
            // Whatever is already posted, taken now; and if the kernel has
            // posted nothing yet, wait for one so progress is made rather than
            // spun on.
            let _ = self.ring.submit_and_wait(1);
            self.reap();
        }
        for slot in 0..self.jobs.len() {
            // `owned`: a finished slot whose answer a future has not taken yet
            // is not free, and reading it as free is what lost a read's bytes
            // before ([`Job::owned`]).
            let free = matches!(
                self.jobs[slot].as_ref(),
                Some(job) if job.outstanding == 0 && job.outcome.is_some() && !job.owned
            );
            if free {
                self.jobs[slot] = None;
            }
        }
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    fn ring() -> Option<Ring> {
        Ring::open().ok()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nikaia-uring-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        dir.join(name)
    }

    /// The kernel reads the file, and the bytes are the file's.
    #[test]
    fn a_file_is_read_whole() {
        let Some(mut ring) = ring() else {
            return; // No ring here; `rt`'s own tests cover the fallback.
        };
        let path = scratch("whole");
        let text = "Hamburg;12.0\nBremen;9.5\n".repeat(4096);
        std::fs::write(&path, &text).expect("write");

        let read = ring.read_many(&[&path]);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].as_ref().expect("read"), text.as_bytes());
    }

    /// An empty file is an input, and `stat` reporting zero must not be read as
    /// a failure.
    #[test]
    fn an_empty_file_reads_empty() {
        let Some(mut ring) = ring() else { return };
        let path = scratch("empty");
        std::fs::write(&path, "").expect("write");
        assert_eq!(ring.read_many(&[&path])[0].as_ref().expect("read"), &[]);
    }

    /// Both reads in flight at once, and both answers in the order they were
    /// asked for - which is ADR-292's shape.
    #[test]
    fn two_reads_are_both_in_flight_and_answered_in_order() {
        let Some(mut ring) = ring() else { return };
        let (a, b) = (scratch("pair-a"), scratch("pair-b"));
        std::fs::write(&a, "eins").expect("write");
        std::fs::write(&b, "zwei").expect("write");

        let read = ring.read_many(&[&a, &b]);
        assert_eq!(read[0].as_ref().expect("a"), b"eins");
        assert_eq!(read[1].as_ref().expect("b"), b"zwei");

        // And the slots are free again, so the next pair reuses them rather
        // than growing the ring's bookkeeping without bound.
        let read = ring.read_many(&[&b, &a]);
        assert_eq!(read[0].as_ref().expect("b"), b"zwei");
        assert_eq!(read[1].as_ref().expect("a"), b"eins");
        assert_eq!(ring.jobs.len(), 2, "slots are reused, not accumulated");
    }

    /// A path that is not there fails before anything is submitted, so a
    /// failure cannot leave a buffer with the kernel.
    #[test]
    fn a_missing_path_fails_without_a_submission() {
        let Some(mut ring) = ring() else { return };
        let read = ring.read_many(&[Path::new("/nikaia/no/such/file")]);
        assert!(read[0].is_err());
        assert_eq!(ring.unreaped, 0);
        assert!(ring.jobs.iter().all(Option::is_none));
    }

    /// Write, then read back through the ring: both halves, and they agree.
    #[test]
    fn a_write_completes_and_reads_back() {
        let Some(mut ring) = ring() else { return };
        let path = scratch("written");
        let _ = std::fs::remove_file(&path);

        let bytes = "eins\n".repeat(20_000).into_bytes();
        ring.write(&path, bytes.clone(), false, true)
            .expect("write");
        assert_eq!(ring.read_many(&[&path])[0].as_ref().expect("read"), &bytes);

        ring.write(&path, b"zwei\n".to_vec(), true, true)
            .expect("append");
        let read = ring.read_many(&[&path]);
        let back = read[0].as_ref().expect("read");
        assert_eq!(back.len(), bytes.len() + 5);
        assert!(back.ends_with(b"zwei\n"), "the append went to the end");
    }

    /// Rule 2 of the module comment, exercised on the bookkeeping: a slot with
    /// a submission outstanding is not free, so its buffer cannot be handed
    /// out to another operation while the kernel may still be writing it.
    #[test]
    fn a_slot_with_a_submission_outstanding_is_never_reused() {
        let Some(mut ring) = ring() else { return };
        let path = scratch("outstanding");
        std::fs::write(&path, "x".repeat(4096)).expect("write");

        let slot = ring.begin(&path, Kind::Read, Vec::new()).expect("opened");
        assert!(ring.submit_one(slot), "submitted");
        assert_eq!(ring.jobs[slot].as_ref().expect("held").outstanding, 1);

        // A second operation while the first is outstanding takes a *different*
        // slot, whatever the first one's caller did.
        let other = ring.begin(&path, Kind::Read, Vec::new()).expect("opened");
        assert_ne!(other, slot, "the outstanding slot was handed out again");

        // And the outstanding one is only released once its completion has
        // been accounted for.
        ring.reconcile();
        ring.drive(&[slot, other]);
        assert!(ring.jobs[slot].is_none() || ring.jobs[slot].as_ref().unwrap().outstanding == 0);
    }

    /// **A slot in flight is not submitted a second time.** A future's
    /// write, begun and not yet polled, was submitted again by the next
    /// synchronous operation's `drive`, and its bytes reached the file twice.
    #[test]
    fn a_write_in_flight_is_not_submitted_again() {
        let Some(mut ring) = ring() else { return };
        let path = scratch("in-flight");
        let _ = std::fs::remove_file(&path);

        let slot = ring
            .begin_write(&path, b"once\n".to_vec(), false, true)
            .expect("begun");
        assert_eq!(ring.jobs[slot].as_ref().expect("held").outstanding, 1);
        assert!(
            !ring.submit_one(slot),
            "a second submission over the same bytes while the first is in flight"
        );
        // And what runs to its end beside it leaves it alone.
        ring.write(
            &scratch("in-flight-other"),
            b"other\n".to_vec(),
            false,
            true,
        )
        .expect("another write");
        let answer = loop {
            if let Some(answer) = ring.poll_slot(slot) {
                break answer;
            }
            ring.reconcile();
        };
        answer.expect("the first write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "once\n");
    }
}
