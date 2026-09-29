//! **A name beside a type this program declares** — `NK1171`.
//!
//! `Op::Mul`, written against `enum Op { Add, Sub }`, used to lower. The
//! compiler had read the declaration, the exhaustiveness check was holding the
//! variant list in its hand, and nothing compared the two — so what refused the
//! program was `rustc`, about the **generated file**, which is
//! [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s class and
//! the one this compiler refuses on principle.
//!
//! **And a path whose head names nothing at all is `NK1181`**
//! ([ADR-183](../../../docs/specification/adr/adr-183.md)), which used to be
//! the half this file recorded as open. `nowhere::wobble` could have been a
//! module, a foreign crate's item or a name no ledger had been told about, and
//! from inside the checker those three looked alike — until the list of what a
//! head may legally be was written down and asked in order. The line between
//! the two refusals is what this file is about: `NK1171` is a **member** a
//! type this compiler read does not have, and `NK1181` is a **head** nothing
//! declares.

use nikaia::check::Finding;
use nikaia::contracts::{Ledger, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    nikaia::check::check(&parsed, &own, &library).findings
}

fn one(source: &str) -> Finding {
    let mut found = findings(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    found.remove(0)
}

const OP: &str = "enum Op { Add, Sub }\n";

/// **A variant the enum does not have, as a value.**
#[test]
fn a_variant_the_enum_does_not_have_is_refused() {
    let found = one(&format!(
        "{OP}\n\
         fn main() {{\n\
         \x20   let chosen = Op::Mul\n\
         \x20   println(f\"{{chosen}}\")\n\
         }}"
    ));
    assert_eq!(found.code, "NK1171");
    assert_eq!(found.message, "`Op` has no variant called `Mul`.");
    assert!(
        found.notes[0].contains("`Op::Add`, `Op::Sub`"),
        "the note lists what there is: {:#?}",
        found.notes
    );
}

/// **And in a `match` arm**, which is where it is most likely to be written.
///
/// The exhaustiveness check cannot say this: it reads which variants an arm
/// *covered*, so a misspelling covers none and what it reports is the variant
/// that is **missing** — a true sentence pointing away from the mistake. The
/// `else` arm below removes even that, and this is what is left.
#[test]
fn a_variant_the_enum_does_not_have_is_refused_in_a_pattern() {
    let found = one(&format!(
        "{OP}\n\
         fn main() {{\n\
         \x20   let chosen = Op::Add\n\
         \x20   match chosen {{\n\
         \x20       Op::Add => println(\"add\"),\n\
         \x20       Op::Mul => println(\"mul\"),\n\
         \x20       else => println(\"other\"),\n\
         \x20   }}\n\
         }}"
    ));
    assert_eq!(found.code, "NK1171");
    assert_eq!(found.message, "`Op` has no variant called `Mul`.");
}

/// **A pattern nests, and so does this** —
/// [ADR-137](../../../docs/specification/adr/adr-137.md) D1's *the parts are
/// patterns*. Stopping at the outer path would find the shallow half of a rule,
/// which is worse than not having it: the reader would learn that the check
/// exists and then meet `rustc` anyway.
#[test]
fn a_variant_inside_a_nested_pattern_is_refused() {
    let found = one(&format!(
        "{OP}\n\
         enum Event {{ Chose(Op), Left }}\n\
         \n\
         fn main() {{\n\
         \x20   let what = Event::Left\n\
         \x20   match what {{\n\
         \x20       Event::Chose(Op::Mul) => println(\"mul\"),\n\
         \x20       else => println(\"other\"),\n\
         \x20   }}\n\
         }}"
    ));
    assert_eq!(found.code, "NK1171");
    assert_eq!(found.message, "`Op` has no variant called `Mul`.");
}

/// **A near miss says which name was meant**, which is the difference between
/// a refusal a reader acts on and one they re-read.
///
/// `Sbu` for `Sub` is a swapped pair, the most common typo there is, and the
/// edit distance this compiler uses charges it **one** rather than Levenshtein's
/// two — which is the difference between it being suggested and not.
#[test]
fn a_near_miss_names_the_variant_that_was_meant() {
    let found = one(&format!(
        "{OP}\n\
         fn main() {{\n\
         \x20   let chosen = Op::Sbu\n\
         \x20   println(f\"{{chosen}}\")\n\
         }}"
    ));
    assert_eq!(found.help.as_deref(), Some("Did you mean `Op::Sub`?"));
}

/// **A struct has no items under `::`**, and the way out is the one that works:
/// a field is read from a value.
#[test]
fn a_struct_has_no_members_under_the_separator() {
    let found = one("struct Point { x: i64, y: i64 }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{Point::x}\")\n\
         }");
    assert_eq!(found.code, "NK1171");
    assert_eq!(
        found.message,
        "`Point` has nothing called `x` to reach with `::`."
    );
    assert_eq!(
        found.help.as_deref(),
        Some("Read it from a value: `value.x`.")
    );
}

/// **A type's shape is reached through a **bound** and not by name**
/// ([ADR-088](../../../docs/specification/adr/adr-088.md) D2, built by
/// [ADR-181](../../../docs/specification/adr/adr-181.md)).
///
/// A reader who writes `Point::fields` read the specification, so *`Point` has
/// nothing called `fields`* would send them looking for a spelling that does
/// not exist. [Part III C.2](../../../docs/specification/30-nikaia-tooling.md)
/// asks for a way out that can be taken, and since 0.0.129 there is one: the
/// note names the shape 10.3 actually writes. **A type named outright has its
/// fields written down already**, which is why this stays a refusal now that
/// the feature is built.
#[test]
fn the_shape_of_a_type_is_reached_through_a_bound() {
    let found = one("struct Point { x: i64, y: i64 }\n\
         \n\
         fn main() {\n\
         \x20   for field in Point::fields {\n\
         \x20       println(field.name)\n\
         \x20   }\n\
         }");
    assert_eq!(found.code, "NK1171");
    assert_eq!(
        found.message,
        "You can't reach `Point::fields` directly: it's reached through a type parameter bound by `Struct`."
    );
    assert!(
        found.notes[0].contains("reached through a bound")
            && found.notes[0].contains("`fn describe[T: Struct](value: T)`, and then `T::fields`"),
        "{:#?}",
        found.notes
    );
    assert!(
        found
            .help
            .as_deref()
            .is_some_and(|help| help.contains("[T: Struct]")),
        "and the way out is the one 10.3 writes: {:?}",
        found.help
    );
}

/// **A method an `impl` declares is not this**, which is the whole reason the
/// refusal asks the ledger first: `Summary::merge` is a real item and the
/// corpus hands it over as a value.
#[test]
fn a_method_the_program_declares_is_not_refused() {
    let source = "struct Summary { total: i64 }\n\
         \n\
         impl Summary {\n\
         \x20   fn merge(self, other: Summary) -> Summary {\n\
         \x20       return Summary { total: self.total + other.total }\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let first = Summary { total: 1 }\n\
         \x20   let second = Summary { total: 2 }\n\
         \x20   println(f\"{first.merge(second).total}\")\n\
         }";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
}

/// **A variant the enum does have is not refused**, which is the guard that
/// says the refusal above is about the name and not about the shape.
#[test]
fn a_variant_the_enum_has_is_not_refused() {
    let source = format!(
        "{OP}\n\
         fn main() {{\n\
         \x20   let chosen = Op::Add\n\
         \x20   match chosen {{\n\
         \x20       Op::Add => println(\"add\"),\n\
         \x20       Op::Sub => println(\"sub\"),\n\
         \x20   }}\n\
         }}"
    );
    assert!(findings(&source).is_empty(), "{:#?}", findings(&source));
}

/// **A head this compiler has not read stays quiet**, which is the line the
/// module doc draws. Refusing here would be refusing on absence, and absence is
/// what a package's module, a foreign crate's item and an undescribed name all
/// look like from inside the checker
/// ([ADR-010](../../../docs/specification/adr/adr-010.md) D1 read the other
/// way: a wrong refusal is worse than a missing one).
/// **A head nothing declares is `NK1181` and not this refusal**
/// ([ADR-183](../../../docs/specification/adr/adr-183.md)).
///
/// The two are different questions and the messages say so: `NK1171` has read
/// the declaration and is naming a misspelling; this one has read nothing
/// under that word at all.
#[test]
fn a_head_nothing_declares_is_its_own_refusal() {
    let source = "fn main() {\n\
         \x20   println(f\"{nowhere::wobble}\")\n\
         }";
    let found = findings(source);
    assert!(found.iter().all(|f| f.code != "NK1171"), "{found:#?}");
    let head = found
        .iter()
        .find(|f| f.code == "NK1181")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert!(
        head.message
            .contains("is written under `nowhere`, but `nowhere` isn't declared anywhere"),
        "{head:#?}"
    );
    // **A way out that can be taken** (Part III C.2): each of the three names
    // a file the reader writes.
    let help = head.help.as_deref().unwrap_or("");
    assert!(
        help.contains(".nika") && help.contains("[dependencies]") && help.contains("describe"),
        "{head:#?}"
    );
}

/// **Every head the corpus writes still passes**, which is the measurement
/// `open-work.md`'s entry rested on: seven distinct paths used as values, each
/// either a variant of an enum the program declares or a key a ledger records.
///
/// And the three the entry named as the reason to fail open — a module of this
/// package, a package the manifest declares, a `std` module — are each a head
/// this compiler has read, which is why the refusal can be written at all.
#[test]
fn a_head_this_program_has_is_not_refused() {
    let source = "enum Op { Times, Divide }\n\
         \n\
         struct Summary { total: i64 }\n\
         \n\
         impl Summary {\n\
         \x20   fn merge(self, other: Summary) -> Summary {\n\
         \x20       Summary { total: self.total + other.total }\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let op = Op::Times\n\
         \x20   let one = Summary { total: 1 }\n\
         \x20   let two = one.merge(Summary { total: 2 })\n\
         \x20   println(f\"{two.total}\")\n\
         }";
    let found = findings(source);
    assert!(
        found.iter().all(|f| f.code != "NK1181"),
        "a head this compiler has read is never this refusal: {found:#?}"
    );
}

/// **A `std` module is a head**, and it is the one an empty package still has:
/// these findings are taken with no manifest and no second file, so `io` can
/// only be answered by `std`'s own ledger.
///
/// The member here is deliberately one `std` does **not** have, because that is
/// where the two refusals could be confused: a head this compiler has read and
/// a member it has not is not `NK1181`, whatever else it may be. What the head
/// may hold is the ledger's question and not this one's.
#[test]
fn a_std_module_is_a_head_whatever_stands_after_it() {
    let source = "fn main() {\n\
         \x20   println(f\"{io::wobble}\")\n\
         }";
    let found = findings(source);
    assert!(
        found.iter().all(|f| f.code != "NK1181"),
        "`io` is a head `std` declares: {found:#?}"
    );
}
