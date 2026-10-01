//! **A value's `cleanup` runs where Rust's ownership says the value died**
//! ([ADR-239](../../../docs/specification/adr/adr-239.md), refining ADR-006 D1
//! and D2): the value's `Drop` parks its `cleanup` in the running task, and the
//! compiler settles what was parked at the end of each block a cleanup value
//! dies in and around the function's body. A failing `cleanup` is the
//! function's failure, or a secondary one when the function already failed;
//! `close()` hands the failure back at the call. Each program prints the same
//! at both settings of `user_parallelism`.

mod common;

use std::process::Command;

use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
}

/// Compiled and run in a directory of its own, which the program may write
/// files into; stdout and stderr and whether it succeeded.
fn ran(purpose: &str, source: &str, how: Build) -> (String, String, bool) {
    let (out, err, status) = ran_with(purpose, source, how, None);
    (out, err, status == Some(0))
}

/// The same with a runtime configuration of its own, and the exit status.
fn ran_with(
    purpose: &str,
    source: &str,
    how: Build,
    runtime: Option<&str>,
) -> (String, String, Option<i32>) {
    let rust = lowered(source, how);
    let dir = common::scratch_dir(&format!("cleanup-points-{purpose}"));
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "{purpose} did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let mut command = Command::new(&binary);
    command.current_dir(&dir);
    if let Some(runtime) = runtime {
        let config = dir.join("nikaia-runtime.toml");
        std::fs::write(&config, runtime).expect("write the runtime configuration");
        command.env("NIKAIA_RUNTIME_CONFIG", &config);
    }
    let out = command.output().expect("run it");
    std::fs::remove_dir_all(&dir).ok();
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code(),
    )
}

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    check::check(&parsed, &own, &library).findings
}

const NOTED: &str = "struct Noted {\n\
    \x20   name: String,\n\
    }\n\
    \n\
    impl Cleanup for Noted {\n\
    \x20   fn cleanup(ref mut self) {\n\
    \x20       println(f\"cleanup {self.name}\")\n\
    \x20   }\n\
    }\n\
    \n\
    fn work() {\n\
    \x20   let a = Noted { name: f\"a\" }\n\
    \x20   let b = Noted { name: f\"b\" }\n\
    \x20   if a.name.len() == 1 {\n\
    \x20       let c = Noted { name: f\"c\" }\n\
    \x20       println(f\"inner {c.name}\")\n\
    \x20   }\n\
    \x20   println(f\"outer {a.name} {b.name}\")\n\
    }\n\
    \n\
    fn main() {\n\
    \x20   work()\n\
    \x20   println(\"done\")\n\
    }\n";

/// **D2: at the end of the block the value dies in, in the order they die.**
#[test]
fn a_cleanup_runs_where_its_value_dies() {
    for how in [Build::default(), Build::parallel()] {
        let (out, err, ok) = ran("noted", NOTED, how);
        assert!(ok, "{err}");
        assert_eq!(
            out, "inner c\ncleanup c\nouter a b\ncleanup b\ncleanup a\ndone\n",
            "at {how:?}"
        );
    }
}

const WRITER: &str = "use std::fs\n\
    \n\
    fn report(lines: Vec[String]) throws {\n\
    \x20   let mut out = fs::create(\"report.txt\", fs::Root::Anywhere)\n\
    \x20   for line in lines {\n\
    \x20       out.write(f\"{line}\\n\")\n\
    \x20   }\n\
    }\n\
    \n\
    fn main() throws {\n\
    \x20   report([f\"eins\", f\"zwei\"])\n\
    \x20   print(fs::read_to_string(\"report.txt\", fs::Root::Anywhere))\n\
    }\n";

/// **The first user in std: `fs::create` hands back a buffered writer** whose
/// cleanup writes what it holds, at the end of `report`.
#[test]
fn a_buffered_writer_is_flushed_when_it_dies() {
    for how in [Build::default(), Build::parallel()] {
        let (out, err, ok) = ran("writer", WRITER, how);
        assert!(ok, "{err}");
        assert_eq!(out, "eins\nzwei\n", "at {how:?}");
    }
}

const FLAKY: &str = "enum FlushError {\n\
    \x20   Refused(String),\n\
    }\n\
    \n\
    impl Error for FlushError {\n\
    \x20   fn message(ref self) -> String {\n\
    \x20       match self {\n\
    \x20           FlushError::Refused(name) => f\"{name} would not flush\"\n\
    \x20       }\n\
    \x20   }\n\
    }\n\
    \n\
    struct Flaky {\n\
    \x20   name: String,\n\
    }\n\
    \n\
    impl Cleanup for Flaky {\n\
    \x20   fn cleanup(ref mut self) throws {\n\
    \x20       throw FlushError::Refused(f\"{self.name}\")\n\
    \x20   }\n\
    }\n\
    \n\
    fn quiet() throws {\n\
    \x20   let f = Flaky { name: f\"quiet\" }\n\
    \x20   println(f\"using {f.name}\")\n\
    }\n\
    \n\
    fn loud(n: i64) throws {\n\
    \x20   let f = Flaky { name: f\"loud\" }\n\
    \x20   if n > 0 {\n\
    \x20       throw FlushError::Refused(f\"the body\")\n\
    \x20   }\n\
    \x20   println(f\"using {f.name}\")\n\
    }\n\
    \n\
    fn closing() {\n\
    \x20   let f = Flaky { name: f\"closed\" }\n\
    \x20   f.close() catch { println(f\"close: {error}\") }\n\
    \x20   println(\"after close\")\n\
    }\n\
    \n\
    fn main() {\n\
    \x20   quiet() catch { println(f\"quiet: {error}\") }\n\
    \x20   loud(1) catch { println(f\"loud: {error.full()}\") }\n\
    \x20   closing()\n\
    }\n";

/// **D3: a failing cleanup is the function's failure**, a secondary one when
/// the function already failed, and `close()` hands it back at the call with
/// nothing parked after it.
#[test]
fn a_failing_cleanup_fails_the_function_or_joins_its_failure() {
    for how in [Build::default(), Build::parallel()] {
        let (out, err, ok) = ran("flaky", FLAKY, how);
        assert!(ok, "{err}");
        assert!(
            out.starts_with(
                "using quiet\nquiet: cleaning up a `Flaky` failed: quiet would not flush\n"
            ),
            "at {how:?}: {out}"
        );
        assert!(
            out.contains("loud: the body would not flush"),
            "at {how:?}: {out}"
        );
        assert!(out.contains("loud would not flush"), "at {how:?}: {out}");
        assert!(
            out.ends_with(
                "close: cleaning up a `Flaky` failed: closed would not flush\nafter close\n"
            ),
            "at {how:?}: {out}"
        );
    }
}

/// **A settle point that cannot fail leaves the function's channel alone**: a
/// function that raises its own error and holds a value whose cleanup cannot
/// fail keeps the error it raised, typed as it was.
#[test]
fn a_cleanup_that_cannot_fail_leaves_a_typed_channel_as_it_is() {
    let source = "enum Refused {\n\
        \x20   Because(String),\n\
        }\n\
        \n\
        impl Error for Refused {\n\
        \x20   fn message(ref self) -> String {\n\
        \x20       match self {\n\
        \x20           Refused::Because(why) => f\"refused: {why}\"\n\
        \x20       }\n\
        \x20   }\n\
        }\n\
        \n\
        struct Noted {\n\
        \x20   name: String,\n\
        }\n\
        \n\
        impl Cleanup for Noted {\n\
        \x20   fn cleanup(ref mut self) {\n\
        \x20       println(f\"cleanup {self.name}\")\n\
        \x20   }\n\
        }\n\
        \n\
        fn guarded(n: i64) -> i64 throws {\n\
        \x20   let held = Noted { name: f\"guard\" }\n\
        \x20   if n > 1 {\n\
        \x20       throw Refused::Because(f\"{n}\")\n\
        \x20   }\n\
        \x20   return n + held.name.len()\n\
        }\n\
        \n\
        fn main() {\n\
        \x20   let a = guarded(1) catch { 0 }\n\
        \x20   let b = guarded(2) catch {\n\
        \x20       println(f\"{error}\")\n\
        \x20       0\n\
        \x20   }\n\
        \x20   println(f\"{a} {b}\")\n\
        }\n";
    for how in [Build::default(), Build::parallel()] {
        let (out, err, ok) = ran("typed", source, how);
        assert!(ok, "{err}");
        assert_eq!(
            out, "cleanup guard\ncleanup guard\nrefused: 2\n6 0\n",
            "at {how:?}"
        );
    }
}

const WALKED: &str = "struct Noted {\n\
    \x20   name: String,\n\
    }\n\
    \n\
    impl Cleanup for Noted {\n\
    \x20   fn cleanup(ref mut self) {\n\
    \x20       println(f\"cleanup {self.name}\")\n\
    \x20   }\n\
    }\n\
    \n\
    impl Drop for Noted {\n\
    \x20   fn drop(ref mut self) {\n\
    \x20       println(f\"drop {self.name}\")\n\
    \x20   }\n\
    }\n\
    \n\
    fn main() {\n\
    \x20   let xs: Vec[i64] = [1, 2]\n\
    \x20   let ys: Vec[i64] = xs.iter().map(fn(x) {\n\
    \x20       let n = Noted { name: f\"{x}\" }\n\
    \x20       return x + 1\n\
    \x20   }).collect()\n\
    \x20   println(f\"mapped {ys.len()}\")\n\
    \x20   let t = spawn fn {\n\
    \x20       let n = Noted { name: f\"task\" }\n\
    \x20       println(f\"in {n.name}\")\n\
    \x20   }\n\
    \x20   t.join()\n\
    \x20   println(\"end\")\n\
    }\n";

/// **A lambda's body is a block like any other**, and a task's: the value
/// dies at its closing brace and is cleaned up there, and a type's own
/// `impl Drop` runs after its cleanup.
#[test]
fn a_lambda_and_a_task_clean_up_what_dies_in_them() {
    for how in [Build::default(), Build::parallel()] {
        let (out, err, ok) = ran("walked", WALKED, how);
        assert!(ok, "{err}");
        assert_eq!(
            out,
            "cleanup 1\ndrop 1\ncleanup 2\ndrop 2\nmapped 2\nin task\ncleanup task\ndrop task\nend\n",
            "at {how:?}"
        );
    }
}

/// **A panic runs no pausable cleanup** (Part I 6.4): only the synchronous
/// `drop`, on the way down.
#[test]
fn a_panic_runs_only_the_drop() {
    let source = WALKED.replace(
        "    t.join()\n",
        "    t.join()\n    let p = Noted { name: f\"p\" }\n    let none: i64? = null\n    let v = none ?? panic(\"boom\")\n",
    );
    for how in [Build::default(), Build::parallel()] {
        let (out, err, ok) = ran("panicked", &source, how);
        assert!(!ok, "{out}");
        assert!(err.contains("boom"), "{err}");
        assert!(out.ends_with("drop task\ndrop p\n"), "at {how:?}: {out}");
    }
}

const REFUSED: &str = "struct Flaky {\n\
    \x20   name: String,\n\
    }\n\
    \n\
    impl Cleanup for Flaky {\n\
    \x20   fn cleanup(ref mut self) throws {\n\
    \x20       println(f\"{self.name}\")\n\
    \x20   }\n\
    \x20   fn drop(ref mut self) {\n\
    \x20       println(\"x\")\n\
    \x20   }\n\
    }\n\
    \n\
    fn silent() {\n\
    \x20   let f = Flaky { name: f\"s\" }\n\
    \x20   println(f.name)\n\
    }\n\
    \n\
    fn main() {\n\
    \x20   silent()\n\
    }\n";

/// **D4: no code of its own.** The cleanup is a call the compiler writes, so
/// a failing one in a function that does not say `throws` is `NK2605`'s, and
/// the message names the value; `NK2601` is an `impl Cleanup` that is not
/// the one method, and a `drop` in it is pointed at `impl Drop`.
#[test]
fn the_refusals_name_the_value_and_the_method() {
    let found = findings(REFUSED);
    let failing: Vec<_> = found.iter().filter(|f| f.code == "NK2605").collect();
    assert_eq!(failing.len(), 1, "{found:#?}");
    assert_eq!(
        failing[0].message,
        "This function can fail, because cleaning up `f` can fail."
    );
    assert!(
        failing[0]
            .help
            .as_deref()
            .is_some_and(|h| h.contains("`f.close() catch { … }`")),
        "{found:#?}"
    );
    let shaped: Vec<_> = found.iter().filter(|f| f.code == "NK2601").collect();
    assert_eq!(shaped.len(), 1, "{found:#?}");
    assert!(
        shaped[0]
            .help
            .as_deref()
            .is_some_and(|h| h.contains("`impl Drop for Flaky")),
        "{found:#?}"
    );
}

/// **Where nothing may pause**: a value with a cleanup that dies inside a
/// door's block would hold the lock across its cleanup - `NK2202`, naming the
/// cleanup as the pause - and one that dies in a lambda handed to a `sync`
/// function type is `NK2206`'s, as any pause there is.
#[test]
fn a_cleanup_where_nothing_may_pause_is_refused_as_a_pause() {
    let source = "struct Noted {\n\
        \x20   name: String,\n\
        }\n\
        \n\
        impl Cleanup for Noted {\n\
        \x20   fn cleanup(ref mut self) {\n\
        \x20       println(f\"cleanup {self.name}\")\n\
        \x20   }\n\
        }\n\
        \n\
        fn twice(f: fn(i64) -> i64 sync) -> i64 {\n\
        \x20   return f(f(1))\n\
        }\n\
        \n\
        fn main() {\n\
        \x20   let hits = SharedMut(0)\n\
        \x20   hits.update fn(mut v) {\n\
        \x20       let held = Noted { name: f\"door\" }\n\
        \x20       v += 1\n\
        \x20   }\n\
        \x20   let n = twice(fn(x) {\n\
        \x20       let kept = Noted { name: f\"{x}\" }\n\
        \x20       return x + 1\n\
        \x20   })\n\
        \x20   println(f\"{n} {hits.get()}\")\n\
        }\n";
    let found = findings(source);
    let named = |code: &str, name: &str| {
        found.iter().any(|f| {
            f.code == code && (f.message.contains(name) || f.notes.iter().any(|n| n.contains(name)))
        })
    };
    assert!(named("NK2202", "The cleanup of `held`"), "{found:#?}");
    assert!(found.iter().any(|f| f.code == "NK2206"), "{found:#?}");
}

const HELD: &str = "struct Noted {\n\
    \x20   name: String,\n\
    }\n\
    \n\
    impl Cleanup for Noted {\n\
    \x20   fn cleanup(ref mut self) {\n\
    \x20       println(f\"cleanup {self.name}\")\n\
    \x20   }\n\
    }\n\
    \n\
    struct Holder {\n\
    \x20   inner: Noted,\n\
    }\n\
    \n\
    fn keep(n: Noted) -> i64 {\n\
    \x20   let all: Vec[Noted] = [n]\n\
    \x20   println(\"kept\")\n\
    \x20   return all.len()\n\
    }\n\
    \n\
    fn main() {\n\
    \x20   if true {\n\
    \x20       let h = Holder { inner: Noted { name: f\"held\" } }\n\
    \x20       println(f\"holding {h.inner.name}\")\n\
    \x20   }\n\
    \x20   let k = keep(Noted { name: f\"given\" })\n\
    \x20   println(f\"after {k}\")\n\
    }\n";

/// **What holds a value with a cleanup dies with it**, a field and an element
/// alike, and a parameter the callee keeps dies in the callee.
#[test]
fn a_holder_and_a_kept_parameter_are_settled_where_they_die() {
    for how in [Build::default(), Build::parallel()] {
        let (out, err, ok) = ran("held", HELD, how);
        assert!(ok, "{err}");
        assert_eq!(
            out, "holding held\ncleanup held\nkept\ncleanup given\nafter 1\n",
            "at {how:?}"
        );
    }
}

const CANCELLED: &str = "use std::time\n\
    \n\
    struct Slow {\n\
    \x20   name: String,\n\
    \x20   wait: i64,\n\
    }\n\
    \n\
    impl Cleanup for Slow {\n\
    \x20   fn cleanup(ref mut self) {\n\
    \x20       time::sleep(self.wait.millis())\n\
    \x20       println(f\"cleanup {self.name}\")\n\
    \x20   }\n\
    }\n\
    \n\
    fn main() {\n\
    \x20   let handle = spawn fn {\n\
    \x20       let s = Slow { name: f\"cancelled\", wait: WAIT }\n\
    \x20       time::sleep(5000.millis())\n\
    \x20       println(f\"the task finished {s.name}\")\n\
    \x20   }\n\
    \x20   time::sleep(20.millis())\n\
    \x20   handle.cancel()\n\
    \x20   println(\"main finished\")\n\
    }\n";

/// **D5: a cancelled task's cleanup is adopted**, and finished before the
/// program ends: the task stops at its pause point, its value dies there, and
/// the runtime runs what it parked after `main` is done.
#[test]
fn a_cancelled_tasks_cleanup_is_finished_before_the_program_ends() {
    let source = CANCELLED.replace("WAIT", "10");
    for how in [Build::default(), Build::parallel()] {
        let (out, err, status) = ran_with("cancelled", &source, how, None);
        assert_eq!(status, Some(0), "{err}");
        assert_eq!(out, "main finished\ncleanup cancelled\n", "at {how:?}");
    }
}

/// **D5: a cleanup the deadline cut off is named**, with exit status 70 and
/// on the panic path, which is what ADR-112 D2 asked for and could only count.
#[test]
fn a_cleanup_the_deadline_cut_off_is_named_with_exit_70() {
    let source = CANCELLED.replace("WAIT", "5000");
    for how in [Build::default(), Build::parallel()] {
        let (out, err, status) = ran_with(
            "cut-off",
            &source,
            how,
            Some("cleanup-deadline = \"100ms\"\n"),
        );
        assert_eq!(status, Some(70), "at {how:?}: {out}\n{err}");
        assert_eq!(out, "main finished\n", "at {how:?}");
        assert!(
            err.contains("the cleanup of a `Slow` did not finish within the 0.1s cleanup deadline"),
            "at {how:?}: {err}"
        );
    }
}

const BRANCHES: &str = "use std::time\n\
    \n\
    enum FlushError {\n\
    \x20   Refused(String),\n\
    }\n\
    \n\
    impl Error for FlushError {\n\
    \x20   fn message(ref self) -> String {\n\
    \x20       match self {\n\
    \x20           FlushError::Refused(name) => f\"{name} would not flush\"\n\
    \x20       }\n\
    \x20   }\n\
    }\n\
    \n\
    struct Flaky {\n\
    \x20   name: String,\n\
    }\n\
    \n\
    impl Cleanup for Flaky {\n\
    \x20   fn cleanup(ref mut self) throws {\n\
    \x20       throw FlushError::Refused(f\"{self.name}\")\n\
    \x20   }\n\
    }\n\
    \n\
    struct Noted {\n\
    \x20   name: String,\n\
    }\n\
    \n\
    impl Cleanup for Noted {\n\
    \x20   fn cleanup(ref mut self) {\n\
    \x20       println(f\"cleanup {self.name}\")\n\
    \x20   }\n\
    }\n\
    \n\
    fn refuse(what: String) -> i64 throws {\n\
    \x20   throw FlushError::Refused(what)\n\
    }\n\
    \n\
    fn both() -> i64 throws {\n\
    \x20   let pair = overlap {\n\
    \x20       {\n\
    \x20           let f = Flaky { name: f\"first\" }\n\
    \x20           refuse(f\"the first branch\")\n\
    \x20       }\n\
    \x20       {\n\
    \x20           let g = Flaky { name: f\"second\" }\n\
    \x20           2\n\
    \x20       }\n\
    \x20   }\n\
    \x20   return pair.0 + pair.1\n\
    }\n\
    \n\
    fn quiet() -> i64 {\n\
    \x20   let pair = overlap {\n\
    \x20       {\n\
    \x20           let a = Noted { name: f\"a\" }\n\
    \x20           time::sleep(20.millis())\n\
    \x20           println(\"a done\")\n\
    \x20           1\n\
    \x20       }\n\
    \x20       {\n\
    \x20           let b = Noted { name: f\"b\" }\n\
    \x20           println(\"b done\")\n\
    \x20           2\n\
    \x20       }\n\
    \x20   }\n\
    \x20   return pair.0 + pair.1\n\
    }\n\
    \n\
    fn main() {\n\
    \x20   println(f\"quiet {quiet()}\")\n\
    \x20   let n = both() catch {\n\
    \x20       println(f\"{error.full()}\")\n\
    \x20       0\n\
    \x20   }\n\
    \x20   println(f\"both {n}\")\n\
    }\n";

/// **Each `overlap` branch settles what it parks** (D3, ADR-115 D3): the
/// branches run at once in one task, and each has a queue of its own, so a
/// branch's values are cleaned up at its own end. A cleanup that fails while
/// its branch is failing joins **that branch's** error, and a later branch's
/// failure joins the winner's list after it - also where the first error
/// arrived in the box from a callee's typed channel, which has no envelope of
/// its own to hold a list.
#[test]
fn an_overlap_branch_settles_its_own_and_joins_its_own_error() {
    for how in [Build::default(), Build::parallel()] {
        let (out, err, ok) = ran("branches", BRANCHES, how);
        assert!(ok, "{err}");
        assert!(
            out.starts_with("b done\ncleanup b\na done\ncleanup a\nquiet 3\n"),
            "at {how:?}: {out}"
        );
        let first = out.find("the first branch would not flush").expect(&out);
        let own = out
            .find("cleaning up a `Flaky` failed: first would not flush")
            .expect(&out);
        let later = out
            .find("cleaning up a `Flaky` failed: second would not flush")
            .expect(&out);
        assert!(first < own && own < later, "at {how:?}: {out}");
        // **Each with its site** (ADR-240): the first error was raised in
        // `refuse`'s typed channel and keeps the site across the `?` into the
        // box; the cleanups say where they ran, and the note about a trace is
        // said once, at the top.
        assert!(
            out.contains("the first branch would not flush\n  raised at refuse\n"),
            "at {how:?}: {out}"
        );
        assert_eq!(
            out.matches("    raised at both\n").count(),
            2,
            "at {how:?}: {out}"
        );
        assert_eq!(out.matches("(no trace;").count(), 1, "at {how:?}: {out}");
        assert!(out.ends_with("both 0\n"), "at {how:?}: {out}");
    }
}

const LAMBDA_FAILS: &str = "enum FlushError {\n\
    \x20   Refused(String),\n\
    }\n\
    \n\
    impl Error for FlushError {\n\
    \x20   fn message(ref self) -> String {\n\
    \x20       match self {\n\
    \x20           FlushError::Refused(name) => f\"{name} would not flush\"\n\
    \x20       }\n\
    \x20   }\n\
    }\n\
    \n\
    struct Flaky {\n\
    \x20   name: String,\n\
    }\n\
    \n\
    impl Cleanup for Flaky {\n\
    \x20   fn cleanup(ref mut self) throws {\n\
    \x20       throw FlushError::Refused(f\"{self.name}\")\n\
    \x20   }\n\
    }\n\
    \n\
    fn walk(xs: Vec[i64]) -> i64 throws {\n\
    \x20   let ys: Vec[i64] = xs.iter().map(fn(x) {\n\
    \x20       let f = Flaky { name: f\"lambda {x}\" }\n\
    \x20       return x\n\
    \x20   }).collect()\n\
    \x20   return ys.len()\n\
    }\n\
    \n\
    fn main() {\n\
    \x20   let n = walk([1, 2]) catch {\n\
    \x20       println(f\"{error.full()}\")\n\
    \x20       0\n\
    \x20   }\n\
    \x20   println(f\"walked {n}\")\n\
    }\n";

/// **A failure is never settled away** (D3): a value that dies in a lambda's
/// body has no channel to fail into there, so it stays parked, and the settle
/// point of the function around the lambda - which has one - runs its cleanup
/// and fails with it.
#[test]
fn a_cleanup_that_fails_in_a_lambda_fails_the_function_around_it() {
    for how in [Build::default(), Build::parallel()] {
        let (out, err, ok) = ran("lambda-fails", LAMBDA_FAILS, how);
        assert!(ok, "{err}");
        assert!(
            out.contains("lambda 1 would not flush"),
            "at {how:?}: {out}"
        );
        assert!(
            out.contains("lambda 2 would not flush"),
            "at {how:?}: {out}"
        );
        assert!(out.ends_with("walked 0\n"), "at {how:?}: {out}");
    }
}
