// crates/nikaia/src/specbook.rs
//
// Every `nika` block in the specification, and how far this compiler gets with
// it.
//
// **Why this is a module and not a test.** The walk is needed twice, by
// `tests/specification.rs` and by `examples/specification.rs` that regenerates
// its baseline, and an example is a binary a test cannot call into — the same
// split `dump.rs` and `errors.rs` already have. Putting it here rather than
// copying it keeps one answer to what the specification contains.
//
// **What it is for.** A page can be wrong in a way nothing notices: a program
// printed as an example, never run, and stale from the day a decision changed
// under it. `docs/README.md` §1 already makes a stale **Status** note a defect
// in its own right; this is the same rule applied to the code beside it. Run by
// hand once, it turned up four things — Part I 4.7's `"User: " + self.username`
// (refused twice over), three plain strings holding holes that ADR-309 made
// text, Part I 4.5's map example not compiling, and a trait whose method pauses.
// Run in CI, it keeps them from coming back.
//
// **A block is not always a program**, and the readings below are why this is
// not simply "compile each one". A chapter shows a signature list, a body
// without its function, items without a `main`, or a sketch with `…` where code
// would be. Each reading is tried and the furthest one is what the block is
// recorded as, so a fragment is reported as a fragment rather than as a failure.

use crate::contracts::LedgerOps;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One fenced `nika` block, by where it is written.
#[derive(Debug, Clone)]
pub struct Block {
    pub file: String,
    /// The 1-based line of the block's first line of code.
    pub line: usize,
    pub code: String,
    /// The page is **discussing a refusal** here — it names a diagnostic, or
    /// says in a comment that a line is not allowed.
    ///
    /// Deliberately not called *"this block should be refused"*, which is what
    /// it was called first and is not what it detects: every page in this
    /// specification shows the offending line **commented out**, so the block
    /// itself compiles and should. Measured, by an assertion that said six such
    /// blocks were stale and was wrong about all six.
    pub discusses_a_refusal: bool,
    /// The page writes it as a **sketch**: an elision stands where code would.
    /// Nothing can compile one and nothing should try.
    pub sketch: bool,
}

/// How far the compiler got, worst to best.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// No reading of it parses.
    Fragment,
    /// It parses and the checker refuses it.
    Refused,
    /// It parses, checks and lowers.
    Lowered,
}

impl Stage {
    pub fn word(self) -> &'static str {
        match self {
            Stage::Fragment => "fragment",
            Stage::Refused => "refused",
            Stage::Lowered => "lowered",
        }
    }
}

/// What one block came to.
#[derive(Debug, Clone)]
pub struct Verdict {
    pub block: Block,
    pub stage: Stage,
    /// Which reading got furthest.
    pub reading: &'static str,
    /// The diagnostic codes the checker raised, where it raised any.
    pub codes: BTreeSet<String>,
}

pub fn specification_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/specification")
        .canonicalize()
        .expect("specification directory")
}

/// Every ```nika block in every page, in file and then line order.
pub fn blocks(dir: &Path) -> Vec<Block> {
    let mut pages: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read the specification directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    pages.sort();

    let mut found = Vec::new();
    for page in pages {
        let name = page
            .file_name()
            .and_then(|n| n.to_str())
            .expect("utf-8 file name")
            .to_string();
        let text = std::fs::read_to_string(&page).expect("read a page");
        // **The scan is Nikaia** (`tools/specbook.nika`, #125).
        for block in nikaia_std::tools::specbook::blocks_in(&text) {
            found.push(Block {
                file: name.clone(),
                line: block.line as usize,
                discusses_a_refusal: block.discusses_a_refusal,
                sketch: block.sketch,
                code: block.code,
            });
        }
    }
    found
}

/// The readings a block can have, best-case first is **not** the order: each is
/// tried and the furthest wins, because a block that parses as written may still
/// be a body that would check if it had a function around it.
pub fn readings(code: &str) -> Vec<(&'static str, String)> {
    nikaia_std::tools::specbook::readings(code)
        .into_iter()
        .map(|reading| {
            let name = match reading.name.as_str() {
                "as written" => "as written",
                "items and a main" => "items and a main",
                "a body" => "a body",
                _ => "items, then a body",
            };
            (name, reading.source)
        })
        .collect()
}

/// The one reading by the name [`verdicts`] recorded it under.
///
/// A caller that wants to go *further* than this module does — hand the lowering
/// to `rustc`, say — needs the source that got furthest, and re-deriving it by
/// guessing which reading won would be a second answer to a question already
/// answered.
pub fn reading(code: &str, name: &str) -> Option<String> {
    readings(code)
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, source)| source)
}

/// **Where a build of a loose file compiles its build-time code**: the user's
/// cache, beside the store.
pub fn workshop() -> PathBuf {
    orchestrator::cache::Layout::user_cache_dir().join("build-time")
}

/// Take every block as far as it goes, as a build reads it: a `comptime` that
/// calls something is compiled and run in `workshop`
/// ([ADR-321](../../docs/specification/adr/adr-321.md) D1).
pub fn verdicts(dir: &Path, workshop: &Path) -> Vec<Verdict> {
    let reads = crate::assets::Reads::at(dir).building_in(workshop);
    blocks(dir)
        .into_iter()
        .map(|block| {
            let mut best = Verdict {
                stage: Stage::Fragment,
                reading: "as written",
                codes: BTreeSet::new(),
                block: block.clone(),
            };
            for (reading, source) in readings(&block.code) {
                let (stage, codes) = reach(&source, &reads);
                if stage > best.stage {
                    best = Verdict {
                        stage,
                        reading,
                        codes,
                        block: block.clone(),
                    };
                }
                if stage == Stage::Lowered {
                    break;
                }
            }
            best
        })
        .collect()
}

/// One source, through the three stages.
fn reach(source: &str, reads: &crate::assets::Reads) -> (Stage, BTreeSet<String>) {
    let Ok(parsed) = crate::parser::parse_to_ast(source) else {
        return (Stage::Fragment, BTreeSet::new());
    };
    let place = reads.workshop().place();
    let own = crate::comptime_run::defaults_compiled_in(place, || {
        crate::contracts::Ledger::infer(&parsed)
    });
    let library = crate::contracts::std_ledger();
    let found = crate::check::check_against(
        &parsed,
        &[],
        &own,
        library,
        &BTreeSet::new(),
        &crate::check::Newly::new(),
        reads,
    );
    let codes: BTreeSet<String> = found.findings.iter().map(|f| f.code.to_string()).collect();
    // A warning is not a refusal, but it is worth recording: `NK1111` on a page
    // is a plain string holding what looks like a hole, which is exactly the
    // kind of staleness this walk exists to find.
    let refused = found
        .findings
        .iter()
        .any(|f| matches!(f.severity, crate::check::Severity::Error));
    if refused {
        return (Stage::Refused, codes);
    }
    match crate::emit::emit_program_reading(&parsed, crate::emit::Build::default(), reads) {
        Ok(_) => (Stage::Lowered, codes),
        Err(_) => (Stage::Refused, codes),
    }
}

/// The report, one line per block, in a form a diff can be read from.
///
/// **A block is addressed by its ordinal in its page and not by its line**, and
/// that is the difference between a baseline worth having and one nobody
/// believes. A line number moves whenever a paragraph above the block gains a
/// sentence, so every prose edit churned the whole tail of the file and the real
/// changes were somewhere in the noise — measured, on a run where twenty-one
/// lines moved and not one verdict did.
///
/// The ordinal moves only when a block is **added, removed or reordered**, which
/// is a change worth seeing. What replaces the line as the thing a reader
/// recognises is the block's own first line of code, which says more about which
/// block it is than a number ever did.
pub fn report(dir: &Path, workshop: &Path) -> String {
    let mut out = String::new();
    let mut nth: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for v in verdicts(dir, workshop) {
        let at = nth.entry(v.block.file.clone()).or_insert(0);
        *at += 1;
        let ordinal = *at;
        let mut notes: Vec<String> = Vec::new();
        if v.block.discusses_a_refusal {
            notes.push("about-a-refusal".to_string());
        }
        if v.block.sketch {
            notes.push("sketch".to_string());
        }
        if !v.codes.is_empty() {
            notes.push(v.codes.iter().cloned().collect::<Vec<_>>().join(","));
        }
        out.push_str(&format!(
            "{} #{ordinal} {} ({}){}  {}\n",
            v.block.file,
            v.stage.word(),
            v.reading,
            match notes.is_empty() {
                true => String::new(),
                false => format!(" [{}]", notes.join(" ")),
            },
            opening(&v.block.code),
        ));
    }
    out
}

/// The block's first line of code, as the thing a reader recognises it by.
///
/// Trimmed and cut, because this is an anchor and not the content: a long line
/// would put the interesting part of the report off the edge of a terminal.
fn opening(code: &str) -> String {
    nikaia_std::tools::specbook::opening(code)
}

#[cfg(test)]
mod tests {
    use nikaia_std::tools::specbook::{discusses_a_refusal, is_a_sketch};

    /// **A diagnostic's name is a mark** (#125): the text is lowered before
    /// it is asked, and the Rust asked it for `error[NK` in capitals, so the
    /// mark never matched and `// without throws: error[NK2701]` read as a
    /// page discussing nothing.
    #[test]
    fn a_named_diagnostic_is_a_refusal_discussed() {
        assert!(discusses_a_refusal(
            "fn tally() -> i64 throws {   // without `throws`: error[NK2701]"
        ));
        assert!(!discusses_a_refusal("fn tally() -> i64 throws {"));
    }

    #[test]
    fn a_spread_is_code_and_an_elision_is_a_sketch() {
        assert!(!is_a_sketch("pub fn execute(self; ...args: Self::dsl) {"));
        assert!(is_a_sketch(
            "fn process_image(path: String) -> Image { ... }"
        ));
        assert!(is_a_sketch("let kasse: SharedMut[i32] = ..."));
        assert!(is_a_sketch("Account(…)"));
    }
}
