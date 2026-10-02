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
    clippy::for_kv_map,
    clippy::needless_borrow,
    clippy::needless_option_as_deref
)]
pub mod tools {
    // **One package, one file** ([ADR-261](../../../docs/specification/adr/adr-261.md)):
    // every `.nika` in `src/tools` shares one namespace, as the files of a
    // package do, and `nikaia lower-std` lowers them into `package.rs`. At its
    // end stands one module per file - `tools::ty`, `tools::ledger` - naming
    // what that file offers, so a Rust caller still says which file a name is
    // in. What follows the `include!` is the Rust that Nikaia cannot say, by
    // the file it serves; `sysroot::HAND_WRITTEN` puts its names in that
    // file's module.
    include!("tools/package.rs");

    /// What a lowered `throws` of `std`'s `io` names: a tool has no prelude
    /// to bring it in.
    use crate::io;

    // --- http1.nika ---

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

    // --- ast.nika ---

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

    // --- paths.nika ---

    #[cfg(test)]
    mod paths_tests {
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

    // --- crossing.nika ---

    #[cfg(test)]
    mod crossing_tests {
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

    // --- signature.nika ---

    #[cfg(test)]
    mod signature_tests {
        use super::*;

        fn texts(all: &[&str]) -> Vec<String> {
            all.iter().map(|s| s.to_string()).collect()
        }

        /// A view is not kept, a value is; the crate's own type is named
        /// by its path and noted; anything else is `?`.
        #[test]
        fn a_rust_type_reads_as_the_ledgers() {
            let types: std::collections::BTreeSet<String> =
                ["Conn".to_string()].into_iter().collect();
            let params = texts(&["T"]);
            let mut mentioned = std::collections::BTreeSet::new();
            let mut said = |t: &str| {
                let out = translate(t, "db", &params, &types, &mut mentioned);
                (text(&out.ty), out.kept)
            };
            assert_eq!(said("&str"), ("ref String".to_string(), false));
            assert_eq!(said("Vec<Conn>"), ("Vec[db::Conn]".to_string(), true));
            assert_eq!(said("Option<T>"), ("$T?".to_string(), true));
            assert_eq!(said("u8"), ("u8".to_string(), false));
            assert_eq!(said("&mut u8"), ("?".to_string(), false));
            assert!(said("other::Thing").1);
            assert!(mentioned.contains("Conn"));
        }

        /// A `Result` throws the crate's own error by its path, and `?` for
        /// any other.
        #[test]
        fn a_result_names_what_it_throws() {
            let types: std::collections::BTreeSet<String> =
                ["Error".to_string()].into_iter().collect();
            let mut mentioned = std::collections::BTreeSet::new();
            let own = result_of(
                "Result<u8, Error>",
                "db",
                &Vec::new(),
                &types,
                &mut mentioned,
            );
            assert_eq!(own.fails.as_deref(), Some("db::Error"));
            let other = result_of(
                "Result<(), io::Error>",
                "db",
                &Vec::new(),
                &types,
                &mut mentioned,
            );
            assert_eq!(other.fails.as_deref(), Some("?"));
            let plain = result_of("u8", "db", &Vec::new(), &types, &mut mentioned);
            assert_eq!(plain.fails, None);
        }

        /// A bound reaches a parameter through its type variable or an
        /// `impl Send` at the parameter itself.
        #[test]
        fn a_send_bound_reaches_its_parameters() {
            assert_eq!(sends("T: Send + 'static, U: Clone, 'a: 'b"), texts(&["T"]));
            assert_eq!(
                sent_across(
                    &texts(&["job", "name", "f"]),
                    &texts(&["&mut T", "&str", "impl FnOnce() + Send"]),
                    "T: Send, "
                ),
                texts(&["job", "f"])
            );
        }
    }

    // --- ty.nika ---

    /// A type prints as its text, which is the one thing a `Display` of
    /// it can say. The language has no `Display` of its own to write this
    /// in, so it is the one line of Rust beside the declaration.
    impl std::fmt::Display for Ty {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&text(self))
        }
    }

    // --- rust.nika ---

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
    pub fn file(text: &str) -> Result<Vec<RustItem<'_>>, crate::grammar::ParseError> {
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
    // ([ADR-269](../../../docs/specification/adr/adr-269.md) D2): the emitter
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
