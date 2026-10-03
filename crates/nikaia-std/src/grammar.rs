//! What a parse fails with
//! ([ADR-296](../../../docs/specification/adr/adr-296.md) D38).
//!
//! A `dsl` entry rule's ledger entry used to write `throws = ["?"]` — *something
//! this compiler cannot name* — because a parse fails with a **rendered
//! string** and a string is not a type. It was the last `"?"` in the tree, and
//! seven of the corpus' eight `main`s carried it into their own set.
//!
//! [`ParseError`] is that name. It carries what the backend already produced,
//! so a program that prints the error prints exactly what it printed before;
//! what changes is that the set it travels in has **two named members** instead
//! of one name and one absence, and a program can tell *the file was not there*
//! from *the file was not the shape the grammar says*.

/// A parse that did not accept its input.
///
/// **It carries the message and nothing else**, which is
/// [ADR-296](../../../docs/specification/adr/adr-296.md) D39 and the smaller of
/// the two answers that were on the table. The backend's own error has an
/// offset, a list of expectations and the rule stack; what it renders out of
/// them is a headline, the line with a caret under it, and what else was
/// possible there — a whole diagnostic, and the thing a program already prints.
/// Fields a program could *read* are a second question, and nobody has asked
/// it: a handler today prints the message or falls back to a default.
///
/// The shape is `Overtaken`'s ([`crate::lock::Overtaken`]): a `std` error type
/// keyed in the ledger with no module in front, because a program never writes
/// a path to it — it arrives in a `catch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    said: String,
}

impl ParseError {
    /// The error the grammar backend rendered, as the type a program names.
    ///
    /// Called by the lowering and by nothing a program writes, which is why it
    /// takes the rendered text rather than the backend's error: the generated
    /// file is where the two meet, and `nikaia-std` does not depend on the
    /// grammar backend's error type for one constructor.
    pub fn of(said: String) -> Self {
        ParseError { said }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.said)
    }
}

impl std::error::Error for ParseError {}

/// **`G::rule(text)`, lowered**: the entry rule run over the whole of `source`,
/// and its failure rendered against it.
///
/// A function rather than the block the lowering used to write, so that the
/// call is an expression like any other: in a `match` or behind a `catch` the
/// block was a scrutinee with statements in it, which `clippy` refuses
/// (`blocks_in_conditions`). The input is read once and named here, because it
/// is needed twice - to parse, and to say *where* a failure was: an offset
/// becomes a line and a column only against the text.
pub fn parse<'a, O>(
    source: &'a str,
    mut rule: impl winnow::Parser<winnow_grammar::ParseInput<'a, ()>, O, winnow_grammar::ParseError>,
) -> Result<O, ParseError> {
    let mut stream = winnow_grammar::ParseInput::<()> {
        state: winnow_grammar::ParseContext::<()>::default(),
        input: winnow::stream::LocatingSlice::new(source),
    };
    rule.parse_next(&mut stream)
        .map_err(|error| ParseError::of(error.render(source)))
}

/// [`parse`] for a `par_fold` rule, which is cut into pieces before it is
/// parsed: `run` is the generated `parse_<rule>_pieces` applied to `source`.
pub fn parse_pieces<'a, O>(
    source: &'a str,
    run: impl FnOnce(&'a str) -> Result<O, winnow_grammar::ParseError>,
) -> Result<O, ParseError> {
    run(source).map_err(|error| ParseError::of(error.render(source)))
}
