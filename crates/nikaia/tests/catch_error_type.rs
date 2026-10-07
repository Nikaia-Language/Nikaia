//! **A handler's `error` is what the guarded calls throw** (Part I 7.1,
//! #501): one error type, as the ledger records the calls, and the checker
//! knows it. It used to be unknown, so a wrong use of it reached `rustc`.

mod common;

use nikaia::check::Finding;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = common::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    common::checked(&parsed, &own, &library).findings
}

/// `fs::read_to_string` throws an `io::IoError`, so `error` is one.
#[test]
fn a_library_call_binds_its_error_type() {
    let source = r#"
use std::fs

fn main() {
    let t = fs::read_to_string("nope.txt", fs::Root::Anywhere) catch {
        let n: i64 = error
        ""
    }
    println(t)
}
"#;
    let found = findings(source);
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK1103" && f.message.contains("`io::IoError`")),
        "{found:#?}"
    );
}

/// And a `match` over it names that type's variants and checks.
#[test]
fn a_match_over_the_library_error_checks() {
    let source = r#"
use std::fs

fn main() {
    let t = fs::read_to_string("nope.txt", fs::Root::Anywhere) catch {
        match error {
            io::IoError::NotFound(p) => println(f"no {p.display()}")
            else => eprintln(f"{error}")
        }
        ""
    }
    println(t)
}
"#;
    let found = findings(source);
    assert!(found.is_empty(), "{found:#?}");
}

/// A function of the program's own binds the error type its body throws.
#[test]
fn a_program_function_binds_its_error_type() {
    let source = r#"
enum ConfigError {
    Missing,
}

impl Error for ConfigError {
    fn message(ref self) -> String {
        return "missing"
    }
}

fn load(on: bool) -> i64 throws {
    if !on {
        throw ConfigError::Missing
    }
    return 1
}

fn main() {
    let n = load(true) catch {
        let wrong: bool = error
        0
    }
    println(f"{n}")
}
"#;
    let found = findings(source);
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK1103" && f.message.contains("`ConfigError`")),
        "{found:#?}"
    );
}
