//! Which of Part I 6.6's states each view in a signature is in
//! ([ADR-283](../../../docs/specification/adr/adr-283.md) D2, D5).
//!
//! **The analysis alone.** Only one of the three states is a representation
//! this compiler emits — Borrowed is the language below's own lifetime, Owned
//! is `.clone()` written by the program, and **Tethered is not built**. So
//! this walk changes no lowering: it writes a column, which is D7, and the
//! column is what a later change reads when the representation exists.
//!
//! That order is the point. An analysis nothing depends on can be held against
//! the whole corpus and read, and being wrong costs a wrong line in a file
//! rather than a wrong program.

use nikaia::contracts::tether::State;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn ledger(source: &str) -> Ledger {
    Ledger::infer(&parse_to_ast(source).expect("the source parses"))
}

fn state(own: &Ledger, key: &str, position: &str) -> Option<State> {
    own.functions
        .get(key)
        .expect("the entry is there")
        .views
        .iter()
        .find(|held| held.position == position)
        .map(|held| held.state)
}

/// **A parameter is lent for the call**, so the buffer is the caller's and
/// outlives every use inside the body — which is D2's *transient = borrow*.
///
/// Whether the body **keeps** it past the call is `NK2302`'s question and a
/// refusal rather than a state.
#[test]
fn a_view_parameter_borrows() {
    let own = ledger(
        "struct Entry { pub name: ref String }\n\
         \n\
         pub fn read(data: ref String) -> Vec[Entry] { return [Entry { name: data }] }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    assert_eq!(state(&own, "read", "data"), Some(State::Borrowed));
    // And the result borrows it, because the caller's buffer outlives the call.
    assert_eq!(state(&own, "read", "<result>"), Some(State::Borrowed));
}

/// **A result built from a buffer this body made escapes it**, which is D2's
/// *escaping = tether*: the value outlives the scope that owns its buffer.
///
/// This is the state that is **not built**. What stands in its place today is
/// the backend refusing the program on the Nikaia line, which is
/// [ADR-283](../../../docs/specification/adr/adr-283.md) D3's residual hard
/// error.
#[test]
fn a_result_built_from_a_local_buffer_tethers() {
    let own = ledger(
        "use std::fs\n\
         \n\
         struct Entry { pub name: ref String }\n\
         \n\
         pub fn load() -> Vec[Entry] throws {\n\
         \x20   let text = fs::read_to_string(\"/etc/hostname\", fs::Root::Anywhere)\n\
         \x20   return [Entry { name: text.trim() }]\n\
         }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    assert_eq!(state(&own, "load", "<result>"), Some(State::Tethered));
}

/// **A view of text that outlives the program borrows**, which is
/// [ADR-283](../../../docs/specification/adr/adr-283.md) D7's own example: it
/// borrows from nothing and is still the cheapest state.
#[test]
fn a_result_that_is_static_text_borrows() {
    let own = ledger(
        "pub fn label() -> ref String { return \"Ada\" }\n\
         \n\
         fn main() { println(label()) }\n",
    );
    assert_eq!(state(&own, "label", "<result>"), Some(State::Borrowed));
}

/// **A list of values is not a buffer.** Nothing views a `Vec[Entry]`; its
/// elements are values, so a body that builds one owns no buffer a returned
/// view could point into.
#[test]
fn a_body_that_owns_no_buffer_borrows() {
    let own = ledger(
        "struct Entry { pub name: ref String }\n\
         \n\
         pub fn made() -> Vec[Entry] {\n\
         \x20   let mut out = Vec()\n\
         \x20   out.push(Entry { name: \"x\" })\n\
         \x20   return out\n\
         }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    assert_eq!(state(&own, "made", "<result>"), Some(State::Borrowed));
}

/// **`.clone()` makes a buffer too**, by the name rather than by a ledger
/// entry: all four entries of either name hand back owned text, which is the
/// same kind of fact about the language below that `len` records.
#[test]
fn text_a_body_owns_is_a_buffer() {
    let own = ledger(
        "struct Entry { pub name: ref String }\n\
         \n\
         pub fn owned(seed: i64) -> Vec[Entry] {\n\
         \x20   let text = seed.to_string()\n\
         \x20   return [Entry { name: text.trim() }]\n\
         }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    assert_eq!(state(&own, "owned", "<result>"), Some(State::Tethered));
}

/// **A signature with no view in it gets no column at all**, which is most of
/// them — and an empty list would be a claim rather than a silence.
#[test]
fn a_signature_without_a_view_says_nothing() {
    let own =
        ledger("pub fn twice(n: i64) -> i64 { return n * 2 }\n\nfn main() { println(\"x\") }\n");
    assert!(own.functions["twice"].views.is_empty());
}

/// **A receiver carries what its type does**, so a method of a struct that
/// holds a view has `self` among its positions.
#[test]
fn a_receiver_that_carries_a_view_is_a_position() {
    let own = ledger(
        "struct Row { pub name: ref String }\n\
         \n\
         impl Row {\n\
         \x20   pub fn width(ref self) -> i64 { return self.name.len() }\n\
         }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    assert_eq!(state(&own, "Row::width", "self"), Some(State::Borrowed));
}

/// **The column renders and parses back**, which Part III 13.5 requires of
/// every column: the file is a pure function of the source, and a column that
/// only renders is one a consumer cannot read.
#[test]
fn the_column_renders_and_parses_back() {
    let own = ledger(
        "use std::fs\n\
         \n\
         struct Entry { pub name: ref String }\n\
         \n\
         pub fn load() -> Vec[Entry] throws {\n\
         \x20   let text = fs::read_to_string(\"/etc/hostname\", fs::Root::Anywhere)\n\
         \x20   return [Entry { name: text.trim() }]\n\
         }\n\
         \n\
         pub fn read(data: ref String) -> Vec[Entry] { return [Entry { name: data }] }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    let rendered = own.render();
    assert!(
        rendered.contains("views = [\"<result>: tethered\"]"),
        "{rendered}"
    );
    let back = Ledger::parse(&rendered).expect("the ledger parses back");
    assert_eq!(back.functions["load"].views, own.functions["load"].views);
    assert_eq!(back.functions["read"].views, own.functions["read"].views);
}

// ---------------------------------------------------------------------------
// A grammar's entry
// ---------------------------------------------------------------------------

/// A parse whose record holds a view, and one whose result is a number.
///
/// The two shapes the corpus splits into, written once for the four tests
/// below: `Stock::file` hands back `Vec[Entry]` and `Entry` holds a
/// `ref String`; `Calc::expr` hands back an `i64` and holds nothing.
const VIEWING: &str = "grammar Stock {\n\
                       \x20   rule FIELD -> ref String = s:until(\";\" | frame_end) { s }\n\
                       \x20   rule COUNT -> i64 = n:dec[i64](digit+) { n }\n\
                       \x20   @frame(boundary: \"\\n\")\n\
                       \x20   rule ENTRY -> Entry =\n\
                       \x20       category:FIELD \";\" => count:COUNT frame_end\n\
                       \x20       { Entry { category, count } }\n\
                       \x20   entry rule file -> Vec[Entry] = entries:ENTRY* { entries }\n\
                       }\n\
                       \n\
                       pub struct Entry { pub category: ref String, pub count: i64 }\n";

const COUNTING: &str = "grammar Calc {\n\
                        \x20   rule NUM -> i64 = n:dec[i64](digit+) { n }\n\
                        \x20   entry rule expr -> i64 = n:NUM { n }\n\
                        }\n";

/// **A parse hands back views into the text it was given**
/// ([ADR-296](../../../docs/specification/adr/adr-296.md) D24,
/// [ADR-186](../../../docs/specification/adr/adr-186.md) D1).
///
/// The buffer is the caller's `input`, which outlives the call — the same
/// sentence `of` reaches for a function with a view among its parameters, read
/// off the shape of a grammar instead of off a signature.
#[test]
fn a_grammar_entry_borrows_the_text_its_record_views() {
    let own = ledger(VIEWING);
    assert_eq!(state(&own, "Stock::file", "input"), Some(State::Borrowed));
    assert_eq!(
        state(&own, "Stock::file", "<result>"),
        Some(State::Borrowed)
    );
}

/// **And a parse that hands back a number holds nothing.** The column is a
/// fact about what the result can point into, so a rule whose result cannot
/// point anywhere writes no position at all.
#[test]
fn a_grammar_entry_that_hands_back_a_number_holds_nothing() {
    let own = ledger(COUNTING);
    assert!(
        own.functions["Calc::expr"].views.is_empty(),
        "an `i64` points into nothing"
    );
}

/// **The whole corpus is the free case**, which is
/// [ADR-283](../../../docs/specification/adr/adr-283.md)'s worked check read
/// off the analysis rather than asserted: *nothing is allocated per row and no
/// refcount traffic occurs in the parallel section.*
///
/// A ceiling rather than an equality, so the number can only be beaten — and if
/// it rises, a program in the tree has started needing the state that is not
/// built.
#[test]
fn nothing_in_the_corpus_needs_a_tether() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let mut pending = vec![root.join("examples"), root.join("benches")];
    let mut files = 0;
    let mut tethering: Vec<String> = Vec::new();
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "nika") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read the program");
            let Ok(parsed) = parse_to_ast(&source) else {
                continue;
            };
            files += 1;
            let own = Ledger::infer_package(&[&parsed], &library);
            for (key, contract) in &own.functions {
                for held in &contract.views {
                    if held.state == State::Tethered {
                        tethering.push(format!("{}: {key} {}", path.display(), held.position));
                    }
                }
            }
        }
    }
    assert!(files >= 18, "only {files} programs were read");
    assert!(
        tethering.is_empty(),
        "no program in the tree needs the state that is not built, and these do: {tethering:#?}"
    );
}

// --- The word above a struct ---

/// **`@borrowed` is gone** ([ADR-283](../../../docs/specification/adr/adr-283.md)
/// D4): no word says where a struct's views live.
///
/// Since [ADR-331](../../../docs/specification/adr/adr-331.md) an `@` line is
/// an attribute, a value of a struct `@meta::Attribute` marks, so the line
/// parses and is refused by what it names: no struct called `borrowed` exists
/// (`NK1135`), and nothing the refusal says offers the word back as one of the
/// language's.
#[test]
fn no_attribute_stands_above_a_struct() {
    let found = findings("@borrowed\nstruct Reading { name: ref String }\n");
    let codes: Vec<&str> = found.iter().map(|f| f.code).collect();
    assert_eq!(codes, vec!["NK1135"], "{found:#?}");
    let help = found[0].help.clone().unwrap_or_default();
    assert!(!help.contains("borrowed"), "{help}");
}

/// **No word is needed for a tether**
/// ([ADR-283](../../../docs/specification/adr/adr-283.md) D4, withdrawing
/// [ADR-283](../../../docs/specification/adr/adr-283.md) D4): where a buffer
/// lives is the compiler's decision, shown by `--tethers` and the ledger, and
/// `@tethers` is not a word of this language - an attribute naming nothing.
#[test]
fn no_word_is_written_for_a_tether() {
    let found = findings("@tethers\nstruct Token { text: ref String }\n");
    let codes: Vec<&str> = found.iter().map(|f| f.code).collect();
    assert_eq!(codes, vec!["NK1135"], "{found:#?}");
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("an `@` line parses (ADR-331)");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    nikaia::check::check(&parsed, &own, &library).findings
}

/// **A view of a buffer the body owns is tethered, not refused**, with no word
/// written anywhere (ADR-283 D10, D4).
#[test]
fn a_view_of_a_buffer_the_body_owns_is_tethered() {
    let source = "struct Token { text: ref String }\n\n\
                  fn first() -> ref String {\n\
                  \x20   let held: String = \"a b\"\n\
                  \x20   return held.trim()\n\
                  }\n";
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let found = nikaia::contracts::tether::check(&parsed, &own, &library);
    assert!(found.is_empty(), "{found:?}");
    assert!(
        own.functions["first"]
            .views
            .iter()
            .any(|h| h.state == State::Tethered)
    );
}
