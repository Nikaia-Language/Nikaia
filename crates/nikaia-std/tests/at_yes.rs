//! **`user_parallelism = yes`, in a process of its own.**
//!
//! Its own test binary and not a `mod tests`, because the runtime is **one per
//! process** ([ADR-303](../../../docs/specification/adr/adr-303.md) D4): the
//! first thing to ask for it decides what it is, and a unit test that ran after
//! one asking for `Sequential` would find no pool and quietly measure the
//! single-threaded executor instead. What is asserted here is a wall-clock
//! difference, so a silent fallback would read as a failure of the pool rather
//! than of the setup.

/// **The shape a generated `main` has at `user_parallelism = yes`**, end to
/// end: the runtime is started as `Concurrent`, `block_on` drives `main` on
/// this thread, and the tasks are on the pool
/// ([ADR-055](../../../docs/specification/adr/adr-055.md) §6 step 1's `yes`
/// half).
///
/// Four tasks that each hold a core for a slice finish in about one slice
/// rather than four. And `main` is *joining* them, so it is parked while they
/// run — which is the piece the one-thread executor never needed: at `no`
/// nothing else can fill a slot while `main` is parked, and here everything
/// does.
#[test]
fn four_tasks_joined_from_main_run_at_the_same_time() {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    if threads < 4 {
        // A machine with fewer cores than tasks cannot show the difference, and
        // asserting it there would be asserting something about the machine.
        return;
    }
    fn spin(until: std::time::Instant) -> i64 {
        let mut n = 0i64;
        while std::time::Instant::now() < until {
            n = n.wrapping_add(1);
            std::hint::spin_loop();
        }
        n.min(1)
    }

    let _runtime = nikaia_std::rt::start(nikaia_std::rt::UserCode::Concurrent);
    assert!(
        nikaia_std::rt::handle().user_pool().is_some(),
        "the pool is what this measures: {}",
        nikaia_std::rt::handle().describe()
    );

    let slice = std::time::Duration::from_millis(400);
    let began = std::time::Instant::now();
    let total = nikaia_std::rt::exec::block_on(async move {
        let a = nikaia_std::task::TaskHandle::start_on_pool(async move {
            spin(std::time::Instant::now() + slice)
        });
        let b = nikaia_std::task::TaskHandle::start_on_pool(async move {
            spin(std::time::Instant::now() + slice)
        });
        let c = nikaia_std::task::TaskHandle::start_on_pool(async move {
            spin(std::time::Instant::now() + slice)
        });
        let d = nikaia_std::task::TaskHandle::start_on_pool(async move {
            spin(std::time::Instant::now() + slice)
        });
        [
            a.join().await,
            b.join().await,
            c.join().await,
            d.join().await,
        ]
        .into_iter()
        .map(|n| n.expect("no task crashed"))
        .sum::<i64>()
    });
    let took = began.elapsed();
    assert_eq!(total, 4, "every task's value came back through its slot");
    assert!(
        took < slice * 3,
        "four {slice:?} tasks joined from `main` took {took:?}: they ran in turn"
    );
}

/// **A task nobody joins still runs, on the pool too** (ADR-055 D5).
///
/// The `no` executor holds `main`'s value until its queue empties, which is
/// what makes that sentence true there. At `yes` the tasks are on another
/// queue entirely, so the drain has to ask the pool as well — and this is the
/// program that says whether it does: nothing joins the task, and its effect
/// has to have happened by the time `block_on` returns.
#[test]
fn a_task_nobody_joins_still_runs_on_the_pool() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let _runtime = nikaia_std::rt::start(nikaia_std::rt::UserCode::Concurrent);
    let ran = Arc::new(AtomicBool::new(false));
    let mine = ran.clone();
    nikaia_std::rt::exec::block_on(async move {
        let _ = nikaia_std::task::TaskHandle::start_on_pool(async move {
            mine.store(true, Ordering::Release);
        });
    });
    assert!(
        ran.load(Ordering::Acquire),
        "the drain waited for a task nobody was holding"
    );
}
