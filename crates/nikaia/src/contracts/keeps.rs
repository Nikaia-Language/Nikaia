// crates/nikaia/src/contracts/keeps.rs
//
// Which parameters a body **keeps** ([ADR-094](../../../docs/specification/adr/adr-094.md) D2).
//
// The question the caller is asked today and should not be: `page(entries)` or
// `page(&entries)`. Whether an argument is lent or handed over is a fact about
// the *callee's body*, and the caller repeating it is 42 `&` in 913 non-comment
// lines of `examples/` saying what the signature had already said — plus a
// `rustc` error about a moved value wherever one is left out.
//
// *Keeps* means the value outlives the call: stored into a struct, assigned
// into a place, handed back by value, given to a task, or passed to a callee
// whose own parameter keeps it. Everything else is a read, and a read can be
// lent.
//
// **Fail closed, and here that means *keeps*.** A use this walk cannot account
// for counts as keeping, because the two wrong answers are not symmetric.
// Saying *kept* of a value that is only read costs a caller an owned argument —
// which is where every caller already is, so it costs nothing anybody has. Saying
// *lent* of a value the body stores emits a `&T` parameter whose body moves it,
// and that is `rustc`'s error about a file nobody wrote (Part III C.1). Same
// polarity as `sync` ([ADR-288](../../../docs/specification/adr/adr-288.md)) and
// as [ADR-010](../../../docs/specification/adr/adr-010.md) D1, pointing the
// other way because the claim points the other way.
//
// **Nothing reads this column yet**, which is ADR-094 §5's first step on
// purpose: the answer can be diffed against the corpus before one call site
// changes.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Expr, Item};
use crate::parser::Parsed;

use super::{INPUT, Ledger};
use crate::contracts::ty::TyOps;

/// Whether a callee **lends** the parameter at `at`, so that the compiler
/// writes the reference and the caller does not
/// ([ADR-094](../../../docs/specification/adr/adr-094.md) D1, D2).
///
/// One answer with three readers — the emitter writing the declaration, the
/// emitter writing the call, and the checker refusing a `&` somebody wrote —
/// because two of the three disagreeing is a `&&T` or a moved value, and both
/// are `rustc`'s words about a file nobody wrote (Part III C.1).
///
/// **The position has to exist and not be the receiver** — `&self` and `self`
/// are D6's and are untouched — and then one of two things has to hold.
///
/// *The type is written as a view.* `&str` and `&Vec[Row]` in a declaration
/// are the assertion D2 keeps: the parameter is a view whatever the body does,
/// so the argument gains a `&` at the call and the declaration is left exactly
/// as it was written. `keeps` is not consulted, because it has nothing to say
/// about a parameter whose kind the author wrote down.
///
/// *Or the value **moves** and the callee does not **keep** it.* That is the
/// inferred half, and both halves of the condition are load-bearing. A number,
/// a `bool`, a `char` and a hull are copied, so lending them buys nothing and
/// costs a dereference at every use; a type this compiler cannot name — `?`, a
/// type variable, a function type — is the absence of an answer, and a
/// reference written on a guess is a guess the language below reports. The
/// `keeps` clause is the column itself.
pub fn lends(contract: &super::FnContract, at: usize) -> bool {
    lends_in(contract, at, &[])
}

/// [`lends`], where a type a ledger says **copies** is copied rather than lent
/// ([ADR-294](../../../docs/specification/adr/adr-294.md) D9.1, the reading):
/// `winnow_grammar::Symbol` is one `u32`, and a `&Symbol` handed on to a
/// described call that takes a `Symbol` is `rustc`'s mismatch about a file
/// nobody wrote. Only a ledger's `copies = true` answers here; a type this
/// program declares is still lent, and reading its copy is the same step for
/// the types a unit declares.
///
/// The rule is `tools/lends.nika`'s (ADR-294, #125), with the three below.
pub fn lends_in(contract: &super::FnContract, at: usize, copying: &[&super::Ledger]) -> bool {
    nikaia_std::tools::lends::lends_in(contract, at as i64, copying)
}

/// Whether a ledger says values of `ty` are copies.
pub fn a_ledger_copies(ty: &super::ty::Ty, copying: &[&super::Ledger]) -> bool {
    nikaia_std::tools::lends::a_ledger_copies(ty, copying)
}

/// Whether a value of this type is **moved** when it is handed on, rather than
/// copied — and whether this compiler can say so at all: `Unknown`, a type
/// variable and a function type's absence of an answer are **no**, because what
/// hangs on it is a reference this compiler would *write*
/// ([ADR-312](../../../docs/specification/adr/adr-312.md) D1, ADR-302 D6,
/// ADR-150 D1, ADR-152 D2, ADR-277 D6).
pub fn moves(ty: &super::ty::Ty) -> bool {
    nikaia_std::tools::lends::moves(ty)
}

/// What one function's body does with each of its parameters, declared in
/// Nikaia (`tools/lends.nika`): kept on its own evidence, or handed to a callee
/// whose answer the fixpoint waits for.
use nikaia_std::tools::lends::{Uses, kept_by};

fn no_uses() -> Uses {
    Uses {
        kept: BTreeSet::new(),
        passed: Vec::new(),
    }
}

/// Give every function in the ledger the `keeps` its body earns.
///
/// A **least** fixpoint, where [`super::sync::infer`]'s is a greatest one, and
/// the difference is which direction is safe: `sync` is a promise and is taken
/// away on doubt, `keeps` is a restriction and is added on doubt. So this
/// starts from *nothing is kept* and adds until nothing changes, and two
/// functions that pass each other a parameter neither stores keep neither —
/// which a greatest fixpoint would have got wrong here exactly as a least one
/// would have got mutual recursion wrong there.
///
/// The iteration walks `BTreeMap`s and repeats until nothing changes, so the
/// answer does not depend on the order the source declared things in — which it
/// must not, because Part III 13.5 makes this file a pure function of (source,
/// toolchain) and `--locked` compares it byte for byte.
pub fn infer(
    ledger: &mut Ledger,
    units: &[&Parsed],
    library: &Ledger,
    resolved: &BTreeMap<String, crate::check::MethodCalls>,
) {
    let mut graph: BTreeMap<String, Uses> = BTreeMap::new();
    // What the package's declarations say about the types a parse hands back,
    // read once for the grammar arm below.
    let package = super::tether::declared_in(units);

    for parsed in units.iter().copied() {
        for item in &parsed.program.items {
            match &item.node {
                Item::Fn { .. } => {
                    if let Some((name, uses)) =
                        uses_of(parsed, &item.node, None, ledger, library, resolved)
                    {
                        graph.insert(name, uses);
                    }
                }
                Item::Impl {
                    target, methods, ..
                } => {
                    let target = parsed.text(target.name).to_string();
                    for method in methods {
                        if let Some((name, uses)) = uses_of(
                            parsed,
                            &method.node,
                            Some(&target),
                            ledger,
                            library,
                            resolved,
                        ) {
                            graph.insert(name, uses);
                        }
                    }
                }
                // **A `pub` rule is an entry, and its one parameter is the
                // text** ([ADR-296](../../../docs/specification/adr/adr-296.md)
                // D1). Whether it keeps that text is not a question about an
                // action block at all: a parse keeps its input exactly when
                // what it hands back holds a view **into** the input, which is
                // [ADR-283](../../../docs/specification/adr/adr-283.md)'s
                // tether read off the rule's declared result.
                //
                // **Before this the entry was in the ledger with the column
                // empty**, and `keeps_its` reads an absent `keeps` on a present
                // entry as *keeps nothing* — so every caller of a parse was
                // told it could lend text a parse holds views into. That is
                // this file's own polarity inverted, and it is what
                // [ADR-186](../../../docs/specification/adr/adr-186.md) D1 is
                // about: the answer is
                // derived now, so an empty column means *asked and no*.
                Item::Grammar(def) => {
                    let named = parsed.text(def.name).to_string();
                    for rule in def.rules.iter().filter(|r| r.is_public) {
                        let mut uses = no_uses();
                        if super::tether::a_parse_that_views(
                            parsed,
                            rule.ret_type.as_ref(),
                            &package,
                        ) {
                            uses.kept.insert(INPUT.to_string());
                        }
                        graph.insert(format!("{named}::{}", parsed.text(rule.name)), uses);
                    }
                }
                _ => {}
            }
        }
    }

    // **The least fixpoint is Nikaia** (`tools/lends.nika`, #125): it starts
    // from what each body keeps on its own evidence and adds what the callees
    // it hands a parameter to keep, until nothing changes.
    let kept = kept_by(&graph, ledger, library);
    for (name, parameters) in kept {
        if let Some(contract) = ledger.functions.get_mut(&name) {
            contract.keeps = parameters.into_iter().collect();
        }
    }
}

/// One function's parameters, and what its body does with each.
///
/// `None` for a declaration with no body — a `trait`'s method, which keeps
/// nothing because it does nothing, and whose `impl`s answer for themselves.
fn uses_of(
    parsed: &Parsed,
    item: &Item,
    target: Option<&str>,
    ledger: &Ledger,
    library: &Ledger,
    resolved: &BTreeMap<String, crate::check::MethodCalls>,
) -> Option<(String, Uses)> {
    let Item::Fn {
        name,
        receiver,
        args,
        body,
        ret_type,
        ..
    } = item
    else {
        return None;
    };

    let own = match name {
        Some(name) => parsed.text(*name).to_string(),
        None => "new".to_string(),
    };
    let key = match target {
        Some(target) => format!("{target}::{own}"),
        None => own,
    };

    let mut parameters: BTreeSet<String> = args
        .iter()
        .map(|a| parsed.text(a.name).to_string())
        .collect();
    // A method's receiver is a parameter, and `self.field = x` is one of the
    // shapes this analysis exists for — so it is in the set like any other.
    if target.is_some() {
        parameters.insert("self".to_string());
    }
    if parameters.is_empty() {
        return Some((key, no_uses()));
    }
    // **The parameters the author lent**: `ref self`, `ref mut self`, an
    // argument whose type is written `ref`, and one written `mut`, which the
    // callee changes in place ([ADR-094](../../../docs/specification/adr/adr-094.md)
    // D3). Each is a reference in the Rust whatever this walk decides, so a
    // `match` over one already matches through a reference - the
    // over-approximation in [`classify`]'s `match` arm is not for them
    // (issue #173, found at 0.0.254).
    let mut lent: BTreeSet<String> = args
        .iter()
        .filter(|a| a.ty.is_view || a.mutable)
        .map(|a| parsed.text(a.name).to_string())
        .collect();
    // Of those, the ones no body keeps: see below.
    let mut unkept: BTreeSet<String> = args
        .iter()
        .filter(|a| a.mutable)
        .map(|a| parsed.text(a.name).to_string())
        .collect();
    if target.is_some() && receiver.as_ref().is_some_and(|r| r.is_ref) {
        lent.insert("self".to_string());
        unkept.insert("self".to_string());
    }

    // **A result that is a view keeps nothing by returning.** `-> &str` hands
    // back a view of a parameter, which `borrows` already records; it is
    // `-> String` that moves the value out of the call.
    // **Text both kinds flow into is not a view to hand back**
    // ([ADR-282](../../../docs/specification/adr/adr-282.md) D21): an
    // `EitherText` may own its text, so returning a field of one takes it out
    // of the parameter, as returning a `String` field does.
    let returns_a_view = ret_type
        .as_ref()
        .is_some_and(|ty| super::holds_view(ty) && !holds_either(ty));

    // **The fields of each parameter whose type this file declares.** A
    // parameter of a type from a package or from `std` has none here, and
    // `hand_over` reads that absence as *unknown*, which keeps.
    let declared = crate::views::fields_of(parsed);
    let fields: BTreeMap<String, BTreeMap<String, super::ty::Ty>> = args
        .iter()
        .filter_map(|arg| {
            let of = declared.get(&arg.ty.name)?;
            Some((
                parsed.text(arg.name).to_string(),
                of.iter()
                    .map(|(name, ty)| (name.clone(), super::ty::Ty::from_ast(parsed, ty)))
                    .collect(),
            ))
        })
        .collect();

    // **Whether this body's method calls resolved at all.** A receiver whose
    // method nothing describes may be a `self`-by-value method, and a parameter
    // handed to one is moved out of — so the fail-closed answer for a *whole
    // body* is the one the type checker already computed
    // ([ADR-288](../../../docs/specification/adr/adr-288.md)).
    let unresolved = resolved.get(&key).is_some_and(|m| m.unresolved);
    // **The walk is Nikaia** (`tools/keeps.nika`, #125): every statement's
    // expressions classified where they stand, then the blocks it holds, and
    // **the value a body ends in leaves the call**, as a `return` does.
    let context = nikaia_std::tools::keeps::KeepsBody {
        parameters: parameters.clone(),
        lent,
        fields,
        returns_a_view,
        hands_back_its_last: ret_type.is_some() && !returns_a_view,
        unresolved,
    };
    let mut uses = nikaia_std::tools::keeps::uses_in(
        body,
        &parsed.interner,
        &|expr: &Expr| crate::emit::literal_expressions(parsed, expr),
        &context,
        ledger,
        library,
    );
    // **A receiver the author wrote `ref` is not kept** (issue #173), **nor an
    // argument written `mut`** (0.0.258): each is a reference by the author's
    // word, which no walk here widens - a use that would need it whole is
    // `NK1131`'s. An argument written `ref` is not in this rule: a parse keeps
    // the text its result views
    // ([ADR-186](../../../docs/specification/adr/adr-186.md) D1).
    uses.kept.retain(|name| !unkept.contains(name));
    uses.passed
        .retain(|passed| !unkept.contains(&passed.parameter));
    Some((key, uses))
}

/// Whether a declared type has text both kinds flow into anywhere in it.
fn holds_either(ty: &crate::ast::Type) -> bool {
    ty.either || ty.generics.iter().any(holds_either)
}
