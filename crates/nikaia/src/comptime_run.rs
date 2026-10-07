//! **A `comptime` that calls a function is compiled and run**
//! ([ADR-321](../../../docs/specification/adr/adr-321.md) D1).
//!
//! The initialiser and everything it calls are lowered as the program is,
//! compiled for the machine that builds and run there; what the program prints
//! is the value. One implementation of the language computes the build-time
//! and the run-time answer, so an overflow stops the build where it would stop
//! the program, and a function of `std`'s Rust half runs as it does when the
//! program runs.
//!
//! **What is built here** (#468's first stage): one small program per
//! `comptime`, against `std` alone. It is linked against one dynamic library
//! that names `std` (D3, the *bundle*), compiled by `rustc` directly and run
//! once; its answer is kept under a key of the code that produced it (D4), so
//! an unchanged `comptime` is neither compiled nor run again - which is also
//! what keeps a lowering inside Cargo's own build from starting another.
//!
//! The program is the file's items without its `fn main` and without its
//! `comptime`s, then the `comptime`s already worked out, each as the literal it
//! came to, and a function that hands back the initialiser. Its `main` writes
//! the value in the encoding a grammar run already uses
//! ([`crate::grammar_run::decode`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::ast::{self, Block, Expr, Item, Span, Spanned, Stmt};
use crate::build_time::Value;
use crate::contracts::ty::Ty;
use crate::grammar_run::Wall;
use crate::parser::Parsed;

/// The name the sub-program hands the value back under.
pub(crate) const VALUE_FN: &str = "__nikaia_comptime_value";

/// **The sub-program for one initialiser**, or nothing where this stage does
/// not build one: a type with no written form, or an earlier `comptime` whose
/// value is not a literal.
///
/// `bound` is the `comptime` being worked out; `known` is what an earlier one
/// came to, by name, and nothing for one not worked out yet - which is left
/// out, as the one being worked out is.
pub(crate) fn sub_program(
    parsed: &Parsed,
    bound: &str,
    value: &Expr,
    ty: &Ty,
    known: &dyn Fn(&str) -> Option<Value>,
) -> Option<Parsed> {
    let mut kept = unit_without_its_constants(parsed, bound, known)?;
    let returns = written_type(ty, &kept)?;
    let main = kept.interner.intern_string("main");
    let value_fn = kept.interner.intern_string(VALUE_FN);
    kept.program.items.push(Spanned::new(
        a_function(main, None, Vec::new()),
        Span::nowhere(),
    ));
    let mut stmts = constants_as_lets(parsed, bound, known, &kept);
    stmts.push(Spanned::new(
        Stmt::Return(Some(value.clone())),
        Span::nowhere(),
    ));
    kept.program.items.push(Spanned::new(
        a_function(value_fn, Some(returns), stmts),
        Span::nowhere(),
    ));
    Some(kept)
}

/// **An earlier `comptime` that holds a `std` type's value**, as a `let` at
/// the head of the value function (ADR-318 D5, #519): `let A:
/// time::Duration = time::Duration::new(90, 0)`. Its constructor is not a
/// program's to write and an item cannot hold it, but this run is the
/// compiler's own, and a later `comptime` reads `A` here.
fn constants_as_lets(
    parsed: &Parsed,
    bound: &str,
    known: &dyn Fn(&str) -> Option<Value>,
    kept: &Parsed,
) -> Vec<Spanned<Stmt>> {
    let name = |text: &str| kept.interner.intern_string(text);
    parsed
        .program
        .items
        .iter()
        .filter_map(|item| {
            let Item::Comptime {
                name: bound_here,
                ty,
                ..
            } = &item.node
            else {
                return None;
            };
            let named = parsed.text(*bound_here);
            if named == bound {
                return None;
            }
            let Some(Value::Constant { constructor, parts }) = known(named) else {
                return None;
            };
            let args = parts
                .iter()
                .map(|part| literal_of(part, kept))
                .collect::<Option<Vec<_>>>()?;
            Some(Spanned::new(
                Stmt::Let {
                    names: vec![name(named)],
                    mutable: false,
                    ty: ty.clone(),
                    value: Expr::Call {
                        func: Box::new(Expr::Path(constructor.split("::").map(name).collect())),
                        args,
                        config: Vec::new(),
                    },
                },
                Span::nowhere(),
            ))
        })
        .collect()
}

/// **One file of the program as the sub-program carries it**: without its
/// `fn main`, and with each `comptime` that is not `bound` as the literal it
/// came to - or left out, where it has not been worked out. Nothing where one
/// that was worked out has no literal here.
pub(crate) fn unit_without_its_constants(
    parsed: &Parsed,
    bound: &str,
    known: &dyn Fn(&str) -> Option<Value>,
) -> Option<Parsed> {
    let mut kept = parsed.keeping(|item| match item {
        // **The program's `main` is not this program's**, and its constants
        // are carried below as what they came to.
        Item::Fn {
            name: Some(name), ..
        } => parsed.text(*name) != "main",
        Item::Comptime { .. } => false,
        _ => true,
    });
    for item in &parsed.program.items {
        let Item::Comptime {
            name,
            ty: declared,
            public,
            ..
        } = &item.node
        else {
            continue;
        };
        let named = parsed.text(*name);
        if named == bound {
            continue;
        }
        let Some(held) = known(named) else {
            continue;
        };
        // Carried by `constants_as_lets` (#519).
        if matches!(held, Value::Constant { .. }) {
            continue;
        }
        kept.program.items.push(Spanned::new(
            Item::Comptime {
                name: *name,
                ty: declared.clone(),
                value: literal_of(&held, &kept)?,
                public: *public,
                bounds: Vec::new(),
            },
            item.span,
        ));
    }
    Some(kept)
}

/// **A program of several files, lowered as one** (Part I 9.1): each file
/// against all of them, in order, as `modules::Program` lowers a package -
/// `entry` is the file that holds the value function and so the `fn main`.
pub(crate) fn lowered_units(
    units: &[Parsed],
    entry: usize,
    build: crate::emit::Build,
    contracts: &crate::contracts::Ledger,
    packages: &BTreeMap<String, crate::contracts::Ledger>,
    library: &crate::contracts::Ledger,
) -> Option<crate::emit::Lowered> {
    use crate::emit::{Lowered, Needs, SourceMap};
    let home = &units[entry].package;
    let beside: Vec<&Parsed> = units.iter().filter(|u| &u.package == home).collect();
    let provenance = crate::contracts::trust::analyse(&units[entry], library).provenance;
    let needs = units.iter().fold(Needs::default(), |acc, unit| {
        acc.join(Needs::of(unit, build))
    });
    let mut rust = String::from("// Generated: a `comptime`'s program (ADR-321).\n\n");
    rust.push_str(&needs.preamble());
    rust.push('\n');
    let mut map = SourceMap::default();
    for (at, unit) in units.iter().enumerate() {
        if &unit.package != home {
            continue;
        }
        let unit = &without_foreign(unit, contracts);
        let body = crate::emit::emit_module_body_at(
            unit,
            &beside,
            build,
            provenance,
            contracts,
            &crate::contracts::Ledger::blank(),
            at == entry,
            &crate::assets::Reads::none(),
        )
        .ok()?;
        map.extend(body.map.placed(rust.len(), at));
        rust.push_str(&body.rust);
        rust.push('\n');
    }
    let refs: Vec<&Parsed> = units.iter().collect();
    rust.push_str(&dependency_modules(&refs, packages, build, provenance)?);
    rust.push_str("\nconst __NIKAIA_SITES: &[nikaia_std::abort::Site] = &[];\n");
    Some(Lowered {
        rust,
        map,
        published: Default::default(),
    })
}

/// **A file without its foreign parts** (ADR-321 D14): no `extern` block, and
/// no function or method whose entry in `ledger` may not run while the
/// program is built - which is every one that reaches C or a Rust crate, and
/// what D9 refuses a build-time call to anyway. So the build-time program
/// neither links a foreign library nor names a Rust crate.
pub(crate) fn without_foreign(parsed: &Parsed, ledger: &crate::contracts::Ledger) -> Parsed {
    let runs = |key: String| {
        ledger
            .functions
            .get(&key)
            .is_none_or(|contract| crate::build_time::may_not_run(contract).is_none())
    };
    let mut kept = parsed.keeping(|item| match item {
        Item::Extern { .. } => false,
        Item::Fn {
            name: Some(name), ..
        } => runs(parsed.text(*name).to_string()),
        _ => true,
    });
    for item in &mut kept.program.items {
        if let Item::Impl {
            target, methods, ..
        } = &mut item.node
        {
            let target = parsed.text(target.name).to_string();
            methods.retain(|method| match &method.node {
                Item::Fn {
                    name: Some(name), ..
                } => runs(format!("{target}::{}", parsed.text(*name))),
                _ => true,
            });
        }
    }
    kept
}

/// **Each dependency, as the module the program names it by** (ADR-321 D13):
/// `lib::tripled` is `crate::lib::tripled`, lowered against the package's own
/// entries and with the prelude its files are written for. Empty where the
/// build reads no dependency.
pub(crate) fn dependency_modules(
    units: &[&Parsed],
    packages: &BTreeMap<String, crate::contracts::Ledger>,
    build: crate::emit::Build,
    provenance: crate::contracts::Provenance,
) -> Option<String> {
    let mut rust = String::new();
    for (name, own) in packages {
        let theirs: Vec<&Parsed> = units
            .iter()
            .copied()
            .filter(|u| u.package.as_deref() == Some(name.as_str()))
            .collect();
        rust.push_str(&format!(
            "pub mod {name} {{\n#[allow(unused_imports)]\nuse nikaia_std::prelude::*;\n"
        ));
        for unit in &theirs {
            let unit = &without_foreign(unit, own);
            let body = crate::emit::emit_module_body_at(
                unit,
                &theirs,
                build,
                provenance,
                own,
                &crate::contracts::Ledger::blank(),
                false,
                &crate::assets::Reads::none(),
            )
            .ok()?;
            rust.push_str(&body.rust);
            rust.push('\n');
        }
        rust.push_str("}\n");
    }
    Some(rust)
}

/// **Each dependency's units, derived as the package they are** (ADR-321
/// D13), by the name the program reaches it by.
pub(crate) fn by_package(units: &[&Parsed]) -> BTreeMap<String, crate::contracts::Ledger> {
    use crate::contracts::LedgerOps;
    let mut names: Vec<&str> = units.iter().filter_map(|u| u.package.as_deref()).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let theirs: Vec<&Parsed> = units
                .iter()
                .copied()
                .filter(|u| u.package.as_deref() == Some(name))
                .collect();
            (
                name.to_string(),
                crate::contracts::Ledger::infer_package(&theirs, crate::contracts::std_ledger()),
            )
        })
        .collect()
}

/// **What a compiled build-time run came to.**
pub(crate) enum Computed {
    /// The value.
    Value(Value),
    /// A callee the rule forbids (D2), named as the call resolves.
    Forbidden {
        callee: String,
        because: &'static str,
    },
    /// The run stopped, and what it said.
    Stopped(String),
    /// This stage builds no program for it, and the interpreter answers.
    NotHere,
}

/// What a call's left-out option stands for where its default is not known in
/// a run (#468): the emitter's placeholder, which only a refused default
/// reaches.
pub(crate) const NOT_COMPUTED_YET: &str = "a default not computed yet";

/// **A default's type as its run builds it** (#468): the value a view
/// views - `ref String`'s `String` - which the run hands back, and which is
/// written into the ledger the same way.
pub(crate) fn as_built(ty: Ty) -> Ty {
    match ty {
        // `ref String` is read as `str`.
        Ty::Named { name, args, .. } if name == "str" => Ty::Named {
            name: "String".to_string(),
            args,
            view: false,
        },
        Ty::Named { name, args, .. } => Ty::Named {
            name,
            args,
            view: false,
        },
        other => other,
    }
}

/// **Compile and run an initialiser** (ADR-321 D1): `value`, standing in
/// `files[here]`, of type `ty`, with every file of the program around it and
/// `known` for what earlier `comptime`s came to. The rule is asked first
/// (D2), of the ledger entry derived for the function that hands the value
/// back: every call it reaches, by name or on a value.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compute(
    files: &[&Parsed],
    here: usize,
    bound: &str,
    value: &Expr,
    ty: &Ty,
    known: &dyn Fn(&str) -> Option<Value>,
    library: &crate::contracts::Ledger,
    workshop: &crate::grammar_run::Workshop,
    bounds: Bounds,
    outer: Option<&crate::contracts::Ledger>,
) -> Computed {
    // **The sub-program's own inference and lowering compute no default
    // again**: they read the same functions, and a default compiled while a
    // default is compiled is the same question asked without end. Where the
    // program's ledger is at hand, they read what it recorded (#468).
    defaults_known_from(outer, || {
        defaults_compiled_in(None, || {
            computed(
                files, here, bound, value, ty, known, library, workshop, bounds,
            )
        })
    })
}

#[allow(clippy::too_many_arguments)]
fn computed(
    files: &[&Parsed],
    here: usize,
    bound: &str,
    value: &Expr,
    ty: &Ty,
    known: &dyn Fn(&str) -> Option<Value>,
    library: &crate::contracts::Ledger,
    workshop: &crate::grammar_run::Workshop,
    bounds: Bounds,
) -> Computed {
    use crate::contracts::LedgerOps;
    let Some(sub) = sub_program(files[here], bound, value, ty, known) else {
        return Computed::NotHere;
    };
    let mut sub = Some(sub);
    let mut units: Vec<Parsed> = Vec::with_capacity(files.len());
    for (at, file) in files.iter().enumerate() {
        let unit = match at == here {
            true => sub.take(),
            false => unit_without_its_constants(file, bound, known),
        };
        match unit {
            Some(unit) => units.push(unit),
            None => return Computed::NotHere,
        }
    }
    // **A dependency's functions are the program's to call** (ADR-321 D13):
    // each package's units are derived on their own, and the program's
    // against `std` and those entries, under the package's name - as the
    // program's own ledger has them.
    // **The run's home is the package of the file it stands in**: a default
    // of a dependency's function is computed in that dependency, where the
    // program's own files are not visible and the other packages are.
    let home = units[here].package.clone();
    let others: Vec<&Parsed> = units
        .iter()
        .filter(|u| u.package.is_some() && u.package != home)
        .collect();
    let packages = by_package(&others);
    let mut with_packages = library.clone();
    for (name, ledger) in &packages {
        with_packages.absorb(Some(name), ledger.clone());
    }
    let unit_refs: Vec<&Parsed> = units.iter().filter(|u| u.package == home).collect();
    let derived = crate::contracts::Ledger::infer_package(&unit_refs, &with_packages);
    if let Some(because) = derived
        .functions
        .get(VALUE_FN)
        .and_then(crate::build_time::may_not_run)
    {
        let callee =
            culprit(files[here], value, &derived, library).unwrap_or_else(|| bound.to_string());
        return Computed::Forbidden { callee, because };
    }
    let counted = crate::emit::Build {
        counts_steps: true,
        ..Default::default()
    };
    let lowered = match units.len() {
        1 => crate::emit::emit_program(&units[here], counted).ok(),
        _ => lowered_units(&units, here, counted, &derived, &packages, library),
    };
    let Some(lowered) = lowered else {
        return Computed::NotHere;
    };
    let Ok(dump) = crate::grammar_run::dumper(&units[here], ty) else {
        return Computed::NotHere;
    };
    let Some(program) = with_driver(&lowered, &dump, bounds) else {
        return Computed::NotHere;
    };
    match workshop.run_comptime(&program, bound) {
        Ok(text) => match crate::grammar_run::decode(&text) {
            Ok(computed) => Computed::Value(computed),
            Err(_) => Computed::NotHere,
        },
        // A default this run did not have is the default's own refusal,
        // said where it is written.
        Err(Wall::Refused { detail }) if detail.contains(NOT_COMPUTED_YET) => Computed::NotHere,
        Err(Wall::Refused { detail }) => Computed::Stopped(detail),
        Err(_) => Computed::NotHere,
    }
}

/// **The callee the rule forbids, named as the call resolves**: a function by
/// its name or its path, a method by its key (`Reader::read`), read off the
/// program's entries and then `std`'s.
fn culprit(
    parsed: &Parsed,
    value: &Expr,
    derived: &crate::contracts::Ledger,
    library: &crate::contracts::Ledger,
) -> Option<String> {
    let forbidden = |key: &str| {
        derived
            .functions
            .get(key)
            .or_else(|| library.functions.get(key))
            .is_some_and(|contract| crate::build_time::may_not_run(contract).is_some())
    };
    let mut found = None;
    crate::emit::visit_expr(value, &mut |expr: &Expr| {
        if found.is_some() {
            return;
        }
        let key = match expr {
            Expr::Call { func, .. } => match &**func {
                Expr::Variable(name) => Some(parsed.text(*name).to_string()),
                Expr::Path(path) => Some(
                    path.iter()
                        .map(|segment| parsed.text(*segment))
                        .collect::<Vec<_>>()
                        .join("::"),
                ),
                _ => None,
            },
            Expr::MethodCall { method, .. } | Expr::SafeMethod { method, .. } => {
                let method = format!("::{}", parsed.text(*method));
                derived
                    .functions
                    .keys()
                    .find(|key| key.ends_with(&method) && forbidden(key))
                    .cloned()
            }
            _ => None,
        };
        if let Some(key) = key.filter(|key| forbidden(key)) {
            found = Some(key);
        }
    });
    found
}

// --- the defaults a ledger records -------------------------------------------

thread_local! {
    /// **Where an option's default is compiled** while a ledger is inferred
    /// ([ADR-318](../../../docs/specification/adr/adr-318.md) D1, ADR-321 D1): the
    /// workshop of the build that reads the program, for as long as it reads it.
    /// The inference has no `Reads` of its own; a build sets this around the read,
    /// and a caller that sets nothing gets the interpreter, as before.
    ///
    /// **One per thread**, because the inference runs on the thread that reads:
    /// two builds side by side, as the tests are, each keep their own.
    static DEFAULTS: std::cell::RefCell<Option<std::sync::Arc<crate::grammar_run::Workshop>>> =
        const { std::cell::RefCell::new(None) };
}

/// Read a program with `at` as the workshop its defaults are compiled in.
pub fn defaults_compiled_in<R>(at: Option<&Path>, read: impl FnOnce() -> R) -> R {
    let workshop = at.map(|at| std::sync::Arc::new(crate::grammar_run::Workshop::at(at)));
    let before = DEFAULTS.with(|held| std::mem::replace(&mut *held.borrow_mut(), workshop));
    let out = read();
    DEFAULTS.with(|held| *held.borrow_mut() = before);
    out
}

thread_local! {
    /// **The defaults the program's ledger recorded**, while a run's
    /// sub-program is inferred (#468): the same functions, so the same
    /// values, read rather than computed a second time.
    static KNOWN: std::cell::RefCell<Option<crate::contracts::Ledger>> =
        const { std::cell::RefCell::new(None) };
}

/// Read with `outer`'s recorded defaults as the ones the inference takes.
fn defaults_known_from<R>(outer: Option<&crate::contracts::Ledger>, read: impl FnOnce() -> R) -> R {
    let before = KNOWN.with(|held| std::mem::replace(&mut *held.borrow_mut(), outer.cloned()));
    let out = read();
    KNOWN.with(|held| *held.borrow_mut() = before);
    out
}

/// What the program's ledger recorded for option `at` of `key` - qualified
/// by the package for one that is not the program's - where a run's
/// sub-program is being read and the ledger holds a value.
pub(crate) fn known_default(package: Option<&str>, key: &str, at: usize) -> Option<String> {
    KNOWN.with(|held| {
        let held = held.borrow();
        let ledger = held.as_ref()?;
        let key = match package {
            Some(package) => format!("{package}::{key}"),
            None => key.to_string(),
        };
        let text = &ledger
            .functions
            .get(&key)?
            .signature
            .as_ref()?
            .config
            .get(at)?
            .default;
        (!text.is_empty()).then(|| text.clone())
    })
}

/// The workshop defaults are compiled in now, where a build set one.
pub(crate) fn defaults_workshop() -> Option<std::sync::Arc<crate::grammar_run::Workshop>> {
    DEFAULTS.with(|held| held.borrow().clone())
}

fn a_function(
    name: winnow_grammar::Symbol,
    ret_type: Option<ast::Type>,
    stmts: Vec<Spanned<Stmt>>,
) -> Item {
    Item::Fn {
        name: Some(name),
        generics: Vec::new(),
        receiver: None,
        args: Vec::new(),
        config: Vec::new(),
        spread: None,
        ret_type,
        body: Block { stmts },
        is_sync: false,
        sync_by: Vec::new(),
        is_public: false,
        can_throw: false,
    }
}

/// **An earlier `comptime`'s value as the literal that writes it**: a number,
/// a truth value, text, and a list, a tuple, a struct or a variant of those.
/// The names a struct or a variant needs are interned in `parsed`, the file
/// the literal goes into.
fn literal_of(value: &Value, parsed: &Parsed) -> Option<Expr> {
    let name = |text: &str| parsed.interner.intern_string(text);
    let all = |items: &[Value]| -> Option<Vec<Expr>> {
        items.iter().map(|item| literal_of(item, parsed)).collect()
    };
    match value {
        Value::Int(n) => Some(Expr::LitInt {
            value: n.magnitude,
            negative: n.negative,
        }),
        Value::Bool(b) => Some(Expr::LitBool(*b)),
        Value::Float(f) => Some(Expr::LitFloat(format!("{f:?}"))),
        Value::Text(text) => Some(Expr::LitStr {
            text: crate::build_time::written(text),
            at: 0,
        }),
        Value::List(items) => Some(Expr::ListLit {
            items: all(items)?,
            at: 0,
        }),
        Value::Tuple(parts) => Some(Expr::Tuple(all(parts)?)),
        // **A `std` type's constructor is not a program's to write**
        // (ADR-318 D5): an item cannot hold one, and the value function
        // gets it as a `let` instead (`constants_as_lets`, #519).
        Value::Constant { .. } => None,
        Value::Struct { name: ty, fields } => Some(Expr::StructLit {
            name: name(ty),
            fields: fields
                .iter()
                .map(|(field, held)| {
                    Some(ast::FieldInit {
                        name: name(field),
                        value: Some(literal_of(held, parsed)?),
                    })
                })
                .collect::<Option<_>>()?,
        }),
        Value::Variant {
            ty,
            variant,
            payload,
        } => {
            let path = Expr::Path(vec![name(ty), name(variant)]);
            match payload.is_empty() {
                true => Some(path),
                false => Some(Expr::Call {
                    func: Box::new(path),
                    args: all(payload)?,
                    config: Vec::new(),
                }),
            }
        }
    }
}

/// `ty` as the sub-program writes it in the value function's signature.
fn written_type(ty: &Ty, parsed: &Parsed) -> Option<ast::Type> {
    let plain = |name: &str| ast::Type {
        name: parsed.interner.intern_string(name),
        generics: Vec::new(),
        is_view: false,
        is_tuple: false,
        is_nullable: false,
        code: Box::new(None),
        count: None,
        is_mut: false,
        is_slice: false,
        either: false,
    };
    match ty {
        Ty::Named { name, args, view } => {
            let mut out = plain(name);
            out.is_view = *view;
            out.generics = args
                .iter()
                .map(|arg| written_type(arg, parsed))
                .collect::<Option<_>>()?;
            Some(out)
        }
        Ty::Count(n) => {
            let mut out = plain(&n.to_string());
            out.count = Some(*n);
            Some(out)
        }
        Ty::Tuple(parts) => {
            let mut out = plain("");
            out.is_tuple = true;
            out.generics = parts
                .iter()
                .map(|part| written_type(part, parsed))
                .collect::<Option<_>>()?;
            Some(out)
        }
        Ty::Nullable(inner) => {
            let mut out = written_type(inner, parsed)?;
            out.is_nullable = true;
            Some(out)
        }
        _ => None,
    }
}

/// **The lowered sub-program with a `main` of its own**: the generated one
/// starts the runtime and runs the program's body, and this one reports a stop,
/// computes the value and writes it.
///
/// **A stop names the byte it came from**, where the program's own table names
/// a `.nika` line (ADR-300 D9): the sub-program has no file of its own, and the
/// checker turns the byte back into a place in the file it is checking. A row
/// is the generated line, [`STOPPED_AT`], and the byte the line's outermost
/// node starts at.
pub(crate) fn with_driver(
    lowered: &crate::emit::Lowered,
    dump: &str,
    bounds: Bounds,
) -> Option<String> {
    let rust = &lowered.rust;
    let start = rust.find("\nfn main() {\n")?;
    let end = start + 1 + rust[start + 1..].find("\n}\n")? + 3;
    // **The value is computed on a thread of its own**, with a stack that
    // holds `build_time::DEEPEST` calls, and against the budget (D7).
    let main = format!(
        "\nfn main() {{\n\
         \x20   nikaia_std::abort::report_in_nikaia_terms(__NIKAIA_SITES);\n\
         \x20   nikaia_bundle::LIVE.bound({memory});\n\
         \x20   let computed = std::thread::Builder::new()\n\
         \x20       .stack_size(nikaia_std::build_time::STACK)\n\
         \x20       .spawn(|| {{\n\
         \x20           nikaia_std::build_time::start({budget});\n\
         \x20           let value = {VALUE_FN}();\n\
         \x20           let mut out = String::new();\n\
         {dump}\
         \x20           out\n\
         \x20       }})\n\
         \x20       .expect(\"the build-time thread\")\n\
         \x20       .join();\n\
         \x20   match computed {{\n\
         \x20       Ok(out) => println!(\"{{out}}\"),\n\
         \x20       Err(_) => std::process::exit(101),\n\
         \x20   }}\n\
         }}\n",
        budget = bounds.steps,
        memory = bounds.bytes
    );
    let mut starts = vec![0usize];
    starts.extend(
        rust.char_indices()
            .filter(|(_, c)| *c == '\n')
            .map(|(i, _)| i + 1),
    );
    let mut rows: std::collections::BTreeMap<usize, (usize, usize)> =
        std::collections::BTreeMap::new();
    // A line below the `main` this replaces moves by what the new one adds.
    let lines = |text: &str| text.matches('\n').count() as isize;
    let moved = lines(&main) - lines(&rust[start..end]);
    for (generated, byte, unit) in lowered.map.rows() {
        if (start..end).contains(&generated) || byte == 0 {
            continue;
        }
        let line = match starts.binary_search(&generated) {
            Ok(i) => i + 1,
            Err(i) => i,
        };
        let line = match generated >= end {
            true => (line as isize + moved) as usize,
            false => line,
        };
        rows.entry(line).or_insert((unit, byte));
    }
    let mut table = String::from("const __NIKAIA_SITES: &[nikaia_std::abort::Site] = &[\n");
    for (line, (unit, byte)) in rows {
        table.push_str(&format!("    ({line}, \"{STOPPED_AT}{unit}\", {byte}),\n"));
    }
    table.push_str("];\n");
    let rest = &rust[end..];
    let empty = rest.find("const __NIKAIA_SITES")?;
    let after = empty + rest[empty..].find("];\n")? + 3;
    // **Nothing goes in front of the program**: the table maps its lines, and
    // one line more above them would name the wrong one.
    Some(format!(
        "{}{main}{}{table}{}{}\nextern crate nikaia_bundle;\n",
        &rust[..start],
        &rest[..empty],
        &rest[after..],
        crate::grammar_run::DUMP_HELPERS
    ))
}

/// **What a build-time run may spend** ([ADR-321](../../../docs/specification/adr/adr-321.md)
/// D7, D10): counted steps and bytes live at once. How a `comptime` raises
/// either is not decided (D11); a workshop carries them, so that this
/// compiler's own tests can ask for less.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub steps: u64,
    pub bytes: u64,
}

impl Default for Bounds {
    /// Ten billion steps (D7) and 4 GiB (D10).
    fn default() -> Bounds {
        Bounds {
            steps: 10_000_000_000,
            bytes: 4 << 30,
        }
    }
}

/// What a stop's row names instead of a file: the file's place among the
/// program's follows it, and then the byte.
pub(crate) const STOPPED_AT: &str = "@";

// --- compiling and running -----------------------------------------------------

/// **The dynamic library build-time code links against** (D3): one crate that
/// names `std` and the grammar runtime a program's `grammar` lowers to, built
/// by Cargo from the same compiled dependencies as anything else in the
/// workshop.
#[derive(Debug)]
pub(crate) struct Bundle {
    /// The library itself.
    dylib: PathBuf,
    /// Each crate it names, as Cargo compiled it, which `rustc` is pointed at
    /// so that a path `nikaia_std::…` resolves in every module; the code is
    /// the library's.
    rlibs: Vec<(String, PathBuf)>,
    /// Where the libraries it was built from are.
    deps: PathBuf,
    /// What changes when the library is rebuilt: part of every key.
    stamp: String,
}

/// **The bundle's one file**: `std`, and the allocator every build-time run
/// uses ([ADR-321](../../../docs/specification/adr/adr-321.md) D10). Here and
/// not in each run's program, because a program linked against `std`
/// dynamically uses the allocator of the library it links; `unsafe` because an
/// allocator is, and generated because `std` has none (ADR-218).
const BUNDLE: &str = "// GENERATED (ADR-321 D3, D10).\n\
pub extern crate nikaia_std;\n\
pub extern crate winnow;\n\
pub extern crate winnow_grammar;\n\
\n\
/// What every run has live, against the bound its program sets.\n\
pub static LIVE: nikaia_std::build_time::Counted = nikaia_std::build_time::Counted::new();\n\
\n\
struct Heap;\n\
\n\
// SAFETY: every request is the system allocator's, unchanged; this only counts.\n\
unsafe impl std::alloc::GlobalAlloc for Heap {\n\
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {\n\
        LIVE.take(layout.size());\n\
        unsafe { std::alloc::GlobalAlloc::alloc(&std::alloc::System, layout) }\n\
    }\n\
    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {\n\
        LIVE.take(layout.size());\n\
        unsafe { std::alloc::GlobalAlloc::alloc_zeroed(&std::alloc::System, layout) }\n\
    }\n\
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {\n\
        LIVE.give(layout.size());\n\
        unsafe { std::alloc::GlobalAlloc::dealloc(&std::alloc::System, ptr, layout) }\n\
    }\n\
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {\n\
        match size >= layout.size() {\n\
            true => LIVE.take(size - layout.size()),\n\
            false => LIVE.give(layout.size() - size),\n\
        }\n\
        unsafe { std::alloc::GlobalAlloc::realloc(&std::alloc::System, ptr, layout, size) }\n\
    }\n\
}\n\
\n\
#[global_allocator]\n\
static HEAP: Heap = Heap;\n";

/// The crates the bundle names: `std`, and what a `grammar` lowers to.
const NAMED: [&str; 3] = ["nikaia_std", "winnow", "winnow_grammar"];

/// Build the bundle, or say why it could not be built.
///
/// **Once per machine, not per project**: what it is depends on its manifest
/// (where `std` comes from), its one file and the toolchain, never on the
/// program - so it lives under the user cache, keyed by the first two, as the
/// compiled `std` of ADR-002 D4 does. Under each project's own workshop it was
/// built again for every new project, some forty crates (half a minute) before
/// the first `comptime` could run. Cargo's lock on the target directory is what
/// makes two builds at once wait for one another rather than collide.
pub(crate) fn bundle(_workshop: &Path) -> Result<Bundle, String> {
    let mut manifest = String::from(
        "# GENERATED. The library build-time code links against (ADR-321 D3).\n\n\
         [workspace]\n\n\
         [package]\nname = \"nikaia_bundle\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n\
         [lib]\npath = \"src/lib.rs\"\n\n[dependencies]\n",
    );
    for (name, value) in crate::project::runtime_dependencies_for(NAMED.join(" ").as_str()) {
        manifest.push_str(&format!("{name} = {value}\n"));
    }
    let key = orchestrator::cache::sha256_hex(format!("{manifest}\n{BUNDLE}").as_bytes());
    let at = orchestrator::cache::Layout::user_cache_dir()
        .join("build-time-bundle")
        .join(&key[..16]);
    let dir = at.join("bundle");
    std::fs::create_dir_all(dir.join("src")).map_err(|e| format!("{}: {e}", dir.display()))?;
    write_if_changed(&dir.join("Cargo.toml"), &manifest)?;
    write_if_changed(&dir.join("src").join("lib.rs"), BUNDLE)?;
    let built = Command::new(cargo())
        .current_dir(orchestrator::cache::Layout::where_tools_start())
        .args(["rustc", "--quiet", "--lib", "--crate-type", "dylib"])
        .arg("--message-format=json")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(at.join("target"))
        .args(["--", "-C", "prefer-dynamic"])
        .output()
        .map_err(|e| format!("running cargo: {e}"))?;
    if !built.status.success() {
        return Err(String::from_utf8_lossy(&built.stderr).to_string());
    }
    let mut dylib = None;
    let mut rlibs: Vec<(String, PathBuf)> = Vec::new();
    for line in String::from_utf8_lossy(&built.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        let files = message["filenames"].as_array().cloned().unwrap_or_default();
        let files = files.iter().filter_map(|f| f.as_str()).map(PathBuf::from);
        match message["target"]["name"].as_str() {
            Some(name) if NAMED.contains(&name) => {
                if let Some(rlib) = files
                    .clone()
                    .find(|f| f.extension().is_some_and(|e| e == "rlib"))
                {
                    rlibs.push((name.to_string(), rlib));
                }
            }
            Some("nikaia_bundle") => {
                dylib = files.clone().find(|f| {
                    f.extension()
                        .is_some_and(|e| e == std::env::consts::DLL_EXTENSION)
                })
            }
            _ => {}
        }
    }
    let (Some(dylib), true) = (dylib, rlibs.len() == NAMED.len()) else {
        return Err("cargo built the bundle and named no library".to_string());
    };
    let deps = rlibs[0]
        .1
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "no directory for std".to_string())?;
    let stamp = std::iter::once(&dylib)
        .chain(rlibs.iter().map(|(_, rlib)| rlib))
        .map(|file| {
            let meta = std::fs::metadata(file).map_err(|e| format!("{}: {e}", file.display()))?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            Ok(format!("{}:{}:{modified}", file.display(), meta.len()))
        })
        .collect::<Result<Vec<String>, String>>()?
        .join("\n");
    Ok(Bundle {
        dylib,
        rlibs,
        deps,
        stamp,
    })
}

/// **Compile and run one build-time program**, or hand back what it said when
/// it ran before: its standard output, or `Wall::Refused` with what it wrote
/// where it stopped. Keyed on the program, the toolchain and the bundle (D4).
pub(crate) fn run(at: &Path, bundle: &Bundle, program: &str, name: &str) -> Result<String, Wall> {
    let key =
        crate::assets::digest(format!("{program}\n{}\n{}", toolchain(), bundle.stamp).as_bytes());
    let dir = at.join("comptime").join(&key);
    let answered = dir.join("answer");
    let stopped = dir.join("stopped");
    if let Ok(answer) = std::fs::read_to_string(&answered) {
        return Ok(answer);
    }
    if let Ok(said) = std::fs::read_to_string(&stopped) {
        return Err(Wall::Refused { detail: said });
    }
    let did_not = |detail: String| Wall::DidNotBuild { detail };
    let source = dir.join("main.rs");
    write_if_changed(&source, program).map_err(did_not)?;
    let binary = dir.join("run");
    let compiled = Command::new(rustc())
        .current_dir(orchestrator::cache::Layout::where_tools_start())
        .args([
            "--edition=2024",
            "--crate-name",
            "comptime",
            "--crate-type",
            "bin",
        ])
        .args(["-C", "opt-level=0", "-C", "overflow-checks=on"])
        .args([
            "-C",
            "debug-assertions=off",
            "-C",
            "prefer-dynamic",
            "-A",
            "warnings",
        ])
        .arg("--extern")
        .arg(format!("nikaia_bundle={}", bundle.dylib.display()))
        .args(bundle.rlibs.iter().flat_map(|(name, rlib)| {
            ["--extern".to_string(), format!("{name}={}", rlib.display())]
        }))
        .arg("-L")
        .arg(format!("dependency={}", bundle.deps.display()))
        .arg("-o")
        .arg(&binary)
        .arg(&source)
        .output()
        .map_err(|e| did_not(format!("running rustc: {e}")))?;
    if !compiled.status.success() {
        return Err(did_not(
            String::from_utf8_lossy(&compiled.stderr).to_string(),
        ));
    }
    let mut libraries: Vec<PathBuf> = Vec::new();
    if let Some(dir) = bundle.dylib.parent() {
        libraries.push(dir.to_path_buf());
    }
    libraries.push(bundle.deps.clone());
    libraries.push(PathBuf::from(target_libdir()));
    if let Some(held) = std::env::var_os(LIBRARY_PATH) {
        libraries.extend(std::env::split_paths(&held));
    }
    let child = Command::new(&binary)
        .env("RUST_BACKTRACE", "0")
        .env(
            LIBRARY_PATH,
            std::env::join_paths(libraries).map_err(|e| did_not(e.to_string()))?,
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| did_not(format!("running {}: {e}", binary.display())))?;
    let ran =
        waited(child, name).map_err(|e| did_not(format!("running {}: {e}", binary.display())))?;
    if ran.status.success() {
        let answer = String::from_utf8_lossy(&ran.stdout).trim().to_string();
        write_if_changed(&answered, &answer).map_err(did_not)?;
        return Ok(answer);
    }
    let mut said = String::from_utf8_lossy(&ran.stderr).trim().to_string();
    // **A run that ended without a word** - killed by the system, most often
    // for its memory - says how it ended instead.
    if said.is_empty() {
        said = format!("it ended without saying why ({})", ran.status);
    }
    // **A stop the program reported is an answer**, and kept as one: the same
    // code stops the same way. A run that died without a word is not, and is
    // tried again next time.
    if said.contains("the program stopped") || said.contains("nikaia-build-time:") {
        write_if_changed(&stopped, &said).map_err(did_not)?;
    }
    Err(Wall::Refused { detail: said })
}

/// **The run, waited for, and named while it takes long**
/// ([ADR-321](../../../docs/specification/adr/adr-321.md) D12): after a few
/// seconds the build says which `comptime` is still running and for how long.
/// A message, never a bound.
fn waited(mut child: std::process::Child, name: &str) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut held = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut held);
        }
        held
    });
    let err = std::thread::spawn(move || {
        let mut held = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_end(&mut held);
        }
        held
    });
    let began = std::time::Instant::now();
    let mut next = std::time::Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if began.elapsed() >= next {
            eprintln!(
                "note: `{name}` is still being computed while the program is built ({} s)",
                began.elapsed().as_secs()
            );
            next += std::time::Duration::from_secs(10);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    Ok(std::process::Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// The variable the loader reads for where libraries are.
#[cfg(target_os = "macos")]
const LIBRARY_PATH: &str = "DYLD_LIBRARY_PATH";
#[cfg(windows)]
const LIBRARY_PATH: &str = "PATH";
#[cfg(not(any(target_os = "macos", windows)))]
const LIBRARY_PATH: &str = "LD_LIBRARY_PATH";

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn rustc() -> String {
    std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string())
}

/// `rustc -vV`, once: the toolchain a key is about.
fn toolchain() -> &'static str {
    static HELD: OnceLock<String> = OnceLock::new();
    HELD.get_or_init(|| {
        Command::new(rustc())
            .current_dir(orchestrator::cache::Layout::where_tools_start())
            .arg("-vV")
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_default()
    })
}

/// Where the toolchain's own `std` is, which a program linked with
/// `prefer-dynamic` loads.
fn target_libdir() -> &'static str {
    static HELD: OnceLock<String> = OnceLock::new();
    HELD.get_or_init(|| {
        Command::new(rustc())
            .current_dir(orchestrator::cache::Layout::where_tools_start())
            .args(["--print", "target-libdir"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default()
    })
}

/// Written only when it changed, so that Cargo's freshness keeps working.
fn write_if_changed(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    if std::fs::read_to_string(path).is_ok_and(|held| held == text) {
        return Ok(());
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}
