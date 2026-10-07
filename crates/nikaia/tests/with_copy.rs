//! **`value with { field: … }`** — [ADR-118](../../../docs/specification/adr/adr-118.md),
//! Part I 4.2.
//!
//! A copy of a value with named fields changed. The braces are the struct
//! literal's, with its field list and its shorthand, so almost nothing here is
//! new: what the record adds is one postfix rule, the field resolution against
//! the **operand's** type, and a lowering to Rust's functional update.
//!
//! **The operand is the whole of what is new.** `Point { x: 1, ..p }` writes
//! the type, and this node does not carry one — so the checker records what it
//! worked out and the emitter reads it back, which is the handover a
//! `comptime`'s value already makes
//! ([ADR-296](../../../docs/specification/adr/adr-296.md) D17). A `with` this
//! compiler could not name a type for is refused rather than guessed at, and
//! that is why the emitter's lookup cannot fail.
//!
//! What a copy holds when the program runs is
//! `tests/language/src/with_copy.nika`; here are the lowering and the refusals.

use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit;
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check(&parsed, &own, &library).findings
}

fn one(source: &str) -> Finding {
    let mut found = findings(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    found.remove(0)
}

fn lower(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit::emit_program(&parsed, Default::default())
        .expect("the source lowers")
        .rust
}

const POINT: &str = "struct Point { x: i64, y: i64, z: i64 }\n";

/// **What it lowers to is one struct expression with a base** (§3), and `..p`
/// rather than `..p.clone()`: no copy is inserted that the program did not
/// write ([ADR-282](../../../docs/specification/adr/adr-282.md) D7).
#[test]
fn it_lowers_to_a_functional_update_with_no_copy_inserted() {
    let rust = lower(&format!(
        "{POINT}\n\
         fn main() {{\n\
         \x20   let p = Point {{ x: 1, y: 2, z: 3 }}\n\
         \x20   let moved = p with {{ x: 4 }}\n\
         \x20   println(f\"{{moved.x}}\")\n\
         }}"
    ));
    assert!(rust.contains("Point { x: 4, ..p }"), "{rust}");
    assert!(!rust.contains(".clone()"), "{rust}");
}

/// **A field the type does not have is the literal's refusal**, `NK1107`, and
/// it is reused rather than written again: one rule, one message.
#[test]
fn a_field_the_type_does_not_have_is_refused() {
    let found = one(&format!(
        "{POINT}\n\
         fn main() {{\n\
         \x20   let p = Point {{ x: 1, y: 2, z: 3 }}\n\
         \x20   let q = p with {{ w: 4 }}\n\
         \x20   println(f\"{{q.x}}\")\n\
         }}"
    ));
    assert_eq!(found.code, "NK1107");
    assert_eq!(found.message, "`Point` has no field called `w`.");
}

/// **A field named twice is `NK1172`** (D1) — **and so is one in a plain
/// literal**, which is where this was found.
///
/// `Point { x: 1, x: 2 }` lowered, and `rustc` answered about the generated
/// file. The rule is the literal's; the record restates it for `with` because
/// `with` borrows the braces, and one rule written twice is two rules waiting
/// to disagree.
#[test]
fn a_field_named_twice_is_refused_in_both_places() {
    for source in [
        format!(
            "{POINT}\n\
             fn main() {{\n\
             \x20   let p = Point {{ x: 1, x: 2, y: 3, z: 4 }}\n\
             \x20   println(f\"{{p.x}}\")\n\
             }}"
        ),
        format!(
            "{POINT}\n\
             fn main() {{\n\
             \x20   let p = Point {{ x: 1, y: 2, z: 3 }}\n\
             \x20   let q = p with {{ x: 4, x: 5 }}\n\
             \x20   println(f\"{{q.x}}\")\n\
             }}"
        ),
    ] {
        let found = one(&source);
        assert_eq!(found.code, "NK1172");
        assert_eq!(found.message, "You're setting `Point.x` twice.");
    }
}

/// **A `with` that names no field is `NK1174`** (D1): a copy that changes
/// nothing is the value itself, and the line is one a reader would stop at
/// looking for what it does.
#[test]
fn a_with_that_names_no_field_is_refused() {
    let found = one(&format!(
        "{POINT}\n\
         fn main() {{\n\
         \x20   let p = Point {{ x: 1, y: 2, z: 3 }}\n\
         \x20   let q = p with {{ }}\n\
         \x20   println(f\"{{q.x}}\")\n\
         }}"
    ));
    assert_eq!(found.code, "NK1174");
    assert!(
        found.help.as_deref() == Some("Name the fields that change, or remove the `with`."),
        "{:?}",
        found.help
    );
}

/// **An enum operand is refused with a message naming `match`** (§4).
///
/// Which fields a copy would carry depends on the variant, and the type does
/// not say. Whether a `with` inside a `match` arm — where the variant *is*
/// known — should be allowed is a decision that record left open, so the way
/// out points at the place where the question has an answer rather than at a
/// form that does not exist.
#[test]
fn an_enum_operand_is_refused_and_the_way_out_names_match() {
    let found = one("enum Op { Add, Sub }\n\
         \n\
         fn main() {\n\
         \x20   let o = Op::Add\n\
         \x20   let bad = o with { x: 1 }\n\
         \x20   println(\"unreachable\")\n\
         }");
    assert_eq!(found.code, "NK1173");
    assert_eq!(
        found.message,
        "`with` copies a struct, but `Op` is an `enum`."
    );
    assert!(
        found.help.as_deref().is_some_and(|h| h.contains("match")),
        "{:?}",
        found.help
    );
}

/// **A view is not something to move from** (D3).
///
/// What `with` does not name it takes from the operand by move, and a copy
/// this compiler inserted would be one the program did not write
/// ([ADR-282](../../../docs/specification/adr/adr-282.md) D7). Without this the
/// emitted `Point { x: 1, ..p }` over a `&Point` is `rustc`'s *cannot move out
/// of `*p`* — about a file nobody wrote.
#[test]
fn a_view_is_not_something_to_copy_from() {
    let found = one(&format!(
        "{POINT}\n\
         fn shifted(p: ref Point) -> Point {{\n\
         \x20   return p with {{ x: 1 }}\n\
         }}\n\
         \n\
         fn main() {{\n\
         \x20   let p = Point {{ x: 1, y: 2, z: 3 }}\n\
         \x20   println(f\"{{shifted(p).x}}\")\n\
         }}"
    ));
    assert_eq!(found.code, "NK1173");
    assert!(
        found.notes[0].contains("moves the fields it doesn't change out of the value")
            && found.notes[0].contains("never copies behind your back"),
        "{:#?}",
        found.notes
    );
}

/// **And a value whose type this compiler could not work out** has nothing to
/// write, because the lowering writes the type's name.
///
/// The headline does not print the type: a `?` in it would be a sentence about
/// nothing, and what the reader needs to know is that it is the *type* that is
/// missing rather than the value.
#[test]
fn a_value_with_no_type_this_compiler_worked_out_is_refused() {
    let found = one("fn main() {\n\
         \x20   let n = 7\n\
         \x20   let q = n with { x: 1 }\n\
         \x20   println(\"unreachable\")\n\
         }");
    assert_eq!(found.code, "NK1173");
    assert!(!found.message.contains('?'), "{}", found.message);
}
