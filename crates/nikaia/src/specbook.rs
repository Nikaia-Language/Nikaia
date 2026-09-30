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
// (refused twice over), three plain strings holding holes that ADR-035 made
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
        let lines: Vec<&str> = text.split('\n').collect();
        let mut at = 0;
        while at < lines.len() {
            if lines[at].trim() != "```nika" {
                at += 1;
                continue;
            }
            let start = at + 1;
            let mut end = start;
            while end < lines.len() && lines[end].trim() != "```" {
                end += 1;
            }
            let code = lines[start..end].join("\n");
            found.push(Block {
                file: name.clone(),
                line: start + 1,
                discusses_a_refusal: discusses_a_refusal(&code),
                sketch: is_a_sketch(&code),
                code,
            });
            at = end + 1;
        }
    }
    found
}

/// Whether the page is talking about a refusal in this block.
///
/// Read off the block's own text rather than off a marker somebody has to
/// remember to add: a page that discusses a refusal names the code, says
/// *"Invalid"*, or says *"error"* — in either language, since Part I is written
/// in both.
fn discusses_a_refusal(code: &str) -> bool {
    const MARKS: &[&str] = &[
        "error[NK",
        "// invalid",
        "// ungültig",
        "// fehler",
        "// error",
        "// compiler error",
        "refused",
        "does not parse",
    ];
    let lower = code.to_lowercase();
    MARKS.iter().any(|m| lower.contains(m))
        || lower
            .split_whitespace()
            .any(|w| w.starts_with("nk1") || w.starts_with("nk2"))
}

/// Whether an elision stands where code would.
///
/// `...args` is not one: three dots against a name are the typed spread of
/// [ADR-007](../../../docs/specification/adr/adr-007.md) D5, which a driver
/// writes as code.
fn is_a_sketch(code: &str) -> bool {
    code.contains('…')
        || code.match_indices("...").any(|(at, _)| {
            !code[at + 3..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
        })
}

/// The readings a block can have, best-case first is **not** the order: each is
/// tried and the furthest wins, because a block that parses as written may still
/// be a body that would check if it had a function around it.
pub fn readings(code: &str) -> Vec<(&'static str, String)> {
    let mut all = vec![("as written", code.to_string())];
    if code.contains("fn main") {
        return all;
    }
    all.push((
        "items and a main",
        format!("{}\n\nfn main() {{\n}}\n", code.trim_end()),
    ));
    all.push(("a body", wrapped(code)));
    // **Items, then statements** — the shape a chapter uses most: a `use` or a
    // `struct` at the top and then the lines that exercise it. Neither reading
    // above can take it, and it is not a defect in the page.
    if let Some((head, tail)) = split_at_the_first_statement(code) {
        all.push((
            "items, then a body",
            format!("{head}\n\n{}", wrapped(&tail)),
        ));
    }
    all
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

fn wrapped(body: &str) -> String {
    let indented: Vec<String> = body
        .trim_end()
        .split('\n')
        .map(|l| match l.trim().is_empty() {
            true => String::new(),
            false => format!("    {l}"),
        })
        .collect();
    format!("fn main() {{\n{}\n}}\n", indented.join("\n"))
}

/// Split a block into its leading items and the statements after them.
///
/// Brace depth rather than a parser, because this runs on text that may not
/// parse at all — which is the case it exists for. A line at depth zero that
/// starts with an item keyword, a comment, or nothing extends the head; the
/// first line at depth zero that does not is where the body begins.
fn split_at_the_first_statement(code: &str) -> Option<(String, String)> {
    const ITEMS: &[&str] = &[
        "use ",
        "struct ",
        "enum ",
        "impl ",
        "trait ",
        "fn ",
        "grammar ",
        "const ",
        "comptime ",
        "pub ",
    ];
    let lines: Vec<&str> = code.trim_end().split('\n').collect();
    let mut head = 0;
    let mut depth: i32 = 0;
    for (at, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if depth == 0 {
            let carries_on = trimmed.is_empty()
                || trimmed.starts_with("//")
                || ITEMS.iter().any(|k| trimmed.starts_with(k));
            if !carries_on {
                break;
            }
            head = at + 1;
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
    }
    match head > 0 && head < lines.len() {
        true => Some((lines[..head].join("\n"), lines[head..].join("\n"))),
        false => None,
    }
}

/// Take every block as far as it goes.
pub fn verdicts(dir: &Path) -> Vec<Verdict> {
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
                let (stage, codes) = reach(&source);
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
fn reach(source: &str) -> (Stage, BTreeSet<String>) {
    let Ok(parsed) = crate::parser::parse_to_ast(source) else {
        return (Stage::Fragment, BTreeSet::new());
    };
    let own = crate::contracts::Ledger::infer(&parsed);
    let library = crate::contracts::std_ledger();
    let found = crate::check::check(&parsed, &own, &library);
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
    match crate::emit::emit_program(&parsed, crate::emit::Build::default()) {
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
pub fn report(dir: &Path) -> String {
    let mut out = String::new();
    let mut nth: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for v in verdicts(dir) {
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
    let line = code
        .split('\n')
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("(empty)");
    match line.chars().count() > 56 {
        true => format!("{}…", line.chars().take(55).collect::<String>()),
        false => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::is_a_sketch;

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
