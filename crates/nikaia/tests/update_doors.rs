//! `update` takes `mut v`, changes it in place, and returns nothing
//! ([ADR-281](../../../docs/specification/adr/adr-281.md) D12) — the first steps
//! of that record.
//!
//! `kasse.update fn(mut v) { v += 100 }` is the one form. `mut` is
//! [ADR-094](../../../docs/specification/adr/adr-094.md) D3's word with its
//! meaning unchanged, one position over: the value is changed in place and the
//! caller whose value changes is the **lock**.
//!
//! **What went with it is the empty slot.** `update` used to move the value
//! out, hand it to the block by value, and move what came back in — so a block
//! that panicked left the lock `None`, a state every other door had to report.
//! D1 takes the cause away rather than the symptom: the block is handed the
//! **address**, nothing is moved out, and no slot is ever empty.
//!
//! **D2's other row is not built.** A value that fits a machine word is meant
//! to be a copy plus a compare-and-swap; here every value takes the address
//! row, so the block runs exactly once. D3 allows that outright — *may* run
//! more than once is a licence, not a requirement — and what is left is speed.

mod common;

use nikaia::contracts::LedgerOps;
use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Compile the lowering and run it, which is the only proof that the address
/// the block is handed is one the language below accepts.
fn ran(purpose: &str, source: &str) -> String {
    let rust = lowered(source);
    let dir = common::scratch_dir(&format!("update-{purpose}"));
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
    let out = Command::new(&binary).output().expect("run it");
    assert!(
        out.status.success(),
        "{purpose} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    std::fs::remove_dir_all(&dir).ok();
    printed
}

/// **Part II 12.2's own counter**, which stopped lowering the day the page was
/// rewritten to D1's form, because a lambda's parameter did not take `mut`.
#[test]
fn the_specifications_counter_compiles_and_runs() {
    let printed = ran(
        "counter",
        "fn main() {\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   counter.update fn(mut n) { n += 1 }\n\
         \x20   counter.update fn(mut n) { n += 1 }\n\
         \x20   println(f\"{counter.get()}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "2");
}

/// **A value that is not a word takes the address and is not copied**, which is
/// what the `Option` was in the way of: a ten-thousand-entry list is changed
/// where it lies.
#[test]
fn a_list_is_changed_where_it_lies() {
    let printed = ran(
        "list",
        "fn main() {\n\
         \x20   let log = SharedMut(Vec())\n\
         \x20   log.update fn(mut entries) { entries.push(1) }\n\
         \x20   log.update fn(mut entries) { entries.push(2) }\n\
         \x20   println(f\"{log.access fn(e) { e.len() }}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "2");
}

/// **The `mut` parameter is an address below**, so every mention of it is
/// dereferenced — `+=` and an assignment need it outright, and writing it
/// everywhere is one rule rather than a list of positions.
#[test]
fn a_mut_lambda_parameter_is_dereferenced() {
    let rust = lowered(
        "fn main() {\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   counter.update fn(mut n) { n += 1 }\n\
         }\n",
    );
    assert!(rust.contains("(*n) += 1"), "{rust}");
}

/// **And a lambda parameter without the word is not**, which is the half that
/// says the dereference comes from `mut` and not from being a lambda's
/// parameter: `access` hands the value where it lies and may not change it
/// ([ADR-281](../../../docs/specification/adr/adr-281.md) D11).
#[test]
fn a_plain_lambda_parameter_is_left_alone() {
    let rust = lowered(
        "fn main() {\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   println(f\"{counter.access fn(n) { n + 1 }}\")\n\
         }\n",
    );
    assert!(!rust.contains("(*n)"), "{rust}");
}

/// **`update_all` is D1 widened** ([ADR-281](../../../docs/specification/adr/adr-281.md)
/// D6): one `mut` per lock, nothing returned, both held for the whole of the
/// block.
#[test]
fn update_all_takes_one_mut_per_lock() {
    let printed = ran(
        "both",
        "fn main() {\n\
         \x20   let payer = SharedMut(100)\n\
         \x20   let payee = SharedMut(0)\n\
         \x20   update_all(payer, payee) fn(mut a, mut b) {\n\
         \x20       a -= 30\n\
         \x20       b += 30\n\
         \x20   }\n\
         \x20   println(f\"{payer.get()} {payee.get()}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "70 30");
}

/// **A lambda inside the block still means the address**, which is why the list
/// accumulates rather than replacing: a body that starts a second lambda would
/// otherwise lose the dereference for the name it captured.
#[test]
fn a_nested_lambda_keeps_the_dereference() {
    let rust = lowered(
        "fn main() {\n\
         \x20   let total = SharedMut(0)\n\
         \x20   let xs = Vec()\n\
         \x20   total.update fn(mut n) {\n\
         \x20       xs.sort_by_key fn(x) { n + x }\n\
         \x20   }\n\
         }\n",
    );
    assert!(rust.contains("(*n) + x"), "{rust}");
}

/// Every finding the checker has about a source.
fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

fn refused(code: &str, source: &str) -> bool {
    findings(source).iter().any(|f| f.code == code)
}

/// **`NK1141`: there is nothing to return.** The old shape —
/// `kasse.update fn(old) { old + 1 }` — lowered to a closure whose value is an
/// `i64` where `()` is wanted, and the answer came from `rustc` about a file
/// nobody wrote.
#[test]
fn a_block_that_hands_a_value_back_is_refused() {
    assert!(refused(
        "NK1141",
        "fn main() {\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   counter.update fn(mut n) { n + 1 }\n\
         }\n"
    ));

    // A `return` carrying one is the same mistake written the other way.
    assert!(refused(
        "NK1141",
        "fn main() {\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   counter.update fn(mut n) { return n }\n\
         }\n"
    ));

    // And D1's form is not refused, which is the half that says the rule is
    // about the value and not about the block.
    assert!(!refused(
        "NK1141",
        "fn main() {\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   counter.update fn(mut n) { n += 1 }\n\
         }\n"
    ));
}

/// **A block whose last statement is a call is left alone**, because whether a
/// call comes to a value is a question about its callee — and answering it
/// wrongly here is [Part III
/// C.4](../../../docs/specification/30-nikaia-tooling.md)'s correct program
/// refused. `log.update fn(mut v) { v.push(1) }` is the shape that must pass.
#[test]
fn a_block_that_ends_in_a_call_is_not_refused() {
    assert!(!refused(
        "NK1141",
        "fn main() {\n\
         \x20   let log = SharedMut(Vec())\n\
         \x20   log.update fn(mut v) { v.push(1) }\n\
         }\n"
    ));
}

/// **`NK1138` at a door**: D1 says a block that changes `v` without the word is
/// refused as any parameter is.
#[test]
fn a_door_block_that_changes_without_the_word_is_refused() {
    assert!(refused(
        "NK1138",
        "fn main() {\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   counter.update fn(n) { n += 1 }\n\
         }\n"
    ));
}

/// **And nowhere else**, which the corpus is what settled.
///
/// `par_fold(…, fn(acc, m) { acc.record(m) })` changes `acc` and has no `mut`,
/// and it **compiles**, because the emitter writes the word itself where it
/// recognises a fold's accumulator; `and_modify fn(tally) { tally.bump() }` is
/// the same shape one library over. A rule held for every lambda would refuse
/// both — programs that run today.
#[test]
fn a_lambda_that_is_not_a_door_is_left_alone() {
    assert!(!refused(
        "NK1138",
        "use std::collections\n\nstruct Tally { n: i64 }\n\
         impl Tally {\n\
         \x20   fn bump(ref mut self) sync { self.n += 1 }\n\
         }\n\
         fn main() {\n\
         \x20   let mut counts = collections::HashMap()\n\
         \x20   counts.entry(\"a\")\n\
         \x20       .and_modify fn (tally) { tally.bump() }\n\
         \x20       .or_insert_with fn { Tally { n: 1 } }\n\
         }\n"
    ));
}

/// **`access` is the read door and is untouched**
/// ([ADR-281](../../../docs/specification/adr/adr-281.md) D11): it hands the
/// value where it lies, may not change it, and its block hands a value back by
/// design.
#[test]
fn the_read_door_is_untouched() {
    let found = findings(
        "fn main() {\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   println(f\"{counter.access fn(n) { n + 1 }}\")\n\
         }\n",
    );
    assert!(
        !found
            .iter()
            .any(|f| f.code == "NK1141" || f.code == "NK1138"),
        "{found:#?}"
    );
}

/// **A lambda kept in a function field takes a handle of the lock it uses**
/// ([ADR-230](../../../docs/specification/adr/adr-230.md) D4, ADR-312 D5): the
/// name it was written beside stays usable on the next line. The capture used
/// to move it, and `rustc` said *use of moved value* about a file nobody wrote.
#[test]
fn a_kept_lambda_takes_its_own_handle_of_a_lock() {
    let printed = ran(
        "kept-handle",
        "struct Button {\n    on_click: fn() sync,\n}\n\n\
         fn main() {\n\
         \x20   let a = SharedMut(0)\n\
         \x20   let button = Button { on_click: fn() { a.update fn(mut n) { n += 1 } } }\n\
         \x20   button.on_click()\n\
         \x20   button.on_click()\n\
         \x20   println(f\"{a.get()}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "2");
}

/// **A function field that may pause, called inside a lambda**, is refused as a
/// pausing function is there (ADR-230 D3), and names the field.
#[test]
fn a_pausing_field_called_inside_a_lambda_is_refused_by_name() {
    let source = "struct Button {\n    on_click: fn(),\n}\n\n\
                  fn main() {\n\
                  \x20   let a = SharedMut(0)\n\
                  \x20   let button = Button { on_click: fn() { let y = 1 } }\n\
                  \x20   a.access fn(x) { button.on_click() }\n\
                  }\n";
    let parsed = nikaia::parser::parse_to_ast(source).expect("parses");
    let refused =
        nikaia::emit::emit_program(&parsed, nikaia::emit::Build::default()).expect_err("refused");
    assert!(
        refused
            .to_string()
            .contains("This lambda calls `button.on_click`, which can pause"),
        "{refused}"
    );
}
