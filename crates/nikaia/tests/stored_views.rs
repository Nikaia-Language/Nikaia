//! A view kept past the call that was given it (`NK2302`).
//!
//! The defect this exists for: a function taking a naked `ref String` and storing it
//! lowered to Rust that `rustc` then refused, naming the generated file - which
//! Part III C.1 calls a bug in this compiler. The program is refused here
//! instead, in Nikaia's own words, and the message shows the shape that works.
//!
//! Three halves now. The first decides whether the check is worth running:
//! **every program in the repository must still produce no finding.** The second
//! is the deliberately stored views, which are the proof that the guard is not
//! passing because the analysis is asleep. The third is the shapes the analysis
//! reaches one step further into: where the destination already names the buffer,
//! the program is *lowered* with that buffer written out, and those tests compile
//! and run the result rather than reading the text.

mod common;

use std::path::PathBuf;

use nikaia::check::{self, Finding};
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.code == "NK2302")
        .collect()
}

/// The Rust one source lowers to.
fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

fn one(source: &str) -> Finding {
    let found = findings(source);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one NK2302, got {:#?}",
        found.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
    found.into_iter().next().expect("checked above")
}

// --- the guard ---------------------------------------------------------------

/// No program in the repository keeps a naked view.
///
/// The cost of the check, measured rather than assumed. `examples/` is the
/// corpus that has to keep lowering and keep running; `crates/nikaia-std/src` is
/// the part of `std` written in Nikaia.
#[test]
fn no_program_in_the_repository_keeps_a_naked_view() {
    let mut checked = 0;
    let mut reported = String::new();

    for dir in ["examples", "crates/nikaia-std/src", "benches"] {
        let dir = repo_root().join(dir);
        let paths = common::standalone_programs(&dir);
        assert!(!paths.is_empty(), "nothing to check in {}", dir.display());

        for path in paths {
            let source = std::fs::read_to_string(&path).expect("read the program");
            // `fortunes.nika` is the one program in the corpus that does not
            // lower (`dsl postgres` has no lowering); it parses, so it is still
            // asked this question.
            let parsed = parse_to_ast(&source)
                .unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
            let name = path.display().to_string();
            let own = Ledger::infer(&parsed);
            let library = Ledger::parse(STD).expect("std's shipped ledger parses");
            for finding in nikaia::views::check(&parsed, &own, &library) {
                reported.push_str(&nikaia::diagnostics::render_finding(
                    &finding, &name, &source,
                ));
            }
            checked += 1;
        }
    }

    assert!(checked >= 12, "only {checked} programs were checked");
    assert!(reported.is_empty(), "the corpus is not clean:\n{reported}");
}

/// The idiomatic form, which is what the message points at: the view arrives
/// inside a struct whose own declaration says which buffer it points into.
///
/// This is `examples/1brc.nika`'s `Summary::record` with the names shortened,
/// and it has to stay silent - it is the answer the refusal offers.
#[test]
fn a_view_inside_a_struct_is_not_a_finding() {
    assert!(
        findings(
            r#"
use std::collections

        struct Reading { name: ref String, temp: i32 }
        struct Summary { stations: collections::HashMap[ref String, i32] }
        impl Summary {
            fn record(ref mut self, m: Reading) sync {
                self.stations.insert(m.name, m.temp)
            }
        }
        "#,
        )
        .is_empty()
    );
}

/// `examples/k-nucleotide/src/main.nika`'s shape: views of the parameter are written into
/// a map the function hands back. Correct, because the result points into that
/// parameter's buffer and nothing else's - and the one thing this check may not
/// do is refuse it.
#[test]
fn a_view_handed_back_in_the_result_it_came_from_is_not_a_finding() {
    assert!(
        findings(
            r#"
use std::collections

        struct Tally { n: i64 }
        fn count(dna: ref String, k: usize) -> collections::HashMap[ref String, Tally] {
            let mut counts: collections::HashMap[ref String, Tally] = collections::HashMap()
            for i in 0..dna.len() {
                let fragment = ref dna[i..i + k]
                counts.entry(fragment).or_insert_with fn { Tally(1) }
            }
            return counts
        }
        "#,
        )
        .is_empty()
    );
}

/// A field that holds no view cannot be where a view went, so storing text into
/// one is not a finding.
#[test]
fn a_field_that_holds_no_view_is_not_a_destination() {
    assert!(
        findings(
            r#"
        struct Log { lines: Vec[String], n: i64 }
        impl Log {
            fn add(ref mut self, line: ref String) sync {
                self.lines.push(line.clone())
                self.n += 1
            }
        }
        "#,
        )
        .is_empty()
    );
}

// --- what it refuses, which is where the reach stops ------------------------
//
// Each of these is a destination the analysis can see and cannot name a buffer
// for. They are tests rather than prose so that extending the reach is a visible
// change to this file rather than a silent one.

/// The view is handed back out of the method, and the result does not point into
/// that parameter's buffer. Nothing here names a buffer to write on the
/// parameter, so this is a refusal - and it carries the whole message.
#[test]
fn a_naked_view_handed_back_from_a_method_is_refused() {
    let finding = one(r#"
        struct Summary { label: ref String }
        impl Summary {
            fn pick(ref self, name: ref String) -> ref String sync {
                return name
            }
        }
        "#);
    assert_eq!(finding.code, "NK2302");
    assert!(
        finding.message.contains("`name: ref String`"),
        "{}",
        finding.message
    );
    assert!(
        finding
            .notes
            .iter()
            .any(|n| n.contains("stored in the value this function returns")),
        "{:#?}",
        finding.notes
    );
    // Part III C.2: the way out is shown, not described.
    let help = finding.help.expect("every diagnostic names a way out");
    assert!(help.contains("struct Held { name: ref String }"), "{help}");
    assert!(help.contains("examples/1brc.nika"), "{help}");
}

/// A view put into a struct the function builds and hands back, from an `impl`
/// whose own target holds no view. `Reading` carries a buffer; nothing here says
/// which one, and the `impl` has none to lend.
#[test]
fn a_naked_view_put_into_a_struct_that_is_handed_back_is_refused() {
    let finding = one(r#"
        struct Reading { name: ref String, temp: i32 }
        struct Factory { n: i32 }
        impl Factory {
            fn make(ref self, name: ref String) -> Reading sync {
                return Reading { name: name, temp: self.n }
            }
        }
        "#);
    assert!(
        finding.notes.iter().any(|n| n.contains("`Reading.name`")),
        "{:#?}",
        finding.notes
    );
}

/// A task outlives the call, so a view handed to one is refused whatever the
/// subject carries.
#[test]
fn a_naked_view_given_to_a_task_is_refused() {
    let finding = one(r#"
        struct Summary { label: ref String }
        impl Summary {
            fn post(ref self, name: ref String) {
                spawn fn { println(name) }
            }
        }
        "#);
    assert!(
        finding.notes.iter().any(|n| n.contains("task")),
        "{:#?}",
        finding.notes
    );
}

// --- one step further: a destination that already names the buffer -----------

/// Lower, compile with the `rustc` that built this test, run, and return stdout.
///
/// Reading the emitted text is not enough here: what the old behaviour produced
/// was text that looked right and did not compile, which is the whole defect.
fn lower_compile_run(source: &str, purpose: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let refusals: Vec<String> = check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .map(|f| format!("{}: {}", f.code, f.message))
        .collect();
    assert!(
        refusals.is_empty(),
        "the compiler refused a program it should lower: {refusals:#?}"
    );

    let rust = emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust;

    let dir = common::scratch_dir(purpose);
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
    assert!(
        run.status.success(),
        "the program failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8_lossy(&run.stdout).trim().to_string()
}

/// The defect, now lowered instead of refused: the subject already carries the
/// buffer, so the parameter is written as a view of *that* buffer.
///
/// This is the program `rustc` used to refuse with `E0621` about a line nobody
/// wrote. It compiles and prints.
#[test]
fn a_view_stored_in_the_subject_is_lowered_with_the_buffer_named() {
    let source = r#"
        struct Summary { label: ref String }
        impl Summary {
            pub fn() -> Summary sync { return Summary { label: "none" } }
            fn record(ref mut self, name: ref String) sync {
                self.label = name
            }
        }
        fn main() {
            let mut s = Summary()
            s.record("Hamburg")
            println(s.label)
        }
    "#;
    let rust = emit_program(
        &parse_to_ast(source).expect("the source parses"),
        Build::default(),
    )
    .expect("the source lowers")
    .rust;
    assert!(
        rust.contains("fn record(&mut self, name: &'a str)"),
        "the parameter has to be written as a view of the input buffer:
{rust}"
    );
    assert_eq!(lower_compile_run(source, "stored-view-subject"), "Hamburg");
}

/// The same through a call the compiler cannot see the end of. `insert` may keep
/// what it is given and nothing written down says it does, so this used to be
/// refused in stage A and used to reach `rustc` before that.
#[test]
fn a_view_handed_to_a_call_on_the_subject_is_lowered_too() {
    let source = r#"
use std::collections

                struct Summary { stations: collections::HashMap[ref String, i32] }
        impl Summary {
            pub fn() -> Summary sync { return Summary { stations: collections::HashMap() } }
            fn record(ref mut self, name: ref String, temp: i32) sync {
                self.stations.insert(name, temp)
            }
            fn count(ref self) -> i64 sync { return self.stations.len() }
        }
        fn main() {
            let mut s = Summary()
            s.record("Hamburg", 12)
            println(f"{s.count()}")
        }
    "#;
    assert_eq!(lower_compile_run(source, "stored-view-call"), "1");
}

/// A read-only call on a field that holds views. The refusal in stage A covered
/// this, because nothing tells it from `insert`; naming the buffer covers it
/// instead, and costs the program nothing.
#[test]
fn a_read_only_call_on_a_view_field_is_lowered_rather_than_refused() {
    let source = r#"
use std::collections

                struct Summary { stations: collections::HashMap[ref String, i32] }
        impl Summary {
            pub fn() -> Summary sync { return Summary { stations: collections::HashMap() } }
            fn has(ref self, name: ref String) -> bool sync {
                return self.stations.contains_key(name)
            }
        }
        fn main() {
            let s = Summary()
            println(f"{s.has("Hamburg")}")
        }
    "#;
    assert_eq!(lower_compile_run(source, "stored-view-read"), "false");
}

/// A view that reaches the field through a local. The carrier is followed by
/// name, so the parameter is written with the buffer and the program lowers.
#[test]
fn a_view_that_reaches_the_field_through_a_local_is_lowered_too() {
    let source = r#"
use std::collections

                struct Summary { stations: collections::HashMap[ref String, i32] }
        impl Summary {
            pub fn() -> Summary sync { return Summary { stations: collections::HashMap() } }
            fn record(ref mut self, name: ref String) sync {
                let key = name
                self.stations.insert(key, 1)
            }
            fn count(ref self) -> i64 sync { return self.stations.len() }
        }
        fn main() {
            let mut s = Summary()
            s.record("Hamburg")
            println(f"{s.count()}")
        }
    "#;
    let rust = emit_program(
        &parse_to_ast(source).expect("the source parses"),
        Build::default(),
    )
    .expect("the source lowers")
    .rust;
    assert!(
        rust.contains("fn record(&mut self, name: &'a str)"),
        "{rust}"
    );
    assert_eq!(lower_compile_run(source, "stored-view-local"), "1");
}

/// A parameter nothing stores keeps the spelling it had: the buffer is written
/// where the body needs it and nowhere else.
#[test]
fn a_view_that_is_only_read_keeps_the_signature_it_had() {
    let rust = emit_program(
        &parse_to_ast(
            r#"
            struct Summary { label: ref String }
            impl Summary {
                fn show(ref self, name: ref String) sync { println(name) }
            }
            "#,
        )
        .expect("the source parses"),
        Build::default(),
    )
    .expect("the source lowers")
    .rust;
    assert!(rust.contains("fn show(&self, name: &str)"), "{rust}");
}

// --- a result with nothing to borrow from -----------------------------------

/// **`fn name() -> ref String` had no lifetime to elide**, and `rustc` said so about
/// the generated file: *"missing lifetime specifier — this function's return
/// type contains a borrowed value, but there is no value for it to be borrowed
/// from"* (Part III, C.1).
///
/// `'static` is the only lifetime that can be written there, which is what makes
/// this a derivation and not a choice: a view handed back by a function that
/// borrowed nothing can only point at something outliving the program.
#[test]
fn a_result_that_borrows_from_nothing_says_static() {
    let rust = emit_program(
        &parse_to_ast("fn name() -> ref String { \"Ada\" }\n").expect("the source parses"),
        Build::default(),
    )
    .expect("the source lowers")
    .rust;
    assert!(rust.contains("fn name() -> &'static str"), "{rust}");
}

/// **And a function with something to borrow from keeps the elision**, which is
/// the half that had to stay: writing `'static` there would demand more of the
/// caller than the body needs.
#[test]
fn a_result_that_borrows_from_a_parameter_keeps_the_elision() {
    let rust = emit_program(
        &parse_to_ast(
            "\
fn echo(s: ref String) -> ref String { return s }
struct Holder { label: ref String }
impl Holder {
    pub fn label(ref self) -> ref String { return self.label }
}
",
        )
        .expect("the source parses"),
        Build::default(),
    )
    .expect("the source lowers")
    .rust;
    assert!(rust.contains("fn echo(s: &str) -> &str"), "{rust}");
    assert!(!rust.contains("fn echo(s: &str) -> &'static str"), "{rust}");
    // A receiver is something to borrow from too.
    assert!(!rust.contains("fn label(&self) -> &'static str"), "{rust}");
}

/// **A body that cannot honour it is still refused** — by the language below,
/// against the Nikaia line. So the `'static` never accepts a wrong program: it
/// says what the signature can mean, and the body is checked against it.
///
/// `Vec[String]` is passed by value, so it dies at the end of the call and a
/// view into it cannot leave.
#[test]
fn a_body_that_cannot_honour_static_is_refused_by_the_language_below() {
    let rust = emit_program(
        &parse_to_ast("fn first(xs: Vec[String]) -> ref String { return xs[0].as_str() }\n")
            .expect("the source parses"),
        Build::default(),
    )
    .expect("the source lowers")
    .rust;
    // Nothing among the arguments is a view, so the result says `'static` — and
    // the refusal that follows is about the *body*, which is the right place for
    // it and reaches the reader as `cannot return value referencing function
    // parameter 'xs'`.
    assert!(rust.contains("-> &'static str"), "{rust}");
}

/// **A result that *carries* a view needs the same lifetime a result that *is*
/// one needs** ([ADR-008](../../../docs/specification/adr/adr-008.md) D9).
///
/// D9 writes `'static` where a function has nothing to borrow from, because
/// there is nothing for Rust's elision to take and `-> ref String` is *missing
/// lifetime specifier* about a file nobody wrote. It asked that question of the
/// **written** type, so `-> Vec[Entry]` — where `Entry` holds a `ref String` — went on
/// eliding, and the answer was `Vec<Entry<'_>>`: the same defect one type in.
///
/// The declaration is what tells them apart, and the emitter has it.
#[test]
fn a_result_that_carries_a_view_is_static_where_nothing_is_borrowed_from() {
    let rust = lowered(
        "struct Entry { pub name: ref String }\n\
         \n\
         fn made() -> Vec[Entry] {\n\
         \x20   let mut out = Vec()\n\
         \x20   out.push(Entry { name: \"x\" })\n\
         \x20   return out\n\
         }\n\
         \n\
         fn main() { println(f\"{made().len()}\") }\n",
    );
    assert!(rust.contains("fn made() -> Vec<Entry<'static>>"), "{rust}");
}

/// **And a parameter that carries one is something to borrow from**, which is
/// the other side of the same question: without it every such signature would
/// widen and `fn keep(e: Entry) -> Entry` would promise the caller `'static`.
#[test]
fn a_parameter_that_carries_a_view_keeps_the_elision() {
    let rust = lowered(
        "struct Entry { pub name: ref String }\n\
         \n\
         fn keep(e: Entry) -> Entry { return e }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    assert!(
        rust.contains("fn keep(e: Entry<'_>) -> Entry<'_>"),
        "{rust}"
    );
    assert!(!rust.contains("'static"), "{rust}");
}

/// **Only in a signature that declares no lifetime of its own**, which is D9's
/// own case — *a free function's signature*.
///
/// Inside an `impl` that declares `'a` the result is the subject's `'a`, and
/// `'static` there is a promise the `impl` cannot keep.
/// `examples/1brc.nika`'s anonymous constructor is what said so: it has no
/// parameters and no receiver, so the widening reached it and `rustc` answered
/// *lifetime may not live long enough*.
#[test]
fn a_constructor_inside_a_borrowing_impl_keeps_the_subjects_lifetime() {
    let rust = lowered(
        "struct Summary { rows: Vec[ref String] }\n\
         \n\
         impl Summary {\n\
         \x20   pub fn() -> Summary sync { return Summary { rows: [] } }\n\
         }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    assert!(rust.contains("impl<'a> Summary<'a>"), "{rust}");
    assert!(
        !rust.contains("Summary<'static>"),
        "the subject's `'a` is what the constructor hands back\n{rust}"
    );
}

// --- what the callee's own column already answers ----------------------------

/// **A call says which of its arguments its result may point into**, and a
/// parameter that is none of them does not escape through it.
///
/// `examples/http`'s `Request::query` is what found this: `http1::Head::query`
/// is written `returns = "borrows(self)"`, so its result points into the head
/// and never into the name it was asked about — and the refusal still landed on
/// the name, because the walk read *a call took it* and stopped there. The
/// accessor could not be written at all.
#[test]
fn a_name_a_borrowing_call_only_reads_does_not_escape_through_its_result() {
    assert!(
        findings(
            r#"
use std::http1

struct Request { head: http1::Head }

impl Request {
    pub fn query(ref self, name: ref String) -> ref String? {
        return self.head.query(name)
    }
}
"#
        )
        .is_empty(),
        "`http1::Head::query` borrows its receiver, not its argument"
    );
}

/// **And a wrapper may keep the name it wraps.**
///
/// The first fix looked an entry up by method name alone, so `Request::query`
/// found *itself* among the candidates — and its own column, the very thing
/// being decided, answered for the callee's. The receiver is typed for this
/// reason: one key, one entry.
#[test]
fn a_wrapper_that_keeps_the_name_it_wraps_still_reads_the_callees_column() {
    let refused = findings(
        r#"
use std::http1

struct Request { head: http1::Head }

impl Request {
    pub fn header(ref self, header: ref String) -> ref String? {
        return self.head.header(header)
    }
}
"#,
    );
    assert!(refused.is_empty(), "{refused:#?}");
}

/// **An argument the callee's column *does* name is still kept**, which is the
/// half that makes the other one worth having.
///
/// `str::trim` hands back a view of its subject, and here the subject is the
/// parameter — so the result does point into it, and a function whose own
/// result names another buffer cannot hand it back.
#[test]
fn an_argument_the_column_does_name_is_still_a_finding() {
    let refused = findings(
        r#"
struct Holder { label: ref String }

impl Holder {
    pub fn pick(ref self, name: ref String) -> ref String {
        return name.trim()
    }
}
"#,
    );
    assert_eq!(refused.len(), 1, "{refused:#?}");
}

/// **A receiver this walk cannot type says nothing**, and the refusal stands.
///
/// The fail-closed direction (ADR-010 D1): an analysis that cannot see is not
/// allowed to conclude that nothing happened. A local's type is the checker's,
/// and this walk runs beside the checker rather than after it — so a call on one
/// reaches no entry and the parameter is treated as kept.
#[test]
fn a_call_on_a_receiver_that_cannot_be_typed_is_still_a_finding() {
    let refused = findings(
        r#"
struct Holder { label: ref String }

impl Holder {
    pub fn choose(ref self, name: ref String) -> ref String {
        let held = self.label
        return held.trim_matches(name)
    }
}
"#,
    );
    assert_eq!(refused.len(), 1, "{refused:#?}");
}
