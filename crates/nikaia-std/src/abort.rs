//! Every abort points at the Nikaia line.
//!
//! [ADR-044](../../../docs/specification/adr/adr-044.md).
//! [ADR-012](../../../docs/specification/adr/adr-012.md) decides that a
//! diagnostic names the `.nika` file the user wrote, and the compiler keeps that
//! promise everywhere it *reports* something. An abort at run time was the one
//! path where it could not: there is no compiler left to translate anything, so
//! Rust's own message named the generated file and line.
//!
//! **One table and one hook** (D1, D2). The emitter knows both ends of every line
//! it writes and keeps that knowledge in the program rather than throwing it
//! away; the hook runs on every abort path, so the translation goes in one place
//! and reaches an overflow, an index, a division by zero and a written `panic()`
//! at once. A per-case fix would be four fixes and a fifth the day a fifth abort
//! path arrives.
//!
//! **The hook stays pure** ([ADR-006](../../../docs/specification/adr/adr-006.md)
//! D6 requires it to be `sync`): a binary search in a slice reads no file and
//! takes no lock.

/// One row: a line of the generated file, and the `.nika` file and line it came
/// from.
///
/// Sorted by the generated line, because the lookup is a binary search and the
/// emitter can sort once at compile time instead of the program sorting at every
/// start.
pub type Site = (u32, &'static str, u32);
/// **`panic(message)`**: the program says it has reached a state it has no
/// answer for ([Part III A.2](../../../docs/specification/30-nikaia-tooling.md),
/// Part I 1.3).
///
/// A.2 lists *an explicit `panic()`* beside an index out of range and a
/// division by zero: an unrecoverable error, which is a bug in the program
/// rather than something the world did. It is **not** `throw` — what `throws`
/// carries is recoverable and the caller may catch it (Part I 7.1) — and it is
/// not a testing helper either, whatever `assert` turns out to be.
///
/// It hands back **never**, so it stands where a value is wanted: the right of
/// a `??` is the shape [ADR-114](../../../docs/specification/adr/adr-114.md) D1
/// writes it in, where a program that knows a key is present says so.
///
/// The message reaches the hook `report_in_nikaia_terms` installed, so what a
/// reader sees is the Nikaia line rather than this file
/// ([ADR-044](../../../docs/specification/adr/adr-044.md) D2).
#[track_caller]
pub fn panic(message: impl std::fmt::Display) -> ! {
    std::panic!("{message}")
}

/// **A false `assert`**
/// ([ADR-269](../../../docs/specification/adr/adr-269.md) D2): unrecoverable,
/// as a `panic` is, and saying what only the compiler knew - the claim as it
/// was written and the value of each operand of its comparison, by the name
/// it was written with. The hook above puts the `.nika` line in front.
///
/// ```text
/// src/tally.nika:4: the program stopped: assertion failed: an empty line is blank
///     assert(blank == 1; message: "an empty line is blank")
///     blank is 2
/// ```
#[track_caller]
pub fn assertion_failed(
    written: &str,
    message: Option<&dyn std::fmt::Display>,
    operands: &[(&str, Option<String>)],
) -> ! {
    std::panic!("{}", failed_claim(written, message, operands))
}

/// The text [`assertion_failed`] stops with.
fn failed_claim(
    written: &str,
    message: Option<&dyn std::fmt::Display>,
    operands: &[(&str, Option<String>)],
) -> String {
    let mut text = String::from("assertion failed");
    if let Some(message) = message {
        text.push_str(&format!(": {message}"));
    }
    text.push_str(&format!("\n    {written}"));
    for (name, value) in operands {
        if let Some(value) = value {
            text.push_str(&format!("\n    {name} is {value}"));
        }
    }
    text
}

/// **An operand of a claim, printed where it can be**
/// ([ADR-269](../../../docs/specification/adr/adr-269.md) D2).
///
/// The compiler writes `(&&Operand(&x)).shown()` for each operand of a false
/// `assert`'s comparison, and does not know whether `x`'s type prints: an
/// integer literal's type is settled only below. Method lookup answers it -
/// [`ShownByDebug`] is found first and exists only where the type is `Debug`;
/// [`ShownByNothing`], one reference further down, is the answer otherwise, and
/// the operand is left out of the message rather than the program refused.
pub struct Operand<'a, T: ?Sized>(pub &'a T);

/// The operand prints as the language below writes it: `2`, `"abc"`, `'x'`.
pub trait ShownByDebug {
    fn shown(self) -> Option<String>;
}

impl<T: std::fmt::Debug + ?Sized> ShownByDebug for &&Operand<'_, T> {
    fn shown(self) -> Option<String> {
        Some(format!("{:?}", self.0))
    }
}

/// The operand does not print, and the message says nothing about it.
pub trait ShownByNothing {
    fn shown(self) -> Option<String>;
}

impl<T: ?Sized> ShownByNothing for &Operand<'_, T> {
    fn shown(self) -> Option<String> {
        None
    }
}

/// Install the hook that translates an abort's location.
///
/// **What it does not do is the important half.** A location the table does not
/// know is handed to the hook that was installed before this one - which is
/// Rust's own - so a program is never worse off than it was. The table covers
/// the lines the emitter wrote *from* a Nikaia line; a panic inside `std`'s own
/// Rust, or inside a foreign crate, has no Nikaia line to name and should say so
/// in the words of whoever wrote it.
pub fn report_in_nikaia_terms(table: &'static [Site]) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let site = info.location().and_then(|at| lookup(table, at.line()));
        match site {
            Some((file, line)) => {
                eprintln!("{file}:{line}: the program stopped: {}", said(info));
            }
            None => previous(info),
        }
    }));
}

/// The `.nika` file and line for a line of the generated file.
fn lookup(table: &[Site], generated: u32) -> Option<(&'static str, u32)> {
    table
        .binary_search_by_key(&generated, |(line, _, _)| *line)
        .ok()
        .map(|at| (table[at].1, table[at].2))
}

/// What the panic said, in the two shapes a payload comes in.
///
/// `panic!("…")` with no arguments is a `&str`; one with arguments is a `String`.
/// Anything else is something no Nikaia program can produce, and "it stopped" is
/// then the honest whole of what is known.
fn said(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_string();
    }
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    "aborted".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &[Site] = &[
        (10, "src/main.nika", 3),
        (24, "src/parse.nika", 41),
        (99, "src/main.nika", 7),
    ];

    #[test]
    fn a_failed_claim_names_what_it_claimed() {
        let values = [("blank", Some("2".to_string())), ("xs", None)];
        assert_eq!(
            failed_claim("assert(blank == 1)", None, &values),
            "assertion failed\n    assert(blank == 1)\n    blank is 2"
        );
        let message: &dyn std::fmt::Display = &"an empty line is blank";
        assert_eq!(
            failed_claim("assert(ok)", Some(message), &[]),
            "assertion failed: an empty line is blank\n    assert(ok)"
        );
    }

    /// Lookup picks `Debug` where there is one, and nothing where there is
    /// not - a type with no `Debug` is left out rather than refused.
    // The `&&` is the point: it is what the emitter writes, and lookup falls
    // back one reference where the type is not `Debug`.
    #[allow(clippy::needless_borrow)]
    #[test]
    fn an_operand_prints_where_it_can() {
        struct Opaque;
        assert_eq!((&&Operand(&2)).shown(), Some("2".to_string()));
        assert_eq!(
            (&&Operand(&"a".to_string())).shown(),
            Some("\"a\"".to_string())
        );
        assert_eq!((&&Operand(&Opaque)).shown(), None);
    }

    #[test]
    fn a_known_line_is_translated() {
        assert_eq!(lookup(TABLE, 24), Some(("src/parse.nika", 41)));
        assert_eq!(lookup(TABLE, 10), Some(("src/main.nika", 3)));
    }

    /// The half that keeps a program from being worse off: a line the table does
    /// not know is not answered wrongly.
    #[test]
    fn an_unknown_line_is_not_guessed_at() {
        assert_eq!(lookup(TABLE, 11), None);
        assert_eq!(lookup(TABLE, 1), None);
        assert_eq!(lookup(TABLE, 1000), None);
        assert_eq!(lookup(&[], 10), None);
    }
}
