//! An argument list of options alone
//! ([ADR-133](../../../docs/specification/adr/adr-133.md)).
//!
//! Part I 5.1 separates a call's **subjects** from its **options** with a `;`,
//! and a function whose only parameters are options was declared
//! `fn execute(; target_age: i64 = 0)` — a semicolon with nothing before it, a
//! shape no reader has seen in any language. The `;` earns its place where it
//! separates two zones; where there is only one zone it separates nothing.
//!
//! **Both halves are built now**, and the call half took a second record to
//! unblock. D3 says *nothing in expression position begins with a name followed
//! by a colon*, and Kap 4.2's struct literal with named fields —
//! `Stats(min: first, max: first)` — was exactly that shape, so this form was
//! read as a literal for a struct nothing declares.
//! [ADR-140](../../../docs/specification/adr/adr-140.md) D1 takes that spelling
//! out of the language: `Name { … }` is the literal and `name(field: value)` is
//! a call. Where the name **is** a type, `NK1146` says so and names the braces —
//! which is the one question the parser can no longer answer and no longer has
//! to.

use nikaia::contracts::LedgerOps;
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> Result<String, String> {
    let parsed = parse_to_ast(source).map_err(|e| format!("{e:#}"))?;
    nikaia::emit::emit_program(&parsed, nikaia::emit::Build::default())
        .map(|it| it.rust)
        .map_err(|e| format!("{e:#}"))
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library = nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

/// **A signature whose parameters are all options writes no `;`** (D1).
#[test]
fn a_signature_of_options_alone_needs_no_semicolon() {
    let rust = lowered(
        "fn execute(target_age: i64 = 0) -> i64 { return target_age }\n\
         fn main() { println(f\"{execute()}\") }",
    )
    .expect("it parses and lowers");
    // The language below has no defaults, so the option becomes positional and
    // the default is the value (Kap 5.1).
    assert!(rust.contains("execute(0)"), "{rust}");
}

/// **A mixed signature keeps its `;`, and keeps it required** (D1, D2).
///
/// The half a rule tried first could have broken: `bare_config_params` reads
/// `path: String` and then wants the `=` that makes a parameter an option, so it
/// has to fail at the `;` and let the positional list take it.
#[test]
fn a_mixed_signature_keeps_its_semicolon() {
    let rust = lowered(
        "fn read(path: String; trusted: bool = false) -> String {\n\
             if trusted { return path }\n\
             return \"no\"\n\
         }\n\
         fn main() { println(read(\"in\"; trusted: true)) }",
    )
    .expect("it parses and lowers");
    assert!(
        rust.contains("fn read(path: String, trusted: bool)"),
        "{rust}"
    );
    assert!(rust.contains("read(String::from(\"in\"), true)"), "{rust}");
}

/// **The leading `;` in a signature is refused, and the message names the new
/// form** (D2).
#[test]
fn a_leading_semicolon_in_a_signature_is_refused() {
    let source = "fn execute(; target_age: i64 = 0) -> i64 { return target_age }";
    let refused = parse_to_ast(source).expect_err("the leading form is a parse error");
    let finding =
        nikaia::diagnostics::refused_finding(&refused).expect("a parse error carries its finding");
    let said = nikaia::diagnostics::render_finding(finding, "app.nika", source);
    assert!(said.contains("This `;` has nothing before it."), "{said}");
    assert!(said.contains("with only options, leave it out"), "{said}");
    assert!(
        said.contains("fn execute(target_age: i64 = 0)"),
        "the message names the one spelling: {said}"
    );
}

/// **A plain signature is untouched**, which is what a rule tried first has to
/// leave alone.
#[test]
fn a_signature_of_subjects_alone_is_untouched() {
    let rust = lowered(
        "fn add(a: i64, b: i64) -> i64 { return a + b }\n\
         fn main() { println(f\"{add(1, 2)}\") }",
    )
    .expect("it lowers");
    assert!(rust.contains("fn add(a: i64, b: i64) -> i64"), "{rust}");
    // And the message a missing default gets stays the one that belongs after a
    // `;`, where a parameter can only be an option - reusing `config_param` here
    // would have put *a configuration parameter needs a default* on this.
    assert!(!rust.contains("needs a default"), "{rust}");
}

/// **A call whose arguments are all options writes no `;`** (D1), which is the
/// half that waited on [ADR-140](../../../docs/specification/adr/adr-140.md) D1.
///
/// It also pins a defect older than either record: `execute(; target_age: 30)`
/// came out as `execute(, 30)` — the comma the emitter writes *between*
/// arguments, written before the first one because a call of options alone has
/// nothing in front of it. Invalid Rust, reported by `rustc` about a file nobody
/// wrote (Part III C.1), for the only spelling such a call had.
#[test]
fn a_call_of_options_alone_needs_no_semicolon() {
    let rust = lowered(
        "fn execute(target_age: i64 = 0) -> i64 { return target_age }\n\
         fn main() {\n\
             let a = execute(target_age: 30)\n\
             let b = execute()\n\
             println(f\"{a} {b}\")\n\
         }",
    )
    .expect("it lowers");
    assert!(rust.contains("execute(30)"), "{rust}");
    assert!(rust.contains("execute(0)"), "{rust}");
    assert!(!rust.contains("execute(,"), "no leading comma: {rust}");
}

/// **The leading `;` at a call is refused, and the message names the new form**
/// (D2). The signature half's rule, reaching the call now that there is a
/// spelling to send a reader to.
#[test]
fn a_leading_semicolon_at_a_call_is_refused() {
    let said = lowered(
        "fn execute(target_age: i64 = 0) -> i64 { return target_age }\n\
         fn main() { println(f\"{execute(; target_age: 30)}\") }",
    )
    .expect_err("the leading form is a parse error");
    assert!(said.contains("`;` has nothing before it."), "{said}");
    assert!(
        said.contains("execute(target_age: 30)"),
        "the message names the one spelling: {said}"
    );
}

/// **A mixed call keeps its `;`**, which the options-only arm tried first must
/// leave alone: it is decided on the second token, so `read("in", …)` fails it at
/// the `,` and the positional list takes it.
#[test]
fn a_mixed_call_keeps_its_semicolon() {
    let rust = lowered(
        "fn read(path: String; trusted: bool = false) -> String {\n\
             if trusted { return path }\n\
             return \"no\"\n\
         }\n\
         fn main() { println(read(\"in\"; trusted: true)) }",
    )
    .expect("it lowers");
    assert!(rust.contains("read(String::from(\"in\"), true)"), "{rust}");
}

/// **A method call takes the form too**, because one rule answers both: the
/// options-only list is in `call_arg_list`, which every call and every method
/// call goes through.
#[test]
fn a_method_call_of_options_alone_needs_no_semicolon() {
    let rust = lowered(
        "struct Query { n: i64 }\n\
         impl Query {\n\
             fn execute(self; target_age: i64 = 0) -> i64 { return self.n + target_age }\n\
         }\n\
         fn main() {\n\
             let q = Query { n: 1 }\n\
             println(f\"{q.execute(target_age: 30)}\")\n\
         }",
    )
    .expect("it lowers");
    assert!(rust.contains("execute(30)"), "{rust}");
}

/// **A struct literal written like a call is `NK1146`**, and the message carries
/// the rewrite ([ADR-140](../../../docs/specification/adr/adr-140.md) D1).
///
/// The silence this closes is what let the collision through two records:
/// `Stats(min: 1, max: 2)` was Kap 4.2's literal, the arm that checks fields
/// `continue`d over a type with none, and a name that declared no struct lowered
/// verbatim for `rustc` to answer about. It is a **call** now, so the question
/// moved with it — and where the name is a type, this is the old spelling and
/// nothing else.
#[test]
fn a_literal_written_like_a_call_names_the_braces() {
    let found: Vec<_> = findings(
        "struct Stats { min: i64, max: i64 }\n\
         fn main() { let s = Stats(min: 1, max: 2) println(f\"{s.min}\") }",
    )
    .into_iter()
    .filter(|f| f.code == "NK1146")
    .collect();
    assert_eq!(found.len(), 1, "one refusal: {found:#?}");
    let help = found[0].help.as_deref().expect("a way out");
    assert!(help.contains("Stats { min: …, max: … }"), "{help}");
}

/// **The brace form is untouched**, which is the half that matters: the refusal
/// above must not reach the literal the language kept.
#[test]
fn a_declared_structs_brace_literal_is_not_refused() {
    let found: Vec<_> = findings(
        "struct Stats { min: i64, max: i64 }\n\
         fn main() { let s = Stats { min: 1, max: 2 } println(f\"{s.min}\") }",
    )
    .into_iter()
    .filter(|f| f.code == "NK1146" || f.code == "NK1135")
    .collect();
    assert!(found.is_empty(), "{found:#?}");
}

/// **And the anonymous constructor is untouched**, which is what D1 keeps:
/// `Stats(a, b)` is a call and always was one.
#[test]
fn the_anonymous_constructor_is_still_a_call() {
    let rust = lowered(
        "struct Stats { n: i64 }\n\
         impl Stats {\n\
             pub fn(first: i64) -> Stats { return Stats { n: first } }\n\
         }\n\
         fn main() { let s = Stats(1) println(f\"{s.n}\") }",
    )
    .expect("it lowers");
    assert!(rust.contains("Stats::new(1)"), "{rust}");
}

/// **A brace literal naming a function gets the sentence that belongs to it**
/// — `NK1135`'s function branch, whose way out is now the call with options
/// rather than the `;` this compiler no longer takes.
#[test]
fn a_brace_literal_naming_a_function_says_so() {
    let found: Vec<_> = findings(
        "fn execute(target_age: i64 = 0) -> i64 { return target_age }\n\
         fn main() { let a = execute { target_age: 30 } println(f\"{a}\") }",
    )
    .into_iter()
    .filter(|f| f.code == "NK1135")
    .collect();
    assert_eq!(found.len(), 1, "one refusal: {found:#?}");
    let help = found[0].help.as_deref().expect("a way out");
    assert!(help.contains("execute(option: value)"), "{help}");
}

/// **A name nothing declares is a call nothing describes**, and that is silence
/// rather than a hole. `Widgit(size: 3)` used to be `NK1135`; it is a call now,
/// and `Widgit(3)` beside it has always been silent for the same reason — this
/// build cannot see whether a dependency declares it, and refusing a correct
/// program is what Part III C.4 forbids. The **brace** form still carries the
/// claim, which is where it belongs.
#[test]
fn a_call_naming_nothing_is_silent_and_the_brace_form_is_not() {
    let call: Vec<_> = findings("fn main() { let w = Widgit(size: 3) println(f\"{w.size}\") }")
        .into_iter()
        .filter(|f| f.code == "NK1135" || f.code == "NK1146")
        .collect();
    assert!(call.is_empty(), "{call:#?}");

    let braces: Vec<_> =
        findings("fn main() { let w = Widgit { size: 3 } println(f\"{w.size}\") }")
            .into_iter()
            .filter(|f| f.code == "NK1135")
            .collect();
    assert_eq!(braces.len(), 1, "{braces:#?}");
    assert!(
        braces[0]
            .notes
            .join(" ")
            .contains("Braces after a name build a struct"),
        "{:#?}",
        braces[0].notes
    );
}
