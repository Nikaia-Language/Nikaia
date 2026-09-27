//! `Shared[T]`, compiled and run.
//!
//! Part I 6.2 says how the first handle on a shared value is made - by writing
//! the type - and [ADR-042](../../../docs/specification/adr/adr-042.md) D2 says
//! a function that only *uses* the value takes an ordinary view of it. Neither
//! claim can be checked by reading the emitted Rust: `Rc` and `Arc` differ in
//! nothing a program can observe except speed and `Send`-ness
//! (ADR-037 D7), so a test that compared text would pass on a
//! lowering that does not run. These compile the emitted Rust and run the
//! binary.
//!
//! At **both settings of `user_parallelism`**, because
//! [ADR-037](../../../docs/specification/adr/adr-037.md) D6 is the claim that
//! one count serves both: a program that printed differently under the two
//! would be the failure that decision exists to rule out.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

/// A `Shared[Connection]` made, lent to a function that takes `&Connection`,
/// and printed.
///
/// `serve` never mentions sharing, which is Part I 6.2's own example: whether
/// the value is shared is the caller's decision, and `serve` has no business
/// knowing. The borrow reaches the inner value through `Shared::deref` and
/// ADR-042 D2, with nothing else added.
const BORROWED: &str = "\
struct Connection {
    host: String
}

fn connect(host: String) -> Connection {
    Connection { host: host }
}

fn serve(db: ref Connection) {
    println(f\"serving {db.host}\")
}

fn main() {
    let db = Shared(connect(\"localhost\"))
    serve(db)
    serve(db)
}
";

/// Lower `source`, and hand back the emitted Rust.
fn lower(dir: &Path, source: &str, flags: &[&str]) -> String {
    let input = dir.join("main.nika");
    std::fs::write(&input, source).expect("the source");
    let output = dir.join("main.rs");
    let run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["--input", input.to_str().expect("utf-8 path")])
        .args(["--output", output.to_str().expect("utf-8 path")])
        .args(["--no-cache"])
        .args(flags)
        .env("NIKAIA_CACHE_DIR", dir.join("cache"))
        .output()
        .expect("the nikaia binary runs");
    assert!(
        run.status.success(),
        "lowering failed:\n{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    std::fs::read_to_string(&output).expect("the emitted Rust")
}

/// Lower, compile, and hand back the binary and the Rust it was built from.
fn build(dir: &Path, source: &str, flags: &[&str]) -> (PathBuf, String) {
    let rust = lower(dir, source, flags);
    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    (binary, rust)
}

/// Lower, compile, run, and hand back what it printed and the Rust behind it.
fn run(purpose: &str, source: &str, flags: &[&str]) -> (String, String) {
    let dir = common::scratch_dir(purpose);
    let (binary, rust) = build(&dir, source, flags);
    let ran = Command::new(&binary).output().expect("run the program");
    assert!(
        ran.status.success(),
        "the program failed: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let printed = String::from_utf8_lossy(&ran.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    (printed, rust)
}

/// The whole of Part I 6.2's worked example, end to end.
#[test]
fn a_shared_value_is_made_by_calling_the_type_and_lent_out_by_a_view() {
    let (printed, rust) = run("shared-borrowed", BORROWED, &[]);
    assert_eq!(printed.trim(), "serving localhost\nserving localhost");
    // Nothing crosses a thread with it, so D7's optimisation applies: the plain
    // count. That is the base case ADR-037 §5 asks for - an analysis answering
    // `atomic` about everything would pass a test that only checked it ran.
    assert!(
        rust.contains("std::rc::Rc::new(connect("),
        "the call is the constructor (ADR-064 D2):\n{rust}"
    );
    // A borrow duplicates nothing (ADR-040 D1's correction). Two of them, and
    // not one step of the count.
    assert!(
        !rust.contains(".clone()"),
        "lending the inner value out must not touch the count:\n{rust}"
    );
}

/// One count at both settings of `user_parallelism`, and the same program.
///
/// ADR-037 D6's whole claim: the representation came off the switch, so a
/// library built at one setting keeps working at the other. The *count* a value
/// gets is per value and never reads the switch either (D4's note), so the two
/// lowerings are byte for byte the same here - which is a stronger statement
/// than "they print the same" and is the one D4 makes.
#[test]
fn the_switches_agree_about_a_shared_value() {
    let (sequential, one) = run("shared-sequential", BORROWED, &["--user-parallelism", "no"]);
    let (parallel, two) = run("shared-parallel", BORROWED, &["--user-parallelism", "yes"]);
    assert_eq!(sequential, parallel, "the two builds print differently");
    // **What may not move is the meaning, and it does not.** The count itself
    // may: since [ADR-061](../../../docs/specification/adr/adr-061.md) D2 one
    // user thread is a build where nothing can cross, so every count there is
    // plain. This value is plain at both settings anyway - nothing crosses with
    // it at either - which is why the two lowerings are still byte for byte the
    // same about the count. The files are not identical and never were - the
    // runtime line names the pool - so it is the count that is compared.
    assert_eq!(
        one.contains("std::rc::Rc<Connection>"),
        two.contains("std::rc::Rc<Connection>"),
        "--- no ---\n{one}\n--- yes ---\n{two}"
    );
}

/// A value the analysis cannot prove stays put gets the atomic floor, and the
/// program still runs and still prints the same thing.
///
/// `Rc` and `Arc` differ in nothing observable (ADR-037 D7), which
/// is what lets the count be inferred at all - so the two lowerings of the same
/// program have to agree about what it prints. Here the public signature is what
/// forces the floor (ADR-037 D8's last row).
#[test]
fn the_atomic_floor_runs_the_same_program() {
    let source = "\
struct Connection {
    host: String
}

fn connect(host: String) -> Connection {
    Connection { host: host }
}

pub fn serve(db: Shared[Connection]) {
    println(f\"serving {db.host}\")
}

fn main() {
    let db = Shared(connect(\"localhost\"))
    serve(db)
}
";
    // **At `yes`**, because that is where the fallback has anything to protect:
    // at one user thread nothing can cross, so there is nothing for a caller in
    // a unit this build cannot see to do with the value
    // ([ADR-061](../../../docs/specification/adr/adr-061.md) D2) and the count
    // is plain there whatever the signature says.
    let (printed, rust) = run("shared-atomic", source, &["--user-parallelism", "yes"]);
    assert_eq!(printed.trim(), "serving localhost");
    assert!(
        rust.contains("std::sync::Arc<Connection>"),
        "a public signature is the one fallback no contract can lift:\n{rust}"
    );

    // …and the same program at one user thread, which prints the same thing and
    // pays nothing for a crossing that cannot happen.
    let (printed, rust) = run("shared-atomic-no", source, &["--user-parallelism", "no"]);
    assert_eq!(printed.trim(), "serving localhost");
    assert!(rust.contains("std::rc::Rc<Connection>"), "{rust}");
}

/// A field whose declared type says the value is shared is the other place the
/// first handle is made (Part I 6.2).
#[test]
fn a_hull_written_in_a_struct_literal_field_makes_a_handle() {
    let source = "\
struct Connection {
    host: String
}

struct Pool {
    db: Shared[Connection],
    size: i64
}

fn connect(host: String) -> Connection {
    Connection { host: host }
}

fn main() {
    let pool = Pool { db: Shared(connect(\"localhost\")), size: 4 }
    println(f\"{pool.size} to {pool.db.host}\")
}
";
    let (printed, rust) = run("shared-field", source, &[]);
    assert_eq!(printed.trim(), "4 to localhost");
    assert!(
        rust.contains("::new(connect("),
        "the field's declared type is what makes the handle:\n{rust}"
    );
}

/// `--sharing` on a program that has a `Shared` value.
///
/// It has never had a real input before: until the type existed, the report was
/// exercised only by the analysis's own tests. This is the flag doing what
/// ADR-037 D7 asks of it, on a program that compiles.
#[test]
fn the_report_explains_a_real_program() {
    let printed = report(BORROWED);
    assert!(printed.starts_with("main:\n"), "{printed}");
    // `(Shared)` and not `(Shared[Connection])`: since
    // [ADR-064](../../../docs/specification/adr/adr-064.md) D2 the hull is made
    // by a call and this line writes no annotation, so what the analysis knows
    // about the slot is that it is a `Shared` - not what it holds. It needs no
    // more than that to decide a count, and the report says what it knows.
    assert!(printed.contains("plain   `db` (Shared)"), "{printed}");
    assert!(
        printed.contains("1 `Shared` value(s): 1 plain, 0 atomic"),
        "{printed}"
    );
}

// --- the handle is duplicated, and the cost is readable (ADR-040) ------------

/// A handle **handed on by value** is duplicated, and a **borrow** duplicates
/// nothing. Both directions in one program, because the pair is the rule.
///
/// [ADR-040](../../../docs/specification/adr/adr-040.md) D1: there is no method
/// to call, so the step is written where the handle is handed on - and D2 makes
/// it unconditional rather than conditional on a later use, because a line
/// further down may not decide what a line further up does to a cleanup point.
#[test]
fn a_handle_handed_on_by_value_is_duplicated_and_a_borrow_is_not() {
    let source = "\
struct Conn { host: String }

struct Pool { db: Shared[Conn] }

fn connect(host: String) -> Conn {
    Conn { host: host }
}

fn serve(db: ref Conn) {
    println(f\"serving {db.host}\")
}

fn peek(db: ref Shared[Conn]) {
    println(f\"peeking {db.host}\")
}

fn keep(db: Shared[Conn]) -> Pool {
    return Pool { db: db }
}

fn main() {
    let db = Shared(connect(\"localhost\"))
    serve(db)
    peek(db)
    let pool = keep(db)
    println(f\"kept {pool.db.host}\")
    serve(db)
}
";
    let (printed, rust) = run("shared-duplicated", source, &[]);
    // `db` is still ours after `keep` took one of its own, which is the whole
    // point of the rule: each handle dies at the end of its own block (D3).
    assert_eq!(
        printed.trim(),
        "serving localhost\npeeking localhost\nkept localhost\nserving localhost"
    );
    assert!(
        rust.contains("keep(db.clone())"),
        "a handle handed on by value is duplicated:\n{rust}"
    );
    // Two borrows and one lent handle, and not one step of the count between
    // them. D1's correction is exactly this line.
    assert_eq!(
        rust.matches(".clone()").count(),
        1,
        "only the by-value handover duplicates:\n{rust}"
    );
    assert!(
        rust.contains("serve(&db)") && rust.contains("peek(&db)"),
        "a borrow is handed on as written:\n{rust}"
    );
}

/// A handle read out of a **field** and handed on by value is duplicated too:
/// `keep(pool.db)` gives the callee an owner as surely as `keep(db)` does.
#[test]
fn a_handle_read_out_of_a_field_is_duplicated_when_it_is_handed_on() {
    let source = "\
struct Conn { host: String }

struct Pool { db: Shared[Conn] }

fn connect(host: String) -> Conn {
    Conn { host: host }
}

fn keep(db: Shared[Conn]) -> Pool {
    return Pool { db: db }
}

fn main() {
    let db = Shared(connect(\"localhost\"))
    let pool = keep(db)
    let again = keep(pool.db)
    println(f\"{pool.db.host} {again.db.host}\")
}
";
    let (printed, rust) = run("shared-field-handover", source, &[]);
    assert_eq!(printed.trim(), "localhost localhost");
    assert!(
        rust.contains("keep(pool.db.clone())"),
        "a handle taken out of a field is handed on by value:\n{rust}"
    );
}

/// **`--sharing` names each duplication site beside the count it printed**
/// ([ADR-040](../../../docs/specification/adr/adr-040.md) D5).
///
/// D1 makes the place one step of the count is paid unwritten in the source, so
/// it has to be readable somewhere, and the count is already printed here.
#[test]
fn the_report_names_each_duplication_site() {
    let source = "\
struct Conn { host: String }

struct Pool { db: Shared[Conn] }

fn connect(host: String) -> Conn {
    Conn { host: host }
}

fn serve(db: ref Conn) { }

fn keep(db: Shared[Conn]) -> Pool {
    return Pool { db: db }
}

fn main() {
    let db = Shared(connect(\"localhost\"))
    serve(db)
    let pool = keep(db)
    println(f\"{pool.db.host}\")
}
";
    let printed = report(source);
    assert!(
        printed.contains("duplicated: handed to `keep` as `db`"),
        "the site the step is paid at has to be in the output: {printed}"
    );
    // And a value that was only ever borrowed says so, rather than saying
    // nothing - the absence of a step is the thing a reader came to look up.
    assert!(
        printed.contains("one handle, so the count is never stepped"),
        "{printed}"
    );
}

/// A borrow contributes no duplication site, which is the other half of D1's
/// correction: a report that listed `serve(&db)` would be charging for an
/// instruction nothing pays.
#[test]
fn a_borrow_is_not_a_duplication_site() {
    let source = "\
struct Conn { host: String }

fn connect(host: String) -> Conn {
    Conn { host: host }
}

fn serve(db: ref Conn) { }

fn peek(db: ref Shared[Conn]) { }

fn main() {
    let db = Shared(connect(\"localhost\"))
    serve(db)
    peek(db)
}
";
    let printed = report(source);
    assert!(
        !printed.contains("duplicated:"),
        "lending the inner value out duplicates nothing: {printed}"
    );
    assert!(
        printed.contains("one handle, so the count is never stepped"),
        "{printed}"
    );
}

/// `--sharing` on a source, and what it printed.
fn report(source: &str) -> String {
    let dir = common::scratch_dir("shared-report");
    let input = dir.join("main.nika");
    std::fs::write(&input, source).expect("the source");
    let run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["--input", input.to_str().expect("utf-8 path")])
        .args([
            "--output",
            dir.join("main.rs").to_str().expect("utf-8 path"),
        ])
        .args(["--sharing", "--no-cache"])
        .env("NIKAIA_CACHE_DIR", dir.join("cache"))
        .output()
        .expect("the nikaia binary runs");
    let printed = String::from_utf8_lossy(&run.stdout).into_owned();
    assert!(run.status.success(), "{printed}");
    let _ = std::fs::remove_dir_all(&dir);
    printed
}

/// A function that returns a shared value wraps on a line of its own, and the
/// `return` then hands on what is already shared.
///
/// Part I 6.2's rule is that sharing starts on a line where the type was written;
/// a signature line is not one. This is not a dead end and the message says why:
/// wrap inside on a line of its own, or return a plain value and let the caller
/// write the type. Both are ordinary, and this runs the first.
#[test]
fn a_function_returning_a_shared_value_wraps_on_a_line_of_its_own() {
    let (printed, rust) = run(
        "shared-returned",
        "struct Connection {\n    \
             host: String\n\
         }\n\
         \n\
         fn connect(host: String) -> Shared[Connection] {\n    \
             let c = Shared(Connection { host: host })\n    \
             return c\n\
         }\n\
         \n\
         fn main() {\n    \
             let db = connect(\"localhost\")\n    \
             println(f\"connected to {db.host}\")\n\
         }\n",
        &[],
    );
    assert_eq!(printed.trim(), "connected to localhost");
    // The annotated `let` is the one constructor, and the `return` adds nothing.
    assert_eq!(
        rust.matches("Rc::new").count() + rust.matches("Arc::new").count(),
        1,
        "the `return` must not wrap a second time:\n{rust}"
    );
}

// --- the shared mutable type, end to end (ADR-064) ---------------------------

/// Part II 12.2's counter, which is the program `user_parallelism` exists for.
const COUNTER: &str = "\
fn zaehle(counter: SharedMut[i64]) {
    counter.update fn(mut alt) { alt += 1 }
}

fn main() {
    let counter = SharedMut(0)
    counter.set(5)
    zaehle(counter)
    println(f\"{counter.get()}\")
}
";

/// **It compiles and runs, at both settings, for the first time.**
///
/// Three things had to be true at once and none of them was
/// ([ADR-064](../../../docs/specification/adr/adr-064.md)): `SharedMut` had to be
/// a type rather than a name that went through into Rust untranslated; the hull
/// had to be makeable around a **number**, which the annotation never could
/// because a literal has no type of its own; and both ends of the value - the
/// local and the parameter it is handed to - had to agree about the count.
#[test]
fn the_counter_of_part_ii_12_2_runs_at_both_settings() {
    for setting in ["no", "yes"] {
        let (printed, rust) = run(
            &format!("sharedmut-{setting}"),
            COUNTER,
            &["--user-parallelism", setting],
        );
        assert_eq!(printed.trim(), "6", "at `{setting}`");

        // **One name above, two hulls below, and always a matching pair** -
        // never an atomic count around a cheap lock (ADR-061 D2, ADR-057 D3).
        //
        // The cheap pair at **both** settings, and that is the per-value answer
        // doing its work: `zaehle` is an ordinary call on the same thread, so
        // nothing crosses even where the build allows it to
        // ([ADR-037](../../../docs/specification/adr/adr-037.md) D7). The test
        // below spawns, and takes the other pair.
        let (count, lock) = ("std::rc::Rc", "lock::Local");
        let _ = setting;
        assert!(
            rust.contains(&format!("{count}::new(nikaia_std::{lock}::new(")),
            "the constructor writes both hulls at `{setting}`:\n{rust}"
        );
        assert!(
            rust.contains(&format!(
                "fn zaehle(counter: {count}<nikaia_std::{lock}<i64>>)"
            )),
            "and the parameter agrees with it at `{setting}`:\n{rust}"
        );
        // The handle is duplicated where it is handed on, not moved.
        assert!(
            rust.contains("zaehle(counter.clone())"),
            "a handle handed on by value is duplicated (ADR-040 D1):\n{rust}"
        );
    }
}

/// **And the long spelling is not a second way to write it**
/// ([ADR-064](../../../docs/specification/adr/adr-064.md) D3), because two
/// spellings of one type were the same bytes below and two types above.
#[test]
fn a_shared_around_a_lock_is_refused_and_names_the_short_form() {
    let dir = common::scratch_dir("sharedmut-spelling");
    let input = dir.join("main.nika");
    std::fs::write(&input, "fn zaehle(counter: Shared[Locked[i64]]) { }").expect("the source");
    let run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["--input", input.to_str().expect("utf-8 path")])
        .args([
            "--output",
            dir.join("main.rs").to_str().expect("utf-8 path"),
        ])
        .args(["--no-cache"])
        .env("NIKAIA_CACHE_DIR", dir.join("cache"))
        .output()
        .expect("the compiler runs");
    let said = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(!run.status.success(), "it must be refused: {said}");
    assert!(said.contains("NK1123"), "{said}");
    assert!(said.contains("`SharedMut[i64]`"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A handle a task uses is duplicated, not moved**
/// ([ADR-040](../../../docs/specification/adr/adr-040.md) D1's task half).
///
/// Built for a call long before this, and unrunnable until `spawn` lowered: the
/// task took the handle with it and `rustc` refused the later use about the
/// generated file. The step is written *outside* the future, so the name the body
/// moves is the new handle and the caller's own survives the `spawn`.
///
/// **And both hulls come out atomic here**, which is the other half of the claim:
/// the same program with no `spawn` takes the cheap pair at `yes`, so this is the
/// per-value answer and not a floor.
const SHARED_WITH_A_TASK: &str = "\
fn main() {
    let counter = SharedMut(0)
    let t = spawn fn { counter.update fn(mut alt) { alt += 1 } }
    t.join()
    println(f\"{counter.get()}\")
}
";

#[test]
fn a_handle_a_task_uses_is_duplicated_and_the_name_survives() {
    for setting in ["no", "yes"] {
        let (printed, rust) = run(
            &format!("task-handle-{setting}"),
            SHARED_WITH_A_TASK,
            &["--user-parallelism", setting],
        );
        assert_eq!(printed.trim(), "1", "at `{setting}`");
        assert!(
            rust.contains("{ let counter = counter.clone(); nikaia_std::task::TaskHandle::start"),
            "the step is written outside the future at `{setting}`:\n{rust}"
        );
    }

    // A value a task takes may reach another thread, so both hulls are the
    // robust shape - and only where it does (ADR-037 D7, ADR-057 D3). For a
    // counter the robust lock is the word itself, a compare-and-swap
    // (ADR-238 D1): the count is atomic either way.
    let (_, crossing) = run(
        "task-handle-pair",
        SHARED_WITH_A_TASK,
        &["--user-parallelism", "yes"],
    );
    assert!(
        crossing.contains("std::sync::Arc::new(nikaia_std::lock::Word::new("),
        "a counter a task takes is atomic, and its lock a word:\n{crossing}"
    );
}

/// **Four tasks on four threads, one lock, and the count is exact**
/// ([ADR-045](../../../docs/specification/adr/adr-045.md) D2).
///
/// The one program neither half of this could run on its own, which is why it
/// is here rather than beside either. The lock had to be a **type**
/// ([ADR-064](../../../docs/specification/adr/adr-064.md)) — it was an
/// annotation that went into the language below untranslated — and a task had
/// to be a **thread** ([ADR-055](../../../docs/specification/adr/adr-055.md) §6
/// step 1's `yes` half), which it was not: every task interleaved on the one
/// thread, so a lock in one was a lock nobody contended for.
///
/// D2's sentence is *"a lock may go into a task of your own, at both
/// settings"*, and at `yes` that means what it sounds like: four threads, a
/// real operating-system lock underneath, and 40 000 increments that add up.
/// A count that came out short would be the lock failing to be one; at `no` the
/// same program has to print the same number, which is the *uniform API* of
/// Part II 11.2 applied to the thing hardest to keep uniform.
#[test]
fn a_lock_shared_by_four_tasks_counts_every_increment() {
    const SHARED_COUNTER: &str = "\
fn bump(counter: SharedMut[i64], times: i64) {
    let mut i = 0
    while i < times {
        counter.update fn(mut old) { old += 1 }
        i = i + 1
    }
}

fn main() {
    let counter = SharedMut(0)
    let a = spawn fn { bump(counter, 2500) }
    let b = spawn fn { bump(counter, 2500) }
    let c = spawn fn { bump(counter, 2500) }
    let d = spawn fn { bump(counter, 2500) }
    a.join()
    b.join()
    c.join()
    d.join()
    println(f\"{counter.get()}\")
}
";

    for setting in ["no", "yes"] {
        let (printed, rust) = run(
            &format!("lock-in-tasks-{setting}"),
            SHARED_COUNTER,
            &["--user-parallelism", setting],
        );
        assert_eq!(
            printed.trim(),
            "10000",
            "`{setting}`: every increment has to be in the total"
        );
        // The lowering is what says the tasks are on threads at all: at `yes`
        // the pool's starter, which is where the `Send` bound is, and at `no`
        // the one-thread queue, which has none.
        let on_the_pool = rust.contains("start_on_pool");
        assert_eq!(
            on_the_pool,
            setting == "yes",
            "`{setting}` picked the wrong executor:\n{rust}"
        );
    }
}
// --- a door over several locks (ADR-065) -------------------------------------

/// **Chapter 12's own transfer**, which the language could not write.
///
/// `access_all` read and `update_all` did not exist, so taking from one account
/// and giving to the other under both locks had no door
/// ([ADR-059](../../../docs/specification/adr/adr-059.md) D1 named the gap as it
/// made it). It runs now, and the block hands back **one new value per lock** —
/// `update`'s rule widened rather than a second rule
/// ([ADR-065](../../../docs/specification/adr/adr-065.md) D2).
const TRANSFER: &str = "\
fn main() {
    let konto_a = SharedMut(100)
    let konto_b = SharedMut(5)
    update_all(konto_a, konto_b) fn(mut von, mut nach) {
        von -= 30
        nach += 30
    }
    let stand = access_all(konto_a, konto_b) fn(a, b) { f\"{a} {b}\" }
    println(stand)
}
";

#[test]
fn a_transfer_between_two_locks_runs_at_both_settings() {
    for setting in ["no", "yes"] {
        let (printed, rust) = run(
            &format!("two-locks-{setting}"),
            TRANSFER,
            &["--user-parallelism", setting],
        );
        assert_eq!(printed.trim(), "70 35", "at `{setting}`");
        // The locks go in by reference and the block is the last argument -
        // which is what the trailing-lambda rule already made of it.
        assert!(
            rust.contains("nikaia_std::lock::update_all(&konto_a, &konto_b, |von, nach|"),
            "at `{setting}`:\n{rust}"
        );
    }
}

/// **A door over several locks takes locks**, and says so in this language's
/// words rather than handing `rustc` a program about a trait bound.
///
/// A number is the case worth testing: a literal carries no type on purpose
/// (Part I 2.4), and reading that absence as *"might be a lock"* is what used to
/// send this to the backend.
#[test]
fn a_door_over_several_locks_refuses_what_is_not_one() {
    let refused = |source: &str| -> String {
        let dir = common::scratch_dir("two-locks-refused");
        let input = dir.join("main.nika");
        std::fs::write(&input, source).expect("the source");
        let run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .args(["--input", input.to_str().expect("utf-8 path")])
            .args([
                "--output",
                dir.join("main.rs").to_str().expect("utf-8 path"),
            ])
            .args(["--no-cache"])
            .env("NIKAIA_CACHE_DIR", dir.join("cache"))
            .output()
            .expect("the compiler runs");
        assert!(!run.status.success(), "it must be refused");
        let said = String::from_utf8_lossy(&run.stderr).into_owned();
        let _ = std::fs::remove_dir_all(&dir);
        said
    };

    let said = refused(
        "fn main() {\n    let a = SharedMut(1)\n    let n = 5\n\
         \x20   access_all(a, n) fn(x, y) { println(f\"{x} {y}\") }\n}",
    );
    assert!(said.contains("NK1124"), "{said}");
    assert!(said.contains("takes locks"), "{said}");

    // One lock is not several, and the block names one value per lock.
    let alone = refused(
        "fn main() {\n    let a = SharedMut(1)\n\
         \x20   access_all(a) fn(x) { println(f\"{x}\") }\n}",
    );
    assert!(alone.contains("at least two"), "{alone}");

    let short = refused(
        "fn main() {\n    let a = SharedMut(1)\n    let b = SharedMut(2)\n\
         \x20   access_all(a, b) fn(x) { println(f\"{x}\") }\n}",
    );
    assert!(short.contains("one value per lock"), "{short}");
}
