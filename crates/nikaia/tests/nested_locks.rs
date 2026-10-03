//! A lock taken while a lock is held is refused
//! ([ADR-281](../../../docs/specification/adr/adr-281.md) D28, `NK2203`).
//!
//! Part II 12.3 called manual nesting an anti-pattern and *"often a
//! compile-time error"*. D2 makes *often* into **always**, and with that no
//! program exists in which a lock's two representations behave differently —
//! which is the whole reason `user_parallelism` may pick one.
//!
//! **The chain is what needed a column.** A second lock written inside the
//! block is one line to see; one reached three calls deep is not, and
//! `contracts::locks` propagates *touches a lock* over the call graph so that
//! both are the same answer here.
//!
//! **A `println` is one of these**, and it is the case
//! [ADR-288](../../../docs/specification/adr/adr-288.md) D21 was written about:
//! it never pauses, so `sync` says nothing about it, and it takes standard
//! output's own lock while yours is open. Two conditions and not one.

mod common;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

fn refused(source: &str) -> bool {
    findings(source).iter().any(|f| f.code == "NK2203")
}

/// **The specification's own example.** Part II 12.2 writes it as a comment
/// saying *Compiler Error*; it is one now.
#[test]
fn a_print_inside_a_door_is_refused() {
    assert!(refused(
        "fn main() {\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   counter.access fn(n) { println(f\"{n}\") }\n\
         }\n"
    ));
}

/// **A second lock written inside the block**, which is D2's first shape.
#[test]
fn a_second_lock_inside_the_block_is_refused() {
    assert!(refused(
        "fn main() {\n\
         \x20   let a = SharedMut(0)\n\
         \x20   let b = SharedMut(1)\n\
         \x20   a.access fn(x) { let y = b.get() }\n\
         }\n"
    ));
}

/// **And one reached through a chain of calls**, which is the shape the column
/// exists for: nothing about `helper` says *lock* where it is called.
#[test]
fn a_lock_three_calls_deep_is_refused() {
    assert!(refused(
        "fn inner(b: SharedMut[i64]) -> i64 { return b.get() }\n\
         fn helper(b: SharedMut[i64]) -> i64 { return inner(b) }\n\
         fn main() {\n\
         \x20   let a = SharedMut(0)\n\
         \x20   let b = SharedMut(1)\n\
         \x20   a.access fn(x) { let y = helper(b) }\n\
         }\n"
    ));
}

/// **`update`'s block holds one open too**, and so is asked the same question.
#[test]
fn the_write_door_holds_one_open() {
    assert!(refused(
        "fn main() {\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   counter.update fn(mut n) { println(f\"{n}\") }\n\
         }\n"
    ));
}

/// **`get` and `set` do not**, and [ADR-281](../../../docs/specification/adr/adr-281.md)
/// D10 says why: while the lock is open in either of them no code of the
/// program's runs, so there is nothing that could take a second one.
#[test]
fn get_and_set_hold_nothing_open() {
    assert!(!refused(
        "fn main() {\n\
         \x20   let a = SharedMut(0)\n\
         \x20   let b = SharedMut(1)\n\
         \x20   a.set(b.get())\n\
         \x20   println(f\"{a.get()}\")\n\
         }\n"
    ));
}

/// **Outside a door nothing is refused**, which is the half that says the rule
/// is about the block and not about the call.
#[test]
fn the_same_calls_outside_a_door_are_a_program() {
    assert!(!refused(
        "fn main() {\n\
         \x20   let a = SharedMut(0)\n\
         \x20   let b = SharedMut(1)\n\
         \x20   println(f\"{a.get()} {b.get()}\")\n\
         }\n"
    ));
}

/// **A block that computes and nothing else is a program**, which is what the
/// door is for: lock, compute, unlock.
#[test]
fn a_block_that_only_computes_is_a_program() {
    assert!(!refused(
        "fn double(n: i64) -> i64 sync { return n * 2 }\n\
         fn main() {\n\
         \x20   let counter = SharedMut(2)\n\
         \x20   counter.update fn(mut n) { n = double(n) }\n\
         }\n"
    ));
}

/// **Doubt refuses nothing.** `Undecided` is not permission
/// ([ADR-010](../../../docs/specification/adr/adr-010.md) D1) and not a refusal
/// either: Stage 0 knows the type of rather less than half of what a program
/// writes, and refusing on doubt is [Part III
/// C.4](../../../docs/specification/30-nikaia-tooling.md)'s correct program
/// refused. What silence costs is the runtime check, which is where every
/// program already is.
#[test]
fn a_callee_nothing_describes_refuses_nothing() {
    assert!(!refused(
        "struct Sink { n: i64 }\n\
         fn murky(sink: Sink) { sink.swallow() }\n\
         fn main() {\n\
         \x20   let a = SharedMut(0)\n\
         \x20   let sink = Sink { n: 1 }\n\
         \x20   a.access fn(x) { murky(sink) }\n\
         }\n"
    ));
}

/// **A `spawn` inside a door is not a chain**, because its body runs later and
/// elsewhere — the same split `contracts::locks` makes when it builds the
/// column ([ADR-281](../../../docs/specification/adr/adr-281.md) D29).
#[test]
fn a_task_started_inside_a_door_is_not_the_doors_reach() {
    assert!(!refused(
        "fn main() {\n\
         \x20   let a = SharedMut(0)\n\
         \x20   let b = SharedMut(1)\n\
         \x20   a.access fn(x) { let h = spawn fn { b.get() } }\n\
         }\n"
    ));
}

/// **No example is refused**, which is the corpus half of the measurement:
/// nothing in `examples/` opens a lock, so nothing there meets this rule — and
/// the first program that does will be the one that writes `SharedMut`.
#[test]
fn no_example_meets_the_rule() {
    for path in common::standalone_programs(std::path::Path::new("../../examples")) {
        if path.extension().is_none_or(|e| e != "nika") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read it");
        if parse_to_ast(&text).is_err() {
            continue;
        }
        assert!(
            !refused(&text),
            "{} is refused as a lock inside a lock",
            path.display()
        );
    }
}

/// **A lambda stored in a function field and called inside a door**
/// ([ADR-230](../../../docs/specification/adr/adr-230.md) D1, D2): what the
/// field holds is what the call reaches, so a lock the stored lambda takes is a
/// lock inside a lock - called here, or through a function that calls it.
#[test]
fn a_stored_lambda_that_takes_a_lock_is_refused_where_it_is_called_inside_one() {
    let stored = "struct Button {\n    on_click: fn() sync,\n}\n\n";
    assert!(refused(&format!(
        "{stored}fn main() {{\n\
         \x20   let a = SharedMut(0)\n\
         \x20   let n = SharedMut(1)\n\
         \x20   let button = Button {{ on_click: fn() {{ let y = n.get() }} }}\n\
         \x20   a.access fn(x) {{ button.on_click() }}\n\
         }}\n"
    )));
    assert!(refused(&format!(
        "{stored}fn press(b: ref Button) {{\n    b.on_click()\n}}\n\n\
         fn main() {{\n\
         \x20   let a = SharedMut(0)\n\
         \x20   let n = SharedMut(1)\n\
         \x20   let button = Button {{ on_click: fn() {{ let y = n.get() }} }}\n\
         \x20   a.access fn(x) {{ press(button) }}\n\
         }}\n"
    )));
}

/// **And only where one does**: a field only quiet code is stored in is called
/// inside a door as any quiet function is.
#[test]
fn a_field_only_quiet_code_is_stored_in_is_not_refused() {
    assert!(!refused(
        "struct Button {\n    on_click: fn() sync,\n}\n\n\
         fn main() {\n\
         \x20   let a = SharedMut(0)\n\
         \x20   let button = Button { on_click: fn() { let y = 1 } }\n\
         \x20   a.access fn(x) { button.on_click() }\n\
         }\n"
    ));
}

/// **Code put into a function field where the column cannot read it** - by an
/// assignment - leaves the field undecided rather than quiet (ADR-230 D1): a
/// quiet literal elsewhere does not speak for it.
#[test]
fn a_field_code_is_assigned_into_is_not_answered_by_the_literal_alone() {
    let found = findings(
        "struct Button {\n    on_click: fn() sync,\n}\n\n\
         fn main() {\n\
         \x20   let a = SharedMut(0)\n\
         \x20   let n = SharedMut(1)\n\
         \x20   let mut button = Button { on_click: fn() { let y = 1 } }\n\
         \x20   button.on_click = fn() { let y = n.get() }\n\
         \x20   a.access fn(x) { button.on_click() }\n\
         }\n",
    );
    // Undecided is not a refusal (ADR-281 D29's third value): the runtime check
    // is where this program stands, as every undecided one does.
    assert!(!found.iter().any(|f| f.code == "NK2203"), "{found:#?}");
}
