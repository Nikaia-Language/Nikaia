//! What `return` means where it is not the last thing a function does.
//!
//! Part I 7.1: `return` leaves the **function**. Nikaia's blocks are
//! expressions (Part I 3.1), so a `return` can sit at the end of something that
//! is itself a value - a `match` arm, an `if` branch in value position, a
//! `catch` handler, a `seq` block - and there the two readings come apart: the
//! value of the expression around it, or the value of the function past it. The
//! language has one answer and it is the second.
//!
//! **These are run and not read, wherever there is something to run.** The
//! lowering turned `1 => { return 10 }` into `1 => { 10 }`, which is not a
//! broken-looking program: where the types line up it compiles, computes
//! something, and is simply a different program from the one that was written.
//! A test that compared emitted text would have been asserting the emitter
//! agrees with itself. So the shapes are run, in
//! `tests/language/src/returns.nika`; the one here that reads a file is run
//! here, and the rest are read, each saying why.

mod common;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Lower, compile, run, and hand back what it printed.
fn output_of(purpose: &str, source: &str, files: &[(&str, &str)]) -> String {
    let dir = common::scratch_dir(purpose);
    let rust = dir.join(format!("{purpose}.rs"));
    std::fs::write(&rust, lowered(source)).expect("write the Rust");
    for (name, body) in files {
        std::fs::write(dir.join(name), body).expect("write a fixture file");
    }

    let binary = dir.join(purpose);
    let built = common::compile(&rust, &["-o", &binary.to_string_lossy()]);
    assert!(
        built.status.success(),
        "the lowering of {purpose} does not compile:\n{}\n--- the Rust ---\n{}",
        String::from_utf8_lossy(&built.stderr),
        lowered(source)
    );

    let run = std::process::Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run it");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let printed = String::from_utf8_lossy(&run.stdout).to_string();
    // The scratch directory holds a linked binary, and the corpus runs often.
    // Removed here and not before the assertions above: a failing test's
    // artefacts are what diagnoses it.
    let _ = std::fs::remove_dir_all(&dir);
    printed
}

/// A block used as a value, and a `seq` block - which is a block that has
/// withdrawn a reordering permission and nothing else (ADR-292 D7). Both had the
/// same defect.
///
/// **Read rather than run, and the shape is the reason.** For a block's *last*
/// statement to be a `return`, the block has to leave the function every time it
/// is entered - so the value it was written to hand back is never produced and
/// there is nothing to print. Before the fix the block handed back the returned
/// expression and the program ran on; now it returns, and a `let` bound to such a
/// block is a `let` bound to nothing, which rustc says so about. That is the one
/// emission in this file whose *verdict* changes, and it changes from "compiles,
/// and is a different program" to an honest complaint about dead code.
///
/// It used to check `seq { … }` beside the bare block, because the two were the
/// same shape with different meanings. `seq` is withdrawn
/// ([ADR-292](../../../docs/specification/adr/adr-292.md) D7) and the bare block
/// is the shape that remains.
const BLOCK_AS_VALUE: &str = "fn pick() -> i64 {\n\
     \x20   let y = {\n\
     \x20       return 2\n\
     \x20   }\n\
     \x20   3\n\
     }\n";

#[test]
fn a_return_in_a_block_used_as_a_value_leaves_the_function() {
    let rust = lowered(BLOCK_AS_VALUE);
    assert!(rust.contains("let y = { return 2; };"), "{rust}");
}

/// **A `catch` handler, which is the one the ordering analysis has an opinion
/// about.**
///
/// [ADR-292](../../../docs/specification/adr/adr-292.md) is about exactly this
/// handler: one that can `return` makes the *next* statement conditional on the
/// guarded operation having succeeded, so the two may not be overlapped.
/// `contracts::order`'s `diverts` counts the `return` at the end of the handler
/// when it refuses - while the emitter was dropping it, which made the analysis
/// reason about a control flow the emitted program did not have. The handler's
/// value and the read's value agree on a type here, so nothing complained.
const CATCH_HANDLER: &str = "use std::fs\n\
     \n\
     fn label(path: String) -> String throws {\n\
     \x20   let text = fs::read_to_string(ref path, fs::Root::Anywhere) catch { return \"missing\" }\n\
     \x20   f\"read {text.len()} bytes\"\n\
     }\n\
     \n\
     fn main() {\n\
     \x20   let here = label(\"there.txt\") catch { \"failed\" }\n\
     \x20   println(here)\n\
     \x20   let gone = label(\"absent.txt\") catch { \"failed\" }\n\
     \x20   println(gone)\n\
     }\n";

#[test]
fn a_return_in_a_catch_handler_leaves_the_function() {
    assert_eq!(
        output_of("return-catch", CATCH_HANDLER, &[("there.txt", "hallo")]),
        "read 5 bytes\nmissing\n"
    );
}

/// … and the analysis and the lowering now agree about that handler: `diverts`
/// says it leaves the function, and the emitted Rust says so too.
#[test]
fn the_ordering_analysis_and_the_lowering_agree_about_a_diverting_handler() {
    let rust = lowered(CATCH_HANDLER);
    assert!(
        rust.contains("return Ok(String::from(\"missing\")"),
        "the handler's `return` is gone from the lowering:\n{rust}"
    );
}

/// The rewrite that *is* right, and which none of this may take away: a
/// function body ending in `return x` is written as `{ x }`, because the value a
/// function body ends in is what the function hands back.
#[test]
fn a_return_at_the_end_of_a_function_is_still_its_value() {
    let rust = lowered(
        "fn f(c: bool) -> i64 {\n    if c {\n        return 1\n    } else {\n        return 2\n    }\n}\n",
    );
    assert!(rust.contains("if c { 1 } else { 2 }"), "{rust}");
    assert!(!rust.contains("return"), "{rust}");
}

/// A loop body is not a value, and a `return` in one was always kept. Pinned
/// because the fix moved the code that decides it.
#[test]
fn a_return_in_a_loop_body_is_kept() {
    let rust = lowered(
        "fn f(n: i64) -> i64 {\n    for i in 0..<n {\n        if i > 2 {\n            return i\n        }\n    }\n    0\n}\n",
    );
    assert!(rust.contains("return i;"), "{rust}");
}

/// A lambda's `return` leaves the **lambda**, so its body's last statement is
/// its own value and the rewrite applies there as it does to a function.
#[test]
fn a_return_at_the_end_of_a_lambda_is_the_lambdas_value() {
    let rust = lowered(
        "fn f(xs: Vec[i64]) -> i64 {\n    let y = xs.map fn (n) { return n + 1 }\n    0\n}\n",
    );
    assert!(rust.contains("|n| { n + 1 }"), "{rust}");
}
