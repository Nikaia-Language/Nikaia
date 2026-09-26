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
// polarity as `sync` ([ADR-027](../../../docs/specification/adr/adr-027.md)) and
// as [ADR-010](../../../docs/specification/adr/adr-010.md) D1, pointing the
// other way because the claim points the other way.
//
// **Nothing reads this column yet**, which is ADR-094 §5's first step on
// purpose: the answer can be diffed against the corpus before one call site
// changes.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Block, Expr, Item, Stmt};
use crate::parser::Parsed;

use super::{INPUT, Ledger};

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
    let Some(signature) = contract.signature.as_ref() else {
        return false;
    };
    let Some((name, ty)) = signature.params.get(at) else {
        return false;
    };
    // **A method's arguments are not lent yet**, and the reason is the one
    // `touches` gives for asking a weaker question: which entry `acc.record(m)`
    // goes to is the type checker's answer and the emitter has none
    // ([ADR-028](../../../docs/specification/adr/adr-028.md)). The declaration
    // is written off this column and the call is not, so a call the checker
    // does not walk — a grammar action's fold lambda, say — would pass a value
    // into a `&T` parameter and `rustc` would say so about a file nobody wrote.
    //
    // A free function has neither half of that problem: the emitter resolves
    // its callee by name, exactly as it resolves everything else.
    // **A `mut` parameter is a third state and not this one**
    // ([ADR-094](../../../docs/specification/adr/adr-094.md) D3): it lowers to
    // `&mut T`, written off the declaration rather than off this column, and
    // both answering would put two references on one parameter.
    !signature.mutable.contains(name)
        && !signature.takes_a_receiver()
        && name != "self"
        && (ty.is_a_view() || (moves(ty) && !contract.keeps.contains(name)))
}

/// Whether a value of this type is **moved** when it is handed on, rather than
/// copied — and whether this compiler can say so at all.
///
/// The same question `NK2101` asks about a task
/// ([ADR-040](../../../docs/specification/adr/adr-040.md) D1), asked one
/// position over and answered more narrowly in one place: `Unknown`, a type
/// variable and a function type answer **no** here, because what hangs on it is
/// a reference this compiler would *write* rather than a refusal it would
/// withhold. A refusal on a guess is refusing a correct program; a reference on
/// a guess is a program that does not compile.
pub fn moves(ty: &super::ty::Ty) -> bool {
    use super::ty::Ty;
    match ty {
        // **An array copies as its elements do**
        // ([ADR-152](../../../docs/specification/adr/adr-152.md) D2): `N`
        // elements inline and nothing allocated, so `Array[i64, 3]` is copied
        // the way a tuple of three `i64` is and `Array[String, 3]` is moved the
        // way one of three `String` is. The count among the arguments answers
        // **no** on its own, which is what makes `any` right here.
        Ty::Named { name, args, .. } if super::ty::base(name) == super::ty::ARRAY => {
            args.iter().any(moves)
        }
        Ty::Named { name, view, .. } => {
            !view
                && !matches!(
                    super::ty::base(name),
                    "i8" | "i16"
                        | "i32"
                        | "i64"
                        // **The two machine-width names are copied too**, and
                        // their absence here was a defect the C boundary found
                        // ([ADR-147](../../../docs/specification/adr/adr-147.md)
                        // D2): a declaration writing `count: usize` got a `&`
                        // in front of its argument, because a type this list
                        // does not name is one that **moves**. No program could
                        // write one before - a length is an `i64` and the
                        // machine-width type left the surface
                        // ([ADR-048](../../../docs/specification/adr/adr-048.md)
                        // D1) - so the first declaration to name one is the
                        // first program to meet it.
                        | "isize"
                        | "usize"
                        | "u8"
                        | "u16"
                        | "u32"
                        | "u64"
                        | "f32"
                        | "f64"
                        | "bool"
                        | "char"
                        // **A span of time is a number of ticks**
                        // ([ADR-150](../../../docs/specification/adr/adr-150.md)
                        // D1), so it copies - and a `Duration` missing from
                        // this list was the `usize` defect one type over: the
                        // first program to write `sleep(50.millis())` got a `&`
                        // in front of its argument, because a type this list
                        // does not name is one that **moves**.
                        | "Duration"
                        // A hull is a handle: handing one on duplicates the
                        // count rather than taking the value away
                        // (ADR-040 D1, D5).
                        | "Shared"
                        | "SharedMut"
                        | "Locked"
                )
        }
        // **A function value moves**, which is what
        // [ADR-102](../../../docs/specification/adr/adr-102.md) D1's parameter
        // needed: it lowers to `impl AsyncFn(A) -> R`, an opaque type with no
        // `Copy`, so a `listen` that hands its handler to an `answer` inside a
        // loop hands it away the first time round. `rustc` said so about a file
        // nobody wrote - *use of moved value: `handler`* - which is [Part III
        // C.1](../../../docs/specification/30-nikaia-tooling.md).
        //
        // It is the answer for Part I 5.4 C's **immediate** context read off the
        // body rather than off a word: a parameter the body only calls or passes
        // on is not in `keeps`, so `lends` says to write the `&` - and `&F` is a
        // function too, which is why one `&` is all it takes.
        Ty::Fn { .. } => true,
        // A nullable of data is still data: `Option<String>` moves.
        Ty::Nullable(inner) => moves(inner),
        // A tuple moves where any part does; one of copied parts is copied.
        Ty::Tuple(parts) => parts.iter().any(moves),
        _ => false,
    }
}

/// What one function's body does with each of its parameters.
#[derive(Debug, Default)]
struct Uses {
    /// Kept on this body's own evidence — stored, assigned, returned by value,
    /// given to a task, or handed to a callee nothing describes.
    kept: BTreeSet<String>,
    /// Handed to a callee: this parameter, the callee as the ledger names it,
    /// and which of that callee's parameters it landed in. Whether it is kept
    /// is that callee's answer, which the fixpoint below waits for.
    passed: BTreeSet<(String, String, usize)>,
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
                // text** ([ADR-082](../../../docs/specification/adr/adr-082.md)
                // D1). Whether it keeps that text is not a question about an
                // action block at all: a parse keeps its input exactly when
                // what it hands back holds a view **into** the input, which is
                // [ADR-008](../../../docs/specification/adr/adr-008.md)'s
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
                        let mut uses = Uses::default();
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

    let mut kept: BTreeMap<String, BTreeSet<String>> = graph
        .iter()
        .map(|(name, uses)| (name.clone(), uses.kept.clone()))
        .collect();

    loop {
        let mut changed = false;
        for (name, uses) in &graph {
            for (parameter, callee, at) in &uses.passed {
                if kept[name].contains(parameter) {
                    continue;
                }
                if keeps_its(callee, *at, &kept, ledger, library) {
                    kept.get_mut(name)
                        .expect("every caller is in the map")
                        .insert(parameter.clone());
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    for (name, parameters) in kept {
        if let Some(contract) = ledger.functions.get_mut(&name) {
            contract.keeps = parameters.into_iter().collect();
        }
    }
}

/// Whether `callee` keeps whatever is passed in position `at`.
///
/// This package's own answer comes from the fixpoint in progress; anybody
/// else's comes from their ledger, where **an absent `keeps` on a present entry
/// means it keeps nothing** — `std.contracts`' own convention for `sync`, said
/// once more for a second column. An entry that is *absent* is unknown, and
/// unknown keeps.
fn keeps_its(
    callee: &str,
    at: usize,
    settling: &BTreeMap<String, BTreeSet<String>>,
    ledger: &Ledger,
    library: &Ledger,
) -> bool {
    let named = |contract: &super::FnContract| {
        contract
            .signature
            .as_ref()
            .and_then(|s| s.params.get(at))
            .map(|(name, _)| name.clone())
    };

    if let Some(parameters) = settling.get(callee) {
        let Some(contract) = ledger.functions.get(callee) else {
            return true;
        };
        // A position the signature does not have is a call this walk read
        // wrongly, and reading it wrongly is not a licence to assume.
        return match named(contract) {
            Some(parameter) => parameters.contains(&parameter),
            None => true,
        };
    }

    for source in [ledger, library] {
        if let Some(contract) = source.functions.get(callee) {
            return match named(contract) {
                Some(parameter) => contract.keeps.contains(&parameter),
                None => true,
            };
        }
    }
    true
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
        return Some((key, Uses::default()));
    }

    // **A result that is a view keeps nothing by returning.** `-> &str` hands
    // back a view of a parameter, which `borrows` already records; it is
    // `-> String` that moves the value out of the call.
    // **Text both kinds flow into is not a view to hand back**
    // ([ADR-226](../../../docs/specification/adr/adr-226.md) D2): an
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

    let mut uses = Uses::default();
    // **Whether this body's method calls resolved at all.** A receiver whose
    // method nothing describes may be a `self`-by-value method, and a parameter
    // handed to one is moved out of — so the fail-closed answer for a *whole
    // body* is the one the type checker already computed
    // ([ADR-028](../../../docs/specification/adr/adr-028.md)).
    let unresolved = resolved.get(&key).is_some_and(|m| m.unresolved);
    let mut walk = Walk {
        parsed,
        parameters: &parameters,
        fields: &fields,
        returns_a_view,
        unresolved,
        ledger,
        library,
        uses: &mut uses,
        aliases: BTreeMap::new(),
    };
    walk.block(body);
    // **The value a body ends in leaves the call**, as a `return` does: `fn
    // f(name: String) -> String { name }` keeps `name`. It used to be read as
    // lent, and the declaration came out `&String` with the body handing the
    // loan back as a `String` - `rustc`'s *mismatched types* about a file
    // nobody wrote (Part III C.1), for the shortest program that keeps.
    if ret_type.is_some()
        && !returns_a_view
        && let Some(Stmt::Expr(value)) = body.stmts.last().map(|s| &s.node)
    {
        walk.hand_over(value);
    }
    Some((key, uses))
}

struct Walk<'a> {
    parsed: &'a Parsed,
    parameters: &'a BTreeSet<String>,
    /// **What each parameter's fields are**, for the one shape a bare name does
    /// not cover: `return answer.text` takes a piece out of `answer`.
    ///
    /// Empty for a parameter whose type this file does not declare, which
    /// [`Walk::hand_over`] reads as *unknown*.
    fields: &'a BTreeMap<String, BTreeMap<String, super::ty::Ty>>,
    returns_a_view: bool,
    /// Whether any method call in this body went to an entry no ledger has.
    unresolved: bool,
    ledger: &'a Ledger,
    library: &'a Ledger,
    uses: &'a mut Uses,
    /// **The names a `let` bound to what may be a parameter**, and which ones:
    /// `let s = name` and then `return s` hands `name` out of the call. A
    /// `let` still keeps nothing by itself; it is the second name leaving that
    /// does, and this is how the walk knows the second name is the first.
    ///
    /// Per body and not per scope, so a shadowing `let` counts both: that can
    /// only keep more, which is this column's safe direction.
    aliases: BTreeMap<String, BTreeSet<String>>,
}

impl Walk<'_> {
    fn block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.stmt(&stmt.node);
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            // **An assignment keeps, whatever the target is.** `self.name = x`,
            // `row.total = x` and `xs[i] = x` all put the value somewhere this
            // call does not end.
            Stmt::Assign { value, .. } => self.hand_over(value),
            Stmt::Return(Some(value)) if !self.returns_a_view => self.hand_over(value),
            Stmt::Let { names, value, .. } => {
                let reached = self.reached(value);
                if !reached.is_empty() {
                    for name in names {
                        let name = self.parsed.text(*name).to_string();
                        self.aliases
                            .entry(name)
                            .or_default()
                            .extend(reached.iter().cloned());
                    }
                }
            }
            // **A `let` does not keep.** It binds a second name to the same
            // value *inside* this body, and what happens to that name is what
            // decides — which the statements below say. What a `let` cannot do
            // is make the value outlive the call.
            _ => {}
        }

        // Then every expression of the statement, each classified where it
        // stands. The scan is flat rather than recursive because
        // [`super::sync::visit_stmt`] already reaches every sub-expression,
        // holes in an `f"…"` included.
        let (parsed, parameters, ledger, library) =
            (self.parsed, self.parameters, self.ledger, self.library);
        let unresolved = self.unresolved;
        let fields = self.fields;
        let uses = &mut *self.uses;
        super::sync::visit_stmt(parsed, stmt, &mut |expr| {
            classify(
                parsed, parameters, fields, ledger, library, unresolved, uses, expr,
            );
        });

        // And the blocks it holds. A lambda's body is one of them — it runs
        // during the call it is given to ([ADR-029](../../../docs/specification/adr/adr-029.md)
        // D4), so what it does with a parameter is what this body does with it.
        // A `spawn` is deliberately not among them and is handled in
        // [`classify`], because its body runs later and elsewhere.
        let mut blocks: Vec<&Block> = Vec::new();
        super::sync::visit_stmt_blocks(stmt, &mut |block| blocks.push(block));
        for block in blocks {
            self.block(block);
        }
    }

    /// This expression's value leaves the call, so a parameter standing at its
    /// top is kept.
    /// **A parameter handed out of the call keeps**, whether it is handed out
    /// whole or in pieces.
    ///
    /// `return answer` is the bare name. `return answer.text` is the second
    /// shape, and it was missing: the parameter stayed **lent**, the declaration
    /// was written `&Answer`, and the body took a piece out of a loan — *cannot
    /// move out of `answer.text` which is behind a shared reference*, about a
    /// file nobody wrote ([Part III
    /// C.1](../../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **A field that copies does not keep**, and that is the half this has to
    /// get right: `return point.x` for an `i64` takes nothing away, and owning
    /// the parameter for it would take the value from a caller that still wants
    /// it — a correct program refused, one call up. Where the field's type is
    /// not known it **keeps**, which is the polarity this whole column already
    /// has ([`keeps_its`]: *unknown keeps*).
    ///
    /// **`self` is not this rule's**, and deliberately: a `ref self` is a word
    /// the author wrote, so what a method does with its subject may not silently
    /// turn it into `fn(self)`. That shape is `NK1131`, which names `.clone()`
    /// and `fn …(self)` as the two ways out.
    fn hand_over(&mut self, expr: &Expr) {
        let reached = self.reached(expr);
        self.uses.kept.extend(reached);
    }

    /// The parameters an expression's value may **be** — what leaves with it
    /// when it leaves.
    ///
    /// **Through every way the value can come out**: an `if`'s two arms, each
    /// arm of a `match`, a block's last expression, and a name a `let` bound to
    /// one of them. `if c { name } else { "anonymous" }` hands `name` out of the
    /// call as surely as `return name` does, and reading only the top of the
    /// expression called it lent.
    fn reached(&self, expr: &Expr) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        match expr {
            Expr::If {
                then_branch,
                else_branch,
                ..
            } => {
                for block in std::iter::once(then_branch).chain(else_branch.as_ref()) {
                    if let Some(Stmt::Expr(value)) = block.stmts.last().map(|s| &s.node) {
                        found.extend(self.reached(value));
                    }
                }
            }
            Expr::Match { arms, .. } => {
                for arm in arms {
                    found.extend(self.reached(&arm.body));
                }
            }
            Expr::Block(block) => {
                if let Some(Stmt::Expr(value)) = block.stmts.last().map(|s| &s.node) {
                    found.extend(self.reached(value));
                }
            }
            Expr::Variable(ident) => {
                let name = self.parsed.text(*ident);
                if self.parameters.contains(name) {
                    found.insert(name.to_string());
                }
                if let Some(aliased) = self.aliases.get(name) {
                    found.extend(aliased.iter().cloned());
                }
            }
            Expr::Field { base, name } => {
                let Some(parameter) = parameter_named(self.parsed, self.parameters, base) else {
                    return found;
                };
                if parameter == "self" {
                    return found;
                }
                let field = self.parsed.text(*name);
                let copies = self
                    .fields
                    .get(&parameter)
                    .and_then(|fields| fields.get(field))
                    .is_some_and(|ty| !moves(ty));
                if !copies {
                    found.insert(parameter);
                }
            }
            _ => {}
        }
        found
    }
}

/// What one expression does with a parameter, where it stands.
///
/// Called on every sub-expression of a statement, so each rule is about *this*
/// node and never about what is under it.
#[allow(clippy::too_many_arguments)]
fn classify(
    parsed: &Parsed,
    parameters: &BTreeSet<String>,
    fields: &BTreeMap<String, BTreeMap<String, super::ty::Ty>>,
    ledger: &Ledger,
    library: &Ledger,
    unresolved: bool,
    uses: &mut Uses,
    expr: &Expr,
) {
    match expr {
        // **A struct literal keeps every field it is given.** The struct
        // outlives the call wherever it goes, and where it goes is not this
        // expression's question.
        Expr::StructLit {
            fields: written, ..
        } => {
            for field in written {
                let Some(value) = field.value.as_ref() else {
                    // `P { x }` is the field and the name in one, and the name
                    // may be a parameter.
                    if parameters.contains(parsed.text(field.name)) {
                        uses.kept.insert(parsed.text(field.name).to_string());
                    }
                    continue;
                };
                if let Some(name) = part_of_a_parameter(parsed, parameters, fields, value) {
                    uses.kept.insert(name);
                }
            }
        }
        // **And three more shapes the *lowering* consumes**, which is the same
        // clause one position over: `a ?? b` is `unwrap_or_else`, `x?.f` takes
        // the value it reaches through (Part I 3.5's own Status note), and `x?`
        // unwraps one. Each moves the value in the language below, so a
        // parameter standing there is kept whatever the body looks like here.
        Expr::Coalesce { value, .. } | Expr::SafeField { base: value, .. } | Expr::Try(value) => {
            if let Some(name) = parameter_named(parsed, parameters, value) {
                uses.kept.insert(name);
            }
        }
        // **A parameter a `match` takes apart counts as kept**, and this one is
        // an over-approximation the fail-closed clause of D2 licenses in as
        // many words: *a use the inference cannot resolve counts as keeping*.
        //
        // What cannot be resolved here is the **lowering**, not the body. Rust
        // matches through a reference with its default binding modes, so
        // `Json::Bool(b)` over a `&Json` binds `b: &bool` and `if b` stops
        // compiling — and this compiler writes no `*`. Lending a matched
        // parameter would therefore hand `rustc` a file nobody wrote
        // (Part III C.1) for every arm that uses a copied payload.
        //
        // `examples/json.nika`'s `show` is where that was met. It is a limit of
        // the emitter and is written down as one; when a deref can be written,
        // this arm goes and nothing else changes.
        Expr::Match { value, .. } => {
            if let Some(name) = parameter_named(parsed, parameters, value) {
                uses.kept.insert(name);
            }
        }
        // **A task keeps everything it names**
        // ([ADR-040](../../../docs/specification/adr/adr-040.md) D1): a body
        // that may outlive the statement takes what it names by value.
        Expr::Spawn { body, .. } => {
            let mut found = Named::default();
            match body.as_ref() {
                // `spawn fn { … }` is the form the language writes
                // ([ADR-049](../../../docs/specification/adr/adr-049.md)), so
                // the body arrives as a lambda; `Expr::Block` is what a
                // `spawn { … }` would be and is kept because both are shapes
                // this walk can read to the bottom.
                Expr::Closure { body, .. } => names_in_block(parsed, body, &mut found),
                Expr::Block(block) => names_in_block(parsed, block, &mut found),
                // A body shape this walk cannot read to the bottom. There is no
                // third answer: every parameter is kept.
                other => {
                    names_in_expr(parsed, other, &mut found);
                    found.exhaustive = false;
                }
            }
            match found.exhaustive {
                true => {
                    for name in found.names {
                        if parameters.contains(&name) {
                            uses.kept.insert(name);
                        }
                    }
                }
                false => uses.kept.extend(parameters.iter().cloned()),
            }
        }
        Expr::Call { func, args, .. } => {
            let callee = resolve(parsed, ledger, library, func);
            for (at, arg) in args.iter().enumerate() {
                let Some(name) = part_of_a_parameter(parsed, parameters, fields, arg) else {
                    continue;
                };
                match &callee {
                    // Recorded rather than decided: whether this keeps is the
                    // callee's answer, and the callee may not have one yet.
                    Some(callee) => {
                        uses.passed.insert((name, callee.clone(), at));
                    }
                    // **A callee no ledger describes** — D2's fail-closed case,
                    // and the one the whole polarity is written for.
                    None => {
                        uses.kept.insert(name);
                    }
                }
            }
        }
        // **Which entry a method call goes to is the type checker's answer and
        // not this file's** ([ADR-028](../../../docs/specification/adr/adr-028.md)).
        // A weaker question can be asked without types, the way
        // [`Ledger::candidates`] already asks it for `touches`: if **no** entry
        // named `::push` keeps its argument, this call keeps none whatever the
        // receiver turns out to be. A name no entry carries at all is
        // unresolved, and unresolved keeps.
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        }
        | Expr::SafeMethod {
            receiver,
            method,
            args,
            ..
        } => {
            let method = parsed.text(*method);
            // **A receiver a method takes by value is moved out of.**
            // `account.access fn(to) { … }` where `access` takes `self` leaves
            // nothing behind, so a parameter standing there is kept.
            //
            // Which entry the call goes to is the type checker's answer
            // ([ADR-028](../../../docs/specification/adr/adr-028.md)), and the
            // weaker question this walk can ask is the one `touches` asks —
            // with one addition: where **any** method call in this body went to
            // an entry no ledger has, the candidates are not the whole list and
            // may not be believed. `crates/nikaia/tests/lambdas.rs`'s `Account`
            // is exactly that: its `access` is a Rust stand-in taking `self`,
            // and the two `::access` entries `std` does carry both take `&self`.
            if let Some(name) = parameter_named(parsed, parameters, receiver) {
                let candidates: Vec<_> = ledger
                    .candidates(method)
                    .into_iter()
                    .chain(library.candidates(method))
                    .collect();
                let consumed = unresolved
                    || candidates.is_empty()
                    || candidates.iter().any(|(_, contract)| {
                        // **A method that changes its subject needs a `&mut`,
                        // and this compiler writes none** (D3). The ledger's
                        // type language spells both receivers `&T`, so the
                        // claim is its own column: without it `out.push(1)` on
                        // a lent parameter reached `rustc` as *cannot borrow as
                        // mutable*, about a file nobody wrote.
                        contract.mutates
                            || !contract
                                .signature
                                .as_ref()
                                .and_then(|s| s.params.first())
                                .is_some_and(|(name, ty)| name == "self" && ty.is_a_view())
                    });
                if consumed {
                    uses.kept.insert(name);
                }
            }
            for (at, arg) in args.iter().enumerate() {
                let Some(name) = part_of_a_parameter(parsed, parameters, fields, arg) else {
                    continue;
                };
                // The receiver is the callee's first parameter, so an
                // argument's own position is one further along.
                if any_candidate_keeps(ledger, library, method, at + 1) {
                    uses.kept.insert(name);
                }
            }
        }
        _ => {}
    }
}

/// Whether any entry a bare method name could reach keeps position `at`.
fn any_candidate_keeps(ledger: &Ledger, library: &Ledger, method: &str, at: usize) -> bool {
    let candidates: Vec<_> = ledger
        .candidates(method)
        .into_iter()
        .chain(library.candidates(method))
        .collect();
    if candidates.is_empty() {
        return true;
    }
    candidates.iter().any(|(_, contract)| {
        match contract
            .signature
            .as_ref()
            .and_then(|s| s.params.get(at))
            .map(|(parameter, _)| parameter.clone())
        {
            Some(parameter) => contract.keeps.contains(&parameter),
            // An entry whose signature is shorter than this call is one this
            // walk did not resolve, and that is fail-closed again.
            None => true,
        }
    })
}

/// The parameter this expression **is**, where it is one.
///
/// A bare name and nothing else. `x.field` hands over the field rather than the
/// parameter, and `f(x)` is the call's question rather than this position's.
fn parameter_named(parsed: &Parsed, parameters: &BTreeSet<String>, expr: &Expr) -> Option<String> {
    let Expr::Variable(ident) = expr else {
        return None;
    };
    let name = parsed.text(*ident).to_string();
    parameters.contains(&name).then_some(name)
}

/// **The parameter an expression is, or is a moving part of**
/// ([ADR-214](../../../docs/specification/adr/adr-214.md) D2).
///
/// `xs.push(p.name)` hands over the field, and a field cannot be taken out of a
/// loan: the parameter was written `&P` and `rustc` said *cannot move out of
/// `p.name` which is behind a shared reference*, about a file nobody wrote.
/// So a part handed to what keeps it keeps the whole, as a part handed **back**
/// already did ([`Walk::reached`]). A field that copies takes nothing away, a
/// field this file cannot see the type of keeps (the column's polarity), and
/// `self` is `NK1131`'s rather than this one's.
fn part_of_a_parameter(
    parsed: &Parsed,
    parameters: &BTreeSet<String>,
    fields: &BTreeMap<String, BTreeMap<String, super::ty::Ty>>,
    expr: &Expr,
) -> Option<String> {
    let Expr::Field { base, name } = expr else {
        return parameter_named(parsed, parameters, expr);
    };
    let root = match parameter_named(parsed, parameters, base) {
        Some(root) => {
            let copies = fields
                .get(&root)
                .and_then(|fields| fields.get(parsed.text(*name)))
                .is_some_and(|ty| !moves(ty));
            (!copies).then_some(root)?
        }
        // A deeper part: its type is not in the table, so it keeps.
        None => part_of_a_parameter(parsed, parameters, fields, base)?,
    };
    (root != "self").then_some(root)
}

/// The ledger key a plain call's callee resolves to, if any names it.
fn resolve(parsed: &Parsed, ledger: &Ledger, library: &Ledger, func: &Expr) -> Option<String> {
    let written = match func {
        Expr::Variable(ident) => parsed.text(*ident).to_string(),
        Expr::Path(segments) => segments
            .iter()
            .map(|s| parsed.text(*s).to_string())
            .collect::<Vec<_>>()
            .join("::"),
        _ => return None,
    };
    ledger
        .lookup(&written)
        .or_else(|| library.lookup(&written))
        .map(|(key, _)| key)
}

/// Every bare name a `spawn` body mentions, and whether the walk reached all of
/// it.
#[derive(Debug)]
struct Named {
    names: BTreeSet<String>,
    /// **A `spawn` inside a `spawn` clears this.** The shared walkers do not
    /// descend into a detached context — deliberately, for `sync`'s sake — so
    /// this walk cannot claim to have read one, and a name it did not see is a
    /// name it would wrongly call lent. The caller then keeps everything, which
    /// is the answer that cannot be wrong.
    exhaustive: bool,
}

impl Default for Named {
    fn default() -> Self {
        Named {
            names: BTreeSet::new(),
            exhaustive: true,
        }
    }
}

fn names_in_block(parsed: &Parsed, block: &Block, found: &mut Named) {
    for stmt in &block.stmts {
        super::sync::visit_stmt(parsed, &stmt.node, &mut |expr| {
            note_name(parsed, expr, found);
        });
        let mut blocks: Vec<&Block> = Vec::new();
        super::sync::visit_stmt_blocks(&stmt.node, &mut |inner| blocks.push(inner));
        for inner in blocks {
            names_in_block(parsed, inner, found);
        }
    }
}

fn names_in_expr(parsed: &Parsed, expr: &Expr, found: &mut Named) {
    super::sync::visit_expr(parsed, expr, &mut |inner| {
        note_name(parsed, inner, found);
    });
    let mut blocks: Vec<&Block> = Vec::new();
    super::sync::visit_expr_blocks(expr, &mut |block| blocks.push(block));
    for block in blocks {
        names_in_block(parsed, block, found);
    }
}

fn note_name(parsed: &Parsed, expr: &Expr, found: &mut Named) {
    match expr {
        Expr::Variable(ident) => {
            found.names.insert(parsed.text(*ident).to_string());
        }
        Expr::Spawn { .. } => found.exhaustive = false,
        _ => {}
    }
}

/// Whether a declared type has text both kinds flow into anywhere in it.
fn holds_either(ty: &crate::ast::Type) -> bool {
    ty.either || ty.generics.iter().any(holds_either)
}
