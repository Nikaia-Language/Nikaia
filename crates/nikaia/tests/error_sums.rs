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

/// Lower it, compile it, run it, and hand back what it printed. Reading the
/// Rust is not enough for a channel: a sum that looks right and does not
/// compile is the defect this closes.
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
// D3: what a handler sees
// ---------------------------------------------------------------------------

/// **A handler matches on the members' variants**, in one block, and the
/// failure that arrives decides which arm runs. This one is the library's half.
#[test]
fn a_librarys_member_is_matched_by_variant() {
    let printed = output(
        "sum-library-member",
        &format!(
            "{TWO_WAYS}fn main() {{\n\
             \x20   let text = load(\"nope.txt\") catch {{\n\
             \x20       match error {{\n\
             \x20           ConfigError::Empty(p) => f\"empty: {{p}}\"\n\
             \x20           io::IoError::NotFound(p) => f\"missing: {{p.display()}}\"\n\
             \x20           else => \"other\".clone()\n\
             \x20       }}\n\
             \x20   }}\n\
             \x20   println(f\"{{text}}\")\n\
             }}\n"
        ),
    );
    assert_eq!(printed, "missing: nope.txt");
}

/// **And this one is the program's own half**, which also has to carry its site
/// ([ADR-023](../../../docs/specification/adr/adr-023.md) D6) — the envelope
/// inside the member, opened where the patterns need the value.
#[test]
fn the_programs_own_member_is_matched_and_keeps_its_site() {
    let dir = common::scratch_dir("sum-own-member");
    let empty = dir.join("empty.conf");
    std::fs::write(&empty, "").expect("write an empty file");
    let printed = output(
        "sum-own-member-run",
        &format!(
            "{TWO_WAYS}fn main() {{\n\
             \x20   let text = load(\"{}\") catch {{\n\
             \x20       println(f\"{{error.full()}}\")\n\
             \x20       match error {{\n\
             \x20           ConfigError::Empty(p) => \"empty\".clone()\n\
             \x20           else => \"other\".clone()\n\
             \x20       }}\n\
             \x20   }}\n\
             \x20   println(f\"{{text}}\")\n\
             }}\n",
            empty.display()
        ),
    );
    assert!(printed.contains("is empty"), "{printed}");
    assert!(printed.contains("raised at load"), "{printed}");
    assert!(printed.ends_with("empty"), "{printed}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **`throw error` puts the failure back in the variant it came out of** (D3),
/// with the site it was raised at where there is one. Two function boundaries,
/// and the message still names the file.
#[test]
fn a_handler_passes_the_rest_on() {
    let printed = output(
        "sum-passed-on",
        &format!(
            "{TWO_WAYS}fn again(path: ref String) -> String throws {{\n\
             \x20   return load(path) catch {{\n\
             \x20       match error {{\n\
             \x20           ConfigError::Empty(p) => f\"defaulted for {{p}}\"\n\
             \x20           else => throw error\n\
             \x20       }}\n\
             \x20   }}\n\
             }}\n\
             fn main() {{\n\
             \x20   let text = again(\"nope.txt\") catch {{ f\"gave up: {{error}}\" }}\n\
             \x20   println(f\"{{text}}\")\n\
             }}\n"
        ),
    );
    assert!(printed.starts_with("gave up:"), "{printed}");
    assert!(printed.contains("nope.txt"), "{printed}");
}

/// **A member the handler says nothing about still gets an arm**, because the
/// sum is an `enum` below and a `match` over one has to cover it. What runs
/// there is the catch-all, which is what the source meant by not naming it.
#[test]
fn a_member_the_handler_ignores_falls_through() {
    let printed = output(
        "sum-unnamed-member",
        &format!(
            "{TWO_WAYS}fn main() {{\n\
             \x20   let text = load(\"nope.txt\") catch {{\n\
             \x20       match error {{\n\
             \x20           ConfigError::Empty(p) => f\"empty: {{p}}\"\n\
             \x20           else => \"fell through\".clone()\n\
             \x20       }}\n\
             \x20   }}\n\
             \x20   println(f\"{{text}}\")\n\
             }}\n"
        ),
    );
    assert_eq!(printed, "fell through");
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
