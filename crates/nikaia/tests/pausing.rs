//! **`async` where a function can pause** — [ADR-055](../../../docs/specification/adr/adr-055.md)
//! §6 step 2, on the emitted Rust and on the program it becomes.
//!
//! The language was specified as implicitly async from ADR-005 on, and until
//! this step the emitted Rust contained the word `async` zero times. What the
//! tests here hold to is that the *ledger* decides: D1 makes a function `async`
//! where `sync` says it can pause, D2 puts an `.await` at every call to one, and
//! nothing in the `.nika` source says either word (`runtime.rs`'s
//! `no_nika_file_says_async_or_names_a_mechanism` is that half).
//!
//! `sync` is inferred, per function, over the call graph, as a **greatest**
//! fixpoint ([ADR-027](../../../docs/specification/adr/adr-027.md) D1) - so
//! `plain` below is `sync` because everything it calls is, and `pausing` is not
//! because `io::read` can pause. Neither writes the word.

mod common;

use std::path::Path;
use std::process::Command;

/// Lower `source` and hand back the emitted Rust.
fn lower(dir: &Path, source: &str) -> String {
    let input = dir.join("main.nika");
    std::fs::write(&input, source).expect("the source");
    let output = dir.join("main.rs");
    let run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["--input", input.to_str().unwrap()])
        .args(["--backend", "rust"])
        .args(["--output", output.to_str().unwrap()])
        .args(["--no-cache"])
        .env("NIKAIA_CACHE_DIR", dir.join("cache"))
        .output()
        .expect("the nikaia binary runs");
    assert!(
        run.status.success(),
        "lowering failed:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    std::fs::read_to_string(&output).expect("the emitted Rust")
}

/// One function that can pause and one that cannot, and nothing saying so.
const BOTH_KINDS: &str = "use std::io\n\
     \n\
     fn plain(n: i64) -> i64 {\n\
     \x20   return n * 2\n\
     }\n\
     \n\
     fn pausing() -> i64 throws {\n\
     \x20   let text = io::read()\n\
     \x20   return text.len()\n\
     }\n\
     \n\
     fn main() throws {\n\
     \x20   println(f\"{plain(21)} {pausing()}\")\n\
     }";

/// D1: the `sync` column decides, and only the functions that can pause become
/// `async`.
///
/// The negative half is the one that carries: if everything became `async` the
/// step would be free and would also be a lie about what a call costs. `plain`
/// is `sync` by inference - nobody wrote the word - and it stays a plain `fn`.
#[test]
fn only_the_functions_that_can_pause_are_async() {
    let dir = common::scratch_dir("pausing-d1");
    let rust = lower(&dir, BOTH_KINDS);

    assert!(rust.contains("fn plain(n: i64) -> i64"), "{rust}");
    assert!(!rust.contains("async fn plain"), "{rust}");
    assert!(rust.contains("async fn pausing()"), "{rust}");

    std::fs::remove_dir_all(&dir).ok();
}

/// D2: the `.await` goes **before** the `?`, and the order is not a choice.
///
/// The future is what can fail, so it has to be driven before there is a
/// `Result` to propagate: `pausing().await?` and never `pausing()?.await`. The
/// second does not compile, so this is the difference between a step that works
/// and a `rustc` error about a file nobody wrote (Part III, C.1).
#[test]
fn the_await_goes_before_the_question_mark() {
    let dir = common::scratch_dir("pausing-order");
    let rust = lower(&dir, BOTH_KINDS);

    assert!(rust.contains("pausing().await?"), "{rust}");
    assert!(!rust.contains("pausing()?.await"), "{rust}");
    // A call to the function that cannot pause takes neither.
    assert!(rust.contains("plain(21)"), "{rust}");
    assert!(!rust.contains("plain(21).await"), "{rust}");

    std::fs::remove_dir_all(&dir).ok();
}

/// ADR-038 D4 and D1 together: the program's own `main` is driven by the
/// executor rather than called.
///
/// `fn main` is Rust's and is the runtime's to write; the program's own entry
/// point is now a future, so what calls it is `block_on` - the `no` half of the
/// executor §6 step 1 built.
#[test]
fn a_pausing_main_is_driven_by_the_executor() {
    let dir = common::scratch_dir("pausing-main");
    let rust = lower(&dir, BOTH_KINDS);

    assert!(rust.contains("async fn __nikaia_main()"), "{rust}");
    assert!(
        rust.contains("nikaia_std::rt::exec::block_on(__nikaia_main())"),
        "{rust}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A `main` that cannot pause is still called and not driven.
///
/// The same negative half as `only_the_functions_that_can_pause_are_async`, at
/// the one place where getting it wrong would cost every program an executor it
/// has no use for.
#[test]
fn a_main_that_cannot_pause_is_called_directly() {
    let dir = common::scratch_dir("pausing-plain-main");
    let rust = lower(&dir, "fn main() {\n\x20   println(f\"{1 + 1}\")\n}");

    assert!(!rust.contains("async fn __nikaia_main"), "{rust}");
    assert!(!rust.contains("block_on"), "{rust}");
    assert!(rust.contains("__nikaia_main()"), "{rust}");

    std::fs::remove_dir_all(&dir).ok();
}

/// D6: a recursive pausing function is boxed, because a recursive `async fn` is
/// an infinitely sized future.
///
/// `examples/json/src/main.nika` is where this was found rather than reasoned about: its
/// `show` and `longest` are both recursive and both pausing, so the corpus had
/// the edge on the day the step landed. `rustc` says *"recursion in an async fn
/// requires boxing"* about the generated file, which is the C.1 class.
#[test]
fn a_recursive_pausing_call_is_boxed() {
    let dir = common::scratch_dir("pausing-recursion");
    let rust = lower(
        &dir,
        "use std::io\n\
         \n\
         fn countdown(n: i64) throws {\n\
         \x20   if n <= 0 { return }\n\
         \x20   let text = io::read()\n\
         \x20   println(f\"{n} {text.len()}\")\n\
         \x20   countdown(n - 1)\n\
         }\n\
         \n\
         fn main() throws {\n\
         \x20   countdown(3)\n\
         }",
    );

    assert!(rust.contains("async fn countdown"), "{rust}");
    assert!(rust.contains("Box::pin(countdown(n - 1)).await"), "{rust}");
    // The call from `main` closes no cycle, so it pays no allocation.
    assert!(rust.contains("countdown(3).await"), "{rust}");
    assert!(!rust.contains("Box::pin(countdown(3))"), "{rust}");

    std::fs::remove_dir_all(&dir).ok();
}

/// The emitted Rust compiles and the program runs.
///
/// The assertions above are about text, and text that compiles is a different
/// claim. This is the one that says the step works: two functions, one of each
/// kind, driven by our own executor.
#[test]
fn a_program_with_both_kinds_runs() {
    let dir = common::scratch_dir("pausing-runs");
    let rust = lower(
        &dir,
        "use std::fs\n\
         \n\
         fn plain(n: i64) -> i64 {\n\
         \x20   return n * 2\n\
         }\n\
         \n\
         fn size(path: ref String) -> i64 throws {\n\
         \x20   let text = fs::read_to_string(path, fs::Root::Anywhere)\n\
         \x20   return text.len()\n\
         }\n\
         \n\
         fn main() throws {\n\
         \x20   println(f\"{plain(21)} {size(\\\"eins.txt\\\")}\")\n\
         }",
    );
    std::fs::write(dir.join("eins.txt"), "hallo").expect("the input");

    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &["--crate-type", "bin", "-o", binary.to_str().unwrap()],
    );
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let run = Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("the program runs");
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42 5");

    std::fs::remove_dir_all(&dir).ok();
}

/// **A lambda that pauses is handed to `std`'s pausing counterpart**
/// ([ADR-233](../../../docs/specification/adr/adr-233.md) D2).
///
/// This test used to record a refusal: the `std` entry takes a synchronous
/// closure, so the lowering had nothing to write. `nikaia_std::seq` now has a
/// counterpart for each entry that takes the language below's `async` closure,
/// and the program the refusal was about - a correct one, which the checker
/// never refused (Part III, C.4) - is built. Where the lambda is handed to
/// something nothing describes, the refusal is still in Nikaia's words
/// (`refusal_lines.rs`).
#[test]
fn a_lambda_that_pauses_is_handed_to_the_pausing_counterpart() {
    let dir = common::scratch_dir("pausing-lambda");
    let rust = lower(
        &dir,
        "use std::io\n\
         \n\
         fn size() -> i64 {\n\
         \x20   let text = io::read() catch { return 0 }\n\
         \x20   return text.len()\n\
         }\n\
         \n\
         fn main(xs: Vec[i64]) {\n\
         \x20   let mut ys = xs\n\
         \x20   ys.sort_by_key fn (n) { size() }\n\
         \x20   println(f\"{ys.len()}\")\n\
         }",
    );

    assert!(
        rust.contains("nikaia_std::seq::sort_by_key(&mut ys, async |n|"),
        "{rust}"
    );
    assert!(rust.contains("size().await"), "{rust}");

    std::fs::remove_dir_all(&dir).ok();
}
