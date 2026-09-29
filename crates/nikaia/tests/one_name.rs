//! A name denotes one thing
//! ([ADR-144](../../../docs/specification/adr/adr-144.md)).
//!
//! Across files it already was refused
//! ([ADR-047](../../../docs/specification/adr/adr-047.md) D1): the files of a
//! package share one namespace, so two `Row`s in it is an error rather than a
//! rule about which of them a line means. Inside **one** file nothing said it,
//! unless the build happened to go through a manifest — and `nikaia --input`
//! skips the module layer, which is the path the corpus, the specification's
//! blocks and a reader's first program all take.
//!
//! **Found by building [ADR-140](../../../docs/specification/adr/adr-140.md)
//! D1**, from the case that matters most: a name that is both a type and a
//! function resolves to the type, so `Foo(n: 1)` is `NK1146` and the function is
//! uncallable through the only spelling
//! [ADR-133](../../../docs/specification/adr/adr-133.md) D1 gives it — with a
//! help that sends the reader to a line building the struct.

use nikaia::contracts::{Ledger, STD};
use nikaia::parser::parse_to_ast;

fn refusals(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.code == "NK1148")
        .collect()
}

/// **The entry's own reproduction** (D1), and the case
/// [ADR-140](../../../docs/specification/adr/adr-140.md) D1's build had picked
/// in silence.
#[test]
fn a_type_and_a_function_of_one_name_are_refused() {
    let found = refusals("struct Foo { n: i64 }\nfn Foo(n: i64) -> i64 { return n }\n");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].message.contains("as a `struct` and as a `fn`"),
        "the message names both kinds: {}",
        found[0].message
    );
    // **The caret is on the second**, because that is the one that arrived and
    // the one to move. The `fn` starts at byte 22, after the `struct` line.
    assert!(found[0].span.at() >= 22, "{:?}", found[0].span);
}

/// **Two of a kind get the same sentence**, because the reader is doing the same
/// thing either way: finding out which of two declarations a line means.
#[test]
fn two_of_one_kind_say_so_once() {
    let found = refusals("struct Foo { n: i64 }\nstruct Foo { m: i64 }\n");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].message.contains("both as a `struct`"),
        "{}",
        found[0].message
    );
}

/// **A `trait` and a `grammar` declare a name** (D2), and neither was counted
/// before — through *any* path, including a build with a manifest.
#[test]
fn a_trait_and_a_grammar_declare_a_name_too() {
    let with_trait = refusals("trait Foo { fn s(ref self) -> i64 }\nstruct Foo { n: i64 }\n");
    assert_eq!(with_trait.len(), 1, "{with_trait:#?}");
    assert!(
        with_trait[0]
            .message
            .contains("as a `trait` and as a `struct`"),
        "{}",
        with_trait[0].message
    );

    let with_grammar = refusals(
        "struct Nums { n: i64 }\n\
         grammar Nums { pub rule number -> i64 = d:dec[i64](digit+) { d } }\n",
    );
    assert_eq!(with_grammar.len(), 1, "{with_grammar:#?}");
    assert!(
        with_grammar[0]
            .message
            .contains("as a `struct` and as a `grammar`"),
        "{}",
        with_grammar[0].message
    );
}

/// **A method belongs to its type**, so two types may each have a `len` (D2).
/// This is the half that matters: a rule that counted methods would refuse every
/// program in the corpus.
#[test]
fn a_method_is_not_a_declaration() {
    let found = refusals(
        "struct A { n: i64 }\n\
         impl A { fn len(ref self) -> i64 { return self.n } }\n\
         struct B { n: i64 }\n\
         impl B { fn len(ref self) -> i64 { return self.n } }\n",
    );
    assert!(found.is_empty(), "{found:#?}");
}

/// **A rule belongs to its grammar** (D2): two grammars may each have a
/// `number`, and it is reached as `Nums::number`
/// ([ADR-140](../../../docs/specification/adr/adr-140.md) D3).
#[test]
fn a_rule_is_not_a_declaration() {
    let found = refusals(
        "grammar A { pub rule number -> i64 = d:dec[i64](digit+) { d } }\n\
         grammar B { pub rule number -> i64 = d:dec[i64](digit+) { d } }\n",
    );
    assert!(found.is_empty(), "{found:#?}");
}

/// **And an ordinary program says nothing**, which is the assertion a rule like
/// this one has to carry: the whole corpus goes through this walk.
#[test]
fn a_program_that_declares_each_name_once_is_untouched() {
    let found = refusals(
        "struct Reading { name: String, temp: i64 }\n\
         enum Op { Plus, Times }\n\
         trait Summary { fn s(ref self) -> i64 }\n\
         grammar Nums { pub rule number -> i64 = d:dec[i64](digit+) { d } }\n\
         fn read(text: ref String) -> i64 { return Nums::number(text) catch { 0 } }\n\
         fn main() { println(f\"{read(\\\"7\\\")}\") }\n",
    );
    assert!(found.is_empty(), "{found:#?}");
}
