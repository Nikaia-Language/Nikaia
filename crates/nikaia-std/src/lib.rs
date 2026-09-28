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
/// complains, about a file nobody wrote. `open-work.md` §1.7 carries it, and it
/// is older than this module - `use std::<anything>` has always been accepted.
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
    /// compiler moved from Rust to Nikaia (0.0.247); the compiler's `dsl`
    /// module calls it.
    pub mod dsl {
        include!("tools/dsl.rs");
    }

    /// **What an HTTP/1.1 request head says**: `src/tools/http1.nika`, a
    /// grammar and the rules a server answers by, lowered to
    /// `src/tools/http1.rs` and committed beside it
    /// ([ADR-038](../../../docs/specification/adr/adr-038.md) D6, 0.0.247).
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
