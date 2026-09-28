//! The type checker (ADR-024).
//!
//! Two halves, and the first is the one that decides whether the tool is worth
//! running: **every program in the repository must produce no findings.** A
//! checker that reports something about correct code is worse than none, so the
//! corpus is the guard and the deliberately-wrong programs below are the proof
//! that the guard is not passing because the checker is asleep.

mod common;

use std::path::PathBuf;

use nikaia::check::{self, Finding, Severity};
use nikaia::contracts::{Ledger, STD};
use nikaia::parser::parse_to_ast;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check(&parsed, &own, &library).findings
}

/// The one finding a source is written to produce, with its code.
fn one(source: &str) -> (String, String) {
    let found = findings(source);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one finding, got {:#?}",
        found.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
    (found[0].code.to_string(), found[0].message.clone())
}

// --- the guard ---------------------------------------------------------------

/// Every `.nika` in the repository is checked, and none of them has anything
/// wrong with it.
///
/// This is the test that keeps the checker honest. `Ty::Unknown` exists so that
/// the checker says nothing where it knows nothing; the way to find out whether
/// that discipline held is to run it over every program there is.
#[test]
fn no_program_in_the_repository_has_a_type_error() {
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let mut checked = 0;
    let mut reported = String::new();

    for dir in ["examples", "crates/nikaia-std/src"] {
        let dir = repo_root().join(dir);
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .map(|entry| entry.expect("dir entry").path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("nika"))
            .collect();
        paths.sort();

        for path in paths {
            let source = std::fs::read_to_string(&path).expect("read the program");
            let parsed = parse_to_ast(&source)
                .unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
            let own = Ledger::infer(&parsed);
            let name = path.display().to_string();
            for finding in check::check(&parsed, &own, &library).findings {
                reported.push_str(&nikaia::diagnostics::render_finding(
                    &finding, &name, &source,
                ));
            }
            checked += 1;
        }
    }

    assert!(checked >= 8, "only {checked} programs were checked");
    assert!(reported.is_empty(), "the corpus is not clean:\n{reported}");
}

// --- what it catches ---------------------------------------------------------

/// A call that passes the wrong number of arguments, answered from this
/// program's own contracts.
#[test]
fn a_call_with_too_few_arguments_is_reported() {
    let (code, message) = one("fn add(a: i32, b: i32) -> i32 { return a + b }\n\
         fn main() { let n = add(1) }");
    assert_eq!(code, "NK1101");
    assert_eq!(message, "`add` takes 2 arguments, and this call passes 1");
}

/// …and one answered from `std`'s shipped ledger, which is what makes the
/// ledger worth shipping (Part III, 13.5).
#[test]
fn a_call_into_std_is_checked_against_the_shipped_ledger() {
    let (code, message) =
        one("use std::io\n\nfn main() throws { let text = io::read_to_string(\"x\") }");
    assert_eq!(code, "NK1101");
    assert_eq!(
        message,
        "`io::read_to_string` takes 0 arguments, and this call passes 1"
    );
    assert_eq!(
        findings("use std::io\n\nfn main() throws { let text = io::read_to_string(\"x\") }")[0]
            .help
            .as_deref(),
        Some("call it as `io::read_to_string()`")
    );
}

/// An argument of the wrong type, with the name the callee gave it.
#[test]
fn an_argument_of_the_wrong_type_is_reported() {
    // A **view** and not a literal: a literal handed to a `String` is one
    // (ADR-207 D2). And a **published** parameter text of its own also goes
    // into, which stays text of its own (ADR-223 D2), so the view still needs
    // its copy written.
    let (code, message) = one("pub fn greet(who: String) -> String { return who }\n\
         pub fn hello(w: ref String) -> String { return greet(w) }\n\
         pub fn world() -> String { return greet(f\"world\") }");
    assert_eq!(code, "NK1102");
    assert_eq!(
        message,
        "`greet` takes `who: String`, and this call passes `ref String`"
    );
}

/// A `let` that says one thing and is given another.
#[test]
fn a_let_annotation_that_disagrees_is_reported() {
    let (code, message) = one("fn main() { let n: i32 = 'x' }");
    assert_eq!(code, "NK1103");
    assert_eq!(message, "this is `char`, and the `let` says `i32`");
}

/// A `return` that does not hand back what the function declared.
#[test]
fn a_return_of_the_wrong_type_is_reported() {
    let (code, message) = one("fn width() -> i32 { return \"wide\" }");
    assert_eq!(code, "NK1104");
    assert_eq!(
        message,
        "this returns `ref String`, and the function declares `i32`"
    );
}

/// The last expression of a body is what the function hands back, so it
/// answers to the declared type exactly as a `return` does.
#[test]
fn a_tail_expression_of_the_wrong_type_is_reported() {
    let (code, _) = one("fn label(name: ref String) -> i32 {\n\
         \x20   name\n\
         }");
    assert_eq!(code, "NK1104");
}

/// An assignment of the wrong type.
#[test]
fn an_assignment_of_the_wrong_type_is_reported() {
    let (code, message) = one("fn main() {\n\
         \x20   let mut n: i32 = 0\n\
         \x20   n = 'x'\n\
         }");
    assert_eq!(code, "NK1105");
    assert_eq!(
        message,
        "this is `char`, and what it is assigned to is `i32`"
    );
}

/// A struct literal whose field is given the wrong type.
#[test]
fn a_struct_field_of_the_wrong_type_is_reported() {
    // Published, and text of its own flows in too: the field stays text of
    // its own (ADR-222 D2), so the view is the wrong type for it.
    let (code, message) = one("pub struct Reading { pub name: String, pub temp: i32 }\n\
         pub fn read(city: ref String) -> Reading { return Reading { name: city, temp: 12 } }\n\
         pub fn made() -> Reading { return Reading { name: f\"x\", temp: 1 } }");
    assert_eq!(code, "NK1106");
    assert_eq!(
        message,
        "`Reading.name` is `String`, and this is `ref String`"
    );
}

/// A field that is not there, with the one that was probably meant.
#[test]
fn a_field_that_does_not_exist_is_reported() {
    let (code, message) = one("struct Reading { name: ref String, temp: i32 }\n\
         fn label(r: ref Reading) -> ref String { return r.nmae }");
    assert_eq!(code, "NK1107");
    assert_eq!(message, "`Reading` has no field `nmae`");
    assert_eq!(
        findings(
            "struct Reading { name: ref String, temp: i32 }\n\
                  fn label(r: ref Reading) -> ref String { return r.nmae }"
        )[0]
        .help
        .as_deref(),
        Some("did you mean `name`?")
    );
}

/// …and one in a struct literal, which is the same mistake in the other place.
#[test]
fn a_field_that_does_not_exist_in_a_literal_is_reported() {
    let (code, message) = one("struct Reading { name: ref String }\n\
         fn main() { let r = Reading { nmae: \"Hamburg\" } }");
    assert_eq!(code, "NK1107");
    assert_eq!(message, "`Reading` has no field `nmae`");
}

/// A `while` decides on a `bool` too, and says which loop it is about.
#[test]
fn a_while_condition_that_is_not_a_bool_is_reported() {
    let (code, message) = one("fn main() {\n\
         \x20   let name = \"Hamburg\"\n\
         \x20   while name { }\n\
         }");
    assert_eq!(code, "NK1108");
    assert_eq!(message, "this is `ref String`, and a condition is a `bool`");
    assert_eq!(
        findings(
            "fn main() {\n\
             \x20   let name = \"Hamburg\"\n\
             \x20   while name { }\n\
             }"
        )[0]
        .notes[0],
        "a `while` repeats while a `bool` holds"
    );
}

/// A condition that is not a `bool`.
#[test]
fn a_condition_that_is_not_a_bool_is_reported() {
    let (code, message) = one("fn main() {\n\
         \x20   let name = \"Hamburg\"\n\
         \x20   if name { }\n\
         }");
    assert_eq!(code, "NK1108");
    assert_eq!(message, "this is `ref String`, and a condition is a `bool`");
}

/// A grammar's action builds the rule's value, so it answers to the rule's
/// declared type - and a struct literal in one is checked like any other.
#[test]
fn a_grammar_action_is_checked_against_its_rule() {
    let (code, message) = one("struct Reading { name: ref String, temp: i32 }\n\
         grammar Measurements {\n\
         \x20   rule LINE -> Reading = name:until(\";\") \";\" temp:digit\n\
         \x20       { Reading { nmae: name, temp: temp } }\n\
         }");
    assert_eq!(code, "NK1107");
    assert_eq!(message, "`Reading` has no field `nmae`");
}

/// …and an action that builds something else entirely.
#[test]
fn a_grammar_action_of_the_wrong_type_is_reported() {
    let (code, message) = one("grammar Measurements {\n\
         \x20   rule COUNT -> i32 = s:until(\";\") { \"one\" }\n\
         }");
    assert_eq!(code, "NK1104");
    assert_eq!(
        message,
        "this action builds `ref String`, and its rule declares `i32`"
    );
}

/// A `for` over a list whose element type is written down binds that type.
#[test]
fn a_loop_binds_the_element_type_of_a_list() {
    let (code, message) = one("struct Row { id: i32 }\n\
         fn count(rows: Vec[Row]) {\n\
         \x20   for row in rows { let n: i32 = row.idd }\n\
         }");
    assert_eq!(code, "NK1107");
    assert_eq!(message, "`Row` has no field `idd`");
}

// --- the subject ; config protocol (Kap 5.1) ---------------------------------

/// An option the callee does not have, with the one that was probably meant.
#[test]
fn an_option_that_does_not_exist_is_reported() {
    let (code, message) = one("fn request(url: ref String; timeout: i32 = 30) { }\n\
         fn main() { request(\"x\"; timout: 5) }");
    assert_eq!(code, "NK1109");
    assert_eq!(message, "`request` has no option `timout`");
    assert_eq!(
        findings(
            "fn request(url: ref String; timeout: i32 = 30) { }\n\
             fn main() { request(\"x\"; timout: 5) }"
        )[0]
        .help
        .as_deref(),
        Some("did you mean `timeout`?")
    );
}

/// …and one passed to a function that has no `;` at all.
#[test]
fn an_option_on_a_function_that_takes_none_is_reported() {
    let (code, _) = one("fn plain(a: i32) { }\n\
         fn main() { plain(1; loud: true) }");
    assert_eq!(code, "NK1109");
}

/// An option is checked by type like anything else.
#[test]
fn an_option_of_the_wrong_type_is_reported() {
    let (code, message) = one("fn request(url: ref String; timeout: i32 = 30) { }\n\
         fn main() { request(\"x\"; timeout: \"soon\") }");
    assert_eq!(code, "NK1106");
    assert_eq!(
        message,
        "`request` takes `timeout: i32`, and this passes `ref String`"
    );
}

/// Options do not count towards the arity: leaving every one of them out is
/// what a default is for.
#[test]
fn options_are_not_counted_as_arguments() {
    assert!(
        findings(
            "fn request(url: ref String; timeout: i32 = 30, method: ref String = \"GET\") { }\n\
         fn main() {\n\
         \x20   request(\"x\")\n\
         \x20   request(\"x\"; timeout: 5)\n\
         \x20   request(\"x\"; method: \"POST\", timeout: 5)\n\
         }"
        )
        .is_empty()
    );
}

/// `std`'s options are read from the ledger it ships, like everything else.
#[test]
fn an_option_of_a_library_function_is_checked_from_its_ledger() {
    assert!(
        findings("use std::fs\n\nfn main() throws { fs::write(\"o\", fs::Root::Anywhere, \"x\"; append: true) }")
            .is_empty()
    );

    let (code, message) = one(
        "use std::fs\n\nfn main() throws { fs::write(\"o\", fs::Root::Anywhere, \"x\"; apend: true) }",
    );
    assert_eq!(code, "NK1109");
    assert_eq!(message, "`fs::write` has no option `apend`");
}

/// An interpolated string is a `format!` in the emitted Rust, and a `format!`
/// is a `String`. Two spellings in Nikaia, two types below - and the checker
/// follows the lowering rather than the syntax.
#[test]
fn an_interpolated_string_is_a_string_and_a_plain_one_is_a_view() {
    assert!(findings("fn label(n: i32) -> String { return f\"{n} rows\" }").is_empty());

    let (code, message) = one("fn label(n: i32) -> ref String { return f\"{n} rows\" }");
    assert_eq!(code, "NK1104");
    assert_eq!(
        message,
        "this returns `String`, and the function declares `ref String`"
    );
}

// --- what a literal hides ----------------------------------------------------

/// A hole is Nikaia source, and it is checked like Nikaia source.
///
/// The same mistake was caught outside a hole and passed silently inside one,
/// which is worth a test of its own: `println("{…}")` is the most-written
/// syntax in the language, so the blind spot covered most of what people type.
#[test]
fn a_hole_in_a_string_is_checked_like_anything_else() {
    let outside = one("fn add(a: i32, b: i32) -> i32 { return a + b }\n\
         fn main() { let n = add(1) }");
    let inside = one("fn add(a: i32, b: i32) -> i32 { return a + b }\n\
         fn main() { println(f\"{add(1)}\") }");
    assert_eq!(
        inside, outside,
        "a hole is checked differently from a statement"
    );
    assert_eq!(inside.0, "NK1101");
}

/// …and a template's holes too (ADR-017), which reach the emitter the same way.
#[test]
fn a_hole_in_a_template_is_checked_too() {
    let (code, _) = one("fn add(a: i32, b: i32) -> i32 { return a + b }\n\
         fn page() -> String {\n\
         \x20   return dsl html {\n\
         \x20       <p>{add(1)}</p>\n\
         \x20   } eod\n\
         }");
    assert_eq!(code, "NK1101");
}

/// A hole that does not parse is the emitter's to report, in its own words at
/// the place it happens. The checker says nothing rather than guessing.
#[test]
fn a_hole_that_does_not_parse_is_not_the_checkers_business() {
    assert!(findings("fn main() { println(f\"{let}\") }").is_empty());
}

// --- a loop whose step can fail (ADR-025) ------------------------------------

/// `NK2701`: a turn of the loop reads, a read can fail, and the function does
/// not say so.
///
/// The rule is Part I 6.4's, stated generally: an implicit call that can fail
/// fails the enclosing function, and the compiler makes you declare it.
#[test]
fn a_loop_that_can_fail_in_a_function_that_does_not_say_so_is_reported() {
    let (code, message) = one("use std::io\n\nfn count() -> i64 {\n\
         \x20   let mut n = 0\n\
         \x20   for line in io::lines() { n += 1 }\n\
         \x20   return n\n\
         }");
    assert_eq!(code, "NK2701");
    assert_eq!(
        message,
        "this function can fail because a turn of this loop can fail"
    );
}

/// …and nothing at all once it does.
#[test]
fn a_loop_that_can_fail_is_fine_where_the_failure_may_leave() {
    assert!(
        findings(
            "use std::io\n\nfn count() -> i64 throws {\n\
         \x20   let mut n = 0\n\
         \x20   for line in io::lines() { n += 1 }\n\
         \x20   return n\n\
         }"
        )
        .is_empty()
    );
}

/// The stream may be named first, and it is the same loop.
///
/// ADR-025 D7: this is why the emitter asks the type checker rather than
/// matching on the name `io::lines` - the name is not there to match.
#[test]
fn naming_the_stream_first_does_not_hide_it() {
    let (code, _) = one("use std::io\n\nfn count() -> i64 {\n\
         \x20   let stream = io::lines()\n\
         \x20   let mut n = 0\n\
         \x20   for line in stream { n += 1 }\n\
         \x20   return n\n\
         }");
    assert_eq!(code, "NK2701");
}

/// A stream of pairs does not exist in `std`, and unwrapping a failure while
/// also taking a pair apart is a shape to design rather than to guess at.
#[test]
fn a_fallible_loop_binds_one_name() {
    let (code, message) = one("use std::io\n\nfn count() throws {\n\
         \x20   for (a, b) in io::lines() { }\n\
         }");
    assert_eq!(code, "NK2701");
    // **The type is described rather than named**, because
    // [ADR-105](../../../docs/specification/adr/adr-105.md) D4 says a program
    // cannot write `Seq[String]`: a message that names a spelling its reader has
    // no way to type is the shape Part III C.1 is about. It said `Lines` while
    // `io::lines()` handed back a named type.
    assert_eq!(
        message,
        "a `for` over a sequence of `String` binds one name, and this binds 2"
    );
}

/// An ordinary loop is not touched by any of this.
#[test]
fn a_loop_over_something_that_cannot_fail_says_nothing() {
    assert!(
        findings(
            "fn count(xs: Vec[i32]) -> i64 {\n\
         \x20   let mut n = 0\n\
         \x20   for x in xs { n += 1 }\n\
         \x20   for i in 0..<10 { n += 1 }\n\
         \x20   return n\n\
         }"
        )
        .is_empty()
    );
}

// --- what it deliberately does not catch -------------------------------------

/// A method `std` does not write down is not an error.
///
/// This is `Unknown` doing its job: `insert_str` is Rust's and no ledger names
/// it, and a checker that had an opinion about it would be guessing.
///
/// `push_str` used to stand here and no longer can, because the ledger now says
/// what `String()` hands back (ADR-033 needed its touch set, and a
/// signature came with it) - so the receiver has a type, the entry for
/// `String::push_str` resolves, and three arguments to a method that takes one
/// is caught. That is the ledger growing and the checker getting sharper
/// together, which is what ADR-028 predicted it would look like.
#[test]
fn a_method_nobody_wrote_down_says_nothing() {
    // **The name is `common::UNDESCRIBED_METHOD`'s**, which is where this
    // repository keeps *"no ledger describes this"* — see the reason there. The
    // three arguments are deliberate: an entry would catch the arity, and the
    // point is that without one nothing is claimed at all.
    assert!(
        findings(&format!(
            "fn main() {{\n\
         \x20   let mut out = String()\n\
         \x20   out.{}(0, \"a\", \"b\")\n\
         }}",
            common::UNDESCRIBED_METHOD
        ))
        .is_empty()
    );
}

/// A bare number fits every numeric type, as it does in the language below.
/// Committing a literal to one would make `add(3)` wrong wherever the parameter
/// is not that one.
#[test]
fn an_integer_literal_fits_any_numeric_parameter() {
    assert!(
        findings(
            "fn small(a: i32) { }\n\
         fn big(a: u64) { }\n\
         fn main() { small(3) big(3) }"
        )
        .is_empty()
    );
}

/// A generic parameter is a name that stands for a type rather than being one,
/// so nothing is claimed about it.
#[test]
fn a_generic_parameter_is_not_a_type() {
    assert!(
        findings(
            "fn first[T](xs: T) -> T { return xs }\n\
         fn main() { let n: i32 = first(1) }"
        )
        .is_empty()
    );
}

/// A type whose **fields** are not written down anywhere is not checked, so a
/// field nobody can look up is never an error.
///
/// **The fixture had to move**, and how it moved is the point. It used to write
/// `&Row` — a type nothing declares — which stopped being an absence the day
/// `NK1135` started asking what declares a type
/// ([ADR-096](../../../docs/specification/adr/adr-096.md)). What this test is
/// actually about is the *fields*, so the receiver is now a type `std`
/// publishes and whose fields its ledger does not record: the type resolves,
/// the field cannot be looked up, and nothing is claimed about it.
#[test]
fn a_field_of_an_unknown_type_says_nothing() {
    assert!(
        findings("use std::fs\n\nfn label(r: ref fs::Mapped) -> ref String { return r.nmae }")
            .is_empty()
    );
}

/// `for (k, v) in map` takes apart a pair whose shape Stage 0 has no signature
/// for, so it binds nothing rather than guessing.
#[test]
fn a_loop_over_pairs_binds_nothing() {
    assert!(
        findings(
            "struct Row { id: i32 }\n\
         fn count(rows: Vec[Row]) {\n\
         \x20   for (a, b) in rows { let n: i32 = a.idd }\n\
         }"
        )
        .is_empty()
    );
}

/// A parameter whose Rust type is a *bound* rather than a type is `?` in the
/// ledger, and this is why: `fs::write` accepts a `String`, a `&str` and a
/// buffer, so a claim of `&str` there would refuse a correct program.
#[test]
fn a_parameter_that_accepts_several_types_claims_none() {
    assert!(
        findings(
            "use std::fs\nuse std::io\n\nfn main() throws {\n\
         \x20   let text = io::read_to_string()\n\
         \x20   let path = \"out.txt\"\n\
         \x20   fs::write(path, fs::Root::Anywhere, text)\n\
         }"
        )
        .is_empty()
    );

    // …and the arity is still checked, which is the half that survives.
    let (code, _) =
        one("use std::fs\n\nfn main() throws { fs::write(\"out.txt\", fs::Root::Anywhere) }");
    assert_eq!(code, "NK1101");
}

/// Both directions of the rule at once: an unknown on either side fits.
///
/// **It is held still by an absence the language decides**, and it took two
/// goes to get there. The example was `cli::args().nth(1)`, which stopped being
/// unwritten the day that entry was written; the replacement was
/// `String::to_uppercase`, which lasted until somebody noticed it obviously has
/// a type and wrote *that* down. Every real `std` name is a candidate for being
/// written, so a test resting on one being absent is a test that breaks when the
/// ledger does its job.
///
/// **The source is [ADR-024](../../../docs/specification/adr/adr-024.md) D4's
/// erased generic**, which this comment already called the better one: an absence
/// the **language** decides rather than one the ledger happens to be silent
/// about. It became usable when a generic function started lowering with its
/// `<T>` ([ADR-074](../../../docs/specification/adr/adr-074.md)); before that it
/// did not compile at all.
///
/// It used to be `HashMap::keys`, whose signature said `(&HashMap[?, ?]) -> ?` —
/// and the question this comment pointed at *was* answered:
/// [ADR-105](../../../docs/specification/adr/adr-105.md) gave the ledger's type
/// language a word for a produced sequence, so `keys()` is a `Seq[$K] sync` and
/// handing one to an `i32` is the mismatch it always was. The fixture was meant
/// to be revisited with that answer, and this is that.
#[test]
fn a_value_from_a_signature_that_claims_nothing_fits_anywhere() {
    assert!(
        findings(
            "fn takes(a: i32) { }\n\
         fn hand[T](x: T) -> T { return x }\n\
         fn main() {\n\
         \x20   takes(hand(1))\n\
         }"
        )
        .is_empty()
    );
}

/// **And a signature that *is* written is checked**, which is the other half of
/// the same rule and what the example above stopped being able to show.
#[test]
fn a_value_from_a_written_signature_is_measured_against_the_parameter() {
    let (code, _) = one("use std::cli\n\nfn takes(a: i32) { }\n\
         fn main() { takes(cli::args().nth(1)) }");
    assert_eq!(code, "NK1102", "a `String?` is not an `i32`");
}

// --- the migration warning (ADR-035 D5) --------------------------------------

/// A string written before the `f` existed looks exactly like one that meant
/// its braces, so the change of meaning cannot be silent - Part III C.1 calls a
/// silent one a bug in this compiler.
#[test]
fn a_string_that_used_to_interpolate_is_warned_about() {
    let found = findings("fn main() { let name = \"welt\" println(\"hallo {name}\") }");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1111");
    assert_eq!(found[0].severity, Severity::Warning);
    // Paste-ready, as Part III C.2 requires of every diagnostic.
    assert_eq!(
        found[0].help.as_deref(),
        Some("write `f\"hallo {name}\"` if the value was meant to appear (Part I, 2.5)")
    );
}

/// **And it never fires on a program that is right.** This is the whole reason
/// it is a warning and not an error: a stylesheet, a regular expression and a
/// JSON document all hold braces, and `margin` is nobody's variable.
#[test]
fn braces_that_name_nothing_are_just_braces() {
    assert!(findings("fn main() { println(\"{ margin: 0 }\") }").is_empty());
    assert!(findings("fn main() { println(\"\\\\d{3}\") }").is_empty());
    assert!(findings("fn main() { println(\"{}\") }").is_empty());
    assert!(findings("fn main() { println(\"\\u{0041}\") }").is_empty());
}

/// The other half of the change of meaning: `"{{}}"` printed `{}` and now
/// prints itself.
#[test]
fn a_doubled_brace_in_a_plain_string_is_warned_about_too() {
    let found = findings("fn main() { print(\"{{}}\") }");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, "NK1111");
    assert_eq!(found[0].severity, Severity::Warning);
    assert!(
        found[0].help.as_deref().unwrap().contains("write `\"{}\"`"),
        "{:?}",
        found[0].help
    );
}

/// An `f"…"` is held to nothing by this - it says what it is.
#[test]
fn an_f_string_is_never_warned_about() {
    assert!(findings("fn main() { let n = 1 println(f\"{n}\") }").is_empty());
    assert!(findings("fn main() { let n = 1 println(f\"{{{n}}}\") }").is_empty());
}

/// **The type comes from the syntax** (ADR-035 D3), so a `&str` return that
/// hands back an `f"…"` is the mistake it was before - and a plain string with
/// a brace in it is a `&str` rather than becoming a `String` by accident.
#[test]
fn the_type_of_a_literal_is_read_off_its_first_character() {
    let (code, _) = one("fn label(n: i32) -> ref String { return f\"{n} rows\" }");
    assert_eq!(code, "NK1104");
    assert!(findings("fn label() -> ref String { return \"{ a brace }\" }").is_empty());
}

// --- a written call that can fail (ADR-023 D8, ADR-025 D1) -------------------

/// `NK2605`: the function calls something that can fail and declares nothing.
///
/// The same rule as `NK2701` one line earlier in the block, and the case the
/// rule was generalised *from*: ADR-025 D1 says an implicit fallible call
/// "fails the enclosing function, exactly as a written call would" - and the
/// written call had no code. So a caller lowered in silence, and the ledger
/// then published `signature = "() -> String"` with no `throws` at all - a
/// committed file that other programs read (ADR-020) saying a function cannot
/// fail when its body provably can.
#[test]
fn a_written_call_that_can_fail_in_a_function_that_does_not_say_so_is_reported() {
    let (code, message) = one(
        "use std::fs\n\nfn liest() -> String throws { return fs::read_to_string(\"x.txt\", fs::Root::Anywhere) }\n\
         fn ruft() -> String { return liest() }",
    );
    assert_eq!(code, "NK2605");
    assert_eq!(message, "this function can fail because `liest` can fail");
}

/// The note is the contract, quoted from the ledger (Part III, C.4), and the
/// help names both of the language's ways out.
#[test]
fn the_note_quotes_the_callees_contract_and_the_help_is_a_way_out() {
    let found = findings(
        "enum ConfigError { NotFound }\n\
         fn liest() -> String throws { throw ConfigError::NotFound }\n\
         fn ruft() -> String { return liest() }",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0]
            .notes
            .iter()
            .any(|n| n.contains(r#"`liest` carries `throws = ["ConfigError"]`"#)),
        "{:#?}",
        found[0].notes
    );
    let help = found[0].help.as_deref().unwrap_or_default();
    assert!(help.contains("add `throws` to `ruft`"), "{help}");
    assert!(help.contains("catch"), "{help}");
}

/// …and nothing at all once the function says so, or catches it.
///
/// Both of these are correct programs, and the one thing this checker may
/// never do is refuse one.
#[test]
fn a_declared_throws_or_a_catch_is_the_end_of_it() {
    let declared = "use std::fs\n\nfn liest() -> String throws { return fs::read_to_string(\"x.txt\", fs::Root::Anywhere) }\n\
                    fn ruft() -> String throws { return liest() }";
    assert!(findings(declared).is_empty(), "{:#?}", findings(declared));

    let caught = "use std::fs\n\nfn liest() -> String throws { return fs::read_to_string(\"x.txt\", fs::Root::Anywhere) }\n\
                  fn ruft() -> String { return liest() catch { return \"\" } }";
    assert!(findings(caught).is_empty(), "{:#?}", findings(caught));
}

/// A `catch` covers what it guards and not what its **handler** does.
///
/// `contracts::order` reached the same conclusion about the same shape: the
/// handler is ordinary code, and a failure raised inside one leaves the
/// function like any other. Reading the handler as covered would be the
/// fail-open direction ADR-010 D1 forbids.
#[test]
fn a_catch_handler_is_not_itself_caught() {
    let (code, _) = one(
        "use std::fs\n\nfn liest() -> String throws { return fs::read_to_string(\"x.txt\", fs::Root::Anywhere) }\n\
         fn ruft() -> String { return liest() catch { return liest() } }",
    );
    assert_eq!(code, "NK2605");
}

/// A method call is a written call too, and the receiver's type is what makes
/// it answerable (ADR-028).
#[test]
fn a_method_that_can_fail_is_the_same_rule() {
    let (code, message) = one("enum ZuVoll { Voll }\n\
         struct Stats { n: i64 }\n\
         impl Stats {\n\
             fn add(ref self, v: i64) throws { if self.n > 100 { throw ZuVoll::Voll } }\n\
         }\n\
         fn record(s: Stats) { s.add(1) }");
    assert_eq!(code, "NK2605");
    assert_eq!(
        message,
        "this function can fail because `Stats::add` can fail"
    );
}

/// ...and it still refuses now that the `?` is emitted for it.
///
/// The two halves have to stay the two halves: the emitter writes the `?` only
/// where the function declares `throws`, and this is what guarantees the `?`
/// always has somewhere to go. An inference that gave `record` a `throws` it
/// never wrote would have made the lowering work and the ledger lie (ADR-023
/// D1, ADR-025 D1), so the refusal is load-bearing rather than a leftover.
#[test]
fn the_refusal_does_not_weaken_when_the_method_call_propagates() {
    let body = "enum ZuVoll { Voll }\n\
                struct Stats { n: i64 }\n\
                impl Stats {\n\
                    fn add(ref self, v: i64) -> i64 throws {\n\
                        if self.n + v > 100 { throw ZuVoll::Voll }\n\
                        return self.n + v\n\
                    }\n\
                }\n";

    // Undeclared: refused, and the set the emitter reads is not what decides it.
    let (code, _) = one(&format!(
        "{body}fn record(s: Stats) -> i64 {{ return s.add(1) }}"
    ));
    assert_eq!(code, "NK2605");

    // Declared: nothing to say, and this is the program that lowers with a `?`.
    let declared = format!("{body}fn record(s: Stats) -> i64 throws {{ return s.add(1) }}");
    assert!(findings(&declared).is_empty(), "{:#?}", findings(&declared));

    // Caught at the call: nothing to say either, and no `?` is emitted there.
    let caught =
        format!("{body}fn record(s: Stats) -> i64 {{ return s.add(1) catch {{ return 0 }} }}");
    assert!(findings(&caught).is_empty(), "{:#?}", findings(&caught));
}

/// What the emitter is handed, said of the checker's own output.
///
/// ADR-028: there is one type checker, and the emitter is not a second one. So
/// the answer about which method calls can fail is computed here, keyed by the
/// statement it is in and the method's name, and the emitter looks it up. Two
/// calls of the same name in one statement where only one can fail produce no
/// entry at all - the set never claims a call fails that does not.
#[test]
fn the_checker_says_which_method_calls_can_fail() {
    let source = "enum ZuVoll { Voll }\n\
                  struct A { n: i64 }\n\
                  impl A {\n\
                      fn add(ref self, v: i64) -> i64 throws {\n\
                          if self.n > 100 { throw ZuVoll::Voll }\n\
                          return self.n + v\n\
                      }\n\
                  }\n\
                  struct B { n: i64 }\n\
                  impl B {\n\
                      fn add(ref self, v: i64) -> i64 { return self.n + v }\n\
                      fn plain(ref self) -> i64 { return self.n }\n\
                  }\n\
                  fn one_of_them(a: A) -> i64 throws { return a.add(1) }\n\
                  fn neither(b: B) -> i64 { return b.plain() }\n\
                  fn both_names(a: A, b: B) -> i64 throws { return a.add(1) + b.add(2) }";
    let parsed = parse_to_ast(source).expect("the source parses");
    let fallible = nikaia::check::propagation_against(
        &parsed,
        &[],
        &Ledger::infer(&parsed),
        // No described crates.
        &Ledger::empty(),
        // No allowlist: a build given none reads nothing (ADR-072 D1).
        &nikaia::assets::Reads::none(),
    )
    .methods;

    let names: Vec<&str> = fallible.iter().map(|(_, name)| name.as_str()).collect();
    assert_eq!(
        names,
        vec!["add"],
        "only the resolved fallible call belongs in the set: {fallible:?}"
    );

    // And it is the one in `one_of_them`, not the pair in `both_names`: the
    // statement there writes `add` twice and one of the two cannot fail, so the
    // pair is removed rather than guessed at.
    let at = fallible.iter().next().expect("one entry").0;
    let line = source[..at].lines().count();
    assert_eq!(
        source.lines().nth(line - 1).unwrap_or_default().trim(),
        "fn one_of_them(a: A) -> i64 throws { return a.add(1) }",
        "the entry should be the statement in `one_of_them`"
    );
}

/// A call nothing describes says nothing here.
///
/// The checker answers only where the fact is written down (C.4). A foreign
/// function has no contract by definition, so this is silence and not
/// approval - `rustc` still type-checks the emitted crate, and ADR-005 D7's
/// translation reports its refusal against this same `.nika` line.
#[test]
fn a_callee_no_ledger_describes_is_not_guessed_at() {
    assert!(findings("fn ruft() -> i64 { return fremd::macht_irgendwas(1) }").is_empty());
}

// --- the withdrawn automatic argument names (Part I 5.3) ---------------------

/// **A lambda that reaches for `a` is refused, and the message says what
/// happened to the form** ([ADR-049](../../../docs/specification/adr/adr-049.md)
/// D1).
///
/// These tests used to hold the opposite: `NK1114`, a warning, because named
/// arguments were the normal form and the automatic naming was carried as
/// experimental. It is withdrawn now - and the reason the refusal has to be
/// *this* compiler's is Part III C.1: `xs.map fn { a + 1 }` lowers to
/// `|| { a + 1 }`, and `rustc` would say *cannot find value `a`* about a file
/// nobody wrote.
#[test]
fn a_lambda_that_reaches_for_a_withdrawn_name_is_refused() {
    for source in [
        "fn ids(xs: Vec[i64]) -> Vec[i64] { return xs.map fn { a + 1 } }",
        "fn total(xs: Vec[i64]) -> i64 { return xs.reduce(0) fn { a + b } }",
    ] {
        let found = findings(source);
        assert!(!found.is_empty(), "{source}");
        assert!(
            found.iter().all(|f| f.code == "NK1117"),
            "{source}: {found:#?}"
        );
        assert!(
            found.iter().all(|f| f.severity == Severity::Error),
            "{source}: refused, not warned about"
        );
        let notes = found[0].notes.join(" ");
        assert!(notes.contains("withdrawn"), "{notes}");
        let help = found[0].help.as_deref().unwrap_or_default();
        assert!(help.contains("fn ("), "{help}");
    }
}

/// **A lambda that names its arguments is fine**, and so is one that names none
/// and reaches for none. The first is the form; the second is the zero-argument
/// lambda `.or_insert_with fn { Stats(0) }` has always wanted.
#[test]
fn a_lambda_that_declares_what_it_uses_is_not_refused() {
    for source in [
        "fn ids(xs: Vec[i64]) -> Vec[i64] { return xs.map fn(x) { x + 1 } }",
        // The three letters are ordinary names once they are declared.
        "fn ids(xs: Vec[i64]) -> Vec[i64] { return xs.map fn(a) { a + 1 } }",
        "fn sevens(xs: Vec[i64]) -> Vec[i64] { return xs.map fn { 7 } }",
        "fn shout(xs: Vec[i64]) -> Vec[i64] { return xs.map fn { let n = 1\n n } }",
        // A local called `a` is a local now, which is the whole of what the
        // withdrawal bought.
        "fn ids(xs: Vec[i64]) -> Vec[i64] { return xs.map fn { let a = 1\n a } }",
    ] {
        assert!(
            findings(source).is_empty(),
            "{source}: {:#?}",
            findings(source)
        );
    }
}

// --- a view of a generic type, and of a transparent one (Part I 6.5) ---------

/// **A view of a generic type keeps its type.** It used to answer nothing, which
/// left every `&Vec[…]`, `&HashMap[…]` and `&Shared[…]` unchecked - so a
/// mismatch was reported by `rustc`, about the generated file, which is the one
/// thing Part III C.1 forbids.
#[test]
fn a_view_of_a_generic_type_is_checked_by_this_compiler() {
    let (code, message) = one("fn count(text: ref String) -> i64 { return 1 }\n\
         fn probe(xs: Vec[i64]) -> i64 { return count(ref xs) }");
    assert_eq!(code, "NK1102");
    assert!(message.contains("ref Vec[i64]"), "{message}");
}

/// And the matching case is accepted, so the rule above is a rule and not a
/// blanket refusal.
#[test]
fn a_view_of_a_generic_type_fits_the_same_view() {
    assert!(
        findings(
            "fn total(xs: ref Vec[i64]) -> i64 { return 1 }\n\
             fn probe(xs: Vec[i64]) -> i64 { return total(xs) }"
        )
        .is_empty(),
        "a view of the declared type must fit"
    );
}

/// **A transparent container is seen through.** Part III says of `fs::Mapped`
/// that "a parser cannot tell the difference", and `std`'s ledger spells it:
/// `fs::Mapped::deref` is `(&Mapped) -> &str`. Nothing consulted that entry, so
/// a function taking a text view refused a mapped file.
#[test]
fn a_view_of_a_transparent_container_fits_what_it_derefs_to() {
    assert!(
        findings(
            "use std::fs\n\nfn count(text: ref String) -> i64 { return 1 }\n\
             fn probe() -> i64 throws { let m = fs::map(\"x\", fs::Root::Anywhere)\n return count(m) }"
        )
        .is_empty(),
        "a mapped file must fit a text view"
    );
}

/// Seeing through one is a **rescue** and never a rule of its own: a container
/// whose `deref` gives something else is still refused, with this compiler's own
/// words.
#[test]
fn a_transparent_container_does_not_fit_just_anything() {
    let (code, message) = one("use std::fs\n\nfn count(n: ref i64) -> i64 { return 1 }\n\
         fn probe() -> i64 throws { let m = fs::map(\"x\", fs::Root::Anywhere)\n return count(ref m) }");
    assert_eq!(code, "NK1102");
    assert!(message.contains("ref Mapped"), "{message}");
}

/// A view of a view is the view. `&&str` is not a type this language has.
#[test]
fn a_view_of_a_view_is_the_view() {
    assert!(
        findings(
            "fn count(text: ref String) -> i64 { return 1 }\n\
             fn probe(text: ref String) -> i64 { return count(text) }"
        )
        .is_empty(),
        "a view of a view must still fit"
    );
}

// --- a shared value: where one is made, and where it is refused (Part I 6.2) --

/// **A call on the type makes the first handle**, and it is the only thing that
/// does ([ADR-064](../../../docs/specification/adr/adr-064.md) D2).
///
/// The annotation used to be the constructor
/// ([ADR-040](../../../docs/specification/adr/adr-040.md) §3), which meant the
/// places a hull could be made were a **list** - and a list somebody keeps
/// complete is a list with holes in it.
#[test]
fn a_call_on_the_type_makes_a_plain_value_shared() {
    assert!(
        findings(
            "struct Conn { host: String }\n\
             fn connect() -> Conn { return Conn { host: \"h\" } }\n\
             fn main() { let db = Shared(connect()) }"
        )
        .is_empty(),
        "the call is where the sharing starts"
    );
}

/// …and it stands wherever an expression may, a struct literal's field included.
///
/// **That is the whole of what changed**
/// ([ADR-064](../../../docs/specification/adr/adr-064.md) D2). There used to be
/// two positions where a plain value could become a shared one, and they were a
/// list somebody had to keep complete. A call needs no list.
#[test]
fn the_constructor_stands_where_any_expression_may() {
    assert!(
        findings(
            "struct Conn { host: String }\n\
             struct Pool { db: Shared[Conn] }\n\
             fn connect() -> Conn { return Conn { host: \"h\" } }\n\
             fn main() { let p = Pool { db: Shared(connect()) } }"
        )
        .is_empty(),
        "a field takes one like anywhere else"
    );
}

/// **A call site is not such a place** (`NK1115`).
///
/// A call in which the word does not appear would move the cleanup point
/// silently, and at such a place there would be no saying whether the value was
/// handed on or duplicated. So it is refused, and the message points at the line
/// where the sharing belongs.
#[test]
fn a_call_that_wants_a_shared_value_and_is_given_a_plain_one_is_refused() {
    let (code, message) = one("struct Conn { host: String }\n\
         fn keep(db: Shared[Conn]) { }\n\
         fn connect() -> Conn { return Conn { host: \"h\" } }\n\
         fn main() { let db = connect()\n keep(db) }");
    assert_eq!(code, "NK1115");
    assert_eq!(message, "`keep` takes a shared value, and `db` is not one");
    let help = findings(
        "struct Conn { host: String }\n\
         fn keep(db: Shared[Conn]) { }\n\
         fn connect() -> Conn { return Conn { host: \"h\" } }\n\
         fn main() { let db = connect()\n keep(db) }",
    )[0]
    .help
    .clone()
    .expect("Part III C.2: every diagnostic names a way out");
    assert!(
        help.contains("Shared(db)"),
        "the way out is the constructor, at the call (ADR-064 D2): {help}"
    );
}

/// A shared value handed where a shared value is wanted is fine, and nothing
/// about the rule above reaches it.
#[test]
fn a_shared_value_fits_a_shared_parameter() {
    assert!(
        findings(
            "struct Conn { host: String }\n\
             fn keep(db: Shared[Conn]) { }\n\
             fn connect() -> Conn { return Conn { host: \"h\" } }\n\
             fn main() { let db = Shared(connect())\n keep(db) }"
        )
        .is_empty(),
        "a handle fits a parameter that takes one"
    );
}

/// **Lending the inner value out duplicates nothing, and the checker is what
/// makes it writable.** `serve(&db)` takes a view of the value inside, through
/// `Shared::deref` and [ADR-042](../../../docs/specification/adr/adr-042.md) D2 -
/// a generic container seen through with what *this* one holds (ADR-031).
#[test]
fn a_view_of_a_shared_value_is_a_view_of_what_it_holds() {
    assert!(
        findings(
            "struct Conn { host: String }\n\
             fn serve(db: ref Conn) { }\n\
             fn connect() -> Conn { return Conn { host: \"h\" } }\n\
             fn main() { let db = Shared(connect())\n serve(db) }"
        )
        .is_empty(),
        "a function that only uses the value takes an ordinary view"
    );
}

/// Seeing a `Shared` through is still a **rescue**: one that holds something
/// else is refused, with this compiler's own words rather than rustc's.
#[test]
fn a_shared_value_does_not_fit_a_view_of_just_anything() {
    let (code, message) = one("struct Conn { host: String }\n\
         fn count(n: ref i64) { }\n\
         fn connect() -> Conn { return Conn { host: \"h\" } }\n\
         fn main() { let db = Shared(connect())\n count(ref db) }");
    assert_eq!(code, "NK1102");
    assert!(message.contains("ref Shared[Conn]"), "{message}");
}

// --- a literal that does not fit its type (Part I 2.2) -----------------------

/// `NK1116` at each of the three places a type stands beside a literal.
///
/// ADR-043 D5. This was already refused where it was written - but by `rustc`,
/// in Rust's words, down to the lint name `overflowing_literals` and the advice
/// to use a `u32`, about a file nobody wrote. So the point is not to prevent an
/// abort, which never happened: it is to take the message back (Part III, C.1).
#[test]
fn a_literal_too_large_for_its_type_is_refused_by_this_compiler() {
    for source in [
        // An annotated `let`.
        "fn main() { let x: i32 = 3000000000 }",
        // A `return` against a declared result.
        "fn gives() -> i32 { return 5000000000 }",
        // An argument whose parameter says what it takes. Asked after the callee
        // is resolved, because a free call walks its arguments before it knows
        // what they are measured against.
        "fn takes(n: i32) -> i32 { return n }\nfn main() { takes(4000000000) }",
    ] {
        let found = findings(source);
        let it = found
            .iter()
            .find(|f| f.code == "NK1116")
            .unwrap_or_else(|| panic!("no NK1116 for {source}: {found:#?}"));
        assert!(it.message.contains("does not fit in an `i32`"), "{it:#?}");
        assert!(
            it.notes.iter().any(|n| n.contains("-2147483648")),
            "the range belongs in the note: {it:#?}"
        );
        assert!(
            it.help.as_deref().is_some_and(|h| h.contains("i64")),
            "every error names a way out (Part III C.2): {it:#?}"
        );
    }
}

/// And a literal that fits is not mentioned, including at the edges.
#[test]
fn a_literal_that_fits_is_not_mentioned() {
    for source in [
        "fn main() { let x: i32 = 2147483647 }",
        "fn main() { let x: i64 = 5000000000 }",
        // No type beside it: a bare literal fits every numeric type, which is
        // what makes `add(3)` right wherever the parameter is numeric.
        "fn main() { let x = 3000000000 }",
    ] {
        assert!(
            !findings(source).iter().any(|f| f.code == "NK1116"),
            "{source} must not be refused"
        );
    }
}

/// **A name pins the type its own value took**
/// ([ADR-063](../../../docs/specification/adr/adr-063.md) D2), so a constant
/// reached through one is arithmetic in that type.
///
/// `let a = 2000000000` is an `i32` — the first type that holds it — so `a + a`
/// is an `i32` sum and does not fit. That is the same answer Kotlin and Rust
/// give, and the way out is one word: `let a: i64 = …`. What changes is who
/// says so: this was `rustc`'s *"this arithmetic operation will overflow"*
/// about the generated file (Part III, C.1).
///
/// **It is deliberately not widened.** Widening `a + a` would mean widening
/// `a`'s declaration, which is a walk back from a use to a binding — the one
/// analysis ADR-060 was cheap for not needing.
#[test]
fn a_constant_reached_through_a_name_is_arithmetic_in_that_name_s_type() {
    let found = findings("fn main() { let a = 2000000000\n let c = a + a }");
    let it = found
        .iter()
        .find(|f| f.code == "NK1116")
        .unwrap_or_else(|| panic!("no NK1116: {found:#?}"));
    assert!(it.message.contains("comes to 4000000000"), "{it:#?}");
    assert!(it.message.contains("does not fit in an `i32`"), "{it:#?}");

    // And the way out compiles, because the declaration pins the wider type.
    assert!(
        !findings("fn main() { let a: i64 = 2000000000\n let c = a + a }")
            .iter()
            .any(|f| f.code == "NK1116"),
        "the annotated form is a correct program"
    );
}

/// **A constant no type holds is refused here too**, which is what makes the
/// rule total: every constant integer expression gets an answer from this
/// compiler. It takes the first type that holds it, and where none does, the
/// message is ours and names the wider one it fell out of.
#[test]
fn a_constant_that_no_type_holds_is_refused_in_this_language_s_words() {
    let found = findings("fn main() { let huge = 9000000000000000000 + 9000000000000000000 }");
    let it = found
        .iter()
        .find(|f| f.code == "NK1116")
        .unwrap_or_else(|| panic!("no NK1116: {found:#?}"));
    assert!(it.message.contains("does not fit in an `i64`"), "{it:#?}");
    assert!(
        it.help.as_deref().is_some_and(|h| h.contains("widest")),
        "every error names a way out (Part III C.2): {it:#?}"
    );
}

/// **And it reaches a name inside an expression**, which is what
/// `docs/open-work.md` carried: `NK1117` used to fire only where a
/// statement *was* one name, so `let n = q + 1` was passed over in silence and
/// `rustc` refused the generated file about a name the user did write.
///
/// The list of what declares a name had to be complete first, and it was not.
/// Two sources were missing, and the corpus is what found them:
///
/// * a **template's `<for>`** — `<for r in :rows>{r.name}</for>` declares `r`
///   for the holes inside it (ADR-017), which `examples/escaping.nika` writes;
/// * a **config option** — `fn f(a: i64; separator: &str = " ")` (Part I 5.1)
///   was simply absent from the checker's scope frame, which nothing noticed
///   while a name in an expression was never asked about.
#[test]
fn an_undeclared_name_inside_an_expression_is_refused_too() {
    for (source, name) in [
        ("fn main() { let n = q + 1 }", "q"),
        // The misparse the statement rule was built for, one position over. It
        // used to be `1_000`, which was a number beside a name; since
        // [ADR-136](../../../docs/specification/adr/adr-136.md) that is the
        // number `1000`, and what is left of the reading is the leading
        // underscore, which is a name in every language that has both.
        ("fn main() { let n = _000 + 1 }", "_000"),
        // A name followed by a block, which used to be `quote { … }` here: that
        // parsed as `let q = quote` beside a block and lowered in silence, and
        // this is the row of `docs/spec-promises.md` it answered. `quote` is a
        // **reserved word** since
        // [ADR-088](../../../docs/specification/adr/adr-088.md) D7, so it no
        // longer reaches this check at all - the grammar refuses it first, which
        // is the earlier and better place. The shape is what mattered, so an
        // ordinary name stands here now.
        ("fn main() { let q = builder { 1 + 1 } }", "builder"),
        // Inside an interpolation, which is Nikaia source too (ADR-032 D3).
        (r#"fn main() { let s = f"{nope}" }"#, "nope"),
        // And as an argument.
        (
            "fn f(n: i64) -> i64 { return n }
fn main() { f(gone) }",
            "gone",
        ),
    ] {
        let found = findings(source);
        let it = found
            .iter()
            .find(|f| f.code == "NK1117")
            .unwrap_or_else(|| panic!("no NK1117 for {source}: {found:#?}"));
        assert!(
            it.message.contains(&format!("nothing declares `{name}`")),
            "{it:#?}"
        );
        assert_eq!(
            found.iter().filter(|f| f.code == "NK1117").count(),
            1,
            "one mistake, one finding: {found:#?}"
        );
    }
}

/// **And the two sources the corpus found are declarations like any other.**
#[test]
fn a_template_binding_and_a_config_option_declare_a_name() {
    for source in [
        // A template's `<for>`, which is `examples/escaping.nika`'s shape.
        r#"
struct Row { name: String }
fn table(rows: Vec[Row]) -> String {
    return dsl html {
        <for r in :rows><td>{r.name}</td></for>
    } eod
}
"#,
        // A config option, which is `examples/tally.nika`'s.
        r#"
fn summary(lines: i64; separator: ref String = " ") -> String {
    return f"{lines}{separator}"
}
"#,
    ] {
        let found = findings(source);
        assert!(
            !found.iter().any(|f| f.code == "NK1117"),
            "{source} declares every name it uses: {found:#?}"
        );
    }
}

// --- a sum of constants that cannot fit (ADR-043 §3) ------------------------

/// **The gap [ADR-043](../../../docs/specification/adr/adr-043.md) §3 named, closing.**
/// `let b = a + 1` where `a` is a constant `i32` at its ceiling was refused by
/// `rustc` with *"attempt to compute `i32::MAX + 1_i32`, which would overflow"* -
/// about the generated file, which is the Part III C.1 class.
///
/// What makes the bare `let` answerable is that an operand's **declaration**
/// pins the type: a literal alone still has none, which is the case below that
/// must stay accepted.
#[test]
fn a_sum_of_constants_that_cannot_fit_is_refused_here() {
    for (source, comes_to) in [
        // The record's own example: the type comes from `a`, not from the `let`.
        (
            "fn main() { let a: i32 = 2147483647\nlet b = a + 1 }",
            "2147483648",
        ),
        // A product, and through two bindings.
        (
            "fn main() { let a: i32 = 2000000000\nlet b = a\nlet c = b * 2 }",
            "4000000000",
        ),
        // Negation past the floor, which has no matching positive (ADR-043 D3).
        (
            "fn main() { let a: i32 = 2147483647\nlet b = -a - 2 }",
            "-2147483649",
        ),
        // An annotation rather than an operand pins it, and the sum is folded
        // all the same.
        ("fn main() { let x: i32 = 2147483000 + 1000 }", "2147484000"),
        // And an `i64` has a ceiling too: the fold is in an `i128` so that the
        // number it reports is the one the program wrote rather than a wrapped
        // one.
        (
            "fn main() { let a: i64 = 9223372036854775807\nlet b = a + 1 }",
            "9223372036854775808",
        ),
    ] {
        let found = findings(source);
        let it = found
            .iter()
            .find(|f| f.code == "NK1116")
            .unwrap_or_else(|| panic!("no NK1116 for {source}: {found:#?}"));
        assert!(
            it.message.contains(comes_to),
            "the message says what it comes to, because that is the part the \
             reader cannot see: {it:#?}"
        );
    }
}

/// **And the polarity holds** (Part III, C.4): everything this checker cannot
/// evaluate is accepted.
///
/// All but one of these `rustc` accepts as well, so this compiler and the one
/// below agree. **The exception is the unpinned literal**, and it is the
/// `open-work.md` out-of-range-literal case rather than a new one: `rustc` refuses
/// `3000000000 + 1` because it *has* inference and defaults the literal to an
/// `i32`, and this checker has none - so refusing here would also refuse the
/// program that passes the sum to an `i64`, which is the one thing it may never
/// do. The residue stays one entry rather than becoming two.
#[test]
fn a_sum_this_checker_cannot_evaluate_is_not_refused() {
    for source in [
        // The pinned type is wide enough.
        "fn main() { let a: i64 = 2147483647\nlet b = a + 1 }",
        // Nothing pins a type at all: `3000000000 + 1` is an `i64` wherever a
        // use asks for one (Part I 2.4) - the §1.5 residue.
        "fn main() { let b = 3000000000 + 1 }",
        // A `mut` local may have been given another value before the name is
        // read again, and this checker does not follow assignments.
        "fn main() { let mut c: i32 = 2147483647\nc = 1\nlet d = c + 1 }",
        // A cast is not folded, and this is the way a program says it meant the
        // wider type.
        "fn main() { let e: i32 = 2147483647\nlet g = e as i64 + 1 }",
        // A parameter is not a constant.
        "fn add(n: i32) -> i32 { return n + 1 }",
        // Neither is a `for` binding.
        "fn main() { for i in 0..3 { let h = i + 1 } }",
    ] {
        let found = findings(source);
        assert!(
            !found.iter().any(|f| f.code == "NK1116"),
            "{source} must not be refused: {found:#?}"
        );
    }
}

// --- a division by a constant zero (`NK1118`) -------------------------------

/// ADR-043 D5.5. The same mechanism found the same class one operator over:
/// `rustc`'s `unconditional_panic`, saying *"attempt to divide `1_i32` by
/// zero"* about the generated file.
#[test]
fn a_division_by_a_constant_zero_is_refused_here() {
    for (source, says) in [
        ("fn main() { let q = 10 / 0 }", "divides by zero"),
        (
            "fn main() { let r = 10 % 0 }",
            "takes the remainder by zero",
        ),
        // Through a binding, which is the shape a program actually reaches it by.
        (
            "fn main() { let n: i32 = 0\nlet q = 1 / n }",
            "divides by zero",
        ),
    ] {
        let found = findings(source);
        let it = found
            .iter()
            .find(|f| f.code == "NK1118")
            .unwrap_or_else(|| panic!("no NK1118 for {source}: {found:#?}"));
        assert!(it.message.contains(says), "{it:#?}");
    }
}

/// A divisor this checker cannot prove is zero says nothing: that division
/// aborts at run time, naming the Nikaia line (ADR-044).
#[test]
fn a_divisor_that_is_not_a_proven_zero_is_not_mentioned() {
    for source in [
        "fn main() { let n: i32 = 2\nlet q = 10 / n }",
        "fn half(n: i32) -> i32 { return n / 2 }",
        "fn by(n: i32, d: i32) -> i32 { return n / d }",
    ] {
        let found = findings(source);
        assert!(
            !found.iter().any(|f| f.code == "NK1118"),
            "{source} must not be refused: {found:#?}"
        );
    }
}

// --- a word this language does not know (`NK1117`) ---------------------------

/// **A statement that is one undeclared name is refused here**, not by `rustc`.
///
/// The grammar is scannerless, so a word this language has no rule for is read as
/// a name - and a name in statement position is a legal statement. Measured, each
/// of these lowered without a word and was then refused by `rustc` about a file
/// nobody wrote (Part III, C.1):
///
/// * `assert c` - a keyword this language does not have;
/// * `unsafe { … }` - the same, with a block after it;
/// * `let n = 1_000` - which *was* `1` beside the name `_000`, and is the
///   number `1000` since [ADR-136](../../../docs/specification/adr/adr-136.md).
///   It stands here as `_000` alone, which is the part of that reading the
///   language kept: a **leading** underscore is a name.
#[test]
fn a_word_this_language_does_not_know_is_refused() {
    // **`unsafe` used to be one of these and is a construct now**
    // ([ADR-124](../../../docs/specification/adr/adr-124.md) D1): the word
    // joined Part I 2.1's list the way that list says one does, with its rule
    // in the same change. It is here as the shape it left, because a word
    // moving *onto* the list is the direction that breaks programs and the one
    // worth a line.
    assert!(
        parse_to_ast("fn main() {\n    unsafe { println(\"x\") }\n}").is_ok(),
        "`unsafe` and a block is a construct now, not a name nothing declares"
    );
    let (code, message) = one("fn main() {\n    println(f\"{_000}\")\n}");
    assert_eq!(code, "NK1117");
    assert!(message.contains("_000"), "{message}");
    // **`assert` left this list with its construct**
    // ([ADR-245](../../../docs/specification/adr/adr-245.md) D2): it is a
    // function now, and the statement form Part III 14.1 used to write is the
    // name alone - which is told how the call is written.
    let (code, message) = one("fn main() {\n    let c = true\n    assert c\n}");
    assert_eq!(code, "NK1195");
    assert!(message.contains("assert(cond)"), "{message}");
}

/// And the names that **are** declared are not refused - the half that decides
/// whether the rule above is usable.
///
/// A block's value is the shape most at risk: `fn f() -> i64 { x }` is a
/// statement that is one name, and it is how a function hands something back.
#[test]
fn a_name_something_declares_is_not_refused() {
    for source in [
        // a local, as a block's value
        "fn f() -> i64 {\n    let x = 7\n    x\n}",
        // a parameter
        "fn f(n: i64) -> i64 {\n    n\n}",
        // a function of this program, named rather than called
        "fn helper() -> i64 {\n    return 1\n}\n\nfn f() {\n    helper\n}",
        // a struct declared here
        "struct Conn { id: i64 }\n\nfn f() {\n    Conn\n}",
        // `error`, which a `catch` block binds
        "use std::fs\n\nfn f(p: ref String) {\n    fs::read_to_string(p, fs::Root::Anywhere) catch { println(f\"{error}\") }\n}",
    ] {
        assert!(
            findings(source).is_empty(),
            "{source}\n{:#?}",
            findings(source)
        );
    }
}

/// **The three withdrawn names are ordinary names again once something declares
/// them** ([ADR-049](../../../docs/specification/adr/adr-049.md) D1).
///
/// This used to be about the opposite: a `fn { … }` declared no parameter list, so
/// its body walked with `a` in scope nowhere, and the automatic names had to be
/// bound from the same function the emitter wrote the parameter list from. Nothing
/// is read off a body now - so `fn { a }` is a lambda of no arguments reaching for
/// a name nothing declares, and `fn (a) { a }` is a lambda whose argument is
/// called `a`.
#[test]
fn a_declared_a_is_an_ordinary_name() {
    let found = findings("fn f(s: ref String) {\n    s.map(fn { a })\n}");
    assert!(
        found.iter().any(|f| f.code == "NK1117"),
        "nothing declares `a` here: {found:#?}"
    );

    assert!(
        findings("fn f(s: ref String) {\n    s.map(fn (a) { a })\n}").is_empty(),
        "and here it is the argument"
    );
}

// --- `self` as a declared name (`NK1119`) ------------------------------------

/// **`self` is the one reserved word the grammar has to accept as a name**, and
/// this is where declaring it is refused
/// ([ADR-051](../../../docs/specification/adr/adr-051.md)).
///
/// Every other reserved word is excluded from the grammar's `NAME` rule, so
/// `let fn = 3` does not parse. `self` cannot be: `self.min` refers to it, and
/// `NAME` is the rule both for declaring a name and for referring to one. The
/// case it takes back is the same C.1 one as the rest - `let self = 3` lowered
/// to `let self = 3;` and `rustc` refused the generated file with *"expected
/// identifier, found keyword `self`"*.
#[test]
fn declaring_a_name_called_self_is_refused() {
    for source in [
        "fn main() { let self = 3 }",
        "fn main() { for self in 0..3 { } }",
        "fn main() { let f = fn(self) { 1 } }",
        // A struct field, which took a span of its own on `FieldDef` to
        // reach: without one the nearest span this walk had was a
        // statement's, on a different line.
        "struct Bad { self: i64 }",
    ] {
        let found = findings(source);
        let it = found
            .iter()
            .find(|f| f.code == "NK1119")
            .unwrap_or_else(|| panic!("no NK1119 for {source}: {found:#?}"));
        assert!(it.message.contains("`self` is a reserved word"), "{it:#?}");
        assert!(
            it.help.is_some(),
            "every error names a way out (Part III C.2): {it:#?}"
        );
    }
}

/// **A parameter named `self` is refused by the grammar**, which is where it
/// turns out to belong: `fn f(self: i64)` never parsed at all, because the
/// receiver rule takes the word and the `: i64` has nowhere to go. It says so
/// in a sentence now rather than *"expected `)`; found `:`"*
/// ([ADR-051](../../../docs/specification/adr/adr-051.md) D4).
#[test]
fn a_parameter_called_self_is_refused_by_the_grammar() {
    let message = format!(
        "{:#}",
        parse_to_ast("fn f(self: i64) -> i64 { return 1 }").expect_err("refused")
    );
    assert!(message.contains("`self` is a reserved word"), "{message}");
    assert!(message.contains("`ref mut self`"), "{message}");
}

/// And *referring* to `self` is untouched, which is the half that had to keep
/// working: every method body in the repository does it.
#[test]
fn referring_to_self_is_not_refused() {
    let source = "\
struct Tally { n: i64 }
impl Tally {
    pub fn bump(ref mut self) { self.n += 1 }
}
";
    let found = findings(source);
    assert!(
        !found.iter().any(|f| f.code == "NK1119"),
        "`self.n` refers to the receiver: {found:#?}"
    );
}

// --- `as` names a type this language offers (Part I 2.2) ---------------------

/// `NK1122`, and the hole it closes was an **escape hatch nobody decided**
/// ([ADR-054](../../../docs/specification/adr/adr-054.md) D1).
///
/// The target of an `as` went to the language below unread, so a cast named any
/// Rust type at all and was emitted verbatim. Two things followed: a program
/// could hold a value of a type Part I 2.2 has no word for, and `-3 as usize`
/// became 18,446,744,073,709,551,613 — silently, in a language where a
/// conversion that does not fit aborts. The same conversion at an index has
/// reported as an access out of bounds since ADR-048 D1; written by hand it
/// reported nothing at all.
#[test]
fn a_cast_to_a_type_the_page_does_not_offer_is_refused() {
    for into in ["usize", "isize", "u32", "u64", "u128", "i8", "i16", "i128"] {
        let (code, message) = one(&format!(
            "fn main() {{ let n: i64 = 3\n let x = n as {into} }}"
        ));
        assert_eq!(code, "NK1122", "{message}");
        assert!(message.contains(into), "it names the type: {message}");
    }
}

/// And every type the page **does** name is accepted, so this is a rule and not
/// a blanket refusal.
///
/// `i32`, `i64`, `u8` and `f64` are the four a conversion narrows between and
/// were never in question; `bool`, `char`, `String` and `&str` are here because
/// the rule is "a type this language offers" and not "a number" — a narrower
/// list would refuse a program for a reason nothing on the page states.
#[test]
fn a_cast_to_a_type_the_page_offers_is_accepted() {
    for into in ["i32", "i64", "u8", "f64"] {
        let found = findings(&format!(
            "fn main() {{ let n: i64 = 3\n let x = n as {into} }}"
        ));
        assert!(
            found.iter().all(|f| f.code != "NK1122"),
            "`{into}` is on the page (Part I, 2.2): {:#?}",
            found.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
    }
}
