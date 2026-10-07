//! The failure channel is **the error type**, where the ledger names one
//! ([ADR-280](../../../docs/specification/adr/adr-280.md)).
//!
//! [ADR-023](../../../docs/specification/adr/adr-023.md) D1 records `throws` as
//! a **set of error types** and the ledger has derived it for a long time; D3
//! makes an error type an `enum` and D4 makes its variants closed, so a `catch`
//! matches on them. What stood between the two was the **channel**: every
//! `throws` lowered to `Result<T, Box<dyn Error>>`, and a `match error {
//! ConfigError::NotFound(p) => … }` over a box is not a program the language
//! below accepts.
//!
//! So Part I 7.1's own `catch` example — the one its Status note called
//! *implemented* — lowered to Rust that does not compile, with `rustc` naming a
//! type in a file the author never wrote. That is
//! [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s class of
//! defect, and it is what these tests hold closed.
//!
//! The programs that run - a handler matching what it caught, `{error}` and
//! `error.full()`, a failure passed on - are
//! `tests/language/src/error_types.nika`; here are the channel's lowering and
//! the sets the ledger records.

mod common;

use nikaia::contracts::{Ledger, LedgerOps};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

fn throws_of(source: &str, of: &str) -> Vec<String> {
    let parsed = parse_to_ast(source).expect("the source parses");
    Ledger::infer(&parsed).functions[of].fails_with.clone()
}

/// Part I 7.1's error type, with both variant shapes it writes.
const CONFIG_ERROR: &str = "enum ConfigError {\n\
                            \x20   NotFound(ref String),\n\
                            \x20   BadSyntax { line: i64, expected: ref String },\n\
                            }\n\
                            \n\
                            impl Error for ConfigError {\n\
                            \x20   fn message(ref self) -> String {\n\
                            \x20       match self {\n\
                            \x20           ConfigError::NotFound(p) => f\"no config at {p}\"\n\
                            \x20           ConfigError::BadSyntax { line, expected } => f\"line {line}: expected {expected}\"\n\
                            \x20       }\n\
                            \x20   }\n\
                            }\n\n";

// ---------------------------------------------------------------------------
// D1: the channel
// ---------------------------------------------------------------------------

/// **The channel is the error type** (D1), so the signature names it and a
/// handler can see what it caught.
#[test]
fn a_function_with_one_error_type_declares_it() {
    let rust = lowered(&format!(
        "{CONFIG_ERROR}fn load() -> i64 throws {{\n\
         \x20   throw ConfigError::NotFound(\"etc\")\n\
         }}\n\
         fn main() {{ }}\n"
    ));
    assert!(
        rust.contains("nikaia_std::error::Thrown<ConfigError"),
        "{rust}"
    );
    assert!(!rust.contains("fn load() -> Result<i64, Box<dyn"), "{rust}");
}

/// **A set with `"?"` in it keeps the box**, which is what `"?"` means: the
/// compiler cannot name what this fails with, so nothing can be named after it.
///
/// **`std` used to be this test's example** and stopped being one twice over:
/// [ADR-280](../../../docs/specification/adr/adr-280.md) gave `std` names, and
/// [ADR-280](../../../docs/specification/adr/adr-280.md) D9 let a channel be
/// named after a type a **ledger** describes. So the example is now a call no
/// ledger describes at all, which is what `"?"` has always meant.
#[test]
fn a_set_with_a_question_mark_keeps_the_box() {
    let rust = lowered(&format!(
        "fn load(s: String) -> String throws {{\n\
         \x20   return {}\n\
         }}\n\
         fn main() {{ }}\n",
        common::undescribed_value("s")
    ));
    assert!(rust.contains("Box<dyn std::error::Error>"), "{rust}");
}

/// **A set with two members is the generated sum**
/// ([ADR-280](../../../docs/specification/adr/adr-280.md) D15), which is what
/// this record left open and what issue #179 carried: until it
/// was built, a channel named after one of two error types would have been a
/// lie, so the opaque one was the honest answer.
#[test]
fn two_error_types_are_a_sum() {
    let source = format!(
        "{CONFIG_ERROR}enum NetError {{ Down }}\n\
         impl Error for NetError {{\n\
         \x20   fn message(ref self) -> String {{ return \"down\" }}\n\
         }}\n\
         fn load(down: bool) -> i64 throws {{\n\
         \x20   if down {{ throw NetError::Down }}\n\
         \x20   throw ConfigError::NotFound(\"etc\")\n\
         }}\n\
         fn main() {{ }}\n"
    );
    assert_eq!(throws_of(&source, "load"), vec!["ConfigError", "NetError"]);
    let rust = lowered(&source);
    assert!(
        rust.contains("-> Result<i64, crate::__NikaiaThrows_ConfigError__NetError"),
        "{rust}"
    );
}

/// **The set names the error *type*, never one of its variants**
/// ([ADR-023](../../../docs/specification/adr/adr-023.md) D1, D4). A variant
/// with named fields is written as a struct literal, and the column used to
/// record `ConfigError::BadSyntax` for it — one error type, two entries, and a
/// set of two is a set nothing can be named after.
#[test]
fn a_named_field_variant_is_its_type_in_the_set() {
    let source = format!(
        "{CONFIG_ERROR}fn load(bad: bool) -> i64 throws {{\n\
         \x20   if bad {{ throw ConfigError::BadSyntax {{ line: 3, expected: \"a number\" }} }}\n\
         \x20   throw ConfigError::NotFound(\"etc\")\n\
         }}\n\
         fn main() {{ }}\n"
    );
    assert_eq!(throws_of(&source, "load"), vec!["ConfigError"]);
}

// ---------------------------------------------------------------------------
// D3: passing it on
// ---------------------------------------------------------------------------

/// **A handler that never reads the error is untouched**
/// ([ADR-308](../../../docs/specification/adr/adr-308.md)): the binding is
/// `_error` and there is no envelope to open.
#[test]
fn a_handler_that_ignores_the_error_is_untouched() {
    let rust = lowered(&format!(
        "{CONFIG_ERROR}fn load() -> i64 throws {{\n\
         \x20   throw ConfigError::NotFound(\"etc\")\n\
         }}\n\
         fn main() {{\n\
         \x20   let port = load() catch {{ 8080 }}\n\
         \x20   println(f\"{{port}}\")\n\
         }}\n"
    ));
    assert!(rust.contains("Err(_error)"), "{rust}");
    assert!(!rust.contains("__nikaia_site"), "{rust}");
}

// ---------------------------------------------------------------------------
// [ADR-280](../../../docs/specification/adr/adr-280.md): a library's type too
// ---------------------------------------------------------------------------

/// **A channel may be named after a type a *ledger* describes** (D1). Before
/// this, `named` meant *declared by this unit*, so a function that read a file
/// had a set of exactly one named member and still travelled in the box —
/// which is the shape issue #179 carried as a measurement.
#[test]
fn a_librarys_error_type_is_a_channel() {
    let rust = lowered(
        "use std::fs\n\
         fn load(path: ref String) -> String throws {\n\
         \x20   return fs::read_to_string(ref path, fs::Root::Anywhere)\n\
         }\n\
         fn main() { }\n",
    );
    assert!(
        rust.contains("-> Result<String, nikaia_std::error::Thrown<io::IoError>>"),
        "{rust}"
    );
    assert!(!rust.contains("Box<dyn std::error::Error>"), "{rust}");
}

/// **It travels in an envelope with no site**
/// ([ADR-280](../../../docs/specification/adr/adr-280.md) D13, which replaced
/// ADR-280 D13's *bare*): the envelope is what carries the list a caller hands
/// on, and a `?` from `std`'s bare error puts it on - so the call is still the
/// plain `?` the language below writes.
#[test]
fn a_librarys_error_travels_in_an_envelope() {
    let rust = lowered(
        "use std::fs\n\
         fn load(path: ref String) -> String throws {\n\
         \x20   return fs::read_to_string(ref path, fs::Root::Anywhere)\n\
         }\n\
         fn main() { }\n",
    );
    assert!(rust.contains("Thrown<io::IoError>"), "{rust}");
    // **The root gets its own `&` from the compiler** (ADR-094 D1): the entry only
    // reads it, so the declaration is `&Root` and the call writes no reference.
    assert!(
        rust.contains(
            "fs::read_to_string(std::path::Path::new(&(&path)), &fs::Root::Anywhere).await?"
        ),
        "{rust}"
    );
}

/// **A program's own type still gets the envelope** (D2's other half), because
/// there the `throw` is the program's and has a site worth carrying.
#[test]
fn the_programs_own_error_still_travels_in_an_envelope() {
    let rust = lowered(&format!(
        "{CONFIG_ERROR}fn load() -> i64 throws {{\n\
         \x20   throw ConfigError::NotFound(\"etc\")\n\
         }}\n\
         fn main() {{ }}\n"
    ));
    assert!(
        rust.contains("nikaia_std::error::Thrown<ConfigError"),
        "{rust}"
    );
}

/// **The set names the type and not the module** (D4). A variant of a type that
/// lives in a module is written with three segments, and both the derivation
/// and the constructor exemption read the **first** of them — so a `throw
/// io::IoError::NotFound(p)` recorded `io`, and the constructor was taken for a
/// callee nothing describes.
#[test]
fn a_variant_of_a_librarys_type_names_the_type() {
    let source = "use std::io\n\
                  fn boom() -> i64 throws {\n\
                  \x20   throw io::IoError::NotFound(\"x\")\n\
                  }\n\
                  fn main() { }\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let throws = Ledger::infer(&parsed).functions["boom"].fails_with.clone();
    assert_eq!(throws, vec!["io::IoError".to_string()], "{throws:#?}");
}

/// **What a handler catches and does not pass on is not the function's**
/// (Part I 7.1, 0.0.297): the call in the guarded half fails into the handler
/// and nowhere else, so `load` throws what its handler throws - not that and
/// the guarded call's error as a sum of two. A handler that writes `throw
/// error` passes the caught error on, and then it is in the set
/// ([ADR-280](../../../docs/specification/adr/adr-280.md) D10).
#[test]
fn a_handler_that_does_not_pass_on_keeps_what_it_caught() {
    let net = "enum NetError { Down }\n\
               impl Error for NetError {\n\
               \x20   fn message(ref self) -> String { return \"down\" }\n\
               }\n\
               fn fetch() -> i64 throws { throw NetError::Down }\n";
    let kept = format!(
        "{CONFIG_ERROR}{net}fn load() -> i64 throws {{\n\
         \x20   let n = fetch() catch {{ throw ConfigError::NotFound(\"etc\") }}\n\
         \x20   return n\n\
         }}\n\
         fn main() {{ }}\n"
    );
    assert_eq!(throws_of(&kept, "load"), vec!["ConfigError"]);
    let passed = format!(
        "{CONFIG_ERROR}{net}fn load(strict: bool) -> i64 throws {{\n\
         \x20   let n = fetch() catch {{\n\
         \x20       if strict {{ throw error }}\n\
         \x20       throw ConfigError::NotFound(\"etc\")\n\
         \x20   }}\n\
         \x20   return n\n\
         }}\n\
         fn main() {{ }}\n"
    );
    assert_eq!(throws_of(&passed, "load"), vec!["ConfigError", "NetError"]);
}
