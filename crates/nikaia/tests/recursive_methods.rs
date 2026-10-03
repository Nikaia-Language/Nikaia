//! A recursive **pausing method** is boxed
//! ([ADR-055](../../../docs/specification/adr/adr-055.md) §6, step 2's
//! remainder).
//!
//! A recursive `async fn` is an infinitely sized future, so a call that closes
//! a cycle of pausing functions puts the future behind a pointer. That was
//! built for a call the emitter can **name** — a free function, resolved the
//! way it resolves anything. A method is not one: `stats.add(5)` names `add`,
//! and only the type checker knows what it goes to
//! ([ADR-288](../../../docs/specification/adr/adr-288.md)), so a cycle through
//! one reached `rustc` as *recursion in an async fn requires boxing*, about a
//! file nobody wrote — [Part III
//! C.1](../../../docs/specification/30-nikaia-tooling.md).

mod common;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Compile the lowering, which is the only thing that says the box is there
/// *and* is the right shape.
fn compiles(purpose: &str, source: &str) -> String {
    let rust = lowered(source);
    let dir = common::scratch_dir(&format!("recursive-{purpose}"));
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let out = dir.join("program");
    let done = common::compile(
        &path,
        &["--crate-type", "bin", "-o", out.to_str().expect("utf-8")],
    );
    assert!(
        done.status.success(),
        "{purpose} did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&done.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
    rust
}

/// **The line this was written for.** A pausing method that calls itself.
#[test]
fn a_method_that_calls_itself_and_pauses_is_boxed() {
    let rust = compiles(
        "direct",
        "use std::fs\n\nstruct Node { n: i64 }\n\
         \n\
         impl Node {\n\
         \x20   fn walk(ref self, depth: i64) -> i64 {\n\
         \x20       if depth == 0 { return self.n }\n\
         \x20       let text = fs::read_to_string(\"x\", fs::Root::Anywhere) catch { return 0 }\n\
         \x20       return self.walk(depth - 1) + (text.len() as i64)\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let node = Node { n: 1 }\n\
         \x20   println(f\"{node.walk(0)}\")\n\
         }\n",
    );
    assert!(rust.contains("Box::pin(self.walk("), "{rust}");
}

/// **And a cycle of two**, which is the shape that says the answer comes from
/// the reachability graph rather than from the name being the same.
#[test]
fn a_cycle_of_two_pausing_methods_is_boxed() {
    let rust = compiles(
        "mutual",
        "use std::fs\n\nstruct Node { n: i64 }\n\
         \n\
         impl Node {\n\
         \x20   fn down(ref self, depth: i64) -> i64 {\n\
         \x20       if depth == 0 { return self.n }\n\
         \x20       let text = fs::read_to_string(\"x\", fs::Root::Anywhere) catch { return 0 }\n\
         \x20       return self.up(depth - 1) + (text.len() as i64)\n\
         \x20   }\n\
         \x20   fn up(ref self, depth: i64) -> i64 {\n\
         \x20       if depth == 0 { return 0 }\n\
         \x20       return self.down(depth - 1)\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let node = Node { n: 1 }\n\
         \x20   println(f\"{node.down(0)}\")\n\
         }\n",
    );
    assert!(rust.contains("Box::pin("), "{rust}");
}

/// **A method that does not close a cycle is not boxed**, which is the half
/// that says this is a measurement and not a pointer on every method call: a
/// box is an allocation per call, and one that buys nothing is one nobody asked
/// for ([ADR-040](../../../docs/specification/adr/adr-040.md) D1's polarity).
#[test]
fn a_pausing_method_that_does_not_recur_is_not_boxed() {
    let rust = compiles(
        "straight",
        "use std::fs\n\nstruct Node { n: i64 }\n\
         \n\
         impl Node {\n\
         \x20   fn read(ref self) -> i64 {\n\
         \x20       let text = fs::read_to_string(\"x\", fs::Root::Anywhere) catch { return 0 }\n\
         \x20       return text.len() as i64\n\
         \x20   }\n\
         \x20   fn twice(ref self) -> i64 { return self.read() + self.read() }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let node = Node { n: 1 }\n\
         \x20   println(f\"{node.twice()}\")\n\
         }\n",
    );
    assert!(!rust.contains("Box::pin("), "{rust}");
}

/// **And a recursive method that never pauses is not boxed either**, because
/// the future it would box does not exist: a plain `fn` recurs the way it
/// always has.
#[test]
fn a_recursive_sync_method_is_left_alone() {
    let rust = compiles(
        "sync",
        "struct Node { n: i64 }\n\
         \n\
         impl Node {\n\
         \x20   fn count(ref self, depth: i64) -> i64 sync {\n\
         \x20       if depth == 0 { return self.n }\n\
         \x20       return self.count(depth - 1) + 1\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let node = Node { n: 1 }\n\
         \x20   println(f\"{node.count(2)}\")\n\
         }\n",
    );
    assert!(!rust.contains("Box::pin("), "{rust}");
}

/// **The free-function case still works**, which is what step 2 built and what
/// this must not disturb: `examples/json/src/main.nika`'s recursion is through free
/// functions.
#[test]
fn a_recursive_pausing_function_is_still_boxed() {
    let rust = compiles(
        "free",
        "use std::fs\n\nfn walk(depth: i64) -> i64 {\n\
         \x20   if depth == 0 { return 0 }\n\
         \x20   let text = fs::read_to_string(\"x\", fs::Root::Anywhere) catch { return 0 }\n\
         \x20   return walk(depth - 1) + (text.len() as i64)\n\
         }\n\
         \n\
         fn main() { println(f\"{walk(0)}\") }\n",
    );
    assert!(rust.contains("Box::pin(walk("), "{rust}");
}
