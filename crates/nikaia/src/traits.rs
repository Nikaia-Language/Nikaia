// crates/nikaia/src/traits.rs
//
// Kap 4.7: what an `impl` owes the `trait` it names.
//
// **A walk of its own, and the reason is the ledger's `sync` column.** The type
// checker runs *before* `sync::infer` — that order is what keeps ADR-027 sound,
// because the checker resolves method calls and the inference reads the result —
// so a rule that needs the finished `sync` cannot live inside it. This runs
// afterwards, from `check_program`, beside `dsl::check` and `views::check` and
// for the same reason each of those is separate: it asks a question the type
// walk does not have the answer to.
//
// **Both rules here are refusals that cost nothing today.** Nothing in
// `examples/`, `tests/samples/` or `crates/nikaia-std/src/` declares a trait at
// all — [ADR-078](../../../docs/specification/adr/adr-078.md) made the
// declaration possible one commit ago — so this is the former backlog file's own
// principle at its cheapest moment: *a refusal is free before programs exist and
// breaking afterwards*.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Item, Receiver, Span};
use crate::check::{Finding, Severity};
use crate::contracts::Ledger;
use crate::parser::Parsed;

/// Every way an `impl` can fail the `trait` it names.
pub fn check(parsed: &Parsed, own: &Ledger) -> Vec<Finding> {
    let mut found = Vec::new();
    for item in &parsed.program.items {
        let Item::Impl {
            trait_name: Some(trait_name),
            target,
            methods,
        } = &item.node
        else {
            continue;
        };
        let named = parsed.text(*trait_name).to_string();
        found.extend(receivers(parsed, &named, methods));
        // A trait this unit does not declare is one nothing here can check
        // against: `impl Error for ConfigError` names a trait the compiler reads
        // rather than one a `.nika` file wrote (ADR-023 D3), and a trait a
        // package publishes cannot be reached at all yet (ADR-078 §4). Silence
        // is the only correct answer about a declaration that is not here.
        let Some(declared) = own.traits.get(&named) else {
            continue;
        };
        let target_name = parsed.text(target.name).to_string();

        let mut given: BTreeMap<String, &Span> = BTreeMap::new();
        for method in methods {
            let Item::Fn {
                name: Some(name), ..
            } = &method.node
            else {
                continue;
            };
            given.insert(parsed.text(*name).to_string(), &method.span);
        }

        for (name, span) in &given {
            if !declared.contains(name) {
                found.push(not_in_the_trait(&named, &target_name, name, span));
                continue;
            }
            found.extend(pausing(own, &named, &target_name, name, span));
        }
        let missing: BTreeSet<&String> = declared
            .iter()
            .filter(|m| !given.contains_key(*m))
            .collect();
        if !missing.is_empty() {
            found.push(incomplete(&named, &target_name, &missing, &item.span));
        }
    }
    found.sort_by_key(|f| f.span.at());
    found
}

/// **`NK1196`: a method takes its `self` another way than the trait says**
/// (0.0.240).
///
/// `impl Error for E { fn message(self) … }` against `Error`'s
/// `message(ref self)` lowered as written, and `rustc` refused the generated
/// file - *cannot move out of `*self`* - about a line nobody wrote
/// ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)). Asked of
/// the three traits the language names and of a trait declared in this file;
/// one declared elsewhere keeps its receivers out of reach here, and silence is
/// the answer about a declaration that is not in view.
fn receivers(
    parsed: &Parsed,
    trait_name: &str,
    methods: &[crate::ast::Spanned<Item>],
) -> Vec<Finding> {
    let written = |receiver: Option<&Receiver>| match receiver {
        None => "no `self`",
        Some(Receiver {
            is_ref: true,
            is_mut: true,
        }) => "`ref mut self`",
        Some(Receiver {
            is_ref: true,
            is_mut: false,
        }) => "`ref self`",
        Some(Receiver {
            is_ref: false,
            is_mut: true,
        }) => "`mut self`",
        Some(Receiver {
            is_ref: false,
            is_mut: false,
        }) => "`self`",
    };
    let language = |method: &str| -> Option<Option<Receiver>> {
        let shared = Receiver {
            is_ref: true,
            is_mut: false,
        };
        let changed = Receiver {
            is_ref: true,
            is_mut: true,
        };
        match (trait_name, method) {
            ("Error", "message") => Some(Some(shared)),
            ("Drop", "drop") | ("Cleanup", "cleanup") => Some(Some(changed)),
            _ => None,
        }
    };
    let declared_here = parsed
        .program
        .items
        .iter()
        .find_map(|item| match &item.node {
            Item::Trait { name, methods, .. } if parsed.text(*name) == trait_name => Some(methods),
            _ => None,
        });
    let mut found = Vec::new();
    for method in methods {
        let Item::Fn {
            name: Some(name),
            receiver,
            ..
        } = &method.node
        else {
            continue;
        };
        let name = parsed.text(*name);
        let wanted = match declared_here {
            Some(declared) => declared
                .iter()
                .find(|m| parsed.text(m.node.name) == name)
                .map(|m| m.node.receiver),
            None => language(name),
        };
        let Some(wanted) = wanted else {
            continue;
        };
        let same = match (&wanted, receiver) {
            (None, None) => true,
            (Some(a), Some(b)) => a.is_ref == b.is_ref && a.is_mut == b.is_mut,
            _ => false,
        };
        if same {
            continue;
        }
        found.push(Finding {
            severity: Severity::Error,
            span: method.span,
            code: "NK1196",
            message: format!(
                "`{trait_name}`'s `{name}` takes {}, but this one takes {}.",
                written(wanted.as_ref()),
                written(receiver.as_ref())
            ),
            notes: vec![
                "Callers go through the trait, so they pass the value the way the trait \
                 declares."
                    .to_string(),
            ],
            help: Some(format!(
                "Take it as {}, like `{trait_name}` declares.",
                written(wanted.as_ref())
            )),
            labels: Vec::new(),
        });
    }
    found
}

/// **`NK1129`: the implementation pauses and the declaration says `sync`**
/// ([ADR-109](../../../docs/specification/adr/adr-109.md) D2), and **`NK1140`**:
/// it fails and the declaration has no `throws`.
///
/// Both are [ADR-027](../../../docs/specification/adr/adr-027.md)'s `NK2202`
/// asked of **somebody else's** signature — a body is checked against the word
/// the trait wrote, exactly as it is checked against its own.
///
/// **The other direction fits and says nothing.** A body that never pauses
/// under a declaration that may, or one that cannot fail under `throws`, is
/// correct: the declaration is the wider claim and a narrower body honours it.
///
/// **This used to refuse every pausing implementation**, and the reason is
/// worth keeping: [ADR-078](../../../docs/specification/adr/adr-078.md) D4
/// asserted `sync` for every trait method because a declaration has no body for
/// `sync::infer` to read, and `async fn` in a trait was a thing the emitter had
/// no way to ask for. ADR-109 D3 takes that cause away — the declaration is
/// lowered `-> impl Future<Output = …>` and the `impl` writes `async fn`, which
/// satisfies it — so the word can mean what it says, and the refusal is a
/// comparison rather than a blanket.
fn pausing(
    own: &Ledger,
    trait_name: &str,
    target: &str,
    method: &str,
    span: &Span,
) -> Vec<Finding> {
    let Some(contract) = own.functions.get(&format!("{target}::{method}")) else {
        return Vec::new();
    };
    let Some(declared) = own.functions.get(&format!("{trait_name}::{method}")) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    if declared.sync_claim.is_sync() && !contract.sync_claim.is_sync() {
        found.push(Finding {
            severity: Severity::Error,
            span: *span,
            code: "NK1129",
            message: format!(
                "`{target}::{method}` can pause, but `{trait_name}` declares `{method}` as \
                 `sync`."
            ),
            notes: vec![
                "`sync` in a trait promises that every implementation never pauses. (The \
                 opposite is fine: a body that never pauses can implement a method that may.)"
                    .to_string(),
            ],
            help: Some(format!(
                "Remove `sync` from `{trait_name}`'s `{method}`, or take out what pauses \
                 in the body, such as a file read, a sleep or a `.join()`."
            )),
            labels: Vec::new(),
        });
    }
    if declared.fails_with.is_empty() && !contract.fails_with.is_empty() {
        found.push(Finding {
            severity: Severity::Error,
            span: *span,
            code: "NK1140",
            message: format!(
                "`{target}::{method}` can fail, but `{trait_name}` declares `{method}` \
                 without `throws`."
            ),
            notes: vec![
                "A trait method without `throws` promises that no implementation fails. \
                 (The opposite is fine: a body that can't fail can implement one with \
                 `throws`.)"
                    .to_string(),
            ],
            help: Some(format!(
                "Add `throws` to `{trait_name}`'s `{method}`, or handle the failure in the \
                 body with `catch`."
            )),
            labels: Vec::new(),
        });
    }
    found
}

/// **`NK1130`: the `impl` and the `trait` do not agree on which methods exist.**
///
/// The half [ADR-078](../../../docs/specification/adr/adr-078.md) §4 left open:
/// a trait existed to be checked against and nothing checked, so a method the
/// trait does not declare, or one it declares and the `impl` leaves out, went to
/// `rustc` — `E0407` and `E0046`, about the generated file.
///
/// Two messages rather than one code per direction, because a reader is doing
/// two different things: adding a method to the trait or taking one out of the
/// `impl`; and finishing an `impl` that is not done.
fn not_in_the_trait(trait_name: &str, target: &str, method: &str, span: &Span) -> Finding {
    Finding {
        severity: Severity::Error,
        span: *span,
        code: "NK1130",
        message: format!("`{trait_name}` has no method called `{method}`."),
        notes: vec![format!(
            "`impl {trait_name} for {target}` can only hold `{trait_name}`'s methods. The \
             type's own methods go in a separate `impl {target}`."
        )],
        help: Some(format!(
            "Move it to `impl {target}`, or add `{method}` to `{trait_name}` if every type \
             that implements it should have one."
        )),
        labels: Vec::new(),
    }
}

fn incomplete(trait_name: &str, target: &str, missing: &BTreeSet<&String>, span: &Span) -> Finding {
    let names: Vec<String> = missing.iter().map(|m| format!("`{m}`")).collect();
    Finding {
        severity: Severity::Error,
        span: *span,
        code: "NK1130",
        message: format!(
            "`{target}` is missing {} from `{trait_name}`.",
            names.join(", ")
        ),
        notes: vec![
            "A trait promises that all its methods are there, which is what lets code \
             call them through a bound."
                .to_string(),
        ],
        help: Some(format!(
            "Add {} to this `impl`, or remove {} from `{trait_name}`.",
            names.join(", "),
            match names.len() {
                1 => "it".to_string(),
                _ => "them".to_string(),
            }
        )),
        labels: Vec::new(),
    }
}
