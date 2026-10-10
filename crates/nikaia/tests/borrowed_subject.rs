//! A field of a borrowed subject, handed out by value
//! ([ADR-083](../../../docs/specification/adr/adr-083.md)).
//!
//! **Part I 6.8 is what decides this**, and it decides it in the language's own
//! words rather than in a judgement of mine:
//!
//! > Ownership rules occasionally reject code […] every such error explains
//! > itself in plain language and tells you what to do next. You never need Rust
//! > knowledge to read a Nikaia error; **if a raw internal (Rust) error ever
//! > reaches you, that is a Nikaia bug.**
//!
//! `return self.username` out of a `ref self` method was exactly that: an
//! ownership rule rejecting code, with `E0507` reaching the user about a file
//! nobody wrote.
//!
//! **The polarity is what these tests are mostly about.** A refusal that fires
//! where it should not is worse than the leak it replaces —
//! [Part III C.4](../../../docs/specification/30-nikaia-tooling.md) says this
//! compiler never refuses a correct program — so every shape that worked before
//! is run, in `tests/language/src/borrowed_subject.nika`, beside every shape
//! that leaked here.
use nikaia::check::CodeOps;

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings
}

/// A struct with one field that moves and one that copies, for both directions.
fn around(body: &str, call: &str) -> String {
    format!(
        r#"
fn takes(s: String) -> i64 {{
    return 1
}}

struct Row {{
    name: String,
    count: i64,
}}

impl Row {{
{body}
}}

fn main() {{
    let r = Row {{ name: "a".to_string(), count: 1 }}
    println(f"{{{call}}}")
}}
"#
    )
}

/// **The three positions that leaked**, each refused by name.
///
/// Measured as a class before anything was built: a field read off a borrowed
/// subject leaks by being handed back, by being bound, and by being passed —
/// and nowhere else.
#[test]
fn every_by_value_position_is_refused() {
    let positions = [
        (
            "handed back",
            "    fn m(ref self) -> String {\n        return self.name\n    }",
        ),
        (
            "bound",
            "    fn m(ref self) -> i64 {\n        let x = self.name\n        return 1\n    }",
        ),
        (
            "passed",
            "    fn m(ref self) -> i64 {\n        return takes(self.name)\n    }",
        ),
    ];
    for (what, body) in positions {
        let found = findings(&around(body, "r.m()"));
        let refusal = found
            .iter()
            .find(|f| f.code == "NK1131")
            .unwrap_or_else(|| panic!("a field {what} by value is refused: {found:#?}"));
        assert!(
            // The message no longer spells out the position (`{what}`); every
            // position reads as giving the field away.
            refusal.message.contains("`self.name`") && refusal.message.contains("give away"),
            "it names the field and that it is being given away ({what}): {}",
            refusal.message
        );
        // **The two ways out this record names**
        // ([ADR-083](../../../docs/specification/adr/adr-083.md) D2), and a
        // third stood here that could not be taken — see
        // `the_help_names_only_what_can_be_taken` below.
        assert!(
            refusal
                .help
                .as_deref()
                .is_some_and(|h| h.contains("self.name.clone()")
                    && h.contains("take `self` instead of `ref self`")),
            "and names both ways out: {:?}",
            refusal.help
        );
    }
}

/// A **free function** is not a method, so nothing here is borrowed and nothing
/// is refused — including a local that happens to be called `self`… which it
/// cannot be (`NK1119`), so this is about the ordinary case.
#[test]
fn a_free_function_is_not_touched() {
    let found = findings(
        r#"
struct Row {
    name: String,
}

fn name_of(r: Row) -> String {
    return r.name
}

fn main() {
    let r = Row { name: "a".to_string() }
    println(name_of(r))
}
"#,
    );
    assert!(
        found.is_empty(),
        "nothing is borrowed in a free function: {found:#?}"
    );
}

/// **And the help names only what can be taken**
/// ([Part III C.2](../../../docs/specification/30-nikaia-tooling.md)'s rule).
///
/// This test used to be *the help names the field's own view type, not always
/// `&str`* — a claim about a way out that reads *declare the result `…` and write
/// `return &self.tags`*, and which no program could take: `&str` stopped being a
/// spelling at [ADR-184](../../../docs/specification/adr/adr-184.md) D4, and a
/// `&` a program writes is `NK1137`. A better spelling for a way out that does
/// not exist is not better.
///
/// **The half of it that was real is built**, and the help offers it: declaring
/// the result a view is enough, and the `&` is the compiler's. What stays
/// refused is the shape here — the result is declared `Vec[i64]`, so the value
/// is moved out of a loan whatever the help says.
#[test]
fn the_help_names_only_what_can_be_taken() {
    let found = findings(
        r#"
struct Row {
    tags: Vec[i64],
}

impl Row {
    fn t(ref self) -> Vec[i64] {
        return self.tags
    }
}

fn main() {
    println("x")
}
"#,
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1131")
        .expect("a Vec field is moved too");
    // **The `&` is never asked of the author**, which is the half of the old
    // help that could not be taken: a `&` a program writes is `NK1137` since
    // [ADR-094](../../../docs/specification/adr/adr-094.md) D1, so a help that
    // asked for one named a way out that does not exist ([Part III
    // C.2](../../../docs/specification/30-nikaia-tooling.md)). The other half —
    // *declare the result a view* — is real and is offered, in the field's own
    // type and never always `&str`.
    assert!(
        refusal
            .help
            .as_deref()
            .is_some_and(|h| h.contains("self.tags.clone()")
                && h.contains("take `self` instead of `ref self`")
                && h.contains("Declare the result `ref Vec[i64]`")
                && !h.contains("&self.tags")),
        "the ways out, and no `&` for the author to write: {:?}",
        refusal.help
    );
}

/// The reference lands in the lowering, and it lands **once**.
///
/// The written form and the unwritten one are one program, so they lower to one
/// file: `&self.name` either way, and never `&&self.name` — which is what a
/// second `&` on a value that already has one would be, reported about a file
/// nobody wrote ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
#[test]
fn the_reference_is_written_once_whichever_way_the_source_says_it() {
    let both = ["return self.name", "self.name"];
    for body in both {
        let source = format!(
            "struct Row {{ name: String }}\n\
             \n\
             impl Row {{\n\
             \x20   fn name(ref self) -> ref String {{\n\
             \x20       {body}\n\
             \x20   }}\n\
             }}\n\
             \n\
             fn main() {{ println(\"x\") }}\n"
        );
        assert!(
            findings(&source).is_empty(),
            "`{body}` is a correct program"
        );
        let rust = emit_program(
            &parse_to_ast(&source).expect("the source parses"),
            Build::default(),
        )
        .expect("the source lowers")
        .rust;
        assert!(rust.contains("&self.name"), "`{body}`:\n{rust}");
        assert!(!rust.contains("&&self.name"), "`{body}`:\n{rust}");
    }
}

/// **A view of the subject is a view of the subject, and the ledger says so.**
///
/// `returns = "borrows(self)"` is the column a caller reads to know the result
/// does not point into what it passed — which is what lets a wrapper hand a view
/// parameter to such a method without being told it escapes. The receiver had
/// been left out of it while nothing read the column across one.
#[test]
fn the_ledger_says_the_result_borrows_the_subject() {
    let parsed = parse_to_ast(
        "struct Row { name: String }\n\
         \n\
         impl Row {\n\
         \x20   pub fn name(ref self) -> ref String { return self.name }\n\
         \x20   pub fn owned(ref self) -> String { return self.name.clone() }\n\
         }\n",
    )
    .expect("the source parses");
    let own = Ledger::infer(&parsed);
    assert_eq!(
        own.functions
            .get("Row::name")
            .expect("the accessor is in the ledger")
            .borrows,
        vec!["self".to_string()],
    );
    // **And only where the result is a view.** A method that hands back an owned
    // value points into nothing, so there is no position to name.
    assert!(
        own.functions
            .get("Row::owned")
            .expect("the copy is in the ledger")
            .borrows
            .is_empty()
    );
}

/// **The way out is offered only where it can be taken.**
///
/// A field *handed back* may become a view, because the result is a place the
/// declaration can name. A field *bound* to a local or *passed* to a call has
/// nowhere declared to point, so the two ways out are the whole answer there —
/// [Part III C.2](../../../docs/specification/30-nikaia-tooling.md) again, from
/// the other side.
#[test]
fn the_third_way_out_is_named_only_where_the_result_is() {
    let positions = [
        (
            "    fn m(ref self) -> String {\n        return self.name\n    }",
            true,
        ),
        (
            "    fn m(ref self) -> i64 {\n        let x = self.name\n        return 1\n    }",
            false,
        ),
        (
            "    fn m(ref self) -> i64 {\n        return takes(self.name)\n    }",
            false,
        ),
    ];
    for (body, offered) in positions {
        let found = findings(&around(body, "r.m()"));
        let refusal = found
            .iter()
            .find(|f| f.code == "NK1131")
            .unwrap_or_else(|| panic!("a field by value is refused: {found:#?}"));
        let help = refusal.help.as_deref().expect("every refusal has one");
        assert_eq!(
            help.contains("Declare the result `ref String`"),
            offered,
            "{body}\n{help}"
        );
    }
}

/// **And a subject that is not borrowed has nothing to lend.**
///
/// `fn m(self) -> ref String` takes the subject by value, so it dies at the end
/// of the call and a view into it cannot leave. The refusal is `NK1104`'s — the
/// value is a `String` where a `ref String` was declared — and not a silent
/// reference the language below would then report about a file nobody wrote.
#[test]
fn a_subject_taken_by_value_lends_nothing() {
    let found = findings(
        r#"
struct Row { name: String }

impl Row {
    fn m(self) -> ref String {
        return self.name
    }
}

fn main() { println("x") }
"#,
    );
    assert!(
        found.iter().any(|f| f.code == "NK1104"),
        "a view out of a subject taken by value is refused: {found:#?}"
    );
}

/// **A `mut` parameter given away whole is the same shape**
/// (issue #161): it is passed as `ref mut`, so the function borrows
/// it as a method borrows `ref self`. `Box { items: out }` and `return out`
/// were `rustc`'s *mismatched types* about a file nobody wrote. The ways out
/// run in `tests/language/src/borrowed_subject.nika`.
#[test]
fn a_mut_parameter_given_away_whole_is_refused() {
    for body in [
        "    out.push(\"x\")\n    return Keep { items: out }",
        "    out.push(\"x\")\n    let taken = out\n    return Keep { items: taken }",
    ] {
        let source = format!(
            "struct Keep {{ items: Vec[String] }}\n\
             fn store(mut out: Vec[String]) -> Keep {{\n{body}\n}}\n\
             fn main() {{\n    let mut v: Vec[String] = []\n    let k = store(v)\n}}\n"
        );
        let found = findings(&source);
        let it = found
            .iter()
            .find(|f| f.code == "NK1131")
            .unwrap_or_else(|| panic!("no NK1131 for {body}: {found:#?}"));
        assert!(it.message.contains("`out`"), "{it:#?}");
        assert!(
            it.help
                .as_deref()
                .is_some_and(|h| h.contains("out.clone()")),
            "{it:#?}"
        );
    }
}

/// **The written `ref` is the compiler's line said twice** (#96, ADR-202 D2):
/// at a `return` of a place inside a borrowed subject or a lent parameter it is
/// `NK1137`. That the line without it lends - a parameter declared `ref Row`
/// as a `ref self` does - is run in `tests/language/src/borrowed_subject.nika`.
#[test]
fn a_written_ref_at_a_lending_return_is_refused() {
    for body in ["return ref self.name", "return ref row.name"] {
        let source = format!(
            "struct Row {{ name: String }}\n\
             \n\
             impl Row {{\n\
             \x20   fn name(ref self) -> ref String {{\n\
             \x20       {}\n\
             \x20   }}\n\
             }}\n\
             \n\
             fn of(row: ref Row) -> ref String {{\n\
             \x20   {}\n\
             }}\n\
             \n\
             fn main() {{ println(\"x\") }}\n",
            if body.contains("self") {
                body
            } else {
                "return self.name"
            },
            if body.contains("row") {
                body
            } else {
                "return row.name"
            },
        );
        let codes: Vec<&str> = findings(&source).iter().map(|f| f.code_str()).collect();
        assert_eq!(codes, vec!["NK1137"], "`{body}`");
    }
}
