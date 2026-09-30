// crates/nikaia/src/views.rs
//
// Where a naked view parameter's view ends up - `NK2302`.
//
// ## What this is for
//
// A view (`&str`, `&[u8]`, `&T`) is a slice of a buffer somebody else owns
// (Part I, 6.6). A struct that holds one says so in its own declaration, and the
// struct therefore *carries* the buffer it points into wherever it goes - which
// is what makes `examples/1brc.nika` free: `Reading` holds a `&str`, so a
// function that takes a `Reading` has been told which buffer the name came from.
//
// A parameter written `&str` on its own has been told nothing of the kind. It is
// a view of *some* buffer, for the duration of the call, and that is all it
// says. So a function that takes one and **stores it** - into a field of its
// subject, into a struct it hands back - is asking to keep something past the
// only moment it was given. Before this check the program reached the language
// below anyway and came back refused in the vocabulary of the generated file,
// which Part III C.1 calls a bug in this compiler rather than in the program.
//
// This module answers the question in Nikaia instead, and the answer points at
// the form that works.
//
// ## Which way it errs, and why
//
// **Fail-closed on "is it stored"**
// ([ADR-010](../../docs/specification/adr/adr-010.md) D1: an analysis that fails
// open is a vulnerability generator - the same polarity holds here, where failing
// open means emitting Rust that does not compile and breaking C.1). Where a
// carrier of the view is handed to a call on the subject, or on a field of the
// subject whose declared type holds a view, this module **cannot see** whether
// the callee keeps it: `insert` does and `contains_key` does not, and `std`'s
// ledger records `key: ?` for both (`crates/nikaia-std/std.contracts`). So it
// reports it as stored. That refuses `self.names.contains_key(name)`, which the
// language below accepts; the cost is named here rather than hidden.
//
// **But the domain is narrow on purpose, and that is not the same thing.** The
// only destinations considered are the ones whose buffer is known *not* to be
// the parameter's own:
//
//   1. a field reached from `self`, whose declared type holds a view;
//   2. a field of a struct or enum the function builds, where that field holds a
//      view;
//   3. the result, where the result holds a view and the language below would
//      tie it to something other than this parameter;
//   4. a task's body, which outlives the call by construction.
//
// Everything else is left alone, and deliberately: `examples/k-nucleotide/src/main.nika`
// writes views of `seq` into a `HashMap[&str, Tally]` it returns, and that is
// correct because the result *is* `seq`'s buffer - case 3 asks exactly that
// question before it reports anything. A local whose declared type holds a view
// is **not** a destination here, because deciding it needs to know which buffer
// each local views, which this compiler does not compute.
//
// ## One step further: a destination that already names the buffer
//
// A field is declared, and its declaration says which buffer it points into -
// that is the whole of [ADR-008](../../docs/specification/adr/adr-008.md) D1. So
// where the destination is a field of the **subject**, or of a struct built
// inside an `impl` whose target holds a view, the buffer is already named and
// the parameter can be written as a view *of that buffer* rather than refused.
// [`Stored::carried`] is that bit, and the emitter reads it through [`carried`].
//
// This is not a second mechanism. It is the same question - where does the view
// go - asked about one more link in the chain, and the refusal is what is left
// where the chain runs out.
//
// ## Where the reach stops
//
// Four places, named so the next increment knows what it is extending:
//
//   1. **A buffer the emitter does not have in hand.** A free function, or a
//      method of an `impl` whose target holds no view, has no named buffer to
//      write, so a field destination there is still refused - including a struct
//      the function builds and hands back.
//   2. **The result.** Handing the view back would need the result's buffer
//      written out too, which is a second edit to the signature and is not made.
//   3. **A local.** A view that reaches a field through a local is followed by
//      name (see [`Scanner::carriers`]) but a *local* is never itself a
//      destination, because deciding one needs to know which buffer each local
//      views.
//   4. **The caller.** Writing the subject's buffer on a parameter narrows what
//      callers may pass, and nothing here checks the call sites. Nothing in the
//      corpus hands such a method a shorter-lived view; a program that did would
//      be refused by the language below rather than here.
//
// One forward pass, no reassignment tracking, names only. A name that ever
// carries the view keeps carrying it.

use std::collections::{BTreeSet, HashMap, HashSet};

use winnow_grammar::Symbol as Ident;

use crate::ast::{Block, Expr, Item, Span, Stmt, Type, VariantFields};
use crate::check::{Finding, Severity};
use crate::contracts::Ledger;
use crate::emit::{borrowing_structs, holds_view, names_borrowing};
use crate::parser::Parsed;

/// One naked view parameter, and the place its view ends up.
#[derive(Debug, Clone)]
pub struct Stored {
    /// The statement that stores it - what the caret goes under.
    pub span: Span,
    /// The parameter, as the source names it.
    pub param: String,
    /// The parameter's declared type, as the source writes it: `&str`.
    pub ty: String,
    /// The function or method it belongs to.
    pub function: String,
    /// The `impl` target, where the function is a method of one.
    pub subject: Option<String>,
    /// Where the view goes.
    pub into: Destination,
    /// Whether the destination's own buffer already names this view, *and* the
    /// emitter has that buffer in hand where the parameter is written.
    ///
    /// True exactly for a field of the subject, or a call on the subject, inside
    /// an `impl` whose target holds a view: there the subject names the buffer,
    /// so the parameter is lowered as a view of it instead of being refused.
    /// False is a refusal.
    pub carried: bool,
    /// The byte the function starts at, which is how the emitter finds the
    /// parameter again.
    pub method: usize,
    /// The parameter's interned name, for the same reason.
    pub symbol: Ident,
    /// Where the view is carried by a struct **parameter** rather than the
    /// subject: that parameter, whose buffer the view is written as a view of.
    pub carrier: Option<Ident>,
}

/// The kind of place a stored view reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// A field whose declared type holds a view. `owner` declares it.
    Field {
        owner: String,
        field: String,
        ty: String,
    },
    /// The subject itself: a call on `self`, whose end is not visible here.
    Subject { owner: String },
    /// The result, and the result holds a view of a buffer that is not this
    /// parameter's.
    Result { ty: String },
    /// A task's body. It outlives the call whatever it does with the view.
    Task,
    /// A field of a **parameter** whose struct holds views: `s.label = name`
    /// in `fn relabel(mut s: Summary, name: ref String)`. The struct names
    /// the buffer, so the view parameter can be written as a view of it.
    ParamField {
        param: Ident,
        owner: String,
        field: String,
        ty: String,
    },
}

/// Every naked view parameter in this unit whose view is stored.
///
/// **The ledgers, for the one question a walk of the source cannot answer**: a
/// call says which of its arguments its result may point into
/// (`returns = "borrows(…)"`), and without that column a parameter handed to
/// `self.head.query(name)` looks like a parameter handed *out* of the function
/// — a correct program refused, which is what [Part III
/// C.4](../../docs/specification/30-nikaia-tooling.md) forbids.
pub fn analyse(parsed: &Parsed, own: &Ledger, library: &Ledger) -> Vec<Stored> {
    let borrowing = borrowing_structs(parsed);
    let fields = fields_of(parsed);
    let unit = Unit {
        parsed,
        borrowing: &borrowing,
        fields: &fields,
        borrows: Borrows { own, library },
    };
    let mut found = Vec::new();

    for item in &parsed.program.items {
        match &item.node {
            Item::Fn { .. } => scan(unit, None, item.span.at(), &item.node, &mut found),
            Item::Impl {
                target, methods, ..
            } => {
                for method in methods {
                    scan(
                        unit,
                        Some(target),
                        method.span.at(),
                        &method.node,
                        &mut found,
                    );
                }
            }
            _ => {}
        }
    }
    found.sort_by_key(|s| s.span.at());
    found
}

/// The refusals: every stored naked view whose destination names no buffer.
pub fn check(parsed: &Parsed, own: &Ledger, library: &Ledger) -> Vec<Finding> {
    analyse(parsed, own, library)
        .iter()
        .filter(|stored| !stored.carried)
        .map(finding)
        .collect()
}

/// The parameters the emitter writes as views of the subject's buffer, by the
/// byte the method they belong to starts at.
///
/// This is the whole of the extension in the emitter: a parameter in here is
/// spelled with the buffer named, where before the choice was between an
/// unwritable signature and a refusal.
pub fn carried(parsed: &Parsed, own: &Ledger, library: &Ledger) -> HashMap<usize, HashSet<Ident>> {
    let mut out: HashMap<usize, HashSet<Ident>> = HashMap::new();
    for stored in analyse(parsed, own, library) {
        if stored.carried {
            let at = out.entry(stored.method).or_default();
            at.insert(stored.symbol);
            // The struct parameter that carries it is written over the same
            // buffer.
            at.extend(stored.carrier);
        }
    }
    out
}

/// What one refusal says.
///
/// Part III C.2: the headline says what is wrong, the notes say why the compiler
/// believes it, and the help is the shape that works - written out rather than
/// described, because `examples/1brc.nika` already writes it and a reader who
/// has to invent it has been told half an answer.
fn finding(stored: &Stored) -> Finding {
    let Stored {
        param,
        ty,
        function,
        subject,
        into,
        ..
    } = stored;

    let where_it_goes = match into {
        Destination::Field { owner, field, .. } | Destination::ParamField { owner, field, .. } => {
            format!("`{owner}.{field}`")
        }
        Destination::Subject { owner } => format!("`{owner}`, through a call on it"),
        Destination::Result { .. } => "the value this function returns".to_string(),
        Destination::Task => "a task, which goes on running after the call".to_string(),
    };

    let why = match into {
        Destination::ParamField { owner, field, .. } => format!(
            "`{owner}.{field}` belongs to a struct that was passed in, and this function \
             stores into more than one such struct, so it's unclear whose data `{param}` \
             points into."
        ),
        Destination::Field {
            owner,
            field,
            ty: field_ty,
        } => format!(
            "`{owner}.{field}` is a `{field_ty}`, so it points into some data, but \
             `{param}: {ty}` doesn't say whose."
        ),
        Destination::Subject { owner } => format!(
            "A call on `{owner}` might keep what it's given, and nothing says it doesn't, so \
             `{param}` counts as kept."
        ),
        Destination::Result { ty: result } => format!(
            "The result is a `{result}`, which holds a view, and this function takes views of \
             other data besides `{param}`, so it's unclear which one the result points into."
        ),
        Destination::Task => "A task keeps running after the call that started it, so \
             anything it looks at has to live longer than the call."
            .to_string(),
    };

    let of = match subject {
        Some(subject) => format!("`{subject}.{function}`"),
        None => format!("`{function}`"),
    };

    Finding {
        severity: Severity::Error,
        span: stored.span,
        code: "NK2302",
        message: format!(
            "{of} keeps `{param}` after this call, but `{param}: {ty}` doesn't say whose data \
             it points into."
        ),
        notes: vec![format!("It's stored in {where_it_goes}."), why],
        help: Some(format!(
            "Put the view in a struct and pass the struct, so its declaration says whose data \
             it is (like `Reading` in `examples/1brc.nika`):\n\
             \x20          struct Held {{ {param}: {ty} }}\n\
             \x20          …\n\
             \x20          fn {function}(…, held: Held) {{ … held.{param} … }}\n\
             \x20      Or keep a copy of the text with `.clone()`, which costs one allocation."
        )),
        labels: Vec::new(),
    }
}

/// Every struct's and enum's fields, by the name that declares them.
///
/// An enum's variants are flattened into it: for the one question this map is
/// asked - does this field hold a view - a variant's field is a field of the
/// enum.
pub(crate) fn fields_of(parsed: &Parsed) -> HashMap<Ident, Vec<(String, Type)>> {
    let mut out: HashMap<Ident, Vec<(String, Type)>> = HashMap::new();
    for item in &parsed.program.items {
        match &item.node {
            Item::Struct { name, fields, .. } => {
                out.insert(
                    *name,
                    fields
                        .iter()
                        .map(|f| (parsed.text(f.name).to_string(), f.ty.clone()))
                        .collect(),
                );
            }
            Item::Enum { name, variants, .. } => {
                let mut carried = Vec::new();
                for variant in variants {
                    match &variant.fields {
                        VariantFields::Unit => {}
                        VariantFields::Tuple(types) => {
                            for (i, ty) in types.iter().enumerate() {
                                carried.push((i.to_string(), ty.clone()));
                            }
                        }
                        VariantFields::Named(fields) => {
                            for field in fields {
                                carried
                                    .push((parsed.text(field.name).to_string(), field.ty.clone()));
                            }
                        }
                    }
                }
                out.insert(*name, carried);
            }
            _ => {}
        }
    }
    out
}

/// Everything about the unit that does not change from one function to the next.
///
/// One parameter rather than four, which is what the walk reads it as: these are
/// facts about the file, and the three below are facts about the function.
#[derive(Clone, Copy)]
struct Unit<'p> {
    parsed: &'p Parsed,
    borrowing: &'p HashSet<Ident>,
    fields: &'p HashMap<Ident, Vec<(String, Type)>>,
    borrows: Borrows<'p>,
}

/// One function or method, asked about each of its naked view parameters.
fn scan(unit: Unit<'_>, target: Option<&Type>, method: usize, item: &Item, out: &mut Vec<Stored>) {
    let Unit {
        parsed,
        borrowing,
        fields,
        borrows,
    } = unit;
    let Item::Fn {
        name,
        receiver,
        args,
        config,
        ret_type,
        body,
        ..
    } = item
    else {
        return;
    };

    // A parameter and an option are the same thing here: both arrive from the
    // caller and both are written with a type (Part I, 5.1).
    let declared: Vec<(Ident, &Type)> = args
        .iter()
        .map(|a| (a.name, &a.ty))
        .chain(config.iter().map(|c| (c.name, &c.ty)))
        .collect();
    if !declared.iter().any(|(_, ty)| ty.is_view) {
        return;
    }

    // Which parameters name a buffer at all. The language below ties an elided
    // result to the subject where there is one, and otherwise to the single
    // input that has a buffer - so a result holding a view belongs to *this*
    // parameter only where it is the one and only source.
    let brings_buffer = |ty: &&Type| holds_view(ty) || names_borrowing(ty, borrowing);
    let sources: Vec<Ident> = declared
        .iter()
        .filter(|(_, ty)| brings_buffer(ty))
        .map(|(name, _)| *name)
        .collect();
    let subject_is_a_source = receiver.is_some_and(|r| r.is_ref);
    let result_holds_view = ret_type.as_ref().is_some_and(|ty| brings_buffer(&ty));

    // Whether the subject names a buffer. Where it does, the emitter writes that
    // buffer and `'a` is in scope on every method of the `impl`; where it does
    // not, there is nothing to write and a stored view is still a refusal.
    let subject_names_a_buffer = target.is_some_and(|t| borrowing.contains(&t.name));
    let subject_name = target.map(|t| parsed.text(t.name).to_string());
    let function = match name {
        Some(name) => parsed.text(*name).to_string(),
        // Part I 4.2's anonymous constructor.
        None => "new".to_string(),
    };
    let empty: Vec<(String, Type)> = Vec::new();
    let subject_fields = target
        .and_then(|t| fields.get(&t.name))
        .unwrap_or(&empty)
        .as_slice();

    for (param, ty) in declared.iter().filter(|(_, ty)| ty.is_view) {
        // Whether the result is this parameter's own buffer. Where it is,
        // handing the view back is exactly what the signature already says.
        let result_is_ours = !subject_is_a_source && sources.len() == 1 && sources[0] == *param;
        let mut scanner = Scanner {
            parsed,
            borrowing,
            fields,
            subject: target,
            subject_fields,
            borrows,
            declared: &declared,
            result: if result_holds_view && !result_is_ours {
                ret_type.as_ref()
            } else {
                None
            },
            ours: ret_type
                .as_ref()
                .filter(|_| result_holds_view && result_is_ours)
                .map(|ty| ty.name),
            carriers: BTreeSet::from([parsed.text(*param).to_string()]),
            found: Vec::new(),
        };
        scanner.block(body, true);

        // **One answer per parameter**, because one signature is what gets
        // written: the parameter is lowered with the subject's buffer named only
        // if *every* destination is one that buffer covers. A single destination
        // it does not cover is a refusal, and that one is what the caret goes
        // under - at the place that says the most about where the view went (see
        // [`rank`]).
        // **A field of one struct parameter** is covered the same way, by
        // that parameter's buffer - where the subject names none, and every
        // such field is the same parameter's.
        let carriers: HashSet<Ident> = scanner
            .found
            .iter()
            .filter_map(|(_, into)| match into {
                Destination::ParamField { param, .. } => Some(*param),
                _ => None,
            })
            .collect();
        let one_carrier = carriers.len() == 1 && !subject_names_a_buffer;
        let (unreached, reached): (Vec<_>, Vec<_>) =
            scanner.found.into_iter().partition(|(_, into)| {
                !(covered_by_the_subject(subject_names_a_buffer, into)
                    || (one_carrier && matches!(into, Destination::ParamField { .. })))
            });
        let pick = |found: Vec<(Span, Destination)>| {
            found
                .into_iter()
                .min_by_key(|(span, into)| (rank(into), span.at()))
        };
        let (carried, picked) = match unreached.is_empty() {
            true => (true, pick(reached)),
            false => (false, pick(unreached)),
        };
        let Some((span, into)) = picked else { continue };
        out.push(Stored {
            span,
            param: parsed.text(*param).to_string(),
            ty: write_type(parsed, ty),
            function: function.clone(),
            subject: subject_name.clone(),
            into,
            carried,
            method,
            symbol: *param,
            carrier: carriers
                .iter()
                .next()
                .copied()
                .filter(|_| one_carrier && carried),
        });
    }
}

/// Whether the subject's own buffer already covers this destination.
///
/// A field is declared, and the declaration names the buffer (ADR-008 D1). So a
/// field of the subject - and a call on the subject, which can only put the view
/// somewhere the subject reaches - is covered as soon as the subject names a
/// buffer at all. The result is not: covering it means writing the result's
/// buffer too, which is the next increment and not this one.
fn covered_by_the_subject(subject_names_a_buffer: bool, into: &Destination) -> bool {
    subject_names_a_buffer
        && matches!(
            into,
            Destination::Field { .. } | Destination::Subject { .. }
        )
}

/// How much a destination says about where the view went, lowest first.
///
/// A named field is the most useful thing to put under the caret: it is a
/// declaration the reader can go and look at. The result is the least, because
/// it says only that the view left.
fn rank(into: &Destination) -> u8 {
    match into {
        Destination::Field { .. } => 0,
        Destination::Subject { .. } => 1,
        Destination::Task => 2,
        Destination::Result { .. } => 3,
        Destination::ParamField { .. } => 0,
    }
}

/// A type as the source writes it, for a message to quote back.
///
/// **The third writer of a written type**, beside `Ty`'s own `Display` and the
/// emitter's — and the one that reads the **AST** rather than the checker's
/// answer, which is why it exists: what a message quotes back has to be what
/// the program wrote, arguments and all
/// ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
pub(crate) fn write_type(parsed: &Parsed, ty: &Type) -> String {
    let mut out = String::new();
    // **`ref X` is a view of an `X`**
    // ([ADR-184](../../docs/specification/adr/adr-184.md) D1). A message that
    // wrote the character this replaced would name a spelling its reader
    // cannot type.
    if ty.is_view {
        out.push_str("ref ");
        if ty.is_mut {
            out.push_str("mut ");
        }
    }
    let parts = || -> Vec<String> { ty.generics.iter().map(|g| write_type(parsed, g)).collect() };
    if ty.is_tuple {
        out.push_str(&format!("({})", parts().join(", ")));
        return out;
    }
    // **And a view of `String` is a view of text** (D2): one noun, whichever
    // door the type came through.
    let name = parsed.text(ty.name);
    let name = match (ty.is_view, name) {
        (true, "str") => crate::contracts::ty::TEXT,
        _ => name,
    };
    out.push_str(name);
    if !ty.generics.is_empty() {
        out.push_str(&format!("[{}]", parts().join(", ")));
    }
    out
}

/// **Which of a call's arguments its result may point into**, read off the two
/// ledgers.
///
/// `returns = "borrows(self)"` is the column, and it names *parameters*. A
/// carrier that is not one of them cannot reach the result through that call,
/// which is the fact [`Scanner::returned`] was missing.
#[derive(Clone, Copy)]
struct Borrows<'p> {
    own: &'p Ledger,
    library: &'p Ledger,
}

impl Borrows<'_> {
    /// The one entry a call goes to, by the key that names it.
    ///
    /// **By the whole key and not by the method name**, because a wrapper keeps
    /// the name it wraps: `Request::query` calling `http1::Head::query` matched
    /// itself when the lookup was by name alone, and its own `borrows` column —
    /// the very thing being decided — answered for the callee's. The receiver
    /// has to be typed for this, and where it cannot be the caller reads the
    /// absence as *nobody said*, which is the fail-closed direction.
    fn of(&self, key: &str) -> Option<&crate::contracts::FnContract> {
        self.own
            .functions
            .get(key)
            .or_else(|| self.library.functions.get(key))
    }
}

/// The walk, for one parameter of one function.
struct Scanner<'p> {
    parsed: &'p Parsed,
    borrowing: &'p HashSet<Ident>,
    fields: &'p HashMap<Ident, Vec<(String, Type)>>,
    subject: Option<&'p Type>,
    subject_fields: &'p [(String, Type)],
    /// The two ledgers, for [`Scanner::hands_back_another_buffer`].
    borrows: Borrows<'p>,
    /// The parameters and their written types, for [`Scanner::type_of`].
    declared: &'p [(Ident, &'p Type)],
    /// The declared result, where handing the view back would be storing it.
    result: Option<&'p Type>,
    /// The declared result's name, where the result holds a view and can only
    /// point into this parameter's buffer: a literal of it handed back puts
    /// the view where the signature already says it goes.
    ours: Option<Ident>,
    /// Names that may carry this parameter's view. Monotone: a name that ever
    /// carries it keeps carrying it, which is the fail-closed direction.
    carriers: BTreeSet<String>,
    found: Vec<(Span, Destination)>,
}

/// What the receiver of a call is rooted in.
enum Root {
    /// `self`.
    Subject,
    /// `self.stations`, `self.a.b` - the field off `self` is what carries.
    SubjectField(Ident),
    /// A parameter whose struct holds views.
    Param(Ident),
    /// `s.label`, off such a parameter: the parameter and the field.
    ParamField(Ident, Ident),
    /// A local, a literal, a free call: not a place this module decides about.
    Elsewhere,
}

impl Scanner<'_> {
    /// `tail` says whether the last statement of this block is the function's
    /// result - true for the body itself and false inside a loop or a lambda,
    /// where a trailing expression is not what the function hands back.
    fn block(&mut self, block: &Block, tail: bool) {
        let last = block.stmts.len().saturating_sub(1);
        for (i, stmt) in block.stmts.iter().enumerate() {
            let returning = tail && i == last;
            match &stmt.node {
                // **Every name**, because a view carried into a tuple reaches
                // each part of it and this walk may not under-approximate
                // ([ADR-098](../../../docs/specification/adr/adr-098.md)).
                Stmt::Let { names, value, .. } => {
                    // **A buffer of its own carries no view of the parameter**
                    // ([ADR-209](../../docs/specification/adr/adr-209.md) §6):
                    // `fs::read_to_string(path, …)` mentions `path` and hands
                    // back a `String` it read, so a view cut from that text is
                    // a view of the text. Reading it as `path`'s blamed the
                    // parameter for a view of a file - the refusal named the
                    // wrong buffer and the way out it gave could not work.
                    let own_buffer = matches!(
                        crate::contracts::tether::makes_a_buffer(
                            self.parsed,
                            value,
                            self.borrows.own,
                            self.borrows.library
                        ),
                        crate::contracts::tether::Buffer::Named(_)
                    );
                    if self.mentions(value) && !own_buffer {
                        for name in names {
                            self.carriers.insert(self.parsed.text(*name).to_string());
                        }
                    }
                    self.stores(value, &stmt.span);
                }
                Stmt::Comptime { name, value, .. } => {
                    if self.mentions(value) {
                        self.carriers.insert(self.parsed.text(*name).to_string());
                    }
                    self.stores(value, &stmt.span);
                }
                Stmt::Assign { target, value, .. } => {
                    if self.mentions(value) {
                        match target {
                            Expr::Variable(name) => {
                                self.carriers.insert(self.parsed.text(*name).to_string());
                            }
                            _ => self.assigned(target, &stmt.span),
                        }
                    }
                    self.stores(target, &stmt.span);
                    self.stores(value, &stmt.span);
                }
                Stmt::Return(Some(value)) => {
                    self.returned(value, &stmt.span);
                    self.handed_back(value, &stmt.span);
                }
                Stmt::Return(None) => {}
                Stmt::Expr(expr) => {
                    if returning {
                        self.returned(expr, &stmt.span);
                        self.handed_back(expr, &stmt.span);
                    } else {
                        self.stores(expr, &stmt.span);
                    }
                }
                Stmt::For {
                    bindings,
                    iter,
                    body,
                } => {
                    if self.mentions(iter) {
                        for binding in bindings {
                            self.carriers.insert(self.parsed.text(*binding).to_string());
                        }
                    }
                    self.stores(iter, &stmt.span);
                    self.block(body, false);
                }
                Stmt::While { cond, body } => {
                    self.stores(cond, &stmt.span);
                    self.block(body, false);
                }
                // Neither stores anything and neither carries a value, so
                // there is no destination for a view to reach here.
                Stmt::Break | Stmt::Continue => {}
            }
        }
    }

    /// **A literal of the result, handed back**, where the result can only
    /// point into this parameter's buffer: its fields are what the signature
    /// says the result views, so they are no store. What stands inside them
    /// is still asked.
    fn handed_back(&mut self, value: &Expr, span: &Span) {
        match value {
            Expr::StructLit { name, .. } if self.ours == Some(*name) => self.descend(value, span),
            _ => self.stores(value, span),
        }
    }

    /// `self.label = name`, and the deeper forms of it.
    fn assigned(&mut self, target: &Expr, span: &Span) {
        match self.root(target) {
            Root::SubjectField(field) => self.field_of_subject(field, span),
            Root::Subject => self.subject_destination(span),
            Root::ParamField(param, field) => self.field_of_param(param, field, span),
            Root::Param(_) | Root::Elsewhere => {}
        }
    }

    /// The struct a parameter's declared type names, where that struct holds
    /// views and the parameter is not itself a view.
    fn a_struct_parameter(&self, name: Ident) -> Option<Ident> {
        self.declared
            .iter()
            .find(|(declared, _)| *declared == name)
            .map(|(_, ty)| *ty)
            .filter(|ty| !ty.is_view && self.borrowing.contains(&ty.name))
            .map(|ty| ty.name)
    }

    /// A store into the field `field` of the parameter `param`.
    fn field_of_param(&mut self, param: Ident, field: Ident, span: &Span) {
        let Some(owner) = self.a_struct_parameter(param) else {
            return;
        };
        let name = self.parsed.text(field).to_string();
        let Some((_, ty)) = self
            .fields
            .get(&owner)
            .and_then(|fields| fields.iter().find(|(declared, _)| *declared == name))
        else {
            return;
        };
        if !holds_view(ty) && !names_borrowing(ty, self.borrowing) {
            return;
        }
        self.found.push((
            *span,
            Destination::ParamField {
                param,
                owner: self.parsed.text(owner).to_string(),
                field: name,
                ty: write_type(self.parsed, ty),
            },
        ));
    }

    /// A value handed back, where the result holds a view of another buffer.
    fn returned(&mut self, value: &Expr, span: &Span) {
        let Some(result) = self.result else { return };
        if !self.mentions(value) {
            return;
        }
        // **A call says which of its arguments its result may point into**, and
        // a carrier that is none of them does not reach the result through it.
        // Without this, `return self.head.query(name)` read as *`name` goes into
        // the value this function hands back* — a correct program refused,
        // because `http1::Head::query` is written `returns = "borrows(self)"`
        // and its result points into the **head**.
        if self.hands_back_another_buffer(value) {
            return;
        }
        self.found.push((
            *span,
            Destination::Result {
                ty: write_type(self.parsed, result),
            },
        ));
    }

    /// Whether this expression is a call whose result points into something that
    /// is **not** a carrier of the view.
    ///
    /// `returns = "borrows(a | b)"` names the positions a result may point into
    /// ([ADR-098](../../../docs/specification/adr/adr-098.md)). An argument that
    /// is none of them does not reach the result through the call, which is what
    /// makes `return self.head.query(name)` a correct program: the result points
    /// into the head, and `name` is only read.
    ///
    /// Everything unwritten reads as *nobody said*: no entry, an empty column, a
    /// receiver this walk cannot type. The refusal stands there, which is where
    /// an analysis that cannot see belongs (ADR-010 D1).
    fn hands_back_another_buffer(&self, value: &Expr) -> bool {
        let (key, receiver, args): (String, Option<&Expr>, &Vec<Expr>) = match value {
            Expr::MethodCall {
                method,
                receiver,
                args,
                ..
            }
            | Expr::SafeMethod {
                method,
                receiver,
                args,
                ..
            } => {
                let Some(ty) = self.type_of(receiver) else {
                    return false;
                };
                let subject = self.parsed.text(ty.name);
                let method = self.parsed.text(*method);
                (
                    format!("{subject}::{method}"),
                    Some(receiver.as_ref()),
                    args,
                )
            }
            // A free call names its callee with an expression, and only two
            // shapes of one are a key a ledger could hold: a bare name and a
            // path. Anything else is code decided at run time, which no column
            // describes.
            Expr::Call { func, args, .. } => match func.as_ref() {
                Expr::Variable(name) => (self.parsed.text(*name).to_string(), None, args),
                Expr::Path(parts) => (
                    parts
                        .iter()
                        .map(|part| self.parsed.text(*part))
                        .collect::<Vec<_>>()
                        .join("::"),
                    None,
                    args,
                ),
                _ => return false,
            },
            _ => return false,
        };
        let Some(contract) = self.borrows.of(&key) else {
            return false;
        };
        let Some(signature) = contract.signature.as_ref() else {
            return false;
        };
        // A column nothing wrote says nothing: the result may point anywhere, so
        // the carrier is not ruled out.
        if contract.borrows.is_empty() {
            return false;
        }
        // **The receiver is a position too**, and `self` is what names it.
        // `self.head.query(name)` borrows its receiver, and the receiver is
        // `self.head` - which carries nothing, so the carrier is clear.
        let takes_a_receiver = signature.takes_a_receiver();
        contract.borrows.iter().all(|borrowed| {
            if borrowed == "self" {
                return match receiver {
                    Some(receiver) => !self.mentions(receiver),
                    // A free function with a `self` in its column is a ledger
                    // this walk does not understand.
                    None => false,
                };
            }
            let Some(at) = signature
                .params
                .iter()
                .position(|(param, _)| param == borrowed)
            else {
                return false;
            };
            let Some(at) = at.checked_sub(usize::from(takes_a_receiver)) else {
                return false;
            };
            match args.get(at) {
                Some(argument) => !self.mentions(argument),
                None => false,
            }
        })
    }

    /// The declared type of a receiver, where the source wrote one down.
    ///
    /// Three places have one: the subject, a field of the subject, and a
    /// parameter. A local has none - its type is the checker's business and this
    /// walk runs beside the checker, not after it.
    fn type_of(&self, receiver: &Expr) -> Option<&Type> {
        match receiver {
            Expr::Variable(name) => {
                let name = self.parsed.text(*name);
                if name == "self" {
                    return self.subject;
                }
                self.declared
                    .iter()
                    .find(|(param, _)| self.parsed.text(*param) == name)
                    .map(|(_, ty)| *ty)
            }
            Expr::Field { base, name } => {
                let fields = match base.as_ref() {
                    Expr::Variable(base) if self.parsed.text(*base) == "self" => {
                        self.subject_fields
                    }
                    _ => {
                        let ty = self.type_of(base)?;
                        self.fields.get(&ty.name)?.as_slice()
                    }
                };
                let name = self.parsed.text(*name);
                fields
                    .iter()
                    .find(|(field, _)| field == name)
                    .map(|(_, ty)| ty)
            }
            _ => None,
        }
    }

    /// Every store inside one expression.
    fn stores(&mut self, expr: &Expr, span: &Span) {
        match expr {
            Expr::MethodCall {
                receiver,
                args,
                config,
                ..
            }
            // A store that may not happen is still a store: what this asks is
            // *where the view could end up*, and a `?.` changes only whether it
            // gets there.
            | Expr::SafeMethod {
                receiver,
                args,
                config,
                ..
            } => {
                let handed_over = args.iter().any(|a| self.mentions(a))
                    || config.iter().any(|c| self.mentions(&c.value));
                if handed_over {
                    match self.root(receiver) {
                        Root::SubjectField(field) => self.field_of_subject(field, span),
                        Root::Subject => self.subject_destination(span),
                        Root::Param(_) | Root::ParamField(..) | Root::Elsewhere => {}
                    }
                }
                self.descend(expr, span);
            }
            Expr::StructLit { name, fields } => {
                if self.borrowing.contains(name) {
                    let declared = self.fields.get(name).cloned().unwrap_or_default();
                    for init in fields {
                        let field = self.parsed.text(init.name).to_string();
                        let mentioned = match &init.value {
                            Some(value) => self.mentions(value),
                            // `Reading { name, temp }` - the shorthand names a
                            // variable spelled like the field.
                            None => self.carriers.contains(&field),
                        };
                        if !mentioned {
                            continue;
                        }
                        let Some((_, ty)) = declared.iter().find(|(f, _)| *f == field) else {
                            continue;
                        };
                        if !holds_view(ty) && !names_borrowing(ty, self.borrowing) {
                            continue;
                        }
                        self.found.push((
                            *span,
                            Destination::Field {
                                owner: self.parsed.text(*name).to_string(),
                                field,
                                ty: write_type(self.parsed, ty),
                            },
                        ));
                    }
                }
                self.descend(expr, span);
            }
            Expr::Spawn { body, .. } => {
                if self.mentions(body) {
                    self.found.push((*span, Destination::Task));
                }
                self.descend(expr, span);
            }
            other => self.descend(other, span),
        }
    }

    /// A call on the subject itself, whose end this module does not see.
    fn subject_destination(&mut self, span: &Span) {
        if let Some(owner) = self.subject_text() {
            self.found.push((*span, Destination::Subject { owner }));
        }
    }

    /// A store into the field `field` of the subject, where that field holds a
    /// view. A field that holds none cannot be where the view went.
    fn field_of_subject(&mut self, field: Ident, span: &Span) {
        let name = self.parsed.text(field).to_string();
        let Some((_, ty)) = self
            .subject_fields
            .iter()
            .find(|(declared, _)| *declared == name)
        else {
            return;
        };
        if !holds_view(ty) && !names_borrowing(ty, self.borrowing) {
            return;
        }
        let Some(owner) = self.subject_text() else {
            return;
        };
        self.found.push((
            *span,
            Destination::Field {
                owner,
                field: name,
                ty: write_type(self.parsed, ty),
            },
        ));
    }

    fn subject_text(&self) -> Option<String> {
        self.subject.map(|t| self.parsed.text(t.name).to_string())
    }

    /// What a call's receiver is rooted in.
    fn root(&self, expr: &Expr) -> Root {
        match expr {
            Expr::Variable(name) if self.parsed.text(*name) == "self" => Root::Subject,
            Expr::Variable(name) if self.a_struct_parameter(*name).is_some() => Root::Param(*name),
            Expr::Field { base, name } => match self.root(base) {
                // The outer field sits inside the root one, so the root field
                // is what carries the buffer.
                Root::Subject => Root::SubjectField(*name),
                Root::Param(param) => Root::ParamField(param, *name),
                other => other,
            },
            Expr::MethodCall { receiver, .. } | Expr::SafeMethod { receiver, .. } => {
                self.root(receiver)
            }
            Expr::Index { base, .. } => self.root(base),
            Expr::Try(inner) => self.root(inner),
            _ => Root::Elsewhere,
        }
    }

    /// Whether a carrier of the view appears anywhere in this expression.
    ///
    /// By name, and nothing more: `name.len()` mentions `name`, and so does
    /// `name.to_owned()`. Over-reporting here is the fail-closed direction, and
    /// the destination's declared type is what keeps it from mattering - a field
    /// that holds no view is never a destination.
    fn mentions(&self, expr: &Expr) -> bool {
        let mut found = false;
        each(expr, &mut |inner| {
            if let Expr::Variable(name) = inner
                && self.carriers.contains(self.parsed.text(*name))
            {
                found = true;
            }
        });
        found
    }

    /// The sub-expressions of `expr`, for [`Scanner::stores`] to recurse into.
    fn descend(&mut self, expr: &Expr, span: &Span) {
        let mut children: Vec<&Expr> = Vec::new();
        let mut blocks: Vec<&Block> = Vec::new();
        parts(expr, &mut children, &mut blocks);
        for child in children {
            self.stores(child, span);
        }
        for block in blocks {
            // A lambda's trailing expression is the lambda's result, not the
            // function's.
            self.block(block, false);
        }
    }
}

/// Every expression inside `expr`, itself included.
fn each(expr: &Expr, f: &mut impl FnMut(&Expr)) {
    f(expr);
    let mut children: Vec<&Expr> = Vec::new();
    let mut blocks: Vec<&Block> = Vec::new();
    parts(expr, &mut children, &mut blocks);
    for child in children {
        each(child, f);
    }
    for block in blocks {
        each_block(block, f);
    }
}

fn each_block(block: &Block, f: &mut impl FnMut(&Expr)) {
    for stmt in &block.stmts {
        for child in stmt_exprs(&stmt.node) {
            each(child, f);
        }
        if let Stmt::For { body, .. } | Stmt::While { body, .. } = &stmt.node {
            each_block(body, f);
        }
    }
}

/// The expressions a statement holds.
fn stmt_exprs(stmt: &Stmt) -> Vec<&Expr> {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Comptime { value, .. } => vec![value],
        Stmt::Expr(expr) | Stmt::Return(Some(expr)) => vec![expr],
        Stmt::Assign { target, value, .. } => vec![target, value],
        Stmt::For { iter, .. } => vec![iter],
        Stmt::While { cond, .. } => vec![cond],
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => Vec::new(),
    }
}

/// An expression's immediate parts: the expressions it holds, and the blocks.
///
/// Every variant is named, so a variant added later is a compile error here
/// rather than a hole in the walk - the same argument [`Expr::LitInterpolated`]
/// makes about itself.
fn parts<'e>(expr: &'e Expr, children: &mut Vec<&'e Expr>, blocks: &mut Vec<&'e Block>) {
    match expr {
        Expr::MethodCall {
            receiver,
            args,
            config,
            ..
        }
        // The same parts: a `?.m()` holds a receiver, its arguments and its
        // config zone, and a view inside any of them is a view.
        | Expr::SafeMethod {
            receiver,
            args,
            config,
            ..
        } => {
            children.push(receiver);
            children.extend(args);
            children.extend(config.iter().map(|c| &c.value));
        }
        Expr::Call { func, args, config } => {
            children.push(func);
            children.extend(args);
            children.extend(config.iter().map(|c| &c.value));
        }
        // A safe reach borrows what a plain one does: whether it happens is
        // decided at run time, and a view analysis is about *what* is reached.
        Expr::Field { base, .. } | Expr::SafeField { base, .. } => children.push(base),
        Expr::Index { base, index } => {
            children.push(base);
            children.push(index);
        }
        Expr::Binary { lhs, rhs, .. } => {
            children.push(lhs);
            children.push(rhs);
        }
        Expr::Unary { expr, .. }
        | Expr::Try(expr)
        | Expr::Throw(expr)
        | Expr::Cast { expr, .. }
        | Expr::Spawn { body: expr, .. } => children.push(expr),
        Expr::Coalesce { value, fallback } => {
            children.push(value);
            children.push(fallback);
        }
        Expr::Range { start, end, .. } => {
            children.push(start);
            children.push(end);
        }
        Expr::Tuple(parts) | Expr::ListLit { items: parts, .. } => children.extend(parts),
        // `return`, `break` and `continue` as expressions
        // ([ADR-138](../../docs/specification/adr/adr-138.md) D1): a `return`
        // holds what it hands back, and the other two hold nothing.
        Expr::Return(value) => children.extend((**value).as_ref()),
        Expr::Break | Expr::Continue => {}
        Expr::StructLit { fields, .. } => {
            children.extend(fields.iter().filter_map(|f| f.value.as_ref()))
        }
        // A copy carries what it did not name, so the operand is a part of it
        // exactly as the written fields are
        // ([ADR-118](../../docs/specification/adr/adr-118.md) D3).
        Expr::With { base, fields, .. } => {
            children.push(base);
            children.extend(fields.iter().filter_map(|f| f.value.as_ref()));
        }
        Expr::Match { value, arms } => {
            children.push(value);
            children.extend(arms.iter().map(|a| &a.body));
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            children.push(cond);
            blocks.push(then_branch);
            blocks.extend(else_branch.as_ref());
        }
        Expr::Block(block)
        | Expr::Unsafe(block)
        | Expr::Overlap(block)
        | Expr::Closure { body: block, .. } => blocks.push(block),
        // An arm holds both: what is raced is an expression, and what runs if
        // it wins is a block ([ADR-148](../../docs/specification/adr/adr-148.md)
        // D1).
        Expr::Select(arms) => {
            for arm in arms {
                children.push(&arm.value);
                blocks.push(&arm.body);
            }
        }
        Expr::TryCatch { expr, handler } => {
            children.push(expr);
            blocks.push(handler);
        }
        Expr::Asm { .. }
        | Expr::Dsl { .. }
        | Expr::Path(_)
        | Expr::Variable(_)
        | Expr::LitInt { .. }
        | Expr::LitFloat(_)
        | Expr::LitStr { .. }
        | Expr::LitInterpolated(_)
        | Expr::LitChar(_)
        | Expr::LitBool(_)
        // `null` holds nothing, so it borrows nothing.
        | Expr::LitNull => {}
    }
}
