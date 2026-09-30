//! `std` names what it throws
//! ([ADR-158](../../../docs/specification/adr/adr-158.md)).
//!
//! [ADR-023](../../../docs/specification/adr/adr-023.md) D1 records `throws` as
//! a **set of error types**, and for every entry in `std`'s ledger that set was
//! `["?"]` — [ADR-024](../../../docs/specification/adr/adr-024.md) D1's absence
//! of a claim, standing in for one. A caller read it as *it fails*, which is
//! what the boolean before the column said, so the column added nothing for the
//! one library every program uses.
//!
//! The measurement behind the record is small enough to hold here as a test:
//! **nine** `std` entries throw, two already named `Overtaken`, and the seven
//! that did not all fail with one and the same Rust type. So there is one error
//! type to name, and the specification had already named it — `IoError`, in
//! [ADR-023](../../../docs/specification/adr/adr-023.md) D6's worked output and
//! in Part III 13.5's own example row.

use nikaia::contracts::{Ledger, STD};
use nikaia::parser::parse_to_ast;

fn std_ledger() -> Ledger {
    Ledger::parse(STD).expect("std ships a ledger")
}

fn throws_of(source: &str, of: &str) -> Vec<String> {
    let parsed = parse_to_ast(source).expect("the source parses");
    Ledger::infer(&parsed).functions[of].fails_with.clone()
}

/// **Nothing in `std` throws without saying what** (D1). This is the claim the
/// record is about, and it is the one a later entry is easiest to forget: a new
/// `std` function that writes `["?"]` puts the absence of a claim back into the
/// one library every program reads.
#[test]
fn no_std_entry_throws_without_naming_it() {
    let unnamed: Vec<_> = std_ledger()
        .functions
        .iter()
        .filter(|(_, c)| c.fails_with.iter().any(|t| t == "?"))
        .map(|(key, _)| key.clone())
        .collect();
    assert!(unnamed.is_empty(), "still unnamed:\n{unnamed:#?}");
}

/// **And what they throw is one type**, which is what made the record small:
/// six file-and-stream entries and the C string's check all fail the same way.
#[test]
fn the_reading_and_writing_entries_name_one_type() {
    let library = std_ledger();
    for key in [
        "fs::map",
        "fs::read",
        "fs::read_to_string",
        "fs::write",
        "io::read",
        "io::read_to_string",
        "foreign::CStr::to_string",
    ] {
        let (_, contract) = library.lookup(key).unwrap_or_else(|| panic!("{key}"));
        assert_eq!(
            contract.fails_with,
            vec!["io::IoError".to_string()],
            "{key}"
        );
    }
}

/// **The lock's two keep their own** ([ADR-039](../../../docs/specification/adr/adr-039.md)
/// D5), which is the half that was already right: a failure with a name had one
/// long before this record.
#[test]
fn the_locks_two_keep_overtaken() {
    let library = std_ledger();
    for key in ["Locked::set(after)", "SharedMut::set(after)"] {
        let (_, contract) = library.lookup(key).unwrap_or_else(|| panic!("{key}"));
        assert_eq!(contract.fails_with, vec!["Overtaken".to_string()], "{key}");
    }
}

/// **`io::IoError` is a type the ledger describes**, so a program that writes
/// it in a signature is not writing a name nothing declares.
#[test]
fn the_type_is_in_the_ledger() {
    assert!(
        std_ledger().types.contains_key("io::IoError"),
        "`io::IoError` is not described"
    );
}

/// **It is reached through its module** ([ADR-154](../../../docs/specification/adr/adr-154.md)
/// D3): the key carries the prefix, so what needs no `use` stays Part I 1.3's
/// list and this is not on it. A program that only passes the failure on names
/// nothing; one that takes it apart writes `use std::io`.
#[test]
fn it_is_not_in_the_prelude() {
    let library = std_ledger();
    assert!(!library.types.contains_key("IoError"));
    assert!(library.types.contains_key("io::IoError"));
}

/// **A program that reads a file now has a set that says something** — which is
/// the whole of what the record buys, and what `["?"]` could never be.
#[test]
fn a_program_that_reads_a_file_names_what_it_throws() {
    let source = "use std::fs\n\
                  fn load(path: ref String) -> String throws {\n\
                  \x20   return fs::read_to_string(ref path, fs::Root::Anywhere)\n\
                  }\n\
                  fn main() { }\n";
    assert_eq!(throws_of(source, "load"), vec!["io::IoError".to_string()]);
}

/// **And a program that reads a file *and* throws its own has both**, which is
/// [ADR-023](../../../docs/specification/adr/adr-023.md) D1's set doing the one
/// thing a set does that a boolean cannot.
#[test]
fn a_program_with_its_own_error_too_has_both() {
    let source = "use std::fs\n\
                  enum ConfigError { Empty }\n\
                  impl Error for ConfigError {\n\
                  \x20   fn message(ref self) -> String { return \"empty\" }\n\
                  }\n\
                  fn load(path: ref String) -> String throws {\n\
                  \x20   let text = fs::read_to_string(ref path, fs::Root::Anywhere)\n\
                  \x20   if text == \"\" { throw ConfigError::Empty }\n\
                  \x20   return text\n\
                  }\n\
                  fn main() { }\n";
    assert_eq!(
        throws_of(source, "load"),
        vec!["ConfigError".to_string(), "io::IoError".to_string()]
    );
}
