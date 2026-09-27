//! A refusal from the **lowering** names the line it is about
//! ([ADR-171](../../../docs/specification/adr/adr-171.md)).
//!
//! [Part III C.2](../../../docs/specification/30-nikaia-tooling.md) asks every
//! diagnostic for a headline, the line with a caret under it, the reason and a
//! concrete way out — and every `NK…` code gives all four. A refusal the
//! **lowering** makes gave the first and the last and nothing in between: it
//! was text, with no position at all, so a reader was told what was wrong and
//! never where.
//!
//! The byte was there the whole time. `Flow::statement` carries it for the type
//! checker's sake ([ADR-028](../../../docs/specification/adr/adr-028.md)), and
//! it is the same number.

use std::path::PathBuf;
use std::process::Command;

fn nikaia(source: &str, name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("nikaia-refusal-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory to work in");
    let file = dir.join(format!("{name}.nika"));
    std::fs::write(&file, source).expect("write the source");
    let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .arg("--input")
        .arg(&file)
        .arg("--no-cache")
        .output()
        .expect("the compiler runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success(), "this source is refused");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn a_file(name: &str) -> PathBuf {
    PathBuf::from(format!("{name}.nika"))
}

/// **A lambda that pauses, handed to what nothing describes** — the refusal
/// [ADR-233](../../../docs/specification/adr/adr-233.md) §3 leaves, and the one
/// that sent this looking, because it named a callee and no place.
#[test]
fn a_pausing_lambda_is_refused_on_its_line() {
    let said = nikaia(
        "use std::fs\n\
         \n\
         fn main() throws {\n\
         \x20   let names = Vec()\n\
         \x20   let sizes = names.par_iter().map fn(n) { fs::read_to_string(n, fs::Root::Anywhere) catch { \"\" } }\n\
         \x20   println(f\"{sizes.len()}\")\n\
         }\n",
        "pausing",
    );
    assert!(said.contains("which can pause"), "{said}");
    // The line, and the caret under the call inside the lambda.
    assert!(said.contains(":5:"), "the line it is about:\n{said}");
    assert!(said.contains("-->"), "{said}");
    assert!(said.contains('^'), "a caret under it:\n{said}");
    assert!(
        said.contains(&a_file("pausing").display().to_string()),
        "and the file:\n{said}"
    );
}

/// **An `overlap` with one branch**, which is a different site and the same
/// shape — because the position comes from the flow rather than from each
/// refusal remembering to carry one.
#[test]
fn a_block_too_small_is_refused_on_its_line() {
    let said = nikaia(
        "fn a() -> i64 { return 1 }\n\
         \n\
         fn main() {\n\
         \x20   let x = overlap {\n\
         \x20       a()\n\
         \x20   }\n\
         \x20   println(f\"{x}\")\n\
         }\n",
        "one-branch",
    );
    assert!(said.contains("at least two branches"), "{said}");
    assert!(said.contains(":4:"), "{said}");
    assert!(said.contains('^'), "{said}");
}

/// **And it reads like the checker's**, which is the point of doing it this way
/// rather than appending a line number to the sentence: a rule enforced in the
/// lowering should not look different from one enforced in the checker
/// ([ADR-012](../../../docs/specification/adr/adr-012.md)).
#[test]
fn it_reads_like_a_checkers_refusal() {
    let lowering = nikaia(
        "fn a() -> i64 { return 1 }\n\
         \n\
         fn main() {\n\
         \x20   let x = overlap {\n\
         \x20       a()\n\
         \x20   }\n\
         \x20   println(f\"{x}\")\n\
         }\n",
        "shape-lowering",
    );
    let checker = nikaia(
        "fn main() {\n\
         \x20   break\n\
         }\n",
        "shape-checker",
    );
    for said in [&lowering, &checker] {
        let head = said.lines().next().expect("a headline");
        assert!(head.starts_with("error"), "{said}");
        assert!(
            said.lines().any(|l| l.trim_start().starts_with("-->")),
            "{said}"
        );
        assert!(said.contains('|'), "{said}");
    }
}

/// **A refusal from a helper that never saw a file** gets the statement's
/// place, which is [ADR-171](../../../docs/specification/adr/adr-171.md) D1
/// one step out.
///
/// `emit::interpolation` works on the **text** of a string literal: it knows an
/// `{` was never closed and cannot know where the string is. The caller is
/// emitting a statement and knows exactly where, so the handover happens there
/// — written once rather than at each call.
#[test]
fn a_malformed_literal_is_refused_on_its_line() {
    let said = nikaia(
        "fn main() {\n\
         \x20   let n = 1\n\
         \x20   println(f\"unclosed {n\")\n\
         }\n",
        "unclosed-hole",
    );
    assert!(said.contains("unclosed `{`"), "{said}");
    assert!(said.contains(":3:"), "the line it is about:\n{said}");
    assert!(said.contains('^'), "{said}");
}

/// **And the ones with no place stay without one**, which is the scope rather
/// than a gap: a switch's value is about a manifest key and a whole build, and
/// a line number would be an invention.
#[test]
fn a_refusal_about_no_statement_has_no_line() {
    let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["--input", "/dev/null", "--user-parallelism", "vielleicht"])
        .output()
        .expect("the compiler runs");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("expected yes or no"), "{said}");
    assert!(
        !said.contains("-->"),
        "no place is invented for it:\n{said}"
    );
}

/// **An item's refusal, which [ADR-171](../../../docs/specification/adr/adr-171.md)
/// §4 left open as *a different walk*** — and which turned out not to be one:
/// the emitter's item arm is handed the item's span already, for the source
/// map's sake.
#[test]
fn an_items_refusal_names_the_item() {
    let said = nikaia(
        "extern \"stdcall\" {\n\
         \x20   fn getpid() -> i64 sync\n\
         }\n\
         \n\
         fn main() { println(f\"{unsafe { getpid() }}\") }\n",
        "unknown-abi",
    );
    assert!(
        said.contains("names an ABI this compiler does not write"),
        "{said}"
    );
    assert!(
        said.contains(":1:"),
        "the line the `extern` stands on:\n{said}"
    );
    assert!(said.contains('^'), "{said}");
}

/// **A grammar entry that names no rule.** `Tiny::two(…)` is the shape
/// [ADR-082](../../../docs/specification/adr/adr-082.md) D2 introduced, and its
/// two refusals — no such rule, and a rule that is not `pub` — were text.
#[test]
fn a_grammar_entry_that_is_not_there_names_its_line() {
    let source = "grammar Tiny {\n\
                  \x20   rule NUM -> i64 = n:dec[i64](digit+) { n }\n\
                  \x20   pub rule one -> i64 = n:NUM { n }\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let n = Tiny::{RULE}(\"7\")\n\
                  \x20   println(f\"{n}\")\n\
                  }\n";
    let missing = nikaia(&source.replace("{RULE}", "two"), "no-such-rule");
    assert!(said_about(&missing, "has no `pub` rule called `two`", 7));

    let private = nikaia(&source.replace("{RULE}", "NUM"), "rule-not-pub");
    assert!(said_about(
        &private,
        "is not `pub`, so it is not an entry",
        7
    ));
}

/// **A `dsl` whose target is no grammar this compiler has**, which is the
/// template's own family — and with it the five refusals inside `template.rs`,
/// which work on text and never saw a file. One handover covers all of them.
#[test]
fn a_template_refusal_names_its_statement() {
    let said = nikaia(
        "fn main() {\n\
         \x20   let page = dsl markdown {\n\
         \x20       # Title\n\
         \x20   } eod\n\
         \x20   println(page)\n\
         }\n",
        "no-such-dsl",
    );
    assert!(said_about(&said, "is not a grammar this compiler has", 2));
}

fn said_about(said: &str, message: &str, line: usize) -> bool {
    assert!(said.contains(message), "{said}");
    assert!(said.contains(&format!(":{line}:")), "line {line}:\n{said}");
    assert!(said.contains('^'), "a caret:\n{said}");
    true
}
