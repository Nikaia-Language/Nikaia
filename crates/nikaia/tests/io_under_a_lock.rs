//! `NK2201`: I/O while holding locked data, and the one thing it is about
//! ([ADR-067](../../../docs/specification/adr/adr-067.md) D1,
//! [ADR-169](../../../docs/specification/adr/adr-169.md)).
//!
//! D1 split Part II 12.2's *no I/O while holding locked data* in two — what
//! **pauses** is `NK2202`'s and what **takes a lock** is `NK2203`'s, and a
//! `println` is the second — and left the third case as a question: *is there
//! I/O that does neither?* It said to answer that before writing a code.
//!
//! The answer is **yes, one**. Of the nine `std` entries whose touch set names
//! I/O, four are the printing functions (`sync`, `locks = true`), four are
//! `fs`'s reads and its write (not `sync`), and one is neither: reading a
//! `fs::Mapped`. A mapping is a file held as memory, so `mapped[i]` is a **page
//! fault** — a disk read with nothing in the source to hang a `touches` on,
//! which neither suspends nor takes a lock.

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
    findings(source).iter().any(|f| f.code == "NK2201")
}

/// **A method on a mapping, inside a door.**
#[test]
fn a_mapping_read_inside_a_door_is_refused() {
    assert!(refused(
        "use std::fs\n\
         \n\
         fn main() throws {\n\
         \x20   let page = fs::map(\"eins.txt\", fs::Root::Anywhere)\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   counter.update fn(mut n) { n = n + page.len() }\n\
         }\n"
    ));
}

/// **And an index of one**, which is the shape Part II 12.2 would reach for and
/// the one with no call in it at all.
#[test]
fn an_index_of_a_mapping_inside_a_door_is_refused() {
    assert!(refused(
        "use std::fs\n\
         \n\
         fn main() throws {\n\
         \x20   let page = fs::map(\"eins.txt\", fs::Root::Anywhere)\n\
         \x20   let seen = SharedMut(0)\n\
         \x20   seen.access fn(n) { println(f\"{page[0]}\") }\n\
         }\n"
    ));
}

/// **The message says what it is and what to do**, which is what separates it
/// from the two codes that say nothing about it
/// ([Part III C.2](../../../docs/specification/30-nikaia-tooling.md)).
#[test]
fn the_refusal_names_the_page_fault_and_the_way_out() {
    let found = findings(
        "use std::fs\n\
         \n\
         fn main() throws {\n\
         \x20   let page = fs::map(\"eins.txt\", fs::Root::Anywhere)\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   counter.update fn(mut n) { n = n + page.len() }\n\
         }\n",
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK2201")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert!(refusal.message.contains("reads a file"), "{refusal:#?}");
    assert!(
        refusal
            .notes
            .iter()
            .any(|n| n.contains("reading it can wait on the disk")),
        "{refusal:#?}"
    );
    assert!(
        refusal
            .notes
            .iter()
            .any(|n| n.contains("doesn't show up as a pause or a second lock")),
        "it says why the other two are silent:\n{refusal:#?}"
    );
    assert!(
        refusal
            .help
            .as_ref()
            .is_some_and(|h| h.contains("before taking the lock")),
        "{refusal:#?}"
    );
}

/// **Outside a door it is an ordinary read**, which is most of what a mapping
/// is for: `1brc.nika` reads one from end to end and takes no lock at all.
#[test]
fn a_mapping_read_outside_a_door_is_untouched() {
    assert!(!refused(
        "use std::fs\n\
         \n\
         fn main() throws {\n\
         \x20   let page = fs::map(\"eins.txt\", fs::Root::Anywhere)\n\
         \x20   println(f\"{page.len()}\")\n\
         }\n"
    ));
}

/// **And a door that reads memory is untouched too**, which is the case the
/// refusal must not reach: the value was read before the door and handed in,
/// which is the way out the message names.
#[test]
fn a_door_over_memory_is_untouched() {
    assert!(!refused(
        "use std::fs\n\
         \n\
         fn main() throws {\n\
         \x20   let page = fs::map(\"eins.txt\", fs::Root::Anywhere)\n\
         \x20   let size = page.len()\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   counter.update fn(mut n) { n = n + size }\n\
         }\n"
    ));
}

/// **Silence is not a claim.** A type with no `touches` recorded is one nobody
/// answered for, and refusing on that would refuse correct programs
/// ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)) — a
/// `String` inside a door is memory and stays silent.
#[test]
fn a_type_that_says_nothing_is_claimed_nothing_about() {
    assert!(!refused(
        "fn main() {\n\
         \x20   let text = \"abc\"\n\
         \x20   let counter = SharedMut(0)\n\
         \x20   counter.update fn(mut n) { n = n + text.len() }\n\
         }\n"
    ));
}

/// **The column is the ledger's, not a list of names.** `fs::Mapped` says it in
/// `std.contracts`, and a second such type needs a line there and nothing in
/// the compiler ([ADR-169](../../../docs/specification/adr/adr-169.md) D1).
#[test]
fn the_claim_lives_in_the_ledger() {
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let mapped = library
        .types
        .get("fs::Mapped")
        .expect("`fs::Mapped` is described");
    assert_eq!(mapped.touches, vec!["file read".to_string()]);

    // And it is the only one, which is the measurement this record rests on.
    let touching: Vec<_> = library
        .types
        .iter()
        .filter(|(_, c)| !c.touches.is_empty())
        .map(|(key, _)| key.as_str())
        .collect();
    assert_eq!(touching, vec!["fs::Mapped"]);
}
