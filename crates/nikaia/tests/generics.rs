//! Part I 4.6's type parameters, built rather than erased
//! ([ADR-295](../../../docs/specification/adr/adr-295.md)).
//!
//! **The form parsed and did not compile.** `fn hand[T](x: T) -> T` lowered to
//! `fn hand(x: T) -> T`, because the emitter's header had no slot for a type
//! parameter — so a program that passed every stage of this compiler asked
//! `rustc` about a type nobody had declared, which is Part III C.1's class
//! exactly. These are the programs that page named.
//!
//! The lowering is compiled rather than compared as text, for the reason
//! `nullable.rs` gives: whether `<T>` lands in the right position is settled by
//! the language below, and reading the emitted string would only say that this
//! compiler agrees with itself.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

fn findings(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check(&parsed, &own, &library).findings
}

/// Lower, compile the result as a Rust library, and hand the emitted text back.
fn compiled(purpose: &str, source: &str) -> String {
    let found = findings(source);
    assert!(
        found.is_empty(),
        "{purpose} is a correct program and the checker says otherwise: {found:#?}"
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

/// D3: the parameter reaches the generated file, so `rustc` has a `T` to read.
#[test]
fn a_generic_function_compiles() {
    let rust = compiled(
        "a generic function",
        r#"
fn hand[T](x: T) -> T {
    return x
}

fn main() {
    let n: i64 = 7
    println(f"{hand(n)}")
}
"#,
    );
    assert!(
        rust.contains("fn hand<T>(x: T) -> T"),
        "the parameter is written where Rust declares one:\n{rust}"
    );
}

/// D3, the struct and the `impl` over it - `impl Stack[T]` declares the `T` it
/// applies, and the declaration and the application are two different lists.
#[test]
fn a_generic_struct_and_its_impl_compile() {
    let rust = compiled(
        "a generic struct",
        r#"
struct Pair[T] {
    first: T,
    second: T,
}

impl Pair[T] {
    fn swapped(self) -> Pair[T] {
        return Pair { first: self.second, second: self.first }
    }
}

fn main() {
    let n: i64 = 1
    let m: i64 = 2
    let p = Pair { first: n, second: m }
    println(f"{p.swapped().first}")
}
"#,
    );
    assert!(
        rust.contains("struct Pair<T>"),
        "the struct declares its parameter:\n{rust}"
    );
    assert!(
        rust.contains("impl<T> Pair<T>"),
        "the impl declares what it applies:\n{rust}"
    );
}

/// D2: **the result is the type the argument had.** Before this, a free call
/// bound nothing — ADR-288 bound from a receiver and a free function has none —
/// so `hand(n)` was `?` and every question about it was unanswerable.
#[test]
fn the_result_is_what_the_argument_was() {
    let found = findings(
        r#"
fn hand[T](x: T) -> T {
    return x
}

fn main() {
    let n: i64 = 7
    let wrong: String = hand(n)
    println(wrong)
}
"#,
    );
    assert_eq!(found.len(), 1, "one finding: {found:#?}");
    assert_eq!(found[0].code, "NK1103");
    assert!(
        found[0].message.contains("i64"),
        "it names what the argument bound: {}",
        found[0].message
    );
}

/// The same, through a generic struct's field: `Pair { first: n }` is a
/// `Pair[i64]` and nothing else says so.
#[test]
fn a_generic_structs_field_is_what_its_literal_put_in() {
    let found = findings(
        r#"
struct Pair[T] {
    first: T,
    second: T,
}

fn main() {
    let n: i64 = 7
    let p = Pair { first: n, second: n }
    let wrong: String = p.first
    println(wrong)
}
"#,
    );
    assert_eq!(found.len(), 1, "one finding: {found:#?}");
    assert_eq!(found[0].code, "NK1103");
}

/// D5, and the reason writing the parameter is worth anything: a body that uses
/// what the caller picked is refused **here**, by name, rather than by `rustc`
/// about a file nobody wrote.
#[test]
fn a_method_on_an_unbounded_parameter_is_refused() {
    let found = findings(
        r#"
fn shout[T](x: T) -> String {
    return x.to_uppercase()
}

fn main() {
    println(shout("hi"))
}
"#,
    );
    assert!(
        found.iter().any(|f| f.code == "NK1126"),
        "the parameter has no bound, so it has no methods: {found:#?}"
    );
    let refusal = found.iter().find(|f| f.code == "NK1126").expect("NK1126");
    assert!(
        refusal.message.contains("`T`") && refusal.message.contains("to_uppercase"),
        "it names the parameter and the member: {}",
        refusal.message
    );
}

/// The field half of the same rule. A field on a `T` would have gone to `rustc`
/// as `no field `count` on type `T``, which is the same message about the same
/// file nobody wrote.
#[test]
fn a_field_on_an_unbounded_parameter_is_refused() {
    let found = findings(
        r#"
fn count_of[T](x: T) -> i64 {
    return x.count
}

fn main() {
    let n: i64 = 1
    println(f"{count_of(n)}")
}
"#,
    );
    assert!(
        found.iter().any(|f| f.code == "NK1126"),
        "a field is a member too: {found:#?}"
    );
}

/// D1: inside its own body a parameter is a **type**, so a value of it does not
/// fit a slot that names something else. This is the comparison ADR-024 D4
/// erased generics to avoid making — and it is a true one, because the body did
/// not pick `T`, its caller did.
#[test]
fn a_parameter_does_not_fit_a_concrete_slot_inside_the_body() {
    let found = findings(
        r#"
fn hand[T](x: T) -> i64 {
    return x
}

fn main() {
    let n: i64 = 1
    println(f"{hand(n)}")
}
"#,
    );
    assert!(
        found.iter().any(|f| f.code == "NK1104"),
        "a `T` is not an `i64`, whatever the caller picks: {found:#?}"
    );
}

/// The other direction is what the erasure was protecting, and it still holds:
/// a **caller** is never told that `i64` is not `T`, because at a call site the
/// parameter is a variable that binds rather than a type that compares.
#[test]
fn a_caller_is_never_told_that_its_type_is_not_the_parameter() {
    for argument in ["7", "\"text\"", "true"] {
        let source = format!(
            r#"
fn hand[T](x: T) -> T {{
    return x
}}

fn main() {{
    let it = hand({argument})
    println(f"{{it}}")
}}
"#
        );
        let found = findings(&source);
        assert!(
            found.is_empty(),
            "`hand({argument})` is a correct program: {found:#?}"
        );
    }
}

/// `impl Stack[i64]` names a type in that slot rather than declaring one, and
/// the emitted `impl` has to say so — `impl<i64> Stack<i64>` is not Rust.
#[test]
fn an_impl_over_a_named_type_declares_nothing() {
    let rust = compiled(
        "an impl at one type",
        r#"
struct Holder[T] {
    it: T,
}

impl Holder[i64] {
    fn doubled(ref self) -> i64 {
        return self.it * 2
    }
}

fn main() {
    let n: i64 = 21
    let h = Holder { it: n }
    println(f"{h.doubled()}")
}
"#,
    );
    assert!(
        rust.contains("impl Holder<i64>"),
        "the slot names a type, so nothing is declared:\n{rust}"
    );
}

/// D1 **through a field**, which is where it was not held. `self` inside
/// `impl Holder[T]` used to be typed as a bare `Holder`, binding none of the
/// declaration's parameters — so `self.value`'s `T` was substituted against an
/// empty map and came out `?`, and a `?` is the one thing this checker says
/// nothing about. The member goes to `rustc` as
/// *no method named `to_uppercase` found for type parameter `T`*, about a file
/// nobody wrote ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
#[test]
fn a_member_on_the_impls_parameter_is_refused_through_a_field() {
    let found = findings(
        r#"
struct Holder[T] {
    value: T,
}

impl Holder[T] {
    fn shout(ref self) -> String {
        return self.value.to_uppercase()
    }
}

fn main() {
    let h = Holder { value: "hi" }
    println(h.shout())
}
"#,
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1126")
        .unwrap_or_else(|| panic!("NK1126, the same as for a parameter: {found:#?}"));
    assert!(
        refusal.message.contains("`T`") && refusal.message.contains("to_uppercase"),
        "it names the parameter and the member: {}",
        refusal.message
    );
}

/// The slot half, and the same sentence
/// `a_parameter_does_not_fit_a_concrete_slot_inside_the_body` gets for an
/// argument: the body did not pick `T`, so a `T` is not an `i64` here either.
#[test]
fn the_impls_parameter_does_not_fit_a_concrete_slot_through_a_field() {
    let found = findings(
        r#"
struct Holder[T] {
    value: T,
}

impl Holder[T] {
    fn count(self) -> i64 {
        return self.value
    }
}

fn main() {
    let h = Holder { value: 7 }
    println(f"{h.count()}")
}
"#,
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1104")
        .unwrap_or_else(|| panic!("NK1104: {found:#?}"));
    assert!(
        refusal.message.contains("`T`") && refusal.message.contains("i64"),
        "it names both sides: {}",
        refusal.message
    );
}

/// `NK1131` over a parameter. A `T` is not known to copy — no bound says so and
/// none can — so handing one out of a borrowed subject takes a piece out of
/// something the method does not own, exactly as a `String` does. The help is
/// worth reading for the same reason: it offers `&T`, which is a type this
/// program can write.
#[test]
fn a_field_of_the_impls_parameter_cannot_leave_a_borrowed_subject() {
    let found = findings(
        r#"
struct Holder[T] {
    value: T,
}

impl Holder[T] {
    fn get(ref self) -> T {
        return self.value
    }
}

fn main() {
    let h = Holder { value: 7 }
    println(f"{h.get()}")
}
"#,
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1131")
        .unwrap_or_else(|| panic!("NK1131: {found:#?}"));
    assert!(
        refusal.notes.iter().any(|n| n.contains("`T`")),
        "the note names the type the field has: {:#?}",
        refusal.notes
    );
    // **Both ways out, and neither names a type**
    // ([ADR-083](../../../docs/specification/adr/adr-083.md) D2). This asserted
    // `&T` — part of a way out that could not be taken, and one whose spelling a
    // generic field made worse: `&T` is not a type this program can write either.
    assert!(
        refusal
            .help
            .as_deref()
            .is_some_and(|h| h.contains("self.value.clone()")
                && h.contains("take `self` instead of `ref self`")),
        "the ways out are what a program can write: {:?}",
        refusal.help
    );
}

/// And the correct program is still correct — which is the half that matters,
/// because a check that reaches a `T` could just as easily refuse every use of
/// one. Compiled rather than merely accepted: `&T` out of a `&self` is the way
/// out the refusal above offers, and it is only advice if it builds.
#[test]
fn an_impl_over_a_parameter_still_hands_its_field_out() {
    compiled(
        "an impl over a parameter",
        r#"
struct Holder[T] {
    value: T,
}

impl Holder[T] {
    fn taken(self) -> T {
        return self.value
    }

    fn seen(ref self) -> ref T {
        return self.value
    }
}

fn main() {
    let h = Holder { value: 21 }
    println(f"{h.seen()}")
    println(f"{h.taken()}")
}
"#,
    );
}

/// `impl Holder[i64]` names a type in that slot, so inside **that** body the
/// field is an `i64` and arithmetic on it is arithmetic — which is the other
/// half of reading the head's brackets, and was `?` before for the same reason.
#[test]
fn an_impl_at_one_type_types_its_fields_as_that_type() {
    let found = findings(
        r#"
struct Holder[T] {
    it: T,
}

impl Holder[i64] {
    fn wrong(ref self) -> String {
        return self.it
    }
}

fn main() {
    let h = Holder { it: 21 }
    println(h.wrong())
}
"#,
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1104")
        .unwrap_or_else(|| panic!("NK1104, and the field is an `i64`: {found:#?}"));
    assert!(
        refusal.message.contains("i64"),
        "it names what the head put in the slot: {}",
        refusal.message
    );
}
