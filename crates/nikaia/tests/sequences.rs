//! `Seq[T]` and `Par[T]`
//! ([ADR-105](../../../docs/specification/adr/adr-105.md)).
//!
//! The ledger's type language had no word for what `keys()`, `chars()` and
//! `io::lines()` hand back: elements of a type, produced one at a time when
//! asked for, with no length until the end and nothing laid out in memory. So
//! those entries said `-> ?`, and because a method is found through its
//! receiver's type, everything called on such a value was unfound too.
//!
//! **A container is not this.** A `Vec[T]` has its elements already, a length
//! and an index; it is walked *as* a `Seq` by a `for` and is not one.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use nikaia::contracts::ty::{self, Ty};
use nikaia::contracts::{Ledger, STD};
use nikaia::parser::parse_to_ast;

fn library() -> Ledger {
    Ledger::parse(STD).expect("std's shipped ledger parses")
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    nikaia::check::check(&parsed, &own, &library()).findings
}

/// **The two words parse, render and round-trip** (D1, D3).
///
/// `sync` and `throws` after the type say what one **step** may do, read as they
/// are after a function type (ADR-102 D2): without `sync` a step may pause,
/// without `throws` it cannot fail.
#[test]
fn the_words_round_trip() {
    for written in [
        "Seq[$K] sync",
        "Seq[String] throws",
        "Seq[char] sync throws",
        "Seq[$T]",
        "Par[$T] sync",
        "Seq[HashMap[$K, $V]] sync",
        "Seq[($K, $V)] sync",
    ] {
        let parsed = Ty::parse(written);
        assert!(
            matches!(parsed, Ty::Seq { .. }),
            "`{written}` is a sequence: {parsed:?}"
        );
        assert_eq!(parsed.text(), written, "it writes itself back");
    }

    // **The bracket is matched and not found at the end**, which is what
    // `Seq[HashMap[$K, $V]] sync` above is for: the tail begins after the
    // sequence's own `]`.
    let nested = Ty::parse("Seq[HashMap[$K, $V]] sync");
    let Ty::Seq { item, is_sync, .. } = &nested else {
        panic!("a sequence: {nested:?}");
    };
    assert!(is_sync);
    assert_eq!(item.text(), "HashMap[$K, $V]");
}

/// **A name that merely begins with `Seq` is not one**, and neither is a tail
/// nobody wrote.
///
/// The boundary matters for the same reason `word_off` has one: a type called
/// `Sequence` ends in nothing and a tail that is not the two words is a claim
/// the file did not make.
#[test]
fn only_the_two_words_may_follow() {
    for written in ["Sequence[$T]", "Seq", "Seq[$T] of stuff", "Seqx[$T]"] {
        assert!(
            !matches!(Ty::parse(written), Ty::Seq { .. }),
            "`{written}` is not a sequence"
        );
    }
}

/// **`Seq` binds through its item, and `Par` binds against a `Seq`** (D1, D3).
///
/// `Seq::collect(Seq[$T]) -> Vec[$T]` is what says a chain hands a `Vec` on, and
/// D3's *otherwise `Par[T]` has `Seq[T]`'s surface* needs the fallback entry —
/// written with a `Seq` receiver — to bind against the `Par` that reached it.
#[test]
fn a_sequence_binds_through_its_item() {
    let mut bound = std::collections::BTreeMap::new();
    ty::bind(
        &Ty::parse("Seq[$T]"),
        &Ty::parse("Seq[String] throws"),
        &mut bound,
    );
    assert_eq!(bound.get("T").map(|t| t.text()), Some("String".to_string()));

    // The two words are not compared: they are what a step may do, and an entry
    // writes the ones its own steps have rather than a demand on the receiver.
    let mut bound = std::collections::BTreeMap::new();
    ty::bind(
        &Ty::parse("Seq[$T]"),
        &Ty::parse("Par[i64] sync"),
        &mut bound,
    );
    assert_eq!(bound.get("T").map(|t| t.text()), Some("i64".to_string()));
}

/// **Neither word is in the surface type grammar** (D4).
///
/// A program writes `keys()`, `for` and `collect()`; the ledger does the naming.
/// `Seq` reaches `NK1135` as a type nothing declares, which is what says the
/// door is shut — and shut by the rule that shuts it for `Widgit`, not by a
/// special case.
#[test]
fn a_program_cannot_write_the_words() {
    for word in ["Seq", "Par"] {
        let found: Vec<_> = findings(&format!(
            "fn f(xs: {word}[i64]) -> i64 {{ return 1 }}\nfn main() {{ println(\"x\") }}"
        ))
        .into_iter()
        .filter(|f| f.code == "NK1135")
        .collect();
        assert_eq!(
            found.len(),
            1,
            "`{word}` is not a type a program has: {found:#?}"
        );
    }
}

/// **`keys()` hands back a sequence, and what is called on it resolves** (D1,
/// and §3's own chain).
#[test]
fn the_chain_off_keys_resolves() {
    let library = library();
    let keys = library
        .functions
        .get("collections::HashMap::keys")
        .expect("`collections::HashMap::keys` is described");
    let result = keys
        .signature
        .as_ref()
        .and_then(|s| s.result.clone())
        .expect("a result");
    // `sized` since ADR-212 D1: a map knows how many keys it holds. And views
    // since ADR-231 D3: a key points into the map.
    assert_eq!(result.text(), "Seq[ref $K] sync sized");

    // And the consumer entries are found under the receiver's own word.
    for method in ["collect", "count", "nth", "join", "map", "filter"] {
        assert!(
            library.functions.contains_key(&format!("Seq::{method}")),
            "`Seq::{method}` is described"
        );
    }
}

/// **`io::lines()` is a `Seq[String] throws`, which says two things in one
/// place** (D1, and §3's *`throws` on a `Seq` is the one spelling*).
///
/// It said `-> Lines`, a named type whose `iterates` column carried the failing
/// step. That worked for the `for` and for nothing else: the binding had no type,
/// so `line.len()` inside the loop was a method on `?`. Six of
/// `examples/tally.nika`'s `.len()` calls were among the 35 §1 counts.
#[test]
fn a_loop_over_standard_input_binds_a_string_and_still_costs_throws() {
    let program = "use std::io\n\
                   \n\
                   fn longest() -> i64 throws {\n\
                   \x20   let mut best = 0\n\
                   \x20   for line in io::lines() {\n\
                   \x20       if line.len() > best { best = line.len() }\n\
                   \x20   }\n\
                   \x20   return best\n\
                   }\n\
                   fn main() throws { println(f\"{longest()}\") }";
    let found = findings(program);
    assert!(
        found.is_empty(),
        "the binding has a type and `len` is a method of it: {found:#?}"
    );

    // **And the step still costs the function its `throws`** — the half the
    // `iterates` column used to carry alone. Without the word on the function
    // this is `NK2701`.
    let without = program.replace("fn longest() -> i64 throws {", "fn longest() -> i64 {");
    assert!(
        findings(&without).iter().any(|f| f.code == "NK2701"),
        "a step of `io::lines()` can fail, and the function has to say so"
    );
}

/// **The measurement, kept** ([ADR-105](../../../docs/specification/adr/adr-105.md)
/// §1 and [ADR-009](../../../docs/specification/adr/adr-009.md) D4).
///
/// §1 counts *35 unanswered method calls in the corpus*. The number this
/// harness reads on the same tree was **38** before these entries and is **33**
/// after — and the difference matters less than what the trace behind it said:
/// §1's *every one of the 35 downstream of a `?` that is a sequence* is not
/// what the corpus shows. Five were. The rest are downstream of a receiver with
/// no type for other reasons — `tail.drain()` in `json.nika`, `map` on the result
/// of a `catch` in `access-log.nika` — and they are somebody else's entry.
///
/// A ceiling rather than a floor, so that the number can only be beaten: what
/// this guards is that a later change does not quietly make the corpus less
/// answerable.
///
/// **It rose to 39, and the six are not new.** A call in a **grammar action**
/// used to land in no entry at all, because the checker filed its answers under
/// the enclosing *function* and an action has none — so six calls that were
/// always unanswered were also uncounted. A `pub` rule is a ledger entry
/// ([ADR-082](../../../docs/specification/adr/adr-082.md) D1), and it has its
/// own key now ([ADR-186](../../../docs/specification/adr/adr-186.md)), which
/// is what made them visible.
/// What a pattern binds has no type in this compiler — that is the parser
/// backend's — so a method call on one is unanswerable by construction, and the
/// entries derived from such an action say *undecided* rather than nothing.
///
/// **It rose to 50, and `examples/rust-signatures.nika` is all of it** — the
/// first Nikaia program in this tree that parses another language
/// ([ADR-196](../../../docs/specification/adr/adr-196.md)), and a grammar with
/// twelve actions that call a method on what a pattern bound. Ten of the twelve
/// are the sentence above, unchanged: `s.trim()` on a `text(…)` binding,
/// `tail.drain()` on a repetition's.
///
/// **The other two are a finding, and they are not unanswerable by
/// construction.** They are `f.parts.drain()` and `r.parts.drain()` in `show`,
/// where `f` and `r` are bound by a `match` arm — `Item::Fun(f)` — and the
/// variant's payload type is written down in the program. [`Checker`]'s
/// `pattern_bindings` gives every name a pattern binds `Ty::Unknown`, so the
/// enum the `match` is over is not consulted even where it answers plainly.
/// Typing those would take this number **down**, which is the direction this
/// ceiling is for; it is not done here because giving a binding a type where it
/// had none can turn a program that compiles into one that is refused, and
/// [Part III C.4](../../../docs/specification/30-nikaia-tooling.md) makes that
/// its own piece of work with its own sweep.
///
/// **51 at 0.0.162**, and the one that arrived is the sentence above again: the
/// grammar's `where_clause` rule keeps its text now
/// ([ADR-193](../../../docs/specification/adr/adr-193.md) D4 needs the bounds),
/// and `w.trim()` on what a pattern bound is a method call on an untyped
/// receiver like the ten before it.
///
/// **62 at 0.0.166**, and the eleven have **one** cause with one exception.
/// `examples/http/`'s server arrived, and **a binding whose value came through a
/// `catch` has no type in this compiler** — the same gap [Part I
/// 6.5](../../../docs/specification/10-nikaia-light.md)'s status block already
/// names for the `&` it cannot write. Four of the five `let`s in `answer` are
/// `… catch { … }`, so `more.len()`, `asked.method()`, `asked.path()`,
/// `asked.length()` and `asked.size()` are ten calls on receivers whose types the
/// program states plainly and this checker does not read. Measured piece by
/// piece: `let b = http1::Buffer()` answers every call on it, and the same
/// `Buffer` reached through a `catch` answers none.
///
/// **The eleventh is the sweep's own shape and not a gap.** It is
/// `request.body.len()` in `examples/hello-http/`, a method on a field of a
/// **lambda parameter** — and a lambda parameter *is* typed from the function
/// type it fits, measured: the same shape with the `struct` declared in the same
/// file answers. What this one cannot see is `http::Request`, because the sweep
/// reads one file at a time and that type is the package next door's.
///
/// **63 at 0.0.175**, and the twelfth is the eleventh again, one accessor over:
/// `request.head.query("name")` in the same handler, reaching
/// [ADR-018](../../../docs/specification/adr/adr-018.md) D4's query string
/// through the head the request holds. Same lambda parameter, same package next
/// door, same one-file sweep.
///
/// **64 at 0.0.177**, and the thirteenth is the eleventh becoming **two calls**.
/// `request.body` is an accessor now rather than a field
/// ([ADR-202](../../../docs/specification/adr/adr-202.md) D1 gave a method the
/// spelling to hand a view of a field back), so `request.body().len()` is a call
/// on the untyped lambda parameter *and* a call on what it hands back, where the
/// field form was one. Measured both ways round on this tree: 63 with the field,
/// 64 with the accessor. So the rise is a line the corpus writes differently and
/// not a receiver that stopped being typed — which is the one thing this ceiling
/// exists to tell apart, and the reason it is raised with a sentence.
#[test]
fn the_corpus_has_no_more_unanswered_method_calls_than_it_had() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let library = library();
    let mut files = 0usize;
    let mut unanswered = 0usize;
    let mut pending = vec![root.join("examples"), root.join("crates/nikaia-std/src")];
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
            let own = Ledger::infer(&parsed);
            let checked = nikaia::check::check(&parsed, &own, &library);
            files += 1;
            unanswered += checked
                .methods
                .values()
                .map(|c| c.unanswered)
                .sum::<usize>();
        }
    }
    assert!(files >= 18, "only {files} programs were read");
    assert!(
        unanswered <= 64,
        "{unanswered} unanswered method calls in {files} programs, and 64 is the \
         ceiling this was last measured at - a rise means a receiver stopped \
         being typed, and a fall means this number goes down with a sentence \
         saying what answered them"
    );
}

/// **No loose program in the repository is refused by any of this**, which is
/// Part III C.4's property for a change to the ledger's type language.
///
/// *Loose*, because this harness hands each file over alone and a **package** is
/// several files that see one another with no `use` (Part I 9.1):
/// `examples/inventory/page.nika` builds an `Entry` its sibling declares, so
/// `NK1135` there is the harness's limitation and not a refusal of the program.
/// The files `nikaia --input` sweeps are the ones checkable this way, which is
/// the top level of `examples/` and `std`'s own sources.
#[test]
fn no_loose_program_in_the_repository_is_refused() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let library = library();
    let mut reported = String::new();
    let mut checked = 0usize;
    for path in every_program(&root) {
        let source = std::fs::read_to_string(&path).expect("read the program");
        let Ok(parsed) = parse_to_ast(&source) else {
            continue;
        };
        let own = Ledger::infer(&parsed);
        let name = path.display().to_string();
        // `fortunes.nika` is the one program that is meant not to build: it
        // reaches for an `http` package the project does not declare, which is
        // ADR-122's step 4 and nothing to do with this.
        if name.contains("fortunes.nika") {
            continue;
        }
        for finding in nikaia::check::check(&parsed, &own, &library)
            .findings
            .iter()
        {
            reported.push_str(&nikaia::diagnostics::render_finding(
                finding, &name, &source,
            ));
        }
        checked += 1;
    }
    assert!(checked >= 11, "only {checked} programs were checked");
    assert!(reported.is_empty(), "a program was refused:\n{reported}");
}

/// The files a `nikaia --input` sweep covers: one program each, no siblings.
fn every_program(root: &Path) -> Vec<PathBuf> {
    let mut out = BTreeSet::new();
    for dir in [root.join("examples"), root.join("crates/nikaia-std/src")] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "nika") {
                out.insert(path);
            }
        }
    }
    out.into_iter().collect()
}

// --- D2: a sequence is walked once -------------------------------------------

/// A second walk of a named sequence is refused (`NK2702`).
fn walked_twice(source: &str) -> Vec<nikaia::check::Finding> {
    findings(source)
        .into_iter()
        .filter(|f| f.code == "NK2702")
        .collect()
}

/// **A `for` walks it, and a second `for` is refused** (D2).
#[test]
fn a_second_for_over_a_sequence_is_refused() {
    let found = walked_twice(
        "use std::io\n\nfn twice() -> i64 throws {\n\
         \x20   let lines = io::lines()\n\
         \x20   let mut n = 0\n\
         \x20   for line in lines { n += 1 }\n\
         \x20   for line in lines { n += 1 }\n\
         \x20   return n\n\
         }\n\
         fn main() throws { println(f\"{twice()}\") }",
    );
    assert_eq!(found.len(), 1, "one refusal: {found:#?}");
    let notes = found[0].notes.join(" ");
    assert!(notes.contains("walking it consumes it"), "{notes}");
    assert!(
        notes.contains("a `Vec` is not this"),
        "and the pair that says what a container does: {notes}"
    );
    assert!(
        found[0]
            .help
            .as_deref()
            .expect("a way out")
            .contains("collect it first"),
        "{:?}",
        found[0].help
    );

    // One walk is a correct program.
    assert!(
        walked_twice(
            "use std::io\n\nfn once() -> i64 throws {\n\
         \x20   let lines = io::lines()\n\
         \x20   let mut n = 0\n\
         \x20   for line in lines { n += 1 }\n\
         \x20   return n\n\
         }\n\
         fn main() throws { println(f\"{once()}\") }",
        )
        .is_empty()
    );
}

/// **A method that takes it by value walks it too** (D2).
///
/// Read off the signature and not off a list of names: every `Seq` entry writes
/// its receiver `(Seq[$T], …)` and a container's writes `(&Vec[$T], …)`, so the
/// file that describes the method is what says whether the walk keeps it.
#[test]
fn a_walking_method_consumes_the_sequence() {
    let found = walked_twice(
        "use std::io\n\nfn both() -> i64 throws {\n\
         \x20   let lines = io::lines()\n\
         \x20   let held = lines.collect()\n\
         \x20   let n = lines.count()\n\
         \x20   return n\n\
         }\n\
         fn main() throws { println(f\"{both()}\") }",
    );
    assert_eq!(found.len(), 1, "one refusal: {found:#?}");
}

/// **A container is walked as often as one likes** (D2), which is the half that
/// matters: a refusal that reached a `Vec` would refuse most programs there are.
#[test]
fn a_container_is_not_consumed_by_walking_it() {
    assert!(
        walked_twice(
            "fn twice(xs: Vec[i64]) -> i64 {\n\
         \x20   let mut n = 0\n\
         \x20   for x in xs { n += x }\n\
         \x20   for x in xs { n += x }\n\
         \x20   return n\n\
         }\n\
         fn main() { println(\"ok\") }",
        )
        .is_empty()
    );
}

/// **An assignment revives the name**, which is `NK2101`'s own rule one word
/// over: giving the name a value again is a correct program.
#[test]
fn a_name_given_another_sequence_may_be_walked_again() {
    assert!(
        walked_twice(
            "use std::io\n\nfn revived() -> i64 throws {\n\
         \x20   let mut lines = io::lines()\n\
         \x20   let mut n = 0\n\
         \x20   for line in lines { n += 1 }\n\
         \x20   lines = io::lines()\n\
         \x20   for line in lines { n += 1 }\n\
         \x20   return n\n\
         }\n\
         fn main() throws { println(f\"{revived()}\") }",
        )
        .is_empty()
    );
}

/// **A temporary has no second use to refuse** (D2, and `NK2101`'s narrowing).
///
/// `map.keys().collect()` walks a sequence nobody named, so there is nothing to
/// say about it — and a refusal keyed on the type rather than on a name would
/// have had to invent one.
#[test]
fn a_walk_of_a_temporary_says_nothing() {
    assert!(
        walked_twice(
            "use std::collections\n\nfn f() -> i64 {\n\
         \x20   let counts: collections::HashMap[ref String, i64] = collections::HashMap()\n\
         \x20   let names = counts.keys().collect()\n\
         \x20   return names.len()\n\
         }\n\
         fn main() { println(f\"{f()}\") }",
        )
        .is_empty()
    );
}

/// **Two sequences fit when their items do, and `Par` fits `Seq`** (D1, D3).
///
/// The fit had no arm for the new type at first, which said that a
/// `Seq[String] throws` did not fit a `Seq[String] throws` — found by the
/// assignment above, whose message read *this is X, and what it is assigned to
/// is X*.
#[test]
fn a_sequence_fits_a_sequence() {
    let seq = Ty::parse("Seq[String] throws");
    assert!(seq.fits(&seq), "a type fits itself");
    assert!(
        Ty::parse("Seq[String] sync").fits(&Ty::parse("Seq[String]")),
        "steps that never pause go where pausing is allowed"
    );
    assert!(
        !Ty::parse("Seq[String]").fits(&Ty::parse("Seq[String] sync")),
        "and not the other way round"
    );
    assert!(
        Ty::parse("Par[i64] sync").fits(&Ty::parse("Seq[i64] sync")),
        "a `Par`'s surface is a `Seq`'s (D3)"
    );
    assert!(
        !Ty::parse("Seq[i64] sync").fits(&Ty::parse("Par[i64] sync")),
        "and a `Seq` is not promised to run at once"
    );
}
