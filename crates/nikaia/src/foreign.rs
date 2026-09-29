// crates/nikaia/src/foreign.rs
//
// A crate is described before it is called - `NK2504`
// ([ADR-104](../../docs/specification/adr/adr-104.md) D1).
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

use crate::ast::{Block, Expr, Item, Span, Stmt, Type};
use crate::check::{Finding, Severity};
use crate::parser::Parsed;

/// The crate word in front of a qualified name: `hyper_shim` of
/// `hyper_shim::serve_once`, and the whole of a name with no `::` in it.
pub fn head_of(name: &str) -> &str {
    name.split("::").next().unwrap_or(name)
}

/// **Every qualified name this unit writes**, with the span of the statement
/// or item it was written in.
///
/// The walk [`check`] runs, handed out rather than copied
/// ([ADR-104](../../docs/specification/adr/adr-104.md) D2): what the refusal
/// asks about is which crates a program reaches into, and what `nikaia
/// describe` asks is which *names* of one it reaches — the same walk, read one
/// segment further.
pub fn qualified_names(parsed: &Parsed) -> BTreeMap<String, Span> {
    let mut out: BTreeMap<String, Span> = BTreeMap::new();
    for item in &parsed.program.items {
        item_names(parsed, &item.node, &item.span, &mut |name: &str, span| {
            out.entry(name.to_string()).or_insert_with(|| *span);
        });
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
    for item in &parsed.program.items {
        item_names(
            parsed,
            &item.node,
            &item.span,
            &mut |name: &str, span: &Span| {
                let crate_name = head_of(name);
                if !declared.contains(crate_name) {
                    return;
                }
                if !described.contains(crate_name) {
                    first.entry(crate_name.to_string()).or_insert_with(|| *span);
                } else if moved.contains_key(crate_name) {
                    stale.entry(crate_name.to_string()).or_insert_with(|| *span);
                }
            },
        );
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
/// ([ADR-104](../../docs/specification/adr/adr-104.md) D5, on
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
        message: format!("`{crate_name}` has moved since its description was reviewed"),
        notes: vec![
            format!(
                "`contracts/{crate_name}.contracts` records what its sources hashed to, \
                 and {which} hashes differently now - so the entries answer for a crate \
                 that is not the one this build links against (ADR-104 D5)"
            ),
            "a ledger whose hashes do not hold is derived again; a description is \
             **reviewed** again instead, because what it says is a person's judgement \
             and not this compiler's - a signature that lies is caught there or by \
             nobody (ADR-100 D3)"
                .to_string(),
        ],
        help: Some(format!(
            "run `nikaia describe {crate_name}` and read the diff before you believe it"
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
        message: format!("`{crate_name}` is not described"),
        notes: vec![
            "every analysis reads a contract at the boundary - what may cross a thread, \
             what the call may reach, whether it pauses, whether it can fail - and an \
             undescribed crate is an absence rather than an answer (ADR-104 D1)"
                .to_string(),
            "the draft is written from the crate's own `pub` signatures, committed as \
             `contracts/<crate>.contracts`, and reviewed like code: what a signature \
             cannot say is written fail-closed (Part III, 15.2)"
                .to_string(),
        ],
        help: Some(format!("run `nikaia describe {crate_name}`")),
        labels: Vec::new(),
    }
}

/// The first segment of every qualified name an item writes, with the span the
/// message should carry.
fn item_names(parsed: &Parsed, item: &Item, span: &Span, found: &mut impl FnMut(&str, &Span)) {
    match item {
        Item::Fn {
            args,
            ret_type,
            body,
            ..
        } => {
            for arg in args {
                ty_names(parsed, &arg.ty, span, found);
            }
            if let Some(ret) = ret_type {
                ty_names(parsed, ret, span, found);
            }
            block_names(parsed, body, found);
        }
        Item::Struct { fields, .. } => {
            for field in fields {
                ty_names(parsed, &field.ty, span, found);
            }
        }
        Item::Impl { methods, .. } => {
            for method in methods {
                item_names(parsed, &method.node, &method.span, found);
            }
        }
        _ => {}
    }
}

fn block_names(parsed: &Parsed, block: &Block, found: &mut impl FnMut(&str, &Span)) {
    for stmt in &block.stmts {
        if let Stmt::Let { ty: Some(ty), .. } = &stmt.node {
            ty_names(parsed, ty, &stmt.span, found);
        }
        crate::contracts::sync::visit_stmt(parsed, &stmt.node, &mut |expr| {
            if let Expr::Path(segments) = expr
                && let Some(head) = segments.first()
            {
                // `unaliased`, because a file may write its own word for a
                // package (ADR-046 D3) - and the manifest key is the only
                // name the declaration has.
                // **The whole name and not its head**, because two
                // readers want it: the refusal takes the word in front
                // (`head_of`) and the describer takes the name after it.
                let head = parsed.unaliased(parsed.text(*head));
                let rest: Vec<String> = segments
                    .iter()
                    .skip(1)
                    .map(|s| parsed.text(*s).to_string())
                    .collect();
                let name = match rest.is_empty() {
                    true => head.to_string(),
                    false => format!("{head}::{}", rest.join("::")),
                };
                found(&name, &stmt.span);
            }
        });
        crate::contracts::sync::visit_stmt_blocks(&stmt.node, &mut |inner| {
            block_names(parsed, inner, found)
        });
    }
}

/// A written type may name a crate too - `hyper_shim::Handle` in a parameter is
/// as much a reach across the boundary as a call is, and it is the one the
/// crossing rules are about.
fn ty_names(parsed: &Parsed, ty: &Type, span: &Span, found: &mut impl FnMut(&str, &Span)) {
    let name = parsed.unaliased(parsed.text(ty.name));
    if name.contains("::") {
        found(&name, span);
    }
    for argument in &ty.generics {
        ty_names(parsed, argument, span, found);
    }
    if let Some(code) = &ty.code
        && let Some(result) = &code.result
    {
        ty_names(parsed, result, span, found);
    }
}
