// crates/nikaia/src/types.rs
//
// Part I 2.2 and Part III C.1: **a type nothing declares is refused here rather
// than lowered** ([ADR-096](../../../docs/specification/adr/adr-096.md)).
//
// A *value* nothing declares has had `NK1117` since
// [ADR-051](../../../docs/specification/adr/adr-051.md) — *"nothing declares
// `q`"*. A type had nothing, so `let x: Widgit = 3` lowered verbatim and came
// back as `rustc`'s *"cannot find type `Widgit` in this scope"*, about a file
// nobody wrote. Part III C.1's rule held for one half of this language's names
// and not the other.
//
// **A walk of its own**, beside `dsl::check`, `views::check` and
// `traits::check`, and for the reason each of those is separate: it asks a
// question about a *written name* rather than about a value's type, so it needs
// the item tree and neither the scope stack nor the inference.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Block, Item, Span, Stmt, Type};
use crate::check::{Finding, Severity};
use crate::contracts::Ledger;
use crate::parser::Parsed;

/// Every type name written in this unit that nothing accounts for.
pub fn check(parsed: &Parsed, own: &Ledger, library: &Ledger) -> Vec<Finding> {
    let mut found = Vec::new();
    declared_once(parsed, &mut found);
    let known = known_names(parsed, own, library);
    let traits = known_traits(parsed, own, library);
    for item in &parsed.program.items {
        let mut here = known.clone();
        here.extend(parameters_of(parsed, &item.node));
        bounds_of(parsed, &item.node, &traits, &item.span, &mut found);
        item_types(parsed, &item.node, &here, &item.span, &mut found);
    }
    found
}

/// **The two bounds that ask what a type *is*** — Part II 10.3 and
/// [ADR-088](../../../docs/specification/adr/adr-088.md) D2.
///
/// `[T: Struct]` and `[T: Enum]` are not traits anybody declares and no
/// `impl` answers them: what answers is the **declaration**, which is the whole
/// of D2's *the bound is what makes the shape reachable*. They are language
/// words, like `sync` and `throws`, and they live in one constant because the
/// checker, the bound check and the emitter each have to know the same two
/// names — the emitter because Rust has no such trait, so a bound that reached
/// it would come back as *cannot find trait `Struct` in this scope*, about a
/// file nobody wrote ([Part III
/// C.1](../../../docs/specification/30-nikaia-tooling.md)).
///
/// **A program that declares one wins.** `trait Struct { … }` beside this is
/// an ordinary trait and every check reads the declaration first, so nothing
/// here takes a name away from a program that wanted it.
pub const SHAPE_BOUNDS: [&str; 2] = ["Struct", "Enum"];

/// The names a **bound** may have: a `trait` this unit declares, or one either
/// ledger records ([ADR-106](../../../docs/specification/adr/adr-106.md)),
/// or one of [`SHAPE_BOUNDS`].
///
/// A set of its own and not `known_names`, because the two questions are not
/// the same one: `[T: Summary]` asks for a trait where `x: Summary` asks for a
/// type, and a `struct` is not an answer to the first.
fn known_traits(parsed: &Parsed, own: &Ledger, library: &Ledger) -> BTreeSet<String> {
    let mut known: BTreeSet<String> = SHAPE_BOUNDS.iter().map(|n| n.to_string()).collect();
    for ledger in [own, library] {
        for key in ledger.traits.keys() {
            known.insert(key.clone());
            if let Some((_, last)) = key.rsplit_once("::") {
                known.insert(last.to_string());
            }
        }
    }
    // Declared in this very unit, which the ledger handed to this walk may not
    // carry - `check` runs per unit and a ledger is the program's.
    for item in &parsed.program.items {
        if let Item::Trait { name, .. } = &item.node {
            known.insert(parsed.text(*name).to_string());
        }
    }
    known
}

/// `NK1135` one position over: **a bound naming a trait nothing declares**.
///
/// `fn describe[T: Struct](value: T)` lowered to `fn describe<T: Struct>(…)` and
/// came back from the backend as *cannot find trait `Struct` in this scope*,
/// which is [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s
/// class and exactly what `NK1135` was built to close for a written type.
///
/// **Qualified names are left alone**, for that refusal's own reason: whether
/// this build can see the package a bound names is a question with its own
/// message.
fn bounds_of(
    parsed: &Parsed,
    item: &Item,
    traits: &BTreeSet<String>,
    span: &Span,
    out: &mut Vec<Finding>,
) {
    let generics = match item {
        Item::Fn { generics, .. } | Item::Struct { generics, .. } => generics.as_slice(),
        _ => return,
    };
    for parameter in generics {
        for bound in &parameter.bounds {
            let name = parsed.text(*bound);
            if !name.contains("::") && !traits.contains(name) {
                out.push(nothing_declares_a_trait(name, span));
            }
        }
    }
}

/// The message [`bounds_of`] raises, under `NK1135`'s code because it is the
/// same claim: a name written where a declaration has to exist, and none does.
fn nothing_declares_a_trait(name: &str, span: &Span) -> Finding {
    Finding {
        severity: Severity::Error,
        span: *span,
        code: "NK1135",
        message: format!("nothing declares the trait `{name}`"),
        notes: vec![
            "a bound names a trait - what a caller's type has to implement (Part I, 4.7) - and this name is not one this program declares with `trait`, nor one a ledger records (ADR-106)"
                .to_string(),
        ],
        help: Some(format!(
            "declare `{name}` with `trait`, or leave the bound off - a parameter without one may be moved and passed and nothing else (ADR-074 D5)"
        )),
        labels: Vec::new(),
    }
}

/// **`NK1148`: a name declared twice in one file**
/// ([ADR-144](../../../docs/specification/adr/adr-144.md) D1).
///
/// Across files this is `modules::one_namespace`, which has something this does
/// not — two paths to name ([ADR-047](../../../docs/specification/adr/adr-047.md)
/// D1). Inside one file nothing said it, unless the build happened to go through
/// a manifest: `nikaia --input` skips the module layer, and that is the path the
/// corpus, the specification's blocks and a reader's first program all take. So
/// `struct Foo` beside `fn Foo` lowered, and `rustc` answered `E0428` about a
/// file nobody wrote ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
///
/// **Here and not in the module layer**, because every build runs the checker
/// and only some builds run that.
fn declared_once(parsed: &Parsed, out: &mut Vec<Finding>) {
    let mut seen: BTreeMap<String, &'static str> = BTreeMap::new();
    for item in &parsed.program.items {
        let Some((name, kind)) = declares(parsed, &item.node) else {
            continue;
        };
        match seen.get(&name) {
            // **The caret is on the second**, because that is the one that
            // arrived and the one to move; the note names the first.
            Some(first) => out.push(declared_twice(&name, first, kind, &item.span)),
            None => {
                seen.insert(name, kind);
            }
        }
    }
}

/// What declares a name, and what it is called in the message
/// ([ADR-144](../../../docs/specification/adr/adr-144.md) D2).
///
/// Five items and no more. A **method** belongs to its type and two types may
/// each have a `len`; a **rule** belongs to its grammar and is reached as
/// `Json::value`; a field, a variant, a type parameter and an `impl` block
/// declare nothing at this level.
fn declares(parsed: &Parsed, item: &Item) -> Option<(String, &'static str)> {
    match item {
        Item::Fn { name, .. } => name.map(|name| (parsed.text(name).to_string(), "fn")),
        Item::Struct { name, .. } => Some((parsed.text(*name).to_string(), "struct")),
        Item::Enum { name, .. } => Some((parsed.text(*name).to_string(), "enum")),
        Item::Trait { name, .. } => Some((parsed.text(*name).to_string(), "trait")),
        Item::Grammar(def) => Some((parsed.text(def.name).to_string(), "grammar")),
        _ => None,
    }
}

/// The message [`declared_once`] raises. Two of a kind get the same sentence as
/// two of different kinds, because the reader is doing the same thing either
/// way: finding out which of two declarations a line means.
fn declared_twice(name: &str, first: &str, second: &str, span: &Span) -> Finding {
    let kinds = match first == second {
        true => format!("both as a `{first}`"),
        false => format!("as a `{first}` and as a `{second}`"),
    };
    Finding {
        severity: Severity::Error,
        span: *span,
        code: "NK1148",
        message: format!("`{name}` is declared twice in this file: {kinds}"),
        notes: vec![
            "a name denotes one thing (ADR-144 D1), so a line that writes it has one \
             meaning and no rule is needed about which declaration wins - the files of a \
             package share one namespace for the same reason (Part I, 9.1)"
                .to_string(),
        ],
        help: Some(format!(
            "rename one of the two - a `struct` and its anonymous constructor are already \
             one name, so `{name}` cannot also be a function of its own"
        )),
        labels: Vec::new(),
    }
}

/// The names a written type may have.
///
/// **Built from what the compiler already knows rather than from a list**, and
/// that is the whole of why this can be answered at all: `OFFERED` is Part I
/// 2.2's own set, the ledger's `types` map is every type this program and
/// `std` declare, and the rest are the names the emitter itself writes. A list
/// maintained beside those would be a second thing to forget.
fn known_names(parsed: &Parsed, own: &Ledger, library: &Ledger) -> BTreeSet<String> {
    let mut known: BTreeSet<String> = BUILT_IN.iter().map(|n| n.to_string()).collect();
    // **`std`'s types by the key and nothing else**, since
    // [ADR-154](../../../docs/specification/adr/adr-154.md): a type that lives in
    // a module is written with it, `fs::Mapped` and `collections::HashMap`, and
    // what needs no prefix is the list on Part I 1.3 — whose names are keyed
    // **bare** there, so the key *is* the rule. It used to insert the last
    // segment too, which is name-for-name resolution and is what made the
    // prelude a set nobody could state.
    known.extend(library.types.keys().cloned());
    // **This program's own, by the key and by the last segment**, and that is
    // not the same question: a package's ledger keys its types with the
    // package's name (`http::Request`) and the files **of that package** write
    // them bare, because they share one namespace
    // ([ADR-047](../../../docs/specification/adr/adr-047.md) D1). A consumer
    // writes the prefix, which the key answers.
    for key in own.types.keys() {
        known.insert(key.clone());
        if let Some((_, last)) = key.rsplit_once("::") {
            known.insert(last.to_string());
        }
    }
    // A type declared in this very unit, which the ledger handed to this walk
    // may not carry: `check` runs per unit and a ledger is the program's.
    known.extend(crate::contracts::declared_types(parsed));
    known
}

/// The names the emitter writes that no ledger declares.
///
/// Part I 2.2's numeric surface beyond the four `OFFERED` names
/// ([ADR-054](../../../docs/specification/adr/adr-054.md) D1 is about what `as`
/// may *name*, which is a smaller set than what may be *written*), the
/// collections the prelude provides, and the hulls.
const BUILT_IN: &[&str] = &[
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
    "f32",
    "f64",
    "bool",
    "char",
    "String",
    "str",
    "Self",
    "Vec",
    "Shared",
    "SharedMut",
    "Locked",
    "TaskHandle",
    // **`Array[T, N]`** ([ADR-152](../../../docs/specification/adr/adr-152.md)
    // D1), which is a name here for the same reason `Vec` is: nothing declares
    // it in a `.nika` file and the emitter writes it (`[T; N]`).
    "Array",
    // **`Bytes`** ([ADR-156](../../../docs/specification/adr/adr-156.md) D1),
    // which is the language's and not `std`'s: it is written bare, like `Vec`
    // and `String`, because a type whose representation the compiler picks is
    // not a type a module owns. Part I 1.3 has listed it since
    // [ADR-154](../../../docs/specification/adr/adr-154.md) D1; this is the
    // line that makes the list true.
    "Bytes",
];

/// The type parameters an item brings into scope for its own body.
fn parameters_of(parsed: &Parsed, item: &Item) -> BTreeSet<String> {
    match item {
        Item::Fn { generics, .. } | Item::Struct { generics, .. } => generics
            .iter()
            .map(|g| parsed.text(g.name).to_string())
            .collect(),
        Item::Impl { target, .. } => {
            let declared = crate::contracts::declared_types(parsed);
            crate::contracts::impl_parameters(parsed, target, &declared)
                .into_iter()
                .collect()
        }
        _ => BTreeSet::new(),
    }
}

fn item_types(
    parsed: &Parsed,
    item: &Item,
    known: &BTreeSet<String>,
    span: &Span,
    out: &mut Vec<Finding>,
) {
    match item {
        Item::Fn {
            args,
            ret_type,
            body,
            ..
        } => {
            for arg in args {
                written_at(parsed, &arg.ty, known, Position::Parameter, span, out);
            }
            if let Some(ret) = ret_type {
                written(parsed, ret, known, span, out);
            }
            block_types(parsed, body, known, out);
        }
        Item::Struct { fields, .. } => {
            for field in fields {
                written(parsed, &field.ty, known, span, out);
            }
        }
        Item::Enum { variants, .. } => {
            for variant in variants {
                match &variant.fields {
                    crate::ast::VariantFields::Unit => {}
                    crate::ast::VariantFields::Tuple(parts) => {
                        for ty in parts {
                            written(parsed, ty, known, span, out);
                        }
                    }
                    crate::ast::VariantFields::Named(fields) => {
                        for field in fields {
                            written(parsed, &field.ty, known, span, out);
                        }
                    }
                }
            }
        }
        // **An `extern "C"` declaration's types are written types**
        // ([ADR-124](../../docs/specification/adr/adr-124.md) D1), and the
        // reason this arm has to exist is Part III 15.1's own example:
        // `Pointer[u8]` is a type nothing declares, and without this it reached
        // `rustc` intact and came back about a file nobody wrote — which is the
        // whole class `NK1135` was built for.
        //
        // `Position::Parameter` for the arguments, as a function's are: a
        // declaration is a signature, and the run-or-kept question `NK1142`
        // asks is about where the type stands rather than about whose body it
        // is in.
        Item::Extern { declarations, .. } => {
            for declaration in declarations {
                for arg in &declaration.node.args {
                    written_foreign(
                        parsed,
                        &arg.ty,
                        known,
                        Position::Parameter,
                        &declaration.span,
                        out,
                    );
                }
                if let Some(ret) = &declaration.node.ret_type {
                    written_foreign(
                        parsed,
                        ret,
                        known,
                        Position::Elsewhere,
                        &declaration.span,
                        out,
                    );
                }
            }
        }
        Item::Trait { methods, .. } => {
            for method in methods {
                let mut here = known.clone();
                here.extend(
                    method
                        .node
                        .generics
                        .iter()
                        .map(|g| parsed.text(g.name).to_string()),
                );
                for arg in &method.node.args {
                    written_at(
                        parsed,
                        &arg.ty,
                        &here,
                        Position::Parameter,
                        &method.span,
                        out,
                    );
                }
                if let Some(ret) = &method.node.ret_type {
                    written(parsed, ret, &here, &method.span, out);
                }
            }
        }
        Item::Impl { methods, .. } => {
            for method in methods {
                let mut here = known.clone();
                here.extend(parameters_of(parsed, &method.node));
                item_types(parsed, &method.node, &here, &method.span, out);
            }
        }
        _ => {}
    }
}

fn block_types(parsed: &Parsed, block: &Block, known: &BTreeSet<String>, out: &mut Vec<Finding>) {
    for stmt in &block.stmts {
        if let Stmt::Let {
            ty: Some(ty),
            value,
            ..
        } = &stmt.node
        {
            written(parsed, ty, known, &stmt.span, out);
            let _ = value;
        }
        let mut inner: Vec<&Block> = Vec::new();
        crate::contracts::sync::visit_stmt_blocks(&stmt.node, &mut |b| inner.push(b));
        for b in inner {
            block_types(parsed, b, known, out);
        }
    }
}

/// One written type, and the arguments inside it.
///
/// **A tuple has no name**, so it is skipped and its parts are walked — the AST
/// puts them where the arguments go, and reading `name` there would read a
/// token nobody wrote as a type.
/// Where a written type stands, for the one rule that cares
/// ([ADR-102](../../../docs/specification/adr/adr-102.md) D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
    /// A parameter of a function, a method or a trait method.
    Parameter,
    /// A struct field, an enum variant's part, a result, a `let`'s annotation —
    /// every position a *kept* function type would stand in.
    Elsewhere,
}

fn written(
    parsed: &Parsed,
    ty: &Type,
    known: &BTreeSet<String>,
    span: &Span,
    out: &mut Vec<Finding>,
) {
    written_at(parsed, ty, known, Position::Elsewhere, span, out)
}

/// The same, where the type stands in an `extern "C"` declaration
/// ([ADR-147](../../../docs/specification/adr/adr-147.md) D1) — the one place
/// this language writes `&mut` and `&[T]`.
fn written_foreign(
    parsed: &Parsed,
    ty: &Type,
    known: &BTreeSet<String>,
    at: Position,
    span: &Span,
    out: &mut Vec<Finding>,
) {
    if let Some(element) = ty.generics.first().filter(|_| ty.is_slice) {
        written(parsed, element, known, span, out);
        return;
    }
    let pointed = Type {
        is_mut: false,
        ..ty.clone()
    };
    written_at(parsed, &pointed, known, at, span, out)
}

fn written_at(
    parsed: &Parsed,
    ty: &Type,
    known: &BTreeSet<String>,
    at: Position,
    span: &Span,
    out: &mut Vec<Finding>,
) {
    // **`&mut` is the C boundary's and nowhere else's**
    // ([ADR-147](../../../docs/specification/adr/adr-147.md) D1): a parameter
    // this language may change is written `mut name: T`
    // ([ADR-094](../../../docs/specification/adr/adr-094.md) D3), so the form
    // is refused outside an `extern "C"` declaration rather than handed to the
    // language below — which is where `written_foreign` above lets it through.
    //
    // **`&[T]` is not**, since 0.0.127
    // ([ADR-179](../../../docs/specification/adr/adr-179.md) D1). It used to
    // be, on the reasoning that *a run of elements whose length the type does
    // not carry has no lowering here* — and that was true of a declaration and
    // never of a **view**: `&str` is exactly such a run and is the type Part I
    // 2.2 already gives text. What a `&[T]` away from the boundary lowers to
    // is Rust's own `&[T]`, a fat pointer that carries its length; what it
    // lowers to **at** the boundary is still D1's address beside D2's count,
    // and that difference is why the two have separate writers.
    //
    // `&mut [T]` is refused for the `mut` rather than for the run, which is
    // what the message now says.
    if ty.is_mut {
        out.push(only_at_the_c_boundary(false, span));
        return;
    }
    // **An `Array[T]` with no count is an array of any length, and where the
    // length comes from is the position**
    // ([ADR-184](../../../docs/specification/adr/adr-184.md) D3).
    //
    // In a **parameter** it comes from the call: *beliebig, aber fest* — every
    // call knows its own length, so the type has a size there and the function
    // is generic over it. In a **field** or a **result** there is no call to
    // ask, and the two readings it could have are both something else: a struct
    // generic over the length makes two `Section`s of different lengths two
    // **types**, which is not what a parser produces
    // ([ADR-179](../../../docs/specification/adr/adr-179.md)'s own case), and a
    // result's length would be bound by nothing.
    //
    // **Before the arguments are walked**, because the element is a type and
    // is not what is wrong here.
    if at == Position::Elsewhere
        && !ty.is_view
        && !ty.is_slice
        && ty.count.is_none()
        && ty.generics.len() == 1
        && parsed.text(ty.name) == crate::contracts::ty::ARRAY
    {
        out.push(a_length_nothing_here_can_give(parsed, ty, span));
        return;
    }
    // **A function type is not a name**
    // ([ADR-102](../../../docs/specification/adr/adr-102.md) D1), the same way
    // a tuple is not: what `fn` holds is a shape, and its parameters are in
    // `generics` where a tuple's parts are. Reading `name` here would report
    // that nothing declares a type called `fn`.
    // **A count is not a name either**
    // ([ADR-152](../../../docs/specification/adr/adr-152.md) D1): the `3` of
    // `Array[f64, 3]` is an argument of the type and nothing declares a type
    // called `3`. The walk over the arguments below reaches it, so the silence
    // has to be here rather than at the one position that writes one.
    if ty.count.is_some() {
        return;
    }
    // **And a slice is not a name**
    // ([ADR-147](../../../docs/specification/adr/adr-147.md) D1): `&[u8]` is a
    // shape, its element is walked below, and `slice` is the word this compiler
    // interns for it rather than a type anybody declares. Reading `name` here
    // would report that nothing declares a type called `slice` - which is
    // exactly what it did.
    if ty.is_slice {
        for argument in &ty.generics {
            written(parsed, argument, known, span, out);
        }
        return;
    }
    if !ty.is_tuple && ty.code.is_none() {
        let name = parsed.text(ty.name);
        if !known.contains(name) && !name.contains("::") {
            out.push(nothing_declares(name, known, span));
        }
    }
    for argument in &ty.generics {
        written(parsed, argument, known, span, out);
    }
    if let Some(code) = &ty.code
        && let Some(result) = &code.result
    {
        written(parsed, result, known, span, out);
    }
}

/// `NK1158`: `&mut` or `&[T]` outside an `extern "C"` declaration
/// ([ADR-147](../../../docs/specification/adr/adr-147.md) D1).
///
/// Both forms exist for the C boundary and have no meaning away from it. A
/// parameter this language may change is written `mut name: T`
/// ([ADR-094](../../../docs/specification/adr/adr-094.md) D3) — the word goes in
/// front of the **name**, because what it decides is also what the caller sees —
/// and a run of elements whose length the type does not carry is `Vec[T]` or
/// `Array[T, N]` here, both of which know how long they are.
///
/// **Refused rather than lowered**, which is this compiler's choice everywhere
/// ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)): what
/// `rustc` would say about `&mut i64` in a generated signature is about a file
/// nobody wrote.
/// **`NK1182`: an `Array[T]` here has no length, and nothing in this position
/// can give it one** ([ADR-184](../../../docs/specification/adr/adr-184.md)
/// D3).
///
/// `Array[T]` is an array of **any** length and every use of it has **one**:
/// in a parameter the call is what says which, and the function is generic over
/// it. A field and a result have no call to ask.
///
/// **Both ways out are named**, because which one the writer meant is not
/// something this compiler can know and each is a different program: a `ref`
/// looks at elements somebody else keeps, a `Vec` owns them and may grow. A
/// message with one of the two would send half its readers the wrong way
/// ([Part III C.2](../../../docs/specification/30-nikaia-tooling.md)).
///
/// **And the third is named too**, because for a *field* it is often what was
/// meant: writing the length down makes it an
/// [ADR-152](../../../docs/specification/adr/adr-152.md) D4 array laid out
/// inline.
///
/// **Without it the language below answers**, with *the size for values of
/// type `[i64]` cannot be known at compilation time* about a file nobody
/// wrote — the sentence [ADR-182](../../../docs/specification/adr/adr-182.md)
/// D2 had just finished removing one construct over.
fn a_length_nothing_here_can_give(parsed: &Parsed, ty: &Type, span: &Span) -> Finding {
    let element = ty
        .generics
        .first()
        .map(|g| parsed.text(g.name).to_string())
        .unwrap_or_else(|| "T".to_string());
    Finding {
        severity: Severity::Error,
        span: *span,
        code: "NK1182",
        message: format!("`Array[{element}]` here has no length to take"),
        notes: vec![format!(
            "`Array[{element}]` is an array of any length, and every use of it has one: in a \
             **parameter** the call says which, and the function is generic over it (Part I, \
             2.2). A field and a result have no call to ask, and the two readings left are \
             something else - a struct generic over its length makes two of different lengths \
             two types, and a result's length would be bound by nothing"
        )],
        help: Some(format!(
            "write `Vec[{element}]` to own elements and be able to grow, `ref Array[{element}]` \
             to look at elements somebody else keeps, or `Array[{element}, N]` to write the \
             length down and have them laid out inline"
        )),
        labels: Vec::new(),
    }
}

fn only_at_the_c_boundary(slice: bool, span: &Span) -> Finding {
    let (what, message, help) = match slice {
        true => (
            "a run of elements",
            "`[T]` is a type only at the C boundary",
            "write `Vec[T]`, or `Array[T, N]` where the length is known while \
             the program is built - both carry their length, which `[T]` does not",
        ),
        false => (
            "a view that may be written through",
            "`&mut` is a type only at the C boundary",
            "write the `mut` in front of the **name** instead - `fn fill(mut out: Vec[i64])` \
             is where in-place change is written (Part I, 2.1)",
        ),
    };
    Finding {
        severity: Severity::Error,
        span: *span,
        code: "NK1158",
        message: message.to_string(),
        notes: vec![format!(
            "{what} is what an `extern \"C\"` declaration lends C, and it lives for \
             the call (ADR-147 D1); away from that boundary this language has its own \
             words for both"
        )],
        help: Some(help.to_string()),
        labels: Vec::new(),
    }
}

/// `NK1135`: a written type name nothing accounts for.
///
/// **Qualified names are left alone**, and that is deliberate rather than
/// unfinished: `http::Response` names a package's type, and whether this build
/// can see that package is [ADR-046](../../../docs/specification/adr/adr-046.md)
/// D2's question with its own message. Saying *"nothing declares it"* about a
/// name a dependency does declare would be
/// [Part III C.4](../../../docs/specification/30-nikaia-tooling.md) — a correct
/// program refused — which is the one thing this may not do.
/// The modules of `std` a type may live in
/// ([ADR-154](../../../docs/specification/adr/adr-154.md) D3).
///
/// Written out because the help says `use std::…`, and that sentence is only
/// true of `std`: a package's own prefix is the package's name and comes with a
/// message of its own. Read off the ledger keys the compiler already has, so a
/// module `std` grows is one line here and not a second list.
const STD_MODULES: &[&str] = &[
    "cli",
    "collections",
    "channel",
    "foreign",
    "fs",
    "io",
    "time",
];

fn nothing_declares(name: &str, known: &BTreeSet<String>, span: &Span) -> Finding {
    // **Where a ledger has the type in a module, the name is not missing — the
    // prefix is** ([ADR-154](../../../docs/specification/adr/adr-154.md) D3).
    // `HashMap` is the case the record is named for: it is
    // `collections::HashMap`, and the help is the two lines that make it one.
    let in_a_module = known
        .iter()
        .filter(|key| key.ends_with(&format!("::{name}")))
        .find_map(|key| key.split_once("::").map(|(module, _)| module.to_string()))
        .filter(|module| STD_MODULES.contains(&module.as_str()));
    Finding {
        severity: Severity::Error,
        span: *span,
        code: "NK1135",
        message: match &in_a_module {
            Some(_) => format!("`{name}` is written without its module"),
            None => format!("nothing declares the type `{name}`"),
        },
        notes: vec![match &in_a_module {
            Some(module) => format!(
                "`{module}::{name}` is what it is called, and a type that lives in a module is \
                 reached through it - what needs no prefix is the list on Part I's first page \
                 (Part I, 1.3)"
            ),
            None => "a type is one Part I 2.2 offers, one this program declares with `struct` or \
                     `enum`, one `std` publishes, or a parameter the declaration around it names \
                     (Part I, 2.2)"
                .to_string(),
        }],
        help: Some(match &in_a_module {
            Some(module) => format!(
                "write `use std::{module}` at the top of the file, and `{module}::{name}` here"
            ),
            None => format!(
                "declare `{name}` with `struct` or `enum`, or write a type that exists - a \
                 misspelling is the usual cause"
            ),
        }),
        labels: Vec::new(),
    }
}
