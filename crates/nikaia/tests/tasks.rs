//! **`spawn`, and what a task means** — Part I 8.2 and 8.3,
//! [ADR-055](../../../docs/specification/adr/adr-055.md) D5.
//!
//! This is what the whole of ADR-055 was written for. `spawn` had no runtime
//! binding because nobody had asked what a task *means* at
//! `user_parallelism = no`: Part II 11.2 says *"interleaved on the same
//! thread"*, and two synchronous Rust closures cannot interleave because
//! neither yields. With the executor (§6 step 1), `async fn` off the ledger
//! (step 2) and `std` suspending for real (step 3), they can — and these are
//! the tests that say so about programs rather than about `std`'s own futures.
//!
//! What a program with tasks computes is in `tests/language/src/tasks.nika`;
//! what stays here needs the order of a program's output, files, a runtime
//! configuration, a build switch, the checker or the emitted Rust.

mod common;

use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit;
use nikaia::parser::parse_to_ast;
use std::process::Command;

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check(&parsed, &own, &library).findings
}

fn lower(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit::emit_program(&parsed, Default::default())
        .expect("the source lowers")
        .rust
}

/// Lower at a given `user_parallelism`, which is the one build switch a `spawn`
/// reads.
fn lower_at(source: &str, user_parallelism: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let build =
        emit::Build::parse("x86_64-linux", user_parallelism, "yes").expect("a known switch");
    emit::emit_program(&parsed, build)
        .expect("the source lowers")
        .rust
}

/// Lower, compile and run, and hand back what it printed.
fn run(purpose: &str, source: &str) -> String {
    let dir = common::scratch_dir(purpose);
    let rust = lower(source);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "a task's lowering does not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run it");
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let out = String::from_utf8_lossy(&ran.stdout).to_string();
    std::fs::remove_dir_all(&dir).ok();
    out
}

/// **Part I 8.2's own example, which keeps no handle.**
///
/// *"A task nobody joins still runs"* (D5). Returning from `block_on` the
/// moment `main` was ready would have made that sentence false — the task
/// would be a future in a queue nobody polls again — so `main`'s value waits
/// until the queue is empty. This is the test that says it does.
#[test]
fn a_task_nobody_joins_still_runs() {
    let printed = run(
        "tasks-unjoined",
        "fn main() {\n\
         \x20   spawn fn { println(\"I am running in the background!\") }\n\
         \x20   println(\"main is done\")\n\
         }",
    );
    assert!(
        printed.contains("I am running in the background!"),
        "{printed}"
    );
    // And `main` got there first, which is what "in the background" means: the
    // task is started, not called.
    let main_at = printed.find("main is done").expect("main printed");
    let task_at = printed.find("I am running").expect("the task printed");
    assert!(main_at < task_at, "{printed}");
}

/// `.join()` is an `.await` (D5); that it hands back the body's value is in
/// `tests/language/src/tasks.nika`.
#[test]
fn a_join_is_an_await() {
    let source = "fn work(n: i64) -> i64 {\n\
         \x20   return n * 2\n\
         }\n\
         \n\
         fn main() throws {\n\
         \x20   let handle = spawn fn { work(21) }\n\
         \x20   println(\"started\")\n\
         \x20   let answer = handle.join()\n\
         \x20   println(f\"{answer}\")\n\
         }";

    // The `.await` is the ledger's answer about `TaskHandle::join`, which has no
    // `sync` line - the same rule every other pausing call gets (ADR-055 D2).
    let rust = lower(source);
    assert!(rust.contains(".join().await"), "{rust}");
    assert!(
        rust.contains("nikaia_std::task::TaskHandle::start(async move"),
        "{rust}"
    );
}

/// **Two tasks, both in flight, on one thread** — Part II 11.2's sentence about
/// something a program writes.
///
/// Each task reads a file, which is a real suspension point since §6 step 3. The
/// claim is in the order: `main` prints before either read finishes, because
/// `spawn` starts a task rather than running it.
#[test]
fn two_tasks_are_in_flight_before_either_finishes() {
    let dir = common::scratch_dir("tasks-in-flight");
    std::fs::write(dir.join("eins.txt"), "eins").expect("write");
    std::fs::write(dir.join("zwei.txt"), "zweizwei").expect("write");

    let source = "use std::fs\n\
         \n\
         fn size(path: ref String) -> i64 {\n\
         \x20   let text = fs::read_to_string(path, fs::Root::Anywhere) catch { return 0 }\n\
         \x20   println(f\"read {path}\")\n\
         \x20   return text.len()\n\
         }\n\
         \n\
         fn main() throws {\n\
         \x20   let first = spawn fn { size(\"eins.txt\") }\n\
         \x20   let second = spawn fn { size(\"zwei.txt\") }\n\
         \x20   println(\"both started\")\n\
         \x20   let a = first.join()\n\
         \x20   let b = second.join()\n\
         \x20   println(f\"{a} {b}\")\n\
         }";

    let rust = lower(source);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run it");
    let printed = String::from_utf8_lossy(&ran.stdout);

    let started = printed.find("both started").expect("main printed");
    let first_read = printed.find("read eins.txt").expect("the first read");
    assert!(
        started < first_read,
        "a task ran at the `spawn` rather than being started:\n{printed}"
    );
    assert!(printed.trim_end().ends_with("4 8"), "{printed}");

    std::fs::remove_dir_all(&dir).ok();
}

/// **`NK2101`: data a task took with it, used again afterwards** (Part I 8.3).
///
/// The message this replaces is `rustc`'s — *"borrow of moved value:
/// `message`"*, with *"consider cloning the value before moving it into the
/// closure"* — a closure the program does not have, about a file nobody wrote
/// (Part III, C.1).
#[test]
fn data_a_task_took_and_the_program_used_again_is_refused() {
    let found = findings(
        "fn main() {\n\
         \x20   let message: String = \"Hello\"\n\
         \x20   spawn fn { println(message) }\n\
         \x20   println(message)\n\
         }",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "NK2101");
    assert!(
        found[0]
            .message
            .contains("You're using `message` after a background task took it"),
        "{:?}",
        found[0]
    );
    // The way out is named, and it is the one Part I 8.3 names.
    let help = found[0].help.clone().unwrap_or_default();
    assert!(help.contains("message.clone()"), "{help}");
    // Nikaia's words and not the backend's.
    for backend in ["closure", "borrow of moved", "async"] {
        assert!(!format!("{found:?}").contains(backend), "`{backend}`");
    }
}

/// `NK2103`: a task's lambda may not name an argument, because a task is handed
/// nothing (Part I, 8.2).
///
/// Dropping the name silently would be worse: the body would then refer to
/// something nothing declared, which `NK1117` reports about a name the author
/// *did* write.
#[test]
fn a_task_that_names_an_argument_is_refused() {
    let found = findings("fn main() { spawn fn(x) { println(f\"{x}\") } }");
    assert!(found.iter().any(|f| f.code == "NK2103"), "{found:?}");
    let first = found
        .iter()
        .find(|f| f.code == "NK2103")
        .expect("checked above");
    assert!(
        first.message.contains("a task isn't given any arguments"),
        "{first:?}"
    );
}

/// A handle on a `Shared[T]` is **duplicated** into a task, not moved
/// ([ADR-312](../../../docs/specification/adr/adr-312.md) D1, D5).
///
/// Part I 8.3 says `NK2101` belongs to the data case only, and this is the other
/// case: using the value again after the task is built is the very thing the
/// duplication serves, so there is nothing to refuse and no `.clone()` to write.
#[test]
fn a_shared_handle_into_a_task_is_not_refused() {
    let found = findings(
        "fn main() {\n\
         \x20   let counts = Shared(Vec())\n\
         \x20   spawn fn { println(f\"{counts.len()}\") }\n\
         \x20   println(f\"{counts.len()}\")\n\
         }",
    );
    assert!(
        !found.iter().any(|f| f.code == "NK2101"),
        "a duplicated handle was refused as a move: {found:?}"
    );
}

/// The parenthesised `spawn ( … )` says what happened to it.
///
/// Part I 8.2 called it *"a bug in the parser and not a second form"*, and
/// programs were written against it — so the arm stays, as a sentence rather
/// than as an alternative.
#[test]
fn the_parenthesised_form_says_what_happened_to_it() {
    let source = "fn main() { spawn(1) }";
    let refused = parse_to_ast(source).expect_err("refused");
    let finding =
        nikaia::diagnostics::refused_finding(&refused).expect("a parse error carries its finding");
    let message = nikaia::diagnostics::render_finding(finding, "app.nika", source);
    assert!(message.contains("`spawn` takes a lambda"), "{message}");
    assert!(message.contains("spawn fn"), "{message}");
}

/// The emitted Rust has no closure in it, and that is the decision.
///
/// A task's body may **pause**, and a body is a **block** rather than something
/// called — so it is an `async move` *block*, which is a future, and a closure
/// around it would only be called once. Not because the language below has no
/// `async` closure, which is what this comment used to say and is false
/// ([ADR-277](../../../docs/specification/adr/adr-277.md) D13, D16). `move` is
/// Part I 8.3's implicit move and Rust's `move` meeting at the same place.
#[test]
fn a_task_is_an_async_block_and_never_a_closure() {
    let rust = lower(
        "use std::fs\n\
         fn main() {\n\
         \x20   spawn fn { fs::read_to_string(\"x\", fs::Root::Anywhere) catch { \"\" } }\n\
         }",
    );
    assert!(rust.contains("TaskHandle::start(async move"), "{rust}");
    // The pausing call inside the task takes its `.await`, which is the thing a
    // closure could not have held.
    assert!(
        rust.contains("read_to_string(std::path::Path::new(&(\"x\")), &fs::Root::Anywhere).await"),
        "{rust}"
    );
    assert!(!rust.contains("start(|| "), "{rust}");
}

/// **A task that never finishes is abandoned at the deadline**
/// ([ADR-297](../../../docs/specification/adr/adr-297.md) D6), rather than
/// becoming a program that never exits.
///
/// Holding `main`'s value until the queue empties is what makes
/// [ADR-055](../../../docs/specification/adr/adr-055.md) D5's *"a task nobody
/// joins still runs"* true; this is the other half of it. D5 already named both
/// the bound and its key — *"waits for parked cleanups **and detached tasks**,
/// bounded by `cleanup-deadline`"* — so nothing here is a new rule.
///
/// **End to end and in its own process**, because the runtime is one per
/// process and the default deadline is 30 seconds: the program gets a
/// `nikaia-runtime.toml` naming a short one. The task loops on a read, which is
/// a real suspension point since §6 step 3 — a loop that never pauses could not
/// be abandoned by any deadline, because a task that never yields never gives
/// the executor the thread back, which is what `user_parallelism = no` means.
#[test]
fn a_task_that_never_finishes_is_abandoned_at_the_deadline() {
    let dir = common::scratch_dir("tasks-drain-deadline");
    std::fs::write(dir.join("eins.txt"), "eins").expect("write");
    std::fs::write(
        dir.join("nikaia-runtime.toml"),
        "cleanup-deadline = \"300ms\"\n",
    )
    .expect("the runtime configuration");

    let rust = lower(
        "use std::fs\n\
         \n\
         fn forever() {\n\
         \x20   while true {\n\
         \x20       let text = fs::read_to_string(\"eins.txt\", fs::Root::Anywhere) catch { \"\" }\n\
         \x20       if text.len() < 0 { return }\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   spawn fn { forever() }\n\
         \x20   println(\"main is done\")\n\
         }",
    );
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let built = common::compile(&file, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&built.stderr)
    );

    let started = std::time::Instant::now();
    let ran = Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run it");
    let took = started.elapsed();

    // **That it ends at all is the claim.** The bound is 300 ms and the margin
    // is generous on purpose: timing a wall clock tightly is how a suite becomes
    // flaky, and a loop with no end would not finish in any of it.
    assert!(
        took < std::time::Duration::from_secs(20),
        "the drain did not end: {took:?}"
    );
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );

    let printed = String::from_utf8_lossy(&ran.stdout);
    assert!(printed.contains("main is done"), "{printed}");

    // And it says what it abandoned rather than exiting quietly (D5).
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(said.contains("cleanup deadline"), "{said}");
    assert!(said.contains("background task"), "{said}");

    std::fs::remove_dir_all(&dir).ok();
}

/// **Which executor a `spawn` goes to is the switch's one reach into a task**
/// ([ADR-055](../../../docs/specification/adr/adr-055.md) §6 step 1,
/// [ADR-037](../../../docs/specification/adr/adr-037.md) D2).
///
/// One Nikaia line and two lowerings. At `user_parallelism = yes` a task may be
/// polled on a thread that did not start it, so its future has to be `Send` (§2
/// D6) and the pool's starter is what asks for that; at `no` it may not, so it
/// does not — and asking there would refuse a task holding the plain count
/// [ADR-312](../../../docs/specification/adr/adr-312.md) D9 gives a `Shared` at
/// one user thread.
#[test]
fn the_switch_decides_which_executor_a_task_is_started_on() {
    let source = "fn work() -> i64 { return 1 }\n\
                  fn main() throws { let t = spawn fn { work() } println(f\"{t.join()}\") }";

    let at_no = lower_at(source, "no");
    assert!(
        at_no.contains("TaskHandle::start(async move"),
        "one thread, so no `Send` is asked for: {at_no}"
    );
    assert!(!at_no.contains("start_on_pool"), "{at_no}");

    let at_yes = lower_at(source, "yes");
    assert!(
        at_yes.contains("TaskHandle::start_on_pool(async move"),
        "a thread of its own, so the future must be `Send`: {at_yes}"
    );
}

/// **A function kept for later takes what it captures** (Part I 8.3, ADR-328
/// D7, #95 step 6): a supervisor's child is called after the statement that
/// made it, so a `String` it captures went with it, and using it again is
/// `NK2101` - with the same help a task's capture has.
#[test]
fn data_a_kept_child_took_and_the_program_used_again_is_refused() {
    let found = findings(
        "use std::supervisor\n\
         fn greet(name: ref String) throws {\n\
         \x20   println(name)\n\
         }\n\
         fn main() throws {\n\
         \x20   let name = \"Ada\".clone()\n\
         \x20   supervisor::run([supervisor::child(fn { greet(name) }; restart: supervisor::Kind::Transient)])\n\
         \x20   println(name)\n\
         }",
    );
    let taken: Vec<_> = found.iter().filter(|f| f.code == "NK2101").collect();
    assert_eq!(taken.len(), 1, "{found:?}");
    assert!(
        taken[0].message.contains("a function kept for later"),
        "{:?}",
        taken[0]
    );
}
