//! The trailing lambda, with its arguments named.
//!
//! Part I 5.3 offers a lambda two spellings of the same form: implicit
//! arguments (`fn { a.id }`) or named ones (`fn(user) { user.id }`). In
//! expression position the parser took both. In *trailing* position - the
//! lambda outside the call's parentheses - it took only the implicit one, so
//! every named example in the specification was read as two expressions: the
//! call without its last argument, and a closure nobody passed anywhere. That
//! type-checks in Rust often enough to be a different program rather than an
//! error, which is why these are compiled and not only compared as text.
//!
//! **The stand-ins.** A trailing lambda's callee is usually something `std`
//! does not have yet - a lock's `access`, `both_of`, `task::scope`. ADR-028
//! D5 keeps an entry out of `std` until a program asks for it, so each test
//! that needs one prepends the Rust it stands for. What is under test is the
//! grammar and the lowering: that the emitted call is a well-formed Rust call
//! whose last argument is the closure.

mod common;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

/// The refusal, rendered as a reader with the file open sees it: its notes
/// and help included.
fn rendered(source: &str) -> String {
    let refused = parse_to_ast(source).expect_err("refused");
    let finding =
        nikaia::diagnostics::refused_finding(&refused).expect("a parse error carries its finding");
    nikaia::diagnostics::render_finding(finding, "app.nika", source)
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Lower, compile the result as a Rust library, and hand the emitted text back.
///
/// `--emit=metadata`: whether the call is well formed is settled by type
/// checking, and there is no reason to pay for code generation to hear it.
fn compiled(purpose: &str, support: &str, source: &str) -> String {
    let rust = lowered(source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("lowered.rs");
    std::fs::write(&file, format!("{support}\n{rust}")).expect("write the Rust");

    let compiled = common::compile(
        &file,
        &[
            "--crate-type",
            "lib",
            "--emit=metadata",
            "-o",
            dir.join("lowered.rmeta").to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "the lowering of {purpose} does not compile:\n{}\n--- the Rust ---\n{support}\n{rust}",
        String::from_utf8_lossy(&compiled.stderr),
    );

    let _ = std::fs::remove_dir_all(&dir);
    rust
}

/// Part I 5.3's own example: `users.map fn(user) { … }`.
#[test]
fn a_method_takes_a_named_trailing_lambda() {
    let rust = compiled(
        "lambda-named",
        "",
        "use std::io\n\
         \n\
         struct User { id: i64, name: String }\n\
         \n\
         fn ids(users: Vec[User]) -> Vec[i64] {\n\
         \x20   return users.map fn(user) { user.id }\n\
         }\n",
    );
    assert!(rust.contains("users.map(|user| { user.id })"), "{rust}");
}

/// Two named parameters, and a receiver that is not the lambda's only
/// argument: `numbers.reduce(0) fn(acc, item) { … }`.
#[test]
fn a_method_takes_arguments_and_a_named_trailing_lambda() {
    let rust = lowered(
        "fn total(numbers: Vec[i64]) -> i64 { return numbers.reduce(0) fn(acc, item) { acc + item } }",
    );
    assert!(
        rust.contains("numbers.reduce(0, |acc, item| { acc + item })"),
        "{rust}"
    );
}

/// Part II 12.3's single `access`: `account.access fn(to) { … }`.
///
/// The stand-in is what `Locked[T]`'s `access` will be - it hands the body a
/// mutable view of the locked value and gives the lock back at the `}`.
#[test]
fn a_lock_is_accessed_through_a_named_trailing_lambda() {
    let rust = compiled(
        "lambda-access",
        "impl Account {\n\
         \x20   fn access<R>(mut self, body: impl FnOnce(&mut Account) -> R) -> R {\n\
         \x20       body(&mut self)\n\
         \x20   }\n\
         }\n",
        "struct Account { balance: i64 }\n\
         \n\
         fn deposit(account: Account) {\n\
         \x20   account.access fn(to) { to.balance += 100 }\n\
         }\n",
    );
    assert!(
        rust.contains("account.access(|to| { to.balance += 100; })"),
        "{rust}"
    );
}

/// A free call with arguments **and** a trailing lambda that names two
/// parameters — the shape Part II 12.3's `access_all` is written in.
///
/// The free-call equivalent of the `"." name args lambda` rule, and the one that
/// needed the lambda to reach a path as well as a method: nothing before the
/// lambda is a method call at all. The callee here is a made-up `both_of` and
/// not the real door, because this is a test of the **grammar** - `access_all`
/// is typed and lowered by the compiler now
/// ([ADR-065](../../../docs/specification/adr/adr-065.md)), so using its name
/// would test that instead.
#[test]
fn a_free_call_takes_arguments_and_a_named_trailing_lambda() {
    let rust = compiled(
        "lambda-access-all",
        "fn both_of<R>(\n\
         \x20   mut a: Account,\n\
         \x20   mut b: Account,\n\
         \x20   body: impl FnOnce(&mut Account, &mut Account) -> R,\n\
         ) -> R {\n\
         \x20   body(&mut a, &mut b)\n\
         }\n",
        "struct Account { balance: i64 }\n\
         \n\
         fn transfer(account_a: Account, account_b: Account) {\n\
         \x20   both_of(account_a, account_b) fn(a, b) {\n\
         \x20       let amount = 100\n\
         \x20       a.balance -= amount\n\
         \x20       b.balance += amount\n\
         \x20   }\n\
         }\n",
    );
    assert!(
        rust.contains("both_of(account_a, account_b, |a, b| {"),
        "{rust}"
    );
}

/// Part II 12.7's `task::scope fn(s) { … }`: a trailing lambda on a **path**,
/// with the scope's own `s.spawn fn { … }` inside it.
///
/// Both spellings in one program: the outer lambda names its argument because
/// the scope handle needs a name, the inner one does not need any.
#[test]
fn a_path_takes_a_named_trailing_lambda() {
    let rust = compiled(
        "lambda-scope",
        "mod task {\n\
         \x20   pub struct Scope;\n\
         \x20   impl Scope {\n\
         \x20       pub fn spawn<R>(&self, body: impl FnOnce() -> R) {\n\
         \x20           let _ = body();\n\
         \x20       }\n\
         \x20   }\n\
         \x20   pub fn scope<R>(body: impl FnOnce(&Scope) -> R) -> R {\n\
         \x20       body(&Scope)\n\
         \x20   }\n\
         }\n",
        "fn work(total: i64) {\n\
         \x20   task::scope fn(s) {\n\
         \x20       s.spawn fn { total + 1 }\n\
         \x20   }\n\
         }\n",
    );
    assert!(rust.contains("task::scope(|s| {"), "{rust}");
    assert!(rust.contains("s.spawn(|| { total + 1 })"), "{rust}");
}

/// **A lambda that names nothing takes nothing**, in every position that has one
/// ([ADR-049](../../../docs/specification/adr/adr-049.md) D1).
///
/// This used to assert the opposite of its first half: `fn { a.id }` was a lambda
/// of one argument called `a`. Nothing is read off a body now, so the same source
/// is a lambda of none - and `a` is a name nothing declares, which the test below
/// holds. The second half is the form that was always the zero-argument one and is
/// unchanged.
#[test]
fn a_lambda_that_names_nothing_takes_nothing() {
    let rust = lowered(
        "fn main() {\n\
         \x20   let n = xs.map fn (x) { x.id } .len()\n\
         \x20   let s = Server::new().route(\"/x\") fn { handler(db) }.listen(\":8080\")\n\
         }",
    );
    assert!(rust.contains(".map(|x| { x.id }).len()"), "{rust}");
    assert!(
        rust.contains(r#".route("/x", || { handler(db) }).listen(":8080")"#),
        "{rust}"
    );
}

/// **And reaching for one of the three withdrawn names is refused here**, not by
/// `rustc` about the generated file (Part III, C.1).
///
/// `xs.map fn { a.id }` is the idiom that existed until ADR-049. It now lowers to
/// `|| { a.id }`, which does not compile - so the message has to be this
/// language's, and it has to say what happened to the form rather than only that
/// a name is unknown. The same ground ADR-022 stands on for `fn: …`.
#[test]
fn reaching_for_a_withdrawn_automatic_name_is_refused() {
    let source = "fn ids(xs: Vec[i64]) -> Vec[i64] { return xs.map fn { a.id } }";
    let parsed = nikaia::parser::parse_to_ast(source).expect("it parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std's ledger parses");
    let found = nikaia::check::check(&parsed, &own, &library).findings;

    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1117");
    assert!(
        found[0].message.contains("`a` isn't declared anywhere"),
        "{found:#?}"
    );
    assert!(
        found[0].notes[0].contains("are no longer there automatically"),
        "the message says what happened to the form: {found:#?}"
    );
    assert!(
        found[0].help.as_deref().is_some_and(|h| h.contains("fn (")),
        "and what to write instead: {found:#?}"
    );

    // A lambda that declares the name for itself never reaches this.
    let fine = "fn ids(xs: Vec[i64]) -> Vec[i64] { return xs.map fn (a) { a.id } }";
    let parsed = nikaia::parser::parse_to_ast(fine).expect("it parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    assert!(
        nikaia::check::check(&parsed, &own, &library)
            .findings
            .is_empty(),
        "`a` is declared here"
    );
}

/// ADR-022's removed `fn: …` still gets its sentence rather than a parse error
/// about a parameter list - from a method and from a path alike.
///
/// The named arm is tried first and starts with the same `fn`, so this is the
/// arm order under test: neither lambda arm can match a colon, and the `fn ":"`
/// arm behind them is what a reader reaches.
#[test]
fn the_expression_lambda_still_says_it_was_removed() {
    for source in [
        "fn main() { let ids = users.map fn: a.id }",
        "fn main() { task::scope fn: a.id }",
    ] {
        let message = rendered(source);
        assert!(
            message.contains("`fn: …` is no longer supported."),
            "{source}: {message}"
        );
        assert!(
            message.contains("help: Write `fn { … }`"),
            "{source}: {message}"
        );
    }
}

/// **Not built, and this is where it stands - a refusal now, not three
/// statements.** Part I 7.2 and ADR-006 D6 write the panic hook as
/// `panic::on_panic fn(info) sync { … }`: the lambda carries an effect
/// annotation, and nothing in the grammar gives a lambda one - a closure's
/// parameter list is followed by its block and by nothing else.
///
/// This test used to assert that the line was **three statements**: the `sync`
/// not part of the lambda, the lambda not part of the call. The reserved-word
/// list ([ADR-051](../../../docs/specification/adr/adr-051.md)) ends that
/// reading - `sync` is not a name, so
/// there is no statement for it to be - and the line is refused where the
/// `sync` is.
///
/// **Which is the whole point of the list.** What is unbuilt is unchanged; what
/// changed is that a program writing the specification's own form is told so
/// instead of quietly meaning something else - and the note says what is not
/// built, rather than that `sync` is not a name, which nobody used it as.
#[test]
fn an_effect_annotation_on_a_lambda_is_refused_where_the_annotation_is() {
    let source = "fn main() { panic::on_panic fn(info) sync { info } }";
    let message = rendered(source);
    assert!(message.contains("`sync`"), "{message}");
    assert!(
        message.contains("`sync` isn't written on a lambda yet"),
        "the note says why, which is what a reader acts on: {message}"
    );
}

/// **`spawn fn { … }` is refused here now**, and the history of this one row is
/// the argument for the reserved-word list in miniature.
///
/// Part I 8.2 writes `spawn fn { … }`; the parser's rule is `spawn "(" expr
/// ")"`, so the keyword form never reached it. It was first read as the
/// *variable* `spawn` followed by a closure (`cannot find value 'spawn'`), and
/// then, once a trailing lambda could reach a path, as a *call* to a function
/// named `spawn` (`cannot find function 'spawn'`). Two different wrong readings,
/// both reported by `rustc` about the generated file, which is the class
/// Part III C.1 forbids.
///
/// `spawn` is a reserved word now, so neither reading exists: there is no
/// variable and no function of that name to be.
///
/// **And the refusal has moved twice since.** It was the parser's, at the `fn`,
/// saying the rule wanted a parenthesis - because `spawn` took `( expr )`, which
/// Part I 8.2 called a bug in the parser rather than a second form. `spawn` now
/// takes the trailing lambda of 5.3, so `spawn fn(x) { … }` *parses*, and what
/// refuses it is `NK2103` (`tasks.rs`): a task is handed nothing, so a named
/// argument has nothing to be bound from. A better place and a better sentence,
/// about the same mistake.
#[test]
fn spawn_with_a_named_lambda_is_refused_rather_than_read_as_something_else() {
    let source = "fn main() { spawn fn(x) { x } }";
    parse_to_ast(source).expect("`spawn fn(x) { … }` parses now");
    // What refuses it is `NK2103`, in `tasks.rs`: the refusal is the checker's
    // now and belongs with the rest of what a task means.

    // And the parenthesised form says what happened to it, rather than
    // *"expected `fn`"*: programs were written against it.
    let message = format!(
        "{:#}",
        parse_to_ast("fn main() { spawn(1) }").expect_err("refused")
    );
    assert!(message.contains("`spawn` takes a lambda"), "{message}");
}

/// **The count comes from the list, and a local called `a` is a local.**
///
/// This is what ADR-049 bought, and it is worth a test of its own. A lambda's
/// arity used to be read off which of `a`, `b`, `c` its body *mentioned*, so a
/// local called `a` inside one became an argument and the closure's arity stopped
/// matching its call. With the arguments written down - the only spelling now - a
/// local of that name is a local.
#[test]
fn a_lambda_takes_its_arguments_from_the_list_and_not_from_its_body() {
    let named =
        lowered("fn ids(xs: Vec[i64]) -> Vec[i64] { return xs.map fn(x) { let a = 1\n x + a } }");
    assert!(named.contains("|x|"), "{named}");
    assert!(!named.contains("|x, a|"), "{named}");

    // The same body with no parameter list is a lambda of no arguments, and the
    // `a` in it is the local the author wrote.
    let none = lowered("fn ids(xs: Vec[i64]) -> Vec[i64] { return xs.map fn { let a = 1\n a } }");
    assert!(none.contains("||"), "{none}");
    assert!(!none.contains("|a|"), "{none}");
}
