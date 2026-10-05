//! Every `nika` block in the specification, taken as far as it goes.
//!
//! **A page can be wrong in a way nothing notices.** A program printed as an
//! example, never run, stale from the day a decision changed under it.
//! `docs/README.md` §1 already makes a stale **Status** note a defect in its own
//! right; this is the same rule applied to the code beside it.
//!
//! It exists because running the blocks by hand once turned up four things, and
//! two of them were on the page rather than in the compiler: Part I 4.7's
//! `return "User: " + self.username` was refused twice over — the checker said
//! it hands back a `&str` where `String` is declared, and the lowering would not
//! have compiled either — and three plain strings in Part I 7 held `{p}`,
//! `{line}` and `{expected}`, written before
//! [ADR-309](../../../docs/specification/adr/adr-309.md) made only `f"…"`
//! interpolate. Both are fixed; this is what stops them coming back.
//!
//! **The baseline records fragments and refusals too**, and that is deliberate.
//! A chapter shows signature lists, bodies without their function, and sketches
//! with `…` where code would be, so "every block compiles" is not the property —
//! *nothing changed without somebody reading the diff* is. A block that stops
//! lowering shows up as a changed line, and so does one that starts.

mod common;

use nikaia::specbook::{self, Stage};
use std::path::{Path, PathBuf};

fn baseline() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/specification/EXPECTED.txt")
        .canonicalize()
        .expect("the baseline")
}

#[test]
fn the_specifications_programs_are_the_ones_in_expected_txt() {
    let expected = std::fs::read_to_string(baseline()).expect("EXPECTED.txt");
    assert_eq!(
        specbook::report(&specbook::specification_dir(), &common::workshop()),
        expected,
        "the specification and tests/specification/EXPECTED.txt have drifted \
         apart. If that is the change you meant, regenerate it with `cargo run \
         -p nikaia --example specification > tests/specification/EXPECTED.txt` \
         and read the diff - a block that stopped lowering is a defect in the \
         page or in the compiler, and a block that started is progress worth \
         seeing"
    );
}

/// **No page holds a plain string with a hole in it**, except where the page is
/// about exactly that.
///
/// `NK1111` is the standing, narrow warning for a plain string that looks as
/// though it meant a hole, such as `"{name}"`
/// ([ADR-309](../../../docs/specification/adr/adr-309.md) D15), and a page that
/// trips it is a page written before that decision. Part I 2.5 trips it on
/// purpose — it is the section that explains the difference — so the allowance
/// is that one block, named by where it is rather than by a marker somebody has
/// to remember.
#[test]
fn only_the_section_about_interpolation_writes_a_plain_string_with_a_hole() {
    let tripped: Vec<_> = specbook::verdicts(&specbook::specification_dir(), &common::workshop())
        .into_iter()
        .filter(|v| v.codes.contains("NK1111"))
        .collect();
    assert_eq!(
        tripped.len(),
        1,
        "a plain string holding a hole is a page written before ADR-309 D15; the \
         one allowed is Part I 2.5, which is about the difference: {:?}",
        tripped
            .iter()
            .map(|v| format!("{} line {}", v.block.file, v.block.line))
            .collect::<Vec<_>>()
    );
    // Recognised by what the block **says** rather than by where it is: a line
    // number moves with every paragraph above it, which is the churn the report
    // above was rebuilt to stop having.
    assert!(
        tripped[0].block.code.contains("the braces are braces"),
        "and the one allowed is the block that explains the difference, not \
         whichever one happens to trip it: {}",
        tripped[0].block.code
    );
}

/// **The three pages carry the version the CHANGELOG's newest heading does.**
///
/// They said **0.0.7** while it said 0.0.31 — twenty-four packages of drift,
/// found by a reader and not by anything here. That is
/// [`docs/README.md`](../../../docs/README.md) §1's own class one level up: a
/// stale **Status** note is a defect because a reader cannot tell a plan from a
/// promise, and a stale version number is the same mistake about the whole
/// document.
///
/// **The CHANGELOG is the source**, because that is where the rule already
/// lives — *every change package raises the patch number by one*, and its own
/// head says *the version is the specification's*. A second place to maintain
/// it by hand is how the first one went stale.
#[test]
fn the_specifications_version_is_the_changelogs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let changelog = std::fs::read_to_string(root.join("CHANGELOG.md")).expect("the CHANGELOG");
    let heading = changelog
        .lines()
        .find(|line| line.starts_with("## ["))
        .expect("a version heading");
    let (version, date) = heading
        .trim_start_matches("## [")
        .split_once("] — ")
        .expect("`## [x.y.z] — date`");

    for page in [
        "10-nikaia-light.md",
        "20-nikaia-advance.md",
        "30-nikaia-tooling.md",
    ] {
        let text = std::fs::read_to_string(root.join("docs/specification").join(page))
            .unwrap_or_else(|_| panic!("{page}"));
        assert!(
            text.contains(&format!("**Version:** {version} (Draft)")),
            "{page} does not carry version {version}, which is the CHANGELOG's newest \
             heading - raise it there and here in the same change"
        );
        assert!(
            text.contains(&format!("**Date:** {date}")),
            "{page} does not carry the date {date} of that heading"
        );
    }
}

/// **How much of the specification is a program this compiler takes**, as a
/// floor rather than as a number to admire.
///
/// It is the page's own progress measure and the one figure the baseline above
/// does not state in one place: a chapter shows signature lists, bodies without
/// their function, and sketches with an elision where code would be, so "all of
/// them" is not the target and never will be. What this guards against is the
/// direction that matters — a change that quietly turns programs back into
/// fragments, which the baseline would also show and which a count says out
/// loud.
///
/// Raise it when it is beaten. That is the point of a floor.
///
/// **Lowered once, from 52 to 48**, and the reason is written here because a
/// floor that moves down needs one. `NK1135`
/// ([ADR-096](../../../docs/specification/adr/adr-096.md)) refuses a type
/// nothing declares, and four blocks that used to lower name one. They are
/// three different things and only the first is the sweep's own doing:
///
/// * **`impl User` and `-> Config`** declare their type in a *neighbouring*
///   block, and this sweep compiles each block separately.
/// * **`NotFound(Path)`** and, in a block already refused for another reason,
///   `-> List[Row]`: `Path` and `List` are names the specification writes as
///   types and `std` does not publish. `fs::map`'s parameter is `?` because
///   Rust's is `impl AsRef<Path>`, and the language's sequence is `Vec`, which
///   is what `ListExt` is implemented for.
/// * **`fn total(p: &Player)`** names a type the specification declares
///   **nowhere at all** — as do `Object` and `SqlParser`.
///
/// So the direction this guards against is not the direction it moved: these
/// are not programs quietly turned back into fragments, they are fragments the
/// compiler had been accepting because it could not read a type name. A count
/// cannot tell those apart, which is why the number is a floor and the sentence
/// beside it is the actual guard.
///
/// **Lowered a second time, from 50 to 49.** Part II 10.1's `grammar Json`
/// writes a rule's action as the block after the pattern, with no second
/// arrow ([ADR-296](../../../docs/specification/adr/adr-296.md) D33), and the
/// parser does not take that form yet — so the page is ahead of the compiler
/// by decision, which is the state every unbuilt record leaves its examples
/// in. The block comes back as a program with ADR-296's first step, and
/// the floor goes back up with it.
///
/// **Lowered a third time, from 49 to 48**, for the same reason — and **raised
/// back** once the parser caught up. Part III 15.1's
/// `script.exec(msg: message)` writes a call whose arguments are all options
/// without the leading `;`
/// ([ADR-133](../../../docs/specification/adr/adr-133.md) D1); the block is a
/// program again, which is the *one* thing a floor is for. Its call half waited
/// on [ADR-140](../../../docs/specification/adr/adr-140.md) D1, because
/// `execute(target_age: 30)` and the named struct literal `Stats(min: first)`
/// were the same five tokens.
///
/// **Lowered a fourth time, from 48 to 47, and this one is the floor going
/// down because the compiler got better.** Part I 9.1's second block writes
/// `Row(id: helper() + 1)` for a `Row` its *first* block declares — the
/// sentence on that page is that files in a package see one another with no
/// `use` — and this harness hands each block over alone. It used to lower and
/// come back from `rustc` as `E0422`, *cannot find struct `Row`*, about a file
/// nobody wrote; it is now `NK1135` in this compiler's own words
/// ([ADR-096](../../../docs/specification/adr/adr-096.md), and the silence that
/// let [ADR-133](../../../docs/specification/adr/adr-133.md)'s collision
/// through). Blocks 28, 62 and 64 were already recorded that way for the same
/// reason. **A block that stops lowering because it is refused *here* is the
/// C.1 class closing**, which is why this number went down and nothing is
/// wrong: what it counts is programs this compiler hands to the backend, and
/// one fewer wrong answer arrives from there.
///
/// **Raised, from 47 to 48**, by ADR-133's call half — Part III 15.1's block,
/// above. That is what the floor is for, and the sentence is here because the
/// four before it are.
///
/// **Raised again, 48 to 49**, by [ADR-140](../../../docs/specification/adr/adr-140.md)
/// D3. Part II 10.2's `fn parse_input` entered its grammar through a **dot**,
/// which made `Json` a name nothing declares (`NK1117`); `Json::value(input)`
/// is a path and resolves. The block beside it, 10.3's `describe[T: Struct]`,
/// moved the other way and is the reason the number is 49 and not 50: with
/// `T::fields` a path rather than a dotted receiver, the `NK1117` that used to
/// refuse it was gone and the block reached `rustc`, which answered *cannot
/// find trait `Struct`* about a file nobody wrote. It is `NK1135` now — the
/// same claim one position over, for a **bound** naming a trait nothing
/// declares — so the block is refused in this compiler's words rather than
/// counted as a program.
#[test]
fn most_of_a_third_of_the_specifications_blocks_are_programs() {
    let verdicts = specbook::verdicts(&specbook::specification_dir(), &common::workshop());
    let lowered = verdicts
        .iter()
        .filter(|v| v.stage == Stage::Lowered)
        .count();
    assert!(
        lowered >= 60,
        "{lowered} of {} blocks lower, and 60 did when this floor was last set - \
         raise it if it is beaten, and read the diff if it is not",
        verdicts.len()
    );
}

/// **And the ones that lower are handed to `rustc`.**
///
/// This is the half that found the defects. A block can pass every stage of this
/// compiler and be rejected by the language below, which
/// [Part III C.1](../../../docs/specification/30-nikaia-tooling.md) calls a bug
/// here rather than there — and two of the specification's own programs were
/// exactly that: Part I 4.5's three-line map example (`E0282`, in a message
/// naming `TrustedMap`, `BuildHasherDefault<FxHasher>` and a type parameter `K`,
/// none of which the program wrote) and every string concatenation except
/// `String + &str` (`E0369`). Both are in issue #144.
///
/// **Most of the failures here are not defects and the baseline says which.** A
/// chapter's block names `User`, `Config`, `postgres` or `http` and leaves them
/// to the chapter around it, so `rustc` cannot resolve them and neither could
/// anything else; `NK1117` deliberately does not refuse a name this compiler
/// cannot see ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)).
/// What the baseline is for is the same thing the one above is for: a block that
/// stops compiling is a line in a diff.
#[test]
fn what_lowers_is_handed_to_rustc_and_the_verdicts_are_the_recorded_ones() {
    let dir = common::scratch_dir("specification-compiles");
    let mut report = String::new();
    // What each refused block was refused **with**, for the failure message
    // rather than for the comparison: see the verdict below.
    let mut detail = String::new();
    let mut nth: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for v in specbook::verdicts(&specbook::specification_dir(), &common::workshop()) {
        // Counted over **every** block and not only the ones that lower, so the
        // ordinal means the same thing here as in the other baseline.
        let at = nth.entry(v.block.file.clone()).or_insert(0);
        *at += 1;
        let ordinal = *at;
        if v.stage != Stage::Lowered {
            continue;
        }
        let source = specbook::reading(&v.block.code, v.reading)
            .expect("the reading that got furthest is one of the readings");
        let parsed = nikaia::parser::parse_to_ast(&source).expect("it parsed once already");
        let rust = nikaia::emit::emit_program_reading(
            &parsed,
            nikaia::emit::Build::default(),
            &common::reads(),
        )
        .expect("it lowered once already")
        .rust;
        let file = dir.join("block.rs");
        std::fs::write(&file, &rust).expect("write the Rust");
        let out = common::compile(
            &file,
            &[
                "--crate-type",
                "lib",
                "--emit=metadata",
                "-o",
                dir.join("block.rmeta").to_str().expect("utf-8 path"),
                "-A",
                "warnings",
            ],
        );
        // **Whether it compiles, and not what `rustc` called the failure.**
        //
        // The codes were in this baseline once and the baseline disagreed with
        // itself between two toolchains twice over: which of a block's errors
        // is printed **first** is the printer's business, and so is **which**
        // errors it finds at all — Part I's `#61` earns an `E0282` under one
        // stable and not under the next, because a later inference proves what
        // an earlier one asked to be annotated.
        //
        // That is not a flake to work around. `rust-toolchain.toml` says
        // `channel = "stable"` and [ADR-001](../../../docs/specification/adr/adr-001.md)
        // D1 makes that the one source of truth, so a diagnostic's identity is
        // a thing this repository has **decided** to let move. A baseline that
        // records it fails for a change nobody made, which is the one way a
        // baseline can stop being read.
        //
        // What does not move is the fact this file is for: a block the language
        // below **rejects** is [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s
        // class of defect, and a block that stops compiling is a line in a
        // diff. The codes are still printed — under the assertion, where a
        // reader who is looking at a diff can see what changed — and are not
        // what it compares.
        let refused = !out.status.success();
        let verdict = match refused {
            false => "compiles",
            true => "refused",
        };
        if refused {
            let stderr = String::from_utf8_lossy(&out.stderr);
            // `error[E0425]` and not `error: aborting due to 3 previous
            // errors`: the second is a count of the first and says nothing a
            // reader could act on.
            let mut codes: Vec<String> = stderr
                .lines()
                .filter(|l| l.starts_with("error["))
                .map(|l| l.split(':').next().unwrap_or(l).to_string())
                .collect();
            codes.sort();
            codes.dedup();
            detail.push_str(&format!(
                "{} #{ordinal}: {}\n",
                v.block.file,
                codes.join(" ")
            ));
        }
        report.push_str(&format!("{} #{ordinal} {verdict}\n", v.block.file));
    }
    let _ = std::fs::remove_dir_all(&dir);

    let baseline = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/specification/COMPILES.txt")
        .canonicalize()
        .expect("the compile baseline");
    let expected = std::fs::read_to_string(baseline).expect("COMPILES.txt");
    assert_eq!(
        report, expected,
        "what the language below says about the specification's programs has \
         changed. Regenerate with `cargo run -p nikaia --example specification \
         --features regenerate` is not a thing - this baseline is written by \
         this test, so copy the left-hand side in and read the diff: a block \
         that stopped compiling is Part III C.1's class.\n\n\
         What each refused block was refused with, on this toolchain - not \
         compared, because it moves (see the verdict above):\n{detail}"
    );
}
