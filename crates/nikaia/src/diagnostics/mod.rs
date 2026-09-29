// crates/nikaia/src/diagnostics/mod.rs
//
// Where an error is reported.
//
// Stage 0 emits Rust and hands it to `rustc`, which then reports everything -
// its own type errors and the parser backend's frame check alike - against a
// file the user never wrote. That is not a small annoyance: it is the whole
// difference between a compiler and a code generator with a compiler bolted on.
//
// The fix is not a second checker (ADR-011 D3). It is a map: the emitter
// records which `.nika` span produced which byte range of the emitted Rust
// (`emit::SourceMap`), rustc reports byte offsets into that same text with
// `--error-format=json`, and this module trades one for the other.
//
// What it deliberately does not do is rewrite the message. Because the lowering
// is name-for-name (ADR-011 D2), "`until(…)` in rule `NAME` can run through the
// boundary" is already a sentence about the Nikaia source - only its position
// was wrong. Correcting a position is a lookup; translating a text would be a
// second compiler.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::ast::Span;
use crate::emit::SourceMap;

/// A statement about the **user's program**, as opposed to a failure of this
/// compiler.
///
/// The two used to leave by the same door, and the door was Rust's: a type error
/// in somebody's `.nika` file ended as an `anyhow::Error` out of `main`, so after
/// the diagnostics had said their piece the terminal also got
///
/// ```text
/// Error: 1 type error
///
/// Stack backtrace:
///    0: anyhow::error::<impl anyhow::Error>::msg
///    1: nikaia::project::check
/// ```
///
/// Ten frames of this compiler's own functions, reached whenever
/// `RUST_BACKTRACE` is set, which is a normal thing for a developer to have set.
/// Part III C.1's rule is about messages from the backend, and this is the same
/// promise from the other side: nothing about how this compiler is built reaches
/// somebody who only wrote a program.
///
/// **An error that is not one of these keeps the backtrace**, deliberately. A
/// compiler that cannot read a file or cannot run `rustc` has a failure of its
/// own, and the frames are then the most useful thing on the screen.
#[derive(Debug)]
pub struct Refused {
    pub message: String,
    /// The byte the statement this is about starts at, where the refusal knew
    /// one ([ADR-171](../../../docs/specification/adr/adr-171.md) D1).
    ///
    /// **A refusal from the lowering had no position at all**, so a reader was
    /// told what was wrong and never where — which
    /// [Part III C.2](../../../docs/specification/30-nikaia-tooling.md) asks of
    /// every diagnostic and which every `NK…` code already does. The lowering
    /// has the byte: `Flow::statement` carries it for the type checker's sake,
    /// and it is the same number.
    ///
    /// `None` where the refusal is about no statement in particular — a
    /// manifest key, a switch, a whole file.
    pub at: Option<usize>,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.message)
    }
}

impl std::error::Error for Refused {}

/// Whether an error - or anything it was wrapped in - is a statement about the
/// program rather than about this compiler.
///
/// The whole chain, because the path a file was read from is added as context
/// around a refusal and the refusal is then no longer the outermost error.
pub fn is_a_refusal(error: &anyhow::Error) -> bool {
    error.chain().any(|link| link.is::<Refused>())
}

/// A refusal, written the way `anyhow!` is written - and [`refuse!`], written the
/// way `bail!` is.
///
/// Macros, and named to mirror that pair exactly, so that a call site changes by
/// one word: `anyhow!(…)` becomes `refused!(…)` and `bail!(…)` becomes
/// `refuse!(…)`, arguments and all. What makes that worth two macros is that
/// **the choice between them is the whole decision** - is this a statement about
/// the program, or a failure of this compiler? - and a reader should be able to
/// see which was made at a glance, rather than by unpicking a `format!`.
#[macro_export]
macro_rules! refused {
    ($($arg:tt)*) => {
        $crate::diagnostics::refuse(format!($($arg)*))
    };
}

/// `bail!`, for a statement about the program. See [`refused!`].
#[macro_export]
macro_rules! refuse {
    ($($arg:tt)*) => {
        return Err($crate::diagnostics::refuse(format!($($arg)*)))
    };
}

/// [`refused!`], about a **statement the program wrote**
/// ([ADR-171](../../../docs/specification/adr/adr-171.md) D1).
///
/// The first argument is the byte it starts at, which the lowering has in
/// `Flow::statement` wherever it is emitting one.
#[macro_export]
macro_rules! refused_at {
    ($at:expr_2021, $($arg:tt)*) => {
        $crate::diagnostics::refuse_at($at, format!($($arg)*))
    };
}

/// Make a refusal, as an `anyhow::Error` so it travels the paths every other
/// error already travels.
pub fn refuse(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Refused {
        message: message.into(),
        at: None,
    })
}

/// The same, about a **statement** the program wrote
/// ([ADR-171](../../../docs/specification/adr/adr-171.md) D1).
///
/// The byte is rendered into a line and a caret by whoever holds the source,
/// which is the unit being lowered — the same arrangement `render_finding` has,
/// because a refusal the lowering makes and one the checker makes should not
/// look different to a reader.
pub fn refuse_at(at: usize, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Refused {
        message: message.into(),
        at: Some(at),
    })
}

/// A refusal's position, where it has one and the error is a refusal at all.
pub fn refusal_at(error: &anyhow::Error) -> Option<(usize, String)> {
    error
        .chain()
        .find_map(|link| link.downcast_ref::<Refused>())
        .and_then(|refused| refused.at.map(|at| (at, refused.message.clone())))
}

/// **A refusal from the lowering, on the line it is about**
/// ([ADR-171](../../../docs/specification/adr/adr-171.md) D2).
///
/// The same shape `NK2202` and every relayed `rustc` message use, because a
/// rule this compiler enforces in the lowering should not look different from
/// one it enforces in the checker. There is no code in front of it: these are
/// refusals the catalogue does not name, and inventing numbers for them is
/// [ADR-171](../../../docs/specification/adr/adr-171.md) §4's own open question.
pub fn render_refusal(message: &str, at: usize, path: &str, source: &str) -> String {
    let (line, column) = winnow_grammar::span::line_column(source, at);
    let mut out = format!("error: {message}\n");
    out.push_str(&format!("  --> {path}:{line}:{column}\n"));
    out.push_str(&winnow_grammar::span::caret(source, at, 1));
    out.push('\n');
    out
}

/// Whether a backend diagnostic is about something somebody wrote.
///
/// **A warning that maps to no Nikaia line does not reach the user.** Part III
/// C.1's rule is that a message about the generated Rust is not a message the user
/// gets, and a warning with no `.nika` line behind it is exactly that: it is about
/// a decision the emitter made. The measured case was every multi-file build
/// printing `unused import: super::*` - a `use` item this compiler writes itself -
/// with a note saying *"no Nikaia source maps to this"* beside it. Recognising it
/// and printing it anyway is the worst of the three possible behaviours.
///
/// **An error is kept even with nowhere to put it**, and that is not an
/// inconsistency. A warning suppressed costs nothing: the build goes on and the
/// user was never able to act on it. An error suppressed leaves a build that failed
/// with no reason given anywhere, which is worse than a message pointing at a
/// generated line. Such an error is a defect in this compiler - the lowering
/// produced Rust that does not compile - and it is reported so that the defect is
/// visible rather than silent.
pub fn is_about_the_program(diagnostic: &Diagnostic) -> bool {
    diagnostic.location.is_some() || diagnostic.level != "warning"
}

/// One message, placed in the `.nika` file where that was possible.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: String,
    pub message: String,
    /// Where in the `.nika` source, if the map knew.
    pub location: Option<Location>,
    /// Where in the emitted Rust it was reported, kept whether or not the map
    /// knew: an unmapped error still has to be reportable, and a mapped one is
    /// worth being able to check against its origin.
    pub generated_line: Option<usize>,
    pub notes: Vec<String>,
    /// The backend's own words, kept **only** where translating them made the
    /// message say the same thing on both sides of an `expected … found …`.
    ///
    /// That is not a message about the program: it is this compiler having
    /// emitted two different Rust types for one Nikaia type, which is a defect
    /// of its own ([ADR-056](../../../../docs/specification/adr/adr-056.md) D2).
    /// So the message becomes an internal error and this is what it carries -
    /// shown to the reader and written to a log, because whoever fixes the
    /// compiler needs the words the compiler below actually said.
    pub internal: Option<String>,
    /// What the backend wrote under the caret, where it wrote anything: the
    /// half of *mismatched types* that says which types
    /// ([ADR-237](../../../../docs/specification/adr/adr-237.md) D2 reads it).
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Location {
    /// 1-based, as an editor counts.
    pub line: usize,
    pub column: usize,
    pub span: Span,
    /// Which file of the program, as an index into the units the map was built
    /// from. A single-file program has one, and it is 0.
    ///
    /// The map has carried this since a program could be several files
    /// (ADR-012); until the project build started reading these messages,
    /// nothing asked for it, and a message about the third module would have
    /// been printed against the first module's text.
    pub unit: usize,
}

/// Turn `rustc --error-format=json` output about the emitted Rust into
/// messages about the Nikaia source.
///
/// Lines that are not JSON, and diagnostics that carry no information about a
/// place (rustc's own "aborting due to N previous errors"), are dropped.
pub fn translate(rustc_json: &str, map: &SourceMap, source: &str) -> Vec<Diagnostic> {
    translate_units(rustc_json, map, &[source])
}

/// The same, for a program of several files - and for what **`cargo`** writes.
///
/// Two things make one function serve both. A program is several `.nika` files
/// joined into one Rust file (Part I 9.1), and the map already knows which file
/// each span came from, so the line a message is about has to be looked up in
/// that file's text rather than in the first one's. And `cargo
/// --message-format=json` wraps each `rustc` diagnostic in a line of its own
/// (`{"reason": "compiler-message", "message": {...}}`), which is one
/// `["message"]` away from what `rustc --error-format=json` writes directly.
///
/// The second is why the project build can report anything at all. Until it
/// existed, `nikaia build` handed Cargo's stderr to the terminal untouched, so
/// every backend diagnostic reached the user as Rust about a generated file -
/// which Part III C.1 calls a bug in this compiler, and which
/// [ADR-005](../../../../docs/specification/adr/adr-005.md) D7 now enumerates
/// `E0277` among.
pub fn translate_units(json: &str, map: &SourceMap, sources: &[&str]) -> Vec<Diagnostic> {
    let lines: Vec<LineIndex> = sources.iter().map(|s| LineIndex::new(s)).collect();
    let mut out = Vec::new();

    for line in json.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // Cargo's own lines ("compiler-artifact", "build-finished") carry no
        // diagnostic; the one that does carries it under `message`.
        let value = match value["reason"].as_str() {
            Some("compiler-message") => value["message"].clone(),
            Some(_) => continue,
            None => value,
        };

        let level = value["level"].as_str().unwrap_or_default();
        if !matches!(level, "error" | "warning") {
            continue;
        }

        let raw = value["message"].as_str().unwrap_or_default();
        if raw.starts_with("aborting due to") {
            continue;
        }
        let message = in_this_language(raw);

        // **The one rule, and it needs no list** (ADR-056 D2). Every name this
        // compiler substituted on the way out is put back on the way in; where
        // putting it back makes the message say the same thing on *both* sides
        // of an `expected … found …`, the substitution collapsed a distinction
        // the reader would need - and it did so because this compiler wrote two
        // different Rust types for one Nikaia type, which is a defect of its
        // own. So that message is not shown as a fact about the program.
        //
        // Detected by comparing the two, which is what makes this general: a
        // substitution added later needs no entry anywhere, because it is the
        // *collapse* that is noticed and not the name.
        let internal =
            match both_sides_the_same(&message).is_some() && both_sides_the_same(raw).is_none() {
                true => Some(raw.to_string()),
                false => None,
            };

        let primary = value["spans"].as_array().and_then(|spans| {
            spans
                .iter()
                .find(|s| s["is_primary"].as_bool() == Some(true))
        });

        let generated_line = primary
            .and_then(|s| s["line_start"].as_u64())
            .map(|l| l as usize);
        let label = primary
            .and_then(|s| s["label"].as_str())
            .map(in_this_language);
        let location = primary
            .and_then(|s| s["byte_start"].as_u64())
            .and_then(|offset| map.locate(offset as usize))
            .and_then(|(unit, span)| Some(lines.get(unit)?.locate(span, unit)));

        let notes = value["children"]
            .as_array()
            .map(|children| {
                children
                    .iter()
                    .filter(|c| matches!(c["level"].as_str(), Some("note") | Some("help")))
                    .filter_map(|c| c["message"].as_str())
                    .filter(|note| !is_rust_internal(note))
                    .map(in_this_language)
                    .collect()
            })
            .unwrap_or_default();

        out.push(Diagnostic {
            level: level.to_string(),
            message,
            location,
            generated_line,
            notes,
            internal,
            label,
        });
    }

    out
}

/// **A boundary whose ledger may be stale**
/// ([ADR-237](../../../../docs/specification/adr/adr-237.md) D2): a package
/// this program depends on, or a Rust crate it describes, by the word a call
/// writes in front of `::`.
#[derive(Debug, Clone)]
pub struct Boundary {
    pub word: String,
    /// A described crate, rather than a package with a ledger of its own.
    pub described: bool,
    /// The crate's files that changed since it was described.
    pub moved: Vec<String>,
}

/// **A boundary mismatch the backend reports is said as a stale ledger**
/// ([ADR-100](../../../../docs/specification/adr/adr-100.md) D6,
/// [ADR-237](../../../../docs/specification/adr/adr-237.md) D2).
///
/// A program never meets a `Result` or a future: both are this compiler's
/// lowering of `throws` and of a pause, written off what a ledger says. So
/// where the backend says *is not a future*, *the `?` operator can only be
/// applied to values that implement `Try`*, or names a `Result`, the lowering
/// believed a ledger the code below does not match - and where the program
/// calls across a boundary, that boundary's ledger is the one to name.
///
/// Which one: the boundary the line itself calls, else one whose sources moved
/// since it was described, else the only one the file calls at all. Where none
/// of the three answers, the message is left as it is - it is then this
/// compiler's defect and is reported as the backend said it (Part III C.1).
///
/// The backend's own words are kept, as a note: this is a translation, and a
/// reader who has to check it needs what it was translated from.
pub fn a_stale_boundary(
    diagnostic: &mut Diagnostic,
    source: &str,
    boundaries: &[Boundary],
) -> bool {
    if diagnostic.level != "error" {
        return false;
    }
    let said = format!(
        "{} {} {}",
        diagnostic.message,
        diagnostic.label.as_deref().unwrap_or_default(),
        diagnostic.notes.join(" ")
    );
    let the_shape = said.contains("is not a future")
        || said.contains("can only be applied to values that implement `Try`")
        || said.contains("`Result<")
        || said.contains("enum `Result`");
    if !the_shape {
        return false;
    }
    let called = |word: &str| format!("{word}::");
    let on_the_line = diagnostic
        .location
        .as_ref()
        .and_then(|at| source.get(at.span.bytes()))
        .and_then(|text| boundaries.iter().find(|b| text.contains(&called(&b.word))));
    let in_the_file: Vec<&Boundary> = boundaries
        .iter()
        .filter(|b| source.contains(&called(&b.word)))
        .collect();
    let boundary = on_the_line
        .or_else(|| in_the_file.iter().copied().find(|b| !b.moved.is_empty()))
        .or(match in_the_file.as_slice() {
            [only] => Some(*only),
            _ => None,
        });
    let Some(boundary) = boundary else {
        return false;
    };
    let word = &boundary.word;
    let below = match &diagnostic.label {
        Some(label) if !label.is_empty() => format!("{} - {label}", diagnostic.message),
        _ => diagnostic.message.clone(),
    };
    let mut notes = vec![format!("the language below said: {below}")];
    if !boundary.moved.is_empty() {
        notes.push(format!(
            "`{word}`'s {} changed since it was described",
            boundary.moved.join(", ")
        ));
    }
    match boundary.described {
        true => {
            diagnostic.message = format!("the description of `{word}` does not match the crate");
            notes.push(format!(
                "help: run `nikaia describe {word}` again, and review what changed in \
                 `contracts/{word}.contracts` - a description is read, never checked \
                 against the crate (ADR-104 D5)"
            ));
        }
        false => {
            diagnostic.message = format!("the ledger of `{word}` does not match its sources");
            notes.push(format!(
                "help: build `{word}` again - `nikaia build` without `--locked` derives its \
                 ledger from its sources (ADR-100 D6)"
            ));
        }
    }
    notes.append(&mut diagnostic.notes);
    diagnostic.notes = notes;
    true
}

/// Whether a note from the backend tells the reader about **Rust** rather than
/// about their program.
///
/// [ADR-012](../../../../docs/specification/adr/adr-012.md): a diagnostic is
/// about the `.nika` file the user wrote. Most of what the backend says survives
/// translation because it is about the program either way - *"the literal
/// `3000000000` does not fit into the type `i32` whose range is
/// `-2147483648..=2147483647`"* is as true here as there. These two classes do
/// not, and the test for both is the same: **can the reader act on it?**
///
/// **A lint attribute cannot be written in this language**, so
/// ``#[deny(overflowing_literals)]` on by default`` and
/// ``#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default`` name
/// a thing a Nikaia program has no way to mention. They are the backend
/// explaining its own configuration.
///
/// **And Rust's tooling is not the reader's tooling.** *"use `cargo add fremd` to
/// add it to your `Cargo.toml`"* points at a file this compiler generates and the
/// reader does not edit; their file is `nikaia.toml` and there is no `nikaia add`.
/// `RUST_BACKTRACE` and `rustc --explain` are the same shape.
///
/// **And a remedy in a type the specification does not offer.**
/// *"consider using the type `u32` instead"* is what `rustc` says about a literal
/// too large for an `i32`. It was kept here once, checked, on the ground that
/// `let x: u32 = 3000000000` compiles - and it does. What changed is
/// [ADR-048](../../../../docs/specification/adr/adr-048.md) D2: the numeric
/// surface is the one Part I 2.2 names, and `u32` is deliberately not on it. So
/// the sentence points at a type a reader should not reach for, when the answer
/// is `i64` or a use that widens the literal. A remedy that works is kept; one
/// that leads out of the language is not.
///
/// **And three notes a `?.` used to hang on a move error.** `x?.field` is an
/// `Option::map`, so using `x` again is a move - a rule the language below is
/// right to hold ([ADR-005](../../../../docs/specification/adr/adr-005.md)), and
/// the headline names the program's own variable on the program's own line. The
/// notes did not: `Option::<T>::map` takes ownership, so *consider calling
/// `.as_ref()`*, `.as_mut()`, or `clone`. None of the three is anything a
/// `.nika` file can write, and all of them are about a shape this compiler
/// chose. **The headline is kept and the notes go**, which is the split this
/// function is for.
///
/// **What is deliberately kept**, because it was checked rather than assumed:
/// *"if this is intentional, prefix it with an underscore"* describes something
/// that works in Nikaia - `let _unused = 5` is accepted - so dropping it would
/// cost a reader a remedy they can follow. A note is suppressed for naming
/// something unreachable, never for sounding foreign.
fn is_rust_internal(note: &str) -> bool {
    const ATTRIBUTES: [&str; 4] = ["#[deny(", "#[warn(", "#[allow(", "#[forbid("];
    const TOOLING: [&str; 4] = [
        "Cargo.toml",
        "cargo add",
        "RUST_BACKTRACE",
        "rustc --explain",
    ];
    const NOT_OFFERED: [&str; 6] = [
        "consider using the type `u32`",
        "consider using the type `u64`",
        "consider using the type `usize`",
        // Part I 3.5: `x?.field` is an `Option::map`, so reusing `x` afterwards
        // is a move error - which the language below is right to hold
        // ([ADR-005](../../../../docs/specification/adr/adr-005.md)), and the
        // headline *"use of moved value"* names the program's own variable on
        // the program's own line. What it hangs three notes on is
        // `Option::<T>::map` and the way out of it: `.as_ref()`, `.as_mut()`,
        // and `clone`. **All three name the shape this compiler emitted**, and
        // none of them is a thing a `.nika` file can write
        // ([ADR-052](../../../../docs/specification/adr/adr-052.md) §4).
        "consider calling `.as_ref()`",
        "consider calling `.as_mut()`",
        "you can `clone` the value and consume it",
    ];

    ATTRIBUTES.iter().any(|a| note.contains(a))
        || TOOLING.iter().any(|t| note.contains(t))
        || NOT_OFFERED.iter().any(|t| note.contains(t))
}

/// **A name this compiler substituted on the way out, put back on the way in.**
///
/// The emitter writes a trusted input's map as `TrustedMap`, which is
/// `HashMap<K, V, BuildHasherDefault<FxHasher>>`
/// ([ADR-010](../../../../docs/specification/adr/adr-010.md) D5). So a `rustc`
/// message about such a map names a hasher the program never mentions - measured,
/// *"type annotations needed for `HashMap<_, _, BuildHasherDefault<FxHasher>>`"*
/// for `let m = HashMap::new()`, which is a real defect in the program reported
/// against the right line in half the backend's words.
///
/// **Every substitution is undone, and there is no list**
/// ([ADR-056](../../../../docs/specification/adr/adr-056.md) D1). This used to be
/// decided per name, on whether the substitution was *"a name and not a
/// translation"* - and `Shared[T]` was kept out on the ground that `Rc` and `Arc`
/// are different types, so a message saying *"expected `Shared[T]`, found
/// `Shared[T]`"* would hide a defect in this compiler.
///
/// **That reasoning had only two options where there are three.** Leaving Rust's
/// words standing does not help the reader either: they wrote `Shared[Conn]` and
/// have no idea what an `Rc` is, and C.1 calls an untranslated backend error
/// reaching them a bug in this compiler. The third option is the one the page
/// already names: report it **as** that bug. So the name is put back like every
/// other, and where putting it back makes the message say the same thing twice,
/// `translate_units` turns it into an internal error carrying the backend's own
/// words (D2).
///
/// **And one that is a translation rather than a name.** `x?.field` over a
/// member that is **not copied** lowers to `Option::map` (or `and_then`), which
/// takes its receiver by value - so using `x` again is a move, and the note
/// explaining *why* says *"`Option::<T>::map` takes ownership of the receiver
/// `self`"*. The explanation is the part a reader needs and the spelling is a
/// shape this compiler chose, so the spelling is replaced with the one the
/// program wrote: `?.`. This is not the `Shared[T]` case - there is no second
/// Nikaia form for it to be confused with, so nothing about this compiler can
/// hide behind it.
///
/// **Narrowed to that one case at 0.0.140**
/// ([ADR-189](../../../../docs/specification/adr/adr-189.md)). A reached
/// **method**, and a field whose member **copies**, take the receiver by
/// `as_ref()` now and there is no move left to explain; what still moves is the
/// member that would come out as a *view*, and that waits on one question about
/// `??` ([ADR-190](../../../../docs/specification/adr/adr-190.md) D2,
/// `docs/open-decisions.md`) rather than on a state - three of the four shapes
/// a `?.` has are Borrowed, and the fourth is `NK2303`'s.
/// The two types an `expected … found …` names, where both are the same.
///
/// Rust writes a type mismatch as *"expected `A`, found `B`"*, so a message
/// naming one thing twice is a message that cannot be about the program. Used on
/// both sides of the translation: a message rustc itself wrote that way is its
/// own business (two types of one name, from two crates), and one that only
/// *became* that way is this compiler's.
fn both_sides_the_same(message: &str) -> Option<String> {
    let expected = backticked_after(message, "expected ")?;
    let found = backticked_after(message, "found ")?;
    match expected == found {
        true => Some(expected),
        false => None,
    }
}

/// What stands between the first pair of backticks after `word`.
fn backticked_after(message: &str, word: &str) -> Option<String> {
    let rest = &message[message.find(word)? + word.len()..];
    let open = rest.find('`')? + 1;
    let close = rest[open..].find('`')? + open;
    Some(rest[open..close].to_string())
}

/// `Rc<T>` and `Arc<T>` are both `Shared[T]`, which is the word the program
/// wrote ([ADR-056](../../../../docs/specification/adr/adr-056.md) D1).
///
/// The brackets are matched rather than searched for, so a nested type comes
/// through whole: `Rc<Vec<Conn>>` is `Shared[Vec<Conn>]`, and the inner one is
/// then substituted by the same pass over the result.
fn shared_names(message: &str) -> String {
    const HULLS: [&str; 6] = [
        "std::rc::Rc<",
        "alloc::rc::Rc<",
        "std::sync::Arc<",
        "alloc::sync::Arc<",
        "Rc<",
        "Arc<",
    ];

    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    'outer: while !rest.is_empty() {
        for hull in HULLS {
            if rest.starts_with(hull) {
                // A bare `Rc<` must not eat the tail of `MyRc<`: a hull is a
                // name, so what stands in front of it may not be one.
                let bare = !hull.contains("::");
                let preceded_by_a_name = bare
                    && out
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':');
                if !preceded_by_a_name && let Some(inner) = balanced(&rest[hull.len()..]) {
                    out.push_str("Shared[");
                    out.push_str(&shared_names(inner));
                    out.push(']');
                    rest = &rest[hull.len() + inner.len() + 1..];
                    continue 'outer;
                }
            }
        }
        let step = rest.chars().next().map(char::len_utf8).unwrap_or(1);
        out.push_str(&rest[..step]);
        rest = &rest[step..];
    }
    out
}

/// What stands before the `>` that closes the `<` already consumed.
fn balanced(rest: &str) -> Option<&str> {
    let mut depth = 1usize;
    for (at, c) in rest.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&rest[..at]);
                }
            }
            _ => {}
        }
    }
    None
}

fn in_this_language(message: &str) -> String {
    shared_names(message)
        // **The lock's two shapes are one written name**
        // ([ADR-057](../../../../docs/specification/adr/adr-057.md)), so they go
        // back like every other substitution (ADR-056 D1) - and where a message
        // names both, the collapse is caught by the comparison that catches
        // every other one.
        .replace("nikaia_std::lock::Crossing", "Locked")
        .replace("nikaia_std::lock::Local", "Locked")
        .replace("lock::Crossing", "Locked")
        .replace("lock::Local", "Locked")
        .replace("Crossing<", "Locked<")
        .replace("Local<", "Locked<")
        .replace(", BuildHasherDefault<FxHasher>>", ">")
        .replace("nikaia_std::hash::TrustedMap", "HashMap")
        .replace("nikaia_std::hash::TrustedSet", "HashSet")
        .replace("TrustedMap", "HashMap")
        .replace("TrustedSet", "HashSet")
        .replace(
            "`Option::<T>::map` takes ownership of the receiver `self`",
            "`?.` over a member that is not copied takes the value it reaches through \
             (Part I, 3.5)",
        )
        .replace(
            "`Option::<T>::and_then` takes ownership of the receiver `self`",
            "`?.` over a member that is not copied takes the value it reaches through \
             (Part I, 3.5)",
        )
}

/// Render a diagnostic the way a compiler does: the place, the message, the
/// line it is about, and what part of it.
/// Write the backend's own words for every internal error to a log beside the
/// generated Rust, and answer where it went
/// ([ADR-056](../../../../docs/specification/adr/adr-056.md) D2).
///
/// Appended rather than replaced, because a build reports what that build found
/// and a reader collecting a report wants the run before it too. A log that
/// cannot be written costs the report and never the build: this is already a
/// message about a defect, and failing to record it would replace a bad message
/// with no message.
pub fn log_internal(diagnostics: &[Diagnostic], gen_dir: &Path) -> Option<PathBuf> {
    use std::io::Write;

    let originals: Vec<&String> = diagnostics
        .iter()
        .filter_map(|d| d.internal.as_ref())
        .collect();
    if originals.is_empty() {
        return None;
    }

    let path = gen_dir.join("internal-errors.log");
    std::fs::create_dir_all(gen_dir).ok()?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    for original in originals {
        writeln!(file, "{original}").ok()?;
    }
    Some(path)
}

pub fn render(diagnostic: &Diagnostic, path: &str, source: &str, generated_path: &str) -> String {
    let mut out = String::new();

    // **An internal error, and the backend's own words with it**
    // ([ADR-056](../../../../docs/specification/adr/adr-056.md) D2). Part III
    // C.1 says an unmapped backend error reaching the user is a bug in this
    // compiler and is reported as one rather than as normal output; this is the
    // neighbouring case, where the mapping *succeeded* and produced a message
    // that cannot be about the program. The reader needs to know it is not their
    // mistake; whoever fixes it needs the sentence `rustc` actually wrote, so
    // that is shown here as well as written to the log beside the generated Rust.
    if let Some(original) = &diagnostic.internal {
        // Where the map knew, the `.nika` line; where it did not, the generated
        // one - saying `.nika` for a line nothing maps to would be a guess, and
        // in a message that already says something went wrong here that is the
        // worst place to make one.
        let at = match &diagnostic.location {
            Some(location) => format!("{path}:{}:{}", location.line, location.column),
            None => match diagnostic.generated_line {
                Some(line) => format!("{generated_path}:{line}"),
                None => generated_path.to_string(),
            },
        };
        out.push_str(&format!(
            "internal error: {at}: this is a Nikaia bug, please report it.\n\
             \x20    = translating the backend's message left it saying the same thing \
             on both sides, which means this compiler emitted two different types \
             for one of yours\n\
             \x20    = your program may well be fine; nothing here is about a mistake \
             you made\n\
             \x20    = what the backend said: {original}\n"
        ));
        if let Some(text) = diagnostic
            .location
            .as_ref()
            .and_then(|l| source.lines().nth(l.line - 1))
        {
            out.push_str(&format!(
                "{:>4} | {text}\n",
                diagnostic.location.as_ref().map_or(0, |l| l.line)
            ));
        }
        return out;
    }

    match &diagnostic.location {
        Some(location) => {
            out.push_str(&format!(
                "{}: {}:{}:{}: {}\n",
                diagnostic.level, path, location.line, location.column, diagnostic.message
            ));

            if let Some(text) = source.lines().nth(location.line - 1) {
                let gutter = format!("{:>4} | ", location.line);
                out.push_str(&gutter);
                out.push_str(text);
                out.push('\n');

                // The caret sits under the span, counting characters so that a
                // station name in the source does not shift it.
                let width = source[diagnostic
                    .location
                    .as_ref()
                    .map(|l| l.span.bytes())
                    .unwrap_or(0..0)]
                .chars()
                .take_while(|c| *c != '\n')
                .count()
                .max(1);
                out.push_str(&" ".repeat(gutter.len() + location.column - 1));
                out.push_str(&"^".repeat(width));
                out.push('\n');
            }
        }
        None => {
            // Nothing in the map covers it: the emitted line is all there is,
            // and saying so is better than pointing somewhere plausible.
            let line = diagnostic
                .generated_line
                .map(|l| format!("{generated_path}:{l}"))
                .unwrap_or_else(|| generated_path.to_string());
            out.push_str(&format!(
                "{}: {}: {} (no Nikaia source maps to this)\n",
                diagnostic.level, line, diagnostic.message
            ));
        }
    }

    for note in &diagnostic.notes {
        out.push_str(&format!("     = {note}\n"));
    }

    out
}

/// A finding of the compiler's own, reported the way it reports rustc's.
///
/// The `.nika` file, the line it is about with a caret under it, then the
/// reasons and a way out. `--explain` has shown rustc's diagnostics in this
/// shape since ADR-012; a rule the compiler checks itself should not look
/// different from one it relays.
pub fn render_sync_violation(
    violation: &crate::contracts::sync::Violation,
    path: &str,
    source: &str,
) -> String {
    let (line, column) = winnow_grammar::span::line_column(source, violation.span.at());
    let ledger = if violation.from_library {
        "`std`'s ledger"
    } else {
        "this program's contracts"
    };

    let mut out = String::new();
    out.push_str(&format!(
        "error[NK2202]: `{}` is `{}`, and `{}` can pause\n",
        violation.caller, violation.promise, violation.callee
    ));
    out.push_str(&format!("  --> {path}:{line}:{column}\n"));
    out.push_str(&winnow_grammar::span::caret(source, violation.span.at(), 1));
    out.push('\n');
    out.push_str(
        "     = a `sync` function promises it cannot pause and does no I/O (Part II, 12.1)\n",
    );
    // **A construct has no ledger entry to name**
    // ([ADR-163](../../../docs/specification/adr/adr-163.md) D1): an `overlap`
    // and a `select` hand their branches to the executor and park until they
    // answer, which is the pause. Saying *carries no `sync`* about one would
    // send a reader looking for an entry that does not exist.
    if violation.construct {
        out.push_str(&format!(
            "     = a `{}` block hands its branches to the executor and waits \
             there, which is a pause (ADR-055)\n",
            violation.callee
        ));
    } else {
        out.push_str(&format!(
            "     = `{}` carries no `sync` in {ledger}\n",
            violation.callee
        ));
    }
    out.push_str(&format!(
        "     help: drop `{}` from `{}`, or move the call out of it\n",
        violation.promise, violation.caller
    ));
    out
}

/// One of the type checker's findings, on the `.nika` line it is about.
///
/// The same shape `NK2202` and every relayed `rustc` message use, because a
/// rule the compiler checks itself should not look different from one it
/// relays (ADR-012). Part III C.2 asks for a headline, the reason, and one
/// concrete way out; a `Finding` carries all three and this writes them down.
pub fn render_finding(finding: &crate::check::Finding, path: &str, source: &str) -> String {
    let (line, column) = winnow_grammar::span::line_column(source, finding.span.at());

    let level = match finding.severity {
        crate::check::Severity::Error => "error",
        crate::check::Severity::Warning => "warning",
    };
    let mut out = String::new();
    out.push_str(&format!("{level}[{}]: {}\n", finding.code, finding.message));
    out.push_str(&format!("  --> {path}:{line}:{column}\n"));
    out.push_str(&winnow_grammar::span::caret(source, finding.span.at(), 1));
    out.push('\n');
    for note in &finding.notes {
        out.push_str(&format!("     = {note}\n"));
    }
    if let Some(help) = &finding.help {
        out.push_str(&format!("     help: {help}\n"));
    }
    out
}

/// Byte offset -> line and column, computed once per file.
struct LineIndex {
    /// Byte offset of the start of each line.
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            source
                .char_indices()
                .filter(|(_, c)| *c == '\n')
                .map(|(i, _)| i + 1),
        );
        Self { starts }
    }

    fn locate(&self, span: Span, unit: usize) -> Location {
        let line = match self.starts.binary_search(&span.at()) {
            Ok(i) => i,
            Err(i) => i - 1,
        };

        Location {
            line: line + 1,
            column: span.at() - self.starts[line] + 1,
            span,
            unit,
        }
    }
}

#[cfg(test)]
mod stale_boundaries {
    use super::*;

    fn an_error(message: &str, span: std::ops::Range<usize>) -> Diagnostic {
        let span = Span::from(span);
        Diagnostic {
            level: "error".to_string(),
            message: message.to_string(),
            location: Some(Location {
                line: 1,
                column: 1,
                span,
                unit: 0,
            }),
            generated_line: None,
            notes: Vec::new(),
            internal: None,
            label: None,
        }
    }

    fn boundary(word: &str, described: bool) -> Boundary {
        Boundary {
            word: word.to_string(),
            described,
            moved: Vec::new(),
        }
    }

    /// **The boundary the line calls is the one named**, as a package's ledger
    /// or a crate's description, and the backend's words are kept.
    #[test]
    fn the_line_names_its_boundary() {
        let source = "let n = util::count()\nlet m = fremd::drei()\n";
        let boundaries = [boundary("util", false), boundary("fremd", true)];

        let mut on_util = an_error("`i64` is not a future", 8..21);
        assert!(a_stale_boundary(&mut on_util, source, &boundaries));
        assert_eq!(
            on_util.message,
            "the ledger of `util` does not match its sources"
        );
        assert!(on_util.notes[0].contains("is not a future"), "{on_util:#?}");

        let mut on_fremd = an_error(
            "the `?` operator can only be applied to values that implement `Try`",
            30..43,
        );
        assert!(a_stale_boundary(&mut on_fremd, source, &boundaries));
        assert_eq!(
            on_fremd.message,
            "the description of `fremd` does not match the crate"
        );
    }

    /// **Silence where the shape is not a boundary's, or no boundary is
    /// called**: that is this compiler's own defect, said as the backend said
    /// it.
    #[test]
    fn nothing_else_is_renamed() {
        let boundaries = [boundary("fremd", true)];
        let mut other = an_error("mismatched types", 0..5);
        assert!(!a_stale_boundary(&mut other, "fremd::x()", &boundaries));
        let mut unnamed = an_error("`i64` is not a future", 0..5);
        assert!(!a_stale_boundary(&mut unnamed, "let n = 1", &boundaries));
        assert_eq!(unnamed.message, "`i64` is not a future");
    }
}
