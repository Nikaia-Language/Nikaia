//! A name the **language below** reserves, written in every position this
//! language has one ([ADR-298](../../../docs/specification/adr/adr-298.md)).
//!
//! `let type = 3` used to lower to `let type = 3;` and `rustc` answered
//! *"expected identifier, found keyword `type`"* about a file nobody wrote, with
//! *"escape `type` to use it as an identifier"* as the help — advice that means
//! nothing in this language, which is
//! [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s class.
//!
//! **The sweep is the coverage proof and that is why it is shaped like this.**
//! An escape applied at the position that *declares* a name and missed at the
//! position that *refers* to one is worse than no escape at all: the program
//! then fails for a reason that is harder to read than the one it started with.
//! Arguing that every position was found is not the same as showing it, so every
//! word goes through every position and the result is handed to `rustc`.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

/// The words the emitter escapes, as its own list writes them.
///
/// Kept here as a literal rather than read from the compiler, on purpose: a test
/// that takes its input from the code under test cannot notice the code losing
/// an entry.
const RESERVED_BELOW: &[&str] = &[
    "abstract", "async", "await", "become", "box", "const", "do", "dyn", "final", "gen", "loop",
    "macro", "mod", "move", "override", "priv", "static", "try", "type", "typeof", "unsized",
    "virtual", "where", "yield",
];

/// The words that have left the sweep since: reserved words of **this** language
/// now, so no program can put one in a name position and the escape can never
/// fire for it. `trait` since
/// [ADR-295](../../../docs/specification/adr/adr-295.md), and `extern` and
/// `unsafe` with their constructs
/// ([ADR-124](../../../docs/specification/adr/adr-124.md) D1) — the two words
/// Part III 15.1 writes, reserved on a measurement that came to zero. All three
/// are words Rust reserves *and* this language now does, which is why the escape
/// can never fire for them.
///
/// They stay in the emitter's list on purpose — that list says what the language
/// *below* reserves, which is still true of all of them, and un-reserving here
/// is the free direction ([ADR-292](../../../docs/specification/adr/adr-292.md) D7).
///
/// **`ref` joined them at 0.0.133**
/// ([ADR-184](../../../docs/specification/adr/adr-184.md) D1), and this test is
/// what decided it. Counting the word over every `.nika` file in the repository
/// gave **zero** uses, which says it is free to reserve or to leave — and it is
/// not free to leave: `ref(x)` is a borrow of `(x)` to the grammar and a call to
/// a function named `ref` to its author, in the same characters. A word that is
/// a name in one position and an operator in the next is worse than a reserved
/// one, because the reading nobody meant is the silent one. The corpus could not
/// see that; a program that writes the word in **every** position could.
///
/// **`macro` went the other way and is in the sweep again**
/// ([ADR-298](../../../docs/specification/adr/adr-298.md) D6). It had been
/// reserved *against* a construct rather than for one, which is the ground
/// [ADR-298](../../../docs/specification/adr/adr-298.md) D1 does not accept, and
/// leaving the list is what put it back where a Nikaia name can be one — so the
/// escape can fire for it again. `const` and `loop` joined it there, and `quote`
/// is a keyword in neither language and needs nothing.
const RESERVED_HERE_TOO: &[&str] = &["trait", "extern", "unsafe", "ref"];

/// The words Rust takes as identifiers, which must therefore **not** be escaped.
///
/// The other half of the measurement, and it is short: `union` is the only one
/// of the sweep's candidates Rust accepts as a name. `gen` was one until the
/// emitted code moved to Edition 2024, which reserves it (ADR-220).
const NOT_RESERVED_BELOW: &[&str] = &["union", "counter"];

/// The words the language below reserves and **cannot escape** — `r#crate` is
/// answered with *"`crate` cannot be a raw identifier"*. There is nothing to
/// write, so the program is refused here instead (`NK1128`).
const UNESCAPABLE_BELOW: &[&str] = &["crate", "super", "Self"];

/// Every position this language can put a name in, in one program.
///
/// The call to the function named `{word}` stands in a function of its own,
/// because anything that *binds* the name shadows it — a local in `main`, a
/// parameter in `holds`. That is not a thing this test is about; measured the
/// hard way, by two versions where every word failed as *"expected function,
/// found `i64`"* and the program rather than the compiler was wrong.
fn every_position(word: &str) -> String {
    format!(
        r#"struct Box {{
    {word}: i64,
}}

fn {word}(x: i64) -> i64 {{
    return x
}}

fn holds({word}: i64) -> i64 {{
    return {word}
}}

fn calls(x: i64) -> i64 {{
    return {word}(x)
}}

fn main() {{
    let {word} = 1
    let b = Box {{ {word}: {word} }}
    let total = holds(b.{word}) + calls(2)
    let mut xs = Vec()
    xs.push(total)
    for {word} in xs {{
        println(f"{{{word}}}")
    }}
}}
"#
    )
}

fn lowered(purpose: &str, source: &str) -> String {
    let parsed = parse_to_ast(source).unwrap_or_else(|e| panic!("{purpose} parses: {e}"));
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let found = check::check(&parsed, &own, &library).findings;
    assert!(
        found.is_empty(),
        "{purpose} is a correct program and the checker says otherwise: {found:#?}"
    );
    emit_program(&parsed, Build::default())
        .unwrap_or_else(|e| panic!("{purpose} lowers: {e}"))
        .rust
}

/// Compile the lowering and hand back `rustc`'s complaint, if it had one.
fn rejected(purpose: &str, rust: &str) -> Option<String> {
    let dir = common::scratch_dir(purpose);
    let file = dir.join("lowered.rs");
    std::fs::write(&file, rust).expect("write the Rust");
    let out = common::compile(
        &file,
        &[
            "--crate-type",
            "lib",
            "--emit=metadata",
            "-o",
            dir.join("lowered.rmeta").to_str().expect("utf-8 path"),
        ],
    );
    let complaint = String::from_utf8_lossy(&out.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    (!out.status.success()).then_some(complaint)
}

/// **The sweep.** Twenty-one words, every position, compiled.
#[test]
fn a_name_the_language_below_reserves_compiles_in_every_position() {
    let mut leaked = Vec::new();
    for word in RESERVED_BELOW {
        let purpose = format!("reserved-below-{word}");
        let rust = lowered(&purpose, &every_position(word));
        if let Some(complaint) = rejected(&purpose, &rust) {
            let line = complaint
                .lines()
                .find(|l| l.starts_with("error"))
                .unwrap_or("(no error line)")
                .to_string();
            leaked.push(format!("  {word}: {line}"));
        }
    }
    assert!(
        leaked.is_empty(),
        "these reach the language below unescaped:\n{}",
        leaked.join("\n")
    );
}

/// The other direction, which is what stops the escape from over-reaching: a
/// word Rust takes as an identifier is written as the source wrote it.
#[test]
fn a_word_the_language_below_allows_is_not_escaped() {
    for word in NOT_RESERVED_BELOW {
        let purpose = format!("allowed-below-{word}");
        let rust = lowered(&purpose, &every_position(word));
        assert!(
            !rust.contains(&format!("r#{word}")),
            "`{word}` is a legal Rust identifier and must not be escaped:\n{rust}"
        );
        if let Some(complaint) = rejected(&purpose, &rust) {
            panic!(
                "`{word}` in every position does not compile:\n{complaint}\n--- the Rust ---\n{rust}"
            );
        }
    }
}

/// The escape is invisible to the program's own output: a field called `type`
/// still prints what was put in it.
#[test]
fn an_escaped_name_still_names_the_same_value() {
    let source = r#"
struct Event {
    type: String,
    at: i64,
}

fn main() {
    let e = Event { type: "click".to_string(), at: 7 }
    println(f"{e.type} {e.at}")
}
"#;
    let rust = lowered("an escaped field", source);
    assert!(
        rust.contains("r#type: String"),
        "the declaration is escaped:\n{rust}"
    );
    let dir = common::scratch_dir("an-escaped-field-runs");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "it compiles:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("the program runs");
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "click 7",
        "`type` names the same value it was given"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// D3: the three the language below cannot escape are refused **here**, in this
/// language's words, at every position that declares a name.
#[test]
fn a_name_that_cannot_be_escaped_is_refused() {
    for word in UNESCAPABLE_BELOW {
        let source = every_position(word);
        let parsed = parse_to_ast(&source).unwrap_or_else(|e| panic!("`{word}` parses: {e}"));
        let own = Ledger::infer(&parsed);
        let library = Ledger::parse(STD).expect("std's shipped ledger parses");
        let found = check::check(&parsed, &own, &library).findings;
        assert!(
            found.iter().any(|f| f.code == "NK1128"),
            "`{word}` cannot be escaped and has to be refused: {found:#?}"
        );
        let refusal = found
            .iter()
            .find(|f| f.code == "NK1128")
            .expect("the refusal");
        assert!(
            refusal.message.contains(word),
            "it names the word: {}",
            refusal.message
        );
    }
}

/// And the positions are the ones a declaration can stand in — an item's own
/// name included, which is the half `NK1119`'s walk never had to cover because
/// `struct self` cannot parse and `struct crate` can.
#[test]
fn every_declaring_position_asks() {
    let positions = [
        (
            "a struct",
            "struct crate {\n    at: i64,\n}\n\nfn main() {\n    println(\"x\")\n}\n",
        ),
        (
            "an enum",
            "enum crate {\n    One,\n}\n\nfn main() {\n    println(\"x\")\n}\n",
        ),
        (
            "a variant",
            "enum Kind {\n    crate,\n}\n\nfn main() {\n    println(\"x\")\n}\n",
        ),
        (
            "a function",
            "fn crate() -> i64 {\n    return 1\n}\n\nfn main() {\n    println(f\"{crate()}\")\n}\n",
        ),
        (
            "a field",
            "struct Row {\n    crate: i64,\n}\n\nfn main() {\n    println(\"x\")\n}\n",
        ),
        (
            "a parameter",
            "fn takes(crate: i64) -> i64 {\n    return crate\n}\n\nfn main() {\n    println(f\"{takes(1)}\")\n}\n",
        ),
        (
            "a `let`",
            "fn main() {\n    let crate = 1\n    println(f\"{crate}\")\n}\n",
        ),
        (
            "a `for` binding",
            "fn main() {\n    let mut xs = Vec()\n    xs.push(1)\n    for crate in xs {\n        println(f\"{crate}\")\n    }\n}\n",
        ),
    ];
    for (what, source) in positions {
        let parsed = parse_to_ast(source).unwrap_or_else(|e| panic!("{what} parses: {e}"));
        let own = Ledger::infer(&parsed);
        let library = Ledger::parse(STD).expect("std's shipped ledger parses");
        let found = check::check(&parsed, &own, &library).findings;
        assert!(
            found.iter().any(|f| f.code == "NK1128"),
            "{what} called `crate` has to be refused: {found:#?}"
        );
    }
}

/// A word this language reserves cannot reach a name position at all, so the
/// escape is unreachable rather than wrong.
#[test]
fn a_word_this_language_reserves_never_reaches_the_escape() {
    for word in RESERVED_HERE_TOO {
        assert!(
            nikaia::parser::RESERVED_WORDS.contains(word),
            "`{word}` is reserved here"
        );
        assert!(
            parse_to_ast(&every_position(word)).is_err(),
            "so a program cannot put `{word}` in a name position"
        );
    }
}
