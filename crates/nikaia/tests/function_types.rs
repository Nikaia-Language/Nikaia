//! A function type says what a handler may do
//! ([ADR-102](../../../docs/specification/adr/adr-102.md) D1 and D2).
//!
//! A parameter could not be a function. The type grammar had a name, a view,
//! type arguments and `?`, so `fn route(path: &str, handler: fn(Request) ->
//! Response)` was a parse error, and the eight `std` entries that take a lambda
//! were written straight into the ledger where the grammar never saw them — a
//! second author could not write `route`, `retry`, `sort_by` or a panic hook.
//!
//! **The type carries two promises and no more** (D2), and their defaults are
//! the language's: without `sync` the code may pause, without `throws` it
//! cannot fail. That is the reading a *declaration* already has, applied to a
//! type — which is why it needs no words of its own.

mod common;

use std::process::Command;

use nikaia::ast::Item;
use nikaia::contracts::ty::Ty;
use nikaia::contracts::ty::TyOps;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

fn signature(source: &str, of: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    Ledger::infer(&parsed).functions[of]
        .signature
        .as_ref()
        .expect("a signature")
        .text()
}

/// Compile the lowering and run it, because "a closure argument" is a claim
/// about the language below.
fn ran(purpose: &str, source: &str) -> String {
    let rust = lowered(source);
    let dir = common::scratch_dir(&format!("function-type-{purpose}"));
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "{purpose} did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary).output().expect("run it");
    assert!(
        out.status.success(),
        "{purpose} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    std::fs::remove_dir_all(&dir).ok();
    printed
}

/// **D1's three forms**, exactly as the record writes them: parameters in
/// parentheses, `-> R` where there is a result, and `sync` and `throws` after
/// it, in the positions a declaration puts them.
#[test]
fn the_records_three_forms_parse() {
    for source in [
        "fn route(path: ref String, handler: fn(Request) -> Response) { }\n",
        "fn on_tick(handler: fn() sync) { }\n",
        "fn load(reader: fn(Path) -> Bytes throws) { }\n",
    ] {
        parse_to_ast(source).unwrap_or_else(|e| panic!("{source}\n{e}"));
    }
}

/// **`fn` is a shape and not a name**, which is what a tuple already is: its
/// parameters go where a named type's arguments go, so nothing that walks a
/// type has to know about it. Reading `name` there reported that nothing
/// declares a type called `fn`.
#[test]
fn a_function_type_is_not_an_undeclared_type_name() {
    let refused = findings("fn twice(x: i64, f: fn(i64) -> i64) -> i64 { return f(f(x)) }\n");
    assert!(!refused.iter().any(|f| f.code == "NK1135"), "{refused:#?}");
}

/// **The ledger writes it and reads it back** — which is the half that makes a
/// package able to publish `route`, since a consumer builds against contracts
/// rather than guesses (Part III 13.5).
#[test]
fn the_ledger_carries_the_type_and_its_two_promises() {
    assert_eq!(
        signature(
            "fn twice(x: i64, f: fn(i64) -> i64 sync) -> i64 { return f(f(x)) }\n",
            "twice"
        ),
        "(x: i64, f: fn(i64) -> i64 sync) -> i64"
    );
    for written in [
        "fn()",
        "fn(ref Stats)",
        "fn(Request) -> Response",
        "fn() sync",
        "fn(Path) -> Bytes throws",
        "fn(i64) -> i64 sync throws",
        // A function type may stand inside another one, which is why the
        // closing parenthesis is counted rather than looked for at the end.
        "fn(fn(i64)) -> i64",
    ] {
        assert_eq!(Ty::parse(written).text(), written);
    }
}

/// **A lambda that does less fits a type that allows more** (D2), and the other
/// direction does not: a type that says `sync` is the same assertion
/// [ADR-027](../../../docs/specification/adr/adr-027.md) makes about a
/// declaration, made about somebody else's code.
#[test]
fn a_lambda_that_does_less_fits_a_type_that_allows_more() {
    let fits = |a: &str, b: &str| Ty::parse(a).fits(&Ty::parse(b));
    assert!(
        fits("fn() sync", "fn()"),
        "never pausing goes where pausing is allowed"
    );
    assert!(
        !fits("fn()", "fn() sync"),
        "the assertion is not given away"
    );
    assert!(
        fits("fn()", "fn() throws"),
        "cannot fail goes where failing is allowed"
    );
    assert!(!fits("fn() throws", "fn()"), "and not the other way");
    assert!(fits("fn(i64) -> i64", "fn(i64) -> i64"));
    assert!(
        !fits("fn(i64)", "fn(ref String)"),
        "the parameters still have to match"
    );
}

/// **D5's run case**: a closure argument, which is what `std`'s own `map` and
/// `access` take and what costs nothing.
#[test]
fn a_run_parameter_lowers_to_a_closure_argument() {
    let printed = ran(
        "twice",
        "fn twice(x: i64, f: fn(i64) -> i64) -> i64 { return f(f(x)) }\n\
         fn main() { println(f\"{twice(2, fn(n) { return n * 3 })}\") }\n",
    );
    assert_eq!(printed.trim(), "18");
    // **A view of it** (Part I 5.4 C): `on_tick` only calls its handler, so the
    // parameter is immediate and borrows, exactly as an ordinary parameter the
    // body only reads does. The `&` used not to be written, which held for as
    // long as nothing handed a handler **on** - and then `rustc` said *use of
    // moved value* about a file nobody wrote.
    let rust = lowered("fn on_tick(handler: fn() sync) { }\nfn main() { }\n");
    assert!(rust.contains("fn on_tick(handler: &impl Fn())"), "{rust}");
}

/// **A run parameter's call hands back what its type says** (#125): typed as
/// nothing, `name.contains` on its result resolved to no method, which is read
/// as one that may pause, and `has` came out `async` with nothing in it that
/// pauses.
#[test]
fn a_run_parameters_result_is_its_declared_type() {
    let source = "fn has(f: fn(ref String) -> String sync, s: ref String) -> bool {\n\
                  \x20   let name = f(s)\n\
                  \x20   return name.contains(\"::\")\n\
                  }\n\
                  fn main() { println(f\"{has(fn(x) { x.clone() }, \"a::b\")}\") }\n";
    let rust = lowered(source);
    assert!(rust.contains("\nfn has("), "{rust}");
    assert_eq!(ran("run-result", source).trim(), "true");
}

/// **And `throws` puts the same `Result` on the closure's result** that a
/// `throws` function's own declaration puts on its (Part I 7.1), which is what
/// makes a lambda that fails fit it.
#[test]
fn throws_on_the_type_is_the_result_a_throws_function_has() {
    // With `sync`, the plain closure: the `Result` is the whole of what the
    // word adds.
    let plain = lowered("fn load(reader: fn(Path) -> Bytes sync throws) { }\nfn main() { }\n");
    assert!(
        plain.contains("impl Fn(Path) -> Result<Bytes, Box<dyn std::error::Error>>"),
        "{plain}"
    );
    // Without it, the same `Result` is what the **async closure** hands back
    // ([ADR-192](../../../docs/specification/adr/adr-192.md) D1). `load` does
    // not keep `reader`, so it is a run parameter and takes the bound rather
    // than the box.
    let run = lowered("fn load(reader: fn(Path) -> Bytes throws) { }\nfn main() { }\n");
    assert!(
        run.contains("impl AsyncFn(Path) -> Result<Bytes, Box<dyn std::error::Error>>"),
        "{run}"
    );
    let nothing = lowered("fn attempt(step: fn() sync throws) { }\nfn main() { }\n");
    assert!(
        nothing.contains("impl Fn() -> Result<(), Box<dyn std::error::Error>>"),
        "{nothing}"
    );
}

/// **A lambda that pauses is an ordinary program now**
/// ([ADR-122](../../../docs/specification/adr/adr-122.md) D2), where it used to
/// be refused at the build, because the lowering had no shape for one.
///
/// **And the shape is the one the body asks for**
/// ([ADR-192](../../../docs/specification/adr/adr-192.md) D1): `run` calls `f`
/// and does not keep it, so the parameter is `impl AsyncFn() -> String` and the
/// lambda an `async` closure — 1.37 ns/call against the box's 11.99, on a
/// 0.31 floor. What this test asserted before that record — that the program
/// lowers rather than being refused — is D2 and holds either way.
#[test]
fn a_lambda_that_pauses_fits_a_parameter_that_allows_pausing() {
    let source = "use std::io\n\nfn run(f: fn() -> String) -> String { return f() }\n\
                  fn main() { let said = run(fn() { return io::read_to_string() })\n\
                  \x20   println(f\"{said}\") }\n";
    // It lowers, which is what "refused at the build" meant: the emitter used
    // to return an error here rather than write anything.
    let rust = lowered(source);
    assert!(
        rust.contains("fn run(f: &impl AsyncFn() -> String)"),
        "a run parameter takes the bound, and a view of it: {rust}"
    );
    // **No `move`**, which is Part I 5.4 A: a lambda handed to a parameter the
    // body only calls borrows what it captures.
    assert!(
        rust.contains("run(&async || {"),
        "and the lambda is an `async` closure: {rust}"
    );
    assert!(
        rust.contains("io::read_to_string().await"),
        "and its body may await inside it: {rust}"
    );
}

/// **The declaration and the call read one column**
/// ([ADR-192](../../../docs/specification/adr/adr-192.md) D3), and this is the
/// test that would catch them drifting apart: the two disagreeing is one
/// parameter with two shapes, which is `rustc`'s words about a file nobody
/// wrote.
///
/// It **runs**, because the shape looking right is what the old lowering also
/// did.
#[test]
fn a_run_parameter_that_pauses_compiles_and_runs() {
    let printed = ran(
        "async-run-parameter",
        "use std::time\n\n\
         fn twice(f: fn() -> i64) -> i64 { return f() + f() }\n\
         fn main() {\n\
         \x20   let said = twice(fn() {\n\
         \x20       time::sleep(1.millis())\n\
         \x20       return 21\n\
         \x20   })\n\
         \x20   println(f\"{said}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "42");
}

/// **A parameter the body *keeps* keeps the box**
/// ([ADR-192](../../../docs/specification/adr/adr-192.md) D1), because
/// `AsyncFn` is a **bound** and a value that outlives the call needs a type.
///
/// `spawn` is the one position that can keep a code parameter today — a field,
/// a result and a `let` are all `NK1142` until
/// [ADR-102](../../../docs/specification/adr/adr-102.md) D5 lands — so this is
/// what holds the kept branch honest until then.
#[test]
fn a_kept_parameter_keeps_the_boxed_closure() {
    let rust = lowered(
        "fn holds(f: fn() -> String) {\n\
         \x20   spawn fn { println(f()) }\n\
         }\n\
         fn main() { }\n",
    );
    assert!(
        rust.contains("nikaia_std::func::Kept<dyn Fn() -> nikaia_std::func::Boxed<String>>"),
        "a kept parameter is a kept value, over a boxed future: {rust}"
    );
    assert!(!rust.contains("impl AsyncFn"), "{rust}");
}

/// **`std`'s own entries are untouched** (D3), and this is the corpus case that
/// said so out loud: `and_modify` is a hand-written description of a **Rust**
/// signature, which takes a plain closure whatever the ledger's `sync` column
/// says. Writing the future shape for one produced
/// *expected `()`, found `Pin<Box<…>>`* against `examples/access-log/src/main.nika`.
///
/// So the shape is written only where the signature was **declared here**.
#[test]
fn a_std_entrys_lambda_keeps_its_plain_closure() {
    let rust = lowered(
        "fn main() {\n\
         \x20   let xs = Vec()\n\
         \x20   let doubled = xs.map fn(n) { n * 2 }\n\
         \x20   println(f\"{doubled.len()}\")\n\
         }\n",
    );
    assert!(!rust.contains("Box::pin"), "{rust}");
}

/// **A function type outside a parameter is a kept value** (D5): a field, a
/// result and a `let` are one shared closure below, and nothing is refused
/// for standing there.
#[test]
fn a_function_type_outside_a_parameter_is_a_kept_value() {
    for source in [
        "struct Router { handler: fn(i64) -> i64 }\nfn main() { }\n",
        "fn make() -> fn(i64) -> i64 sync { return fn(x) { x } }\nfn main() { }\n",
        "fn main() { let f: fn(i64) -> i64 sync = fn(x) { x + 1 }\n println(f\"{f(1)}\") }\n",
    ] {
        let refused = findings(source);
        assert!(refused.is_empty(), "{source}\n{refused:#?}");
        assert!(
            lowered(source).contains("nikaia_std::func::Kept<dyn Fn("),
            "{source}"
        );
    }
}

/// **`NK2206`: a lambda that pauses, handed to a `fn() sync`** (D2).
///
/// A type that says `sync` is the assertion
/// [ADR-027](../../../docs/specification/adr/adr-027.md) makes about a
/// declaration, made about somebody else's code: a caller who wrote it has
/// promised their own callers something, and a handler that pauses takes the
/// promise away without saying so.
#[test]
fn a_pausing_lambda_handed_to_a_sync_type_is_refused() {
    let refused = findings(
        "use std::io\n\nfn on_tick(handler: fn() sync) { }\n\
         fn main() { on_tick(fn() { let t = io::read_to_string() }) }\n",
    );
    let about = refused
        .iter()
        .find(|f| f.code == "NK2206")
        .unwrap_or_else(|| panic!("{refused:#?}"));
    assert!(about.message.contains("`sync`"), "{}", about.message);

    // …and the direction that is allowed is allowed: a lambda that does less
    // fits a type that allows more.
    let fine = findings(
        "fn on_tick(handler: fn() sync) { }\nfn main() { on_tick(fn() { let n = 1 + 1 }) }\n",
    );
    assert!(fine.is_empty(), "{fine:#?}");
}

/// **`NK2606`: a lambda that fails, handed to a type that declares none** (D2).
///
/// And it is the **whole** message: the function *around* the lambda is not the
/// one that has to answer for a failure the type refuses, so `NK2605` — *this
/// function can fail because …* — does not stand beside it. Where the type
/// **does** say `throws`, the failure travels to the caller by
/// [ADR-029](../../../docs/specification/adr/adr-029.md) D3 and `NK2605` is
/// right again.
#[test]
fn a_failing_lambda_handed_to_a_type_without_throws_is_refused() {
    const ERROR: &str = "enum E { Bad }\n\
                         impl Error for E { fn message(ref self) -> String { return \"bad\" } }\n\
                         fn risky() throws { throw E::Bad }\n";

    let refused = findings(&format!(
        "{ERROR}fn attempt(step: fn()) {{ }}\nfn main() {{ attempt(fn() {{ risky() }}) }}\n"
    ));
    assert!(refused.iter().any(|f| f.code == "NK2606"), "{refused:#?}");
    assert!(
        !refused.iter().any(|f| f.code == "NK2605"),
        "one mistake, one message: {refused:#?}"
    );

    let allowed = findings(&format!(
        "{ERROR}fn attempt(step: fn() throws) {{ }}\nfn main() {{ attempt(fn() {{ risky() }}) }}\n"
    ));
    assert!(!allowed.iter().any(|f| f.code == "NK2606"), "{allowed:#?}");
    assert!(
        allowed.iter().any(|f| f.code == "NK2605"),
        "the failure does reach `main` here: {allowed:#?}"
    );
}

/// **A failure caught inside the lambda does not leave it** (D2).
///
/// `NK2606`'s help says *handle it inside the lambda with `… catch { … }`*, and
/// a program that did exactly that was refused with the same message: the
/// lambda was marked as failing at the call, before anyone asked whether a
/// `catch` guarded it. The `catch` that counts is one in the lambda's **body**.
/// One around the call the lambda is handed to handles what that call throws,
/// not what the lambda does when the callee runs it, so it still says nothing
/// here — and neither does a handler, whose own failure leaves the lambda.
#[test]
fn a_failure_caught_inside_the_lambda_is_not_the_lambdas() {
    const ERROR: &str = "enum E { Bad }\n\
                         impl Error for E { fn message(ref self) -> String { return \"bad\" } }\n\
                         fn risky(x: i64) -> i32 throws { throw E::Bad }\n";

    // A user function's failure, caught in the body.
    let caught = findings(&format!(
        "{ERROR}fn attempt(step: fn()) {{ }}\n\
         fn main() {{ attempt(fn() {{ let n = risky(1) catch {{ 0 }} }}) }}\n"
    ));
    assert!(caught.is_empty(), "{caught:#?}");

    // A `std` entry's, through `map`: the report's own program.
    let read = findings(
        "use std::fs\nfn main() {\n    let names = [\"a.txt\", \"b.txt\"]\n    \
         let texts = names.map fn(n) { fs::read(n, fs::Root::Anywhere) catch { \"\" } }\n}\n",
    );
    assert!(!read.iter().any(|f| f.code == "NK2606"), "{read:#?}");
    let mapped = findings(&format!(
        "{ERROR}fn main() {{ let ys = [1, 2].map fn(x) {{ risky(x) catch {{ 0 }} }} }}\n"
    ));
    assert!(!mapped.iter().any(|f| f.code == "NK2606"), "{mapped:#?}");
    // …and what is let through is a program `rustc` takes, with the handler's
    // value standing where the failure was.
    let ran = ran(
        "caught-in-lambda",
        "enum E { Bad }\n\
         impl Error for E { fn message(ref self) -> String { return \"bad\" } }\n\
         fn risky(x: i64) -> i64 throws {\n    if x > 1 { throw E::Bad }\n    return x * 10\n}\n\
         fn main() {\n    let ys = [1, 2].map fn(x) { risky(x) catch { 0 } }\n    \
         for y in ys { println(f\"{y}\") }\n}\n",
    );
    assert_eq!(ran.trim(), "10\n0");

    // Not caught in the body, and a `catch` around the call does not stand in
    // for one.
    let outside = findings(&format!(
        "{ERROR}fn attempt(step: fn()) throws {{ risky(0) }}\n\
         fn main() {{ attempt(fn() {{ let n = risky(1) }}) catch {{ }} }}\n"
    ));
    assert!(
        outside.iter().any(|f| f.code == "NK2606"),
        "a `catch` outside the lambda does not handle its failure: {outside:#?}"
    );

    // Nor does a `catch` in an *enclosing* lambda's body: the inner lambda is
    // a function of its own and the edge is the same edge.
    let nested = findings(&format!(
        "{ERROR}fn attempt(step: fn()) {{ }}\n\
         fn run(step: fn() -> i32) -> i32 throws {{ return risky(0) }}\n\
         fn main() {{ attempt(fn() {{ let n = run(fn() {{ risky(1) }}) catch {{ 0 }} }}) }}\n"
    ));
    assert!(
        nested.iter().any(|f| f.code == "NK2606"),
        "the inner lambda's failure is its own: {nested:#?}"
    );

    // A handler is ordinary code again: what fails in it leaves the lambda.
    let handler = findings(&format!(
        "{ERROR}fn attempt(step: fn()) {{ }}\n\
         fn main() {{ attempt(fn() {{ let n = risky(1) catch {{ risky(2) }} }}) }}\n"
    ));
    assert!(
        handler.iter().any(|f| f.code == "NK2606"),
        "the handler's own failure is the lambda's: {handler:#?}"
    );
}

/// **A free call resolves its callee before it walks its arguments**, which is
/// [ADR-029](../../../docs/specification/adr/adr-029.md)'s ordering — the
/// *method* path was given it when that record landed and this one was not.
///
/// It is what makes the two refusals above possible at all: a lambda's
/// promises are read off the parameter's type, and the type is not in hand
/// until the callee is. It also types the lambda's **parameters**, which is
/// what `hand(fn(n) { … })` had never had.
#[test]
fn a_free_calls_lambda_is_typed_from_the_signature() {
    // Nothing is refused, and that is the claim: the walk reaches the body
    // through the door that carries types rather than the one that does not.
    let source = "fn twice(x: i64, f: fn(i64) -> i64) -> i64 { return f(f(x)) }\n\
                  fn main() { println(f\"{twice(2, fn(n) { return n * 3 })}\") }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
}

/// **The type decides, and nothing inferred stands behind it**
/// ([ADR-122](../../../docs/specification/adr/adr-122.md) D1).
///
/// This test used to assert the opposite, and the record it asserted is the one
/// ADR-122 answers: [ADR-102](../../../docs/specification/adr/adr-102.md) D3
/// gave a **run** parameter `sync = "from(f)"` — *the lambda decides* — on
/// [ADR-029](../../../docs/specification/adr/adr-029.md) D3's reasoning that
/// the lambda's body is counted in the caller. That reasoning holds while the
/// lambda is a plain closure. It stops holding the moment the parameter's type
/// may pause, because then the lambda is a **future the callee awaits**, and a
/// function that awaits is `async` whatever its caller handed over.
///
/// So the column follows the type: `sync` on the parameter and the callee keeps
/// its claim; nothing on it and the callee pauses, run or kept.
#[test]
fn the_parameters_type_decides_whether_the_callee_pauses() {
    let sync_of = |source: &str, of: &str| {
        let parsed = parse_to_ast(source).expect("the source parses");
        Ledger::infer(&parsed).functions[of].sync_claim.clone()
    };
    assert!(
        sync_of(
            "fn twice(x: i64, f: fn(i64) -> i64 sync) -> i64 { return f(f(x)) }\n",
            "twice"
        )
        .is_sync(),
        "a plain closure's call adds nothing"
    );
    assert!(
        !sync_of(
            "fn twice(x: i64, f: fn(i64) -> i64) -> i64 { return f(f(x)) }\n",
            "twice"
        )
        .is_sync(),
        "a call to a parameter that may pause is an await, and this function does it"
    );
    // …and the two lowerings say the same thing, which is the half a reader
    // sees: one is a function, the other is an `async fn` whose call is awaited.
    let plain = lowered(
        "fn twice(x: i64, f: fn(i64) -> i64 sync) -> i64 { return f(f(x)) }\n\
         fn main() { println(f\"{twice(2, fn(n) { return n * 3 })}\") }\n",
    );
    assert!(plain.contains("fn twice("), "{plain}");
    assert!(!plain.contains("async fn twice("), "{plain}");
    let future = lowered(
        "fn twice(x: i64, f: fn(i64) -> i64) -> i64 { return f(f(x)) }\n\
         fn main() { println(f\"{twice(2, fn(n) { return n * 3 })}\") }\n",
    );
    assert!(future.contains("async fn twice("), "{future}");
    assert!(future.contains("f(f(x).await).await"), "{future}");
}

/// **A kept one is answered from the type** (D3), and a body that stores *and*
/// runs gets the kept answer, which is the safe one and what ADR-102 §4 says it
/// gets.
#[test]
fn a_kept_parameter_is_answered_from_the_type() {
    let sync_of = |source: &str, of: &str| {
        let parsed = parse_to_ast(source).expect("the source parses");
        Ledger::infer(&parsed).functions[of].sync_claim.clone()
    };
    // Stored and run, and the type allows pausing: the claim is off.
    assert!(
        !sync_of(
            "fn keep(g: fn()) { }\nfn both(f: fn()) { keep(f)\n    f() }\n",
            "both"
        )
        .is_sync(),
        "a wrapper that stores and runs gets the kept answer"
    );
    // The same body, with a type that says the code never pauses.
    assert!(
        sync_of(
            "fn keep(g: fn() sync) { }\nfn both(f: fn() sync) { keep(f)\n    f() }\n",
            "both"
        )
        .is_sync(),
        "a kept `fn() sync` cannot pause, so neither does this"
    );
    // And a body that only hands it on keeps its own answer: storing code is
    // not running it.
    assert!(
        sync_of(
            "fn keep(g: fn()) { }\nfn hold(f: fn()) { keep(f) }\n",
            "hold"
        )
        .is_sync(),
        "handing it on is not running it"
    );
}

/// **A promise is still taken away by anything else the body does.** `from` is
/// *this call adds no pausing of its own*, not *this call cannot pause*.
#[test]
fn a_body_that_also_pauses_keeps_no_claim() {
    let parsed = parse_to_ast(
        "use std::io\n\nfn twice(x: i64, f: fn(i64) -> i64) -> i64 { let t = io::read_to_string()\n\
         \x20   return f(x) }\n",
    )
    .expect("the source parses");
    assert!(
        !Ledger::infer(&parsed).functions["twice"]
            .sync_claim
            .is_sync()
    );
}

/// **The trailing words are greedy**, which settles the one ambiguity D1 does
/// not name: in `fn make() -> fn(i64) -> i64 sync` the `sync` belongs to the
/// *result type*.
///
/// **And the declaration's own pre-arrow form is gone**
/// ([ADR-140](../../../docs/specification/adr/adr-140.md) D4), which is what
/// D1's note used to point at as the way to mean the other thing. Nothing can
/// write that shape today — a function type in a **result** is `NK1142` until
/// D5's boxed lowering exists — so what it costs is a spelling for a program
/// that does not compile, and D5 is the record that owes one.
#[test]
fn a_trailing_sync_belongs_to_the_type_it_follows() {
    let parsed = parse_to_ast("fn make() -> fn(i64) -> i64 sync { }\n").expect("the source parses");
    // **The AST and not the ledger**, which is what this test used to ask and
    // could not have answered: `sync` is *inferred* from the body, and an empty
    // body pauses at nothing — so the ledger says `sync` whichever of the two
    // the word attached to, and the old assertion held for the wrong reason.
    let Item::Fn {
        is_sync, ret_type, ..
    } = &parsed.program.items[0].node
    else {
        panic!("a function");
    };
    assert!(!is_sync, "the trailing `sync` is not the declaration's");
    let code = ret_type
        .as_ref()
        .and_then(|t| (*t.code).as_ref())
        .expect("the result is a function type");
    assert!(code.is_sync, "it is the result type's");
}

/// **The pre-arrow form is refused, and the message names the order**
/// ([ADR-140](../../../docs/specification/adr/adr-140.md) D4).
#[test]
fn a_promise_before_the_arrow_is_refused() {
    let refused = parse_to_ast("fn load() throws -> String { return \"a\" }")
        .expect_err("the pre-arrow form is a parse error");
    let said = format!("{refused:#}");
    assert!(said.contains("comes after the result type"), "{said}");
    let help = nikaia::diagnostics::refused_finding(&refused)
        .and_then(|f| f.help.clone())
        .unwrap_or_default();
    assert!(help.contains("fn f() -> String throws"), "{said}: {help}");
}

/// **A declaration with no result type writes the word where it always did**,
/// because there is nothing for it to be before or after — which is why the
/// refusal's cut sits after the arrow and not after the word.
#[test]
fn a_promise_with_no_result_type_is_untouched() {
    let parsed = parse_to_ast("fn tick() sync { }\n").expect("it parses");
    assert!(
        Ledger::infer(&parsed).functions["tick"]
            .sync_claim
            .is_sync()
    );
}

/// **A handler handed on, in a loop** — which is what the first program to write
/// D1's parameter for real did, and what `rustc` refused about a file nobody
/// wrote: *use of moved value: `handler`*.
///
/// A function value lowers to `impl AsyncFn(A) -> R`, an opaque type with no
/// `Copy`, so a `listen` that passes its handler to an `answer` inside its accept
/// loop handed it away the first time round. The answer is the one Part I 5.4 C
/// already gives and the one every other parameter gets: a parameter the body
/// only **calls or passes on** is not in `keeps`, so `contracts::keeps::lends`
/// says to write the `&` in the declaration, and a call that hands the lent
/// view on hands it as it is. `&F` is a function too, which is why one `&` is
/// all it takes.
#[test]
fn a_handler_passed_on_inside_a_loop_is_lent() {
    let source = "fn twice(n: i64, f: fn(i64) -> i64) -> i64 {\n\
                  \x20   return f(n) + f(n)\n\
                  }\n\n\
                  fn all(f: fn(i64) -> i64) -> i64 {\n\
                  \x20   let mut total = 0\n\
                  \x20   for n in [1, 2, 3] {\n\
                  \x20       total = total + twice(n, f)\n\
                  \x20   }\n\
                  \x20   return total\n\
                  }\n\n\
                  fn main() {\n\
                  \x20   println(f\"{all(fn(x) { x * 10 })}\")\n\
                  }\n";
    let rust = lowered(source);
    assert!(
        rust.contains("f: &impl AsyncFn(i64) -> i64"),
        "the declaration takes a view of it: {rust}"
    );
    // **And the call hands it on as it has it**: `f` is a `&impl AsyncFn`
    // already, and a reference is a copy. A second `&` compiled here and was a
    // new type at every level of a recursion (ADR-252 D6, found moving `fold`).
    assert!(
        rust.contains("twice(n, f)"),
        "and the call hands on the view it was lent: {rust}"
    );
    // (1+1) + (2+2) + (3+3) = 12, times ten.
    assert_eq!(ran("passed-on", source).trim(), "120");
}

/// **A parameter the declaration wrote `mut` is handed straight on**
/// ([ADR-094](../../../docs/specification/adr/adr-094.md) D3).
///
/// `&mut` at a call is written off the callee's word, and the argument here is a
/// `mut` parameter of the function the call stands in — which is a `&mut T`
/// already. A second one is not a `&mut &mut T` that works by deref: a `&mut`
/// may only be taken of a binding that is itself `mut`, and a parameter is not
/// one, so `rustc` refused it — *cannot borrow `connection` as mutable, as it is
/// not declared as mutable* — about a file nobody wrote
/// ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
///
/// The first program to meet it was `examples/http`'s server, whose `answer`
/// hands its connection to a `refuse`.
#[test]
fn a_mut_parameter_handed_to_another_mut_parameter_gains_no_second_reference() {
    let source = "fn bump(mut xs: Vec[i64]) {\n\
                  \x20   xs.push(1)\n\
                  }\n\n\
                  fn twice(mut xs: Vec[i64]) {\n\
                  \x20   bump(xs)\n\
                  \x20   bump(xs)\n\
                  }\n\n\
                  fn main() {\n\
                  \x20   let mut xs = [0]\n\
                  \x20   twice(xs)\n\
                  \x20   println(f\"{xs.len()}\")\n\
                  }\n";
    let rust = lowered(source);
    assert!(
        rust.contains("bump(xs);"),
        "the reference is already there: {rust}"
    );
    assert!(
        !rust.contains("bump(&mut xs)"),
        "and a second one is not written: {rust}"
    );
    // **And the `let mut` still gains one**, which is the half this must not have
    // taken away: a local is a value, so the call means the reference.
    assert!(
        rust.contains("twice(&mut xs)"),
        "a `let mut` argument is still lent: {rust}"
    );
    assert_eq!(ran("mut-passed-on", source).trim(), "3");
}
