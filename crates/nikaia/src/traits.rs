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
// declaration possible one commit ago — so this is the cheapest moment for a
// refusal: *a refusal is free before programs exist and breaking afterwards*.

use crate::check::Finding;
use crate::contracts::Ledger;
use crate::parser::Parsed;

/// Every way an `impl` can fail the `trait` it names - asked by
/// `tools/traits.nika` ([ADR-250](../../../docs/specification/adr/adr-250.md),
/// #125), the first check written in Nikaia. What stays here is the adapter:
/// the tree and the ledger go in, and the findings come back as the
/// compiler's own (`check::from_nikaia`), in the order of the source.
pub fn check(parsed: &Parsed, own: &Ledger) -> Vec<Finding> {
    let mut found: Vec<Finding> =
        nikaia_std::tools::traits::check_impls(&parsed.interner, &parsed.program.items, own)
            .into_iter()
            .map(from_nikaia)
            .collect();
    found.sort_by_key(|f| f.span.at());
    found
}

/// **A finding a check written in Nikaia handed back**, as the compiler's own
/// (`tools/findings.nika`, ADR-250). The one conversion there is: what the
/// check said is what is reported. A code is a `&'static str` here and text
/// there, so each code is kept once for the life of the compiler.
pub fn from_nikaia(found: nikaia_std::tools::findings::Finding) -> Finding {
    Finding {
        severity: match found.warning {
            true => crate::check::Severity::Warning,
            false => crate::check::Severity::Error,
        },
        span: found.span,
        code: static_code(&found.code),
        message: found.message,
        notes: found.notes,
        help: found.help,
        labels: Vec::new(),
    }
}

/// A code as the `&'static str` a `Finding` holds: kept once per code.
fn static_code(code: &str) -> &'static str {
    static CODES: std::sync::Mutex<std::collections::BTreeSet<&'static str>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    let mut codes = CODES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(kept) = codes.get(code) {
        return kept;
    }
    let kept: &'static str = Box::leak(code.to_string().into_boxed_str());
    codes.insert(kept);
    kept
}
