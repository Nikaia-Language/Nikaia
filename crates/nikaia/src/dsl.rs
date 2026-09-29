// crates/nikaia/src/dsl.rs
//
// The shadow type a DSL with deferred parameters generates (ADR-007 D5).
//
// `dsl mysql { … :id … :active } eod` is a statement with two holes that are
// not filled where it is written. The values arrive at the call site, as named
// arguments after the `;` - DSL parameters are *configuration*, and the Subject
// `;` Config protocol applies to metaprogramming without exception.
//
// For that to be checkable rather than hoped for, the body's holes have to
// become a **type**: one struct per parameter list, a field per `:name`, built
// where the parameters are supplied and never on the heap. Without it the
// compiler cannot say that a call forgot `:active`, which is the whole safety
// argument for embedding SQL rather than concatenating strings.
//
// Two rules govern what is here, and both are restrictions:
//
//   * the lowering is **syntactic** (ADR-011 D2). The names come from the body
//     as written; the *types* come from the arguments at the call site, because
//     nothing in the source says what `:id` is. The struct is therefore generic
//     in each field and monomorphised where it is built - the compiler never
//     invents `i32`.
//   * a hole is a hole because the source wrote `:name`, not because anything
//     was inferred about the foreign syntax around it. The statement's text
//     reaches the driver exactly as written, holes included: a deferred
//     parameter is not string interpolation, so the value may not be spliced
//     into the source text (Part III, 15.3).

use std::collections::BTreeMap;

use crate::ast::{Block, Expr, Item, Span, Stmt};
use crate::check::{Finding, Severity};
use crate::parser::Parsed;

/// The target the bootstrap compiler compiles itself, whose `:name` is an
/// **immediate** capture rather than a deferred parameter (ADR-017, ADR-007 D4).
const TEMPLATE: &str = "html";

/// The deferred parameters of one `dsl … { … } eod` body, in the order the body
/// first names them, and empty where it names none.
///
/// **Written in Nikaia** (0.0.248): `nikaia-std/src/tools/dsl.nika`, whose
/// comment says what the scan reads and what it cannot tell apart. This is the
/// call, kept so that the compiler's callers name a `dsl` question in `dsl`.
pub fn parameters(body: &str) -> Vec<String> {
    nikaia_std::tools::dsl::parameters(body)
}

/// Whether a `dsl <target> { … } eod` body's holes are deferred parameters.
///
/// `html` is the one target this compiler *is* the grammar for, and ADR-017
/// built its `:name` as an immediate capture. For every other target the holes
/// are the grammar author's business, and the only binding this compiler can
/// give them without inventing a meaning is the deferred one (ADR-007 D4).
pub fn is_deferred(target: &str, body: &str) -> bool {
    target != TEMPLATE && !parameters(body).is_empty()
}

/// The Rust name of the shadow type for a parameter list.
///
/// Derived from the names themselves, so the call site can write the struct
/// literal knowing only what it passes - a struct literal names its fields, so
/// the order a call writes them in does not have to match the body's.
pub fn type_name(parameters: &[String]) -> String {
    let mut names: Vec<&str> = parameters.iter().map(String::as_str).collect();
    names.sort_unstable();
    format!("NikaiaDslParams_{}", names.join("_"))
}

/// Every distinct shadow type a program needs, with the field order of the body
/// that first asked for it.
pub fn shadow_types(parsed: &Parsed) -> BTreeMap<String, Vec<String>> {
    let mut types = BTreeMap::new();
    for item in &parsed.program.items {
        for body in bodies(&item.node) {
            visit_block(body, &mut |expr| {
                if let Expr::Dsl {
                    target, content, ..
                } = expr
                {
                    let parameters = parameters(content);
                    if is_deferred(parsed.text(*target), content) {
                        types
                            .entry(type_name(&parameters))
                            .or_insert_with(|| parameters.clone());
                    }
                }
            });
        }
    }
    types
}

/// The functions of this program that accept a DSL's parameters, by name.
///
/// A call carrying a `;` is an ordinary options call unless the callee declared
/// the typed spread, and this is what tells the two apart. The name is the key
/// because it is what a call site has: the emitter resolves methods by name
/// everywhere else for the same reason.
pub fn drivers(parsed: &Parsed) -> Vec<String> {
    let mut names = Vec::new();
    let mut note = |item: &Item| {
        if let Item::Fn {
            name: Some(name),
            spread: Some(_),
            ..
        } = item
        {
            names.push(parsed.text(*name).to_string());
        }
    };
    for item in &parsed.program.items {
        match &item.node {
            Item::Impl { methods, .. } => methods.iter().for_each(|m| note(&m.node)),
            other => note(other),
        }
    }
    names
}

/// Every call site that supplies a DSL's parameters and gets them wrong.
///
/// Both halves of the check, because both are the same mistake seen from
/// opposite ends: a `:name` the body has and the call does not pass, and a name
/// the call passes and the body does not have. Reported against the `.nika`
/// statement they are in, like every other finding of this compiler (ADR-012).
pub fn check(parsed: &Parsed) -> Vec<Finding> {
    let mut findings = Vec::new();
    for item in &parsed.program.items {
        for body in bodies(&item.node) {
            check_block(parsed, body, &mut Bindings::default(), &mut findings);
        }
    }
    findings
}

/// What a name in this function is bound to, where it is a DSL statement.
#[derive(Debug, Default)]
struct Bindings {
    of: BTreeMap<String, Vec<String>>,
}

fn check_block(parsed: &Parsed, block: &Block, bound: &mut Bindings, out: &mut Vec<Finding>) {
    for stmt in &block.stmts {
        // **One name only**: a deferred `dsl` block is one value, so a tuple
        // destructure cannot be bound to one
        // ([ADR-098](../../../docs/specification/adr/adr-098.md)).
        if let Stmt::Let { names, value, .. } = &stmt.node {
            let [name] = names.as_slice() else {
                continue;
            };
            let text = parsed.text(*name).to_string();
            match value {
                Expr::Dsl {
                    target, content, ..
                } if is_deferred(parsed.text(*target), content) => {
                    bound.of.insert(text, parameters(content));
                }
                // Any other value: the name no longer stands for a statement.
                _ => {
                    bound.of.remove(&text);
                }
            }
        }

        stmt_exprs(&stmt.node).for_each(|expr| {
            visit_expr(expr, &mut |inner| {
                calls(parsed, inner, bound, &stmt.span, out)
            });
        });
    }
}

/// One call, checked where it names a statement this function bound.
fn calls(parsed: &Parsed, expr: &Expr, bound: &Bindings, span: &Span, out: &mut Vec<Finding>) {
    let (config, subjects): (&[crate::ast::ConfigArg], Vec<&Expr>) = match expr {
        Expr::MethodCall {
            receiver,
            args,
            config,
            ..
        }
        // Kap 5.1's zone reaches a `?.m()` for the same reason it reaches a
        // `.m()`: a `?.` decides whether the call happens, never what a call is
        // ([ADR-066](../../docs/specification/adr/adr-066.md)).
        | Expr::SafeMethod {
            receiver,
            args,
            config,
            ..
        } => (
            config,
            std::iter::once(&**receiver).chain(args.iter()).collect(),
        ),
        Expr::Call { args, config, .. } => (config, args.iter().collect()),
        _ => return,
    };
    if config.is_empty() {
        return;
    }

    // The statement is whichever subject of this call is one. A DSL statement
    // may be the receiver (`stm.execute(; …)`) or an argument
    // (`db.execute(stm; …)`): both spell the same protocol, subject before the
    // `;` and configuration after it, so neither needs a rule of its own.
    let Some((name, declared)) = subjects.iter().find_map(|subject| match subject {
        Expr::Variable(name) => {
            let text = parsed.text(*name);
            bound.of.get(text).map(|params| (text, params))
        }
        _ => None,
    }) else {
        return;
    };

    let passed: Vec<&str> = config.iter().map(|a| parsed.text(a.name)).collect();

    for parameter in declared {
        if !passed.contains(&parameter.as_str()) {
            out.push(Finding {
                severity: Severity::Error,
                span: *span,
                code: "NK1112",
                message: format!("`{name}` needs `:{parameter}`, and this call does not pass it"),
                notes: vec![format!(
                    "the statement's parameters are {}",
                    list(declared.iter().map(String::as_str))
                )],
                help: Some(format!("pass it after the `;`: `{parameter}: …`")),
            });
        }
    }

    for name_passed in &passed {
        if declared.iter().any(|d| d == name_passed) {
            continue;
        }
        let near = nearest(name_passed, declared);
        out.push(Finding {
            severity: Severity::Error,
            span: *span,
            code: "NK1113",
            message: format!("`{name}` has no parameter `:{name_passed}`"),
            notes: vec![match declared.is_empty() {
                true => format!("`{name}` is a statement with no parameters at all"),
                false => format!(
                    "the statement's parameters are {}",
                    list(declared.iter().map(String::as_str))
                ),
            }],
            help: Some(match near {
                Some(near) => format!("did you mean `{near}`?"),
                None => format!("write `:{name_passed}` in the statement, or drop it here"),
            }),
        });
    }
}

/// A name in `declared` that differs from `name` in one edit, if there is one.
fn nearest<'a>(name: &str, declared: &'a [String]) -> Option<&'a str> {
    declared
        .iter()
        .map(String::as_str)
        .find(|candidate| nikaia_std::tools::spelling::one_edit_apart(name, candidate))
}

fn list<'a>(names: impl Iterator<Item = &'a str>) -> String {
    names
        .map(|n| format!("`:{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The bodies of a top-level item: a function's, or every method of an `impl`.
fn bodies(item: &Item) -> Vec<&Block> {
    match item {
        Item::Fn { body, .. } => vec![body],
        Item::Impl { methods, .. } => methods
            .iter()
            .filter_map(|m| match &m.node {
                Item::Fn { body, .. } => Some(body),
                _ => None,
            })
            .collect(),
        Item::Test { body, .. } | Item::Bench { body, .. } => vec![body],
        _ => Vec::new(),
    }
}

fn stmt_exprs(stmt: &Stmt) -> impl Iterator<Item = &Expr> {
    let mut found: Vec<&Expr> = Vec::new();
    match stmt {
        Stmt::Let { value, .. } | Stmt::Comptime { value, .. } => found.push(value),
        Stmt::Expr(expr) | Stmt::Return(Some(expr)) => found.push(expr),
        Stmt::Assign { target, value, .. } => {
            found.push(target);
            found.push(value);
        }
        Stmt::For { iter, .. } => found.push(iter),
        Stmt::While { cond, .. } => found.push(cond),
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
    }
    found.into_iter()
}

/// Every expression of a block, statements and nested blocks alike.
fn visit_block(block: &Block, f: &mut impl FnMut(&Expr)) {
    for stmt in &block.stmts {
        for expr in stmt_exprs(&stmt.node) {
            visit_expr(expr, f);
        }
        if let Stmt::While { body, .. } | Stmt::For { body, .. } = &stmt.node {
            visit_block(body, f);
        }
    }
}

fn visit_expr(expr: &Expr, f: &mut impl FnMut(&Expr)) {
    f(expr);
    match expr {
        Expr::MethodCall { receiver, args, .. } | Expr::SafeMethod { receiver, args, .. } => {
            visit_expr(receiver, f);
            args.iter().for_each(|a| visit_expr(a, f));
        }
        Expr::Call { func, args, .. } => {
            visit_expr(func, f);
            args.iter().for_each(|a| visit_expr(a, f));
        }
        Expr::Block(block) | Expr::Overlap(block) => visit_block(block, f),
        Expr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            visit_expr(cond, f);
            visit_block(then_branch, f);
            if let Some(block) = else_branch {
                visit_block(block, f);
            }
        }
        Expr::Binary { lhs, rhs, .. } => {
            visit_expr(lhs, f);
            visit_expr(rhs, f);
        }
        Expr::Unary { expr, .. }
        | Expr::Try(expr)
        | Expr::Field { base: expr, .. }
        | Expr::SafeField { base: expr, .. } => visit_expr(expr, f),
        Expr::TryCatch { expr, handler } => {
            visit_expr(expr, f);
            visit_block(handler, f);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hole_is_a_colon_and_a_name() {
        assert_eq!(
            parameters("SELECT name FROM users WHERE id = :id AND active = :active"),
            ["id", "active"]
        );
    }

    #[test]
    fn a_path_and_a_time_are_not_holes() {
        assert!(parameters("std::io::write(x)").is_empty());
        assert!(parameters("at 12:30 sharp").is_empty());
    }

    #[test]
    fn a_repeated_hole_is_one_parameter() {
        assert_eq!(parameters("WHERE a = :id OR b = :id"), ["id"]);
    }

    #[test]
    fn the_type_name_does_not_depend_on_the_order_a_call_writes() {
        assert_eq!(
            type_name(&["id".to_string(), "active".to_string()]),
            type_name(&["active".to_string(), "id".to_string()])
        );
    }
}
