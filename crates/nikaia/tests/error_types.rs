//! The failure channel is **the error type**, where the ledger names one
//! ([ADR-157](../../../docs/specification/adr/adr-157.md)).
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

mod common;

use nikaia::contracts::{Ledger, STD};
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

fn throws_of(source: &str, of: &str) -> Vec<String> {
    let parsed = parse_to_ast(source).expect("the source parses");
    Ledger::infer(&parsed).functions[of].throws.clone()
}

/// Lower it, compile it, run it, and hand back what it printed — which is the
/// only way to hold C.1 closed: a lowering that *looks* right and does not
/// compile is exactly the defect.
fn output(purpose: &str, source: &str) -> String {
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    assert!(
        ran.status.success(),
        "the program runs:\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8_lossy(&ran.stdout).trim().to_string()
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
/// [ADR-158](../../../docs/specification/adr/adr-158.md) gave `std` names, and
/// [ADR-159](../../../docs/specification/adr/adr-159.md) D1 let a channel be
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
/// ([ADR-160](../../../docs/specification/adr/adr-160.md) D1), which is what
/// this record left open and what `docs/open-work.md` §2.13 carried: until it
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

/// **An error that carries a view is a type with a lifetime**, and a typed
/// channel is what lets it have one: `Box<dyn Error>` is `'static`, so
/// `ConfigError::NotFound(path)` — Part I 7.1's own line — used to be
/// *borrowed data escapes outside of function* (`E0521`) about a generated
/// file.
#[test]
fn an_error_that_borrows_the_caller_s_buffer_compiles() {
    let printed = output(
        "error-borrowing",
        &format!(
            "{CONFIG_ERROR}fn load(path: ref String) -> i64 throws {{\n\
             \x20   throw ConfigError::NotFound(path)\n\
             }}\n\
             fn main() {{\n\
             \x20   let port = load(\"etc\") catch {{ 8080 }}\n\
             \x20   println(f\"{{port}}\")\n\
             }}\n"
        ),
    );
    assert_eq!(printed, "8080");
}

// ---------------------------------------------------------------------------
// D2: what a handler was handed
// ---------------------------------------------------------------------------

/// **Part I 7.1's own `catch`, as a program that runs.** This is the defect the
/// record closes: the page writes this and called it implemented, and it did
/// not compile.
#[test]
fn the_pages_own_catch_runs() {
    let printed = output(
        "error-catch-page",
        &format!(
            "{CONFIG_ERROR}fn load() -> i64 throws {{\n\
             \x20   throw ConfigError::BadSyntax {{ line: 7, expected: \"a number\" }}\n\
             }}\n\
             fn main() {{\n\
             \x20   let port = load() catch {{\n\
             \x20       match error {{\n\
             \x20           ConfigError::NotFound(p) => 1\n\
             \x20           ConfigError::BadSyntax {{ line, .. }} => line\n\
             \x20           else => 8080\n\
             \x20       }}\n\
             \x20   }}\n\
             \x20   println(f\"{{port}}\")\n\
             }}\n"
        ),
    );
    assert_eq!(printed, "7");
}

/// **`{error}` is still the message the author wrote** (Part I 7.1), because
/// opening the envelope hands the handler the author's own value.
#[test]
fn the_short_form_is_the_authors_message() {
    let printed = output(
        "error-short-form",
        &format!(
            "{CONFIG_ERROR}fn load() -> i64 throws {{\n\
             \x20   throw ConfigError::NotFound(\"etc\")\n\
             }}\n\
             fn main() {{\n\
             \x20   let port = load() catch {{\n\
             \x20       println(f\"{{error}}\")\n\
             \x20       8080\n\
             \x20   }}\n\
             \x20   println(f\"{{port}}\")\n\
             }}\n"
        ),
    );
    assert_eq!(printed, "no config at etc\n8080");
}

/// **And `error.full()` still names the site**
/// ([ADR-023](../../../docs/specification/adr/adr-023.md) D6). The envelope was
/// opened at the binding, so the long form is assembled from the two halves
/// rather than from one value — which is the lowering's business and not the
/// page's.
#[test]
fn the_long_form_names_the_site() {
    let printed = output(
        "error-long-form",
        &format!(
            "{CONFIG_ERROR}fn load() -> i64 throws {{\n\
             \x20   throw ConfigError::NotFound(\"etc\")\n\
             }}\n\
             fn main() {{\n\
             \x20   let port = load() catch {{\n\
             \x20       println(f\"{{error.full()}}\")\n\
             \x20       8080\n\
             \x20   }}\n\
             \x20   println(f\"{{port}}\")\n\
             }}\n"
        ),
    );
    assert!(printed.contains("no config at etc"), "{printed}");
    assert!(printed.contains("raised at load"), "{printed}");
    // **And it says a trace was not captured**
    // ([ADR-036](../../../docs/specification/adr/adr-036.md)), rather than
    // leaving a reader to wonder whether one was lost. This is the line that
    // caught the first shape of D2: opening the envelope kept the site and
    // dropped the trace, so the long form was two lines and silently shorter.
    assert!(printed.contains("NIKAIA_TRACE=1"), "{printed}");
}

// ---------------------------------------------------------------------------
// D3: passing it on
// ---------------------------------------------------------------------------

/// **`throw error` passes the error on** (D3), and it keeps the site it was
/// raised at rather than taking the handler's — which is what
/// [ADR-023](../../../docs/specification/adr/adr-023.md) D6 asks for.
#[test]
fn a_handler_may_pass_the_error_on() {
    let printed = output(
        "error-passed-on",
        &format!(
            "{CONFIG_ERROR}fn load() -> i64 throws {{\n\
             \x20   throw ConfigError::NotFound(\"etc\")\n\
             }}\n\
             fn again() -> i64 throws {{\n\
             \x20   return load() catch {{\n\
             \x20       match error {{\n\
             \x20           ConfigError::BadSyntax {{ .. }} => 1\n\
             \x20           else => throw error\n\
             \x20       }}\n\
             \x20   }}\n\
             }}\n\
             fn main() {{\n\
             \x20   let port = again() catch {{ println(f\"{{error.full()}}\") 8080 }}\n\
             \x20   println(f\"{{port}}\")\n\
             }}\n"
        ),
    );
    assert!(printed.contains("raised at load"), "{printed}");
    assert!(printed.ends_with("8080"), "{printed}");
}

/// **A failure propagates without being written**
/// ([ADR-023](../../../docs/specification/adr/adr-023.md) D8), and a typed
/// channel carries it the same way a box did: the caller's set is the callee's,
/// so the two agree by construction.
#[test]
fn a_failure_propagates_through_a_typed_channel() {
    let printed = output(
        "error-propagates",
        &format!(
            "{CONFIG_ERROR}fn load() -> i64 throws {{\n\
             \x20   throw ConfigError::NotFound(\"etc\")\n\
             }}\n\
             fn outer() -> i64 throws {{\n\
             \x20   return load()\n\
             }}\n\
             fn main() {{\n\
             \x20   let port = outer() catch {{ 8080 }}\n\
             \x20   println(f\"{{port}}\")\n\
             }}\n"
        ),
    );
    assert_eq!(printed, "8080");
}

/// **A handler that never reads the error is untouched**
/// ([ADR-090](../../../docs/specification/adr/adr-090.md)): the binding is
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
// [ADR-159](../../../docs/specification/adr/adr-159.md): a library's type too
// ---------------------------------------------------------------------------

/// **A channel may be named after a type a *ledger* describes** (D1). Before
/// this, `named` meant *declared by this unit*, so a function that read a file
/// had a set of exactly one named member and still travelled in the box —
/// which is the shape `docs/open-work.md` carried as a measurement.
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
/// ([ADR-241](../../../docs/specification/adr/adr-241.md) D1, which replaced
/// ADR-159 D2's *bare*): the envelope is what carries the list a caller hands
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
        rust.contains("fs::read_to_string(&path, &fs::Root::Anywhere).await?"),
        "{rust}"
    );
}

/// **The whole of it, as a program that runs**: a failure crosses a function
/// boundary and the handler takes it apart by variant.
#[test]
fn a_failure_from_std_is_matched_by_variant() {
    let printed = output(
        "library-error-matched",
        "use std::fs\n\
         use std::io\n\
         fn load(path: ref String) -> String throws {\n\
         \x20   return fs::read_to_string(ref path, fs::Root::Anywhere)\n\
         }\n\
         fn main() {\n\
         \x20   let text = load(\"nope.txt\") catch {\n\
         \x20       match error {\n\
         \x20           io::IoError::NotFound(p) => f\"no file: {p}\"\n\
         \x20           else => \"other\".clone()\n\
         \x20       }\n\
         \x20   }\n\
         \x20   println(f\"{text}\")\n\
         }\n",
    );
    assert_eq!(printed, "no file: nope.txt");
}

/// **`{error}` is the message and `error.full()` says there is no site** (D3),
/// which is not a gap but the truth: no `throw` in this program raised it, and
/// it is the same sentence the opaque channel has always used for an error that
/// came from below.
#[test]
fn the_long_form_says_there_is_no_site() {
    let printed = output(
        "library-error-full",
        "use std::fs\n\
         fn load(path: ref String) -> String throws {\n\
         \x20   return fs::read_to_string(ref path, fs::Root::Anywhere)\n\
         }\n\
         fn main() {\n\
         \x20   let text = load(\"nope.txt\") catch {\n\
         \x20       println(f\"{error.full()}\")\n\
         \x20       \"fallback\".clone()\n\
         \x20   }\n\
         \x20   println(f\"{text}\")\n\
         }\n",
    );
    assert!(printed.contains("nope.txt"), "{printed}");
    assert!(printed.contains("no site recorded"), "{printed}");
    assert!(printed.ends_with("fallback"), "{printed}");
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
    let throws = Ledger::infer(&parsed).functions["boom"].throws.clone();
    assert_eq!(throws, vec!["io::IoError".to_string()], "{throws:#?}");
}

/// **The list survives a hop to a caller that only propagates**
/// ([ADR-241](../../../docs/specification/adr/adr-241.md), `open-work.md`
/// §2.25): `pair` joins two failing reads, and `relay` hands its failure on.
/// `relay`'s channel was a bare `io::IoError`, which nothing converts an
/// envelope into, and `rustc` refused the file; now it is the envelope, and
/// the handler reads both failures, in each of the two places.
#[test]
fn a_joined_failure_survives_a_caller_that_propagates_it() {
    let printed = output(
        "library-error-hop",
        "use std::fs\n\
         \n\
         fn pair() -> String throws {\n\
         \x20   let both = overlap {\n\
         \x20       fs::read_to_string(\"missing-one.txt\", fs::Root::Anywhere)\n\
         \x20       fs::read_to_string(\"missing-two.txt\", fs::Root::Anywhere)\n\
         \x20   }\n\
         \x20   return f\"{both.0}{both.1}\"\n\
         }\n\
         \n\
         fn relay() -> String throws {\n\
         \x20   return pair()\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let a = pair() catch {\n\
         \x20       println(f\"pair: {error.full()}\")\n\
         \x20       f\"\"\n\
         \x20   }\n\
         \x20   let b = relay() catch {\n\
         \x20       match error {\n\
         \x20           io::IoError::NotFound(what) => println(f\"relay: not found {what}\")\n\
         \x20           else => println(\"relay: other\")\n\
         \x20       }\n\
         \x20       println(f\"{error.full()}\")\n\
         \x20       f\"\"\n\
         \x20   }\n\
         \x20   println(f\"{a}{b}\")\n\
         }\n",
    );
    assert!(
        printed.contains("relay: not found missing-one.txt"),
        "{printed}"
    );
    assert_eq!(printed.matches("and then:").count(), 2, "{printed}");
    assert_eq!(printed.matches("missing-two.txt").count(), 2, "{printed}");
}
