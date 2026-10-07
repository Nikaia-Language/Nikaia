//! The compiler writes the `&` at the call
//! ([ADR-094](../../../docs/specification/adr/adr-094.md) D1 and D2 together) —
//! the third of that record's five steps, and the one the first two were built
//! for.
//!
//! Step 1 inferred the `keeps` column and let nothing read it. Step 2 made a
//! `for` lend. This step joins the two ends: a parameter the callee only
//! **reads** is declared `&T` below and given its `&` at every call, so
//! `page(entries)` is the line and `page(&entries)` is refused. **One answer in
//! two positions** — `contracts::keeps::lends`, read by the emitter when it
//! writes the declaration and by the checker when it writes the argument —
//! because the two disagreeing is a `&&T` or a moved value in the language
//! below, which is `rustc` talking about a file nobody wrote
//! ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
//!
//! **Most of this file is about the claim being withheld.** The rule is a
//! rewrite of what every existing call means, so the tests that matter are the
//! ones where it must not fire: a kept parameter, a copy type, an argument
//! that is already a view, a method's argument, a callee no ledger describes.
//! A version of this rule that lent everywhere would compile this file's first
//! test and break the language.
//!
//! **What the programs compute** with their arguments lent is
//! `tests/language/src/lent_arguments.nika`, run by `nikaia test`: that the
//! declaration and the call agree is shown by the language below accepting
//! both at once. Here stay where the `&` is written and what is refused.

use nikaia::contracts::LedgerOps;
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

/// The Rust this source lowers to.
fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Every finding the checker has about a source.
fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

/// Whether the source is refused with `NK1137`.
fn refused(source: &str) -> bool {
    findings(source).iter().any(|f| f.code == "NK1137")
}

/// **Both halves off one column.** The declaration gains its `&` and so does
/// the argument, which is the invariant the whole step rests on: either half
/// alone is a type error below.
#[test]
fn the_declaration_and_the_call_gain_the_reference_together() {
    let rust = lowered(
        "fn width(text: String) -> i64 { return text.len() as i64 }\n\
         fn main() { let t: String = \"hi\" println(f\"{width(t)}\") }\n",
    );
    // A `String` the body only reads is a `&str` below (ADR-282 D5), and
    // the caller's `&t` reaches it the way it reached a `&String`.
    assert!(rust.contains("fn width(text: &str)"), "{rust}");
    assert!(rust.contains("width(&t)"), "{rust}");
}

/// **`NK1137`: the `&` is the compiler's to write**, at a call now and not only
/// in a `for` head. Left alone it is a `&&String`, which does not fit.
#[test]
fn a_written_ampersand_at_a_lending_call_is_refused() {
    assert!(refused(
        "fn width(text: String) -> i64 { return text.len() as i64 }\n\
         fn main() { let t = \"hi\" println(f\"{width(ref t)}\") }\n"
    ));

    // And the same call without it is not refused, which is the half that says
    // the rule is about the `&` and not about the call.
    assert!(!refused(
        "fn width(text: String) -> i64 { return text.len() as i64 }\n\
         fn main() { let t: String = \"hi\" println(f\"{width(t)}\") }\n"
    ));
}

/// **A parameter the body keeps is not lent**, so the `&` a caller writes there
/// is the program's own and stays. `keeps` is a restriction and doubt adds it
/// (D2), and this is the position where that polarity is spent.
#[test]
fn a_kept_parameter_keeps_its_owned_argument() {
    let rust = lowered(
        "struct Row { name: String }\n\
         fn wrap(name: String) -> Row { return Row { name: name } }\n\
         fn main() { let n = \"a\" let r = wrap(n) println(f\"{r.name}\") }\n",
    );
    assert!(rust.contains("fn wrap(name: String)"), "{rust}");
    assert!(!rust.contains("wrap(&n)"), "{rust}");

    // And nothing is refused there, because nothing would have been written.
    assert!(!refused(
        "struct Row { name: String }\n\
         fn wrap(name: String) -> Row { return Row { name: name } }\n\
         fn main() { let n = \"a\" let r = wrap(n) println(f\"{r.name}\") }\n"
    ));
}

/// **A value that copies is not lent.** A `&i64` parameter costs a dereference
/// at every use and buys nothing, and the borrow it takes out is one a caller
/// mutating the same place would collide with — `E0502` about a file nobody
/// wrote, which is the shape `examples/n-body/src/main.nika` met in D4's step.
#[test]
fn a_copy_type_is_not_lent() {
    let rust = lowered(
        "fn twice(n: i64) -> i64 { return n * 2 }\n\
         fn main() { println(f\"{twice(21)}\") }\n",
    );
    assert!(rust.contains("fn twice(n: i64)"), "{rust}");
    assert!(!rust.contains("&i64"), "{rust}");
}

/// **A method's argument is not lent**, which is `touches`' reason for asking a
/// weaker question said once more: which entry `acc.record(m)` goes to is the
/// type checker's answer and the emitter has none
/// ([ADR-288](../../../docs/specification/adr/adr-288.md)). The declaration is
/// written off the column and the call would not be, so a call the checker does
/// not walk — a grammar action's fold lambda — would hand a value into a `&T`.
#[test]
fn a_method_argument_is_not_lent() {
    let rust = lowered(
        "struct Sink { n: i64 }\n\
         impl Sink {\n\
         \x20   fn measure(ref mut self, text: String) sync { self.n = text.len() as i64 }\n\
         }\n\
         fn main() {\n\
         \x20   let mut s = Sink { n: 0 }\n\
         \x20   let t = \"abc\"\n\
         \x20   s.measure(t)\n\
         \x20   println(f\"{s.n}\")\n\
         }\n",
    );
    assert!(rust.contains("text: String"), "{rust}");
    assert!(!rust.contains("text: &String"), "{rust}");
}

/// **And a `&` written at a method call is left alone**, for the same reason:
/// the compiler is not writing one there, so taking the program's own away
/// would leave the line with no way to say what it means.
#[test]
fn a_written_ampersand_at_a_method_call_is_left_alone() {
    assert!(!refused(
        "struct Sink { n: i64 }\n\
         impl Sink {\n\
         \x20   fn measure(ref mut self, text: ref String) sync { self.n = text.len() as i64 }\n\
         }\n\
         fn main() {\n\
         \x20   let mut s = Sink { n: 0 }\n\
         \x20   let t = \"abc\"\n\
         \x20   s.measure(ref t)\n\
         }\n"
    ));
}

/// **A `&` in front of an argument whose type this compiler could not work out
/// is the program's own.** `fs::write(path: ?, fs::Root::Anywhere)` is the absence of a claim
/// ([ADR-024](../../../docs/specification/adr/adr-024.md) D1), and refusing the
/// `&` there would take away the only way to say what the line means before an
/// inference that could answer exists — [Part III
/// C.4](../../../docs/specification/30-nikaia-tooling.md), a correct program
/// refused.
#[test]
fn an_argument_no_signature_describes_keeps_its_written_ampersand() {
    assert!(!refused(
        "use std::fs\n\nfn main() {\n\
         \x20   let out = \"/tmp/x\"\n\
         \x20   let text = \"hi\"\n\
         \x20   fs::write(ref out, fs::Root::Anywhere, ref text) catch { return }\n\
         }\n"
    ));
}

/// **A `&` in front of a genuinely wrong argument is `NK1102` and stays one.**
///
/// Saying *the `&` here is the compiler's* about an argument that is the wrong
/// value would send the reader to fix the punctuation of a line whose type is
/// wrong — so the refusal lands only where the call would be right with the
/// reference in it.
#[test]
fn a_wrong_argument_is_a_type_error_and_not_this_one() {
    let found = findings(
        "struct Request { path: String }\n\
         fn route(r: Request) -> i64 { return r.path.len() as i64 }\n\
         fn main() {\n\
         \x20   let t = \"hi\"\n\
         \x20   let n = route(ref t)\n\
         \x20   println(f\"{n}\")\n\
         }\n",
    );
    assert!(!found.iter().any(|f| f.code == "NK1137"), "{found:#?}");
    assert!(
        found.iter().any(|f| f.code == "NK1102"),
        "the wrong value is still refused: {found:#?}"
    );
}

/// **A type the checker could not pin still gets its `&` at the call**, which
/// is the invariant the two halves rest on rather than a nicety.
///
/// The *declaration* is written off `lends` alone, so any condition the checker
/// adds before recording the argument is a call whose callee takes a `&T` and
/// whose argument does not have one — `rustc` about a file nobody wrote
/// ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)). `Unknown`
/// is the one that nearly got in: it fits everything, so it reaches the
/// recording, and a guard placed one line too early took `&entries` off
/// `examples/inventory/src/main.nika`'s `render` while leaving its parameter a view.
///
/// **What `Unknown` may do is silence the refusal**, and that half is here too:
/// a written `&` over a value nothing pinned is left where it is, and it lowers
/// to the same character.
#[test]
fn an_argument_whose_type_is_not_known_is_still_lent() {
    let source = "struct Entry { count: i64 }\n\
                  \n\
                  fn read(data: ref String) -> Vec[Entry] throws {\n\
                  \x20   let mut out = Vec()\n\
                  \x20   out.push(Entry { count: data.len() as i64 })\n\
                  \x20   return out\n\
                  }\n\
                  \n\
                  fn total(entries: Vec[Entry]) -> i64 {\n\
                  \x20   let mut sum = 0\n\
                  \x20   for e in entries { sum += e.count }\n\
                  \x20   return sum\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let data = \"abcd\"\n\
                  \x20   let entries = if data.len() > 0 { read(data) catch { return } } else { Vec() }\n\
                  \x20   println(f\"{total(entries)}\")\n\
                  }\n";

    // An `if` whose branches this compiler does not bring to one type hands
    // back a value it does not name, and the declaration lends all the same -
    // so the call must too. (It was a bare `catch` until 0.0.230, when a
    // `catch` took the type of what it guards.)
    let rust = lowered(source);
    assert!(rust.contains("fn total(entries: &[Entry])"), "{rust}");
    assert!(rust.contains("total(&entries)"), "{rust}");

    // And the same program that writes its own `&` is not refused, because
    // nothing here was checked to refuse it on.
    assert!(!refused(
        &source.replace("total(entries)", "total(ref entries)")
    ));
}

/// **A parameter a method *changes* is not lent**
/// ([ADR-094](../../../docs/specification/adr/adr-094.md) D3's half of D1's
/// rule), and this is the one the corpus could not have found.
///
/// The ledger's type language spells a view `&T` and has no second spelling for
/// a mutable one, so `Vec::push` and `Vec::len` write the same receiver type.
/// That was harmless while nothing read it. The moment D1 began writing a `&`
/// off the column it stopped being harmless: `fill(out)` lent `out`, the
/// declaration became `&Vec<i64>`, and `out.push(1)` inside it came back as
/// `rustc`'s *cannot borrow `*out` as mutable* — about a file nobody wrote,
/// which is [Part III C.1](../../../docs/specification/30-nikaia-tooling.md).
///
/// No example does this, because every mutating call in `examples/` is on
/// `self` or on a local. The claim lives in its own column now, `mutates`.
/// **And the word D3 gives it is `mut`**, which is what makes the program run:
/// `&mut Vec<i64>` in the declaration and `&mut xs` at the call, both off the
/// one word. Without the column this test compiled to `&Vec<i64>`.
#[test]
fn a_parameter_a_method_changes_in_place_is_not_lent() {
    // The changing half runs in `tests/language/src/lent_arguments.nika`. Here
    // is the reading twin beside it, which is the half that says the column
    // is about *changing* the receiver and not about calling a method on one.
    let rust = lowered(
        "fn width(xs: Vec[i64]) -> i64 { return xs.len() as i64 }\n\
         fn main() { let xs = Vec() println(f\"{width(xs)}\") }\n",
    );
    // A list the body only reads is a slice below (0.0.373).
    assert!(rust.contains("fn width(xs: &[i64])"), "{rust}");
}

/// **The column survives the round trip**, which `--locked` needs: it compares
/// bytes, so a column that rendered differently than it parsed would fail a
/// build that changed nothing. And `std` carries it by hand, so it has to parse
/// out of a file this compiler did not write.
#[test]
fn the_mutates_column_renders_and_parses_back() {
    let ledger = nikaia::contracts::Ledger::infer(
        &parse_to_ast(
            "struct Stats { min: i64 }\n\
             impl Stats {\n\
             \x20   fn add(ref mut self, temp: i64) sync { self.min = temp }\n\
             \x20   fn read(ref self) -> i64 sync { return self.min }\n\
             }",
        )
        .expect("the source parses"),
    );
    assert!(ledger.functions["Stats::add"].mutates);
    assert!(
        !ledger.functions["Stats::read"].mutates,
        "`ref self` is not a claim to change anything"
    );

    // **Written the way the source writes it** (ADR-251 D4): the receiver
    // says `ref mut self`, and no key beside the signature repeats it.
    let rendered = ledger.render();
    assert!(
        rendered.contains("signature = \"(ref mut self, temp: i64)\""),
        "{rendered}"
    );
    assert!(!rendered.contains("mutates = true"), "{rendered}");
    let read = nikaia::contracts::Ledger::parse(&rendered).expect("its own output parses");
    assert!(read.functions["Stats::add"].mutates);
    assert_eq!(read.render(), rendered);

    // `std` says which of its own change their subject, and it is seven of
    // ninety-four - the same ratio `keeps` has and for the same reason: a file
    // that claimed every method might would leave the language where it was.
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    assert!(library.functions["Vec::push"].mutates);
    assert!(!library.functions["Vec::len"].mutates);
}

/// **`std`'s `push_str` takes a view**, and says so (found moving
/// `contracts::trust` into Nikaia): its ledger entry said `text: ?`, so
/// `out.push_str(name)` for a `name: String` passed the check and was
/// `rustc`'s *expected `&str`, found `String`*. **The compiler writes the
/// `&` there now** (0.0.377): a parameter declared a view is lent at a method
/// as at a free function (ADR-094 D1), and a `ref` the program writes at a
/// method call stays its own.
#[test]
fn push_str_takes_a_view_and_says_so() {
    let found = findings(
        "fn main() {\n\
         \x20   let mut out: String = \"a\"\n\
         \x20   let name = \"b\".clone()\n\
         \x20   out.push_str(name)\n\
         \x20   println(out)\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:#?}");
    let rust = lowered(
        "fn main() {\n\
         \x20   let mut out: String = \"a\"\n\
         \x20   let name = \"b\".clone()\n\
         \x20   out.push_str(name)\n\
         \x20   out.push_str(ref name)\n\
         \x20   out.push_str(ref f\"{name.len()}\")\n\
         \x20   println(out)\n\
         }\n",
    );
    assert_eq!(rust.matches("out.push_str(&name)").count(), 2, "{rust}");
    assert!(rust.contains("out.push_str(&format!("), "{rust}");
}

/// **An `else if` lends what an `if` lends**: the condition is a statement of
/// the `else` block, and the checker records the `&` it owes under that
/// statement. Written under the outer `if`'s, the reference was missing -
/// `else if !has(r.items, x)` handed a `Vec` to a slice parameter (found
/// moving `touch`'s walk into Nikaia, #125).
#[test]
fn an_else_if_lends_its_arguments_as_an_if_does() {
    let source = "\
struct R { items: Vec[i64], unknown: bool }
fn has(xs: ref Vec[i64], x: i64) -> bool {
    for one in xs {
        if one == x {
            return true
        }
    }
    return false
}
fn add(given: ref Vec[i64], mut r: R) {
    for x in given {
        if x < 0 {
            r.unknown = true
        } else if !has(r.items, x) {
            r.items.push(x)
        }
    }
}
fn main() {
    let mut r = R { items: [], unknown: false }
    add([1, 1, -2, 3], r)
    println(f\"{r.items.len()} {r.unknown}\")
}
";
    let lowered = lowered(source);
    assert!(lowered.contains("else if !has(&r.items, x)"), "{lowered}");
}
