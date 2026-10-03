// crates/nikaia/src/foreign.rs
//
// A crate is described before it is called - `NK2504`
// ([ADR-290](../../docs/specification/adr/adr-290.md) D1).
//
// ## What this is for
//
// A call into a Rust crate no ledger describes was **silent**. Every analysis
// this compiler has - what may cross a thread, what a call may reach, whether it
// pauses, whether it can fail - reads a contract at the boundary, and where
// there is none they all read the same thing: nothing. Fail-closed answers stop
// a few of the questions (`crosses_into_an_unseen_call` hands the crossing on
// rather than accepting it), and the rest simply do not happen.
//
// D1 removes the third answer. There is a described crate and a refused call,
// and the message names the command that turns the second into the first.
//
// ## Which way it errs
//
// **Only where the build itself declared the crate.** The set comes from
// `[dependencies]` with `type = "rust"` (`manifest::foreign_crates`), so a
// qualified name this compiler cannot account for - a module of the program, a
// Nikaia package, a `std` path, a typo - is not this refusal's business and
// keeps whatever message it already had. Refusing on a name nobody declared
// would be [Part III C.4](../../docs/specification/30-nikaia-tooling.md)'s
// correct program refused, which is the worse of the two mistakes.
//
// **And once per crate, not once per call.** The reader's next move is one
// command for the whole crate, so four calls into `regex` are one thing to do
// and one message to read. The caret is on the first call, which is where they
// will start.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Expr, Span};
use crate::check::{Finding, Severity};
use crate::parser::Parsed;
pub(crate) use nikaia_std::tools::foreign::Seen;

/// The crate word in front of a qualified name: `hyper_shim` of
/// `hyper_shim::serve_once`, and the whole of a name with no `::` in it.
pub fn head_of(name: &str) -> &str {
    name.split("::").next().unwrap_or(name)
}

/// **Every qualified name this unit writes**, with the span of the statement
/// or item it was written in.
///
/// The walk [`check`] runs, handed out rather than copied
/// ([ADR-290](../../docs/specification/adr/adr-290.md) D2): what the refusal
/// asks about is which crates a program reaches into, and what `nikaia
/// describe` asks is which *names* of one it reaches — the same walk, read one
/// segment further.
pub fn qualified_names(parsed: &Parsed) -> BTreeMap<String, Span> {
    let mut out: BTreeMap<String, Span> = BTreeMap::new();
    for (name, span) in written(parsed) {
        out.entry(name).or_insert(span);
    }
    out
}

/// Every call and every written type that reaches into a crate nothing
/// describes.
///
/// `declared` is what the manifest says this build links against, under the
/// name a program writes; `described` is the crates a ledger was found for.
/// Both empty - a loose file, with no project around it - is silence, which is
/// the right answer: nothing was declared, so nothing is undescribed.
pub fn check(
    parsed: &Parsed,
    declared: &BTreeSet<String>,
    described: &BTreeSet<String>,
    moved: &BTreeMap<String, Vec<String>>,
) -> Vec<Finding> {
    if declared.is_empty() {
        return Vec::new();
    }
    let mut first: BTreeMap<String, Span> = BTreeMap::new();
    let mut stale: BTreeMap<String, Span> = BTreeMap::new();
    for (name, span) in written(parsed) {
        let crate_name = head_of(&name);
        if !declared.contains(crate_name) {
            continue;
        }
        if !described.contains(crate_name) {
            first.entry(crate_name.to_string()).or_insert(span);
        } else if moved.contains_key(crate_name) {
            stale.entry(crate_name.to_string()).or_insert(span);
        }
    }
    first
        .into_iter()
        .map(|(crate_name, span)| undescribed(&crate_name, &span))
        .chain(stale.into_iter().map(|(crate_name, span)| {
            has_moved(&crate_name, moved.get(&crate_name).expect("found"), &span)
        }))
        .collect()
}

/// `NK2505`: the crate moved and its description did not
/// ([ADR-290](../../docs/specification/adr/adr-290.md) D5, on
/// [ADR-100](../../docs/specification/adr/adr-100.md) D3's rule).
///
/// **The same rule as a stale ledger's, with the one difference that matters.**
/// D3 says a ledger is believed while its hashes hold and **derived again**
/// where they do not; a description cannot be derived again, because what it
/// says is a reviewer's judgement — `crosses = false` read off a field, a
/// signature that lies corrected. So the second row of that table becomes a
/// refusal and the command, which is the same thing said to a person instead of
/// to a build.
///
/// **Once per crate**, like `NK2504` and for the same reason: the reader's next
/// move is one command for the whole crate.
fn has_moved(crate_name: &str, files: &[String], span: &Span) -> Finding {
    let which = match files.len() {
        1 => format!("`{}`", files[0]),
        _ => format!(
            "{} files, among them `{}`",
            files.len(),
            files.first().map(String::as_str).unwrap_or("?")
        ),
    };
    Finding {
        severity: Severity::Error,
        span: *span,
        code: "NK2505",
        message: format!("`{crate_name}` has changed since its description was reviewed."),
        notes: vec![
            format!(
                "`contracts/{crate_name}.contracts` describes an older version: {which} \
                 changed since."
            ),
            "A description is written by a person, so it has to be reviewed again, not \
             just regenerated."
                .to_string(),
        ],
        help: Some(format!(
            "Run `nikaia describe {crate_name}` and review the diff before committing it."
        )),
        labels: Vec::new(),
    }
}

/// `NK2504`, and the message is the command.
fn undescribed(crate_name: &str, span: &Span) -> Finding {
    Finding {
        severity: Severity::Error,
        span: *span,
        code: "NK2504",
        message: format!("`{crate_name}` has no description yet."),
        notes: vec![
            "Nikaia needs to know what a Rust crate's functions do: whether they pause, \
             can fail, reach locks, or move values between threads."
                .to_string(),
            "`nikaia describe` drafts a description from the crate's `pub` signatures into \
             `contracts/<crate>.contracts`, for you to review and commit like code."
                .to_string(),
        ],
        help: Some(format!("Run `nikaia describe {crate_name}`.")),
        labels: Vec::new(),
    }
}

/// **Every qualified name the unit writes, by the walk in Nikaia**
/// (`tools/foreign.nika`, #125), in the order it meets them. What the holes
/// of a literal are, and what a file's alias stands for, are this compiler's
/// to answer: a template's holes are parsed out of its text, and the aliases
/// are the parse's.
fn written(parsed: &Parsed) -> Vec<(String, Span)> {
    seen(parsed, &|_: &str| -1)
        .into_iter()
        .filter_map(|seen| match seen {
            Seen::Path { name, span } => Some((name, span)),
            _ => None,
        })
        .collect()
}

/// What the walk meets in one block, paths and calls, where nobody asks about
/// a `root`.
pub(crate) fn seen_in(parsed: &Parsed, block: &crate::ast::Block) -> Vec<Seen> {
    nikaia_std::tools::foreign::seen_in(
        block,
        &parsed.interner,
        &|name: &str| parsed.unaliased(name),
        &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
        &|_: &str| -1,
        false,
    )
}

/// **Every bare name a block mentions**, by the same walk, and whether it met
/// a `spawn` it did not read into: what a body keeps is asked of the names it
/// reaches (`contracts::keep`, `contracts::keeps`).
pub(crate) fn names_in_block(parsed: &Parsed, block: &crate::ast::Block) -> (Vec<String>, bool) {
    named(nikaia_std::tools::foreign::seen_in(
        block,
        &parsed.interner,
        &|name: &str| parsed.unaliased(name),
        &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
        &|_: &str| -1,
        true,
    ))
}

/// The same from one expression, and the blocks it holds.
pub(crate) fn names_in_expression(parsed: &Parsed, expr: &Expr) -> (Vec<String>, bool) {
    named(nikaia_std::tools::foreign::seen_in_expression(
        expr,
        &parsed.interner,
        &|name: &str| parsed.unaliased(name),
        &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
        &|_: &str| -1,
        true,
    ))
}

fn named(seen: Vec<Seen>) -> (Vec<String>, bool) {
    let mut names = Vec::new();
    let mut a_spawn = false;
    for one in seen {
        match one {
            Seen::Name(name) => names.push(name),
            Seen::Spawn { .. } => a_spawn = true,
            _ => {}
        }
    }
    (names, a_spawn)
}

/// What the walk meets, paths and calls: `root_at` says where a callee's
/// `root` parameter stands, for `--trust`, and `-1` where nobody asks.
pub(crate) fn seen(parsed: &Parsed, root_at: &impl Fn(&str) -> i64) -> Vec<Seen> {
    nikaia_std::tools::foreign::written_in(
        &parsed.program,
        &parsed.interner,
        &|name: &str| parsed.unaliased(name),
        &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
        root_at,
    )
}
