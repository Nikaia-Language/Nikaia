//! **Where more than one error type arrives, `error` is an `Error`, one of
//! them** ([ADR-280](../../../docs/specification/adr/adr-280.md) D30, #512).
//! It answers `Error`'s methods, prints and is thrown on as one type, is taken
//! apart by member in a `match`, and a part of one member read anywhere else is
//! refused. What such a handler computes is `tests/language`'s
//! `error_sums.nika`.

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn errors(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

/// `both()` reads a file (`io::IoError`) and throws a `ConfigError`.
fn handler(body: &str) -> String {
    format!(
        "use std::fs\n\n\
         enum ConfigError {{\n    NotFound(ref String),\n    BadSyntax {{ line: i64, expected: ref String }},\n}}\n\n\
         impl Error for ConfigError {{\n    fn message(ref self) -> String {{\n        return \"config\"\n    }}\n}}\n\n\
         fn both() -> i64 throws {{\n    let t = fs::read_to_string(\"x\", fs::Root::Anywhere)\n    \
         throw ConfigError::BadSyntax {{ line: t.len(), expected: \"x\" }}\n}}\n\n\
         fn again() -> i64 throws {{\n    return both() catch {{\n{body}\n    }}\n}}\n\n\
         fn main() {{\n    let n = again() catch {{ 0 }}\n    println(f\"{{n}}\")\n}}\n"
    )
}

#[test]
fn what_every_error_has_passes() {
    let found = errors(&handler(
        "        let m: String = error.message()\n        let f: String = error.full()\n        \
         println(f\"{error} {m} {f}\")\n        throw error",
    ));
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn the_value_is_said_as_an_error_of_its_members() {
    let found = errors(&handler("        let wrong: i64 = error\n        0"));
    assert!(
        found.iter().any(|f| f.code == "NK1103"
            && f.message
                == "This value is an error, one of `ConfigError`, `io::IoError`, but the `let` declares `i64`."),
        "{found:#?}"
    );
}

#[test]
fn a_part_of_one_member_is_reached_through_the_match() {
    let outside = errors(&handler("        let l = error.line\n        l"));
    assert!(
        outside.iter().any(|f| f.code == "NK1107"
            && f.message
                == "This is an error, one of `ConfigError`, `io::IoError`, and an error has no field `line`."),
        "{outside:#?}"
    );
    let typed = errors(&handler(
        "        match error {\n            ConfigError::BadSyntax { line, .. } => {\n                \
         let l: String = line\n                0\n            }\n            else => 0,\n        }",
    ));
    assert!(
        typed.iter().any(|f| f.code == "NK1103"
            && f.message == "This value is `i64`, but the `let` declares `String`."),
        "{typed:#?}"
    );
}
