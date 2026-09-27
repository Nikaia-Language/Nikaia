//! Kap 7.1: how a program declares an error, raises one, and catches it.
//!
//! Until this, `throws` and `catch` existed and nothing could *produce* an
//! error: `throw` was in the specification exactly once, defined nowhere, and
//! absent from the parser ([ADR-023](../../../docs/specification/adr/adr-023.md)
//! §1). A program could propagate what `std` handed it and never raise one of
//! its own.

mod common;

use nikaia::contracts::Ledger;
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

/// The ledger this source produces, rendered the way it is committed.
fn ledger_for(source: &str) -> String {
    Ledger::infer(&parse_to_ast(source).expect("the source parses")).render()
}

fn emit(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Kap 4.7: a trait impl names the trait, and the `for` is what tells it from
/// the inherent form. Before this the AST had no room for the name at all.
#[test]
fn an_impl_may_name_a_trait() {
    let rust = emit(
        r#"
        struct User { name: String }
        impl Summarize for User {
            fn summary(ref self) -> String { return self.name }
        }
        "#,
    );
    assert!(
        rust.contains("impl Summarize for User {"),
        "expected a trait impl, got:\n{rust}"
    );
}

/// The inherent form still lowers the way it did - `impl User`, no `for`.
#[test]
fn an_inherent_impl_is_unchanged() {
    let rust = emit(
        r#"
        struct User { name: String }
        impl User {
            fn shout(ref self) -> String { return self.name }
        }
        "#,
    );
    assert!(rust.contains("impl User {"), "{rust}");
    assert!(!rust.contains(" for User"), "{rust}");
}

/// Kap 7.1: `Error` is the one trait the compiler reads rather than relays. The
/// author writes `message`; being `Display`, and being an error at all, are what
/// the failure channel needs and neither is a decision they make.
#[test]
fn an_error_impl_brings_what_the_failure_channel_needs() {
    let rust = emit(
        r#"
        enum ConfigError { NotFound }
        impl Error for ConfigError {
            fn message(ref self) -> String { return "no config" }
        }
        "#,
    );
    assert!(rust.contains("impl ConfigError {"), "{rust}");
    assert!(
        rust.contains("impl std::fmt::Display for ConfigError"),
        "the message has to reach `Display`:\n{rust}"
    );
    assert!(
        rust.contains("f.write_str(&self.message())"),
        "`Display` is written from `message`, not beside it:\n{rust}"
    );
    assert!(
        rust.contains("impl std::error::Error for ConfigError {}"),
        "a value gets into `Box<dyn Error>` by being one:\n{rust}"
    );
}

/// ADR-023 D2: `throw` leaves the function with the value in the failure
/// channel. `Box::new` is the emitter's to write - the language has one way to
/// raise an error, not one way plus a conversion.
#[test]
fn throw_leaves_the_function() {
    let rust = emit(
        r#"
        enum ConfigError { NotFound }
        fn load() -> i64 throws {
            throw ConfigError::NotFound
        }
        "#,
    );
    assert!(
        rust.contains(r#"return Err(nikaia_std::error::raise(ConfigError::NotFound, &"load"))"#),
        "{rust}"
    );
}

/// ADR-023 D8: there is no postfix `?`. It parsed until now, which meant a
/// program could still write the operator the decision removed.
#[test]
fn there_is_no_postfix_question_mark() {
    let parsed = parse_to_ast("fn main() throws { let x = load()? }");
    assert!(
        parsed.is_err(),
        "`?` should no longer parse, but it did: {parsed:?}"
    );
}

/// `??` is the null-coalescing operator of Kap 3.5 and keeps working - removing
/// the postfix `?` must not take its first character with it.
#[test]
fn coalescing_still_parses() {
    let rust = emit(
        r#"use std::cli

fn main() { let p = cli::args().nth(1) ?? "x" }"#,
    );
    // **`index::or` and not `unwrap_or_else`** since
    // [ADR-161](../../../docs/specification/adr/adr-161.md) D2: the left of a
    // `??` may be a view into a container, so the two sides do not have the
    // same type and the language below is what joins them.
    assert!(rust.contains("nikaia_std::index::or("), "{rust}");
}

/// The whole chapter in one program, compiled and run: an error type with a
/// payload, raised, caught, and its message printed.
#[test]
fn an_error_is_declared_raised_caught_and_printed() {
    let source = r#"
        enum ConfigError { NotFound(String) }

        impl Error for ConfigError {
            fn message(ref self) -> String {
                match self {
                    ConfigError::NotFound(p) => { return f"no config at {p}" }
                }
            }
        }

        fn load(path: String) -> String throws {
            throw ConfigError::NotFound(path)
        }

        fn main() {
            let text = load("app.conf".to_string()) catch {
                println(f"{error}")
                return
            }
            println(f"{text}")
        }
    "#;
    let rust = emit(source);
    // **`throwing` and not `raise`** since
    // [ADR-157](../../../docs/specification/adr/adr-157.md) D1: this program's
    // error set is one type it declares, so the channel is that type and the
    // value goes in unboxed. `raise` is what the **box** takes, and the two
    // have the same job — a `throw` puts the value in the failure channel with
    // the site the compiler knew (ADR-023 D6).
    assert!(
        rust.contains("return Err(nikaia_std::error::throwing("),
        "{rust}"
    );
    assert!(
        rust.contains("impl std::error::Error for ConfigError"),
        "{rust}"
    );

    let dir = common::scratch_dir("errors");
    let path = dir.join("prog.rs");
    std::fs::write(&path, &rust).expect("write the emitted Rust");
    let binary = dir.join("prog");
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
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let run = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    let out = String::from_utf8_lossy(&run.stdout);
    assert_eq!(out.trim(), "no config at app.conf", "stdout was: {out:?}");
}

/// ADR-023 D8: `throws` propagates on its own, and the lowering is where that
/// becomes true rather than merely said.
///
/// Nothing marks a failing call in Nikaia; the language below marks every one,
/// so the `?` is written here. Before this the emitted Rust was `Ok(liest())`,
/// a `Result` inside an `Ok`, and the only shape of Kap 7.1 that lowered at all
/// was a `catch` at the call. The whole corpus uses one, which is why no test
/// found it.
///
/// Compiled and run, because "the emitted Rust is what `rustc` then rejects"
/// is exactly what an assertion about the text would not have caught.
#[test]
fn a_written_call_propagates_its_failure() {
    let source = r#"
        enum ConfigError { NotFound(String) }

        impl Error for ConfigError {
            fn message(ref self) -> String {
                match self {
                    ConfigError::NotFound(p) => { return f"no config at {p}" }
                }
            }
        }

        fn load(path: String) -> String throws {
            throw ConfigError::NotFound(path)
        }

        fn ruft(path: String) -> String throws {
            return load(path)
        }

        fn main() {
            let text = ruft("app.conf".to_string()) catch {
                println(f"{error}")
                return
            }
            println(text)
        }
    "#;
    let rust = emit(source);
    // The written call takes the `?`...
    assert!(rust.contains("Ok(load(path)?)"), "{rust}");
    // ...and the `catch` does not, because the `match` beside it is what
    // handles the failure.
    assert!(rust.contains("match ruft("), "{rust}");
    assert!(!rust.contains("ruft(\"app.conf\".to_string())?"), "{rust}");

    let dir = common::scratch_dir("propagate");
    let path = dir.join("prog.rs");
    std::fs::write(&path, &rust).expect("write the emitted Rust");
    let binary = dir.join("prog");
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
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let run = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    let out = String::from_utf8_lossy(&run.stdout);
    assert_eq!(
        out.trim(),
        "no config at app.conf",
        "the failure has to have travelled through `ruft`; stdout was: {out:?}"
    );
}

/// The same, for a call on a **receiver**: `s.add(1)` takes the `?` too.
///
/// ADR-023 D8 does not distinguish the two shapes, and until now the compiler
/// did: a fallible method call lowered only through a `catch`, so passing the
/// failure on - the thing D8 says a call does by itself - was the one case you
/// had to write more code for than the code you wanted.
///
/// What changed is not that the emitter learned to resolve receivers. There is
/// one type checker (ADR-028), and it is the one that knows `s` is a `Stats` and
/// that `Stats::add` carries `throws`. It writes that answer down as
/// `check::Checked::fallible_methods` and the emitter looks it up, exactly as
/// `fallible_loops` has been handed over since ADR-025 D7.
///
/// Compiled and run, because `Ok(s.add(1))` is a `Result` inside an `Ok` and
/// only `rustc` says so.
#[test]
fn a_method_call_propagates_its_failure() {
    let source = r#"
        enum ZuVoll { Voll }

        impl Error for ZuVoll {
            fn message(ref self) -> String { return "too full".to_string() }
        }

        struct Stats { n: i64 }

        impl Stats {
            pub fn(n: i64) -> Stats { return Stats { n: n } }
            fn add(ref self, v: i64) -> i64 throws {
                if self.n + v > 100 { throw ZuVoll::Voll }
                return self.n + v
            }
        }

        fn record(s: Stats) -> i64 throws {
            return s.add(1)
        }

        fn main() {
            let first = record(Stats(2)) catch {
                println(f"{error}")
                return
            }
            println(f"{first}")
            let second = record(Stats(500)) catch {
                println(f"{error}")
                return
            }
            println(f"{second}")
        }
    "#;
    let rust = emit(source);
    assert!(rust.contains("Ok(s.add(1)?)"), "{rust}");

    let dir = common::scratch_dir("propagate-method");
    let path = dir.join("prog.rs");
    std::fs::write(&path, &rust).expect("write the emitted Rust");
    let binary = dir.join("prog");
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
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let run = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    let out = String::from_utf8_lossy(&run.stdout);
    assert_eq!(
        out.trim(),
        "3\ntoo full",
        "the failure has to have travelled out of `record`; stdout was: {out:?}"
    );
}

/// And the guarded half of a `catch` still does not take one.
///
/// One rule, asked in one place: the emitter's condition for a method call is
/// the same `flow.throws && !flow.caught && <the callee can fail>` the call by
/// name is given, and only the last question is answered from somewhere else.
/// So `s.add(1) catch { … }` keeps the `Result` the `match` beside it needs,
/// and the next call in the same function still propagates.
#[test]
fn a_caught_method_call_keeps_its_result() {
    let rust = emit(
        r#"
        enum ZuVoll { Voll }

        struct Stats { n: i64 }

        impl Stats {
            fn add(ref self, v: i64) -> i64 throws {
                if self.n + v > 100 { throw ZuVoll::Voll }
                return self.n + v
            }
        }

        fn record(s: Stats) -> i64 throws {
            let first = s.add(1) catch { return 0 }
            return first + s.add(2)
        }
        "#,
    );
    assert!(rust.contains("match s.add(1) {"), "{rust}");
    assert!(!rust.contains("s.add(1)?"), "{rust}");
    assert!(rust.contains("Ok(first + s.add(2)?)"), "{rust}");
}

/// A method no ledger describes is left exactly as it was.
///
/// The emitter adds nothing on a guess, and the checker's answer is an answer
/// about calls it resolved: `text.len()` is `std`'s and cannot fail, and a
/// method on a receiver whose type is not known produces no entry at all. Both
/// come out without a `?`.
#[test]
fn a_method_that_cannot_fail_takes_no_question_mark() {
    let rust = emit(
        r#"
        fn size(text: String) -> i64 throws {
            return text.len()
        }
        "#,
    );
    // `as i64` and no `?`: a length is an `i64` (ADR-048 D1) and the conversion
    // is the emitter's, which is a different thing from a failure being added.
    assert!(rust.contains("Ok(text.len() as i64)"), "{rust}");
}

/// `throws` names no type, and saying so is the parser's job (ADR-023 D1).
///
/// The specification wrote `throws IoError` in four places, so this is a form
/// a reader will try. It used to be a parse error at the type name offering
/// `->` and `sync` as alternatives - which says nothing about why - and the
/// precedent for a sentence instead is `ADR-022`'s removed `fn: …`.
#[test]
fn throws_with_a_type_is_refused_with_the_reason() {
    let refused = parse_to_ast("fn f() throws IoError { return 1 }")
        .expect_err("`throws IoError` must not parse");
    let message = format!("{refused:#}");
    assert!(
        message.contains("`throws` names no error type"),
        "{message}"
    );
    assert!(message.contains("ADR-023 D1"), "{message}");
    assert!(message.contains("nikaia.contracts"), "{message}");

    // And every legal placement still parses. **`sync` before `throws`**, in
    // both positions, since [ADR-140](../../../docs/specification/adr/adr-140.md)
    // D4 left one slot: `fn b() throws sync { }` used to parse only because the
    // two words sat in different ones.
    for legal in [
        "fn a() throws { }",
        "fn b() sync throws { }",
        "fn c() -> i64 throws { return 1 }",
        "fn d() -> i64 sync { return 1 }",
        "fn e() -> i64 sync throws { return 1 }",
    ] {
        assert!(parse_to_ast(legal).is_ok(), "{legal} should parse");
    }

    // And the other order is refused with the sentence, rather than with
    // *expected `{`* about a form that was legal a version ago.
    for backwards in [
        "fn f() throws sync { }",
        "fn g() -> i64 throws sync { return 1 }",
    ] {
        let said = format!(
            "{:#}",
            parse_to_ast(backwards).expect_err("`throws sync` must not parse")
        );
        assert!(said.contains("`sync` stands before `throws`"), "{said}");
    }
}

// --- the ledger ------------------------------------------------------------

/// ADR-023 D1: `throws` in the source says *that* a function fails; the ledger
/// says with what, inferred over the call graph.
#[test]
fn the_ledger_names_the_errors_a_function_throws() {
    let ledger = ledger_for(
        r#"
        enum ConfigError { NotFound }
        fn load() -> i64 throws { throw ConfigError::NotFound }
        "#,
    );
    assert!(
        ledger.contains(r#"throws = ["ConfigError"]"#),
        "expected the error named, got:\n{ledger}"
    );
}

/// Nothing marks a failing call (D8), so propagation is what a call does - and
/// the set grows along the call graph without anyone writing it down.
#[test]
fn an_error_set_grows_through_a_caller() {
    let ledger = ledger_for(
        r#"
        enum ConfigError { NotFound }
        enum NetError { Timeout }
        fn load() -> i64 throws { throw ConfigError::NotFound }
        fn fetch() -> i64 throws { throw NetError::Timeout }
        fn both() -> i64 throws { let a = load() return fetch() }
        "#,
    );
    let both = ledger
        .split("[fn.\"both\"]")
        .nth(1)
        .expect("an entry for `both`");
    assert!(
        both.contains(r#"throws = ["ConfigError", "NetError"]"#),
        "both callees' errors should reach it:\n{ledger}"
    );
}

/// Mutual recursion terminates because the sets only grow and the names are
/// finite - the same reason `sync`'s greatest fixpoint terminates going the
/// other way.
#[test]
fn mutual_recursion_settles() {
    let ledger = ledger_for(
        r#"
        enum E { X }
        fn ping(n: i32) -> i32 throws { if n == 0 { throw E::X } return pong(n - 1) }
        fn pong(n: i32) -> i32 throws { return ping(n - 1) }
        "#,
    );
    let pong = ledger
        .split("[fn.\"pong\"]")
        .nth(1)
        .expect("an entry for `pong`");
    assert!(
        pong.contains(r#"throws = ["E"]"#),
        "`pong` reaches `ping`'s error:\n{ledger}"
    );
}

/// A failure the compiler cannot name is `?`, ADR-024 D1's absence of a claim -
/// never silence, because reading "I cannot see it" as "it does not fail" is
/// the direction ADR-010 D1 calls a vulnerability generator.
#[test]
fn what_cannot_be_named_is_a_question_mark() {
    // **`std` is no longer the example.** It was: every entry wrote `["?"]`,
    // and this test read `io::read_to_string` to see one. Since
    // [ADR-158](../../../docs/specification/adr/adr-158.md) D1 `std` names what
    // it throws, so the rule needs a call that genuinely cannot be named — one
    // **no ledger describes**, which is what `"?"` has always meant.
    let named = ledger_for(
        r#"use std::io

fn reads() -> String throws { return io::read_to_string() }"#,
    );
    assert!(
        named.contains(r#"throws = ["io::IoError"]"#),
        "`std` names what it throws:\n{named}"
    );

    let unnamed = ledger_for(&format!(
        "fn reads(s: String) -> String throws {{ return {} }}\n",
        common::undescribed_value("s")
    ));
    assert!(
        unnamed.contains(r#"throws = ["?"]"#),
        "a call no ledger describes can fail with something nobody wrote down:\n{unnamed}"
    );
}

/// A declared `throws` never loses its entry. The declaration is a promise a
/// caller already relies on, and inference is here to say more than it.
#[test]
fn a_declared_throws_keeps_its_entry() {
    let ledger = ledger_for(r#"fn maybe() -> i64 throws { return 1 }"#);
    assert!(ledger.contains(r#"throws = ["?"]"#), "{ledger}");
}

/// A function that cannot fail says nothing, because the file says only what is
/// true (13.5).
#[test]
fn a_function_that_cannot_fail_has_no_entry() {
    let ledger = ledger_for(r#"fn pure(n: i64) -> i64 { return n }"#);
    // The header explains the key, so look for the key being *set*.
    assert!(!ledger.contains("throws = "), "{ledger}");
}

/// **The ledger never publishes "cannot fail" about a body that can.**
///
/// The subject here is the committed file, not the message. `nikaia.contracts`
/// is read by other programs ([ADR-020](../../../docs/specification/adr/adr-020.md)),
/// so the worst outcome of the hole `NK2605` closes was never the missing
/// caret: it was this program compiling and shipping
///
/// ```toml
/// [fn."ruft"]
/// signature = "() -> String"
/// ```
///
/// with no `throws` at all.
///
/// **Which of the two answers, and why this one.** The inference is *not*
/// extended to give `ruft` a `throws` it never declared, the way `sync` is
/// inferred. ADR-023 D1 derives the error **set** and leaves the declaration
/// in the source - "in source, `throws` is bare" - and ADR-025 D1 says what
/// happens when a body contradicts it in so many words: "the function must
/// declare `throws`, and the compiler says which implicit call is the reason".
/// Inferring the keyword would also have to know about `catch`, which this
/// walk does not: `fn f() { g() catch { … } }` cannot fail, and an inference
/// blind to that would publish the *opposite* false fact about it. So the
/// program is refused, before `Ledger::render` is ever reached - `project.rs`
/// checks every unit and then renders - and a wrong fact that is never
/// computed is never published.
#[test]
fn the_ledger_is_never_published_for_a_caller_that_does_not_say_it_can_fail() {
    let source = "use std::fs\n\nfn liest() -> String throws { return fs::read_to_string(\"x.txt\", fs::Root::Anywhere) }\n\
                  fn ruft() -> String { return liest() }\n\
                  fn main() { }";

    // What the ledger *would* say, which is why the program may not get there.
    let would_say = ledger_for(source);
    assert!(
        would_say.contains("[fn.\"ruft\"]\nsignature = \"() -> String\"\n"),
        "the inference records the declaration as written:\n{would_say}"
    );

    // And it does not get there. `project::lower` checks every unit and only
    // then calls `Ledger::render`, so a refusal here is the file never being
    // written - there is no path in the compiler that renders a ledger for a
    // program this says no to.
    let dir = common::scratch_dir("ledger-refused");
    let path = dir.join("prog.nika");
    std::fs::write(&path, source).expect("write the program");
    let parsed = parse_to_ast(source).expect("the source parses");
    let refused = nikaia::project::check(
        &parsed,
        &Ledger::infer(&parsed),
        &std::collections::BTreeSet::new(),
        // Nothing declared and nothing described, which is a loose file's own
        // state: ADR-104 D1 asks a question about the *build*'s boundary and
        // this one has none. And nothing has gained an error since a committed
        // ledger, which is every build but the one after a change (ADR-101 D1).
        nikaia::project::Around {
            foreign: &nikaia::project::Foreign::default(),
            newly: &nikaia::check::Newly::new(),
            // One file is the whole program here, so a `comptime` has
            // nowhere else to call into.
            beside: &[],
            // …and this build was given no allowlist, so it reads nothing
            // while it builds (ADR-072 D1).
            reads: &nikaia::assets::Reads::none(),
        },
        &path,
        source,
        "no",
    );
    let error = format!("{:#}", refused.expect_err("the build must refuse this"));
    assert!(error.contains("can fail without saying so"), "{error}");

    // Declared, the entry says what is true - and since
    // [ADR-158](../../../docs/specification/adr/adr-158.md) D1 it says it with
    // a **name**: the set that reaches `ruft` is `fs::read_to_string`'s, and
    // `std` writes that down now. It used to be `["?"]`, which is
    // [ADR-024](../../../docs/specification/adr/adr-024.md) D1's absence of a
    // claim, and the entry said no more than the boolean before it.
    let declared = ledger_for(
        "use std::fs\n\nfn liest() -> String throws { return fs::read_to_string(\"x.txt\", fs::Root::Anywhere) }\n\
         fn ruft() -> String throws { return liest() }\n\
         fn main() { }",
    );
    assert!(
        declared
            .contains("[fn.\"ruft\"]\nthrows = [\"io::IoError\"]\nsignature = \"() -> String\"\n"),
        "{declared}"
    );
}

// --- the site, and the trace ------------------------------------------------

/// ADR-023 D6: an error knows where it was raised, and the compiler wrote that
/// down rather than the author. `raise` is what carries it.
#[test]
fn a_throw_carries_the_site_it_came_from() {
    let rust = emit(
        r#"
        enum E { X }
        fn load() -> i64 throws { throw E::X }
        "#,
    );
    assert!(
        rust.contains(r#"nikaia_std::error::raise(E::X, &"load")"#),
        "the raise site should be in the call:\n{rust}"
    );
}

/// …and `main` is not an exception, though its name in the lowering is.
///
/// [ADR-038](../../../docs/specification/adr/adr-038.md) D4 gives `fn main` to
/// the runtime and emits the program's own entry point under a name the author
/// never wrote. The *site* is the author's word, so it stays `main`: a site
/// naming the emitter's wrapper would be ADR-023 D6's whole point undone by an
/// implementation detail.
#[test]
fn a_throw_in_main_names_main_and_not_the_lowering() {
    let rust = emit(
        r#"
        enum E { X }
        fn main() throws { throw E::X }
        "#,
    );
    assert!(
        rust.contains(r#"nikaia_std::error::raise(E::X, &"main")"#),
        "the site is the name the author wrote:\n{rust}"
    );
    assert!(
        rust.contains("fn __nikaia_main"),
        "…and the lowering did rename the function, or this proves nothing:\n{rust}"
    );
}

/// The whole of Kap 7.1's reporting rule, compiled and run: `{error}` is the
/// message and nothing else, `error.full()` adds the site, and the trace is
/// absent unless the program asked - with its absence stated rather than left
/// to be guessed at.
#[test]
fn short_is_safe_and_full_is_asked_for() {
    let source = r#"
        enum ConfigError { NotFound(String) }

        impl Error for ConfigError {
            fn message(ref self) -> String {
                match self {
                    ConfigError::NotFound(p) => { return f"no config at {p}" }
                }
            }
        }

        fn load(path: String) -> String throws {
            throw ConfigError::NotFound(path)
        }

        fn main() {
            let text = load("app.conf".to_string()) catch {
                println(f"short: {error}")
                println(f"full: {error.full()}")
                return
            }
            println(text)
        }
    "#;
    let rust = emit(source);
    let dir = common::scratch_dir("trace");
    let path = dir.join("prog.rs");
    std::fs::write(&path, &rust).expect("write the emitted Rust");
    let binary = dir.join("prog");
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
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let run = std::process::Command::new(&binary)
        .env_remove("NIKAIA_TRACE")
        .output()
        .expect("run the program");
    let out = String::from_utf8_lossy(&run.stdout);

    // The short form is the message the author wrote, and nothing else. It is
    // what a generic 500 may carry (ADR-018).
    assert!(
        out.contains("short: no config at app.conf"),
        "stdout was:\n{out}"
    );
    // The full form adds where it came from...
    assert!(out.contains("raised at load"), "stdout was:\n{out}");
    // ...and says a trace was not captured, rather than leaving a reader to
    // wonder whether one was lost.
    assert!(out.contains("NIKAIA_TRACE=1"), "stdout was:\n{out}");

    // Asked for, there is one - and `NIKAIA_TRACE` alone has to be enough.
    // `RUST_BACKTRACE` is cleared deliberately: leaving it to the environment
    // is what once let a `Backtrace::capture()` that needs it pass locally and
    // fail in CI.
    let traced = std::process::Command::new(&binary)
        .env("NIKAIA_TRACE", "1")
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .output()
        .expect("run the program with tracing on");
    let traced = String::from_utf8_lossy(&traced.stdout);
    assert!(
        !traced.contains("NIKAIA_TRACE=1"),
        "with the switch on there should be a trace:\n{traced}"
    );
}
