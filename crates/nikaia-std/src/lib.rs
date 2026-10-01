//! Nikaia's `std`, as far as Stage 0 needs it.
//!
//! A transpiler has to answer "what is `fs::map`" somewhere, and the two places
//! it could are a table inside the emitter or a crate the generated program
//! links against. This is the second, because it makes the answer readable: the
//! semantics of `std::fs` are code with tests, not string substitutions in a
//! printer (ADR-013).
//!
//! What is here is exactly what `examples/1brc.nika` reaches for, and nothing
//! is stubbed: every function does what the specification says it does, or it
//! is not here at all.
//!
//! Some of it is written in Nikaia (ADR-014 D1). The `.nika` source is the one
//! to edit; the `.rs` beside it is what the Stage 0 compiler lowered it to, is
//! committed, and is what the modules below include. **This crate has no build
//! dependency on the compiler** and must not grow one: building `std` needs
//! nothing but `rustc` (ADR-002 D4). `nikaia lower-std` regenerates the `.rs`,
//! and `crates/nikaia/tests/sysroot.rs` fails if what is committed has drifted
//! from what the compiler produces.

// **No `unsafe` here** (ADR-218): what safe Rust cannot say lives in a small
// crate of its own under `crates/unsafe/`, with its argument and its own
// checks, and this crate uses it through a safe API.
#![forbid(unsafe_code)]

// **A lowered `.nika` names this crate the way a program does**: the emitter
// writes `nikaia_std::index::get(…)`, and a module lowered into this crate
// itself (`tools/spelling.nika` was the first to index a list, 0.0.238) needs
// that path to resolve here too. Rust's own idiom for it, so the lowering
// needs no second spelling for code that lives in `std`.
extern crate self as nikaia_std;

pub mod abort;
pub mod boxed;
pub mod bytes;
pub mod channel;
pub mod cleanup;
pub mod cli;
pub mod concat;
pub mod count;
pub mod either_text;
pub mod error;
pub mod fixed;
pub mod foreign;
pub mod fs;
pub mod func;
pub mod grammar;
pub mod hash;
pub mod html;
pub mod http1;
pub mod index;
pub mod io;
pub mod list;
pub mod lock;
pub mod net;
pub mod num;
pub mod par;
pub mod process;
pub mod range;
pub mod rt;
pub mod seq;
pub mod task;
pub mod tether;
pub mod time;

/// The parser backend a generated program's grammars run on.
pub use winnow_grammar;

/// `std::text`, and the first module here that is Nikaia rather than Rust:
/// `src/text.nika`, lowered to `src/text.rs` by the Stage 0 compiler.
///
/// `include!` rather than `mod text;` so that the file keeps reading as what it
/// is - the compiler's output, committed - rather than as something written by
/// hand here.
pub mod text {
    include!("text.rs");
}

/// **Nikaia that the toolchain - or `std` itself, below its surface - uses and
/// `std` does not publish.** `http1` is the second kind: the text half of
/// `crate::http1`, which a program reaches through that module and never here.
///
/// A directory of its own, because a `.nika` beside `lib.rs` means something:
/// that `std` offers it, and that `std.contracts` has to carry every `pub`
/// thing it declares (`crates/nikaia/tests/contracts.rs`). Nothing in here is
/// offered, nothing below re-exports it, and `std.contracts` must **not** name
/// it.
///
/// It lives in this crate anyway because this is where `nikaia lower-std`
/// already looks for `.nika` sources (ADR-002 D4, ADR-195 D2), and because the
/// compiler - which is Rust - reaches what is in here as an ordinary Rust
/// module (ADR-196 D1). That is the whole interface: a call.
///
/// **What none of it is, is *refused***, and that is a defect rather than a
/// decision: `use std::tools` in a Nikaia program lowers, and `rustc` is what
/// complains, about a file nobody wrote. issue #146 carries it, and it
/// is older than this module - `use std::<anything>` has always been accepted.
///
/// **Clippy reads what the emitter wrote**, and one lint is allowed here
/// because the Rust it would ask for is not one Nikaia can say:
/// `explicit_counter_loop` (Rust 1.98) wants a counter kept beside a `for`
/// written as `(start..).zip(…)`, and a Nikaia `for` walks one sequence while a
/// `let mut` counts - which the emitter lowers as written, correctly
/// (`template.nika`'s `here`). Every other lint still holds the lowered code,
/// and has moved a `.nika` to a better line before (`ref Array` for a list only
/// read, 0.0.252).
///
/// **And `clone_on_copy`, for as long as ADR-252 D4.1 is half built.** A type
/// whose every part is a copy derives `Copy` (0.0.264), and the language does
/// not yet read its values as copies: a `.nika` that keeps one past a move
/// writes `.clone()` (`template.nika`'s `at.clone()`), as it must, and the Rust
/// below calls that a clone of a copy. When the checker reads the copy, the
/// `.clone()` is not written and this goes.
///
/// **And `manual_map`, for a `?.` on a method**
/// ([ADR-066](../../../docs/specification/adr/adr-066.md)): the reach is a
/// `match` below because a method may pause or fail, and neither an `.await`
/// nor a `?` works inside the closure `.map` takes. Where the method does
/// neither, the `match` is the same thing written longer, which is what the
/// lint says (`ty.nika`'s `result?.text()`, ADR-257).
///
/// **And eight that say how the lowering writes, not what it does**
/// (`ledger.nika`'s reader, ADR-257 step (c)): a written `return` at a
/// function's end, `Ok(f()?)` where a failure passes through unchanged, a
/// closure around a constructor for `??`'s fallback, a `match` over a
/// reach that a `?` would say shorter, and a `ref Vec[T]` parameter that
/// `Vec::contains` is described on, and `c >= 'a' && c <= 'z'`, which is how
/// the language writes a range test, a copied binding handed straight back
/// (`Ty::Named { view, .. } => view` is `let view = *view; view`), and a
/// `match` with one pattern beside an empty `else`, which is how the language
/// writes what Rust calls `if let` (`ty.nika`'s walks, ADR-257 step (d)). Each
/// is the same machine code as the shorter spelling. So is `&*value` where the
/// grammar's input is already a view (`grammar::parse`, 0.0.297): the
/// lowering writes one form for owned text, a mapping and a view alike.
#[allow(
    clippy::borrow_deref_ref,
    clippy::explicit_counter_loop,
    clippy::clone_on_copy,
    clippy::manual_map,
    clippy::needless_return,
    clippy::needless_question_mark,
    clippy::redundant_closure,
    clippy::question_mark,
    clippy::ptr_arg,
    clippy::manual_range_contains,
    clippy::let_and_return,
    clippy::single_match,
    clippy::useless_conversion,
    clippy::for_kv_map
)]
pub mod tools {
    /// **How close one word is to another**, for the compiler's *did you
    /// mean*: `src/tools/spelling.nika`, lowered to `src/tools/spelling.rs`
    /// and committed beside it. The first piece of the compiler moved from
    /// Rust to Nikaia (0.0.238); the checker and `dsl` call it as ordinary
    /// Rust functions.
    pub mod spelling {
        include!("tools/spelling.rs");
    }

    /// **The holes of a `dsl` block's body**: `src/tools/dsl.nika`, lowered
    /// to `src/tools/dsl.rs` and committed beside it. The second piece of the
    /// compiler moved from Rust to Nikaia (0.0.248); the compiler's `dsl`
    /// module calls it.
    pub mod dsl {
        include!("tools/dsl.rs");
    }

    /// **The table a `comptime` map crosses as**: `src/tools/fixed.nika`,
    /// FNV-1a and CHD, lowered to `src/tools/fixed.rs` and committed beside it
    /// ([ADR-248](../../../docs/specification/adr/adr-248.md), 0.0.250). The
    /// compiler's `fixed` module calls it while it builds a program;
    /// `crate::fixed` is the lookup a program runs, and computes the same hash.
    pub mod fixed {
        include!("tools/fixed.rs");
    }

    /// **The `html` template DSL split where it is written**:
    /// `src/tools/template.nika`, the HTML scan that decides where a hole sits
    /// and whether escaping can make it safe (ADR-017), lowered to
    /// `src/tools/template.rs` and committed beside it (ADR-250, 0.0.252). The
    /// compiler's `emit::template` calls it.
    pub mod template {
        include!("tools/template.rs");
    }

    /// **How a ledger is written down, read back**: `src/tools/ledger.nika`,
    /// the lines, tables, quoted strings and lists of `nikaia.contracts`
    /// (Part III 13.5), lowered to `src/tools/ledger.rs` and committed beside
    /// it (ADR-250, 0.0.258). The compiler's `Ledger::parse` calls it and keeps
    /// what each key means.
    pub mod ledger {
        use super::ty::*;

        include!("tools/ledger.rs");
    }

    /// **What `nikaia.toml` may say, and what each key does**:
    /// `src/tools/manifest.nika`, the `[build]` keys that exist, have moved or
    /// are withdrawn, the machines and keys of `[build.<target>]`, and the
    /// three shapes of a dependency, lowered to `src/tools/manifest.rs` and
    /// committed beside it (ADR-250). The compiler's `Manifest::parse` reads
    /// the file with the `toml` crate and asks this about every key.
    pub mod manifest {
        include!("tools/manifest.rs");
    }

    /// **What an HTTP/1.1 request head says**: `src/tools/http1.nika`, a
    /// grammar and the rules a server answers by, lowered to
    /// `src/tools/http1.rs` and committed beside it
    /// ([ADR-038](../../../docs/specification/adr/adr-038.md) D6, 0.0.248).
    /// `crate::http1` keeps the bytes and calls this for the text.
    pub mod http1 {
        include!("tools/http1.rs");

        /// **The one call a Rust caller makes into the grammar**, for
        /// `rust::file`'s reason: the generated `Http1::parse_head()` hands
        /// back a parser rather than a result, and a Rust caller has no
        /// emitter to write the four lines that turn one into the other.
        pub fn written(text: &str) -> Result<Written<'_>, crate::grammar::ParseError> {
            use winnow::Parser;
            let mut stream = winnow_grammar::ParseInput::<()> {
                state: winnow_grammar::ParseContext::<()>::default(),
                input: winnow::stream::LocatingSlice::new(text),
            };
            Http1::parse_head()
                .parse_next(&mut stream)
                .map_err(|error| crate::grammar::ParseError::of(error.render(text)))
        }
    }

    /// **The compiler's syntax tree**: `src/tools/ast.nika`, lowered to
    /// `src/tools/ast.rs` and committed beside it
    /// ([ADR-252](../../../docs/specification/adr/adr-252.md) D1). The
    /// compiler's `crate::ast` re-exports it.
    ///
    /// What stands here in Rust is what Nikaia cannot say, and it is in this
    /// crate because Rust lets an `impl` of a type stand only where the type
    /// is declared: a `Span`'s `usize` doors, which slice a source the
    /// compiler holds as a `&str`, and the grammar backend's `WithSpan`.
    pub mod ast {
        include!("tools/ast.rs");

        /// The longest source the compiler reads: a `Span` holds a byte offset
        /// in a `u32` (ADR-252 D2).
        pub const LONGEST_SOURCE: usize = u32::MAX as usize;

        /// A byte offset as a `Span` holds it. The 4 GiB refusal in
        /// `parser::parse` is what makes this total.
        pub fn offset(at: usize) -> u32 {
            u32::try_from(at).expect("a source is at most 4 GiB (ADR-252 D2)")
        }

        impl Span {
            /// The span from byte `start` to byte `end`.
            pub fn new(start: usize, end: usize) -> Span {
                Span {
                    start: offset(start),
                    end: offset(end),
                }
            }

            /// The bytes it covers, to slice the source with.
            pub fn bytes(self) -> std::ops::Range<usize> {
                self.start as usize..self.end as usize
            }

            /// The byte it starts at, which is the key the checker files its
            /// answers under (ADR-028).
            pub fn at(self) -> usize {
                self.start as usize
            }

            /// The byte after its last.
            pub fn stop(self) -> usize {
                self.end as usize
            }

            /// Whether byte `offset` is inside it.
            pub fn contains(self, offset: usize) -> bool {
                self.bytes().contains(&offset)
            }
        }

        impl From<std::ops::Range<usize>> for Span {
            fn from(range: std::ops::Range<usize>) -> Span {
                Span::new(range.start, range.end)
            }
        }

        /// Lets a rule written `-> Spanned<T> @=` wrap its value without an
        /// action.
        impl<T> winnow_grammar::WithSpan<T> for Spanned<T> {
            fn with_span(node: T, span: std::ops::Range<usize>) -> Self {
                Spanned::new(node, Span::from(span))
            }
        }
    }

    /// **What a constant integer expression comes to**:
    /// `src/tools/fold.nika`, a magnitude and a sign held to the type an
    /// operand pinned, lowered to `src/tools/fold.rs` and committed beside it
    /// ([ADR-252](../../../docs/specification/adr/adr-252.md) D6). The
    /// compiler's `fold` turns its answer into an `i128`.
    pub mod fold {
        use super::ast::*;

        include!("tools/fold.rs");
    }

    /// **What `--trust` says about a program**: `src/tools/trust.nika`, which
    /// written root is a way around the root check and what the report says,
    /// lowered to `src/tools/trust.rs` and committed beside it (ADR-250). The
    /// compiler's `contracts::trust` walks the calls and reads the ledger.
    pub mod trust {
        use super::ast::*;

        include!("tools/trust.rs");
    }

    /// **The files `nikaia describe` reads a crate from**:
    /// `src/tools/sources.nika`, every `.rs` under a crate's `src` by
    /// `fs::walk`, lowered to `src/tools/sources.rs` and committed beside it
    /// ([ADR-195](../../../docs/specification/adr/adr-195.md) D4). The
    /// compiler's `describe` drives it to its end.
    pub mod sources {
        /// What the lowered `throws` names: `std`'s own `io`, which a tool
        /// has no prelude to bring in.
        use crate::io;

        include!("tools/sources.rs");
    }

    /// **How `nikaia describe` spells a Rust crate's names**:
    /// `src/tools/paths.nika`, the module a file is and what a `pub use`
    /// offers, lowered to `src/tools/paths.rs` and committed beside it
    /// ([ADR-195](../../../docs/specification/adr/adr-195.md) D4). The
    /// compiler's `describe` keeps the table the answers fill.
    pub mod paths {
        include!("tools/paths.rs");

        #[cfg(test)]
        mod tests {
            use super::*;

            fn one(name: &str, alias: &str) -> Option<Named> {
                Some(Named {
                    name: name.into(),
                    alias: alias.into(),
                })
            }

            /// The module a file is: `lib.rs` and `mod.rs` are their
            /// directory, a binary's file is nobody's.
            #[test]
            fn a_file_is_the_module_its_path_names() {
                assert_eq!(module_of("src/lib.rs").as_deref(), Some(""));
                assert_eq!(module_of("src/a/b.rs").as_deref(), Some("a::b"));
                assert_eq!(module_of("src/a/mod.rs").as_deref(), Some("a"));
                assert_eq!(module_of("src/main.rs"), None);
                assert_eq!(module_of("src/bin/tool.rs"), None);
                assert_eq!(module_of("build.rs"), None);
            }

            /// A `pub use` in its three shapes: a group, a glob, one name.
            #[test]
            fn a_pub_use_offers_what_it_names() {
                let group = exported("m", "inner::{a, b as c}");
                assert_eq!(group.len(), 2);
                assert_eq!(group[0].prefix, "inner");
                assert_eq!(group[0].name, one("a", "a"));
                assert_eq!(group[1].name, one("b", "c"));

                let glob = exported("", "crate::inner::*");
                assert_eq!(glob[0].prefix, "crate::inner");
                assert_eq!(glob[0].name, None);

                let single = exported("", " self::x::Y as Z ");
                assert_eq!(single[0].prefix, "self::x");
                assert_eq!(single[0].name, one("Y", "Z"));

                assert_eq!(exported("", "spawn")[0].name, one("spawn", "spawn"));
            }

            /// `crate`, `self` and `super` are answered from where the `use`
            /// stands; a bare path is tried there and at the root.
            #[test]
            fn a_use_path_resolves_from_where_it_was_written() {
                let at = |prefix: &str| Export {
                    at: "a::b".into(),
                    prefix: prefix.into(),
                    name: None,
                };
                assert_eq!(bases(&at("crate::x")), ["x"]);
                assert_eq!(bases(&at("crate")), [""]);
                assert_eq!(bases(&at("self::x")), ["a::b::x"]);
                assert_eq!(bases(&at("super::x")), ["a::x"]);
                assert_eq!(bases(&at("x")), ["a::b::x", "x"]);
                assert_eq!(after("crated", "crate"), None);
            }

            /// Commas inside brackets do not split.
            #[test]
            fn only_the_outer_commas_split() {
                assert_eq!(
                    split_top_level("a: Vec<(u8, u8)>, b: [u8; 2] ,"),
                    ["a: Vec<(u8, u8)>", "b: [u8; 2]"]
                );
                assert_eq!(joined("", "x"), "x");
                assert_eq!(joined("a", ""), "a");
                assert_eq!(joined("a", "x"), "a::x");
            }
        }
    }

    /// **What `nikaia describe` says about threads**:
    /// `src/tools/crossing.nika`, whether a type's fields let it cross, whether
    /// a bound names `Send`, and the shortest way a function reaches a thread,
    /// lowered to `src/tools/crossing.rs` and committed beside it
    /// ([ADR-193](../../../docs/specification/adr/adr-193.md) D3-D5). It reads
    /// the ledger's records, as `ledger` does.
    pub mod crossing {
        use super::ty::*;

        include!("tools/crossing.rs");

        #[cfg(test)]
        mod tests {
            use super::*;

            fn texts(all: &[&str]) -> Vec<String> {
                all.iter().map(|s| s.to_string()).collect()
            }

            /// `Rc` never crosses, plain data does, and anything else - a view,
            /// an unknown type, no fields known - is silence.
            #[test]
            fn what_crosses_is_read_from_the_fields() {
                assert_eq!(crosses(&texts(&["Rc<u8>", "u8"])), Crosses::MayNot);
                assert_eq!(
                    crosses(&texts(&["Vec<Option<u8>>", "String"])),
                    Crosses::May
                );
                assert_eq!(crosses(&texts(&["Vec<u8, u16>"])), Crosses::May);
                assert_eq!(crosses(&texts(&["&str"])), Crosses::Undecided);
                assert_eq!(crosses(&texts(&["Handle"])), Crosses::Undecided);
                assert_eq!(crosses(&Vec::new()), Crosses::Undecided);
                assert_eq!(generic_of(" Option <u8>", "Option").as_deref(), Some("u8"));
                assert_eq!(generic_of("Optional<u8>", "Option"), None);
            }

            /// `Send` as a word, and not as letters inside one.
            #[test]
            fn a_bound_names_send_as_a_word() {
                assert!(bounds_send("T: Send + 'static"));
                assert!(bounds_send("Send"));
                assert!(!bounds_send("T: Sender<u8>, U: Resend"));
                assert!(!bounds_send("Sendé"));
            }

            /// The article and the commas.
            #[test]
            fn a_list_reads_as_a_sentence() {
                assert_eq!(a_list(&texts(&["a"]), "the one", "the many"), "the one `a`");
                assert_eq!(
                    a_list(&texts(&["a", "b"]), "x", "the many"),
                    "the many `a` and `b`"
                );
                assert_eq!(
                    a_list(&texts(&["a", "b", "c"]), "x", "y"),
                    "y `a`, `b` and `c`"
                );
                assert_eq!(a_list(&Vec::new(), "x", "y"), "");
            }

            /// The shortest way to a sink, through the crate's own calls, and
            /// an ambiguous short name is not followed.
            #[test]
            fn the_shortest_way_to_a_thread_is_named() {
                let mut calls = std::collections::BTreeMap::new();
                calls.insert("a::run".to_string(), texts(&["helper", "slow"]));
                calls.insert("a::helper".to_string(), texts(&["tokio::spawn"]));
                calls.insert("a::slow".to_string(), texts(&["a::slower"]));
                calls.insert("a::slower".to_string(), texts(&["std::thread::spawn"]));
                calls.insert("a::loops".to_string(), texts(&["a::loops"]));
                assert_eq!(
                    reaches_a_thread("a::run", &calls),
                    Some(texts(&["a::helper", "tokio::spawn"]))
                );
                assert_eq!(reaches_a_thread("a::loops", &calls), None);
                calls.insert("b::helper".to_string(), Vec::new());
                assert_eq!(the_crates_own("helper", &calls), None);
            }
        }
    }

    /// **The type language the checker reasons in and the ledger records**:
    /// `src/tools/ty.nika`, `Ty` and `Shape` and the text a type is written
    /// as, lowered to `src/tools/ty.rs` and committed beside it (ADR-257 D1).
    /// The compiler's `contracts::ty` re-exports them and keeps the algorithms
    /// over them until ADR-257's step (d).
    pub mod ty {
        include!("tools/ty.rs");

        /// A type prints as its text, which is the one thing a `Display` of
        /// it can say. The language has no `Display` of its own to write this
        /// in, so it is the one line of Rust beside the declaration.
        impl std::fmt::Display for Ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&text(self))
            }
        }
    }

    /// **A Rust file's public surface**, read by a Nikaia grammar:
    /// `src/tools/rust.nika`, lowered to `src/tools/rust.rs` by the Stage 0
    /// compiler and committed beside it.
    ///
    /// This is the reading half of `nikaia describe` (ADR-195 D3). What it
    /// reads, what it deliberately does not, and how it differs from the
    /// character scanner it replaces are in the `.nika`'s own header.
    pub mod rust {
        include!("tools/rust.rs");

        /// **The one call a Rust caller makes**, and the only hand-written Rust
        /// in this module.
        ///
        /// The generated `Rust::parse_file()` hands back a parser rather than a
        /// result; what turns one into the other is four lines the emitter
        /// writes at every call site in a Nikaia program, and a Rust caller has
        /// no emitter. So they are written once, here, beside the thing they
        /// are about - which is the same argument ADR-013 makes for `std` being
        /// a crate rather than a table inside the printer.
        ///
        /// The error is [`crate::grammar::ParseError`], already rendered: a
        /// headline, the line with a caret under it, and what else was possible
        /// there.
        pub fn file(text: &str) -> Result<Vec<Item<'_>>, crate::grammar::ParseError> {
            use winnow::Parser;
            let mut stream = winnow_grammar::ParseInput::<()> {
                state: winnow_grammar::ParseContext::<()>::default(),
                input: winnow::stream::LocatingSlice::new(text),
            };
            Rust::parse_file()
                .parse_next(&mut stream)
                .map_err(|error| crate::grammar::ParseError::of(error.render(text)))
        }
    }
}

/// What a `use std::…` in a Nikaia program brings into scope.
pub mod prelude {
    pub use crate::channel;
    pub use crate::channel::{Receiver, Sender};
    // **The one shared buffer**
    // ([ADR-156](../../../docs/specification/adr/adr-156.md) D1): `Bytes` is a
    // language type, written bare, so the generated Rust has to find it
    // without a `use` the program did not write.
    // **`panic`** ([Part III A.2](../../../docs/specification/30-nikaia-tooling.md),
    // Part I 1.3's list): written bare, because it is on that list and because
    // a program that reaches a state it has no answer for should not need an
    // import to say so.
    pub use crate::abort::panic;
    // **How a false `assert` prints its operands**
    // ([ADR-245](../../../docs/specification/adr/adr-245.md) D2): the emitter
    // writes `.shown()` on each, and a method needs its trait in scope.
    pub use crate::abort::{ShownByDebug as _, ShownByNothing as _};
    pub use crate::bytes::Bytes;
    pub use crate::cli;
    pub use crate::collections;
    // **How a value goes into text both kinds flow into**
    // ([ADR-224](../../../docs/specification/adr/adr-224.md) D2): the tier
    // pass writes `value.into_either()` (and `into_either_maybe`,
    // `either_items`) into the program, and a method needs its trait in scope.
    pub use crate::either_text::{EitherItems, EitherPairs, IntoEither, IntoEitherMaybe};
    pub use crate::error::Full;
    pub use crate::seq::Join as _;
    // **The C boundary's one `std` type**
    // ([ADR-147](../../../docs/specification/adr/adr-147.md) D4): a program
    // that declares `fn getenv(name: &[u8]) -> CStr` has to be able to name
    // it, and until [ADR-154](../../../docs/specification/adr/adr-154.md)
    // decides what the prelude is, this is how a name reaches a program.
    pub use crate::foreign::CStr;
    pub use crate::fs;
    // **The socket `std` lends** (ADR-194 D1). Here for `fs`'s reason: a module
    // a program reaches through its prefix has to be in scope in the generated
    // file, and nothing a program writes says where it comes from.
    pub use crate::net;
    // **Another program, started and waited for** (ADR-243). Here for `net`'s
    // reason: a module reached through its prefix has to be in scope in the
    // generated file.
    pub use crate::process;
    // **What a parse fails with**
    // ([ADR-173](../../../docs/specification/adr/adr-173.md) D1): written bare,
    // like `Overtaken`, because a program never writes a path to it — it
    // arrives in a `catch`, and the generated file has to find it there
    // without a `use` the program did not write.
    pub use crate::fixed::Fixed;
    pub use crate::grammar::ParseError;
    pub use crate::hash::{TrustedMap, TrustedSet};
    pub use crate::html;
    // **HTTP/1.1's text half** ([ADR-194](../../../docs/specification/adr/adr-194.md)
    // D5), which the `http` package calls. **Not named `http`**: a program
    // reaches a *package* by that word, and a `std` module of the same name
    // would make `http::Response` mean two things.
    pub use crate::http1;
    pub use crate::io;
    pub use crate::list::ListExt;
    pub use crate::task;
    // **A duration and the call that waits one out**
    // ([ADR-150](../../../docs/specification/adr/adr-150.md) D1, D2). The type
    // so that `let deadline: Duration = 2.minutes()` can be written, the
    // extension because `5.seconds()` is a method call and a trait has to be
    // in scope for one, and `sleep` because Part II 12.4 writes it bare — the
    // same way `digit_value` is written bare.
    pub use crate::time;
    pub use crate::time::{Duration, DurationExt, sleep};
    // `rt` is in the prelude so that the `fn main` the emitter writes can name
    // `rt::start` without a `use` the program did not ask for. Nothing in a
    // `.nika` file reaches it: ADR-038 D3's whole point is that a program says
    // `fs::read_to_string(p)` and the runtime is invisible.
    pub use crate::rt;
    pub use crate::text::digit_value;
    pub use std::collections::HashMap;

    // **The modules a `.nika` file reaches through a prefix**
    // ([ADR-154](../../../docs/specification/adr/adr-154.md) D3, D5): `use
    // std::text` then `text::digit_value`, and the same for the rest. They are
    // here because the generated Rust writes the prefix the source wrote — the
    // **emitter's** prelude is not the program's list, and this is the half
    // that answers *what the generated file needs to compile*.
    pub use crate::foreign;
    pub use crate::text;
}

/// **What `use std::collections` reaches**
/// ([ADR-154](../../../docs/specification/adr/adr-154.md) D3).
///
/// A module of this crate and not a re-export of the language below's, because
/// one name in it is **ours**: which hash a map gets follows the provenance of
/// the program's input ([ADR-010](../../../docs/specification/adr/adr-010.md)
/// D5), so `collections::HashMap` in a trusted program is written
/// `collections::TrustedMap` and has to resolve.
pub mod collections {
    pub use crate::hash::{TrustedMap, TrustedSet};
    pub use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
}
