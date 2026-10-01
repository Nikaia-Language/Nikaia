// crates/nikaia/src/contracts/tether.rs
//
// Which of Part I 6.6's three states each view in a signature is in
// ([ADR-008](../../../../docs/specification/adr/adr-008.md) D2, D7).
//
// ## What this is, and what it deliberately is not
//
// D2 gives every view one of three states and says the compiler picks the least
// one that makes the program valid:
//
//     Borrowed ⊑ Tethered ⊑ Owned
//     Transient = borrow. Escaping = tether. Copying = yours to ask for.
//
// **Only the first of those is built as a representation.** Borrowed is the
// language below's own lifetime and costs nothing; Owned is `.to_owned()`,
// written by the program; and **Tethered is not built at all** — where a value
// would tether, the program is refused today (`NK2302`, or the backend on the
// Nikaia line for a view of a local that escapes). So this file computes the
// state and **changes no lowering**. It writes a column, which is D7, and the
// column is what a later change reads when the representation exists.
//
// That order is deliberate: an analysis whose answers nothing depends on can be
// held against the whole corpus and read, and being wrong costs a wrong line in
// a file rather than a wrong program.
//
// ## Owned is never this file's word
//
// D5: *promotion to `Owned` is never automatic*. A `.to_owned()` hands back a
// `String`, which is not a view at all — so no **view** position is ever Owned,
// and the lattice this file solves over is the two states below it. The top of
// the lattice exists for the program to reach, not for the analysis to assign.
//
// ## Which way it errs
//
// **Towards Tethered**, which is D7's own polarity for the case it names: a
// state barrier *widens* to Tethered, *"the widest representation, never to
// Owned"*. A position this file cannot decide is therefore Tethered, and the
// cost of being wrong that way is a representation wider than it had to be —
// never a program that does the wrong thing. The other direction would tell a
// later reader that a value borrows when it escapes, which is the use-after-free
// the state exists to prevent.
//
// ## What is not here, and why
//
// **D7's state barrier is not applied.** *Crossing a `dyn` boundary or a
// published non-generic API is a state barrier* is a consequence of there being
// three representations and one ABI to choose; with one representation there is
// no barrier to cross. It belongs with the layouts rather than with the
// analysis, and marking every `pub` signature Tethered today would fill the
// column with a state nothing has.
//
// **The buffer table is not here either** (D4). Its shape is a fact about a
// container's representation, and this file writes no representation.

use crate::contracts::LedgerOps;
pub use nikaia_std::tools::ty::{Held, State};
use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Item, Type};
use crate::emit::{borrowing_structs, holds_view};
use crate::parser::Parsed;

use super::{INPUT, Ledger};

/// The position a function's result occupies in the column.
///
/// The same spelling `sharing` uses for the same thing, because they are the
/// same position read by two analyses.
pub const RESULT: &str = "<result>";

/// What stays Rust of a [`Held`] (ADR-257 step (b)): reading one back.
pub trait HeldOps: Sized {
    fn parse(text: &str) -> Option<Self>;
}

impl HeldOps for Held {
    fn parse(text: &str) -> Option<Held> {
        nikaia_std::tools::ledger::held_of(text)
    }
}

/// Give every entry of this package the states its views solved to (D7).
///
/// After the other inferences and reading none of them: what it asks is about
/// the **shape** of a signature and about which buffer a returned view came
/// from, and no column above answers either.
pub fn infer(ledger: &mut Ledger, units: &[&Parsed], library: &Ledger) {
    // Which struct or enum holds a view, per unit: a `Symbol` belongs to the
    // parse that interned it, so the set cannot be shared between files.
    let mut solved: BTreeMap<String, Vec<Held>> = BTreeMap::new();
    // A type whose instances a function hands back tethered. `D3`: all view
    // fields of an instance share one state, so a type has one answer.
    let mut tethering: BTreeSet<String> = BTreeSet::new();
    // **The package rather than the unit**, and only the grammar arm below
    // reads it: a rule's result names a type of this package and the grammar it
    // stands in is emitted into one file, so the unit is the wrong horizon for
    // that one question and the right one for every other here.
    let package = declared_in(units);

    for parsed in units.iter().copied() {
        // By **name** rather than by `Symbol`, because a receiver is asked
        // about through its `impl`'s target, which is text here.
        let borrowing: BTreeSet<String> = borrowing_structs(parsed)
            .into_iter()
            .map(|s| parsed.text(s).to_string())
            .collect();
        let carries = |ty: &Type| carries_a_view(parsed, ty, &borrowing);
        let by_name = |name: &str| borrowing.contains(name);
        for item in &parsed.program.items {
            match &item.node {
                Item::Fn { .. } => {
                    if let Some((key, held, tethers)) = of(
                        parsed, &item.node, None, &carries, &by_name, ledger, library,
                    ) {
                        tethering.extend(tethers);
                        solved.insert(key, held);
                    }
                }
                Item::Impl {
                    target, methods, ..
                } => {
                    let target = parsed.text(target.name).to_string();
                    for method in methods {
                        if let Some((key, held, tethers)) = of(
                            parsed,
                            &method.node,
                            Some(&target),
                            &carries,
                            &by_name,
                            ledger,
                            library,
                        ) {
                            tethering.extend(tethers);
                            solved.insert(key, held);
                        }
                    }
                }
                // **A `pub` rule is an entry too**
                // ([ADR-082](../../../../docs/specification/adr/adr-082.md)
                // D1), and it is the one entry whose buffer is not a question:
                // a parse hands back views **into the text it was given**, and
                // that text is the caller's `input`. So the same sentence `of`
                // reaches by asking whether anything is there to borrow from,
                // read straight off the shape of a grammar: where the result
                // carries a view, the entry holds the input and the result
                // borrows it.
                //
                // **An action block is not walked for this**, for the reason
                // `touches` gives one column over: a rule's pattern names other
                // rules and their actions run with it. It does not need to be —
                // what a parse can hand back is the rule's **declared result**,
                // and an action that built something else would not type-check.
                Item::Grammar(def) => {
                    let named = parsed.text(def.name).to_string();
                    for rule in def.rules.iter().filter(|r| r.is_public) {
                        if !a_parse_that_views(parsed, rule.ret_type.as_ref(), &package) {
                            continue;
                        }
                        solved.insert(
                            format!("{named}::{}", parsed.text(rule.name)),
                            vec![
                                Held {
                                    position: INPUT.to_string(),
                                    state: State::Borrowed,
                                },
                                Held {
                                    position: RESULT.to_string(),
                                    state: State::Borrowed,
                                },
                            ],
                        );
                    }
                }
                _ => {}
            }
        }
    }

    for (key, held) in solved {
        if let Some(contract) = ledger.functions.get_mut(&key) {
            contract.views = held;
        }
    }
    // **D7's second half is not written**, and that is the same line the module
    // header draws: *per struct: the solved state and the buffer-table shape*
    // is a fact about a **representation**, and the buffer table is named in the
    // same breath. A type already records which of its fields hold a view
    // (`tethered`); what state its instances take belongs with the layouts.
    let _ = tethering;
    // **Which positions really leave, and so take a keep from the caller**
    // (ADR-209 D6), over the whole package, to a fixpoint.
    super::keep::infer(ledger, units, library);
}

/// One function's positions, and the type names its result tethers.
fn of(
    parsed: &Parsed,
    item: &Item,
    target: Option<&str>,
    carries: &impl Fn(&Type) -> bool,
    carries_by_name: &impl Fn(&str) -> bool,
    here: &Ledger,
    library: &Ledger,
) -> Option<(String, Vec<Held>, Vec<String>)> {
    let Item::Fn {
        name,
        receiver,
        args,
        ret_type,
        body,
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

    let mut held = Vec::new();
    // **A parameter is lent for the call**, so the buffer is the caller's and
    // outlives every use inside the body. Whether the body *keeps* it past the
    // call is `NK2302`'s question and a refusal rather than a state.
    // A receiver carries whatever its type does, and that is the `impl`'s
    // target — a view carrier exactly when the type is.
    if receiver.is_some() && target.is_some_and(carries_by_name) {
        held.push(Held {
            position: "self".to_string(),
            state: State::Borrowed,
        });
    }
    for arg in args {
        if carries(&arg.ty) {
            held.push(Held {
                position: parsed.text(arg.name).to_string(),
                state: State::Borrowed,
            });
        }
    }

    let mut tethers = Vec::new();
    if let Some(ret) = ret_type.as_ref().filter(|t| carries(t)) {
        // **Something to borrow from** is what decides it, and it is the same
        // question [ADR-008](../../../../docs/specification/adr/adr-008.md) D9
        // asks about the lifetime: where a receiver or a parameter carries a
        // view, the buffer is the caller's and the result borrows it. Where
        // nothing does, the view can only come from a local buffer or from
        // something that outlives the program.
        let borrows_from_a_caller = receiver.is_some() || args.iter().any(|a| carries(&a.ty));
        let buffer = match borrows_from_a_caller {
            true => Buffer::None,
            false => owns_a_buffer(parsed, body, here, library),
        };
        // **Borrowed here, and `keep::infer` says otherwise where a view
        // really leaves** ([ADR-209](../../../../docs/specification/adr/adr-209.md)
        // D6). Owning a buffer is not escaping it: a body may read a file and
        // hand back a view of something else. The column is one fact with one
        // author, and the author is the walk that follows views.
        let state = State::Borrowed;
        let _ = buffer;
        if state == State::Tethered {
            tethers.extend(names_in(parsed, ret));
        }
        held.push(Held {
            position: RESULT.to_string(),
            state,
        });
    }

    (!held.is_empty()).then_some((key, held, tethers))
}

/// Whether this body **owns a buffer** a returned view could point into.
///
/// That is the whole of the question, once the caller's buffers are out of the
/// picture: a view handed back by a function with no view among its parameters
/// points either at something that outlives the program — `fn name() -> &str {
/// "Ada" }`, D9's own example — or at a buffer this body made. The second is
/// the escape, and the first is Borrowed.
///
/// **What counts as making one** is read off the ledger rather than from a list
/// of names: a `let` whose initialiser is a call whose **result** is owned text
/// or bytes. `fs::read_to_string` hands back a `String`, `fs::read` a
/// `Vec[u8]`, `fs::map` a `Mapped`. Beside that, `to_owned` and `to_string` by
/// name, which is the same kind of fact about the language below that
/// `is_length` records for `len`: all four entries of either name hand back
/// owned text.
///
/// **Unsure is *owns*,** which is the module header's polarity: an unresolved
/// call is a buffer this walk could not rule out, and the wider state is the
/// safe one.
fn owns_a_buffer(
    parsed: &Parsed,
    body: &crate::ast::Block,
    own: &Ledger,
    library: &Ledger,
) -> Buffer {
    let mut found = Buffer::None;
    bindings(body, &mut |ty, value| {
        let made = match makes_a_buffer(parsed, value, own, library) {
            Buffer::Named(name) => Buffer::Named(name),
            _ if declares_text(parsed, ty) => Buffer::Named("String".to_string()),
            other => other,
        };
        found = found.clone().or(made);
    });
    found
}

/// **A name declared `String` holds text of this frame's own**, whatever its
/// initialiser was: `let held: String = "a b"` builds it
/// ([ADR-207](../../../docs/specification/adr/adr-207.md) D2), and a name
/// handed over is moved here (ADR-216 D4).
pub(crate) fn declares_text(parsed: &Parsed, ty: Option<&crate::ast::Type>) -> bool {
    ty.is_some_and(|ty| {
        parsed.text(ty.name) == "String" && ty.generics.is_empty() && !ty.is_view && !ty.is_nullable
    })
}

/// Whether a body makes a buffer, and whether this walk could **name** it.
///
/// The difference is what a refusal may be built on. A named buffer is a fact:
/// this body made a `String` and hands back a view of it. `Unknown` is a call
/// no ledger describes, which the **state** errs towards Tethered for — and a
/// refusal on that would refuse a correct program, which is the one thing this
/// compiler may never do
/// ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)). So the
/// column reads the two alike and a refusal reads only the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Buffer {
    /// Nothing in this body owns text or bytes.
    None,
    /// This type, made here: `String`, `Bytes`, `Mapped`, a run of `u8`.
    Named(String),
    /// A call this walk could not resolve, which may be one.
    Unknown,
}

impl Buffer {
    /// The join, keeping the most that is known: a named buffer beats a doubt,
    /// because the doubt adds nothing once one is in hand.
    fn or(self, other: Buffer) -> Buffer {
        match (self, other) {
            (Buffer::Named(name), _) | (_, Buffer::Named(name)) => Buffer::Named(name),
            (Buffer::Unknown, _) | (_, Buffer::Unknown) => Buffer::Unknown,
            _ => Buffer::None,
        }
    }
}

/// Every `let`'s declared type and initialiser in a body, the blocks inside it
/// included.
fn bindings(
    block: &crate::ast::Block,
    f: &mut impl FnMut(Option<&crate::ast::Type>, &crate::ast::Expr),
) {
    use crate::ast::Stmt;
    for stmt in &block.stmts {
        if let Stmt::Let { value, ty, .. } = &stmt.node {
            f(ty.as_ref(), value);
        }
        super::sync::visit_stmt_blocks(&stmt.node, &mut |inner| bindings(inner, f));
    }
}

/// Whether this expression hands back a buffer of its own.
pub(crate) fn makes_a_buffer(
    parsed: &Parsed,
    expr: &crate::ast::Expr,
    own: &Ledger,
    library: &Ledger,
) -> Buffer {
    use crate::ast::Expr;
    match expr {
        // `text.to_owned()`: owned text by the name, which every entry of it
        // agrees on.
        //
        // **`to_string` is the text form of a value** (ADR-216 D4): a number's
        // is text of its own and text's is the text itself - a view stays the
        // view it was, a literal the literal. Without types this walk cannot
        // tell the two apart, so beside the literals it says *unsure*, which
        // the state errs on and no refusal stands on.
        Expr::MethodCall {
            method, receiver, ..
        } => match parsed.text(*method) {
            "to_owned" => Buffer::Named("String".to_string()),
            "to_string" => match receiver.as_ref() {
                Expr::LitStr { .. } => Buffer::None,
                Expr::LitInterpolated { .. } | Expr::LitInt { .. } | Expr::LitFloat(_) => {
                    Buffer::Named("String".to_string())
                }
                _ => Buffer::Unknown,
            },
            // A copy of a literal is text of its own (ADR-216 D2); a copy of
            // a name is `keep`'s question, which knows the name's type.
            "clone"
                if matches!(
                    receiver.as_ref(),
                    Expr::LitStr { .. } | Expr::LitInterpolated { .. }
                ) =>
            {
                Buffer::Named("String".to_string())
            }
            _ => Buffer::None,
        },
        Expr::Call { func, .. } => {
            let name = match func.as_ref() {
                Expr::Variable(name) => parsed.text(*name).to_string(),
                Expr::Path(segments) => parsed.unaliased(
                    &segments
                        .iter()
                        .map(|s| parsed.text(*s))
                        .collect::<Vec<_>>()
                        .join("::"),
                ),
                _ => return Buffer::None,
            };
            let Some((_, contract)) = own
                .lookup(&name)
                .or_else(|| library.lookup(&name))
                .or_else(|| own.lookup(&format!("{name}::new")))
                .or_else(|| library.lookup(&format!("{name}::new")))
            else {
                // A call no ledger describes is a buffer this walk cannot rule
                // out, which is the safe direction for the **state** and not
                // one a refusal may stand on.
                return Buffer::Unknown;
            };
            match contract
                .signature
                .as_ref()
                .and_then(|s| s.result.as_ref())
                .filter(|ty| is_a_buffer(ty))
            {
                Some(ty) => Buffer::Named(ty.to_string()),
                None => Buffer::None,
            }
        }
        // `catch` hands back whichever half ran, so either may be the buffer.
        Expr::TryCatch { expr, .. } => makes_a_buffer(parsed, expr, own, library),
        _ => Buffer::None,
    }
}

/// Whether a type is owned text or bytes — something a view can point **into**.
///
/// `String`, `Bytes` and a mapped file are what a `&str` points into; a run of
/// `u8` is what a `&[u8]` does. A `Vec[Entry]` is not a buffer: nothing views
/// it, its elements are values.
///
/// **The one shape this does not decide** is a buffer built element by element
/// into a `Vec` whose element type the ledger does not name — `Vec()` hands
/// back `Vec[?]`, and reading that as a buffer would make every empty list in
/// every body one. No program in the tree builds a byte buffer that way and
/// then hands back a view into it; where one does, the backend is still what
/// refuses it, because nothing reads this column yet.
fn is_a_buffer(ty: &super::ty::Ty) -> bool {
    let super::ty::Ty::Named { name, args, .. } = ty else {
        return false;
    };
    match super::ty::base(name) {
        "String" | "Bytes" | "Mapped" => true,
        "Vec" | "List" => matches!(
            args.first(),
            Some(super::ty::Ty::Named { name, .. }) if super::ty::base(name) == "u8"
        ),
        _ => false,
    }
}

/// The type names a written type mentions, so that a result that tethers can
/// say which types it tethers.
fn names_in(parsed: &Parsed, ty: &Type) -> Vec<String> {
    let mut out = vec![parsed.text(ty.name).to_string()];
    for argument in &ty.generics {
        out.extend(names_in(parsed, argument));
    }
    out
}

/// What a **package's** declarations say about the types a parse can hand back.
///
/// By name and not by `Symbol`, because a `Symbol` belongs to the parse that
/// interned it and the question crosses files: a grammar is emitted into the
/// file it stands in ([ADR-030](../../../../docs/specification/adr/adr-030.md)
/// §7), and the record it yields is a declaration of the *package*.
pub(super) struct Declared {
    /// Every struct or enum of this package that holds a view.
    borrowing: BTreeSet<String>,
    /// Every type name this package declares, whether it holds one or not.
    named: BTreeSet<String>,
}

/// Read both sets off the units in one pass.
pub(super) fn declared_in(units: &[&Parsed]) -> Declared {
    let mut borrowing = BTreeSet::new();
    let mut named = BTreeSet::new();
    for parsed in units.iter().copied() {
        for symbol in borrowing_structs(parsed) {
            borrowing.insert(parsed.text(symbol).to_string());
        }
        for item in &parsed.program.items {
            match &item.node {
                Item::Struct { name, .. } | Item::Enum { name, .. } => {
                    named.insert(parsed.text(*name).to_string());
                }
                _ => {}
            }
        }
    }
    Declared { borrowing, named }
}

/// Whether a grammar entry's result **may point into the text it parsed**.
///
/// One question with two readers — this file's `views` column and
/// [`super::keeps`]' — and one polarity for both, which is the second's:
/// **a result this walk cannot see into may view**. `keeps` fails closed by
/// its own header, and a wrongly open answer there would lend the text to a
/// parser that holds views into it; `views` errs towards the wider reading by
/// this file's, and being wrong that way writes a position into a column
/// nothing reads yet.
///
/// Three answers, in order:
///
/// * The result **is** a view or names a type of this package that holds one —
///   `Vec[Entry]` where `Entry` has a `ref String` field. That is a fact.
/// * Every name in it is one this package declares and `borrowing_structs` did
///   not name, or one [Part I 2.2](../../../../docs/specification/10-nikaia-light.md)
///   offers, or a container that holds what it is given. Then there is no room
///   for a view, and the entry keeps nothing.
/// * Anything else is a name this walk did not read a declaration for, and a
///   type it cannot see into is one it cannot rule a view out of.
pub(super) fn a_parse_that_views(parsed: &Parsed, ret: Option<&Type>, package: &Declared) -> bool {
    let Some(ty) = ret else {
        // A rule with no declared result hands back nothing, and nothing holds
        // no view.
        return false;
    };
    if ty.is_view || package.borrowing.contains(parsed.text(ty.name)) {
        return true;
    }
    if ty
        .generics
        .iter()
        .any(|g| a_parse_that_views(parsed, Some(g), package))
    {
        return true;
    }
    let name = parsed.text(ty.name);
    !package.named.contains(name) && !a_type_with_no_room_for_a_view(name)
}

/// Whether a name is one the language below owns outright, so that a value of
/// it holds no view of its own.
///
/// Two kinds, and the second is why this is not just
/// [Part I 2.2](../../../../docs/specification/10-nikaia-light.md)'s list: a
/// container holds exactly what it is given, and the walk above has already
/// asked its arguments. `Vec[Entry]` is a view carrier when `Entry` is and
/// nothing when it is not, and the name `Vec` decides neither.
fn a_type_with_no_room_for_a_view(name: &str) -> bool {
    matches!(
        super::ty::base(name),
        "i8" | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
            | "bool"
            | "char"
            | "String"
            | "Duration"
            | "Instant"
            // The containers, whose arguments the walk above already read.
            | "Vec"
            | "List"
            | "Array"
            | "Option"
            | "Map"
            | "Set"
            | "Shared"
            | "SharedMut"
            | "Locked"
    )
}

/// Whether a written type **carries** a view: it is one, it names a type that
/// holds one, or one of its arguments does.
fn carries_a_view(parsed: &Parsed, ty: &Type, borrowing: &BTreeSet<String>) -> bool {
    holds_view(ty)
        || borrowing.contains(parsed.text(ty.name))
        || ty
            .generics
            .iter()
            .any(|g| carries_a_view(parsed, g, borrowing))
}

/// `--tethers`: what the analysis solved, per function
/// ([ADR-008](../../../../docs/specification/adr/adr-008.md) D6).
///
/// *The inverse tool is inspection, not assertion*, and it is the half of D6
/// that survived: the assertion beside it is gone
/// ([ADR-201](../../../../docs/specification/adr/adr-201.md) D1) and this shows
/// what was chosen without being asked. Read off the ledger the build produced
/// rather than computed again here, so that what a person reads is what the file
/// records.
pub fn report(parsed: &Parsed, ledger: &Ledger) -> String {
    // **This file's own entries**, because a report is about one file and the
    // ledger is the package's. A key names a function of this unit exactly when
    // this unit's own inference produced it.
    let here = Ledger::infer(parsed);
    let mut lines: Vec<String> = Vec::new();
    // **What a `String` field or result is below** (ADR-222 D5), where it is
    // not text of its own.
    if !parsed.text_tiers.is_empty() {
        lines.push("text (declared `String`):\n".to_string());
        for line in &parsed.text_tiers {
            lines.push(format!("    {line}\n"));
        }
    }
    for (key, contract) in &ledger.functions {
        if contract.views.is_empty() || !here.functions.contains_key(key) {
            continue;
        }
        lines.push(format!("{key}:\n"));
        for held in &contract.views {
            lines.push(format!(
                "    {:<9} `{}`\n",
                held.state.as_str(),
                held.position
            ));
        }
    }
    // **And where each buffer lives** (ADR-209 D6): the keep plan, per
    // function, in the words a reader asks the question in.
    let library = crate::contracts::std_ledger();
    for plan in super::keep::plans(parsed, ledger, library) {
        let mut said = Vec::new();
        for (at, keep) in &plan.puts {
            // By the name the buffer is bound to, which is what a reader looks
            // for in the source.
            let name = plan
                .escapes
                .iter()
                .find_map(|(source, _)| match source {
                    super::keep::Source::Buffer { at: a, name, .. } if a == at => {
                        Some(format!("`{name}`"))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| "a buffer".to_string());
            said.push(format!(
                "    {name} lives {}\n",
                super::keep::describe(*keep)
            ));
        }
        for ((_, callee), keep) in &plan.calls {
            said.push(format!(
                "    what `{callee}` reads lives {}\n",
                super::keep::describe(*keep)
            ));
        }
        for keeper in &plan.element_keepers {
            // ADR-221: a struct of views carries a handle on each buffer it
            // points into, and the report says how many that is.
            let line = match plan.struct_keepers.contains(keeper) {
                true => {
                    let width = plan.widths.get(keeper).copied().unwrap_or(1);
                    let handles = match width {
                        1 => "a handle on the buffer it points into".to_string(),
                        n => format!("a handle on each of the {n} buffers it points into"),
                    };
                    format!("    `{keeper}` holds each struct with {handles}\n")
                }
                false => format!("    `{keeper}` holds each view with its own handle\n"),
            };
            said.push(line);
        }
        if said.is_empty() {
            continue;
        }
        lines.push(format!("{} (keeps):\n", plan.key));
        lines.extend(said);
    }
    match lines.is_empty() {
        // "here" and not "in this program", for the reason `sharing`'s report
        // gives: a report is about one file.
        true => "no view in a signature here, so there is no state to solve.\n".to_string(),
        false => lines.concat(),
    }
}

// ---------------------------------------------------------------------------
// The refusal: where the tether would be needed and is not built
// ---------------------------------------------------------------------------

/// A view handed back that points into a buffer the body **owns** (`NK2303`).
///
/// This is the one shape Part I 6.6's `Tethered` exists for, and
/// [ADR-156](../../../../docs/specification/adr/adr-156.md) D4 is what to do
/// about it while the state is not built: refuse on the Nikaia line, naming the
/// buffer and the mechanism, rather than lower a function whose result outlives
/// the buffer it points into and let `rustc` speak about the generated file
/// ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
///
/// **It stands on a named buffer and never on a doubt.** [`Buffer::Unknown`] is
/// a call no ledger describes; the *column* errs towards Tethered for it,
/// because a wide state costs a wide representation. A refusal cannot err that
/// way — refusing a correct program is the worse of the two mistakes
/// ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)) — so this
/// walk asks for the buffer **by name** and for the returned expression to be
/// derived from it. Where either is missing the program is lowered exactly as
/// it was before this check existed.
pub fn check(parsed: &Parsed, own: &Ledger, library: &Ledger) -> Vec<crate::check::Finding> {
    // **Where a view outlives its buffer is the keep plan's answer**
    // ([ADR-209](../../../../docs/specification/adr/adr-209.md)): it follows
    // views through `push`, `for`, fields and calls, which the walk that stood
    // here did not, and it refuses only what it cannot lower or what no
    // declaration permits.
    super::keep::plans(parsed, own, library)
        .into_iter()
        .flat_map(|plan| plan.refusals)
        .collect()
}
