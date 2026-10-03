// crates/nikaia/src/contracts/sync.rs
//
// Part II 12.1: a `sync` function may only call `sync` functions.
//
// Two analyses of one rule, running in **opposite directions**, and keeping
// them apart is the whole design (ADR-288).
//
// `check` verifies an assertion. Someone wrote `sync`, and this reports the
// calls that contradict it (`NK2202`). It is conservative in the **permissive**
// direction: with no receiver types, `a.method()` cannot be looked up, so it is
// not an error. What *can* be looked up is a call by name - a function in this
// unit, or a path like `io::read_to_string` into a library's ledger - and that
// is where the rule earns its keep, because `fs::` and `io::` are named rather
// than called on a receiver. The check never rejects a program the rule allows,
// and does not yet catch every program the rule forbids. An unchecked promise
// catches nothing at all, so that is worth having and worth saying.
//
// `infer` makes a claim, and therefore runs the other way. It writes `sync`
// into the ledger for a function nobody annotated, and that entry is **shipped**
// (Part III 13.5): a consumer reads it and puts the function inside `access`.
// So it is conservative in the **restrictive** direction - a call it cannot
// resolve is a call it cannot vouch for, and the function does not get the
// promise. The ledger settled this polarity once already, for provenance: "an
// analysis that fails open is a vulnerability generator". Inferring `sync` from
// a body full of calls one cannot see would be exactly that.
//
// **Why infer at all.** Before this, `sync` was opt-in, so almost nothing was
// `sync`, so `access`, `access_all`, `par_iter` and the panic hook - everything
// Part II 12.2 makes safe by demanding a `sync` lambda - could call almost
// nothing. The restrictive side of the language was the unusable one, and the
// way out was to annotate a chain of pure helpers by hand. Now a body that
// provably cannot pause says so on its own, and `sync` in the source becomes an
// **assertion you write where you want it held**, checked against the body,
// rather than a mode you have to enter.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Block, Expr, Item, Span, Stmt};
use crate::parser::Parsed;

use crate::check::MethodCalls;

use super::{Ledger, Sync};

/// One call that a `sync` function may not make.
#[derive(Debug, Clone)]
pub struct Violation {
    /// The statement the call is in. Expression-level spans are open work
    /// (`docs/project_status_and_roadmap.md`, Phase 2), so this points at the
    /// statement rather than at the call inside it.
    pub span: Span,
    /// The `sync` function making the call.
    pub caller: String,
    /// What it promised, as the source wrote it: `sync`, or `sync(f)`
    /// ([ADR-288](../../../docs/specification/adr/adr-288.md) D31).
    pub promise: String,
    /// What it called, as the ledger names it.
    pub callee: String,
    /// Which ledger answered - this program's, or a library's.
    pub from_library: bool,
    /// Whether `callee` is another package's that never pauses and does not
    /// promise it ([ADR-288](../../../docs/specification/adr/adr-288.md) D28):
    /// refused for the missing word, not for a pause, and said so.
    pub unpromised: bool,
    /// Whether `callee` is a **construct** rather than a function
    /// ([ADR-292](../../../docs/specification/adr/adr-292.md) D16).
    ///
    /// An `overlap` and a `select` join on the executor, so a body holding one
    /// pauses - and no ledger says so, because neither is a call. The
    /// diagnostic says where the pause is instead of naming an entry that does
    /// not exist.
    pub construct: bool,
}

/// Every call a `sync` function makes that the ledgers say can pause.
pub fn check(parsed: &Parsed, own: &Ledger, library: &Ledger) -> Vec<Violation> {
    let mut found = Vec::new();

    for item in &parsed.program.items {
        match &item.node {
            Item::Fn { .. } => walk_fn(parsed, &item.node, None, own, library, &mut found),
            Item::Impl {
                target, methods, ..
            } => {
                let target = parsed.text(target.name).to_string();
                for method in methods {
                    walk_fn(
                        parsed,
                        &method.node,
                        Some(&target),
                        own,
                        library,
                        &mut found,
                    );
                }
            }
            _ => {}
        }
    }

    found.sort_by_key(|v| v.span.at());
    found
}

/// What one function's body does to its own claim to be `sync`.
#[derive(Debug, Default)]
struct Reach {
    /// It calls something that can pause, or something that cannot be resolved.
    /// Either way the claim is off the table and no fixpoint will bring it back.
    blocked: bool,
    /// The functions in this **package** it calls — every unit of it, since
    /// [ADR-100](../../../docs/specification/adr/adr-100.md) D2. Its claim holds
    /// only while all of theirs do.
    calls: BTreeSet<String>,
    /// **The code parameter it runs**
    /// ([ADR-277](../../../docs/specification/adr/adr-277.md) D8), where there
    /// is one: the answer is *the lambda decides*, which is `sync = "from(f)"`.
    ///
    /// A lambda the callee **runs during the call** adds nothing to the
    /// caller's own answers, because the lambda's body is walked as part of the
    /// function that writes it and its calls are already counted there
    /// ([ADR-288](../../../docs/specification/adr/adr-288.md) D15). So the claim
    /// holds here and the question travels to the caller with the name.
    ///
    /// **Only the first**, where a body runs two. The ledger's spelling names
    /// one parameter and no `std` entry or written signature has ever had two;
    /// a second would want a spelling before it wants an inference.
    runs: Option<String>,
    /// **The parameters that are code and may pause, which it calls**
    /// ([ADR-288](../../../docs/specification/adr/adr-288.md) D29). Each takes
    /// the claim away as `blocked` does; kept apart so that a function whose
    /// *only* pausing is its lambdas' can be named as one that could promise
    /// `sync(f)`.
    through_code: BTreeSet<String>,
    /// **Where it first pauses itself**, if a call or a construct in its own
    /// body is why ([ADR-288](../../../docs/specification/adr/adr-288.md) D32):
    /// the statement and what in it pauses. `None` where the pause is a method
    /// call the type checker resolved, or a function of the package it calls.
    site: Option<(Span, String)>,
    /// The unit the function is written in, by its place in the walk's list.
    unit: usize,
}

/// Give every function in the ledger the `sync` its body earns.
///
/// Runs after the entries exist, and only ever *adds* [`Sync::Inferred`]: an
/// assertion in the source is what the source said and is left exactly as it
/// was written, so that `NK2202` still has something to contradict.
///
/// The fixpoint is a greatest one - start from "every candidate is `sync`" and
/// take the claim away from anything that reaches a function without it. Two
/// consequences worth naming. Mutual recursion between pure functions keeps the
/// claim, which is correct and is what a least fixpoint would have got wrong.
/// And the iteration walks a `BTreeMap` and repeats until nothing changes, so
/// the answer does not depend on the order the source declared things in -
/// which it must not, because 13.5 makes this file a pure function of (source,
/// toolchain) and `--locked` compares it byte for byte.
///
/// `resolved` is the type checker's answer to the one question this walk cannot
/// ask: what a method call goes to (ADR-288). Handing it in rather than
/// computing it here keeps one type checker in the compiler; the alternative
/// was a second, worse one living in this file.
///
/// **Hands back the functions that pause only where their lambdas do**
/// ([ADR-288](../../../docs/specification/adr/adr-288.md) D29): by key, the code
/// parameters that may pause and that each one calls. Nothing else it reaches
/// can pause, so `sync(those)` is a promise its body would keep. The ledger is
/// not told - the inference never writes `sync(f)`, because a promise is the
/// source's to make (§4) - and the build says so in a note instead.
pub fn infer(
    ledger: &mut Ledger,
    units: &[&Parsed],
    library: &Ledger,
    resolved: &BTreeMap<String, MethodCalls>,
) -> Noted {
    let mut graph: BTreeMap<String, Reach> = BTreeMap::new();

    for (unit, parsed) in units.iter().copied().enumerate() {
        for item in &parsed.program.items {
            match &item.node {
                Item::Fn { .. } => {
                    if let Some((name, mut reach)) =
                        reach_of(parsed, &item.node, None, ledger, library, resolved)
                    {
                        reach.unit = unit;
                        graph.insert(name, reach);
                    }
                }
                Item::Impl {
                    trait_name,
                    target,
                    methods,
                    ..
                } => {
                    let target = parsed.text(target.name).to_string();
                    let declared_by = trait_name.map(|t| parsed.text(t).to_string());
                    for method in methods {
                        if let Some((name, mut reach)) = reach_of(
                            parsed,
                            &method.node,
                            Some(&target),
                            ledger,
                            library,
                            resolved,
                        ) {
                            // **A declaration is the wider claim, and its
                            // implementations carry it**
                            // ([ADR-288](../../../docs/specification/adr/adr-288.md)
                            // D2): a trait method without `sync` is lowered
                            // `-> impl Future<…>`, so every `impl` of it hands
                            // back a future whether or not its own body pauses
                            // — and a caller writing `t.go()` on the concrete
                            // type has to `.await` it.
                            //
                            // **An edge and not a correction afterwards**, so
                            // the fixpoint carries it the rest of the way: the
                            // `main` that calls `t.go()` is `async` for the
                            // same reason `t.go()` is.
                            //
                            // **Only where this unit declares the trait.** A
                            // `impl Error for ConfigError` names a trait the
                            // compiler reads rather than one a `.nika` file
                            // wrote, and an edge to a name the graph has no
                            // entry for is read as *pauses* by
                            // `unwrap_or(false)` — which would make every error
                            // type's methods `async`. Silence about a
                            // declaration that is not here is the rule rather
                            // than a gap, and `traits::check` says the same.
                            if let Some(declared_by) = &declared_by
                                && let Some(own) = name.rsplit("::").next()
                            {
                                let declared = format!("{declared_by}::{own}");
                                // **Either ledger**, because a trait a
                                // *dependency* publishes is declared just
                                // as much as one written here — the
                                // `handler::Handler` an app implements is
                                // the case (ADR-100 D1: a consumer reads a
                                // dependency's contracts).
                                if ledger.functions.contains_key(&declared)
                                    || library.functions.contains_key(&declared)
                                {
                                    reach.calls.insert(declared);
                                }
                            }
                            reach.unit = unit;
                            graph.insert(name, reach);
                        }
                    }
                }
                // Kap 4.7: a trait's methods are in this package's ledger, so a body
                // that reaches one through a bound names a callee the graph has to
                // know about. **A leaf** — a declaration has no body, so it reaches
                // nothing — and its `blocked` is the **word it was written with**
                // ([ADR-288](../../../docs/specification/adr/adr-288.md) D24): a
                // trait method reads like a function type, so without `sync` it may
                // pause, and a body that calls it through a bound pauses with it.
                //
                // **It used to be `blocked: false` whatever the declaration said**
                // ([ADR-295](../../../docs/specification/adr/adr-295.md) D9),
                // because a plain `fn` was the only thing the emitter could write
                // in a trait and `No` would have made every call through a bound an
                // `.await`. ADR-288 D26 takes that cause away with the
                // return-position form, so the word is read rather than overridden.
                //
                // Without the entry at all the callee is absent from `holds` and
                // `unwrap_or(false)` reads that as *pauses* — which is why a leaf
                // is inserted either way rather than left out.
                Item::Trait { name, methods, .. } => {
                    let own = parsed.text(*name).to_string();
                    for method in methods {
                        graph.insert(
                            format!("{own}::{}", parsed.text(method.node.name)),
                            Reach {
                                blocked: !method.node.is_sync,
                                calls: BTreeSet::new(),
                                runs: None,
                                through_code: BTreeSet::new(),
                                site: None,
                                unit: 0,
                            },
                        );
                    }
                }
                // **A grammar's `pub` rules are leaves too, and they hold**
                // ([ADR-296](../../../docs/specification/adr/adr-296.md) D35, D36).
                // An entry has no body in this graph's sense — its action blocks
                // are checked rather than walked here — and an action may not
                // pause, so the entry is `sync` and a caller keeps its own claim.
                //
                // **Inserted for the reason the trait leaf above is**: without an
                // entry the callee is absent from `holds` and `unwrap_or(false)`
                // reads that as *pauses*, which is what made every function that
                // parses `async` the day
                // [ADR-140](../../../docs/specification/adr/adr-140.md) D3 turned
                // the entry into a call by name.
                Item::Grammar(def) => {
                    let grammar = parsed.text(def.name).to_string();
                    for rule in def.rules.iter().filter(|r| r.is_public) {
                        graph.insert(
                            format!("{grammar}::{}", parsed.text(rule.name)),
                            Reach {
                                blocked: false,
                                calls: BTreeSet::new(),
                                runs: None,
                                through_code: BTreeSet::new(),
                                site: None,
                                unit: 0,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
    }

    // **The fixpoint is Nikaia** (`tools/sync.nika`, #125): start optimistic,
    // take the claim away until nothing changes, and name the functions whose
    // only pausing is their lambdas' (ADR-288 D29).
    let reached: BTreeMap<String, nikaia_std::tools::sync::SyncReach> = graph
        .iter()
        .map(|(name, reach)| {
            (
                name.clone(),
                nikaia_std::tools::sync::SyncReach {
                    blocked: reach.blocked,
                    calls: reach.calls.clone(),
                    through_code: reach.through_code.clone(),
                    pauses_here: reach.site.is_some(),
                },
            )
        })
        .collect();
    let holds = nikaia_std::tools::sync::sync_holds(&reached);
    let by_code = nikaia_std::tools::sync::paused_only_by_code(&reached, &holds);

    for (name, holds) in &holds {
        if !holds {
            continue;
        }
        if let Some(contract) = ledger.functions.get_mut(name)
            && contract.sync_claim == Sync::No
        {
            // **`from(f)` where a code parameter is what decides**
            // ([ADR-277](../../../docs/specification/adr/adr-277.md) D8,
            // [ADR-288](../../../docs/specification/adr/adr-288.md) D15), and
            // `inferred` otherwise. A caller reads both the same way — *this
            // call adds no pausing of its own* — and the difference is that
            // `from` says **whose** answer it is, which is what a reader of
            // the ledger and a second build of the same package need.
            contract.sync_claim = match graph.get(name).and_then(|reach| reach.runs.clone()) {
                Some(parameter) => Sync::From(parameter),
                None => Sync::Inferred,
            };
        }
    }

    // **Why each function that does not hold pauses** (ADR-288 D32): the
    // shortest way through the package's calls to a statement that pauses
    // itself. Breadth-first and over the map's order, so the answer is the same
    // on every build.
    let mut why: BTreeMap<String, Pause> = BTreeMap::new();
    for start in graph.keys().filter(|name| !holds[name.as_str()]) {
        let chain = nikaia_std::tools::sync::pause_chain(&reached, &holds, start);
        if let Some(reach) = chain.last().and_then(|at| graph.get(at)) {
            why.insert(
                start.clone(),
                Pause {
                    chain: chain.clone(),
                    unit: reach.unit,
                    site: reach.site.clone(),
                },
            );
        }
    }
    Noted { by_code, why }
}

/// What [`infer`] learns besides the ledger's own column, for the build to say
/// ([ADR-288](../../../docs/specification/adr/adr-288.md) D29, D32).
#[derive(Debug, Default)]
pub struct Noted {
    /// The functions that pause only where their lambdas do, with the code
    /// parameters that decide (D2).
    pub by_code: BTreeMap<String, Vec<String>>,
    /// Why each function that can pause does (D5).
    pub why: BTreeMap<String, Pause>,
}

/// **The way from a function to the statement that makes it pause**.
#[derive(Debug, Clone)]
pub struct Pause {
    /// The function, the ones of its package it calls on the way, and last the
    /// one that pauses itself.
    pub chain: Vec<String>,
    /// The unit the last one is written in, by its place in the list
    /// [`infer`] was handed.
    pub unit: usize,
    /// The statement and what in it pauses; `None` where it is a method call
    /// only the type checker resolved.
    pub site: Option<(Span, String)>,
}

/// One function's calls, split into what settles the question now and what
/// depends on the rest of the package.
///
/// `None` where the item is not a function. A function whose body cannot be
/// seen at all would be `blocked`, not absent - but Stage 0 has no such thing.
fn reach_of(
    parsed: &Parsed,
    item: &Item,
    target: Option<&str>,
    own: &Ledger,
    library: &Ledger,
    resolved: &BTreeMap<String, MethodCalls>,
) -> Option<(String, Reach)> {
    let Item::Fn {
        name, body, args, ..
    } = item
    else {
        return None;
    };
    let own_name = match name {
        Some(name) => parsed.text(*name).to_string(),
        None => "new".to_string(),
    };
    let key = match target {
        Some(target) => format!("{target}::{own_name}"),
        None => own_name,
    };

    // **The parameters that are code, and whether each may pause**
    // ([ADR-277](../../../docs/specification/adr/adr-277.md) D6,
    // [ADR-277](../../../docs/specification/adr/adr-277.md) D11). Built before
    // the walk because the walk is what reads it: a call to one of these names
    // is not a name nothing describes.
    let code: BTreeMap<String, bool> = args
        .iter()
        .filter_map(|arg| {
            let declared = (*arg.ty.code).as_ref()?;
            Some((parsed.text(arg.name).to_string(), !declared.is_sync))
        })
        .collect();

    let mut reach = Reach::default();
    collect_reach(parsed, body, own, library, &code, &mut reach);

    // What the walk above left to somebody else: every method call this
    // function makes, as the type checker resolved it (ADR-288). The two are
    // merged rather than reconciled - the walk skips method calls entirely and
    // this covers exactly those - so nothing is counted twice and nothing is
    // dropped.
    if let Some(methods) = resolved.get(&key) {
        // One method whose receiver is not known is enough. It is the absence
        // of an answer, and D2's polarity says what to do with one.
        reach.blocked |= methods.unresolved || methods.code_pauses;
        for callee in &methods.resolved {
            if own.functions.contains_key(callee) {
                reach.calls.insert(callee.clone());
            } else if !library
                .functions
                .get(callee)
                .is_some_and(|contract| contract.sync_claim.is_sync())
            {
                // A library method that can pause, or one that resolved to a
                // name this ledger does not carry after all.
                reach.blocked = true;
            }
        }
    }

    Some((key, reach))
}

fn collect_reach(
    parsed: &Parsed,
    block: &Block,
    own: &Ledger,
    library: &Ledger,
    code: &BTreeMap<String, bool>,
    reach: &mut Reach,
) {
    use crate::foreign::Seen;
    use nikaia_std::tools::calls::Callee;
    for seen in crate::foreign::seen_in(parsed, block) {
        let (callee, span) = match seen {
            Seen::Joins { construct, span } => {
                reach.blocked = true;
                reach
                    .site
                    .get_or_insert((span, format!("an `{construct}` block")));
                continue;
            }
            Seen::Call { name, span, .. } => (named(parsed, name, own, library), span),
            Seen::Spawn { span } | Seen::Opaque { span } => (Some(Callee::Opaque(None)), span),
            _ => continue,
        };
        match callee {
            Some(Callee::Own(name)) => {
                reach.calls.insert(name);
            }
            Some(Callee::Opaque(Some(name))) if code.contains_key(&name) => {
                if code[&name] {
                    reach.site.get_or_insert((span, format!("`{name}`")));
                    reach.through_code.insert(name);
                }
            }
            Some(Callee::Library {
                never_pauses: false,
                key,
            }) => {
                reach.blocked = true;
                reach.site.get_or_insert((span, format!("`{key}`")));
            }
            Some(Callee::Opaque(name)) => {
                reach.blocked = true;
                reach.site.get_or_insert((
                    span,
                    name.map_or(
                        "something this compiler cannot see the end of".to_string(),
                        |n| format!("`{n}`"),
                    ),
                ));
            }
            Some(Callee::Method) => {}
            Some(Callee::Library {
                never_pauses: true, ..
            })
            | None => {}
        }
    }
}

fn walk_fn(
    parsed: &Parsed,
    item: &Item,
    target: Option<&str>,
    own: &Ledger,
    library: &Ledger,
    found: &mut Vec<Violation>,
) {
    let Item::Fn {
        name,
        args,
        body,
        is_sync,
        sync_by,
        ..
    } = item
    else {
        return;
    };
    if !*is_sync && sync_by.is_empty() {
        return;
    }
    // **A parameter whose type says `sync` cannot pause** (Part I 5.4 C,
    // 0.0.244): the caller is held to it - `NK2206` refuses a pausing lambda
    // handed to one - so a call of it here keeps the promise. Read as a call
    // to something no ledger knows, `apply(f: fn(i64) -> i64 sync, …) sync`
    // was refused for calling `f`.
    //
    // **And a parameter `sync(f)` names** (ADR-288 D31): its lambda is the
    // caller's to answer for, so a call of it is the one pause the promise
    // allows.
    let mut sync_code: BTreeSet<String> = args
        .iter()
        .filter(|arg| (*arg.ty.code).as_ref().is_some_and(|code| code.is_sync))
        .map(|arg| parsed.text(arg.name).to_string())
        .collect();
    sync_code.extend(sync_by.iter().map(|name| parsed.text(*name).to_string()));
    let promise = match sync_by.is_empty() {
        true => "sync".to_string(),
        false => format!(
            "sync({})",
            sync_by
                .iter()
                .map(|name| parsed.text(*name))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };

    let own_name = match name {
        Some(name) => parsed.text(*name).to_string(),
        None => "new".to_string(),
    };
    let caller = match target {
        Some(target) => format!("{target}::{own_name}"),
        None => own_name,
    };

    walk_block(
        parsed, body, &caller, &promise, &sync_code, own, library, found,
    );
}

#[allow(clippy::too_many_arguments)]
fn walk_block(
    parsed: &Parsed,
    block: &Block,
    caller: &str,
    promise: &str,
    sync_code: &BTreeSet<String>,
    own: &Ledger,
    library: &Ledger,
    found: &mut Vec<Violation>,
) {
    use crate::foreign::Seen;
    use nikaia_std::tools::calls::Callee;
    for seen in crate::foreign::seen_in(parsed, block) {
        let violation = |span, callee: String, from_library, unpromised, construct| Violation {
            span,
            caller: caller.to_string(),
            promise: promise.to_string(),
            callee,
            from_library,
            unpromised,
            construct,
        };
        let (callee, span) = match seen {
            Seen::Joins { construct, span } => {
                found.push(violation(span, construct, false, false, true));
                continue;
            }
            // A code parameter the promise names is the caller's to keep: a
            // call of one by its bare name is not this body's pause.
            Seen::Call { name, .. } if !name.contains("::") && sync_code.contains(&name) => {
                continue;
            }
            Seen::Call { name, span, .. } => (named(parsed, name, own, library), span),
            Seen::Spawn { span } | Seen::Opaque { span } => (Some(Callee::Opaque(None)), span),
            _ => continue,
        };
        let pauses = match callee {
            Some(Callee::Own(name)) => own.functions.get(&name).and_then(|contract| {
                let unpromised = contract.sync_claim == Sync::Unpromised;
                (!contract.sync_claim.is_sync()).then_some((name, false, unpromised))
            }),
            Some(Callee::Library { key, never_pauses }) => {
                (!never_pauses).then_some((key, true, false))
            }
            Some(Callee::Method) | None => None,
            Some(Callee::Opaque(name)) => Some((
                name.unwrap_or_else(|| "something this compiler cannot resolve".to_string()),
                false,
                false,
            )),
        };
        if let Some((callee, from_library, unpromised)) = pauses {
            found.push(violation(span, callee, from_library, unpromised, false));
        }
    }
}

/// What a call by `name` goes to: `calls::callee_named`, the one resolution
/// every analysis shares (ADR-288), for a name the walk in Nikaia read.
fn named(
    parsed: &Parsed,
    name: String,
    own: &Ledger,
    library: &Ledger,
) -> Option<nikaia_std::tools::calls::Callee> {
    nikaia_std::tools::calls::callee_named(
        &parsed.interner,
        name,
        own,
        library,
        &parsed.program.items,
    )
}

/// What one expression tells either analysis, where it is a call at all.
///
/// One resolution rule, written once. The check and the inference disagree
/// about what to *do* with `Opaque` - the first shrugs, the second refuses -
/// and that disagreement is the design. Having them disagree about what a call
/// even resolves to would just be a bug waiting to happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reached {
    /// A function this package declares, by the name the ledger records it under.
    Own(String),
    /// A function in a library, and what that library's ledger says about it.
    Library { key: String, sync: bool },
    /// A method call. Neither analysis here can resolve one: `stats.add(5)`
    /// names `add` and says nothing about what `stats` is.
    ///
    /// The **type checker** can, and does (ADR-288). So this is not "unknown"
    /// but "asked elsewhere", and the two callers of `reached` take it
    /// differently: the inference merges in the checker's answer per function,
    /// and the check looks the resolved name up the same way it looks up any
    /// other. Collapsing this into `Opaque` was what threw the answer away.
    Method,
    /// A call whose target this compiler cannot name and nobody else can
    /// either: a name no ledger knows.
    ///
    /// Also everything that is not a plain call but still *runs* something -
    /// `spawn`, a `dsl` - because a body containing one is not the pure CPU
    /// task Part II 12.1 describes, whatever the thing it runs turns out to do.
    ///
    /// `Some(name)` where the source wrote one and no ledger knew it, `None`
    /// for a construct that has no callee to name. Both block, and the name is
    /// carried only so the diagnostic can say which call it was about.
    Opaque(Option<String>),
}

/// What a call resolves to, by the same rule for every analysis:
/// `tools/calls.nika`'s `callee_of` (#125). This compiler's walk hands it each
/// expression and reads the answer back as its own enum.
///
/// `None` means the expression is not a call at all - or a variant of a type,
/// which builds a value and runs no body - the one case no analysis has
/// anything to say about.
pub(crate) fn reached(
    parsed: &Parsed,
    expr: &Expr,
    own: &Ledger,
    library: &Ledger,
) -> Option<Reached> {
    use nikaia_std::tools::calls::{Callee, callee_of};
    Some(
        match callee_of(&parsed.interner, expr, own, library, &parsed.program.items)? {
            Callee::Own(name) => Reached::Own(name),
            Callee::Library { key, never_pauses } => Reached::Library {
                key,
                sync: never_pauses,
            },
            Callee::Method => Reached::Method,
            Callee::Opaque(name) => Reached::Opaque(name),
        },
    )
}

/// Every expression a statement holds, without descending into nested blocks -
/// those are walked separately so that each keeps its own statement's span.
pub(crate) fn visit_stmt(parsed: &Parsed, stmt: &Stmt, f: &mut impl FnMut(&Expr)) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Comptime { value, .. } => visit_expr(parsed, value, f),
        Stmt::Assign { target, value, .. } => {
            visit_expr(parsed, target, f);
            visit_expr(parsed, value, f);
        }
        Stmt::For { iter, .. } => visit_expr(parsed, iter, f),
        Stmt::While { cond, .. } => visit_expr(parsed, cond, f),
        Stmt::Return(Some(value)) => visit_expr(parsed, value, f),
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
        Stmt::Expr(expr) => visit_expr(parsed, expr, f),
    }
}

pub(crate) fn visit_stmt_blocks<'a>(stmt: &'a Stmt, f: &mut impl FnMut(&'a Block)) {
    match stmt {
        Stmt::For { body, .. } | Stmt::While { body, .. } => f(body),
        Stmt::Let { value, .. } | Stmt::Comptime { value, .. } | Stmt::Expr(value) => {
            visit_expr_blocks(value, f)
        }
        Stmt::Assign { target, value, .. } => {
            visit_expr_blocks(target, f);
            visit_expr_blocks(value, f);
        }
        Stmt::Return(Some(value)) => visit_expr_blocks(value, f),
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
    }
}

/// Every block an expression holds, including the body of a lambda passed as
/// an argument.
///
/// A trailing lambda runs *during* the call it is given to - `.and_modify fn {
/// … }` is not deferred - so what it calls, the function around it calls. The
/// one shape that is different is `spawn`, whose body runs later and elsewhere;
/// it is a detached context (Part I, 5.4) and is not walked here.
pub(crate) fn visit_expr_blocks<'a>(expr: &'a Expr, f: &mut impl FnMut(&'a Block)) {
    match expr {
        // An `unsafe` block is part of the function that writes it
        // ([ADR-302](../../../docs/specification/adr/adr-302.md) D3): it makes
        // no boundary of its own, so what it calls, the function calls.
        Expr::Block(block)
        | Expr::Unsafe(block)
        | Expr::Overlap(block)
        | Expr::Closure { body: block, .. } => f(block),
        Expr::Call { func, args, config } => {
            visit_expr_blocks(func, f);
            args.iter().for_each(|a| visit_expr_blocks(a, f));
            config.iter().for_each(|c| visit_expr_blocks(&c.value, f));
        }
        Expr::MethodCall {
            receiver,
            args,
            config,
            ..
        }
        | Expr::SafeMethod {
            receiver,
            args,
            config,
            ..
        } => {
            visit_expr_blocks(receiver, f);
            args.iter().for_each(|a| visit_expr_blocks(a, f));
            config.iter().for_each(|c| visit_expr_blocks(&c.value, f));
        }
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => {
            f(then_branch);
            if let Some(block) = else_branch {
                f(block);
            }
        }
        Expr::TryCatch { expr, handler } => {
            visit_expr_blocks(expr, f);
            f(handler);
        }
        Expr::Match { arms, .. } => {
            for arm in arms {
                visit_expr_blocks(&arm.body, f);
            }
        }
        // A `select` arm's body runs in this function once its value won.
        Expr::Select(arms) => arms.iter().for_each(|arm| f(&arm.body)),
        _ => {}
    }
}

/// Every expression inside one, excluding the bodies of nested blocks.
pub(crate) fn visit_expr(parsed: &Parsed, expr: &Expr, f: &mut impl FnMut(&Expr)) {
    f(expr);

    // **A hole is a call like any other.** Its expression is parsed out of the
    // literal on the way to the emitter, so until this walk existed a call
    // inside `"{io::read_to_string()}"` was invisible here - and `sync` is
    // *inferred* from what a body calls (ADR-288), so the function came out of
    // the ledger claiming it cannot pause. ADR-288 D2 and ADR-010 D1 name that
    // direction the dangerous one.
    for hole in crate::emit::literal_expressions(parsed, expr) {
        visit_expr(parsed, &hole, f);
    }

    match expr {
        // What stands after a `;` is an expression too, and one that can
        // pause. `sync` is *inferred* from what a body calls (ADR-288), so a
        // call this walk does not reach is a function claiming it cannot pause
        // - the fail-open direction ADR-288 D2 names as the dangerous one. A
        // DSL's deferred parameters stand there (ADR-296 D5).
        Expr::Call { func, args, config } => {
            visit_expr(parsed, func, f);
            args.iter().for_each(|a| visit_expr(parsed, a, f));
            config.iter().for_each(|c| visit_expr(parsed, &c.value, f));
        }
        Expr::MethodCall {
            receiver,
            args,
            config,
            ..
        }
        | Expr::SafeMethod {
            receiver,
            args,
            config,
            ..
        } => {
            visit_expr(parsed, receiver, f);
            args.iter().for_each(|a| visit_expr(parsed, a, f));
            config.iter().for_each(|c| visit_expr(parsed, &c.value, f));
        }
        Expr::Binary { lhs, rhs, .. } => {
            visit_expr(parsed, lhs, f);
            visit_expr(parsed, rhs, f);
        }
        // **`throw` holds an expression, and it was not walked** — so every
        // derived column was blind to whatever built the error.
        // `throw wrap(io::read())` left its function looking `sync`, and it is
        // this walk that says otherwise. Found by `keeps`
        // ([ADR-094](../../../docs/specification/adr/adr-094.md) D2) reading a
        // parameter as lent because the `throw` that stores it was invisible;
        // the same hole was `sync`'s, `throws`' and `touches`'.
        Expr::Unary { expr, .. }
        | Expr::Try(expr)
        | Expr::Throw(expr)
        | Expr::Cast { expr, .. } => visit_expr(parsed, expr, f),
        Expr::Field { base, .. } | Expr::SafeField { base, .. } => visit_expr(parsed, base, f),
        Expr::Index { base, index } => {
            visit_expr(parsed, base, f);
            visit_expr(parsed, index, f);
        }
        Expr::Range { start, end, .. } => {
            visit_expr(parsed, start, f);
            visit_expr(parsed, end, f);
        }
        Expr::Tuple(parts) => parts.iter().for_each(|p| visit_expr(parsed, p, f)),
        Expr::Coalesce { value, fallback } => {
            visit_expr(parsed, value, f);
            visit_expr(parsed, fallback, f);
        }
        Expr::TryCatch { expr, .. } => visit_expr(parsed, expr, f),
        Expr::If { cond, .. } => visit_expr(parsed, cond, f),
        // **An arm is an expression of this function** (issue #171
        // issue #171): `R::A => slow()` is a call, and only an arm that is a block
        // reached `visit_expr_blocks`. `pick` came out of the ledger `sync`
        // while its lowering awaited `slow()`, which `rustc` refused - the
        // fail-open direction ADR-288 D2 names, for `throws`, `touches` and
        // `keeps` as much as for `sync`. A guard runs too.
        Expr::Match { value, arms } => {
            visit_expr(parsed, value, f);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    visit_expr(parsed, guard, f);
                }
                visit_expr(parsed, &arm.body, f);
            }
        }
        Expr::StructLit { fields, .. } => fields
            .iter()
            .filter_map(|field| field.value.as_ref())
            .for_each(|value| visit_expr(parsed, value, f)),
        // **And the four other places a value is computed** that the walk
        // passed by, for the same reason: a list's items, the copy a `with`
        // is made of and its fields, what a `return` hands back where it is an
        // expression (ADR-276), and what each arm of a `select` starts.
        Expr::ListLit { items, .. } => items.iter().for_each(|item| visit_expr(parsed, item, f)),
        Expr::With { base, fields, .. } => {
            visit_expr(parsed, base, f);
            fields
                .iter()
                .filter_map(|field| field.value.as_ref())
                .for_each(|value| visit_expr(parsed, value, f));
        }
        Expr::Return(value) => {
            if let Some(value) = &**value {
                visit_expr(parsed, value, f);
            }
        }
        Expr::Select(arms) => arms
            .iter()
            .for_each(|arm| visit_expr(parsed, &arm.value, f)),
        _ => {}
    }
}
