//! **A Rust file's public surface, read by a Nikaia grammar** — driven from
//! Rust, which is the whole of what
//! [ADR-290](../../../docs/specification/adr/adr-290.md) D16 decided.
//!
//! `src/tools/rust.nika` is the source; `src/tools/rust.rs` beside it is what
//! `nikaia lower-std` made of it and what this links against. There is no
//! process here and no C ABI: `nikaia_std::tools::rust::file` is a function call, and
//! what comes back is `Vec<Item>` with `&str` views into the text that was
//! handed in.
//!
//! What is asserted is mostly what is **not** found. The hand-written character
//! scanner this replaces (`crates/nikaia/src/describe.rs`) reports four
//! functions that do not exist on the fixture below, and this is where that
//! stops being true.

use nikaia_std::tools::rust::{RustItem, file};

/// A crate whose text contains items that are not items.
///
/// Every trap here is one the scanner falls into, and they are the reason the
/// fixture is shaped this way rather than being a tidy example.
const HAZARDS: &str = r##"
//! A crate whose text contains items that are not items.
//!
//! pub fn ghost(a: i32) -> i32

/* A block comment that also says
pub fn spectre(a: i32) -> i32
and means nothing by it. */

pub trait Describe {
    fn describe(&self) -> String;
}

pub fn real(a: i32, b: &str) -> i32 {
    let template = "fn main() {
pub fn phantom(a: i32) -> i32 { a }
}";
    let brace = '}';
    let quote = '"';
    let _ = (template, brace, quote);
    a + b.len() as i32
}

mod private {
    pub fn hidden(x: i32) -> i32 { x }
}

pub use private::hidden as rescued;

pub mod shown {
    pub fn seen(x: i32) -> i32 { x }
    mod deeper {
        pub fn buried() {}
    }
}

pub(crate) fn not_public(_x: i32) {}

fn republish() {}

pub struct Holder<'a, T: Clone> where T: Send {
    pub items: Vec<(T, &'a str)>,
}

pub enum Answer {
    Yes,
    No(String),
}

pub const unsafe extern "C" fn c_call(_p: *const u8) -> usize { 0 }

pub async fn later(_x: Box<dyn Fn(i32) -> i32 + Send + 'static>) -> Result<(), String> {
    Ok(())
}

impl<'a, T: Clone + Send> Holder<'a, T> {
    pub fn first(&self) -> Option<&(T, &'a str)> { self.items.first() }
    fn secret(&mut self) {}
}

struct Smuggled<T>(T);

unsafe impl<T> Send for Smuggled<T> {}
"##;

/// Every item, with the path it was found under, one per line.
fn lines(items: &[RustItem<'_>], path: &str, out: &mut Vec<String>) {
    for item in items {
        match item {
            RustItem::Fun(f) => {
                let parts = f
                    .parts
                    .iter()
                    .map(|p| match p.name {
                        "self" => p.ty.to_string(),
                        name => format!("{name}: {}", p.ty),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut line = format!("{path}{}({parts})", f.name);
                if !f.result.is_empty() {
                    line.push_str(&format!(" -> {}", f.result));
                }
                if f.pauses {
                    line.push_str(" pauses");
                }
                out.push(line);
            }
            // A `fn` that is not `pub`. Reported rather than skipped, because
            // the call graph goes through it
            // ([ADR-290](../../../docs/specification/adr/adr-290.md) D9), and
            // shown here so a test that asserts what is *not* an item can see
            // that this one is not an offer either.
            RustItem::Hidden(f) => out.push(format!("{path}{}() (private)", f.name)),
            RustItem::Rec(r) => out.push(format!("{} {path}{}", r.what, r.name)),
            RustItem::Export(text) => out.push(format!("use {text}")),
            RustItem::Used(text) => out.push(format!("(use {text})")),
            RustItem::Group(g) => {
                let shut = match g.visible {
                    true => "",
                    false => " (private)",
                };
                match g.via.is_empty() {
                    true => out.push(format!("{} {path}{}{shut}", g.what, g.name)),
                    false => out.push(format!("{} {} for {path}{}{shut}", g.what, g.via, g.name)),
                }
                lines(&g.items, &format!("{path}{}::", g.name), out);
            }
        }
    }
}

fn read(text: &str) -> Vec<String> {
    let items = file(text).expect("the fixture parses");
    let mut out = Vec::new();
    lines(&items, "", &mut out);
    out
}

/// **The four items that are not items, and the fifth that is in the wrong
/// place.**
///
/// This is the measurement [ADR-290](../../../docs/specification/adr/adr-290.md)
/// D3 rests on, written as an assertion rather than as a sentence: run
/// `nikaia describe` over the same text and it reports `spectre`, `phantom`,
/// `hidden` and `buried` as functions of the crate, and puts `seen` at the
/// crate root instead of under `shown`.
#[test]
fn nothing_that_is_not_an_item_is_reported() {
    let found = read(HAZARDS);

    // **These are not items at all** — one in a doc comment, one in a block
    // comment, one on the second line of a string literal.
    for phantom in ["ghost", "spectre", "phantom"] {
        assert!(
            !found.iter().any(|line| line.contains(phantom)),
            "`{phantom}` is not an item of this crate, and `{found:?}` says it is"
        );
    }

    // **And these are items the crate does not offer**, reported and marked
    // rather than dropped: a private `fn` is on the way to a public one, and
    // [ADR-290](../../../docs/specification/adr/adr-290.md) D9's call graph
    // goes through it. `pub(crate)` is on this list because it is not `pub`.
    for hidden in ["republish", "secret", "not_public"] {
        let line = found
            .iter()
            .find(|line| line.contains(hidden))
            .unwrap_or_else(|| panic!("`{hidden}` is an item: {found:?}"));
        assert!(
            line.ends_with("(private)"),
            "`{hidden}` is not offered, and says so: {line}"
        );
    }

    // **A private `mod` is reported and marked, not thrown away**, and what is
    // in it is reported under it. Neither `hidden` nor `buried` is reachable
    // where a program would write it, and both are findable where a `pub use`
    // would name them — which is the difference between dropping a private
    // `mod` and understanding one, and what
    // [Part III C.4](../../../docs/specification/30-nikaia-tooling.md) needs:
    // a call this crate really answers must not be refused for want of looking.
    assert!(!found.iter().any(|l| l.starts_with("hidden(")), "{found:?}");
    assert!(!found.iter().any(|l| l.starts_with("buried(")), "{found:?}");
    assert!(
        found.contains(&"mod private (private)".to_string()),
        "{found:?}"
    );
    assert!(
        found.contains(&"private::hidden(x: i32) -> i32".to_string()),
        "{found:?}"
    );
    assert!(
        found.contains(&"mod shown::deeper (private)".to_string()),
        "{found:?}"
    );
    assert!(
        found.contains(&"use private::hidden as rescued".to_string()),
        "{found:?}"
    );

    // And the one that is real is where it was written.
    assert!(
        found.contains(&"shown::seen(x: i32) -> i32".to_string()),
        "{found:?}"
    );
}

/// What a signature says, including the three things a scanner has no way to
/// produce: the module path, the receiver, and a trait method that is public
/// because its trait is.
#[test]
fn a_signature_is_read_with_its_types_and_its_path() {
    let found = read(HAZARDS);

    for expected in [
        "real(a: i32, b: &str) -> i32",
        "trait Describe",
        "Describe::describe(&self) -> String",
        "struct Holder",
        "enum Answer",
        "mod shown",
        "c_call(_p: *const u8) -> usize",
        "later(_x: Box<dyn Fn(i32) -> i32 + Send + 'static>) -> Result<(), String> pauses",
        "impl Holder<'a, T>",
        "Holder<'a, T>::first(&self) -> Option<&(T, &'a str)>",
    ] {
        assert!(
            found.contains(&expected.to_string()),
            "`{expected}` is missing from {found:?}"
        );
    }
}

/// **`unsafe impl Send for …`**, which is
/// [ADR-290](../../../docs/specification/adr/adr-290.md) D10's flag and the most
/// useful sentence a tool can write about a foreign crate. It arrives free with
/// the parser: the word is part of the item header, so reading the header reads
/// it.
#[test]
fn an_unsafe_impl_names_the_trait_it_claims() {
    let found = read(HAZARDS);
    assert!(
        found.contains(&"unsafe impl Send for Smuggled<T>".to_string()),
        "{found:?}"
    );
}

/// A character literal is not a lifetime, and getting that wrong is silent.
///
/// A rule that read `'` and then anything would swallow the `'static` and
/// everything up to the next quote - here, the rest of the file. `CHAR`
/// requires the closing quote, so a lifetime falls through to one character at
/// a time.
#[test]
fn a_lifetime_is_not_a_character_literal() {
    let found = read("pub fn a(x: &'static str) -> i32 { 0 }\npub fn b() {}");
    assert_eq!(found, vec!["a(x: &'static str) -> i32", "b()"]);
}

/// **The views are into the text that was handed in**, which is the whole
/// reason the types say `ref String` in the source: a crate is read once and
/// nothing about its signatures is copied.
#[test]
fn what_comes_back_are_views_into_the_input() {
    let text = String::from("pub fn only(a: i32) -> i32 { a }");
    let items = file(&text).expect("parses");
    let RustItem::Fun(f) = &items[0] else {
        panic!("{items:?}");
    };
    let inside = |view: &str| {
        let start = text.as_ptr() as usize;
        let at = view.as_ptr() as usize;
        at >= start && at + view.len() <= start + text.len()
    };
    assert!(inside(f.name), "the name is a copy");
    assert!(inside(f.parts[0].ty), "the type is a copy");
}
