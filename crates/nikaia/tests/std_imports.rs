//! A `use` brings no name in, for `std` as for a package
//! ([ADR-140](../../../docs/specification/adr/adr-140.md) D5).
//!
//! [ADR-046](../../../docs/specification/adr/adr-046.md) D2's rule is the
//! language's and `std` was the one place it was not followed:
//! `use std::collections::HashMap` parsed, and what it did was **nothing** —
//! `HashMap` works with no `use` at all, because it is a name this compiler
//! already knows. A line that reads like an import and does nothing is what
//! this closes.

use nikaia::contracts::{Ledger, STD};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

/// **A `use` that names a type is refused** (D5), and since
/// [ADR-154](../../../docs/specification/adr/adr-154.md) D3 the way out is the
/// **module**: `HashMap` lives in one, so the line to write is
/// `use std::collections` and the name is `collections::HashMap`.
#[test]
fn a_use_that_names_a_type_is_refused() {
    let found: Vec<_> = findings("use std::collections::HashMap\nfn main() { }\n")
        .into_iter()
        .filter(|f| f.code == "NK1156")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    let help = found[0].help.as_deref().unwrap_or_default();
    assert!(help.contains("use std::collections"), "{help}");
    assert!(help.contains("collections::HashMap"), "{help}");
}

/// **A type that needs no `use` at all is told to drop the line**, which is the
/// other half: `String` is on Part I 1.3's list and is written bare.
#[test]
fn a_use_that_names_a_type_from_the_list_says_to_drop_the_line() {
    let found: Vec<_> = findings("use std::String\nfn main() { }\n")
        .into_iter()
        .filter(|f| f.code == "NK1156")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0]
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("Remove the line: `String` is always available")
    );
}

/// **A map is reached through its module** — `use std::collections` and
/// `collections::HashMap`, which is D3 and the name that makes the prelude a
/// rule rather than a tidy-up.
#[test]
fn the_prelude_needs_no_use() {
    let source = "use std::collections\n\nfn main() {\n\
                  \x20   let mut scores = collections::HashMap()\n\
                  \x20   scores[\"a\"] = 1\n\
                  \x20   println(f\"{scores.len()}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
}

/// **A module is left alone**, which is what a `use` names: `use std::fs` and
/// then `fs::read_to_string`.
#[test]
fn a_use_that_names_a_module_is_left_alone() {
    let found: Vec<_> =
        findings("use std::fs\nuse std::io\nuse std::cli\nuse std::html\nfn main() { }\n")
            .into_iter()
            .filter(|f| f.code == "NK1156")
            .collect();
    assert!(found.is_empty(), "{found:#?}");
}

/// **A module nothing describes yet is left alone too**, which is
/// [Part III C.4](../../../docs/specification/30-nikaia-tooling.md): refusing
/// on a surface that does not exist is a correct program refused.
#[test]
fn a_module_no_ledger_describes_is_not_refused() {
    let found: Vec<_> = findings("use std::db::postgres\nuse std::backend::x86\nfn main() { }\n")
        .into_iter()
        .filter(|f| f.code == "NK1156")
        .collect();
    assert!(found.is_empty(), "{found:#?}");
}

/// **A package's `use` is not this rule's**, and its own refusal is the module
/// layer's ([ADR-046](../../../docs/specification/adr/adr-046.md) D2).
#[test]
fn a_packages_use_is_untouched() {
    let found: Vec<_> = findings("use http\nfn main() { }\n")
        .into_iter()
        .filter(|f| f.code == "NK1156")
        .collect();
    assert!(found.is_empty(), "{found:#?}");
}

/// **And nothing in the tree writes the form any more**, which is the half a
/// refusal cannot check for itself.
#[test]
fn no_nika_source_in_the_tree_brings_a_name_in() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut left = Vec::new();
    let mut walk = vec![root];
    while let Some(dir) = walk.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if path.is_dir() {
                if !matches!(name.as_str(), "target" | ".git" | "vendor" | "node_modules") {
                    walk.push(path);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("nika") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (n, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("use std::collections::") {
                    left.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(left.is_empty(), "still brought in:\n{}", left.join("\n"));
}

// --- A module `std` does not have (NK1186) ---

fn refused(source: &str) -> Vec<nikaia::check::Finding> {
    findings(source)
        .into_iter()
        .filter(|f| f.code == "NK1186")
        .collect()
}

/// **`use std::<anything>` was accepted, and `rustc` was the one that said
/// otherwise** — [Part III C.1](../../../docs/specification/30-nikaia-tooling.md),
/// which says the backend must never speak about the generated file.
///
/// The `use` became a comment in the generated Rust and the call was emitted
/// verbatim, so what the programmer read was *failed to resolve: use of
/// unresolved module or unlinked crate `nosuchthing`* about a file they did not
/// write, with a `help` telling them to `cargo add` a crate that does not exist.
#[test]
fn a_std_module_nobody_declared_is_refused_here() {
    let found = refused("use std::nosuchthing\n\nfn main() { println(\"x\") }\n");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0]
            .message
            .contains("`std` has no module called `std::nosuchthing`"),
        "{:?}",
        found[0].message
    );
    // **The help prints what there is**, because a name nobody declared is most
    // often a name misremembered.
    let help = found[0].help.as_deref().unwrap_or_default();
    assert!(help.contains("fs"), "{help}");
    assert!(help.contains("io"), "{help}");
}

/// A near miss gets the name and not the list, which is the same help every other
/// misremembered name in this compiler gets.
#[test]
fn a_near_miss_is_named() {
    let found = refused("use std::collection\n\nfn main() { }\n");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].help.as_deref(),
        Some("Did you mean `use std::collections`?")
    );
}

/// **A module the crate has and `std` does not offer gets its own sentence**, and
/// that is the case that found this defect: `crates/nikaia-std/src/tools/` holds
/// [ADR-196](../../../docs/specification/adr/adr-196.md)'s Rust-signature
/// grammar, which the compiler calls and a program may not. *Nobody has written
/// that down* would be false and would send the reader looking for a typo.
#[test]
fn the_toolchains_own_module_says_why_it_is_not_reachable() {
    let found = refused("use std::tools\n\nfn main() { }\n");
    assert_eq!(found.len(), 1, "{found:#?}");
    let note = found[0].notes.join(" ");
    assert!(note.contains("toolchain's own"), "{note}");
    assert!(note.contains("a program cannot reach"), "{note}");
}

/// **A module a record names and the compiler has not built is accepted**, which
/// is [Part III C.4](../../../docs/specification/30-nikaia-tooling.md): the day
/// it lands the line is unchanged, so refusing it today refuses a correct
/// program. `use std::db` is how
/// [ADR-143](../../../docs/specification/adr/adr-143.md)'s driver is reached.
#[test]
fn a_module_a_record_promises_is_accepted() {
    for module in [
        "db", "json", "process", "thread", "panic", "build", "task", "backend",
    ] {
        let source = format!("use std::{module}\n\nfn main() {{ }}\n");
        assert!(
            refused(&source).is_empty(),
            "`use std::{module}` is a record's and not a typo"
        );
    }
}

/// **The answer is the module the path starts at**, which is the reading
/// [ADR-140](../../../docs/specification/adr/adr-140.md) already gave a longer
/// path: `use std::db::postgres` is `db`'s question, and one that starts nowhere
/// is refused whatever follows it.
#[test]
fn a_longer_path_is_answered_by_the_module_it_starts_at() {
    assert!(refused("use std::db::postgres\nfn main() { }\n").is_empty());
    assert!(refused("use std::backend::x86\nfn main() { }\n").is_empty());
    assert_eq!(
        refused("use std::nosuchthing::deeper\nfn main() { }\n").len(),
        1
    );
}

/// **Every module `std`'s ledger declares is importable**, asked of the ledger
/// rather than of a list written here — which is the half of this check that
/// needs no upkeep: a module `std` gains is one a program may import the day it
/// lands.
#[test]
fn every_module_the_ledger_declares_is_importable() {
    for module in [
        "channel",
        "cli",
        "collections",
        "foreign",
        "fs",
        "html",
        "http1",
        "io",
        "net",
        "text",
        "time",
    ] {
        let source = format!("use std::{module}\n\nfn main() {{ }}\n");
        assert!(refused(&source).is_empty(), "`use std::{module}` is real");
    }
}

/// **And a prefix the ledger keys that is *not* a module is not importable.**
/// `str::len` and `list::ListExt::map` sit beside `fs::read` and look the same
/// from outside; a primitive and a trait are reached without a line (Part I 1.3,
/// 2.2). `str` and `i64` are refused one code earlier, as types.
#[test]
fn a_prefix_that_is_not_a_module_is_not_importable() {
    assert_eq!(refused("use std::list\nfn main() { }\n").len(), 1);
    for word in ["str", "i64"] {
        let source = format!("use std::{word}\nfn main() {{ }}\n");
        let codes: Vec<&str> = findings(&source).iter().map(|f| f.code).collect();
        assert!(
            codes.contains(&"NK1156") || codes.contains(&"NK1186"),
            "`use std::{word}` is refused by this compiler: {codes:?}"
        );
    }
}
