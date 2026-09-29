//! **The ledger writes Nikaia's syntax wherever Nikaia has one**
//! ([ADR-251](../../../docs/specification/adr/adr-251.md) D4): a receiver as
//! the source writes it, where a result points as `ref(a | b)` in the result,
//! a type variable declared in brackets, and a promise about a lambda as
//! `sync(f)`. Each is written, read back to the same contract, and written
//! again to the same bytes - and `std`'s hand-written ledger, which moved to
//! this spelling at 0.0.254, is read by the same reader.

use nikaia::contracts::{Ledger, Sync};
use nikaia::parser::parse_to_ast;

fn written(source: &str) -> (Ledger, String) {
    let ledger = Ledger::infer(&parse_to_ast(source).expect("the source parses"));
    let text = ledger.render();
    let read = Ledger::parse(&text).expect("its own output parses");
    assert_eq!(read.render(), text, "written again, the same bytes");
    assert_eq!(
        read.functions, ledger.functions,
        "read back, the same contract"
    );
    (read, text)
}

/// **A receiver is `self`, `ref self` or `ref mut self`**, and `mutates` is
/// what the receiver says rather than a key beside it.
#[test]
fn a_receiver_is_written_as_the_source_writes_it() {
    let (read, text) = written(
        "pub struct Row { pub n: i64 }\n\
         impl Row {\n\
         \x20   pub fn get(ref self) -> i64 sync { return self.n }\n\
         \x20   pub fn bump(ref mut self, by: i64) sync { self.n = self.n + by }\n\
         }\n",
    );
    assert!(text.contains("signature = \"(ref self) -> i64\""), "{text}");
    assert!(
        text.contains("signature = \"(ref mut self, by: i64)\""),
        "{text}"
    );
    assert!(!text.contains("mutates"), "{text}");
    assert!(read.functions["Row::bump"].mutates);
    assert!(!read.functions["Row::get"].mutates);
}

/// **Where a result points is `ref(…)` in the result**, where `returns`
/// stood beside the signature.
#[test]
fn where_a_result_points_is_written_in_the_result() {
    let (read, text) = written(
        "pub fn longest(a: ref String, b: ref String) -> ref String sync {\n\
         \x20   if a.len() > b.len() {\n\
         \x20       return a\n\
         \x20   }\n\
         \x20   return b\n\
         }\n",
    );
    assert!(
        text.contains("signature = \"(a: ref String, b: ref String) -> ref(a | b) String\""),
        "{text}"
    );
    assert!(!text.contains("returns ="), "{text}");
    assert_eq!(read.functions["longest"].borrows, ["a", "b"]);
}

/// **A type variable is declared in brackets before the parameters**, as
/// `fn hand[T](x: T) -> T` declares it, and a bound stays where it stood.
#[test]
fn a_type_variable_is_declared_in_brackets() {
    let (_, text) = written(
        "pub fn hand[T](x: T) -> T sync { return x }\n\
         pub trait Speaks { fn speak(ref self) -> String }\n\
         pub fn tell[T: Speaks](x: T) -> String { return x.speak() }\n",
    );
    assert!(text.contains("signature = \"[T](x: T) -> T\""), "{text}");
    assert!(
        text.contains("signature = \"[T: Speaks](x: T) -> String\""),
        "{text}"
    );
    assert!(!text.contains('$'), "{text}");
}

/// **`std`'s own ledger is in this spelling**, and a promise about a lambda is
/// `sync(f)` - the word ADR-244 D4 gives the source.
#[test]
fn stds_ledger_is_written_in_nikaias_spelling() {
    let std = Ledger::parse(nikaia::contracts::STD).expect("std's ledger parses");
    let map = std
        .functions
        .iter()
        .find(|(_, c)| matches!(c.sync, Sync::From(_)))
        .expect("std has a function whose lambda decides");
    assert!(
        nikaia::contracts::STD.contains(&format!("[fn.\"{}\"]", map.0)),
        "the entry is there"
    );
    assert!(nikaia::contracts::STD.contains("sync = \"sync(f)\""));
    assert!(!nikaia::contracts::STD.contains("sync = \"from("));
    for line in nikaia::contracts::STD.lines() {
        if line.starts_with("signature") {
            assert!(!line.contains('$'), "{line}");
        }
        assert!(!line.starts_with("returns = \"borrows("), "{line}");
    }
}
