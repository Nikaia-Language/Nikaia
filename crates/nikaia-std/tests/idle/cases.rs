//! The cases [ADR-263](../../../../docs/specification/adr/adr-263.md) D1 and D2
//! decide, asked of one runtime. `idle_at_no.rs` and `idle_at_yes.rs` each
//! start the runtime one way and run all of them.
//!
//! **One `#[test]` per binary, on purpose.** D2 asks the whole process whether
//! anything is in flight, so a second test running beside these on another
//! thread would make `main` look busy and the cases that expect the calling
//! thread would fail for a reason that is not theirs.

use nikaia_std::rt::exec;
use nikaia_std::rt::io::{self, InFlight};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the operation was made on the calling thread: finished before it
/// was ever polled.
fn here(operation: &InFlight) -> bool {
    matches!(operation, InFlight::Done(_))
}

/// Wait an operation out, so that the next case starts with nothing in flight.
fn finish(mut operation: InFlight) -> std::io::Result<Vec<u8>> {
    loop {
        let since = io::generation();
        if let Some(done) = io::poll(&mut operation) {
            return done;
        }
        io::park(since);
    }
}

/// A future that stays pending until `done` is set, and asks to be polled
/// again every round - a task that is alive while `main` reads.
async fn until(done: Arc<AtomicBool>) {
    std::future::poll_fn(move |context| {
        if done.load(Ordering::Acquire) {
            return std::task::Poll::Ready(());
        }
        context.waker().wake_by_ref();
        std::task::Poll::Pending
    })
    .await
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nikaia-idle-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

pub fn run(name: &str, start_on_pool: bool) {
    let dir = scratch(name);
    let text = dir.join("text");
    std::fs::write(&text, "Hamburg;12.0\n").expect("write");
    let text: &'static Path = Box::leak(text.into_boxed_path());

    // **`main` alone: a regular file is read on the calling thread**, and it
    // reads what the runtime would have read.
    let read = exec::block_on(async { io::begin_read(text) });
    assert!(here(&read), "`main` alone read through the runtime");
    assert_eq!(finish(read).expect("read"), b"Hamburg;12.0\n");

    // And written there, on the path that has a ring to skip; the fallback
    // writes on the calling thread anyway.
    let written = dir.join("written");
    let wrote = exec::block_on(async { io::begin_write(&written, b"Kiel;8.5\n", false, true) });
    assert!(here(&wrote), "`main` alone wrote through the runtime");
    finish(wrote).expect("write");
    assert_eq!(std::fs::read(&written).expect("read back"), b"Kiel;8.5\n");

    // **A file that is not a regular one goes through the runtime** (D1).
    #[cfg(target_os = "linux")]
    {
        let device = exec::block_on(async { io::begin_read(Path::new("/dev/null")) });
        assert!(!here(&device), "a device was read on the calling thread");
        assert_eq!(finish(device).expect("read"), b"");
    }

    // **Outside `main`**: no executor is polling anything, so nothing says the
    // caller is alone.
    let outside = io::begin_read(text);
    assert!(
        !here(&outside),
        "a read outside `block_on` took the calling thread"
    );
    finish(outside).expect("read");

    // **Both branches of an `overlap` stay in flight together** - the case D2
    // as written missed: the first branch finds nothing in flight, because the
    // second has not started yet.
    let (a, b) = exec::block_on(async {
        nikaia_std::task::overlap2(async { io::begin_read(text) }, async {
            io::begin_read(text)
        })
        .await
    });
    assert!(
        !here(&a) && !here(&b),
        "an overlap's branches were read one after the other"
    );
    finish(a).expect("read");
    finish(b).expect("read");

    // **A `select` arm** is the same question.
    let raced = exec::block_on(async {
        match nikaia_std::task::race2(async { io::begin_read(text) }, std::future::pending::<()>())
            .await
        {
            nikaia_std::task::Race2::First(read) => read,
            nikaia_std::task::Race2::Second(()) => unreachable!("never ready"),
        }
    });
    assert!(
        !here(&raced),
        "a select's arm was read on the calling thread"
    );
    finish(raced).expect("read");

    // **A task alive beside `main`**, on this thread's queue at `no` and on the
    // pool at `yes`: `main`'s read goes through the runtime, and so does the
    // task's own - a task is never `main`.
    let done = Arc::new(AtomicBool::new(false));
    let (main_alone, task_alone, read) = exec::block_on({
        let done = done.clone();
        async move {
            let waiting = until(done.clone());
            // The task's operation is dropped unanswered, which gives its slot
            // back: what is asserted is where it was made.
            let reading = async move { here(&io::begin_read(text)) };
            let (waiter, reader) = if start_on_pool {
                (
                    nikaia_std::task::TaskHandle::start_on_pool(waiting),
                    nikaia_std::task::TaskHandle::start_on_pool(reading),
                )
            } else {
                (
                    nikaia_std::task::TaskHandle::start(waiting),
                    nikaia_std::task::TaskHandle::start(reading),
                )
            };
            let read = io::begin_read(text);
            let main_alone = here(&read);
            let task_alone = reader.join().await.expect("the reader did not crash");
            done.store(true, Ordering::Release);
            waiter.join().await.expect("the waiter did not crash");
            (main_alone, task_alone, read)
        }
    });
    finish(read).expect("read");
    assert!(
        !main_alone,
        "`main` read on the calling thread while a task was alive"
    );
    assert!(!task_alone, "a task read on the calling thread");

    let _ = std::fs::remove_dir_all(&dir);
}
