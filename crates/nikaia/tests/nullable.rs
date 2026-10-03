//! Part I 2.3's nullable types, and Part I 3.5's `??` over one.
//!
//! **The section's own example did not parse.** A trailing `?` on a type was a
//! parse error and `null` was read as an ordinary name, so neither line of it
//! was accepted — which is what issue #263 carried. These are the
//! programs that page names, compiled rather than only compared as text:
//! whether the `Some(…)` lands in the right places is settled by the language
//! below, and reading the emitted string would only say that this compiler
//! agrees with itself.

mod common;

use nikaia::check;
use nikaia::contracts::ty::TyOps;
use nikaia::contracts::{Ledger, LedgerOps, STD, ty::Ty};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Lower, compile the result as a Rust library, and hand the emitted text back.
///
/// **The checker runs too**, because a build runs it: without that a program
/// this compiler would have refused reaches `rustc` and fails there, and the
/// test then reports the wrong thing. (Written after exactly that: a `?.` on a
/// plain value came back as *"`User` is not an iterator"*.)
fn compiled(purpose: &str, source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let findings = check::check(&parsed, &own, &library).findings;
    assert!(
        findings.is_empty(),
        "{purpose} is a correct program and the checker says otherwise: {findings:#?}"
    );

    let rust = lowered(source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("lowered.rs");
    std::fs::write(&file, &rust).expect("write the Rust");

    let out = common::compile(
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
        out.status.success(),
        "the lowering of {purpose} does not compile:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr),
    );

    let _ = std::fs::remove_dir_all(&dir);
    rust
}

/// Lower, compile the result as a **binary**, run it, and hand back what it
/// printed.
///
/// [`compiled`] settles whether the Rust is well typed, which is enough for a
/// wrap in the right place. Short-circuiting is not that kind of question: a
/// `?.` that reached through a `null` and a `?.` that did not both compile, and
/// the only thing that tells them apart is what the program prints.
fn ran(purpose: &str, source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let found = check::check(&parsed, &own, &library).findings;
    assert!(
        found.is_empty(),
        "{purpose} is a correct program and the checker says otherwise: {found:#?}"
    );

    let rust = lowered(source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering of {purpose} does not compile:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr),
    );
    let ran = std::process::Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run the program");
    assert!(
        ran.status.success(),
        "{purpose} failed: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let printed = String::from_utf8_lossy(&ran.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    printed
}

/// What the checker says about `source`.
fn findings(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check(&parsed, &own, &library).findings
}

/// **Part I 2.3's example, both lines.**
///
/// `&str?` is an `Option<&str>` and `null` is `None`. The `?` is peeled before
/// the rest of the type is rendered, which is what keeps the view a view: the
/// wrapper is applied to the type and not to its name.
#[test]
fn the_sections_own_example_lowers() {
    let rust = compiled(
        "nullable-example",
        "\
fn main() {
    let strictly_string: String = \"Hello\"
    let mut maybe_string: String? = null
    maybe_string = \"World\"
    let shown = maybe_string ?? \"nothing\"
    println(f\"{strictly_string} {shown}\")
}
",
    );
    assert!(
        rust.contains("let mut maybe_string: Option<String> = None;"),
        "{rust}"
    );
    // The second line is the one that needs the constructor written: a
    // `String` standing where a `String?` is wanted.
    assert!(
        rust.contains("maybe_string = Some(String::from(\"World\"));"),
        "{rust}"
    );
}

/// **A nullable result, and the `Some(…)` at a `return`.**
///
/// `return 42` against a declared `i64?` is the case a type alone cannot answer:
/// a number has no type of its own on purpose (Part I 2.4), so the checker reads
/// the *value* as well and a literal is never a `T?`.
#[test]
fn a_function_may_hand_back_a_nullable() {
    let rust = compiled(
        "nullable-return",
        "\
fn score(id: i64) -> i64? {
    if id > 0 {
        return 42
    }
    return null
}

fn main() {
    let hit = score(1) ?? 0
    let miss = score(0) ?? 0
    println(f\"{hit} {miss}\")
}
",
    );
    assert!(rust.contains("fn score(id: i64) -> Option<i64>"), "{rust}");
    assert!(rust.contains("return Some(42);"), "{rust}");
    // `null` needs no wrap, being a `T?` itself — and as the body's tail it
    // keeps no `return` either (Part I 3.1).
    assert!(rust.contains("None"), "{rust}");
    assert!(!rust.contains("Some(None)"), "{rust}");
}

/// **A nullable is not wrapped twice.** Handing a `T?` where a `T?` is wanted
/// needs nothing, and a value whose type this checker could not work out is
/// left alone rather than guessed at — wrapping one that is already an
/// `Option<T>` would make an `Option<Option<T>>`.
#[test]
fn a_value_that_is_already_nullable_is_not_wrapped() {
    let rust = compiled(
        "nullable-no-double-wrap",
        "\
fn score(id: i64) -> i64? {
    return null
}

fn main() {
    let mut a: i64? = null
    a = score(1)
    let shown = a ?? 0
    println(f\"{shown}\")
}
",
    );
    assert!(rust.contains("a = score(1);"), "{rust}");
    assert!(!rust.contains("Some(score(1))"), "{rust}");
}

/// **A `T?` round-trips through the ledger as `T?`**, and never as
/// `Option[T]` — which is a name no program can write (Part III, C.1). The
/// specification's own mapping is Rust `Option<T>` to Nikaia `T?`
/// (Part III, 15.2).
#[test]
fn a_nullable_type_round_trips_through_its_text() {
    for text in ["i64?", "ref String?", "String?", "Vec[i64]?", "Vec[i64?]"] {
        let parsed = Ty::parse(text);
        assert_eq!(parsed.text(), text, "{text} does not come back as itself");
    }

    // `?` alone is `Unknown`, which is the one spelling this shares a character
    // with, and the two stay apart.
    assert!(Ty::parse("?").is_unknown());
    assert!(!Ty::parse("i64?").is_unknown());
}

/// **The one widening this language has**, and only in that direction.
///
/// A plain `T` fits a `T?`, because Part I 2.3 writes it: `let mut m: &str? =
/// null` and then `m = "World"`. A `T?` does not fit a `T` — that is the whole
/// point of the type being separate, and `??` (3.5) is how a program gets from
/// one to the other.
#[test]
fn a_plain_value_fits_a_nullable_slot_and_not_the_other_way() {
    let plain = Ty::parse("i64");
    let nullable = Ty::parse("i64?");
    assert!(plain.fits(&nullable), "a `T` stands where a `T?` is wanted");
    assert!(!nullable.fits(&plain), "a `T?` does not stand for a `T`");

    // And two nullables fit when what they may hold fits.
    assert!(nullable.fits(&Ty::parse("i64?")));
    assert!(!nullable.fits(&Ty::parse("String?")));
}

/// A `T?` where a `T` is wanted is refused, with both types in this language's
/// words.
#[test]
fn handing_a_nullable_where_a_plain_value_is_wanted_is_refused() {
    let source = "\
fn plain(n: i64) -> i64 { return n }
fn main() {
    let maybe: i64? = null
    println(f\"{plain(maybe)}\")
}
";
    let parsed = parse_to_ast(source).expect("parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let findings = check::check(&parsed, &own, &library).findings;
    let it = findings
        .iter()
        .find(|f| f.code == "NK1102")
        .unwrap_or_else(|| panic!("no NK1102: {findings:#?}"));
    assert!(it.message.contains("i64?"), "{it:#?}");
    assert!(
        !it.message.contains("Option"),
        "a message may not name a type the program cannot write (C.1): {it:#?}"
    );
}

// --- Part I 3.5's `?.` ------------------------------------------------------

/// **The section's own shape**, chained, and it compiles.
///
/// `map` over a plain field and `and_then` over one that is itself a `T?`: the
/// second is the whole difficulty, because `map` there would give an
/// `Option<Option<T>>` and `a?.b?.c` would come out holding a nullable of a
/// nullable. Which of the two is right is a question about the declared type,
/// so the checker decides and this emitter writes the word
/// ([ADR-278](../../../docs/specification/adr/adr-278.md),
/// [ADR-288](../../../docs/specification/adr/adr-288.md)).
#[test]
fn a_safe_reach_maps_over_a_plain_field_and_flattens_a_nullable_one() {
    let rust = compiled(
        "safe-navigation",
        "\
struct Address { city: String, zip: String? }
struct User { name: String, home: Address? }

fn find(id: i64) -> User? {
    if id > 0 {
        let home = Address { city: \"Bletchley\", zip: null }
        return User { name: \"Ada\", home: home }
    }
    return null
}

fn main() {
    let name = find(1)?.name ?? \"nobody\"
    let city = find(1)?.home?.city ?? \"nowhere\"
    let zip = find(1)?.home?.zip ?? \"none\"
    println(f\"{name} {city} {zip}\")
}
",
    );
    // `name` is a plain `String`, so `map`.
    assert!(
        rust.contains(".map(|__nikaia_it| __nikaia_it.name)"),
        "{rust}"
    );
    // `home` is an `Address?`, so `and_then` — otherwise `?.home?.city` would
    // be reaching through a nullable of a nullable.
    assert!(
        rust.contains(".and_then(|__nikaia_it| __nikaia_it.home)"),
        "{rust}"
    );
    // And `zip` is a `String?`, so `and_then` again.
    assert!(
        rust.contains(".and_then(|__nikaia_it| __nikaia_it.zip)"),
        "{rust}"
    );
}

/// **A struct-literal field is Part I 2.3's fourth position for the wrap.**
///
/// `Address(city: …, zip: null)` needs nothing, and `User(name: …, home: home)`
/// where `home` is an `Address` and the field is an `Address?` needs the
/// `Some(…)`. Keyed by the field's own name, because a struct literal has one of
/// these per field and a statement has only one span.
#[test]
fn a_plain_value_in_a_nullable_field_is_wrapped() {
    let rust = compiled(
        "nullable-field",
        "\
struct Address { city: String }
struct User { name: String, home: Address? }

fn main() {
    let home = Address { city: \"Bletchley\" }
    let u = User { name: \"Ada\", home: home }
    let city = u.home?.city ?? \"nowhere\"
    println(f\"{city}\")
}
",
    );
    assert!(rust.contains("home: Some(home)"), "{rust}");
    // **The one line option A migrated** ([ADR-278](../../../docs/specification/adr/adr-278.md)
    // D2). `u.home` roots in a binding, so `?.city` is a view of it, and the
    // fallback is a text literal - which is already a view, reads the same and
    // costs nothing. The `.to_string()` that stood here was only ever matching
    // a left side that used to be owned.
    assert!(
        rust.contains("__nikaia_it.city.as_str()"),
        "the reach is a view of `u`:\n{rust}"
    );
}

/// **`?.` through something that cannot be absent is `NK1121`.**
///
/// The language below has no `map` on a plain struct, so this was `rustc`'s
/// refusal about the generated file. The way out is the plain `.`, which is
/// what the program meant.
#[test]
fn reaching_through_a_plain_value_is_refused() {
    let source = "\
struct User { name: String }
fn main() {
    let u = User { name: \"Ada\" }
    let n = u?.name
    println(f\"{n}\")
}
";
    let parsed = parse_to_ast(source).expect("parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let findings = check::check(&parsed, &own, &library).findings;
    let it = findings
        .iter()
        .find(|f| f.code == "NK1121")
        .unwrap_or_else(|| panic!("no NK1121: {findings:#?}"));
    assert!(it.message.contains("`User`"), "{it:#?}");
    assert!(
        it.help.as_deref() == Some("Write `.name`."),
        "every error names a way out (Part III C.2): {it:#?}"
    );
}

/// And a receiver this checker could not work out says nothing: refusing there
/// would refuse a correct program (Part III, C.4).
#[test]
fn reaching_through_an_unknown_receiver_is_not_refused() {
    let source = "use std::cli\n\n\
fn main() {
    let whatever = cli::args().nth(1)
    let n = whatever?.something
}
";
    let parsed = parse_to_ast(source).expect("parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let findings = check::check(&parsed, &own, &library).findings;
    assert!(
        !findings.iter().any(|f| f.code == "NK1121"),
        "{findings:#?}"
    );
}

/// **`a ?? b` is not `a?.b`**, and the grammar keeps them apart by spelling
/// `?.` as one token: the generator cannot insert the implicit whitespace
/// inside a literal, so `a ? . b` is not safe navigation either.
#[test]
fn the_coalescing_operator_is_not_a_safe_reach() {
    let rust = lowered("fn main() { let a: i64? = null\nlet b = a ?? 1 }");
    // `index::or` since [ADR-293](../../../docs/specification/adr/adr-293.md)
    // D2; what this asserts is that a `??` is not a **reach**, which is the
    // same either way.
    assert!(rust.contains("nikaia_std::index::or("), "{rust}");
    assert!(!rust.contains("map("), "{rust}");

    assert!(
        parse_to_ast("fn main() { let a: i64? = null\nlet b = a ? . x }").is_err(),
        "`?` and `.` apart is not `?.`"
    );
}

// --- the wrap at an argument, and `?.m()` -----------------------------------

/// **The third position for D4's wrap**, keyed by the callee as the source
/// wrote it and the argument's position.
///
/// That is the narrowest key that works: an expression carries no span, a
/// statement may hold several calls, and one call may pass several arguments.
/// The **written** name and not the resolved one, because the emitter has only
/// what the source says — a method's key is `Type::method` and a constructor's
/// is `Type::new`, and neither stands at the call.
#[test]
fn a_plain_value_in_a_nullable_parameter_is_wrapped() {
    let rust = compiled(
        "nullable-argument",
        "\
fn shown(what: String?) -> String {
    return what ?? \"nothing\"
}

fn pair(a: i64?, b: i64?) -> i64 {
    return (a ?? 0) + (b ?? 0)
}

fn main() {
    println(f\"{shown(\"here\")}\")
    println(f\"{shown(null)}\")
    println(f\"{pair(1, 2)}\")
}
",
    );
    assert!(
        rust.contains("shown(Some(String::from(\"here\")))"),
        "{rust}"
    );
    // `null` is a `T?` already, so nothing goes round it.
    assert!(rust.contains("shown(None)"), "{rust}");
    assert!(!rust.contains("Some(None)"), "{rust}");
    // Two parameters, told apart by their position.
    assert!(rust.contains("pair(Some(1), Some(2))"), "{rust}");
}

/// And a value that is **already** nullable is handed on as it is.
#[test]
fn a_nullable_argument_is_not_wrapped() {
    let rust = compiled(
        "nullable-argument-passthrough",
        "\
struct Box { label: String? }

fn shown(what: String?) -> String {
    return what ?? \"nothing\"
}

fn main() {
    let b = Box { label: \"on it\" }
    println(f\"{shown(b.label)}\")
}
",
    );
    assert!(rust.contains("shown(b.label)"), "{rust}");
    assert!(!rust.contains("Some(b.label)"), "{rust}");
    // The field itself does need the wrap, which is the other position.
    assert!(
        rust.contains("label: Some(String::from(\"on it\"))"),
        "{rust}"
    );
}

/// **`?.` reaches a method**, because Part I 3.5 says it reaches a *member* and
/// a method is one ([ADR-278](../../../docs/specification/adr/adr-278.md)).
///
/// It used to be refused with a sentence, on the reading that the section's
/// example writes a field. The word the section actually uses is "member", and
/// the owner settled which reading is the language's.
///
/// Three things at once, and each is a way the call is a **call** and not a
/// field: the arguments reach it, the receiver is reached exactly once, and
/// short-circuiting still answers `null`.
#[test]
fn a_safe_reach_calls_a_method_and_short_circuits() {
    let printed = ran(
        "safe-method",
        "\
struct User { name: String }

impl User {
    fn greet(ref self, greeting: ref String) -> String {
        return f\"{greeting}, {self.name}\"
    }
}

fn find(id: i64) -> User? {
    if id > 0 {
        return User { name: \"Ada\" }
    }
    return null
}

fn main() {
    let here = find(1)?.greet(\"Hallo\") ?? \"nobody\"
    let gone = find(0)?.greet(\"Hallo\") ?? \"nobody\"
    println(f\"{here} | {gone}\")
}
",
    );
    assert_eq!(printed.trim(), "Hallo, Ada | nobody");
}

/// **A `match` and not the field's `map`**, and the reason is what a method can
/// do that a field cannot: pause and fail.
///
/// Inside a closure an `.await` does not compile and a `?` has nowhere to go, so
/// `map` would have bought a form that works for the easy half of the language
/// and refuses the rest. The reach is written out instead, which is the one
/// shape that lets the call be whatever a call is — and this program has a
/// method that is **both** fallible and pausing, inside the reach.
#[test]
fn a_reached_method_may_pause_and_may_fail() {
    let printed = ran(
        "safe-method-throws",
        "\
use std::fs

struct Store { root: String }

impl Store {
    fn read(ref self, path: ref String) -> String throws {
        return fs::read_to_string(path, fs::Root::Anywhere)
    }
}

fn open(yes: bool) -> Store? {
    if yes {
        return Store { root: \".\" }
    }
    return null
}

fn main() throws {
    fs::write(\"note.txt\", fs::Root::Anywhere, \"hallo\")
    let text = open(true)?.read(\"note.txt\") ?? \"\"
    let none = open(false)?.read(\"note.txt\") ?? \"missing\"
    println(f\"{text} | {none}\")
}
",
    );
    assert_eq!(printed.trim(), "hallo | missing");
}

/// **A method whose own result is a `T?` flattens**, exactly as a field of that
/// shape does (ADR-278 D9, which this extends rather than changes).
///
/// Without it `a?.b()?.c` would reach through a nullable of a nullable, and the
/// program would not compile at all — so a result that comes back is what says
/// the flattening happened.
#[test]
fn a_reached_method_that_answers_a_nullable_does_not_nest() {
    let printed = ran(
        "safe-method-flatten",
        "\
struct User { name: String }

fn long_enough(name: String) -> String? {
    if name.len() > 3 {
        return name
    }
    return null
}

impl User {
    fn nickname(ref self) -> String? {
        return long_enough(self.name.clone())
    }
}

fn find(name: ref String) -> User? {
    return User { name: name.clone() }
}

fn main() {
    let long = find(\"Alexandra\")?.nickname() ?? \"none\"
    let short = find(\"Ada\")?.nickname() ?? \"none\"
    println(f\"{long} | {short}\")
}
",
    );
    assert_eq!(printed.trim(), "Alexandra | none");
}

/// **`?.` onto a method of something that cannot be absent is `NK1121`**, the
/// same refusal the field gets — and the way out is spelled as a *call*, which
/// is the one place the two members differ.
#[test]
fn a_reached_method_on_a_plain_value_is_refused_in_the_spelling_it_was_written() {
    let found = findings(
        "\
struct U { name: String }
impl U { fn n(ref self) -> i64 { return 1 } }
fn main() {
    let u = U { name: \"a\" }
    let x = u?.n()
}
",
    );
    let one = found
        .iter()
        .find(|f| f.code == "NK1121")
        .expect("a reach through a plain value is refused");
    assert!(one.message.contains("`U`"), "{:?}", one.message);
    let help = one.help.clone().unwrap_or_default();
    assert!(help.contains(".n(…)"), "a call, not a field: {help}");
    assert!(
        one.notes.iter().any(|n| n.contains("the method")),
        "{:?}",
        one.notes
    );
}

/// **`??` chains**, which it did not
/// ([ADR-278](../../../docs/specification/adr/adr-278.md)).
///
/// `a ?? b ?? c` was a parse error naming the *second* `??`, in a language whose
/// page says the operator provides a fallback and nowhere says a value may have
/// only one. The tail was parsed at a precedence *below* the rule itself, so it
/// could not hold another one.
///
/// **Right-associative**: `a ?? (b ?? c)`, which is what the types ask for — the
/// last fallback is the plain value that ends the chain and every `??` before it
/// takes the `T?` on its left.
#[test]
fn a_chain_of_fallbacks_takes_the_first_one_that_has_a_value() {
    let printed = ran(
        "coalesce-chain",
        "\
fn a() -> String? { return null }
fn b() -> String? { return null }
fn c() -> String? { return \"third\" }

fn main() {
    let none = a() ?? b() ?? \"last\"
    let third = a() ?? b() ?? c() ?? \"last\"
    println(f\"{none} | {third}\")
}
",
    );
    assert_eq!(printed.trim(), "last | third");
}

/// **The two operators of 3.5, in one expression.**
///
/// A reach that answers `null` falls through to the next one, and the chain ends
/// in the plain value — which is the shape the section's own prose describes and
/// neither half could carry on its own before this.
#[test]
fn a_reach_that_answers_null_falls_through_to_the_next_fallback() {
    let printed = ran(
        "coalesce-and-reach",
        "\
struct User { name: String }

impl User {
    fn greet(ref self) -> String { return f\"hi, {self.name}\" }
}

fn find(id: i64) -> User? {
    if id > 0 {
        return User { name: \"Ada\" }
    }
    return null
}

fn main() {
    let found = find(0)?.greet() ?? find(1)?.greet() ?? \"nobody\"
    let neither = find(0)?.greet() ?? find(0)?.greet() ?? \"nobody\"
    println(f\"{found} | {neither}\")
}
",
    );
    assert_eq!(printed.trim(), "hi, Ada | nobody");
}

/// **A plain `.` on a `T?` is refused**, and it is `NK1121` the other way round
/// ([ADR-278](../../../docs/specification/adr/adr-278.md) D14).
///
/// `a?.b.c` guards `a` and nothing else — which is what safe navigation means in
/// every language that has it, and was worth getting right rather than assuming.
/// Where `null` inhabits every reference type, the unguarded `.c` is a crash at
/// run time. Here it cannot be: types are non-nullable by default and `T?` is a
/// **separate type** (Part I 2.3), so `.c` on one is a member the type does not
/// have — answerable where it is written, like `.c` on an `i64`.
///
/// It used to be neither: `find(1)?.b.c` lowered to `find(1).map(…).c`, a field
/// read off an `Option`, and the reader met `rustc` about a file nobody wrote.
#[test]
fn a_member_of_a_nullable_is_refused_with_the_guarded_form_as_the_way_out() {
    let source = "\
struct Inner { c: i64 }
struct Outer { b: Inner }

fn find(id: i64) -> Outer? {
    if id > 0 { return Outer { b: Inner { c: 7 } } }
    return null
}

fn main() {
    let x = find(1)?.b.c
}
";
    let found = findings(source);
    let one = found
        .iter()
        .find(|f| f.code == "NK1125")
        .unwrap_or_else(|| panic!("a member of a `T?` is refused: {found:#?}"));
    assert!(one.message.contains("`Inner?`"), "{}", one.message);
    let help = one.help.clone().unwrap_or_default();
    assert!(
        help.contains("?.c"),
        "the guarded form is the way out: {help}"
    );
    assert!(help.contains("??"), "and so is ending the chain: {help}");
}

/// **And the guarded chain runs**, which is the other half of the same rule: the
/// refusal above is only worth having if what it asks for works.
///
/// Three shapes, and the middle one is the point — `find(0)` answers `null` and
/// the rest of the chain is never reached, which is the short-circuit. The third
/// takes the value with `??` first and then reaches into it plainly, which is
/// the refusal's second way out.
#[test]
fn a_guarded_chain_reaches_through_and_short_circuits() {
    let printed = ran(
        "nullable-chain",
        "\
struct Inner { c: i64 }
struct Outer { b: Inner }

fn find(id: i64) -> Outer? {
    if id > 0 { return Outer { b: Inner { c: 7 } } }
    return null
}

fn main() {
    let there = find(1)?.b?.c ?? 0
    let gone = find(0)?.b?.c ?? 0
    let taken = (find(1) ?? Outer { b: Inner { c: 1 } }).b.c
    println(f\"{there} {gone} {taken}\")
}
",
    );
    assert_eq!(printed.trim(), "7 0 7");
}

/// **A value this checker cannot type gets `.into()` and not `Some(…)`**
/// ([ADR-278](../../../docs/specification/adr/adr-278.md)).
///
/// The rule used to write the constructor where it **knew** the value was a
/// plain `T`, and stay silent otherwise — right about the risk, since wrapping a
/// value that is already a `T?` makes an `Option<Option<T>>`, and wrong about
/// what to do with it: the program then failed in the language below with
/// `rustc`'s *"try wrapping the expression in `Some`"* about a form Nikaia does
/// not have.
///
/// **Both directions, in one program, through one rule.** Both values are `?` to
/// this checker — and one is a plain `String` while the other is **already** an
/// `Option<&str>`. `.into()` is the wrap for the first and the identity for the
/// second, which is the whole of why it needs no answer to a question the
/// checker could not settle.
///
/// **Fragile on purpose, and named as such**: the `?` comes from `.clone()` and
/// `strip_prefix` being in no ledger, and every real `std` name is a candidate
/// for being written down — two fixtures in this repository have already broken
/// that way. The stable source would be
/// [ADR-024](../../../docs/specification/adr/adr-024.md) D4's erased generic,
/// which is an absence the **language** decides; it cannot be used until a
/// generic function lowers with its `<T>`, which it now does
/// ([ADR-295](../../../docs/specification/adr/adr-295.md)). Whoever writes
/// `String::clone` down should move this fixture rather than delete it — what it
/// measures is the rule, not the ledger.
#[test]
fn a_value_of_unknown_type_is_converted_rather_than_left_alone() {
    let printed = ran(
        "nullable-into",
        "\
struct U { name: String }

impl U {
    fn copy(ref self) -> String? { return self.name.clone() }
    fn rest(ref self) -> ref String? { return self.name.strip_prefix(\"A\") }
}

fn main() {
    let u = U { name: \"Ada\" }
    let v = U { name: \"zzz\" }
    println(f\"{u.copy() ?? \"none\"}\")
    println(f\"{u.rest() ?? \"no prefix\"}\")
    println(f\"{v.rest() ?? \"no prefix\"}\")
}
",
    );
    assert_eq!(printed.trim(), "Ada\nda\nno prefix");
}

/// **And a value it can type keeps the constructor**, which is what the second
/// form is for: the generated Rust goes on saying `Some(…)` wherever the
/// compiler knows enough to say it, so the conversion is what uncertainty
/// costs rather than what every program pays.
///
/// The untypeable half used to be `self.name.clone()` and is
/// `common::undescribed_value`'s method now:
/// [ADR-083](../../../docs/specification/adr/adr-083.md) put `String::clone` in
/// the ledger, because `NK1131`'s advice is to write one and a way out that
/// makes another diagnostic fire is not a way out. So this test's example of a
/// value nothing can type became one that something can — the ledger getting
/// better, and a test that had been resting on it.
#[test]
fn a_value_of_known_type_still_gets_the_constructor() {
    let unknown = common::undescribed_value("self.name");
    let rust = lowered(&format!(
        "\
struct U {{ name: String }}

impl U {{
    fn known(ref self) -> String? {{ return \"lit\" }}
    fn unknown(ref self) -> String? {{ return {unknown} }}
}}

fn free() -> String? {{ return \"lit\" }}
",
    ));
    assert!(
        rust.contains("Some(String::from(\"lit\"))"),
        "a known type keeps the constructor:\n{rust}"
    );
    assert!(
        rust.contains(&format!("{unknown}.into()")),
        "an unknown one takes the conversion:\n{rust}"
    );
    // Two of the three are `Some(…)`, so the conversion is the exception rather
    // than the rule - the `impl` and the free function alike.
    assert_eq!(
        rust.matches("Some(String::from(\"lit\"))").count(),
        2,
        "{rust}"
    );
}

// --- one statement, two wraps ------------------------------------------------
//
// Part I 2.3's wrap is recorded by the checker and written by the emitter, and
// what joins the two is a key built out of what both can see — an argument
// carries no span of its own. The key named the **parameter** and not the
// *call*, so a
// statement that called one function twice had one entry for two arguments and
// the last one walked won. Found while writing a test for something else, in an
// `f"…"`; it has nothing to do with strings.

/// Two calls to one function in one statement, one argument needing the wrap
/// and one already being it.
///
/// Compiled, because that is the whole question: `pick(Some(None))` is what the
/// defect emitted and `rustc` is what refused it, about a file nobody wrote.
#[test]
fn two_calls_to_one_function_in_one_statement_get_their_own_wraps() {
    let rust = compiled(
        "two-calls",
        "fn pick(a: i64?) -> i64 {\n\
         \x20   return a ?? 7\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let r = pick(1) + pick(null)\n\
         \x20   println(f\"{r}\")\n\
         }\n",
    );
    assert!(rust.contains("pick(Some(1)) + pick(None)"), "{rust}");
}

/// **And in the other order**, because the entry that won was whichever was
/// walked last: with the `null` first the defect wrapped *both*.
#[test]
fn the_order_of_two_calls_does_not_decide_their_wraps() {
    let rust = compiled(
        "two-calls-reversed",
        "fn pick(a: i64?) -> i64 {\n\
         \x20   return a ?? 7\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let r = pick(null) + pick(1)\n\
         \x20   println(f\"{r}\")\n\
         }\n",
    );
    assert!(rust.contains("pick(None) + pick(Some(1))"), "{rust}");
}

/// The shape it was found in: two holes of one `f"…"`. The holes are text until
/// each pass parses them, so nothing about them is addressable — which is why
/// the key has to be structural.
#[test]
fn two_holes_of_one_string_get_their_own_wraps() {
    let rust = compiled(
        "two-holes",
        "fn pick(a: i64?) -> i64 {\n\
         \x20   return a ?? 7\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(f\"{pick(1)}{pick(null)}\")\n\
         }\n",
    );
    assert!(rust.contains("pick(Some(1)), pick(None)"), "{rust}");
}

/// The same key design, the same defect, in a struct literal: a statement may
/// build two of them.
#[test]
fn two_struct_literals_in_one_statement_get_their_own_wraps() {
    let rust = compiled(
        "two-literals",
        "struct P {\n\
         \x20   x: i64?,\n\
         }\n\
         \n\
         fn hold(p: P) -> i64 {\n\
         \x20   return p.x ?? 9\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let r = hold(P { x: 2 }) + hold(P { x: null })\n\
         \x20   println(f\"{r}\")\n\
         }\n",
    );
    assert!(
        rust.contains("P { x: Some(2) }") && rust.contains("P { x: None }"),
        "{rust}"
    );
}

/// **Two structs, one field name, both written in the shorthand** — which the
/// value cannot tell apart, because both are the name `x`. The type is in the
/// key for this case.
#[test]
fn two_structs_sharing_a_field_name_get_their_own_wraps() {
    let rust = compiled(
        "two-structs",
        "struct P {\n\
         \x20   x: i64?,\n\
         }\n\
         \n\
         struct Q {\n\
         \x20   x: i64,\n\
         }\n\
         \n\
         fn hold(a: P, b: Q) -> i64 {\n\
         \x20   return (a.x ?? 0) + b.x\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let x = 1\n\
         \x20   let r = hold(P { x }, Q { x })\n\
         \x20   println(f\"{r}\")\n\
         }\n",
    );
    assert!(
        rust.contains("P { x: Some(x) }") && rust.contains("Q { x }"),
        "{rust}"
    );
}

/// And the arithmetic is the point: the wraps are in the right places, so the
/// program means what it says.
#[test]
fn a_statement_with_two_wraps_runs() {
    let source = "struct P {\n\
         \x20   x: i64?,\n\
         }\n\
         \n\
         fn pick(a: i64?) -> i64 {\n\
         \x20   return a ?? 7\n\
         }\n\
         \n\
         fn hold(p: P) -> i64 {\n\
         \x20   return p.x ?? 9\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let r = pick(1) + pick(null)\n\
         \x20   let s = hold(P { x: 2 }) + hold(P { x: null })\n\
         \x20   println(f\"{r} {s}\")\n\
         }\n";
    // 1 + 7, and 2 + 9.
    assert_eq!(ran("two-wraps-run", source).trim(), "8 11");
}

// ---------------------------------------------------------------------------
// `?.` reaches through a view of its receiver
// ([ADR-278](../../../docs/specification/adr/adr-278.md) D16,
// [ADR-278](../../../docs/specification/adr/adr-278.md))
// ---------------------------------------------------------------------------

/// **The line [ADR-278](../../../docs/specification/adr/adr-278.md) was written
/// for, for a member that copies.** `user?.id` used to take `user`, so a second
/// reach was `rustc`'s *use of moved value* about a file nobody wrote
/// (Part III, C.1) — with a `help: consider calling .as_ref()` and a
/// `.clone()` beside it, neither of which this language has.
///
/// It **runs** rather than only compiles, because the thing that would go wrong
/// with `as_ref()` is a value read out of the wrong place.
#[test]
fn a_reached_field_that_copies_leaves_the_receiver_where_it_was() {
    let printed = ran(
        "safe-field-lends",
        "\
struct User { name: String, id: i64 }

fn main() {
    let user: User? = User { name: \"Ada\", id: 7 }
    let first = user?.id ?? 0
    let again = user?.id ?? 0
    println(f\"{first} {again}\")
}
",
    );
    assert_eq!(printed.trim(), "7 7");
}

/// **And for a method**, which is the half that needs no representation at all:
/// what comes out of a reached method is the **call's** result rather than a
/// view of the receiver.
#[test]
fn a_reached_method_leaves_the_receiver_where_it_was() {
    let printed = ran(
        "safe-method-lends",
        "\
struct User { name: String }

impl User {
    fn greet(ref self, word: ref String) -> String {
        return f\"{word}, {self.name}\"
    }
}

fn main() {
    let u: User? = User { name: \"Ada\" }
    let first = u?.greet(\"Hallo\") ?? \"nobody\"
    let again = u?.greet(\"Servus\") ?? \"nobody\"
    println(f\"{first} | {again}\")
}
",
    );
    assert_eq!(printed.trim(), "Hallo, Ada | Servus, Ada");
}

/// **The scrutinee is lent only where every candidate says the call changes
/// nothing**, which is `NK1138`'s own rule one construct over.
///
/// A name this compiler cannot resolve is claimed nothing about, and the reach
/// lowers exactly as it did — the direction that cannot break a program that
/// worked ([C.4](../../../docs/specification/30-nikaia-tooling.md)).
#[test]
fn a_reach_this_compiler_cannot_resolve_lowers_as_it_did() {
    let rust = lowered(
        "\
fn main() {
    let x = whatever()?.wobble()
}
",
    );
    assert!(rust.contains("match whatever()"), "{rust}");
    assert!(!rust.contains("whatever().as_ref()"), "{rust}");
}

/// **And a member that does not copy is a view of the receiver**
/// ([ADR-278](../../../docs/specification/adr/adr-278.md) D17,
/// [ADR-278](../../../docs/specification/adr/adr-278.md) D19) — **Borrowed**,
/// not Tethered: the view points into a binding that outlives the statement,
/// which is what [ADR-283](../../../docs/specification/adr/adr-283.md) D2 calls
/// the free case.
///
/// It **runs**, with the receiver read on both sides of the view.
#[test]
fn a_reached_field_that_moves_over_a_place_is_a_view() {
    let printed = ran(
        "safe-field-view",
        "\
struct User { name: String, tags: Vec[i64] }

fn main() {
    let user: User? = User { name: \"Ada\", tags: [1, 2, 3] }
    let name = user?.name ?? \"nobody\"
    let many = user?.tags?.len() ?? 0
    println(f\"{name} {many}\")
}
",
    );
    assert_eq!(printed.trim(), "Ada 3");
}

/// **A receiver that is a temporary keeps its old lowering, and that is the
/// remainder** ([ADR-278](../../../docs/specification/adr/adr-278.md) D19).
///
/// A view of `find(1)` would point into a value that dies at the `;`, and
/// binding it is `rustc`'s *temporary value dropped while borrowed* about a
/// file nobody wrote — so the reach takes the value, as it always did. A
/// temporary has no next line to stay usable on, so
/// [ADR-278](../../../docs/specification/adr/adr-278.md) D16's promise is kept
/// where it means anything.
///
/// Written as an assertion about the **lowering**, so the day the remainder is
/// built the test that has to change says so.
#[test]
fn a_reached_field_over_a_temporary_still_takes_it() {
    let rust = lowered(
        "\
struct User { name: String }

fn find(id: i64) -> User? {
    if id > 0 { return User { name: \"Ada\" } }
    return null
}

fn main() {
    let name = find(1)?.name ?? \"nobody\"
    println(f\"{name}\")
}
",
    );
    assert!(
        rust.contains("find(1).map(|__nikaia_it| __nikaia_it.name)"),
        "a temporary receiver is unchanged:\n{rust}"
    );
    assert!(!rust.contains("find(1).as_ref()"), "{rust}");
}

/// **`??` joins two views, and a fallback that owns is refused** (`NK1185`,
/// [ADR-278](../../../docs/specification/adr/adr-278.md) D20).
///
/// The three ways to hand back one value that is both a view and an owned one:
/// a copy on the borrowed branch, which
/// [ADR-283](../../../docs/specification/adr/adr-283.md) D3 bans outright; a
/// view fallback, which a text literal already is; or saying so here, rather
/// than letting `rustc` say *expected `String`, found `&str`* about a file
/// nobody wrote.
#[test]
fn a_fallback_that_owns_what_the_reach_views_is_refused() {
    let source = "\
struct User { name: String }

fn main() {
    let user: User? = User { name: \"Ada\" }
    let nobody: String = \"nobody\"
    let name = user?.name ?? nobody
    println(f\"{name}\")
}
";
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let found = check::check(&parsed, &own, &library).findings;
    let refused = found.iter().find(|f| f.code == "NK1185").expect("refused");
    assert!(refused.message.contains("`ref String`"), "{refused:#?}");
    assert!(refused.message.contains("`String`"), "{refused:#?}");
    // A way out the program can take, which is what C.2 asks of one.
    assert!(
        refused.help.as_deref().unwrap_or_default().contains("view"),
        "{refused:#?}"
    );

    // **And the way out is accepted**, which is the half that makes it a way
    // out: a text literal is already a view.
    let taken = source.replace("?? nobody", "?? \"nobody\"");
    let parsed = parse_to_ast(&taken).expect("the way out parses");
    let own = Ledger::infer(&parsed);
    assert!(
        check::check(&parsed, &own, &library).findings.is_empty(),
        "the way out is a program"
    );
}

/// **A `?.` view out of a temporary is held for the rest of the block**
/// ([ADR-278](../../../docs/specification/adr/adr-278.md) D21): `find(1)` is
/// bound on the line before, and the view the reach hands back points into
/// that binding. It used to be `rustc`'s *temporary value dropped while
/// borrowed*.
const HELD: &str = "\
struct User {
    name: String,
    tags: Vec[String],
}

impl User {
    fn first(ref self) -> ref String {
        return self.tags[0]
    }
    fn label(ref self) -> ref String {
        return self.name
    }
}

fn find(id: i64) -> User? {
    if id == 1 {
        return User { name: f\"ada\", tags: [f\"a\", f\"b\"] }
    }
    return null
}

fn main() {
    let a = find(1)?.label() ?? \"none\"
    let b = find(2)?.first() ?? \"none\"
    let c = find(1)?.first()
    println(f\"{a} {b} {c ?? \"-\"}\")
}
";

#[test]
fn a_view_out_of_a_temporary_is_held_for_the_rest_of_the_block() {
    assert_eq!(ran("held", HELD).trim(), "ada none a");
    let rust = lowered(HELD);
    assert!(rust.contains("let __nikaia_held_0 = find(1);"), "{rust}");
}

/// **Where holding it first would change what runs, the program is refused**
/// (ADR-278 D22): on the lazy side of `??`, `find` would run where it did not.
#[test]
fn a_view_out_of_a_temporary_on_the_lazy_side_is_refused() {
    let source = HELD.replace(
        "let a = find(1)?.label() ?? \"none\"",
        "let given: String? = null\n    let a = given ?? find(1)?.label() ?? \"none\"",
    );
    let parsed = nikaia::parser::parse_to_ast(&source).expect("parses");
    let refused =
        nikaia::emit::emit_program(&parsed, nikaia::emit::Build::default()).expect_err("refused");
    assert!(
        refused
            .to_string()
            .contains("Put the value in a `let` on the line before"),
        "{refused}"
    );
}

/// **What a `match` over a call binds has the variant's type**
/// (issue #171, found moving `fold` into Nikaia): `let a = match
/// make(n) { R::Value(c) => c, other => return other }` left `a` untyped, so
/// `a.name?.clone()` moved the field out where `a` was read again, and a
/// method on `a` made the function look as if it paused.
#[test]
fn a_part_of_a_matched_call_is_typed_and_read_in_place() {
    let printed = ran(
        "matched-call",
        "struct C { name: String?, n: i64 }\n\
         enum R { Value(C), Nothing }\n\
         \n\
         fn make(n: i64) -> R {\n\
         \x20   return if n > 0 { R::Value(C { name: \"a\", n: n }) } else { R::Nothing }\n\
         }\n\
         \n\
         fn both(n: i64) -> R {\n\
         \x20   let a = match make(n) {\n\
         \x20       R::Value(c) => c,\n\
         \x20       other => return other,\n\
         \x20   }\n\
         \x20   let b = match make(n + 1) {\n\
         \x20       R::Value(c) => c,\n\
         \x20       other => return other,\n\
         \x20   }\n\
         \x20   let pinned: String? = if a.name == null { b.name?.clone() } else { a.name?.clone() }\n\
         \x20   return R::Value(C { name: pinned, n: a.n + b.n })\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   match both(1) {\n\
         \x20       R::Value(c) => println(f\"{c.n}\"),\n\
         \x20       R::Nothing => println(\"none\"),\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(printed.trim(), "3");
}

/// **A nullable view is lent inside its option, and a copy of it is text of
/// its own** (issue #171, found moving `fold` into Nikaia): a
/// `ref String?` parameter is an `Option<&str>` (Part I 2.3), its caller hands
/// `x.as_deref()` and `None` for `null`, and `b?.clone()` in the body is
/// `to_owned()` of the `&str` reached.
#[test]
fn a_nullable_view_is_handed_and_copied() {
    let printed = ran(
        "nullable-view",
        "fn either(a: ref String?, b: ref String?) -> String? {\n\
         \x20   return b?.clone() if a == null\n\
         \x20   return a?.clone()\n\
         }\n\
         \n\
         struct Holder { name: String? }\n\
         \n\
         fn main() {\n\
         \x20   let x: String? = \"x\"\n\
         \x20   let y: String? = null\n\
         \x20   let h = Holder { name: \"h\" }\n\
         \x20   let got = either(y, x) ?? \"-\"\n\
         \x20   let from = either(h.name, null) ?? \"-\"\n\
         \x20   let none = either(null, null) ?? \"-\"\n\
         \x20   let kept = x ?? \"?\"\n\
         \x20   println(f\"{got} {from} {none} {kept}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "x h - x");
}

/// **A plain arm beside a `null` is `Some`** (found moving `contracts::trust`
/// into Nikaia): `if b { 1 } else { null }` was `if b { 1 } else { None }
/// .into()` below, which the language below cannot type - the conversion
/// belongs to the arm, and the arm is a plain value.
#[test]
fn a_plain_arm_beside_a_null_is_the_value() {
    let printed = ran(
        "arm-beside-null",
        "enum W { A, B }\n\
         \n\
         fn f(b: bool) -> i64? {\n\
         \x20   return if b { 1 } else { null }\n\
         }\n\
         \n\
         fn g(n: i64) -> W? {\n\
         \x20   return match n {\n\
         \x20       0 => if n == 0 { W::B } else { null },\n\
         \x20       else => null,\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let x = f(true) ?? 0\n\
         \x20   let y = f(false) ?? 7\n\
         \x20   let w = g(0) ?? W::A\n\
         \x20   let z = match w {\n\
         \x20       W::B => 2,\n\
         \x20       else => 3,\n\
         \x20   }\n\
         \x20   println(f\"{x} {y} {z}\")\n\
         }\n",
    );
    assert_eq!(printed.trim(), "1 7 2");
}

/// **`NK1205`: a variant matched on a `T?`** (issue #180): the
/// pattern says nothing about `null`, and the lowering was a `match` over an
/// `Option<W>` with `W::B` as its pattern. The way out, `??` first, compiles.
#[test]
fn a_variant_matched_on_a_nullable_is_refused() {
    let source = |scrutinee: &str| {
        format!(
            "enum W {{ A, B }}\n\
             fn g(n: i64) -> W? {{\n\
             \x20   return if n == 0 {{ W::B }} else {{ null }}\n\
             }}\n\
             fn main() {{\n\
             \x20   {scrutinee}\n\
             \x20   println(f\"{{z}}\")\n\
             }}\n"
        )
    };
    let found = findings(&source(
        "let z = match g(0) {\n        W::B => 2,\n        else => 3,\n    }",
    ));
    let it = found
        .iter()
        .find(|f| f.code == "NK1205")
        .unwrap_or_else(|| panic!("no NK1205: {found:#?}"));
    assert!(it.message.contains("`W::B`"), "{it:#?}");
    assert!(
        it.help.as_deref().is_some_and(|h| h.contains("??")),
        "{it:#?}"
    );

    let printed = ran(
        "variant-after-fallback",
        &source(
            "let w = g(0) ?? W::A\n    let z = match w {\n        W::B => 2,\n        else => 3,\n    }",
        ),
    );
    assert_eq!(printed.trim(), "2");
}

/// **A nullable part of a lent value, handed to a `ref T?`**: `name` in
/// `Shape::Named { name, .. }` over a `ref Shape` is a view of an `Option`
/// below, and the parameter is an option of a view. `shown(name)` reached
/// `rustc` as *expected `Option<&str>`, found `&Option<String>`* - found
/// moving `Ty` into Nikaia (ADR-294), where `Ty::Fn`'s result is one.
#[test]
fn a_nullable_part_of_a_lent_value_is_opened_for_a_nullable_view() {
    let printed = ran(
        "lent-nullable-part",
        r#"pub struct Row {
    pub id: i64,
}

pub enum Shape {
    Named { name: String?, count: i64 },
    Held { row: Row? },
    Nothing,
}

fn shown(name: ref String?) -> String {
    return name ?? "none"
}

fn id_of(row: ref Row?) -> i64 {
    if row == null {
        return 0
    }
    return row?.id ?? 0
}

fn describe(s: ref Shape) -> String {
    return match s {
        Shape::Named { name, count } => f"{shown(name)} {count}",
        Shape::Held { row } => f"row {id_of(row)}",
        Shape::Nothing => "nothing",
    }
}

fn main() {
    println(describe(Shape::Named { name: "a", count: 2 }))
    println(describe(Shape::Named { name: null, count: 3 }))
    println(describe(Shape::Held { row: Row { id: 7 } }))
    println(describe(Shape::Held { row: null }))
    println(describe(Shape::Nothing))
}
"#,
    );
    assert_eq!(printed, "a 2\nnone 3\nrow 7\nrow 0\nnothing\n");
}

/// **A name on the left of `??` is handed over** when what it holds does not
/// copy: `viewed ?? "none"` takes the option, and a use of `viewed` after the
/// line reached `rustc` as *borrow of moved value* (found moving `Ty::parse`
/// into Nikaia, ADR-294). It is `NK2105` now, with the copy as the way out;
/// a nullable number is copied and stays usable.
#[test]
fn a_name_given_to_a_coalesce_is_handed_over() {
    let taken = findings(
        "fn find(n: i64) -> String? {\n    if n > 0 {\n        return \"some\"\n    }\n    return null\n}\n\n\
         fn main() {\n    let viewed = find(1)\n    let first = viewed ?? \"none\"\n    println(first)\n    \
         if viewed != null {\n        println(\"again\")\n    }\n}\n",
    );
    let refusal = taken
        .iter()
        .find(|f| f.code == "NK2105")
        .unwrap_or_else(|| panic!("NK2105: {taken:#?}"));
    assert!(
        refusal.message.contains("given to `??`"),
        "{}",
        refusal.message
    );

    let copied = findings(
        "fn count(n: i64) -> i64? {\n    if n > 0 {\n        return n\n    }\n    return null\n}\n\n\
         fn main() {\n    let c = count(1)\n    let first = c ?? 0\n    println(f\"{first}\")\n    \
         if c != null {\n        println(\"again\")\n    }\n}\n",
    );
    assert!(copied.is_empty(), "{copied:#?}");
}

/// **`??` lends its left side where the answer is only read, and takes it
/// where the answer is kept** (ADR-279 D5): the rule an argument follows. Each
/// read shape - a text literal, a name of text, a declared variant, a name of
/// a struct - leaves the name usable after the line and copies nothing; a
/// `return` and a `let` take it, as `let b = a` does.
#[test]
fn a_coalesce_lends_where_it_is_read_and_takes_where_it_is_kept() {
    let source = r#"pub struct Row {
    pub id: i64,
}

pub enum Shape {
    Square,
    Nothing,
}

fn find(n: i64) -> String? {
    if n > 0 {
        return "some"
    }
    return null
}

fn row(n: i64) -> Row? {
    if n > 0 {
        return Row { id: n }
    }
    return null
}

fn shape(n: i64) -> Shape? {
    if n > 0 {
        return Shape::Square
    }
    return null
}

fn shown(text: ref String) -> String {
    return f"<{text}>"
}

fn id_of(r: ref Row) -> i64 {
    return r.id
}

fn named(s: ref Shape) -> String {
    return match s {
        Shape::Square => "square",
        Shape::Nothing => "nothing",
    }
}

fn kept(n: i64) -> String {
    let user = find(n)
    return user ?? "x"
}

fn main() {
    let user = find(1)
    println(user ?? "guest")
    println(shown(user ?? "guest"))
    let other: String = "other"
    let nobody = find(0)
    println(shown(nobody ?? other))
    println(other)
    let r = row(3)
    let spare = Row { id: 0 }
    println(f"{id_of(r ?? spare)}")
    let s = shape(0)
    println(named(s ?? Shape::Nothing))
    if user != null && r != null && s == null {
        println("still usable")
    }
    println(kept(1))
    let name = user ?? "guest"
    println(name)
}
"#;
    let lowered = lowered(source);
    assert!(lowered.contains("user.as_deref()"), "{lowered}");
    // `Row` is a struct: lent whatever it copies (ADR-279 D8).
    assert!(lowered.contains("r.as_ref(), || &spare"), "{lowered}");
    assert!(!lowered.contains("clone()"), "nothing is copied: {lowered}");
    assert_eq!(
        ran("coalesce-lends", source),
        "some\n<some>\n<other>\nother\n3\nnothing\nstill usable\nsome\nsome\n"
    );
}

/// **A field on the left of `??` is a place, as a name is** (ADR-279 D5).
/// Over a `ref self`, `self.parameter ?? "none"` handed to a reading position
/// lends the field; bound by a `let`, it is a part of a loan handed over,
/// `NK2106`. Unrecorded, both reached `rustc` as *cannot move out of
/// `self.parameter`* (found moving the ledger's records into Nikaia, ADR-294).
#[test]
fn a_field_on_the_left_of_a_coalesce_is_a_place() {
    let read = r#"pub struct Touch {
    pub kind: String,
    pub parameter: String?,
}

fn shown(text: ref String) -> String {
    return f"<{text}>"
}

impl Touch {
    pub fn read(ref self) -> String {
        return shown(self.parameter ?? "none")
    }
}

fn main() {
    let t = Touch { kind: "file", parameter: "path" }
    let u = Touch { kind: "lock", parameter: null }
    println(t.read())
    println(u.read())
}
"#;
    assert!(lowered(read).contains("self.parameter.as_deref()"));
    assert_eq!(ran("field-coalesce", read), "<path>\n<none>\n");
    let kept = read.replace(
        "        return shown(self.parameter ?? \"none\")",
        "        let parameter = self.parameter ?? \"none\"\n        return shown(parameter)",
    );
    let found = findings(&kept);
    assert!(found.iter().any(|f| f.code == "NK2106"), "{found:#?}");
}

/// **The other positions that only read a `??`** (issue #98): an
/// `f"…"` hole, a comparison of text and the receiver of a method that only
/// reads it. Each lends the left side, so the name is used again after the
/// line with no `.clone()` written - which was `NK2105` before.
#[test]
fn a_coalesce_lends_in_a_hole_a_comparison_and_a_receiver() {
    let source = r#"struct Tag {
    word: String,
}

impl Tag {
    fn shown(ref self) -> String {
        return f"<{self.word}>"
    }
}

fn find(n: i64) -> String? {
    if n > 0 {
        return "found"
    }
    return null
}

fn tag(n: i64) -> Tag? {
    if n > 0 {
        return Tag { word: "b" }
    }
    return null
}

fn main() {
    let name = find(1)
    let none = find(0)
    let fallback = Tag { word: "none" }
    let t = tag(1)
    let guest = "guest"
    println(f"hole: {name ?? guest} {none ?? guest}")
    if (name ?? "guest") == "found" {
        println("compared")
    }
    let length = (name ?? "guest").len()
    let shown = (t ?? fallback).shown()
    println(f"{length} {shown}")
    println(f"{name ?? guest} {none ?? guest}")
    if t != null {
        println("kept")
    }
}
"#;
    let rust = lowered(source);
    assert!(rust.contains(".as_deref()"), "{rust}");
    assert_eq!(
        ran("coalesce-lends-more", source),
        "hole: found guest\ncompared\n5 <b>\nfound guest\nkept\n"
    );
}

/// **`NK1206`: a `for` over a `T?`**. It was checked as a walk over the
/// list's elements and lowered as a walk over the option - once, with the
/// whole list as the binding - so `word.len()` counted the list: the program
/// printed `2` for `["xy", "z"]` and nothing was refused. A map read is a `T?`
/// too, so `for x in m[k]` is the same refusal. The way out compiles and
/// counts the letters.
#[test]
fn a_loop_over_a_nullable_is_refused() {
    let source = |iter: &str| {
        format!(
            "use std::collections\n\
             fn pick(n: i64) -> Vec[String]? {{\n\
             \x20   if n > 0 {{ return [\"xy\", \"z\"] }}\n\
             \x20   return null\n\
             }}\n\
             fn main() {{\n\
             \x20   let mut m: collections::BTreeMap[String, Vec[String]] = collections::BTreeMap()\n\
             \x20   m.insert(\"a\", [\"xyz\"])\n\
             \x20   let mut total = 0\n\
             \x20   for word in {iter} {{\n\
             \x20       total += word.len()\n\
             \x20   }}\n\
             \x20   println(f\"{{total}}\")\n\
             }}\n"
        )
    };
    for iter in ["pick(1)", "m[\"a\"]"] {
        let found = findings(&source(iter));
        let it = found
            .iter()
            .find(|f| f.code == "NK1206")
            .unwrap_or_else(|| panic!("no NK1206 for {iter}: {found:#?}"));
        assert!(it.message.contains(iter), "{it:#?}");
        assert!(
            it.help.as_deref().is_some_and(|h| h.contains("?? []")),
            "{it:#?}"
        );
    }

    assert_eq!(
        ran("loop-after-fallback", &source("pick(1) ?? []")).trim(),
        "3"
    );
    assert_eq!(
        ran("loop-over-a-copy", &source("m[\"a\"]?.clone() ?? []")).trim(),
        "3"
    );
}

/// **A name bound to a map read is the map read** (#297). `let found =
/// calls.get(key)` holds a view of the list the map keeps, as `calls[key]`
/// does: `found ?? []` is `NK1185` with the copy to write, and the copy runs.
/// And a map's text beside a name that is a view of text is a view whichever
/// side answers - `imports.get(call) ?? call` - where it reached `rustc` as an
/// `.into()` nothing could infer.
#[test]
fn a_name_bound_to_a_map_read_is_read_as_one() {
    let source = |walk: &str| {
        format!(
            "use std::collections\n\
             fn count(key: ref String, calls: ref collections::BTreeMap[String, Vec[String]]) -> i64 {{\n\
             \x20   let found = calls.get(key)\n\
             \x20   let mut n = 0\n\
             \x20   for call in {walk} {{\n\
             \x20       n += call.len()\n\
             \x20   }}\n\
             \x20   return n\n\
             }}\n\
             fn resolve(call: ref String, imports: ref collections::BTreeMap[String, String]) -> String {{\n\
             \x20   let full = imports.get(call) ?? call\n\
             \x20   return full.clone()\n\
             }}\n\
             fn main() {{\n\
             \x20   let mut m: collections::BTreeMap[String, Vec[String]] = collections::BTreeMap()\n\
             \x20   m.insert(\"a\", [\"xy\", \"z\"])\n\
             \x20   let mut i: collections::BTreeMap[String, String] = collections::BTreeMap()\n\
             \x20   i.insert(\"spawn\", \"tokio::spawn\")\n\
             \x20   let n = count(\"a\", m)\n\
             \x20   println(f\"{{n}} {{resolve(\"spawn\", i)}} {{resolve(\"x\", i)}}\")\n\
             }}\n"
        )
    };
    let found = findings(&source("found ?? []"));
    let it = found
        .iter()
        .find(|f| f.code == "NK1185")
        .unwrap_or_else(|| panic!("no NK1185: {found:#?}"));
    assert!(it.message.contains("in the map"), "{it:#?}");
    assert!(
        it.help
            .as_deref()
            .is_some_and(|h| h.contains("found?.clone()")),
        "{it:#?}"
    );
    assert_eq!(
        ran("map-read-bound", &source("found?.clone() ?? []")).trim(),
        "3 tokio::spawn x"
    );
}

/// **A value that is a word by its kind is copied, a type made of parts is
/// lent** ([ADR-279](../../../docs/specification/adr/adr-279.md) D8), where
/// `??`'s answer is only read: an enum whose variants hold nothing is copied
/// as a number is, and a struct that copies - 64 bytes of it here - is lent,
/// not copied at every read.
#[test]
fn a_word_by_its_kind_is_copied_and_a_struct_is_lent() {
    let source = r#"
enum Kind { A, B }
struct Grid { cells: Array[f64, 8] }
fn named(k: ref Kind) -> String {
    return match k {
        Kind::A => "a",
        Kind::B => "b",
    }
}
fn total(g: ref Grid) -> f64 {
    let mut sum = 0.0
    for c in g.cells {
        sum += c
    }
    return sum
}
fn kind_of(n: i64) -> Kind? {
    if n > 0 {
        return Kind::A
    }
    return null
}
fn grid_of(n: i64) -> Grid? {
    if n > 0 {
        return Grid { cells: [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0] }
    }
    return null
}
fn main() {
    let kind = kind_of(0)
    let found = grid_of(1)
    let spare = Grid { cells: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] }
    println(named(kind ?? Kind::B))
    println(f"{total(found ?? spare)}")
    println(f"{kind == null} {found == null}")
}
"#;
    let lowered = lowered(source);
    assert!(lowered.contains("found.as_ref(), || &spare"), "{lowered}");
    assert!(!lowered.contains("kind.as_ref()"), "{lowered}");
    assert_eq!(ran("word-or-parts", source), "b\n8\ntrue false\n");
}

/// **A loan on the left of `??` and a jump on the right is a view**
/// ([ADR-279](../../../docs/specification/adr/adr-279.md) D10): `let s =
/// c.signature ?? return -1` over a lent `c` binds a view of the field, for
/// text as for anything else, and the view kept past the loan is refused in
/// the language's words with `.clone()` as the way out (D2).
#[test]
fn a_loan_beside_a_jump_binds_a_view() {
    let source = r#"
struct Sig { names: Vec[String] }
struct Contract { signature: Sig?, label: String? }
fn count(c: ref Contract) -> i64 {
    let signature = c.signature ?? return -1
    return signature.names.len()
}
fn labels(cs: ref Vec[Contract]) -> i64 {
    let mut total = 0
    for c in cs {
        let label = c.label ?? continue
        total += label.len()
    }
    return total
}
fn main() {
    let a = Contract { signature: Sig { names: ["x", "y"] }, label: "ab" }
    let b = Contract { signature: null, label: null }
    println(f"{count(a)} {count(b)} {labels([a, b])}")
}
"#;
    let lowered = lowered(source);
    assert!(lowered.contains("match c.signature.as_ref()"), "{lowered}");
    assert_eq!(ran("loan-beside-a-jump", source), "2 -1 2\n");

    let kept = r#"
struct Sig { names: Vec[String] }
struct Contract { signature: Sig? }
fn kept(c: ref Contract) -> Sig {
    let signature = c.signature ?? return Sig { names: [] }
    return signature
}
fn main() {
    println(f"{kept(Contract { signature: null }).names.len()}")
}
"#;
    let found = findings(kept);
    assert!(found.iter().any(|f| f.code == "NK1104"), "{found:#?}");
    assert!(found.iter().all(|f| f.code != "NK2106"), "{found:#?}");
}

/// **`.to_string()` of text is the text itself, so it is no copy beside `??`**
/// ([ADR-282](../../../docs/specification/adr/adr-282.md) D8, Part I 2.3).
///
/// `?? "nobody".to_string()` is the literal and `?? who.to_string()` of a
/// `ref String` is the view - both join the view on the left, and both run.
/// Owned text stays owned text handed over, so `?? nobody.to_string()` of a
/// `String` name is refused exactly as `?? nobody` is.
#[test]
fn the_text_form_of_text_beside_a_view_is_the_text_itself() {
    let printed = ran(
        "to-string-beside-a-view",
        "\
struct User { name: String }

fn pick(user: ref User?, who: ref String) -> i64 {
    let name = user?.name ?? who.to_string()
    return name.len()
}

fn main() {
    let user: User? = User { name: \"Ada\" }
    let none: User? = null
    let name = user?.name ?? \"nobody\".to_string()
    let other = none?.name ?? \"nobody\".to_string()
    let who: String = \"someone\"
    println(f\"{name} {other} {pick(none, who)}\")
}
",
    );
    assert_eq!(printed.trim(), "Ada nobody 7");

    let found = findings(
        "\
struct User { name: String }

fn main() {
    let user: User? = User { name: \"Ada\" }
    let nobody: String = \"nobody\"
    let name = user?.name ?? nobody.to_string()
    println(f\"{name}\")
}
",
    );
    assert!(
        found.iter().any(|f| f.code == "NK1185"),
        "owned text stays owned text: {found:#?}"
    );

    // The text form of a number is text of its own, and is refused as such.
    let found = findings(
        "\
struct User { name: String }

fn main() {
    let user: User? = User { name: \"Ada\" }
    let n = 5
    let name = user?.name ?? n.to_string()
    println(f\"{name}\")
}
",
    );
    assert!(
        found.iter().any(|f| f.code == "NK1185"),
        "a number's text form is owned: {found:#?}"
    );
}
