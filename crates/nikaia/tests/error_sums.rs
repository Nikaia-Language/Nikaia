//! A set with more than one error type in it: the generated sum
//! ([ADR-280](../../../docs/specification/adr/adr-280.md)).
//!
//! [ADR-023](../../../docs/specification/adr/adr-023.md) D1 records `throws` as
//! a **set**, and the three records before this one made a set of **one** a
//! channel: the program's own type ([ADR-280](../../../docs/specification/adr/adr-280.md)),
//! then `std`'s names ([ADR-280](../../../docs/specification/adr/adr-280.md)),
//! then a library's type ([ADR-280](../../../docs/specification/adr/adr-280.md)).
//! What was left is the shape a program reaches by doing two ordinary things:
//! reading a file **and** throwing an error of its own.
//!
//! The sum is a name no program writes. A `catch` matches on the **members'**
//! variants ([ADR-023](../../../docs/specification/adr/adr-023.md) D4), so
//! `ConfigError::Empty(p)` and `io::IoError::NotFound(p)` stand in one block and
//! the lowering takes the match apart by member.
//!
//! What a handler sees when the program runs is tested in the language:
//! `tests/language/src/error_sums.nika`. What stays here is the type the
//! lowering writes and the refusal.

mod common;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// The shape: an error of the program's own, and a file read, in one function.
const TWO_WAYS: &str = "use std::fs\n\
                        use std::io\n\
                        \n\
                        enum ConfigError { Empty(ref String) }\n\
                        \n\
                        impl Error for ConfigError {\n\
                        \x20   fn message(ref self) -> String {\n\
                        \x20       match self { ConfigError::Empty(p) => f\"config at {p} is empty\" }\n\
                        \x20   }\n\
                        }\n\
                        \n\
                        fn load(path: ref String) -> String throws {\n\
                        \x20   let text = fs::read_to_string(ref path, fs::Root::Anywhere)\n\
                        \x20   if text == \"\" { throw ConfigError::Empty(path) }\n\
                        \x20   return text\n\
                        }\n\n";

// ---------------------------------------------------------------------------
// D1: the type
// ---------------------------------------------------------------------------

/// **A set of two named members is a generated sum** (D1), and not the opaque
/// channel it fell back to.
#[test]
fn two_error_types_are_a_sum() {
    let rust = lowered(&format!("{TWO_WAYS}fn main() {{ }}\n"));
    assert!(rust.contains("enum __NikaiaThrows_"), "{rust}");
    // **Named `crate::…` wherever it is used**, because the type is defined
    // once at the crate root: a module is a file of its own below, and a bare
    // name would be a different type in each of them.
    assert!(
        rust.contains("-> Result<String, crate::__NikaiaThrows_"),
        "{rust}"
    );
    assert!(!rust.contains("Box<dyn std::error::Error>"), "{rust}");
}

/// **Each member keeps the channel it would have had alone** (D2): an
/// envelope, the program's own type and a library's alike
/// ([ADR-280](../../../docs/specification/adr/adr-280.md) D13) - and a `?`
/// straight from `std` hands the library's error bare, so the sum puts the
/// envelope on for it.
#[test]
fn a_member_keeps_its_own_channel() {
    let rust = lowered(&format!("{TWO_WAYS}fn main() {{ }}\n"));
    assert!(
        rust.contains("ConfigError(nikaia_std::error::Thrown<ConfigError"),
        "{rust}"
    );
    assert!(
        rust.contains("io_IoError(nikaia_std::error::Thrown<io::IoError>)"),
        "{rust}"
    );
    assert!(rust.contains("From<io::IoError> for "), "{rust}");
}

/// **One type per distinct set and not per function**, which is what makes
/// propagation free: two functions that fail the same way get the same type, so
/// a `?` between them converts nothing.
#[test]
fn two_functions_with_one_set_share_a_type() {
    let rust = lowered(&format!(
        "{TWO_WAYS}fn again(path: ref String) -> String throws {{\n\
         \x20   return load(path)\n\
         }}\n\
         fn main() {{ }}\n"
    ));
    assert_eq!(rust.matches("enum __NikaiaThrows_").count(), 1, "{rust}");
    // And the propagation is the plain `?`, with nothing written around it.
    assert!(rust.contains("load(path).await?"), "{rust}");
}

// ---------------------------------------------------------------------------
// D4: the refusal
// ---------------------------------------------------------------------------

/// **A `match` over a `catch`'s error always needs `else`** (D4). The variants
/// **within** one error type are closed and the set of error **types** is open
/// ([ADR-023](../../../docs/specification/adr/adr-023.md) D4), so a handler that
/// names variants of two of them has covered no set at all: a callee that gains
/// a failure sends a third type here.
#[test]
fn a_match_over_two_error_types_needs_an_else() {
    let found: Vec<_> = findings(&format!(
        "{TWO_WAYS}fn main() {{\n\
         \x20   let text = load(\"x\") catch {{\n\
         \x20       match error {{\n\
         \x20           ConfigError::Empty(p) => f\"empty: {{p}}\"\n\
         \x20           io::IoError::NotFound(p) => f\"missing: {{p.display()}}\"\n\
         \x20       }}\n\
         \x20   }}\n\
         \x20   println(f\"{{text}}\")\n\
         }}\n"
    ))
    .into_iter()
    .filter(|f| f.code == "NK1151")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("else"), "{}", found[0].message);
}

/// **A member with one variant draws no warning** (#542): its half of the
/// match names every case, so the catch-all copied into it cannot be reached,
/// and what `rustc` would say is about the copy, in a file nobody wrote.
#[test]
fn a_one_variant_member_draws_no_warning() {
    let rust = lowered(&format!(
        "{TWO_WAYS}fn main() {{\n\
         \x20   let text = load(\"x\") catch {{\n\
         \x20       match error {{\n\
         \x20           ConfigError::Empty(p) => f\"empty: {{p}}\"\n\
         \x20           else => \"other\".clone()\n\
         \x20       }}\n\
         \x20   }}\n\
         \x20   println(f\"{{text}}\")\n\
         }}\n"
    ));
    let dir = common::scratch_dir("error-sums-one-variant");
    let path = dir.join("main.rs");
    std::fs::write(&path, &rust).expect("write the lowered program");
    let compiled = common::compile(
        &path,
        &[
            "--crate-type",
            "bin",
            "-o",
            &dir.join("program").to_string_lossy(),
        ],
    );
    let said = String::from_utf8_lossy(&compiled.stderr);
    assert!(compiled.status.success(), "{said}\n--- emitted ---\n{rust}");
    assert!(
        !said.contains("unreachable"),
        "{said}\n--- emitted ---\n{rust}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **And one error type is not this rule's.** A handler over a set of one is
/// matching a closed `enum`, so naming every variant of it is exhaustive and
/// nothing is missing.
#[test]
fn one_error_type_is_left_alone() {
    let source = "enum ConfigError { Empty, Bad }\n\
                  impl Error for ConfigError {\n\
                  \x20   fn message(ref self) -> String { return \"no\" }\n\
                  }\n\
                  fn load() -> i64 throws { throw ConfigError::Empty }\n\
                  fn main() {\n\
                  \x20   let n = load() catch {\n\
                  \x20       match error {\n\
                  \x20           ConfigError::Empty => 1\n\
                  \x20           ConfigError::Bad => 2\n\
                  \x20       }\n\
                  \x20   }\n\
                  \x20   println(f\"{n}\")\n\
                  }\n";
    let found: Vec<_> = findings(source)
        .into_iter()
        .filter(|f| f.code == "NK1151")
        .collect();
    assert!(found.is_empty(), "{found:#?}");
}

/// **A set with a `"?"` in it is still the opaque channel**, because a sum with
/// a hole in it is the box by another spelling.
#[test]
fn a_set_with_a_question_mark_is_not_a_sum() {
    let rust = lowered(&format!(
        "{TWO_WAYS}fn unclear(s: String) -> String throws {{\n\
         \x20   let t = load(\"x\")\n\
         \x20   return {}\n\
         }}\n\
         fn main() {{ }}\n",
        common::undescribed_value("s")
    ));
    assert!(
        rust.contains("fn unclear(s: String) -> Result<String, Box<dyn"),
        "{rust}"
    );
}
