// crates/nikaia/src/parser/mod.rs
use crate::ast;
use anyhow::Result;
use winnow::Parser;
use winnow::stream::{LocatingSlice, Location};
use winnow_grammar::error::ParseError;
use winnow_grammar::{InternerContext, ParseContext, ParseInput, StateOf, Symbol, grammar};

/// What the whitespace skip has seen: where it ended, whether it held a line
/// break, and the run of `///` lines in it.
///
/// **The grammar's one piece of state, and both things in it are there for the
/// same reason.** The generator skips the implicit whitespace *before* an
/// alternative is tried, so by the time a rule runs, what stood in front of it
/// is consumed and gone — and a PEG only looks ahead. A `[` at the start of a
/// line is a list literal and not an index
/// ([ADR-135](../../../docs/specification/adr/adr-135.md) D3); a run of `///`
/// before an item is that item's documentation
/// ([ADR-139](../../../docs/specification/adr/adr-139.md) D1). Neither
/// question can be asked of the input that is left.
///
/// **Idempotent, which is what backtracking asks of state**: what whitespace
/// stands at a position is a property of the input, so a branch that reads it
/// and then loses has written the same answer a branch that wins would. The
/// doc buffer keeps that true by position — a comment that starts before where
/// the run already ended is one this has seen, and appending it again is the
/// one thing that would make re-running it matter.
#[derive(Clone, Debug, Default)]
pub struct Trivia {
    /// Where the run ended. Compared against a token's start, so trivia that
    /// ended somewhere else is trivia this token does not follow.
    end: usize,
    /// Whether it held a `\n`.
    newline: bool,
    /// The run of `///` lines seen most recently, joined by newlines and with
    /// the slashes and one leading space taken off.
    doc: String,
    /// Where that run ended, which is what makes appending to it idempotent.
    doc_end: usize,
    /// How far the unbroken trivia after that run reaches — the position the
    /// prose is *about*.
    ///
    /// **A position and not a flag**, and that is the whole of what makes this
    /// survive backtracking. `item` tries nine alternatives and eight of them
    /// fail, but a failed alternative has still consumed tokens and still run
    /// the whitespace skip after them — so a flag saying *the run still
    /// reaches here* is false by the time the arm that matches asks. A
    /// position is a fact about the input, and a later skip somewhere else
    /// neither extends it nor spoils it.
    doc_reaches: usize,
    /// Whether a run has been seen at all. Nothing clears it: a run that no
    /// longer reaches the position being asked about is ruled out by
    /// `doc_reaches`, which is a fact about the input rather than a flag a
    /// losing branch can spoil.
    doc_live: bool,
}

impl Trivia {
    /// Record a run of trivia.
    ///
    /// A comment that a line break ran straight into **carries** the flag, so
    /// `\n/* c */[1]` is a line that begins with a `[` exactly as `\n[1]` is.
    ///
    /// **And a doc run reaches as far as the trivia after it is unbroken**,
    /// which is what says which item the prose is about: a gap means a token
    /// was consumed in between, and a sentence on the other side of one
    /// belongs to nothing here.
    fn ran(&mut self, at: std::ops::Range<usize>, newline: bool) {
        let carried = self.end == at.start && self.newline;
        self.end = at.end;
        self.newline = newline || carried;
        if self.doc_reaches == at.start {
            self.doc_reaches = at.end;
        }
    }

    /// One `//…` comment, which is a doc comment when a third slash follows
    /// ([ADR-139](../../../docs/specification/adr/adr-139.md) D1).
    ///
    /// `body` is what stands after the two slashes, so a doc comment's body
    /// begins with the third. `////` is an ordinary comment, as it is in every
    /// language that has both.
    fn comment(&mut self, at: std::ops::Range<usize>, body: &str) {
        let line = match body.strip_prefix('/') {
            Some(rest) if !rest.starts_with('/') => rest,
            // **An ordinary comment neither adds to a run nor ends one.** It is
            // trivia, so a token has not been consumed and the prose is still
            // about the item below — and the shape it allows is the one this
            // repository is written in: a sentence for whoever reaches the
            // item, then a note for whoever reads the source. What ends a run
            // is *code*, because then `doc_reaches` stops at the token.
            _ => {
                self.ran(at, false);
                return;
            }
        };
        // Seen already — a `WS` is run speculatively at the same position many
        // times, and appending twice is the only way that could matter.
        if at.start < self.doc_end {
            self.ran(at, false);
            return;
        }
        // A run continues where the trivia after the last line runs straight
        // into this one; anywhere else it is a new run.
        if !self.doc_live || self.doc_reaches != at.start {
            self.doc.clear();
        }
        if !self.doc.is_empty() {
            self.doc.push('\n');
        }
        self.doc.push_str(line.strip_prefix(' ').unwrap_or(line));
        self.doc_end = at.end;
        self.doc_reaches = at.end;
        self.doc_live = true;
        self.ran(at, false);
    }

    /// The documentation standing immediately before the item at this byte.
    ///
    /// **Read and not taken**, which is what backtracking asks for: `item`
    /// tries nine alternatives and eight of them fail, so a read that emptied
    /// the buffer would hand the prose to whichever arm failed first. Nothing
    /// has to be cleared, because the run expires on its own — the next item
    /// has a token between it and this prose, so `ran` finds the trivia broken
    /// and `doc_live` is false by the time it asks.
    fn doc_at(&self, at: usize) -> Option<String> {
        match self.doc_live && !self.doc.is_empty() && self.doc_reaches == at {
            true => Some(self.doc.clone()),
            false => None,
        }
    }

    /// Whether the token at this byte is the first thing on its line.
    fn began_a_line(&self, at: usize) -> bool {
        self.end == at && self.newline
    }
}

/// The documentation standing in front of the item about to be parsed
/// ([ADR-139](../../../docs/specification/adr/adr-139.md) D1).
///
/// Written by hand and called **first in the rule** rather than read in its
/// action, because the two are different moments: by the time an item's action
/// runs, the whitespace skip has been over the item's whole body and what it
/// recorded is the trivia inside it. This consumes nothing and asks at the one
/// position where the question has an answer.
fn doc_here<'a, S>(i: &mut ParseInput<'a, S>) -> Result<Option<String>, ParseError>
where
    S: Clone + std::fmt::Debug + StateOf<Trivia>,
{
    let at = i.current_token_start();
    Ok(StateOf::<Trivia>::state(&mut i.state.user_state).doc_at(at))
}

/// A token that is **not** the first thing on its line
/// ([ADR-135](../../../docs/specification/adr/adr-135.md) D3).
///
/// Written by hand because the question is about what is *behind* the position
/// and a PEG only looks ahead: the grammar records the whitespace it skips
/// ([`Trivia`]) and this reads it back. It consumes nothing.
fn same_line<'a, S>(i: &mut ParseInput<'a, S>) -> Result<(), ParseError>
where
    S: Clone + std::fmt::Debug + StateOf<Trivia>,
{
    let at = i.current_token_start();
    match StateOf::<Trivia>::state(&mut i.state.user_state).began_a_line(at) {
        true => Err(ParseError::from_stream(i)),
        false => Ok(()),
    }
}

/// Part I 2.2's number, in the four spellings
/// ([ADR-136](../../../docs/specification/adr/adr-136.md) D1): `255`,
/// `1_000_000`, `0xFF`, `0b1010` and `0o17`.
///
/// **Written by hand for two reasons.** The grammar's character classes are
/// `digit` and `any`, so *a binary digit* and *a hexadecimal digit* have no
/// spelling in it; and a number that does not fit has to be **refused**, which
/// an action cannot do — it used to `unwrap` a failed `parse`, so
/// `99999999999999999999` took this compiler down rather than the program.
///
/// **The separator is not in the value** (D3): what comes back is the number,
/// and `1_000` is `1000` to everything downstream including a diagnostic's
/// text. **The radix is a spelling** (D2): `0xFF` is `255` and takes the first
/// type that holds it, exactly as `255` does
/// ([ADR-060](../../../docs/specification/adr/adr-060.md)).
fn number_lit<'a, S>(i: &mut ParseInput<'a, S>) -> Result<i128, ParseError>
where
    S: Clone + std::fmt::Debug,
{
    number_at(i, false)
}

/// **A number with its `-` in front of it**, where the sign is written directly
/// against the digits.
///
/// One number in the range has no other spelling: `-9223372036854775808` is
/// `i64::MIN`, and as a **negation of a positive literal** its digits are
/// `9223372036854775808`, which no `i64` holds — so the parser refused a number
/// that is in the type. Every other number in the range was writable, which is
/// what made this a completeness item rather than a blocker.
///
/// **Only where it sits directly in front of one**, which is the whole of the
/// rule: the `-` and the first digit are adjacent bytes here, because the
/// generator has already skipped the trivia in front of this rule and skips none
/// inside it. `- 5` and `-x` are the unary operator they always were, and
/// `a - 5` never reaches this at all — a binary operator is matched by the rule
/// that wrote it, not by the operand's.
fn negative_number_lit<'a, S>(i: &mut ParseInput<'a, S>) -> Result<i128, ParseError>
where
    S: Clone + std::fmt::Debug,
{
    number_at(i, true)
}

/// The two above, which differ in one byte at the front.
fn number_at<'a, S>(i: &mut ParseInput<'a, S>, signed: bool) -> Result<i128, ParseError>
where
    S: Clone + std::fmt::Debug,
{
    let bytes = winnow::stream::AsBStr::as_bstr(i);
    // **The sign is part of the literal or this rule does not apply**, and the
    // second digit-check is what keeps `-x` and `- 5` out: a `-` with no digit
    // against it is the unary operator.
    let bytes = match signed {
        true => match bytes {
            [b'-', rest @ ..] if rest.first().is_some_and(|c| c.is_ascii_digit()) => rest,
            _ => return Err(ParseError::from_stream(i).add_expected("digits")),
        },
        false => bytes,
    };
    // **Lower-case, and only at the front** (D1). `0X` and `0B` are not second
    // spellings: one form per thing is the rule this language keeps.
    let (radix, prefix) = match bytes {
        [b'0', b'x', ..] => (16u32, 2),
        [b'0', b'b', ..] => (2u32, 2),
        [b'0', b'o', ..] => (8u32, 2),
        _ => (10u32, 0),
    };
    let mut value = String::new();
    let mut at = prefix;
    let mut after_separator = false;
    let mut wrong = None;
    while at < bytes.len() {
        let c = bytes[at] as char;
        if c.is_digit(radix) {
            value.push(c);
            after_separator = false;
            at += 1;
            continue;
        }
        // **An underscore stands between digits and nowhere else** (D1): not
        // beside the prefix, not doubled, not last.
        if c == '_' {
            if value.is_empty() || after_separator {
                wrong = Some(
                    "an underscore in a number stands between digits: `1_000_000`, \
                     never `1__0` and never beside the radix prefix (Part I, 2.2)",
                );
                break;
            }
            after_separator = true;
            at += 1;
            continue;
        }
        break;
    }
    if wrong.is_none() {
        if value.is_empty() {
            // A prefix with no digits is a refusal, because `0x` is nothing
            // else; no digits at all is simply not a number here, and the
            // alternative that wanted one says so in the ordinary way.
            if prefix == 0 {
                return Err(ParseError::from_stream(i).add_expected("digits"));
            }
            wrong = Some("a radix prefix takes at least one digit (Part I, 2.2)");
        } else if after_separator {
            wrong = Some(
                "an underscore in a number stands between digits, so a number does not \
                 end in one (Part I, 2.2)",
            );
        } else if prefix > 0 && at < bytes.len() && (bytes[at] as char).is_ascii_alphanumeric() {
            // **A digit the radix does not have** — `0b1210`, `0o19` — is the
            // misparse this record is about, one prefix along: without this the
            // number ends at the bad digit and what follows is a second number
            // nobody wrote.
            wrong = Some(match radix {
                2 => "`0b` takes the digits `0` and `1` (Part I, 2.2)",
                8 => "`0o` takes the digits `0` to `7` (Part I, 2.2)",
                _ => "`0x` takes the digits `0` to `9` and `a` to `f` (Part I, 2.2)",
            });
        }
    }
    // **A float keeps the sign it always had.** `-1.16e+00` and `-1..5` both
    // begin with digits and are not integers, and the rules that read them are
    // tried after this one — so a signed match here would take the `-1` and
    // leave the rest stranded, which is what `examples/n-body/src/main.nika` said the
    // first time this ran. Refusing hands the `-` back to the unary operator
    // and the number to whichever rule it belongs to.
    if signed && radix == 10 && matches!(bytes.get(at), Some(b'.' | b'e' | b'E')) {
        return Err(ParseError::from_stream(i).add_expected("digits"));
    }
    // **The sign goes into the text the radix parser reads**, rather than being
    // applied afterwards: `-9223372036854775808` parses and `-(9223372036854775808)`
    // does not, which is the one number this rule exists for.
    if signed {
        value.insert(0, '-');
    }
    let number = match wrong {
        Some(_) => 0,
        // **As wide as a `u64` holds** ([ADR-248](../../../docs/specification/adr/adr-248.md)
        // D2), and as low as an `i64` does: which of the two a number is, is
        // asked where a type stands beside it (`NK1116`).
        None => match i128::from_str_radix(&value, radix) {
            Ok(number) if number <= u64::MAX as i128 && number >= i64::MIN as i128 => number,
            _ => {
                wrong = Some(
                    "this number does not fit the widest integers this language has, \
                     which are `u64` and `i64` (Part I, 2.2)",
                );
                0
            }
        },
    };
    // **The caret goes on the character that is wrong**, and getting there
    // means walking to it: a diagnostic is ranked by how far the parse got, so
    // a refusal built at the start of the literal loses to the float rule's
    // failure further along — which is a true sentence about this parser and no
    // help at all to a reader.
    let _ = winnow::stream::Stream::next_slice(i, at + usize::from(signed));
    match wrong {
        Some(message) => Err(ParseError::from_stream(i)
            .with_message(message)
            .with_priority(winnow_grammar::error::PRIO_STRUCTURAL)),
        None => Ok(number),
    }
}

// --- Public API ---

/// A parsed program together with the interner that produced its identifiers.
///
/// Identifiers in the AST are `Symbol` handles, which are only meaningful with
/// the `InternerContext` they were interned into - so the two travel together.
///
/// Note the cost of interning: the derived `Debug` prints identifiers as
/// `Symbol(3)` rather than their text. Use [`Parsed::text`] when a name needs to
/// be read.
#[derive(Debug)]
pub struct Parsed {
    pub program: ast::Program,
    pub interner: InternerContext,
    /// What this file calls a package, to what the package is called
    /// ([ADR-046](../../../docs/specification/adr/adr-046.md) D3).
    ///
    /// **Here because every consumer holds a `Parsed` and none of them should
    /// have to know about aliasing.** An alias is a name local to one file, so
    /// resolving it is not name resolution in ADR-011 D2's sense - it is reading
    /// the file's own dictionary, and the dictionary is part of what was parsed.
    /// The alternative was the same lookup at fourteen call sites in three
    /// modules, where the fifteenth would have been the one that forgot.
    aliases: std::collections::BTreeMap<String, String>,
    /// What [`crate::text_tiers`] decided about a `String` field or result
    /// that is not text of its own below, one line each, for `--tethers`
    /// ([ADR-222](../../../docs/specification/adr/adr-222.md) D5).
    pub text_tiers: Vec<String>,
    /// **What goes into a mixed position from inside an `f"…"` hole**, by the
    /// hole's text: the shape of each value and the method it is handed over
    /// with ([ADR-229](../../../docs/specification/adr/adr-229.md) D1). A hole
    /// is parsed again by every reader, so the wrap is applied where it is
    /// parsed - [`crate::emit::literal_expressions`] - and every reader sees it.
    pub hole_wraps: std::collections::BTreeMap<String, Vec<(String, winnow_grammar::Symbol)>>,
}

impl Parsed {
    /// The same file with only the items `keep` says yes to.
    ///
    /// **The interner travels with them**, which is the whole reason this is a
    /// method rather than a struct literal somewhere: a `Symbol` is meaningful
    /// only with the context it was interned into, so a subset of the items has
    /// to keep the same one. Written for the sub-project a grammar run compiles
    /// ([`crate::grammar_run`]), which wants the types and the grammar and not
    /// the program's `fn main`.
    pub fn keeping(&self, keep: impl Fn(&ast::Item) -> bool) -> Parsed {
        Parsed {
            program: ast::Program {
                items: self
                    .program
                    .items
                    .iter()
                    .filter(|item| keep(&item.node))
                    .cloned()
                    .collect(),
            },
            interner: self.interner.clone(),
            aliases: self.aliases.clone(),
            text_tiers: self.text_tiers.clone(),
            hole_wraps: self.hole_wraps.clone(),
        }
    }

    /// **Every `&[T]` written back as the `Vec[T]` it is a view of**, for the
    /// sub-program only ([ADR-179](../../../docs/specification/adr/adr-179.md) D3).
    ///
    /// A `&[T]` is the **crossed** form: it is what the program holds once the
    /// build has handed it over. The build itself does not hold one — a parser
    /// produces a `Vec`, and a rule action writing `Section { name, settings }`
    /// with `settings: Vec<Setting>` cannot build a struct whose field is a view
    /// of a run nobody owns.
    ///
    /// So the two programs declare the same struct differently, and that is the
    /// crossing rather than a disagreement: the sub-program is the **growable**
    /// side of [ADR-079](../../../docs/specification/adr/adr-079.md) D1 and the
    /// program is the fixed one. What holds them together is that the **dump** is
    /// generated from the program's declaration and reads a run either way — so a
    /// field the sub-program owns arrives as the list the program views.
    ///
    /// Fields and variant payloads, which is where a type may stand inside a value
    /// this crosses. A parameter or a result is untouched: nothing in the
    /// sub-program calls the program's functions.
    pub fn growing(&self) -> Parsed {
        let vec = self.interner.intern_string("Vec");
        fn grown(ty: &ast::Type, vec: Symbol) -> ast::Type {
            let mut out = ty.clone();
            out.generics = out.generics.iter().map(|g| grown(g, vec)).collect();
            if out.is_slice && !out.is_mut {
                return ast::Type {
                    name: vec,
                    generics: out.generics,
                    is_view: false,
                    is_slice: false,
                    either: false,
                    ..out
                };
            }
            out
        }
        let mut items = self.program.items.clone();
        for item in &mut items {
            match &mut item.node {
                ast::Item::Struct { fields, .. } => {
                    for field in fields.iter_mut() {
                        field.ty = grown(&field.ty, vec);
                    }
                }
                ast::Item::Enum { variants, .. } => {
                    for variant in variants.iter_mut() {
                        match &mut variant.fields {
                            ast::VariantFields::Unit => {}
                            ast::VariantFields::Tuple(types) => {
                                for ty in types.iter_mut() {
                                    *ty = grown(ty, vec);
                                }
                            }
                            ast::VariantFields::Named(fields) => {
                                for field in fields.iter_mut() {
                                    field.ty = grown(&field.ty, vec);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Parsed {
            program: ast::Program { items },
            interner: self.interner.clone(),
            aliases: self.aliases.clone(),
            text_tiers: self.text_tiers.clone(),
            hole_wraps: self.hole_wraps.clone(),
        }
    }

    /// Resolve an identifier back to its text.
    pub fn text(&self, sym: Symbol) -> &str {
        self.interner.resolve(sym)
    }

    /// A qualified name with this file's aliases resolved: `h::Request` is
    /// `http::Request` where the file wrote `use http as h`.
    ///
    /// Only the **head** segment, because only a package is aliased. A name with
    /// no `::` in it, or one whose head is not an alias, comes back untouched -
    /// which is every name in a program that writes no alias, so this costs such
    /// a program one failed lookup.
    pub fn unaliased(&self, name: &str) -> String {
        if self.aliases.is_empty() {
            return name.to_string();
        }
        // A view keeps its `&`, which sits in front of the name here.
        let (amp, bare) = match name.strip_prefix('&') {
            Some(rest) => ("&", rest),
            None => ("", name),
        };
        match bare.split_once("::") {
            Some((head, rest)) => match self.aliases.get(head) {
                Some(package) => format!("{amp}{package}::{rest}"),
                None => name.to_string(),
            },
            None => name.to_string(),
        }
    }

    /// The aliases this file declares, read off its `use` items.
    fn aliases_of(
        program: &ast::Program,
        interner: &InternerContext,
    ) -> std::collections::BTreeMap<String, String> {
        let mut out = std::collections::BTreeMap::new();
        for item in &program.items {
            if let ast::Item::Import {
                path,
                alias: Some(alias),
            } = &item.node
                && let [package] = path.as_slice()
            {
                out.insert(
                    interner.resolve(*alias).to_string(),
                    interner.resolve(*package).to_string(),
                );
            }
        }
        out
    }
}

/// Parse one expression, interning into an existing table.
///
/// The holes of an interpolated string (`"{a}={s.mean()}"`) are Nikaia
/// expressions inside a literal: they are not seen by the grammar that read the
/// literal, so they are parsed here, with the program's own interner so that
/// the symbols they produce mean the same as everywhere else.
pub fn parse_expression(interner: &InternerContext, input: &str) -> Result<ast::Expr> {
    let context = ParseContext::<Trivia> {
        interner: interner.clone(),
        ..Default::default()
    };

    let mut stream = ParseInput::<Trivia> {
        state: context,
        input: LocatingSlice::new(input),
    };

    let expr = CompilerGrammar::parse_expr()
        .parse_next(&mut stream)
        .map_err(|e| crate::diagnostics::refuse(e.render(input)))?;

    if !stream.input.is_empty() {
        return Err(crate::diagnostics::refuse(format!(
            "trailing input after expression: `{}`",
            &input[input.len() - stream.input.len()..]
        )));
    }

    Ok(expr)
}

/// **The words this language keeps for itself**
/// ([ADR-051](../../../docs/specification/adr/adr-051.md)).
///
/// This is the same list the grammar's `RESERVED` rule alternates over, and it
/// is here in Rust because two readers need it outside the grammar: the note
/// [`reserved_word_note`] adds to a parse error, and the checker, which refuses
/// `self` as a *declared* name (the one word the grammar cannot exclude,
/// because `self.min` refers to it).
///
/// `rule`, `boundary`, `fold`, `par_fold` and `unchecked` are **not** here: they
/// are the grammar sublanguage's vocabulary, reserved inside a `grammar` block
/// and words a program may want everywhere else.
///
/// `crates/nikaia/tests/parser.rs` holds the two halves together by behaviour -
/// every word here is refused as a name, and the sublanguage's words are not -
/// so the list and the rule cannot drift apart in silence.
pub const RESERVED_WORDS: [&str; 38] = [
    "as", "break", "catch", "comptime", "continue", "dsl", "else", "enum", "extern", "false", "fn",
    "for", "grammar", "if", "impl", "in", "let", "match", "mut", "null", "overlap", "pub", "ref",
    "return", "select", "self", "spawn", "struct", "sync", "test", "throw", "throws", "trait",
    "true", "unsafe", "use", "while", "with",
];

/// The note a parse error gets when what it tripped over is a reserved word.
///
/// **Read off the rendered message rather than off the error**, because what a
/// reader needs is attached to what a reader sees, and the grammar backend's
/// error carries the position but not the word. The shape it looks for is the
/// backend's own *"found unexpected token `…`"*; where that is not there, or the
/// token is an ordinary name, the note is simply absent. A note that
/// disappears is the safe way for this to be wrong - it adds a sentence and
/// corrects nothing.
/// A lambda's parameter list, split into the names and the ones written `mut`
/// ([ADR-110](../../../docs/specification/adr/adr-110.md) D1).
///
/// The grammar reads them together because that is how they are written; the
/// AST holds them apart because almost no parameter is one, and a bare `Ident`
/// is what every reader of `params` already has in hand — the same arrangement
/// `contracts::Signature` uses for the same question.
fn split_mut(params: Vec<(bool, Symbol)>) -> (Vec<Symbol>, Vec<Symbol>) {
    let mutable = params
        .iter()
        .filter(|(mutable, _)| *mutable)
        .map(|(_, name)| *name)
        .collect();
    (params.into_iter().map(|(_, name)| name).collect(), mutable)
}

/// **The note a parse error gets where the program wrote the spelling `ref`
/// replaced** ([ADR-184](../../../docs/specification/adr/adr-184.md) D4).
///
/// `&` left the language at 0.0.134 and the grammar has no rule for it, so what
/// a program written before that gets is *found unexpected token `&`* — true,
/// and no help at all. A way out that cannot be taken is not one
/// ([Part III C.2](../../../docs/specification/30-nikaia-tooling.md)), and here
/// there is one and it is a single word.
///
/// **Only for a bare `&`.** `&&` is a program's *and* and is untouched, which
/// is the one place the character is still the language's.
fn the_spelling_ref_replaced_note(rendered: &str) -> String {
    const MARK: &str = "found unexpected token `";
    let Some(after) = rendered.split(MARK).nth(1) else {
        return String::new();
    };
    if after.split('`').next() != Some("&") {
        return String::new();
    }
    "\nnote: a view is written `ref T` (Part I, 6.5): `ref String` for text, \
     `ref Array[T]` for a run of elements, `ref value` for the borrow an \
     expression writes. `&` was the spelling until 0.0.134 and is not one now - \
     `&&` is still this language's *and*."
        .to_string()
}

fn reserved_word_note(rendered: &str) -> String {
    const MARK: &str = "found unexpected token `";
    let Some(after) = rendered.split(MARK).nth(1) else {
        return String::new();
    };
    let Some(token) = after.split('`').next() else {
        return String::new();
    };
    if !RESERVED_WORDS.contains(&token) {
        return String::new();
    }
    format!(
        "\nnote: `{token}` is a reserved word, so it is not a name (Part I, 2.1). \
         Either pick another name, or - if the construct was meant - it does not \
         belong in this position."
    )
}

/// **The message an unclosed `/* … */` gets, said at the `/*` that opened it**
/// ([ADR-134](../../../../docs/specification/adr/adr-134.md) D2).
///
/// `BLOCK_COMMENT`'s cut fires where the input ran out, which is the end of the
/// file and not the place the reader has to fix - a comment that swallowed the
/// rest of a program fails at its last byte, and the caret there points at
/// nothing. The grammar cannot say better: the opening may be thousands of bytes
/// behind the position it failed at.
///
/// So the opening is **found by scanning**, and the scan is the reading the lexer
/// does: a `"` opens a string until its unescaped close, a `//` runs to the end
/// of its line, and `/*` and `*/` count against each other. `None` where the
/// counts come out even, which is a `*/` the grammar wanted for some other
/// reason and whose own message is the better one.
///
/// **Only ever reached on a parse that has already failed with `*/` missing**,
/// which is what keeps the one thing this cannot read - a `/*` inside a `dsl … eod`
/// or a `grammar { … }` block, where it is text - from turning a good message into
/// a wrong one. The condition is the grammar's own: it was inside a
/// `BLOCK_COMMENT` and wanted its close.
fn unclosed_block_comment(rendered: &str, source: &str) -> Option<String> {
    if !rendered.contains("expected `*/`") {
        return None;
    }
    let at = opening_of_an_unclosed_comment(source)?;
    let (line, column) = winnow_grammar::span::line_column(source, at);
    Some(format!(
        "Parse error:\nunclosed block comment, opened at line {line}, column {column}\n{}\n\
         note: `/*` opens a comment that `*/` closes, and they nest - so a `/*` inside \
         this one needs its own `*/` before this one's (Part I, 2; ADR-134 D2)",
        winnow_grammar::span::caret(source, at, 2)
    ))
}

/// The byte offset of the outermost `/*` that never closed, if there is one.
///
/// Bytes and not characters, deliberately: everything compared here is ASCII, and
/// every byte of a multi-byte character is `>= 0x80`, so the scan cannot stop
/// inside one.
fn opening_of_an_unclosed_comment(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut open: Vec<usize> = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        // A string and a line comment are only themselves *outside* a block
        // comment: D1's *a `//` inside a block comment is comment, not a second
        // comment*, and the same of a quote.
        if open.is_empty() {
            match bytes[at] {
                b'"' => {
                    at += 1;
                    while at < bytes.len() {
                        match bytes[at] {
                            b'\\' => at += 2,
                            b'"' => {
                                at += 1;
                                break;
                            }
                            _ => at += 1,
                        }
                    }
                    continue;
                }
                b'/' if bytes.get(at + 1) == Some(&b'/') => {
                    while at < bytes.len() && bytes[at] != b'\n' {
                        at += 1;
                    }
                    continue;
                }
                _ => {}
            }
        }
        if bytes[at] == b'/' && bytes.get(at + 1) == Some(&b'*') {
            open.push(at);
            at += 2;
            continue;
        }
        if bytes[at] == b'*' && bytes.get(at + 1) == Some(&b'/') && !open.is_empty() {
            open.pop();
            at += 2;
            continue;
        }
        at += 1;
    }
    open.first().copied()
}

/// The note a parse error gets when a `??`'s fallback reached for an operator.
///
/// **A refusal in the grammar cannot always say why**, and this is the case that
/// proves it. `coalesce_fallback` takes one value, so `a ?? 0 > 3` fails — but
/// it fails *after* the fallback has succeeded on `0`, in whatever rule was
/// enclosing it, and that rule's message is about its own closing brace. The
/// caret lands on a token the reader did not type wrong.
///
/// So the note is added from what a reader **sees**: a source line that has a
/// `??` before the column the error points at, and a binary operator at it.
/// Read off the rendered message for the same reason
/// [`reserved_word_note`] is — what a reader needs is attached to what a reader
/// sees, and a note that disappears is the safe way for this to be wrong, since
/// it adds a sentence and corrects nothing
/// ([ADR-089](../../../docs/specification/adr/adr-089.md) D2).
fn coalesce_fallback_note(rendered: &str) -> String {
    const MARK: &str = "found unexpected token `";
    // **`=` is in the list and that is not a mistake.** The backend names the
    // *first* character it could not use, so a `==` is reported as `=`. A line
    // that has a `??` to the left of the caret and a stray `=` at it is this
    // shape and not an assignment typo, which is what keeps the pair of
    // conditions together rather than either alone.
    const OPERATORS: &[&str] = &[
        "==", "!=", "<=", ">=", "=", "<", ">", "&&", "|", "||", "..", "+", "-", "*", "/", "%", "as",
    ];
    let Some(after) = rendered.split(MARK).nth(1) else {
        return String::new();
    };
    let Some(token) = after.split('`').next() else {
        return String::new();
    };
    // The message shows the offending line with a caret under it. A `??` on
    // that line, to the left of the caret, is what makes this the shape rather
    // than an ordinary typo — and where the rendering is not that shape, the
    // note is simply absent.
    let mut lines = rendered.lines();
    let source = lines.find(|l| l.contains(" | "));
    let caret = rendered.lines().find(|l| l.trim_start().starts_with('^'));
    let (Some(source), Some(caret)) = (source, caret) else {
        return String::new();
    };
    let at = caret.find('^').unwrap_or(0);
    let before = &source[..source.len().min(at)];
    if !before.contains("??") || !OPERATORS.contains(&token) {
        return String::new();
    }
    // **The token is not written into the message**, and that is deliberate: the
    // backend reports the first character it could not use, so a `==` arrives
    // here as `=` and a help that quoted it back would read `(a ?? 0) = …`.
    // The shape is the same whichever operator it was, so the message shows the
    // shape.
    let _ = token;
    "\nnote: the fallback of a `??` is one value, or an expression in brackets \
     (Part I, 3.5). Without them it reaches rightwards across the operator, so \
     `a ?? 0 > 3` is `a ?? (0 > 3)` and not what the line looks like.\n\
     help: put brackets around the side you mean - `(a ?? 0) > 3`, or `a ?? (0 > 3)`."
        .to_string()
}

/// **A `let` taking apart something a flat tuple of names cannot**
/// ([ADR-098](../../../docs/specification/adr/adr-098.md)).
///
/// `let (a, b) = …` binds names by position and nothing else: a **nested**
/// tuple is written nowhere in the specification and is refused rather than
/// quietly accepted, which is this compiler's rule for a form nobody decided.
/// What a bare parse error says about it is a list of tokens, and a list of
/// tokens does not tell a reader that the *shape* is the thing that is missing.
///
/// **`_` is not part of this**, and deliberately so: it parses as a name and
/// has since long before this form existed (`let _ = f()` lowers today), so
/// refusing it here would narrow something already accepted and would say
/// nothing about the single-name spelling beside it. What it *means* - Rust's
/// wildcard rather than a binding - is on `open-work.md` as its own finding.
///
/// Recognised from the rendering rather than from the grammar, for the reason
/// [`coalesce_fallback_note`] has: the failure happens after `(` has already
/// matched, so the backend's message is about the token it stopped at and the
/// note is what supplies the sentence.
fn let_names_note(rendered: &str) -> String {
    const MARK: &str = "found unexpected token `";
    let Some(after) = rendered.split(MARK).nth(1) else {
        return String::new();
    };
    let Some(token) = after.split('`').next() else {
        return String::new();
    };
    if token != "(" {
        return String::new();
    }
    // The offending line, with a `let (` to the left of the caret, is what
    // makes this the shape rather than an ordinary typo.
    let source = rendered.lines().find(|l| l.contains(" | "));
    let caret = rendered.lines().find(|l| l.trim_start().starts_with('^'));
    let (Some(source), Some(caret)) = (source, caret) else {
        return String::new();
    };
    let at = caret.find('^').unwrap_or(0);
    let before = &source[..source.len().min(at)];
    if !before.contains("let (") && !before.contains("let mut (") {
        return String::new();
    }
    "\nnote: a `let` binds one name, or a flat tuple of them - `let (tx, rx) = …` \
     (Part I, 2.1). It is not a pattern, so a tuple inside a tuple has no \
     spelling here.\n\
     help: bind the outer parts with one name each, and read the inner ones from \
     the name that holds them."
        .to_string()
}

pub fn parse_to_ast(input: &str) -> Result<Parsed> {
    // Generated parsers run on a `Stateful` stream: `LocatingSlice` supplies the
    // spans, `ParseContext` carries the shared parser state including the
    // interner. Cloning the interner out shares it (it is an `Arc` inside), so
    // the handles in the AST stay resolvable after parsing.
    let context = ParseContext::<Trivia>::default();
    let interner = context.interner.clone();

    let mut stream = ParseInput::<Trivia> {
        state: context,
        input: LocatingSlice::new(input),
    };

    // `parse_<rule>()` is a factory: calling it builds the parser, which we then
    // drive with `.parse_next()`.
    let program = CompilerGrammar::parse_program()
        .parse_next(&mut stream)
        // A refusal and not a failure of this compiler: the program is what is
        // wrong, so it leaves without a backtrace (`diagnostics::Refused`).
        .map_err(|e| {
            let rendered = e.render(input);
            // **An unclosed `/* … */` is reported at its opening** and not at
            // the end of the file, which is the one place a parse error's
            // position is the reader's to be told rather than the parser's
            // ([ADR-134](../../../../docs/specification/adr/adr-134.md) D2).
            if let Some(rendered) = unclosed_block_comment(&rendered, input) {
                return crate::diagnostics::refuse(rendered);
            }
            let note = format!(
                "{}{}{}{}",
                reserved_word_note(&rendered),
                the_spelling_ref_replaced_note(&rendered),
                coalesce_fallback_note(&rendered),
                let_names_note(&rendered)
            );
            crate::diagnostics::refuse(format!("Parse error:\n{rendered}{note}"))
        })?;

    // The generated entry point already refuses leftover input; this is a
    // backstop so a partial parse can never be reported as a success.
    if !stream.input.is_empty() {
        return Err(crate::diagnostics::refuse(format!(
            "Parse error: unexpected trailing input at byte {}",
            input.len() - stream.input.len()
        )));
    }

    let aliases = Parsed::aliases_of(&program, &interner);
    let mut parsed = Parsed {
        program,
        interner,
        aliases,
        text_tiers: Vec::new(),
        hole_wraps: std::collections::BTreeMap::new(),
    };
    // **What a `String` field or result is below is decided here, once**
    // ([ADR-222](../../../../docs/specification/adr/adr-222.md)): by what flows
    // into it, and written into its type so every later reader agrees.
    crate::text_tiers::refine(&mut parsed);
    Ok(parsed)
}

// --- Action-block helpers ---
//
// The generated parser is a module with `use super::*`, so these are in scope
// inside the action blocks below. They exist because a PEG has no precedence
// table: the expression rules parse a head and a list of tails, and the shape
// is rebuilt here.

/// What may follow a primary expression: `.field`, `.method(..)`, `?`.
#[derive(Debug, Clone)]
pub enum Postfix {
    Field(Symbol),
    /// Part I 3.5: `?.name`, which reaches the field only if the receiver holds
    /// something.
    SafeField(Symbol),
    /// Part I 3.5 again, onto a **method**: the call happens only if the
    /// receiver holds something ([ADR-066](../../../docs/specification/adr/adr-066.md)).
    SafeMethod(Symbol, Vec<ast::Expr>, Vec<ast::ConfigArg>),
    Method(Symbol, Vec<ast::Expr>, Vec<ast::ConfigArg>),
    Index(Box<ast::Expr>),
    /// `with { x: 1 }` — [ADR-118](../../../docs/specification/adr/adr-118.md)
    /// D1. The `usize` is the byte the `with` stands at, which is the key the
    /// checker records the operand's type under.
    With(Vec<ast::FieldInit>, usize),
}

/// A declaration's parameters, with the config zone split into its two shapes.
pub fn params_of(
    receiver: Option<ast::Receiver>,
    args: Vec<ast::FnArg>,
    zone: Option<ast::ConfigZone>,
) -> ast::FnParams {
    let (config, spread) = match zone {
        Some(ast::ConfigZone::Options(options)) => (options, None),
        Some(ast::ConfigZone::Spread(name)) => (Vec::new(), Some(name)),
        None => (Vec::new(), None),
    };
    ast::FnParams {
        receiver,
        args,
        config,
        spread,
    }
}

/// Left-associative: `a - b - c` is `(a - b) - c`.
/// **Every binary node carries a span, and it is the operator's own.**
///
/// The span of the *tail* - the operator and what stands to its right - rather
/// than of the whole expression, because that is what the grammar has in hand
/// and because uniqueness is the only property a key needs: two `+`s in one
/// statement have two spans, which is exactly what a statement-keyed channel
/// could not give ([ADR-081](../../../docs/specification/adr/adr-081.md) D1).
///
/// `contracts/sync.rs` has said *"expression-level spans are open work"* in its
/// own words since it was written. This is the first of them, and it is here
/// rather than on every expression because one variant needed it and a field
/// nobody reads is a field that goes stale.
pub fn fold_binary(head: ast::Expr, tail: Vec<(ast::BinaryOp, ast::Expr, ast::Span)>) -> ast::Expr {
    tail.into_iter()
        .fold(head, |lhs, (op, rhs, span)| ast::Expr::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            span,
            grouped: false,
        })
}

/// A parenthesised expression, which is the expression - with a bit operation
/// marked as written inside them ([ADR-248](../../../docs/specification/adr/adr-248.md)
/// D4): that is the one place the tree has to remember a parenthesis.
pub fn grouped(e: ast::Expr) -> ast::Expr {
    match e {
        ast::Expr::Binary {
            op, lhs, rhs, span, ..
        } => ast::Expr::Binary {
            op,
            lhs,
            rhs,
            span,
            grouped: true,
        },
        other => other,
    }
}

pub fn fold_postfix(base: ast::Expr, tail: Vec<Postfix>) -> ast::Expr {
    tail.into_iter().fold(base, |recv, step| match step {
        Postfix::Field(name) => ast::Expr::Field {
            base: Box::new(recv),
            name,
        },
        Postfix::SafeField(name) => ast::Expr::SafeField {
            base: Box::new(recv),
            name,
        },
        Postfix::SafeMethod(method, args, config) => ast::Expr::SafeMethod {
            receiver: Box::new(recv),
            method,
            args,
            config,
        },
        Postfix::Method(method, args, config) => ast::Expr::MethodCall {
            receiver: Box::new(recv),
            method,
            args,
            config,
        },
        Postfix::Index(index) => ast::Expr::Index {
            base: Box::new(recv),
            index,
        },
        Postfix::With(fields, at) => ast::Expr::With {
            base: Box::new(recv),
            fields,
            at,
        },
    })
}

// --- Grammar Definition ---

grammar! {
    grammar CompilerGrammar {
        use crate::ast::*;
        use winnow::ascii::{digit1, multispace1};

        state Trivia;

        // Declared so the validator knows the name; the parsers themselves are
        // above ([ADR-135](../../../../docs/specification/adr/adr-135.md) D3,
        // [ADR-136](../../../../docs/specification/adr/adr-136.md) D1).
        extern rule same_line -> ();
        extern rule number_lit -> i128;
        extern rule negative_number_lit -> i128;
        extern rule doc_here -> Option<String>;

        // --- Entry Point ---
        // Rule 'program' -> generates 'parse_program'
        pub rule program -> Program =
            items:item*
            -> {
                Program { items }
            }

        // Comments are whitespace, and `WS` is the only place that can be said:
        // the generator inserts it between the tokens of every syntactic rule.
        // All three are UPPERCASE on purpose - a lowercase `comment` would be
        // syntactic, so the generator would insert `WS` between *its* tokens,
        // and `WS` calls it. That cycle recurses until the stack is gone.
        //
        // **All three record where they ended**
        // ([ADR-135](../../../../docs/specification/adr/adr-135.md) D3), which
        // is the only way a later rule can find out that a line break stood in
        // front of it: the generator skips trivia before an alternative is
        // tried, so the break is gone by the time anything could look.
        rule WSE = w:multispace1 @ at -> { _state.user().ran(at, w.contains('\n')) }
        rule WS = (WSE | COMMENT | BLOCK_COMMENT)*
        rule COMMENT @= "//" body:until(line_ending) -> { _state.user().comment(_span, body) }

        // **`/* … */`, and it nests**
        // ([ADR-134](../../../../docs/specification/adr/adr-134.md) D1, D2).
        // Whitespace to the parser exactly as a line comment is, which is why
        // it is listed in `WS` and nowhere else: the generator puts `WS`
        // between the tokens of every syntactic rule, so a block comment stands
        // wherever whitespace may - inside an argument list, inside a
        // `grammar { … }` block, across as many lines as it needs.
        //
        // `BLOCK_INNER` tries the nested comment **first**, which is the whole
        // of D2: at a `/*` the recursion takes it, so `/* a /* b */ c */` is one
        // comment ending at the second close. Only where the inner one has no
        // close does the `any` alternative take the slash, which is the case the
        // cut below is about.
        //
        // **The cut is what puts the error at the opening** (D2). Once `/*` has
        // been read this input is a block comment and nothing else, so an
        // unclosed one fails here rather than letting `WS` succeed with zero
        // repetitions and the `/*` reaching the next rule as a token nobody
        // wants. A cut reaches all the way up, which is why the message names
        // this position.
        //
        // **A `/*` inside a string literal is text** (D1), and `STRING` below
        // needs no help to say so: it is a lexical rule, so no implicit `WS`
        // runs between its characters and nothing here is ever asked.
        rule BLOCK_COMMENT # "block comment" @= "/*" => BLOCK_INNER* "*/"
            -> { _state.user().ran(_span, false) }

        // One step inside a block comment: a nested comment, or one character
        // that is not the close. **Tried in that order**, which is the whole of
        // D2 - at a `/*` the recursion takes it, so the first `*/` closes the
        // inner comment and not this one.
        //
        // It hands back a `char` it has no use for, and the reason is the
        // generator: two alternatives of different value types make it write a
        // unit conversion, which `clippy::unused_unit` refuses. The nested arm
        // answers with a space, which is what a comment is to the parser anyway.
        rule BLOCK_INNER -> char =
            BLOCK_COMMENT -> { ' ' }
          | not("*/") c:any -> { c }

        // String literals keep their escapes: the boundary of a frame is
        // written `"\n"`, and what the emitter hands to the parser backend is
        // that same text. The built-in `string` recognizes only `\\` and `\"`,
        // which is one escape short of a newline.
        //
        // UPPERCASE: a lexical rule, or the implicit whitespace would eat the
        // spaces inside the literal.
        rule STRING -> String =
            "\"" parts:STR_CHAR* "\"" -> { parts.concat() }

        rule STR_CHAR -> String =
            "\\" c:any -> {
                let mut s = String::from("\\");
                s.push(c);
                s
            }
          | not("\"") c:any -> { c.to_string() }

        // The explicit form, for the leading and trailing positions where there
        // is no preceding token for the implicit `WS` to follow.
        // --- Top-Level Items ---
        //
        // `grammar` and `struct` come before `fn`: all three are keyword-led,
        // and the order is what keeps `grammar` from being read as an
        // identifier.
        // `@=` puts the byte range this rule matched into `_span`, which is
        // how every node below gets the place in the `.nika` file it came from.
        // Without it a diagnostic can only name generated Rust.
        // Labelled, here and below: where one of these fails at the position
        // it started at, none of its alternatives got anywhere, and listing
        // what each could have begun with says less than the word for what was
        // expected. A rule that got *further* keeps its own message - the
        // label only replaces the list (winnow-grammar `# "…"`).
        // **And every arm takes the prose standing in front of it**
        // ([ADR-139](../../../../docs/specification/adr/adr-139.md) D1). The
        // `///` lines are gone by the time this runs — the implicit whitespace
        // skip ate them — so what is read back is what the skip recorded
        // (`Trivia::comment`), and `take_doc` is a *take*: an item consumes its
        // own prose, so the next one does not inherit it.
        rule item -> Spanned<Item> # "item" @=
            d:doc_here g:grammar_item -> { Spanned::documented(g, _span, d) }
          | d:doc_here s:struct_item -> { Spanned::documented(s, _span, d) }
          | d:doc_here e:enum_item -> { Spanned::documented(e, _span, d) }
          | d:doc_here im:impl_item -> { Spanned::documented(im, _span, d) }
          | d:doc_here t:trait_item -> { Spanned::documented(t, _span, d) }
          | d:doc_here u:use_item -> { Spanned::documented(u, _span, d) }
          | d:doc_here c:comptime_item -> { Spanned::documented(c, _span, d) }
          | d:doc_here e:extern_item -> { Spanned::documented(e, _span, d) }
          | d:doc_here i:fn_item -> { Spanned::documented(i, _span, d) }
          | d:doc_here t:test_item -> { Spanned::documented(t, _span, d) }

        // **`test "name" { … }`**
        // ([ADR-245](../../../../docs/specification/adr/adr-245.md) D1): where a
        // `fn` may stand, seeing what the package sees. Compiled only by
        // `nikaia test` - `modules` leaves it out of every other build.
        rule test_item -> Item =
            KW_TEST title:STRING body:block -> { Item::Test { name: title, body } }

        // Part III 15.1: `extern "C" { fn getpid() -> i32 }`
        // ([ADR-124](../../../../docs/specification/adr/adr-124.md) D1).
        //
        // **The declarations are `trait_method`s**, because a signature without
        // a body is the same shape wherever it stands. What differs is how one
        // *reads* - D2 makes an `extern` declaration `sync` and gives it no
        // `throws`, where a trait method without `sync` may pause - and that is
        // the ledger's business rather than the grammar's.
        //
        // The ABI is the string the source wrote. Only `"C"` means anything
        // today, and refusing a second one is a **check** rather than a shape,
        // so the grammar takes any string and the ledger pass says which.
        //
        // **A block holds two shapes since**
        // ([ADR-147](../../../../docs/specification/adr/adr-147.md) D3): a
        // signature, and an `opaque type … released by …`. They may interleave
        // freely in the source and are taken apart here, so nothing downstream
        // has to walk a list of two kinds.
        rule extern_item -> Item =
            KW_EXTERN abi:STRING "{" members:extern_member* "}"
            -> {
                let mut declarations = Vec::new();
                let mut opaque = Vec::new();
                for member in members {
                    match member {
                        ExternMember::Declared(d) => declarations.push(d),
                        ExternMember::Opaque(o) => opaque.push(o),
                    }
                }
                Item::Extern { abi, declarations, opaque }
            }

        rule extern_member -> ExternMember =
            o:opaque_item -> { ExternMember::Opaque(o) }
          | d:trait_method -> { ExternMember::Declared(d) }

        // **`opaque type T released by f`**
        // ([ADR-147](../../../../docs/specification/adr/adr-147.md) D3): an
        // address the language never dereferences, whose release is a `cleanup`
        // the compiler runs at the end of its scope (Part I 6.4).
        //
        // **None of the four words is reserved**, and that is deliberate. The
        // grammar is scannerless, so a word only means something where a rule
        // asks for it — and `opaque`, `type`, `released` and `by` are all names
        // a program may want. Reserving a word buys exactly one thing, the
        // sentence a reader who writes it gets
        // ([ADR-117](../../../../docs/specification/adr/adr-117.md) D2), and
        // this position can say that sentence without taking the word away.
        rule opaque_item -> Spanned<OpaqueType> @=
            KW_OPAQUE KW_TYPE name:NAME KW_RELEASED KW_BY released_by:NAME
            -> { Spanned::new(OpaqueType { name, released_by }, _span) }

        // Kap 4.2: behaviour lives in an `impl`, never in the struct.
        // Kap 4.2 and 4.7: `impl User` gives a type behaviour of its own,
        // `impl Summarize for User` gives it a trait's. The trait name comes
        // first and the `for` is what tells the two apart, so the grammar reads
        // a name and only then finds out which form it was in.
        rule impl_item -> Item =
            KW_IMPL first:type_ref rest:impl_for_target?
            "{" methods:impl_method* "}"
            -> {
                let (trait_name, target) = match rest {
                    Some(t) => (Some(first.name), t),
                    None => (None, first),
                };
                Item::Impl { trait_name, target, methods }
            }

        rule impl_for_target -> Type = KW_FOR t:type_ref -> { t }

        // Kap 4.7: `trait Summarize { fn summary(&self) -> String }`.
        //
        // The methods are **signatures**, so the body's `{ … }` is absent and
        // the rule stops at the return type. That is what tells this apart from
        // an `impl` at the grammar level rather than at a later check
        // ([ADR-078](../../../../docs/specification/adr/adr-078.md) D1).
        rule trait_item -> Item =
            vis:kw_pub?
            KW_TRAIT
            name:NAME
            "{" methods:trait_method* "}"
            -> {
                Item::Trait { name, methods, is_public: vis.is_some() }
            }

        // A trait's methods are entries too — `Summarize::summary` is how a
        // bound is looked up — so they take their prose the same way
        // ([ADR-139](../../../../docs/specification/adr/adr-139.md) D1).
        rule trait_method -> Spanned<TraitMethod> @=
            d:doc_here
            KW_FN
            name:NAME
            generics:generic_list?
            params:fn_params
            promise_before_the_arrow?
            ret:return_type_arrow?
            promise:promise_after_the_type
            -> {
                let (sync, throws) = promise;
                Spanned::documented(TraitMethod {
                    name,
                    generics: generics.unwrap_or_default(),
                    receiver: params.receiver,
                    args: params.args,
                    config: params.config,
                    ret_type: ret,
                    is_sync: sync,
                    throws,
                }, _span, d)
            }

        // **A method takes its prose too**
        // ([ADR-139](../../../../docs/specification/adr/adr-139.md) D1, D2), and
        // it is the shape a package's surface is mostly made of: a `pub`
        // method's entry is in the ledger under `Type::name`, so a consumer
        // reads it there or nowhere.
        rule impl_method -> Spanned<Item> @=
            d:doc_here f:fn_item -> { Spanned::documented(f, _span, d) }

        rule kw_sync -> () = KW_SYNC -> { () }
        rule kw_pub -> () = KW_PUB -> { () }

        // **`sync` and `throws` stand after the result type**
        // ([ADR-140](../../../../docs/specification/adr/adr-140.md) D4), which
        // is the order [ADR-102](../../../../docs/specification/adr/adr-102.md)
        // D1 already fixed for a function *type* - one order in a declaration
        // and in a type. Both sides used to parse and the specification wrote
        // both: Part II `fn add(…) sync`, Part III
        // `pub fn read(path: Path) -> Bytes throws`, Part I 7.1
        // `fn fetch_config() throws -> String`.
        //
        // A declaration with **no** result type writes them in the same place,
        // because there is nothing for them to be before or after:
        // `fn tick() sync { … }` is untouched.
        rule fn_item -> Item =
            vis:kw_pub?
            KW_FN
            name:NAME?
            generics:generic_list?
            params:fn_params
            promise_before_the_arrow?
            ret:return_type_arrow?
            promise:promise_after_the_type
            body:block
            -> {
                let (sync, throws) = promise;
                Item::Fn {
                    name,
                    generics: generics.unwrap_or_default(),
                    receiver: params.receiver,
                    args: params.args,
                    config: params.config,
                    spread: params.spread,
                    ret_type: ret,
                    body,
                    is_sync: sync,
                    is_public: vis.is_some(),
                    throws,
                }
            }

        // The refusal, and it needs a rule of its own because the message is
        // the whole point: without it the parser offers `{` where a reader
        // wrote `throws` and says nothing about the order
        // ([ADR-140](../../../../docs/specification/adr/adr-140.md) D4).
        //
        // **The cut sits after the arrow**, so the word alone is not enough to
        // fire it: `fn tick() sync { … }` matches `kw_sync`, finds no `->`,
        // backtracks, and the trailing slot takes the same word one position
        // later. Only a promise *followed by* an arrow is the old form.
        // **`sync` stands before `throws`**, which is the order
        // [ADR-102](../../../../docs/specification/adr/adr-102.md) D1 fixes for
        // a function type and the order
        // [ADR-140](../../../../docs/specification/adr/adr-140.md) D4's own
        // example writes. `fn f() throws sync { … }` used to parse, and only by
        // accident: `throws` took the slot before the arrow and `sync` the one
        // after it, and D4 leaves one slot. The refusal is here because
        // *expected `{`* is what a reader would otherwise get for a form that
        // was legal a version ago.
        rule promise_after_the_type -> (bool, bool) =
            kw_throws kw_sync fail(
                "`sync` stands before `throws` (ADR-140 D4, ADR-102 D1): write \
                 `sync throws`. It is the order a function *type* has had since \
                 ADR-102, and `throws sync` used to parse here only because the \
                 two words sat in different slots - one before the result type \
                 and one after it - which D4 leaves as one."
            ) -> { (true, true) }
          | s:kw_sync? t:kw_throws? -> { (s.is_some(), t.is_some()) }

        rule promise_before_the_arrow -> () =
            kw_sync kw_throws? "->" => fail(
                "`sync` and `throws` stand after the result type (ADR-140 D4): \
                 write `fn f() -> String sync`. That is the order ADR-102 D1 \
                 already fixes for a function *type*, where the trailing words \
                 are greedy, so a declaration and a type read the same way \
                 round. A declaration with no result type writes the word in \
                 the same place it always did."
            ) -> { () }
          | kw_throws kw_sync? "->" => fail(
                "`throws` and `sync` stand after the result type (ADR-140 D4): \
                 write `fn f() -> String throws`. That is the order ADR-102 D1 \
                 already fixes for a function *type*, where the trailing words \
                 are greedy, so a declaration and a type read the same way \
                 round. A declaration with no result type writes the word in \
                 the same place it always did."
            ) -> { () }

        // ADR-023 D1: `throws` carries no type list. The specification itself
        // wrote one in four places, so a reader will try it, and a parse error
        // at the type name says nothing about why - it offered `->` and `sync`
        // as if either were the point. The precedent is `trailing_lambda`
        // below: a form that was in the specification deserves a sentence.
        //
        // `not(kw_sync)` because `fn f() throws sync { … }` is legal - `sync`
        // may stand on either side of the return type - and `sync` is an
        // ordinary identifier to `type_ref`.
        rule kw_throws -> () =
            KW_THROWS not(kw_sync) type_refs fail(
                "`throws` names no error type (ADR-023 D1): write `throws` on its own. \
                 *Which* errors can leave a function follows from its body and from \
                 everything the body reaches, so it is derived rather than written: \
                 the set is inferred whole-program and recorded in `nikaia.contracts`, \
                 beside the build (Part III, 13.5). Written by hand it would go stale \
                 the first time a callee gained a failure - and a failure that comes \
                 from a resource's cleanup would make you name a type you never \
                 mentioned (ADR-006 D4)."
            ) -> { () }
          | KW_THROWS -> { () }

        // Kap 4.2: `ref mut self`, `ref self`, `self` - the subject, when there
        // is one.
        rule fn_params -> FnParams =
            "(" body:fn_params_body? ")" -> {
                body.unwrap_or_default()
            }

        rule fn_params_body -> FnParams =
            r:receiver args:fn_arg_def_tail* config:config_zone? -> {
                params_of(Some(r), args, config)
            }
          // **A signature whose parameters are all options writes no `;`**
          // ([ADR-133](../../../../docs/specification/adr/adr-133.md) D1). Tried
          // before the positional list and decided on the token after the type:
          // `bare_config_params` insists on the `= value` that makes a parameter
          // an option, so `fn add(a: i64, b: i64)` fails it at the `,` and the
          // alternative below takes it. Where one zone is empty there is nothing
          // for the separator to stand between.
          | config:bare_config_params -> {
                params_of(None, Vec::new(), Some(ConfigZone::Options(config)))
            }
          | head:fn_arg_def tail:fn_arg_def_tail* config:config_zone? -> {
                let mut args = vec![head];
                args.extend(tail);
                params_of(None, args, config)
            }
          // **And the leading `;` is refused rather than accepted beside it**
          // (D2). Two spellings for one shape would be worse than either alone:
          // the old one would keep appearing in code that reads the old pages,
          // and a reader would wonder what the difference is.
          //
          // The cut is what makes this the message. Without it the alternative
          // fails, the rule backtracks, and `config_zone`'s own `;` arm - the one
          // a *mixed* signature needs - would parse the old form and the sentence
          // would never be read.
          | ";" => fail(
                "a parameter list with no subjects writes its options without the `;` \
                 (ADR-133 D1): `fn execute(target_age: i64 = 0)`. The `;` stands between \
                 the two zones of Kap 5.1 - subjects before it, options after - and where \
                 one zone is empty it separates nothing. A *mixed* list keeps it, and \
                 keeps it required."
            ) -> {
                params_of(None, Vec::new(), None)
            }

        // Kap 5.1: everything after the `;` is an option. Named at the call,
        // never positional - which is what the separator buys, and why it is a
        // separator rather than a convention about where the flags go.
        //
        // ADR-007 D5 puts one more thing there: `...args: Self::dsl`, the typed
        // spread a DSL driver accepts deferred parameters with. It is tried
        // first because `...` cannot begin an option's name, so a `;` followed
        // by one is unambiguous.
        rule config_zone -> ConfigZone =
            ";" "..." name:NAME ":" "Self::dsl" -> { ConfigZone::Spread(name) }
          | ";" "..." name:NAME ":" fail(
                "a typed spread is written `...name: Self::dsl` (ADR-007 D5): \
                 `Self::dsl` is the parameter type the DSL string generates, and \
                 it is the only type this parameter can have."
            ) -> { ConfigZone::Spread(name) }
          | ";" head:config_param tail:config_param_tail* -> {
                let mut params = vec![head];
                params.extend(tail);
                ConfigZone::Options(params)
            }

        rule config_param_tail -> ConfigParam = "," p:config_param -> { p }

        // The options-only list, without the `;`
        // ([ADR-133](../../../../docs/specification/adr/adr-133.md) D1).
        //
        // **Its own rule and not `config_param` reused**, because `config_param`
        // ends in a `fail` for a missing default - the right message *after* a
        // `;`, where a parameter can only be an option, and the wrong one here,
        // where `name: T` with no default is an ordinary subject and the
        // alternative below this one is what should read it. A `fail` outranks
        // the alternatives at its position, so reusing it would have put
        // *a configuration parameter needs a default* on every plain signature
        // that failed for some later reason.
        rule bare_config_params -> Vec<ConfigParam> =
            head:bare_config_param tail:bare_config_param_tail* -> {
                let mut params = vec![head];
                params.extend(tail);
                params
            }

        rule bare_config_param_tail -> ConfigParam = "," p:bare_config_param -> { p }

        rule bare_config_param -> ConfigParam =
            name:NAME ":" ty:type_ref "=" default:literal_expr -> {
                ConfigParam { name, ty, default }
            }

        // The default is required, and that is what makes this an *option*: a
        // caller may leave it out, and leaving it out is never a question about
        // what the value is. A parameter that must be passed belongs before the
        // `;`.
        rule config_param -> ConfigParam =
            name:NAME ":" ty:type_ref "=" default:literal_expr -> {
                ConfigParam { name, ty, default }
            }
          | name:NAME ":" ty:type_ref fail(
                "a configuration parameter needs a default (Kap 5.1): write \
                 `name: T = value`. Without one it has to be passed at every \
                 call, and a parameter that has to be passed belongs before the \
                 `;`."
            ) -> {
                ConfigParam { name, ty, default: Expr::LitBool(false) }
            }

        // A **literal**, and only a literal. An option's default is a constant
        // in every program anyone writes, and an arbitrary expression would
        // raise a question Stage 0 has no answer for: whether it is evaluated
        // where the function is declared or where it is called.
        rule literal_expr -> Expr =
            b:bool_lit -> { b }
          | s:str_lit -> { s }
          | c:char_lit -> { c }
          | "-" f:float_lit -> { Expr::Unary { op: UnaryOp::Neg, expr: Box::new(f) } }
          | f:float_lit -> { f }
          | "-" i:int_lit -> { Expr::Unary { op: UnaryOp::Neg, expr: Box::new(i) } }
          | i:int_lit -> { i }

        rule receiver -> Receiver =
            KW_REF KW_MUT KW_SELF -> {
                Receiver { is_ref: true, is_mut: true }
            }
          | KW_REF KW_SELF -> { Receiver { is_ref: true, is_mut: false } }
          // Before the bare arm: `self: i64` is a parameter somebody named
          // `self`, and the bare arm would take the word and leave the `: i64`
          // to fail as the *next* parameter - at the colon, with nothing to say.
          // `self` is a reserved word (ADR-051 D1) and the only one the grammar
          // cannot exclude from `NAME`, so this is where declaring one is
          // refused in a position `NAME` never sees.
          //
          // **The colon is consumed and not peeked**, which is the opposite of
          // what ADR-046 D2's import refusals do, and measured both ways: with
          // `peek(":")` - or `peek((KW_SELF ":"))` - the bare arm below reaches
          // just as far and its *"expected `)`"* wins, because a `fail` is high
          // priority and not fatal ("progress before priority"). So the arm has
          // to get further than the alternative, and the cost is the caret: it
          // lands on the type rather than on the word, one token past where a
          // reader would put it. The sentence is what carries the answer.
          | KW_SELF ":" fail(
                "`self` is a reserved word, so a parameter may not be called \
                 that (Part I, 2.1). It already names one thing - the value a \
                 method was called on - and that is written `self`, `ref self` or \
                 `ref mut self`, with no type beside it"
            ) -> { Receiver { is_ref: false, is_mut: false } }
          | KW_SELF -> { Receiver { is_ref: false, is_mut: false } }

        // Kap 9.2: use std::fs
        rule use_item -> Item =
            // **Names are not brought in** (ADR-046 D2), and the two forms that
            // try to are worth a sentence rather than a parse error at the brace
            // or the star: both are in every language that has them, so a reader
            // will write one. First, because the arm below matches `use http` and
            // leaves the rest to fail as the next item - at the same place, with
            // nothing to say.
            KW_USE NAME "::" peek("{") fail("names are not brought in; a package is reached \
                                      through its name. Write `use http`, and \
                                      `http::Request` where you need it - and \
                                      `use http as h` if the prefix is long \
                                      (Part I, 9.1)") -> {
                Item::Import { path: Vec::new(), alias: None }
            }
          | KW_USE NAME "::" peek("*") fail("a package is reached through its name, and \
                                      nothing brings every name in. Write `use http`, \
                                      and `http::Request` where you need it \
                                      (Part I, 9.1)") -> {
                Item::Import { path: Vec::new(), alias: None }
            }
          | KW_USE head:NAME tail:path_segment* alias:use_alias? -> {
                let mut path = vec![head];
                path.extend(tail);
                Item::Import { path, alias }
            }

        // `use http as h` (ADR-046 D3): the one thing that record adds rather
        // than refuses, and what makes the qualified-only rule affordable. It
        // shortens the prefix once, in one place, and settles a collision - two
        // libraries that both want to be `http` are the consumer's to name apart.
        rule use_alias -> Symbol = KW_AS n:NAME -> { n }

        // **A segment after `::` may be a reserved word**, and `SEGMENT` rather
        // than `NAME` is what says so. Nothing can be misread there: a segment
        // follows a `::` and no construct begins in that position, so the word
        // is a name whatever else it is elsewhere. `Self::dsl` is the case that
        // requires it - the shadow type of a deferred-parameter DSL is spelled
        // with the keyword (ADR-007 D5, Part II 10.5) - and the general rule is
        // the reason to allow it rather than that one type.
        rule path_segment -> Symbol = "::" n:SEGMENT -> { n }

        // **A name that can only be a name.** `SEGMENT` is `NAME` without the
        // reserved-word check, and it is used exactly where a separator has
        // already decided that what follows is a member: after `::` and after
        // `.`. No construct begins in either position, so a reserved word there
        // is a name whatever it is elsewhere - which is what lets `Self::dsl`
        // (ADR-007 D5) and `scope.spawn fn { … }` (Part II 12.5) keep their
        // spellings.
        //
        // Everywhere a name is *declared* or stands on its own, `NAME` is what
        // the grammar uses, and that is where the list bites.
        rule SEGMENT -> Symbol = not(digit) n:ident -> { n }

        // --- Structs ---
        //
        // **No attribute stands here** ([ADR-201](../../docs/specification/adr/adr-201.md)
        // D1). `@borrowed` used to, asserting that no value of this type ever
        // tethers, and it forbade nothing: the transition it refused does not
        // exist. D2 names `@tethers` as the word that will stand here — a
        // **permission** and not an assertion — and D3 keeps it out of this
        // grammar until the state it permits is built, so a program that writes
        // it gets the parse error listing what is possible.
        rule struct_item -> Item =
            vis:kw_pub?
            KW_STRUCT
            name:NAME
            generics:generic_list?
            "{"
            fields:field_defs?
            "}"
            -> {
                Item::Struct {
                    name,
                    generics: generics.unwrap_or_default(),
                    fields: fields.unwrap_or_default(),
                    is_public: vis.is_some(),
                }
            }

        // Kap 4.4. The three shapes the specification shows and no others: a
        // name, a name with positional types, a name with named fields.
        rule enum_item -> Item =
            vis:kw_pub?
            KW_ENUM name:NAME
            "{" variants:enum_variants "}"
            -> {
                Item::Enum { name, variants, is_public: vis.is_some() }
            }

        rule enum_variants -> Vec<EnumVariant> =
            head:enum_variant tail:enum_variant_tail* ","? -> {
                let mut variants = vec![head];
                variants.extend(tail);
                variants
            }

        rule enum_variant_tail -> EnumVariant = "," v:enum_variant -> { v }

        rule enum_variant -> EnumVariant =
            name:NAME "(" types:type_refs ")" -> {
                EnumVariant { name, fields: VariantFields::Tuple(types) }
            }
          | name:NAME "{" fields:field_defs "}" -> {
                EnumVariant { name, fields: VariantFields::Named(fields) }
            }
          | name:NAME -> {
                EnumVariant { name, fields: VariantFields::Unit }
            }

        rule field_defs -> Vec<FieldDef> =
            head:field_def tail:field_def_tail* ","? -> {
                let mut fields = vec![head];
                fields.extend(tail);
                fields
            }

        rule field_def_tail -> FieldDef = "," f:field_def -> { f }

        // Kap 9.2: `pub` on a field, which is a different question from `pub`
        // on the struct - a public type may keep its parts to itself, and 9.3
        // says that is the point.
        // `@=` for the span: a diagnostic about a field puts its caret on the
        // field, and the nearest span the walk around it has is a statement's
        // (ADR-051 D4).
        rule field_def -> FieldDef @=
            vis:kw_pub? name:NAME ":" ty:type_ref -> {
                FieldDef { name, ty, is_public: vis.is_some(), span: _span }
            }

        // --- Argumente & Typen ---

        rule fn_arg_list -> Vec<FnArg> =
            "(" args:fn_arg_defs? ")" -> { args.unwrap_or_default() }

        rule fn_arg_defs -> Vec<FnArg> =
            head:fn_arg_def tail:fn_arg_def_tail* -> {
                let mut args = vec![head];
                args.extend(tail);
                args
            }

        rule fn_arg_def_tail -> FnArg = "," arg:fn_arg_def -> { arg }

        // `mut out: Vec[i64]` is a parameter the callee changes **in place**,
        // and the caller's value is what changes
        // ([ADR-094](../../../../docs/specification/adr/adr-094.md) D3) - which
        // is `&mut self`'s rule held for every parameter. The call shows
        // nothing, exactly as `xs.push(1)` shows nothing.
        // **`_` is a parameter a shape dictates** (ADR-126 D1). It still carries
        // its type, because the caller needs it - what `_` says is that this body
        // does not read the value, never that the signature is shorter.
        rule fn_arg_def -> FnArg @=
            mutable:kw_mut? name:NAME ":" ty:type_ref -> {
                FnArg { name, ty, mutable: mutable.is_some(), span: _span }
            }
          | UNDERSCORE ":" ty:type_ref -> {
                FnArg { name: _state.intern("_"), ty, mutable: false, span: _span }
            }

        rule return_type_arrow -> Type =
            "->" ty:type_ref -> { ty }

        // USING [ ] SYNTAX directly for testing
        rule generic_list -> Vec<GenericParam> =
            [ params:generic_params? ] -> { params.unwrap_or_default() }

        rule generic_params -> Vec<GenericParam> =
            head:generic_param tail:generic_param_tail* -> {
                let mut params = vec![head];
                params.extend(tail);
                params
            }

        rule generic_param_tail -> GenericParam = "," p:generic_param -> { p }

        // Kap 4.7: `[T: Summarize]`, and `[T: A + B]` for several
        // ([ADR-078](../../../../docs/specification/adr/adr-078.md) D2).
        rule generic_param -> GenericParam =
            name:NAME bounds:generic_bound?
            -> { GenericParam { name, bounds: bounds.unwrap_or_default() } }

        // **A bound takes a path, as every other position that names a type
        // does** ([ADR-106](../../../../docs/specification/adr/adr-106.md) D1):
        // `[H: http::Handler]`, and `[T: A + http::B]` mixes a local and a
        // foreign one. `type_name` is that rule, and reusing it is the record's
        // own sentence — *the grammar's rule for a type name is the rule for a
        // bound's trait name* — so the whole path is interned as one name,
        // exactly as `impl http::Handler for Fixed` already had it.
        rule generic_bound -> Vec<Symbol> =
            ":" head:type_name tail:generic_bound_tail* -> {
                let mut bounds = vec![head];
                bounds.extend(tail);
                bounds
            }

        rule generic_bound_tail -> Symbol = "+" n:type_name -> { n }

        // **`&[u8]` and `&mut [u8]`, what the C boundary lends**
        // ([ADR-147](../../../../docs/specification/adr/adr-147.md) D1): a run of
        // elements whose length the caller knows and the type does not.
        //
        // **Always behind a `&`**, which is the record's own shape: a bare `[T]`
        // is a value of no size and this language has nowhere to put one. The
        // element goes where a tuple's parts go, so everything that walks a
        // type's arguments walks it.
        //
        // **First**, because the named alternative below begins with the same
        // optional `&` and would take the `&` and then fail on the `[`.
        // **`ref Array[T]` is the run, and the bracket form is gone with the
        // `&`** ([ADR-184](../../../../docs/specification/adr/adr-184.md) D3,
        // D4): `Array[T]` with no count names a run and `ref` makes it a view
        // of one, which is the second alternative below reading its own name.
        // Two spellings for one idea is what that record exists to remove.
        rule type_ref -> Type # "type" =
          // `ref String` is a view marker (Part II, 10.6), not a lifetime - the
          // word is recorded and the emitter decides what it becomes.
            view:amp?
            name:type_name
            generics:generic_type_args?
            nullable:question?
            -> {
                let mutable = matches!(view, Some(true));
                Type {
                    name,
                    generics: generics.unwrap_or_default(),
                    is_view: view.is_some(),
                    is_tuple: false,
                    is_nullable: nullable.is_some(),
                    code: None,
                    count: None,
                    is_mut: mutable,
                    is_slice: false,
                    either: false,
                }
            }
          // **An integer where a type argument stands**
          // ([ADR-152](../../../../docs/specification/adr/adr-152.md) D1): the
          // `3` of `Array[f64, 3]`. It is an alternative of `type_ref` rather
          // than a second kind of argument, because that is what makes the
          // bracket generic the type language already has carry it — nothing
          // that walks a type's arguments learns a new shape.
          //
          // **The position is free.** No name begins with a digit, so a number
          // here can be nothing else; what a number means where a type is
          // *expected* rather than where an argument stands is the checker's
          // (`NK1135` stays silent, and the type it names is unknown).
          | n:number_lit -> {
                Type {
                    name: _state.intern(&n.to_string()),
                    generics: Vec::new(),
                    is_view: false,
                    is_tuple: false,
                    is_nullable: false,
                    code: None,
                    count: i64::try_from(n).ok(),
                    is_mut: false,
                    is_slice: false,
                    either: false,
                }
            }
          // `(A, B)`. The parts go where a named type's arguments go, so
          // everything that walks a type's arguments walks a tuple's parts.
          //
          // **No `?` here**, and that is Part I 2.3's own scope: `(A, B)?` is
          // not in the specification, and a form the emitter would have to
          // invent a lowering for is worse unwritten than written wrong.
          | "(" parts:type_refs ")" -> {
                Type {
                    name: _state.intern("tuple"),
                    generics: parts,
                    is_view: false,
                    is_tuple: true,
                    is_nullable: false,
                    code: None,
                    count: None,
                    is_mut: false,
                    is_slice: false,
                    either: false,
                }
            }
          // **A parameter may be code**
          // ([ADR-102](../../../../docs/specification/adr/adr-102.md) D1):
          // `fn(Request) -> Response`, with `sync` and `throws` after the
          // result, in the positions a declaration puts them. The parameters go
          // where a tuple's parts go.
          //
          // **The trailing words are greedy**, which settles the one ambiguity
          // D1 does not name: in `fn make() -> fn(i64) -> i64 sync` the `sync`
          // belongs to the *result type*. A function whose own promise is meant
          // writes it before the arrow, which `fn_item` accepts already —
          // `fn make() sync -> fn(i64) -> i64`.
          | KW_FN "(" params:type_refs? ")" result:return_type_arrow?
            s:kw_sync? t:kw_throws? -> {
                Type {
                    name: _state.intern("fn"),
                    generics: params.unwrap_or_default(),
                    is_view: false,
                    is_tuple: false,
                    is_nullable: false,
                    code: Some(Box::new(Code {
                        result,
                        is_sync: s.is_some(),
                        throws: t.is_some(),
                    })),
                    count: None,
                    is_mut: false,
                    is_slice: false,
                    either: false,
                }
            }

        // **The `&`, and whether a `mut` followed it**
        // ([ADR-147](../../../../docs/specification/adr/adr-147.md) D1). `&mut T`
        // is a view the callee may write through, and the C boundary is the one
        // place this language writes one: a parameter's mutability is otherwise
        // the word in front of its *name*
        // ([ADR-094](../../../../docs/specification/adr/adr-094.md) D3).
        //
        // **`ref` is the word and `&` is the spelling it replaces**
        // ([ADR-184](../../../../docs/specification/adr/adr-184.md) D1). Both
        // parse while the corpus moves; D4 is where the second one leaves.
        rule amp -> bool = KW_REF m:kw_mut? -> { m.is_some() }

        // Part I 2.3: the trailing `?` that makes a type nullable. It comes
        // last, after the arguments, so `Vec[i64]?` is a nullable list and not
        // a list of nullables - which is `Vec[i64?]`.
        rule question -> () = "?" -> { () }

        // USING [ ] SYNTAX directly for testing
        rule generic_type_args -> Vec<Type> =
            [ args:type_refs? ] -> { args.unwrap_or_default() }

        // A type may be named by a path: `postgres::Connection`, `html::Raw`.
        //
        // The whole path is interned as one name, because that is what the name
        // *is* to a compiler that lowers name for name (ADR-011 D2) - nothing
        // here resolves a module, and a path can therefore never collide with a
        // struct this file declares, which is correct.
        rule type_name -> Symbol =
            head:NAME tail:path_segment* -> {
                if tail.is_empty() {
                    head
                } else {
                    let mut path = String::new();
                    path.push_str(_state.interner.resolve(head));
                    for segment in tail {
                        path.push_str("::");
                        path.push_str(_state.interner.resolve(segment));
                    }
                    _state.intern(&path)
                }
            }

        rule type_refs -> Vec<Type> =
            head:type_ref tail:type_ref_tail* -> {
                let mut args = vec![head];
                args.extend(tail);
                args
            }

        rule type_ref_tail -> Type = "," t:type_ref -> { t }

        // --- Part II, Kapitel 10: Grammatiken ---

        rule grammar_item -> Item =
            KW_GRAMMAR name:NAME
            "{" rules:grammar_rule* "}"
            -> { Item::Grammar(GrammarDef { name, rules }) }

        rule grammar_rule -> GrammarRule @=
            frame:frame_attr?
            vis:kw_pub?
            KW_RULE
            name:NAME
            ret:return_type_arrow?
            label:rule_label?
            "="
            alts:g_alts
            -> {
                GrammarRule {
                    name,
                    is_public: vis.is_some(),
                    frame,
                    ret_type: ret,
                    label,
                    alts,
                    span: _span,
                }
            }

        // `rule expr -> Expr # "expression" = …`: what the rule is called when
        // it fails where it began. Spelled as the backend spells it, because
        // the lowering is name for name (ADR-011 D2) and a second spelling for
        // the same thing would be one more thing to know.
        rule rule_label -> String = "#" text:STRING -> { text }

        // ADR-009 D1: the attribute is keyed. `@frame`, `@frame(boundary: "\n")`,
        // `@frame(boundary: "\n", unchecked)`; the positional form is withdrawn,
        // so that every future cut-point key (quote, start, escape, scan) has
        // room without changing what the existing ones mean.
        rule frame_attr -> FrameAttr =
            "@frame" args:frame_args? -> {
                args.unwrap_or_default()
            }

        rule frame_args -> FrameAttr =
            "(" head:frame_arg tail:frame_arg_tail* ")" -> {
                let mut attr = head;
                for a in tail {
                    if a.boundary.is_some() { attr.boundary = a.boundary; }
                    attr.unchecked = attr.unchecked || a.unchecked;
                }
                attr
            }

        rule frame_arg_tail -> FrameAttr = "," a:frame_arg -> { a }

        rule frame_arg -> FrameAttr =
            KW_BOUNDARY ":" b:STRING -> {
                FrameAttr { boundary: Some(b), unchecked: false }
            }
          | KW_UNCHECKED -> {
                FrameAttr { boundary: None, unchecked: true }
            }

        rule g_alts -> Vec<GrammarAlt> =
            head:g_alt tail:g_alt_tail* -> {
                let mut alts = vec![head];
                alts.extend(tail);
                alts
            }

        rule g_alt_tail -> GrammarAlt = "|" a:g_alt -> { a }

        // **The action is the block after the pattern, with no second arrow**
        // ([ADR-120](../../../../docs/specification/adr/adr-120.md) D2, Part II
        // 10.8). The one `->` a rule writes is its result type, as a function's
        // is; the arrow in front of the action was a wart from the days when the
        // two sat on one line.
        //
        // An action block is required today (Part II, 10.1, note) - with one
        // exception that is not an omission: a `par_fold` must be the whole
        // body of its rule (ADR-009 D2), so there is nothing for an action to
        // add. The emitter supplies the binding such a rule needs.
        //
        // **The old form is refused by name and not by *expected `{`***, which
        // is what a reader of a program written a version ago would otherwise
        // get. The arrow alternative stands first, so it is what matches when
        // the arrow is there, and the `fail` stands **at** the arrow rather than
        // after the block, so the caret is on the character to delete.
        rule g_alt -> GrammarAlt =
            p:g_seq "->" fail(
                "an action is the block after the pattern, with no arrow in front \
                 of it (ADR-120 D2, Part II 10.8): write `pattern { action }`. The \
                 one `->` a rule writes is its result type, as a function's is."
            ) -> {
                // Never reached: `fail` has already ended the alternative. The
                // binding is here because the action has to be well-typed.
                GrammarAlt { pattern: p, action: None }
            }
          | p:g_seq action:block -> {
                GrammarAlt { pattern: p, action: Some(action) }
            }
          | f:g_fold -> {
                GrammarAlt { pattern: f, action: None }
            }

        rule g_seq -> Spanned<Pattern> @=
            head:g_elem tail:g_elem_tail* -> {
                if tail.is_empty() {
                    // One element is its own span, not the sequence's.
                    head
                } else {
                    let mut parts = vec![head];
                    parts.extend(tail);
                    Spanned::new(Pattern::Seq(parts), _span)
                }
            }

        rule g_elem_tail -> Spanned<Pattern> = e:g_elem -> { e }

        rule g_elem -> Spanned<Pattern> =
            c:g_cut -> { c }
          | b:g_bind -> { b }
          | p:g_postfix -> { p }

        // Part II, 10.1: the commit point. Once passed, a later failure is an
        // error rather than a reason to try the next alternative.
        rule g_cut -> Spanned<Pattern> @= "=>" -> { Spanned::new(Pattern::Cut, _span) }

        rule g_bind -> Spanned<Pattern> @=
            name:NAME ":" p:g_postfix -> {
                Spanned::new(Pattern::Bind { name, pat: Box::new(p) }, _span)
            }

        rule g_postfix -> Spanned<Pattern> @=
            a:g_atom rep:g_repeat? -> {
                match rep {
                    Some(r) => Spanned::new(Pattern::Repeat { pat: Box::new(a), rep: r }, _span),
                    None => a,
                }
            }

        // ADR-009 D5: a bounded repetition is how a format states a fixed
        // width. `*` and `+` say "unbounded" and mean it.
        rule g_repeat -> Repeat =
            "*" -> { Repeat::Star }
          | "+" -> { Repeat::Plus }
          | "?" -> { Repeat::Optional }
          | r:g_bounds -> { r }

        // A brace group is a bound only when its content starts with a digit,
        // which is the rule the backend states for the same ambiguity
        // (SYNTAX.md, "Braces").
        //
        // **And *immediately* starts with one**, which is what makes
        // [ADR-120](../../../../docs/specification/adr/adr-120.md) D2's sentence
        // true rather than nearly true. Since the action is the block after the
        // pattern, `digit { 1 }` is a rule that matches a digit and yields `1`,
        // and it is indistinguishable from a bound of one by *a digit follows the
        // brace* alone. `G_BOUND_START` is **lexical**, so nothing is skipped
        // inside it: `digit{1}` is a bound and `digit { 1 }` is an action, which
        // is the one place in a grammar where a space decides something and is
        // written down on the page for that reason.
        rule g_bounds -> Repeat =
            peek(G_BOUND_START) "{" b:g_bound_body "}" -> { b }

        rule G_BOUND_START -> () = "{" digit -> { () }

        rule g_bound_body -> Repeat =
            n:number "," m:number -> { Repeat::Between(n, m) }
          | n:number "," -> { Repeat::AtLeast(n) }
          | n:number -> { Repeat::Exactly(n) }

        rule number -> u32 =
            d:digit1 -> { d.parse().unwrap_or(0) }

        rule g_atom -> Spanned<Pattern> @=
            f:g_fold -> { f }
          | s:STRING -> { Spanned::new(Pattern::Literal(s), _span) }
          | g:g_group -> { g }
          | r:g_ref -> { r }

        rule g_group -> Spanned<Pattern> @=
            "(" p:g_choice ")" -> {
                Spanned::new(Pattern::Group(Box::new(p)), _span)
            }

        // A rule reference, a built-in (`digit`, `frame_end`), or a call to
        // either (`until(";" | frame_end)`, `list(pair, ",")`). The grammar
        // cannot tell them apart, and does not need to: what a name means is
        // the backend's question.
        rule g_ref -> Spanned<Pattern> @=
            name:NAME generics:generic_type_args? args:g_args? -> {
                Spanned::new(
                    Pattern::Ref {
                        name,
                        generics: generics.unwrap_or_default(),
                        args: args.unwrap_or_default(),
                    },
                    _span,
                )
            }

        rule g_args -> Vec<Spanned<Pattern>> =
            "(" head:g_choice tail:g_arg_tail* ")" -> {
                let mut args = vec![head];
                args.extend(tail);
                args
            }

        rule g_arg_tail -> Spanned<Pattern> = "," p:g_choice -> { p }

        rule g_choice -> Spanned<Pattern> @=
            head:g_seq tail:g_choice_tail* -> {
                if tail.is_empty() {
                    head
                } else {
                    let mut parts = vec![head];
                    parts.extend(tail);
                    Spanned::new(Pattern::Choice(parts), _span)
                }
            }

        rule g_choice_tail -> Spanned<Pattern> = "|" p:g_seq -> { p }

        // ADR-009 D2: parallel parsing is a frame plus a monoid. `fold` is the
        // accumulator; the merge is what makes it parallelisable, and asking
        // for it is how the user says a different chunk count is the same
        // answer to them.
        rule g_fold -> Spanned<Pattern> @=
            KW_PAR_FOLD "("
            r:NAME ","
            init:expr ","
            step:expr ","
            merge:expr ")"
            -> {
                Spanned::new(Pattern::Fold(Box::new(FoldSpec {
                    parallel: true,
                    rule: r,
                    init,
                    step,
                    merge: Some(merge),
                })), _span)
            }
          | KW_FOLD "("
            r:NAME ","
            init:expr ","
            step:expr ")"
            -> {
                Spanned::new(Pattern::Fold(Box::new(FoldSpec {
                    parallel: false,
                    rule: r,
                    init,
                    step,
                    merge: None,
                })), _span)
            }

        // --- Statements & Blocks ---

        rule block -> Block =
            "{" stmts:stmt_list "}" -> { Block { stmts } }

        rule stmt_list -> Vec<Spanned<Stmt>> =
            stmts:stmt* -> { stmts }

        rule stmt -> Spanned<Stmt> # "statement" @=
            c:comptime_stmt -> { Spanned::new(c, _span) }
          | l:let_stmt -> { Spanned::new(l, _span) }
          | r:return_stmt -> { Spanned::new(r, _span) }
          | t:throw_stmt -> { Spanned::new(t, _span) }
          | w:while_stmt -> { Spanned::new(w, _span) }
          | f:for_stmt -> { Spanned::new(f, _span) }
          | a:assign_stmt -> { Spanned::new(a, _span) }
          | e:expr_stmt -> { Spanned::new(e, _span) }
          | b:break_stmt -> { Spanned::new(b, _span) }
          | c:continue_stmt -> { Spanned::new(c, _span) }

        rule return_stmt -> Stmt =
            KW_RETURN value:expr? ";"? -> {
                Stmt::Return(value)
            }

        // Kap 3.3 and [ADR-084](../../../docs/specification/adr/adr-084.md).
        // **No value and no label**, which is why each of these is one keyword
        // and a rule of two lines: `break` in a language whose loops are
        // statements has nothing to carry out (D3), and the label is a second
        // naming scheme for the one case the unlabelled form does not reach
        // (D2).
        //
        // **Last in `stmt`, and that is measured rather than tidy** (D8). The
        // alternation is tried in order, so arms here are reached only by a
        // statement no earlier arm took - which, placed last, means the `}` that
        // ends a block and nothing else. Beside `return_stmt`, where they read
        // best, the two are tried and fail for *every* assignment and every bare
        // expression: **740 instructions a statement** and +1.28% on a
        // statement-dense file, against **665 per block** here, and a program has
        // far fewer blocks than statements (ADR-084 D8).
        //
        // It is free to choose because both words are reserved (ADR-071 D1), so
        // `NAME` cannot take one and no earlier arm can swallow a jump. The
        // ordering costs reading order and is paid back at 740 instructions a
        // statement, which is the whole reason this comment is here: an ordering
        // with no reason attached is one the next person tidies.
        rule break_stmt -> Stmt =
            KW_BREAK ";"? -> { Stmt::Break }

        rule continue_stmt -> Stmt =
            KW_CONTINUE ";"? -> { Stmt::Continue }

        // Kap 7.1: `throw` is the only way an error originates. Without it a
        // program could propagate what `std` produced and never produce one of
        // its own (ADR-023 D2).
        rule throw_stmt -> Stmt =
            KW_THROW value:expr ";"? -> {
                Stmt::Expr(Expr::Throw(Box::new(value)))
            }

        rule kw_mut -> () = KW_MUT -> { () }

        // Part II 10.2: `comptime LIMIT = 4 * 1024`, and the same form at item
        // level. **No `mut`**, which is not an omission
        // ([ADR-073](../../../../docs/specification/adr/adr-073.md) D6): a
        // constant is a value rather than a place, so there is nothing for a
        // second assignment to reach.
        rule comptime_stmt -> Stmt =
            KW_COMPTIME
            name:NAME
            ty:type_annotation?
            "="
            val:expr
            ";"?
            -> {
                Stmt::Comptime { name, ty, value: val }
            }

        // Part I 9.2 and [ADR-073](../../../../docs/specification/adr/adr-073.md)
        // D2's other half: the same form where an item stands.
        //
        // **`pub` is Part I 9.2's existing rule for Constants** rather than a
        // new one, which is why the statement form has no such flag and this
        // one does: what `pub` means here is what it means on a `struct`.
        //
        // Written as a rule of its own rather than by giving `comptime_stmt` an
        // optional `pub`, because the two produce different types - a `Stmt`
        // and an `Item` - and one rule handing back either is a rule that says
        // less about where it may stand.
        rule comptime_item -> Item =
            vis:kw_pub?
            KW_COMPTIME
            name:NAME
            ty:type_annotation?
            "="
            val:expr
            ";"?
            -> {
                Item::Comptime {
                    name,
                    ty,
                    value: val,
                    public: vis.is_some(),
                }
            }

        rule let_stmt -> Stmt =
            KW_LET
            mutable:kw_mut?
            names:let_names
            ty:type_annotation?
            "="
            val:expr
            ";"?
            -> {
                Stmt::Let {
                    names,
                    mutable: mutable.is_some(),
                    ty,
                    value: val
                }
            }

        // **One name, or a flat tuple of them**
        // ([ADR-098](../../../../docs/specification/adr/adr-098.md)).
        //
        // Part I 8.1.2 writes `let (user, rights, prefs) = overlap { … }` and
        // Part II 12.5 writes `let (tx, rx) = channel::bounded(100)`; Part I
        // 2.1 introduces `let` with a name and says nothing about a pattern.
        // So this is a tuple of **names** and not a pattern language: `match`
        // has patterns already and these two sites need neither.
        //
        // Nesting and `_` do not parse, and the note beside a `let`'s parse
        // error is what says so in a sentence rather than in a list of tokens.
        // **`_` is a position of a destructured tuple**
        // ([ADR-126](../../../../docs/specification/adr/adr-126.md) D1), and the
        // single-name form takes it too - so that `let _ = f()` reaches the
        // checker and gets `NK1144`'s sentence rather than a parse error at a
        // character. D2 is a refusal *with a message*, and only the checker can
        // give one.
        //
        // Two `_` in one list are fine: nothing is bound, so nothing collides.
        rule let_names -> Vec<Symbol> =
            one:let_name -> { vec![one] }
          | "(" first:let_name rest:let_name_tail* ")" -> {
                let mut names = vec![first];
                names.extend(rest);
                names
            }

        rule let_name_tail -> Symbol = "," n:let_name -> { n }

        // The ignore pattern carries the symbol `_`, which is what the emitter
        // writes below and what `NK1144` looks for. It is a symbol no `NAME` can
        // produce any more, so nothing else in the compiler can mistake a name
        // for it.
        rule let_name -> Symbol =
            n:NAME -> { n }
          | UNDERSCORE -> { _state.intern("_") }

        rule type_annotation -> Type =
            ":" ty:type_ref -> { ty }

        // Kap 3.3. The head is parsed with the brace-free expression grammar:
        // in `for d in whole { ... }` the brace opens the body, never a struct
        // literal - the same restriction Rust puts on this position.
        // Kap 3.3. Before `expr_stmt` in `stmt`, or `while` would be read as an
        // identifier and the condition as a statement of its own - which is
        // exactly what happened before this rule existed: three statements, no
        // error, and `rustc` complaining about a file nobody wrote.
        rule while_stmt -> Stmt =
            KW_WHILE cond:head_expr body:block ";"?
            -> {
                Stmt::While { cond, body }
            }

        rule for_stmt -> Stmt =
            KW_FOR bindings:for_bindings KW_IN
            iter:head_expr body:block ";"?
            -> {
                Stmt::For { bindings, iter, body }
            }

        // **`_` is a position of a destructured tuple here too** (Part I 2.1,
        // 0.0.250): `for (_, bucket) in order` ignores the half it does not
        // need, as `let (name, _) = pair()` does. It parsed in the `let` and
        // not here, and a name nobody reads is a warning below.
        rule for_bindings -> Vec<Symbol> =
            "(" head:let_name tail:let_name_tail* ")" -> {
                let mut names = vec![head];
                names.extend(tail);
                names
            }
          | n:let_name -> { vec![n] }

        rule ident_tail -> Symbol = "," n:NAME -> { n }

        rule assign_stmt -> Stmt =
            target:postfix_expr op:assign_op value:expr ";"?
            -> { Stmt::Assign { target, op, value } }

        // The compound forms first: a bare `=` would take the first character
        // of `+=` and leave an expression that cannot parse.
        rule assign_op -> Option<BinaryOp> =
            "+=" -> { Some(BinaryOp::Add) }
          | "-=" -> { Some(BinaryOp::Sub) }
          | "*=" -> { Some(BinaryOp::Mul) }
          | "/=" -> { Some(BinaryOp::Div) }
          | "&=" -> { Some(BinaryOp::BitAnd) }
          | "|=" -> { Some(BinaryOp::BitOr) }
          | "^=" -> { Some(BinaryOp::BitXor) }
          | "<<=" -> { Some(BinaryOp::Shl) }
          | ">>=" -> { Some(BinaryOp::Shr) }
          | "=" -> { None }

        // **A statement whose expression is a jump *is* that statement**
        // ([ADR-138](../../../../docs/specification/adr/adr-138.md) D2), and
        // this line is what makes that true rather than nearly true.
        //
        // `break_stmt` and `continue_stmt` are **last** in `stmt`, measured and
        // argued there (740 instructions a statement,
        // [ADR-084](../../../../docs/specification/adr/adr-084.md) D8) — so
        // once D1 put the two words in the expression grammar, `expr_stmt`
        // reached them first and a bare `break` became `Stmt::Expr(Expr::Break)`.
        // Every analysis that asks about a jump asks about the *statement*, so
        // `NK1133` — *nothing after a `break` in the same block is reached*,
        // which is D3's whole safety net — stopped firing.
        //
        // Normalising in the action costs nothing to parse and hands the rest
        // of the compiler the tree it had. The alternative was two failed
        // keyword matches in front of every expression statement, which is the
        // cost that ordering was chosen to avoid.
        rule expr_stmt -> Stmt =
            e:expr ";"? -> {
                match e {
                    Expr::Break => Stmt::Break,
                    Expr::Continue => Stmt::Continue,
                    Expr::Return(value) => Stmt::Return(value.map(|v| *v)),
                    e => Stmt::Expr(e),
                }
            }

        // --- Expressions ---

        pub rule expr -> Expr # "expression" =
            c:closure_expr -> { c }
          | e:catch_expr -> { e }

        // Kap 7.1: `fs::map(path, fs::Root::Anywhere) catch { … }` - the error is `error` inside.
        rule catch_expr -> Expr =
            value:coalesce_expr handler:catch_tail? -> {
                match handler {
                    Some(handler) => Expr::TryCatch { expr: Box::new(value), handler },
                    None => value,
                }
            }

        rule catch_tail -> Block =
            KW_CATCH b:block -> { b }

        // Kap 3.5: `value ?? fallback`.
        rule coalesce_expr -> Expr =
            value:range_expr fallback:coalesce_tail? -> {
                match fallback {
                    Some(fallback) => Expr::Coalesce {
                        value: Box::new(value),
                        fallback: Box::new(fallback),
                    },
                    None => value,
                }
            }

        // **Right-associative**, so `a ?? b ?? c` is `a ?? (b ?? c)`
        // ([ADR-066](../../../docs/specification/adr/adr-066.md)). It used to
        // be `or_expr`, which is *below* this rule in the precedence chain and
        // therefore cannot hold a second `??` - so a chain was a parse error
        // naming the second one, in a language whose page says `??` provides a
        // fallback and never says a value may have only one.
        //
        // The direction is what the types ask for: the last fallback is the
        // plain value that ends the chain, and every `??` before it takes the
        // `T?` on its left. Left-associative would work too, since coalescing
        // is associative - and this is the reading every language with the
        // operator has, which is worth more than a coin toss.
        rule coalesce_tail -> Expr =
            "??" e:coalesce_fallback -> { e }
          // **There is no postfix `??`**
          // ([ADR-165](../../../docs/specification/adr/adr-165.md) D1). Without
          // this the reader of [ADR-018](../../../docs/specification/adr/adr-018.md)
          // D3's own line got *expected one value* and a list of tokens - true,
          // and no help at all to someone who wrote the two characters on
          // purpose. [Part III C.2](../../../docs/specification/30-nikaia-tooling.md)
          // asks for the reason and one concrete way out, and there are three.
          | "??" fail(
                "there is no postfix `??` (ADR-165 D1): `??` is null coalescing and \
                 takes a fallback on its right - `a ?? b` (Part I, 3.5). The same two \
                 characters with nothing after them would mean something else \
                 entirely, and what it would mean is an abort written as punctuation. \
                 The three things a postfix unwrap is reached for each have a \
                 spelling: a fallback value is `a ?? b`; a jump is \
                 `a ?? throw NotFound` or `a ?? return r`, since the four jumps are \
                 expressions (ADR-138 D1); and saying the value is known to be there \
                 is `a ?? panic(\"...\")`, which ends the program with your own words"
            ) e:coalesce_fallback -> { e }

        // **A fallback is one value, or it is bracketed**
        // ([ADR-089](../../../docs/specification/adr/adr-089.md) D1).
        //
        // `??` sits above the whole binary chain, so its fallback used to reach
        // rightwards across every operator there is - and `a ?? 0 > 3` was
        // `a ?? (0 > 3)` while looking like `(a ?? 0) > 3`. That is not a
        // theory: with `a: bool?`, `a ?? x == y` type-checks **both** ways and
        // the two answers differ, measured at `false` against `true`.
        //
        // Taking `unary_expr` rather than `coalesce_expr` is the whole fix: a
        // literal, a name, a call, a field, a `-1` and a bracketed expression
        // are all reachable from there, and no binary operator is. The
        // recursive `coalesce_tail?` keeps `a ?? b ?? c` a chain
        // ([ADR-066](../../../docs/specification/adr/adr-066.md) D4).
        //
        // The `#` label is what keeps the refusal in this language's words:
        // without it the message lists every token that could have followed.
        rule coalesce_fallback -> Expr # "one value, or an expression in brackets" =
            head:unary_expr tail:coalesce_tail? -> {
                match tail {
                    Some(fallback) => Expr::Coalesce {
                        value: Box::new(head),
                        fallback: Box::new(fallback),
                    },
                    None => head,
                }
            }

        // Kap 5.2/5.3: a lambda, with its arguments named or implicit.
        rule closure_expr -> Expr =
            KW_FN "(" params:closure_params? ")" body:block
            -> {
                let (params, mutable) = split_mut(params.unwrap_or_default());
                Expr::Closure { params, mutable, body }
            }
          // `fn { … }` takes **no arguments**, and used to take however many of
          // `a`, `b`, `c` its body mentioned (ADR-049 withdrew that). A body that
          // reaches for one of the three is now a body naming something nothing
          // declares, which `NK1117` refuses - so the form needs no rule of its
          // own to be refused by.
          | KW_FN body:block -> {
                Expr::Closure { params: Vec::new(), mutable: Vec::new(), body }
            }

        // **A lambda's parameter takes `mut`**
        // ([ADR-110](../../../../docs/specification/adr/adr-110.md) D1):
        // `kasse.update fn(mut v) { v += 100 }` changes `v` in place, and the
        // caller whose value changes is the lock. It is
        // [ADR-094](../../../../docs/specification/adr/adr-094.md) D3's word
        // with its meaning unchanged, one position over.
        rule closure_params -> Vec<(bool, Symbol)> =
            head:closure_param tail:closure_param_tail* -> {
                let mut params = vec![head];
                params.extend(tail);
                params
            }

        // **`_` is a lambda's argument** (ADR-126 D1), which is the position
        // [ADR-102](../../../../docs/specification/adr/adr-102.md)'s function
        // types make common: a lambda handed to a callback type of two arguments
        // can ignore one without inventing a name.
        //
        // No `mut` in front of it: there is nothing bound to change.
        rule closure_param -> (bool, Symbol) =
            mutable:kw_mut? name:NAME -> { (mutable.is_some(), name) }
          | UNDERSCORE -> { (false, _state.intern("_")) }

        rule closure_param_tail -> (bool, Symbol) = "," p:closure_param -> { p }

        // Kap 3.3. It binds looser than every operator below it, so `0..<n - 1`
        // is a range ending at `n - 2` rather than a range subtracted from -
        // which is the reading a `for` head wants and the only one that is ever
        // useful.
        //
        // **`..` includes its end and `..<` does not**
        // ([ADR-137](../../../../docs/specification/adr/adr-137.md) D3, D4),
        // which is one spelling per meaning across the whole language: a `for`,
        // a slice and a pattern read the same two operators the same way.
        // Kotlin and Swift are the precedent and both read `..<` as *up to, not
        // including*.
        rule range_expr -> Expr =
            start:or_expr end:range_tail? -> {
                match end {
                    Some((inclusive, end)) => Expr::Range {
                        start: Box::new(start),
                        end: Box::new(end),
                        inclusive,
                    },
                    None => start,
                }
            }

        // `..<` first, and `..=` before the bare `..`: a PEG keeps the first
        // alternative that matches, so `..` would take the two dots of either
        // and leave the third character stranded.
        //
        // **`..=` is a refusal and not a spelling** (D5). It was the inclusive
        // range this language had, and D4 makes `..` that range - so keeping it
        // would be the two-spellings defect of
        // [ADR-140](../../../../docs/specification/adr/adr-140.md) introduced by
        // the record that argues *one meaning per spelling*.
        rule range_tail -> (bool, Expr) =
            "..<" e:or_expr -> { (false, e) }
          | "..=" fail(
                "`..` includes its end, so the inclusive range is written `0..n` \
                 (ADR-137 D3). `..=` was this language's inclusive range and is \
                 withdrawn, because one meaning per spelling is what that record \
                 is for; the range that stops before its end is `0..<n`."
            ) -> { (true, Expr::LitInt(0)) }
          | ".." e:or_expr -> { (true, e) }

        rule or_expr -> Expr =
            head:and_expr tail:or_tail* -> { fold_binary(head, tail) }

        rule or_tail -> (BinaryOp, Expr, Span) @= "||" e:and_expr -> { (BinaryOp::Or, e, _span) }

        rule and_expr -> Expr =
            head:cmp_expr tail:and_tail* -> { fold_binary(head, tail) }

        rule and_tail -> (BinaryOp, Expr, Span) @= "&&" e:cmp_expr -> { (BinaryOp::And, e, _span) }

        rule cmp_expr -> Expr =
            head:bit_or_expr tail:cmp_tail? -> {
                fold_binary(head, tail.into_iter().collect::<Vec<_>>())
            }

        rule cmp_tail -> (BinaryOp, Expr, Span) @= op:cmp_op e:bit_or_expr -> { (op, e, _span) }

        // **The bit operators, below the comparisons and above the sums**
        // ([ADR-248](../../../../docs/specification/adr/adr-248.md) D4): Rust's
        // and Go's order, `|` then `^` then `&` then the shifts. Each one-
        // character operator is kept off its two-character neighbour - `|` off
        // `||` and `|=`, `&` off `&&` and `&=`, `^` off `^=` - so a line that
        // writes the other is read as the other.
        rule bit_or_expr -> Expr =
            head:bit_xor_expr tail:bit_or_tail* -> { fold_binary(head, tail) }

        rule bit_or_tail -> (BinaryOp, Expr, Span) @=
            "|" not("|") not("=") e:bit_xor_expr -> { (BinaryOp::BitOr, e, _span) }

        rule bit_xor_expr -> Expr =
            head:bit_and_expr tail:bit_xor_tail* -> { fold_binary(head, tail) }

        rule bit_xor_tail -> (BinaryOp, Expr, Span) @=
            "^" not("=") e:bit_and_expr -> { (BinaryOp::BitXor, e, _span) }

        rule bit_and_expr -> Expr =
            head:shift_expr tail:bit_and_tail* -> { fold_binary(head, tail) }

        rule bit_and_tail -> (BinaryOp, Expr, Span) @=
            "&" not("&") not("=") e:shift_expr -> { (BinaryOp::BitAnd, e, _span) }

        rule shift_expr -> Expr =
            head:add_expr tail:shift_tail* -> { fold_binary(head, tail) }

        rule shift_tail -> (BinaryOp, Expr, Span) @= op:shift_op e:add_expr -> { (op, e, _span) }

        rule shift_op -> BinaryOp =
            "<<" not("=") -> { BinaryOp::Shl }
          | ">>" not("=") -> { BinaryOp::Shr }

        // `<=` before `<`: the shorter one would win otherwise and leave `=`
        // to be read as an assignment.
        rule cmp_op -> BinaryOp =
            "==" -> { BinaryOp::Eq }
          | "!=" -> { BinaryOp::Ne }
          | "<=" -> { BinaryOp::Le }
          | ">=" -> { BinaryOp::Ge }
          | "<" -> { BinaryOp::Lt }
          | ">" -> { BinaryOp::Gt }

        rule add_expr -> Expr =
            head:mul_expr tail:add_tail* -> { fold_binary(head, tail) }

        rule add_tail -> (BinaryOp, Expr, Span) @= op:add_op e:mul_expr -> { (op, e, _span) }

        rule add_op -> BinaryOp =
            "+" -> { BinaryOp::Add }
          | "-" -> { BinaryOp::Sub }

        rule mul_expr -> Expr =
            head:cast_expr tail:mul_tail* -> { fold_binary(head, tail) }

        rule mul_tail -> (BinaryOp, Expr, Span) @= op:mul_op e:cast_expr -> { (op, e, _span) }

        rule cast_expr -> Expr =
            head:unary_expr casts:cast_tail* -> {
                casts.into_iter().fold(head, |expr, ty| Expr::Cast {
                    expr: Box::new(expr),
                    ty,
                })
            }

        rule cast_tail -> Type = KW_AS ty:type_ref -> { ty }

        rule mul_op -> BinaryOp =
            "*" -> { BinaryOp::Mul }
          | "/" -> { BinaryOp::Div }
          | "%" -> { BinaryOp::Rem }

        // Labelled as well as `expr`, and for the operand rather than for the
        // whole: `1 + ` fails inside `add_tail`, whose operand is a
        // `mul_expr`, so the label on `expr` never sees it. Every operand
        // chain bottoms out here at the position the operand should have
        // started.
        rule unary_expr -> Expr # "expression" =
            // **A `-` written against the digits is part of the number**, and
            // it is first because the alternative below would otherwise take
            // it: `-9223372036854775808` is `i64::MIN`, and as a negation of a
            // positive literal its digits do not fit the type it belongs to.
            // Every other number in the range parsed either way, which is why
            // this was one number missing rather than a hole.
            n:negative_number_lit -> { Expr::LitInt(n) }
          | op:unary_op e:unary_expr -> {
                Expr::Unary { op, expr: Box::new(e) }
            }
          | e:postfix_expr -> { e }

        rule unary_op -> UnaryOp =
            "-" -> { UnaryOp::Neg }
          | "!" -> { UnaryOp::Not }
          // **`ref x` is a view of `x`**
          // ([ADR-184](../../../../docs/specification/adr/adr-184.md) D1), and
          // `&x` is the spelling it replaces. A bare `ref` with no expression
          // after it is not this arm at all, which is what leaves `ref` an
          // ordinary name.
          | KW_REF -> { UnaryOp::Ref }

        rule postfix_expr -> Expr =
            base:primary_expr tail:postfix_tail* -> { fold_postfix(base, tail) }

        // The trailing-lambda form first (Kap 5.2): `.map fn: a.id` has no
        // parentheses, so the plain method rule would stop before the `fn:` and
        // leave it stranded.
        rule postfix_tail -> Postfix =
            // `.route("/x") fn { … }`: arguments *and* a trailing lambda. It
            // has to be tried before the plain call, or the call matches and
            // the lambda is left over - which is the parse error this form did
            // not have a grammar for until ADR-022.
            "." name:SEGMENT args:call_arg_list lambda:trailing_lambda -> {
                let (mut positional, config) = args;
                positional.push(lambda);
                Postfix::Method(name, positional, config)
            }
          | "." name:SEGMENT lambda:trailing_lambda -> {
                Postfix::Method(name, vec![lambda], Vec::new())
            }
          | "." name:SEGMENT args:call_arg_list? -> {
                match args {
                    // Kap 5.1's `;` reaches a method call too, and what stands
                    // after it is kept: ADR-007 D5's deferred parameters arrive
                    // exactly here, and dropping them was why a `dsl` statement
                    // could not be given any.
                    Some((args, config)) => Postfix::Method(name, args, config),
                    None => Postfix::Field(name),
                }
            }
          // Part I 3.5: `?.name`. **One literal** rather than `"?" "."`, so the
          // generator cannot insert the implicit whitespace between them -
          // `a ? . b` is not safe navigation, and `a ?? b` is the coalescing
          // operator, which this cannot begin to match.
          //
          // **And onto a method**, in the same three shapes the plain `.` has
          // and for the same reason: Part I 3.5 says `?.` reaches a *member*,
          // and a method is one ([ADR-066](../../../docs/specification/adr/adr-066.md)).
          // Each of these must be tried before the bare-field arm below, or
          // that one matches the name and leaves the `(` to fail as an empty
          // parenthesised expression - which is what a reader used to get.
          | "?." name:SEGMENT args:call_arg_list lambda:trailing_lambda -> {
                let (mut positional, config) = args;
                positional.push(lambda);
                Postfix::SafeMethod(name, positional, config)
            }
          | "?." name:SEGMENT lambda:trailing_lambda -> {
                Postfix::SafeMethod(name, vec![lambda], Vec::new())
            }
          | "?." name:SEGMENT args:call_arg_list -> {
                let (args, config) = args;
                Postfix::SafeMethod(name, args, config)
            }
          | "?." name:SEGMENT -> { Postfix::SafeField(name) }
          // **`[` on a line of its own is a list literal and not an index**
          // ([ADR-135](../../../../docs/specification/adr/adr-135.md) D3). An
          // index is written on the line its receiver is on; an index across a
          // line break is a shape nobody writes, and reading it as one costs
          // every reader of every list literal a moment of doubt. Rust and
          // JavaScript both chose the other way and both carry the footgun in
          // their style guides.
          | same_line "[" index:expr "]" -> {
                Postfix::Index(Box::new(index))
            }
          // `not("?")`: `??` is the null-coalescing operator (Kap 3.5), and a
          // `t.0` - a tuple's parts are numbered, and the number is a field
          // name like any other, so nothing downstream has to know.
          | "." index:digits -> { Postfix::Field(_state.intern(&index)) }
          // **`value with { field: … }`**
          // ([ADR-118](../../../../docs/specification/adr/adr-118.md) D1): a
          // copy of a value with named fields changed. A postfix rather than a
          // binary operator, because D1's left operand is *any expression of a
          // struct type* - a name, a field, a call - which is exactly what
          // stands to the left of a postfix tail, and because it nests: D2's
          // `p with { pos: p.pos with { x: 1 } }` is this rule twice.
          //
          // The braces are the literal's and so is `field_inits`, which is why
          // the shorthand `user with { name }` costs nothing here.
          | t:with_tail -> { t }


        // `@=` for the byte: the checker records the operand's type under it and
        // the emitter reads it back, because Rust's functional update needs a
        // name this node does not carry.
        //
        // **An empty `{ }` parses and the checker refuses it**, with `NK1174`.
        // D1's reason is about meaning — a copy that changes nothing is a line
        // the reader would puzzle over — so it belongs where meaning is read;
        // and a `fail` here put its caret on the line *after* the braces,
        // because the parser had already consumed them and the implicit
        // whitespace behind them.
        rule with_tail -> Postfix @=
            KW_WITH "{" fields:field_inits? "}" -> {
                Postfix::With(fields.unwrap_or_default(), _span.start)
            }

        // The lambda `closure_expr` reads, in the position where it follows
        // the call instead of sitting inside its parentheses. Its arguments are
        // the ones it names (Kap 5.2); a `fn { … }` takes none.
        //
        // `fn(user) { … }` is not a second lambda form - it is the explicit
        // spelling of the one form (Kap 5.2), so both positions have to accept
        // the same thing, and `closure_params` is shared with `closure_expr`
        // rather than written twice.
        //
        // The named arm first, for the reason `closure_expr` puts it first: a
        // PEG keeps the first alternative that matches, and the bare arm matches
        // the `fn` of `fn(user) { … }` and then fails on the `(` with the
        // parameter list already unreachable. The `fn ":"` arm stays last and
        // still wins at a `fn:`, because neither arm above it can match a colon.
        rule trailing_lambda -> Expr =
            KW_FN "(" params:closure_params? ")" body:block -> {
                let (params, mutable) = split_mut(params.unwrap_or_default());
                Expr::Closure { params, mutable, body }
            }
          | KW_FN body:block -> {
                Expr::Closure { params: Vec::new(), mutable: Vec::new(), body }
            }
            // ADR-022: `fn: expr` was removed, and a form that was in the
            // specification deserves a sentence rather than a parse error at
            // the colon. `fail` beats the alternatives at this position, so
            // this is what a reader gets.
          | KW_FN ":" fail("the `fn: …` form was removed (ADR-022): write `fn { … }`. \
                           Its body ran to the end of the expression, so a `.method()` \
                           after it landed *inside* the lambda - silently") -> {
                Expr::Closure {
                    params: Vec::new(),
                    mutable: Vec::new(),
                    body: Block { stmts: Vec::new() },
                }
            }

        // Kap 5.1: subjects, then a `;`, then options by name. The separator
        // is the whole protocol - what is before it is data and may be
        // positional, what is after it is configuration and may not.
        //
        // **The options-only list comes first**
        // ([ADR-133](../../../../docs/specification/adr/adr-133.md) D1), and it
        // is decided on the second token: an option is `name:` and nothing else
        // in expression position begins that way, so `f(a, b)` fails it at the
        // `,` that follows `a` and the positional alternative takes it.
        //
        // D3's premise is true **since**
        // [ADR-140](../../../../docs/specification/adr/adr-140.md) D1 and was not
        // before it. What used to stand in the way was the other struct literal,
        // Kap 4.2's `Stats(min: first, max: first)`, which is
        // `execute(target_age: 30)` spelled identically and was tried ahead of
        // every call - so this form was read as a literal for a struct nothing
        // declares. D1 takes that spelling out of the language: `Name { … }` is
        // the literal, and `name(field: value)` is a call with options. Where the
        // name **is** a type, `NK1146` says so and names the brace form, because
        // the parser can no longer be the one to tell them apart and does not
        // need to be.
        rule call_arg_list -> (Vec<Expr>, Vec<ConfigArg>) =
            "(" config:bare_config_args ")" -> { (Vec::new(), config) }
          // **And the leading `;` is refused rather than accepted beside it**
          // (D2), which is the signature half's rule reaching the call now that
          // there is a spelling to send a reader to. The cut is what makes this
          // the message: without it the alternative fails, the rule backtracks,
          // and `config_args`' own `;` would parse the old form silently.
          | "(" ";" => fail(
                "an argument list with no subjects writes its options without the `;` \
                 (ADR-133 D1): `execute(target_age: 30)`. The `;` stands between the \
                 two zones of Kap 5.1 - subjects before it, options after - and where \
                 one zone is empty it separates nothing. A *mixed* call keeps it, and \
                 keeps it required."
            ) -> { (Vec::new(), Vec::new()) }
          | "(" args:call_args? config:config_args? ")" -> {
                (args.unwrap_or_default(), config.unwrap_or_default())
            }

        rule bare_config_args -> Vec<ConfigArg> =
            head:config_arg tail:config_arg_tail* ","? -> {
                let mut args = vec![head];
                args.extend(tail);
                args
            }

        rule call_args -> Vec<Expr> =
            head:expr tail:call_args_tail* -> {
                let mut args = vec![head];
                args.extend(tail);
                args
            }

        rule call_args_tail -> Expr = "," e:expr -> { e }

        rule config_args -> Vec<ConfigArg> =
            ";" head:config_arg tail:config_arg_tail* -> {
                let mut args = vec![head];
                args.extend(tail);
                args
            }

        rule config_arg_tail -> ConfigArg = "," a:config_arg -> { a }


        rule config_arg -> ConfigArg =
            name:NAME ":" value:expr -> { ConfigArg { name, value } }

        // Keyword-led forms first, then the struct literal, then a plain path:
        // `Reading { .. }` must be tried before `Reading` on its own, because a
        // PEG keeps the first alternative that matches.
        rule primary_expr -> Expr =
            sp:spawn_expr -> { sp }
          | d:dsl_block_expr -> { d }
          | d:dsl_from_expr -> { d }
          | i:if_expr -> { i }
          | m:match_expr -> { m }
          // Before `struct_lit` and `path_expr`, and both orders matter: a PEG
          // keeps the first alternative that matches, so `seq { … }` would
          // otherwise be read as a struct literal called `seq` - its statements
          // taken for shorthand fields - or as a variable followed by a block
          // of its own, which is the trap the `while` rule records.
          | o:overlap_expr -> { o }
          // Its pair ([ADR-148](../../../../docs/specification/adr/adr-148.md)
          // D4), and here for the same reason: a keyword and a braced body,
          // which a PEG would read as a struct literal called `select`.
          | s:select_expr -> { s }
          // Part III 15.1's other half, and it sits here for `overlap`'s reason
          // ([ADR-124](../../../../docs/specification/adr/adr-124.md) D3): a
          // keyword and a block, which a PEG would otherwise read as a struct
          // literal called `unsafe`. `unsafe` is a reserved word now, so it
          // could not be one - but the ordering is the rule and not the
          // accident.
          | u:unsafe_expr -> { u }
          | s:struct_lit -> { s }
          | b:bool_lit -> { b }
          | n:null_lit -> { n }
          // Before `path_expr`: a PEG keeps the first alternative that matches,
          // and `f"…"` starts with what `NAME` reads as the variable `f`.
          | s:f_str_lit -> { s }
          | p:path_expr -> { p }
          | s:str_lit -> { s }
          | c:char_lit -> { c }
          | f:float_lit -> { f }
          | i:int_lit -> { i }
          | b:block_expr -> { b }
          // **Both parenthesised forms in one alternative**, and it is the
          // alternation's **width** that asks for it: the backend's `alt` takes
          // twenty-two arms, and this rule was at the last one when
          // [ADR-148](../../../../docs/specification/adr/adr-148.md) added
          // `select`. These two are the pair it costs nothing to merge - both
          // begin with a `(`, so the *"also possible here"* list a parse error
          // carries is the same either way. That is not true of two
          // keyword-led forms, which is how this was found: nesting `overlap`
          // and `select` together took the word `overlap` out of six recorded
          // messages.
          | g:parenthesised_expr -> { g }
          | l:list_lit -> { l }
          // **The four jumps, where an expression stands**
          // ([ADR-138](../../../../docs/specification/adr/adr-138.md) D1):
          // `=> throw NotFound`, `?? throw Missing`, `else => return 0`. Their
          // type is never, so they fit every expected type.
          //
          // **Last, and keyword-led**, for the reason `dsl_from_expr` gives
          // three lines down in the head chain: each of the four words is
          // reserved, so no earlier alternative can take one, and an
          // alternative that cannot be taken costs a failed keyword match for
          // every primary in every expression. The statement forms are
          // untouched (D2) — `stmt` tries them first, so a `break` on a line of
          // its own is the statement it always was and `NK1133` still answers
          // about what follows it (D3).
          | j:jump_expr -> { j }

        rule jump_expr -> Expr =
            KW_THROW e:expr -> { Expr::Throw(Box::new(e)) }
          | KW_RETURN e:expr? -> { Expr::Return(e.map(Box::new)) }
          | KW_BREAK -> { Expr::Break }
          | KW_CONTINUE -> { Expr::Continue }

        rule float_lit -> Expr =
            f:FLOAT -> { Expr::LitFloat(f) }

        // Uppercase, so this is lexical: `1 . 5` is not a number, and neither
        // is `1.0 e5`. The exponent is what a program about physical
        // quantities is written in - `9.54791938424326609e-04` beside a `1.0`
        // is the mass of Jupiter, and spelling it out in zeroes is how a digit
        // gets lost. The text is kept as written and handed to the language
        // below, which spells a float literal the same way.
        rule FLOAT -> String =
            w:DIGITS "." f:DIGITS e:EXPONENT? -> {
                format!("{w}.{f}{}", e.unwrap_or_default())
            }
          | w:DIGITS e:EXPONENT -> { format!("{w}{e}") }

        rule EXPONENT -> String =
            "e" "-" d:digit1 -> { format!("e-{d}") }
          | "e" "+" d:digit1 -> { format!("e+{d}") }
          | "e" d:digit1 -> { format!("e{d}") }
          | "E" "-" d:digit1 -> { format!("E-{d}") }
          | "E" "+" d:digit1 -> { format!("E+{d}") }
          | "E" d:digit1 -> { format!("E{d}") }

        // The same set without the two brace-led forms, for the head of an
        // `if` or a `for`, where a `{` is the body.
        //
        // **The chain mirrors the ordinary one level for level**
        // ([ADR-087](../../../../docs/specification/adr/adr-087.md) D1): `??`,
        // range, `||`, `&&`, comparison, `+`, `*`, `as`, unary, postfix,
        // primary. **Every level, with no exception** - the only difference
        // between a head and any other position is in `head_primary`, which
        // drops the forms a `{` begins, and those are reachable through
        // parentheses like anything else (D2).
        //
        // A head that parses a *different language* from the body it introduces
        // is the thing this shape must not become, and it had become exactly
        // that four times over: `&&`, `||`, `??`, `as`, `null` and a tuple were
        // each refused in a position the specification put no restriction on.
        rule head_expr -> Expr =
            value:head_range fallback:head_coalesce_tail? -> {
                match fallback {
                    Some(fallback) => Expr::Coalesce {
                        value: Box::new(value),
                        fallback: Box::new(fallback),
                    },
                    None => value,
                }
            }

        // Right-associative, like `coalesce_tail`
        // ([ADR-066](../../../../docs/specification/adr/adr-066.md) D4).
        // The head's half of [ADR-089](../../../docs/specification/adr/adr-089.md)
        // D1, and it is here because `a_head_parses_what_a_body_parses` caught
        // it: narrowing the body's fallback and not this one would have made a
        // `while a ?? x == y` parse where the same line in a body does not,
        // which is exactly the drift [ADR-076](../../../docs/specification/adr/adr-076.md)
        // put that test there to stop.
        rule head_coalesce_tail -> Expr =
            "??" e:head_coalesce_fallback -> { e }
          // The head's half, for the reason the fallback's narrowing has one:
          // `a_head_parses_what_a_body_parses` would catch the difference.
          | "??" fail(
                "there is no postfix `??` (ADR-165 D1): `??` is null coalescing and \
                 takes a fallback on its right - `a ?? b` (Part I, 3.5). The same two \
                 characters with nothing after them would mean something else \
                 entirely, and what it would mean is an abort written as punctuation. \
                 The three things a postfix unwrap is reached for each have a \
                 spelling: a fallback value is `a ?? b`; a jump is \
                 `a ?? throw NotFound` or `a ?? return r`, since the four jumps are \
                 expressions (ADR-138 D1); and saying the value is known to be there \
                 is `a ?? panic(\"...\")`, which ends the program with your own words"
            ) e:head_coalesce_fallback -> { e }

        rule head_coalesce_fallback -> Expr # "one value, or an expression in brackets" =
            head:head_unary tail:head_coalesce_tail? -> {
                match tail {
                    Some(fallback) => Expr::Coalesce {
                        value: Box::new(head),
                        fallback: Box::new(fallback),
                    },
                    None => head,
                }
            }

        rule head_range -> Expr =
            start:head_or end:head_range_tail? -> {
                match end {
                    Some((inclusive, end)) => Expr::Range {
                        start: Box::new(start),
                        end: Box::new(end),
                        inclusive,
                    },
                    None => start,
                }
            }

        // The head chain's copy of `range_tail`, and the same three
        // alternatives in the same order
        // ([ADR-137](../../../../docs/specification/adr/adr-137.md) D4, D5).
        rule head_range_tail -> (bool, Expr) =
            "..<" e:head_or -> { (false, e) }
          | "..=" fail(
                "`..` includes its end, so the inclusive range is written `0..n` \
                 (ADR-137 D3). `..=` was this language's inclusive range and is \
                 withdrawn, because one meaning per spelling is what that record \
                 is for; the range that stops before its end is `0..<n`."
            ) -> { (true, Expr::LitInt(0)) }
          | ".." e:head_or -> { (true, e) }

        // **Neither connective can begin a block**, which is the whole of why
        // they are safe here: the brace problem is about what may stand
        // *immediately* before the `{`, and after `&&` comes a `head_cmp`,
        // which descends to the same brace-free `head_primary` as everything
        // else in this chain.
        rule head_or -> Expr =
            head:head_and tail:head_or_tail* -> { fold_binary(head, tail) }

        rule head_or_tail -> (BinaryOp, Expr, Span) @= "||" e:head_and -> { (BinaryOp::Or, e, _span) }

        rule head_and -> Expr =
            head:head_cmp tail:head_and_tail* -> { fold_binary(head, tail) }

        rule head_and_tail -> (BinaryOp, Expr, Span) @= "&&" e:head_cmp -> { (BinaryOp::And, e, _span) }

        rule head_cmp -> Expr =
            head:head_add tail:cmp_head_tail? -> {
                fold_binary(head, tail.into_iter().collect::<Vec<_>>())
            }

        rule cmp_head_tail -> (BinaryOp, Expr, Span) @= op:cmp_op e:head_add -> { (op, e, _span) }

        rule head_add -> Expr =
            head:head_mul tail:head_add_tail* -> { fold_binary(head, tail) }

        rule head_add_tail -> (BinaryOp, Expr, Span) @= op:add_op e:head_mul -> { (op, e, _span) }

        rule head_mul -> Expr =
            head:head_cast tail:head_mul_tail* -> { fold_binary(head, tail) }

        rule head_mul_tail -> (BinaryOp, Expr, Span) @= op:mul_op e:head_cast -> { (op, e, _span) }

        // `as` names a type ([ADR-054](../../../../docs/specification/adr/adr-054.md)),
        // and a type is not brace-led either - `i64`, `&str`, `Vec[T]`, `T?`.
        rule head_cast -> Expr =
            head:head_unary casts:cast_tail* -> {
                casts.into_iter().fold(head, |expr, ty| Expr::Cast {
                    expr: Box::new(expr),
                    ty,
                })
            }

        rule head_unary -> Expr =
            op:unary_op e:head_unary -> {
                Expr::Unary { op, expr: Box::new(e) }
            }
          | e:head_postfix -> { e }

        rule head_postfix -> Expr =
            base:head_primary tail:postfix_tail* -> { fold_postfix(base, tail) }

        // **`primary_expr` minus the forms a `{` begins, and nothing else**
        // ([ADR-087](../../../../docs/specification/adr/adr-087.md) D1). What is
        // absent is absent for that one reason and the list is short enough to
        // give in full: `struct_lit`, `block_expr`, `if_expr`, `match_expr`,
        // `overlap_expr`, a `dsl … { … } eod` and a `spawn`, whose lambda is a
        // brace. Each of them is written in a head by putting it in parentheses,
        // which `paren_expr` takes a whole `expr` inside (D2).
        //
        // `dsl_from_expr` is here although its sibling is not: the form it
        // refuses has no brace, and the two are separate rules precisely because
        // one of them is a block and the other is not.
        rule head_primary -> Expr =
            b:bool_lit -> { b }
          // Before `path_expr`, for the reason `primary_expr` gives.
          | s:f_str_lit -> { s }
          | p:path_expr -> { p }
          | s:str_lit -> { s }
          | c:char_lit -> { c }
          | f:float_lit -> { f }
          | i:int_lit -> { i }
          // **After the common ones, and measured there** (D5). `primary_expr`
          // lists `null` beside `false` because they read as a pair; nothing
          // makes that an ordering constraint, since `null` is a reserved word
          // and no other alternative can take one. Third in the list it was one
          // failed match in front of every *name* in every head.
          | n:null_lit -> { n }
          // `(a, b)` before `(a)`, for the reason `primary_expr` gives: a tuple
          // is a parenthesised expression until the comma.
          | t:tuple_expr -> { t }
          | p:paren_expr -> { p }
          | l:list_lit -> { l }
          // **Last, and measured there** (D5). In `primary_expr` this stands
          // near the front because the brace-led forms around it constrain the
          // order; here nothing does - `dsl` is a reserved word, so no other
          // alternative can take one - and at the front it was tried and failed
          // for *every* primary in *every* head, which is a keyword match and
          // its bookkeeping per operand. It also cost the one thing a parse
          // error has: `if { }` answered *"expected one of `!`, `-`, `dsl`,
          // `false`, `true`, identifier"*, leading with the rarest of the six.
          | d:dsl_from_expr -> { d }

        // `(a, b)` before `(a)`: a PEG keeps the first alternative that
        // matches, and a tuple is a parenthesised expression until the comma.
        rule tuple_expr -> Expr =
            "(" head:expr tail:call_args_tail+ ","? ")" -> {
                let mut parts = vec![head];
                parts.extend(tail);
                Expr::Tuple(parts)
            }

        rule paren_expr -> Expr =
            "(" e:expr ")" -> { grouped(e) }

        // A tuple and a grouping, which a reader tells apart by the comma and a
        // PEG tells apart by trying the longer one first.
        rule parenthesised_expr -> Expr =
            t:tuple_expr -> { t }
          | p:paren_expr -> { p }

        // **Part I 2.2's one container, written down**
        // ([ADR-135](../../../../docs/specification/adr/adr-135.md) D1):
        // `[1, 2, 3]`, `[]`, and a trailing comma allowed because a list is
        // what a program grows a line at a time.
        //
        // Nothing else in an expression begins with a `[`, so the position in
        // the alternation is free; it stands beside the other bracketed forms
        // where a reader looks for it.
        //
        // **`@=` because this literal carries its position**
        // ([ADR-152](../../../../docs/specification/adr/adr-152.md) D4): whether
        // it is laid out inline or allocated is its *use*'s answer, the use is
        // the checker's and the lowering is the emitter's, and a statement's
        // byte cannot tell two literals in one statement apart.
        rule list_lit -> Expr @=
            "[" items:list_items? "]" -> {
                Expr::ListLit { items: items.unwrap_or_default(), at: _span.start }
            }

        rule list_items -> Vec<Expr> =
            head:expr tail:call_args_tail* ","? -> {
                let mut items = vec![head];
                items.extend(tail);
                items
            }

        // **Part I 8.2: `spawn fn { … }`, and that is the one spelling.**
        //
        // It used to be `spawn ( expr )`, which the specification called a bug
        // in the parser rather than a second form - so `spawn` takes the
        // trailing lambda of 5.3, the same rule every other lambda position
        // takes, and there is one lambda form in the language.
        //
        // The parenthesised form is kept as a `fail` and not as an alternative,
        // because programs were written against it: the arm reaches further
        // than the lambda arm can at a `(`, so a reader gets the sentence
        // rather than *"expected `fn`"*.
        rule spawn_expr -> Expr =
            KW_SPAWN body:trailing_lambda -> {
                Expr::Spawn { body: Box::new(body), is_move: false }
            }
          | KW_SPAWN "(" fail("`spawn` takes a lambda: write `spawn fn { … }` \
                              (Part I, 8.2). The parenthesised form was a bug in \
                              this parser and not a second form") -> {
                Expr::Spawn {
                    body: Box::new(Expr::Closure {
                        params: Vec::new(),
                        mutable: Vec::new(),
                        body: Block { stmts: Vec::new() },
                    }),
                    is_move: false,
                }
            }

        // Part II, 10.2: a named grammar run over an input, which is a call now
        // Part II, 10.5: a DSL block ends with `} eod`, and the end cannot be
        // found by counting braces - the body is foreign syntax where a `}` may
        // be a string character or absent entirely. So the body is what lies
        // before the marker, taken verbatim; what it *means* is the target
        // grammar's business and is decided when it is lowered.
        rule dsl_block_expr -> Expr =
            KW_DSL name:NAME "{" body:until("} eod") "} eod" -> {
                Expr::Dsl { target: name, context: None, content: body.to_string() }
            }

        // **The form that is gone** ([ADR-082](../../../docs/specification/adr/adr-082.md)
        // D1). A grammar is entered by an ordinary call — `Json.value(input)` —
        // and every `pub` rule is an entry (D2), which is what took the silent
        // choice away: the emitter used to pick the *first* `pub` rule, a
        // `par_fold` one beating an earlier one.
        //
        // `fail` beats the alternatives at this position, the shape ADR-022
        // gave `fn:`: a form the specification taught deserves a sentence
        // rather than a parse error at whatever token happens to be next.
        rule dsl_from_expr -> Expr =
            KW_DSL name:NAME KW_FROM fail("`dsl X from e` was removed (ADR-082): \
                                           a grammar is entered by a call, so write \
                                           `X.rule(e)` naming the rule you mean. \
                                           Every `pub` rule is an entry, and the old \
                                           form picked one of them by source order") -> {
                Expr::Variable(name)
            }

        // Kap 3.4. The value is a `head_expr` for the reason `if`'s condition is
        // one: `match value {` would otherwise read `value { … }` as a struct
        // literal and take the arms for fields.
        rule match_expr -> Expr =
            KW_MATCH value:head_expr "{" arms:match_arm+ "}" -> {
                Expr::Match { value: Box::new(value), arms }
            }

        // **The guard is `if` and it stands on the arm**
        // ([ADR-137](../../../../docs/specification/adr/adr-137.md) D2): the
        // pattern says which values reach the arm and the guard says which of
        // those it takes, so an or-pattern has one guard rather than one per
        // alternative.
        //
        // `head_expr` and not `expr`, for `if`'s own reason one construct over:
        // what follows the condition is `=>` here rather than a `{`, but a
        // struct literal in the guard would still swallow the arm's body if the
        // body is a block, and the head chain is where that is already decided.
        rule match_arm -> MatchArm =
            pattern:match_pattern guard:match_guard? "=>" body:match_arm_body ","? -> {
                MatchArm { pattern, guard, body }
            }

        rule match_guard -> Expr = KW_IF e:head_expr -> { e }

        rule match_arm_body -> Expr =
            b:block -> { Expr::Block(b) }
          | e:expr -> { e }

        // **The catch-all arm is `else`**
        // ([ADR-145](../../../../docs/specification/adr/adr-145.md) D1), and a
        // bare `_` there is refused with the replacement in the message. The
        // last arm is *everything else*, which is what `else` means one
        // construct over and what `_` does not:
        // [ADR-126](../../../../docs/specification/adr/adr-126.md) is careful
        // that `_` means **ignore a value that arrived**, and nothing arrives
        // here.
        //
        // Refused in the parser because this is where the two spellings meet,
        // and `only where no name follows it` is still the `UNDERSCORE` rule's
        // own condition - `_name` is a name and reaches the alternative below.
        // **An or-pattern is the outermost shape**
        // ([ADR-137](../../../../docs/specification/adr/adr-137.md) D1), so it
        // is this rule and the one below is *one* alternative. `(0, y) | (y, 0)`
        // is one arm; a `|` inside a tuple's parts is the same rule one level
        // down, because a part is a whole pattern.
        rule match_pattern -> MatchPattern =
            head:match_alternative tail:match_or_tail+ -> {
                let mut all = vec![head];
                all.extend(tail);
                MatchPattern::Or(all)
            }
          | p:match_alternative -> { p }

        rule match_or_tail -> MatchPattern = "|" p:match_alternative -> { p }

        rule match_alternative -> MatchPattern =
            KW_ELSE -> { MatchPattern::Otherwise }
          | UNDERSCORE fail(
                "a `match`'s catch-all arm is written `else` (ADR-145 D1): \
                 `else => \"other\"`. `_` is the ignore pattern and says something \
                 different - it ignores a value that *arrived*, in a tuple \
                 position or a parameter (ADR-126 D1) - and nothing arrives at \
                 the arm taken when none of the others matched. `else` is what \
                 that is called one construct over, and this language already \
                 has it."
            ) -> { MatchPattern::Otherwise }
          // **A range before a bare literal**, or the literal takes the start
          // and leaves the `..` stranded. Inclusive at both ends (D3), and
          // `..<` is deliberately not here: a pattern is one shape, and an
          // exclusive range is written by moving the end (D4).
          | start:pattern_lit ".." end:pattern_lit -> {
                MatchPattern::Range { start, end }
            }
          | pattern_lit "..<" fail(
                "`..<` is not written in a pattern (ADR-137 D4). A pattern's \
                 range includes both ends, and one that stops earlier is \
                 written by moving the end: `200..298`."
            ) -> { MatchPattern::Literal(Expr::LitInt(0)) }
          | l:pattern_lit -> { MatchPattern::Literal(l) }
          // **A bare tuple**, which names no type: `(0, 0)`.
          | "(" parts:match_part_list ")" -> {
                MatchPattern::Tuple { path: Vec::new(), parts }
            }
          | path:pattern_path "(" parts:match_part_list ")" -> {
                MatchPattern::Tuple { path, parts }
            }
          | path:pattern_path "{" bindings:ident_list rest:named_rest? "}" -> {
                MatchPattern::Named { path, bindings, rest: rest.is_some() }
            }
          // `Point { .. }` - every field, none of them named.
          | path:pattern_path "{" ".." "}" -> {
                MatchPattern::Named { path, bindings: Vec::new(), rest: true }
            }
          | path:pattern_path -> { MatchPattern::Path(path) }

        // `, ..` - the fields this pattern does not name (D1).
        rule named_rest = "," ".."

        // **A part is a whole pattern**, which is what makes a pattern nest:
        // `Event::Click(Point { x, .. })` is a tuple whose one part is a named
        // pattern ([ADR-137](../../../../docs/specification/adr/adr-137.md) D1).
        rule match_part_list -> Vec<MatchPattern> =
            head:match_part tail:match_part_tail* ","? -> {
                let mut parts = vec![head];
                parts.extend(tail);
                parts
            }

        rule match_part_tail -> MatchPattern = "," p:match_part -> { p }

        // **`_` is the ignore pattern inside a pattern**
        // ([ADR-145](../../../../docs/specification/adr/adr-145.md) D2): a
        // part is a value that arrived, which is what `_` ignores - `(0, _)`,
        // `Shape::Circle(_)`. It was refused with the catch-all arm's message,
        // which itself says `_` is for a tuple position.
        rule match_part -> MatchPattern =
            UNDERSCORE -> { MatchPattern::Otherwise }
          | p:match_pattern -> { p }

        // --- Keywords ---
        //
        // **A word keyword must not match the beginning of a longer word.** This
        // grammar is scannerless (ADR-001 D2): there is no lexer deciding where a
        // word ends, so a bare `"as"` matched the first two characters of
        // `assert` and left `sert` behind as a type name. Measured, and all of it
        // silent:
        //
        //     true asfoo     ->  `true as foo`
        //     returnx        ->  `return x`
        //     assert c       ->  `let c = true as sert; c;`
        //     forx in 0..3   ->  `for x in 0..3`
        //
        // The last one is the shape that matters most: it is a **valid program
        // with a different meaning**, because it binds `x` where the source says
        // `forx`. The others end as `rustc` errors about a name nobody wrote,
        // which is the Part III C.1 class. Either way the source said one thing
        // and the compiler read another.
        //
        // `not(ident)` is the boundary, and it is the whole fix: it consumes
        // nothing and demands that what follows the word cannot continue it. One
        // rule per keyword because the generator has no parameters - and
        // UPPERCASE, which is not a style choice: a lowercase rule is syntactic,
        // so the generator would insert the implicit whitespace *between* the
        // word and the boundary, and `as i32` would then be refused for having a
        // space in it.
        //
        // A digit and an underscore continue a word too (`as2`, `as_of`), and the
        // backend's `ident` accepts both, so they are covered by the same line.
        rule KW_AS = "as" not(ident)
        rule KW_BOUNDARY = "boundary" not(ident)
        rule KW_BREAK = "break" not(ident)
        rule KW_CATCH = "catch" not(ident)
        rule KW_COMPTIME = "comptime" not(ident)
        rule KW_TEST = "test" not(ident)
        rule KW_CONTINUE = "continue" not(ident)
        rule KW_DSL = "dsl" not(ident)
        rule KW_ELSE = "else" not(ident)
        rule KW_ENUM = "enum" not(ident)
        rule KW_EXTERN = "extern" not(ident)
        rule KW_FALSE = "false" not(ident)
        rule KW_FN = "fn" not(ident)
        rule KW_FOLD = "fold" not(ident)
        rule KW_FOR = "for" not(ident)
        // **Not in `RESERVED`** ([ADR-116](../../../../docs/specification/adr/adr-116.md)
        // D1): `from` is an ordinary name, and this rule exists only so the
        // removed `dsl X from e` still gets its sentence rather than a parse
        // error at whatever token happens to be next. A token matcher and a
        // reserved word are two different things, and this is the first.
        rule KW_FROM = "from" not(ident)
        rule KW_GRAMMAR = "grammar" not(ident)
        rule KW_IF = "if" not(ident)
        rule KW_IMPL = "impl" not(ident)
        rule KW_IN = "in" not(ident)
        rule KW_LET = "let" not(ident)
        rule KW_MATCH = "match" not(ident)
        rule KW_MUT = "mut" not(ident)
        rule KW_NULL = "null" not(ident)
        rule KW_OVERLAP = "overlap" not(ident)
        rule KW_PAR_FOLD = "par_fold" not(ident)
        rule KW_PUB = "pub" not(ident)
        rule KW_RETURN = "return" not(ident)
        rule KW_RULE = "rule" not(ident)
        rule KW_SELECT = "select" not(ident)
        rule KW_SELF = "self" not(ident)
        rule KW_SPAWN = "spawn" not(ident)
        rule KW_STRUCT = "struct" not(ident)
        rule KW_SYNC = "sync" not(ident)
        rule KW_THROW = "throw" not(ident)
        rule KW_THROWS = "throws" not(ident)
        rule KW_TRAIT = "trait" not(ident)
        rule KW_TRUE = "true" not(ident)
        rule KW_UNCHECKED = "unchecked" not(ident)
        rule KW_UNSAFE = "unsafe" not(ident)

        // **`ref X` is a view of an `X`**
        // ([ADR-184](../../../../docs/specification/adr/adr-184.md) D1), which
        // is the word this language writes where the one below writes `&`.
        //
        // **In `RESERVED`, and the corpus is not what decided it.** Counting
        // `ref` over every `.nika` file gave **zero** uses, which says the word
        // is free — and it is not: a call written `ref(x)` is a borrow of `(x)`
        // to this grammar and a call to a function named `ref` to its author,
        // and the two are the same characters. A word that is a name in one
        // position and an operator in the next is worse than a reserved one,
        // because the reading nobody meant is the **silent** one
        // ([ADR-010](../../../../docs/specification/adr/adr-010.md) D1's
        // polarity). `crates/nikaia/tests/reserved_below.rs` is what found it,
        // by writing the word in every position a name may stand.
        rule KW_REF = "ref" not(ident)

        // The four words of `opaque type T released by f`
        // ([ADR-147](../../../../docs/specification/adr/adr-147.md) D3). They
        // are **not** in `RESERVED`: each is a name everywhere else, and this
        // position is the only one that asks for it.
        rule KW_OPAQUE = "opaque" not(ident)
        rule KW_TYPE = "type" not(ident)
        rule KW_RELEASED = "released" not(ident)
        rule KW_BY = "by" not(ident)
        rule KW_USE = "use" not(ident)
        rule KW_WHILE = "while" not(ident)
        rule KW_WITH = "with" not(ident)

        // **Every reserved word, in one rule** (ADR-051).
        //
        // UPPERCASE for the same reason the `KW_` rules are: this is lexical,
        // and a lowercase rule would let the generator insert the implicit
        // whitespace inside it.
        //
        // **The list is the Nikaia level and nothing else.** `rule`,
        // `boundary`, `fold`, `par_fold` and `unchecked` are the grammar
        // sublanguage's vocabulary - they are keywords inside a `grammar`
        // block and words a program may want everywhere else, so they are not
        // here. `overlap` is here and the grammar has no construct for it yet
        // ([ADR-050](../../../../docs/specification/adr/adr-050.md) D2):
        // reserving a word costs nothing before programs exist and breaks them
        // afterwards, so the free moment is now.
        //
        // **`self` is deliberately absent**, and that is not an oversight. It
        // is the one keyword that *is* a name - `self.min` refers to it - and
        // `NAME` is the rule both for declaring a name and for referring to
        // one, so excluding it here would refuse every method body in the
        // repository. Declaring it is refused where the declaration is, by the
        // checker, which can say what is wrong.
        //
        // Order does not matter: every alternative carries its own `not(ident)`
        // boundary, so `throw` does not match the start of `throws`.
        // **Split in three** because the alternation the backend generates is a
        // tuple, and a tuple has a width the library implements `Alt` up to.
        // Twenty-eight was over it, which is what forced the first split;
        // thirty-one is further over. Nothing else distinguishes the parts.
        //
        // **And every arm hands back a `0` that nothing reads**, which is not a
        // style choice either: an alternation with no action generates a unit
        // expression per arm, and `clippy::unused_unit` refuses the whole macro
        // expansion for it. A value is the smallest thing that is not a unit.
        rule RESERVED -> u8 =
            w:RESERVED_A -> { w } | w:RESERVED_B -> { w } | w:RESERVED_C -> { w }

        rule RESERVED_A -> u8 =
            KW_AS -> { 0 }
          | KW_REF -> { 0 }
          | KW_CATCH -> { 0 }
          | KW_DSL -> { 0 }
          | KW_ELSE -> { 0 }
          | KW_ENUM -> { 0 }
          | KW_FALSE -> { 0 }
          | KW_FN -> { 0 }
          | KW_FOR -> { 0 }
          | KW_GRAMMAR -> { 0 }
          | KW_IF -> { 0 }
          | KW_IMPL -> { 0 }
          | KW_IN -> { 0 }
          | KW_LET -> { 0 }

        rule RESERVED_B -> u8 =
            KW_MATCH -> { 0 }
          | KW_MUT -> { 0 }
          | KW_NULL -> { 0 }
          | KW_OVERLAP -> { 0 }
          | KW_PUB -> { 0 }
          | KW_RETURN -> { 0 }
          | KW_SPAWN -> { 0 }
          | KW_STRUCT -> { 0 }
          | KW_SYNC -> { 0 }
          | KW_THROW -> { 0 }
          | KW_THROWS -> { 0 }
          | KW_TRUE -> { 0 }
          | KW_USE -> { 0 }
          | KW_WHILE -> { 0 }

        // **Four words left this half**
        // ([ADR-117](../../../../docs/specification/adr/adr-117.md) D1): `loop`,
        // `const`, `macro` and `quote` are names now. They were reserved
        // *against* the possibility of a construct rather than for one, which is
        // the ground [ADR-051](../../../../docs/specification/adr/adr-051.md) D1
        // asks for — and reserving a word buys exactly one thing, which is the
        // sentence a reader who writes it gets. `NK1117` can say that sentence
        // about a name, so the words were paying for nothing (D2).
        //
        // **`with` stays because it is about to mean something**
        // ([ADR-118](../../../../docs/specification/adr/adr-118.md)), which is D3:
        // reserved *for* a construct is the one ground that holds.
        //
        // **`trait` is the one here that has a construct**
        // ([ADR-078](../../../../docs/specification/adr/adr-078.md) D1): it is in
        // this half because `RESERVED_B` is where the alternation's width broke
        // last time, not because nothing uses it. It is also the one word here
        // that was found as a *name* rather than reserved on purpose — `let trait
        // = 3` was a legal program that `rustc` refused about the generated file
        // ([ADR-076](../../../../docs/specification/adr/adr-076.md) §1).
        rule RESERVED_C -> u8 =
            KW_BREAK -> { 0 }
          | KW_COMPTIME -> { 0 }
          | KW_CONTINUE -> { 0 }
          | KW_TRAIT -> { 0 }
          | KW_WITH -> { 0 }
          // **Reserved with their constructs**
          // ([ADR-124](../../../../docs/specification/adr/adr-124.md) D1), which
          // is what tells them from the four above: a word reserved so that a
          // reader can be told something is one `NK1117`'s help can tell them
          // about instead ([ADR-117](../../../../docs/specification/adr/adr-117.md)).
          // The number that allowed it is **zero** - nothing in `examples/`, in
          // `tests/` or in the three pages writes either as a name.
          | KW_EXTERN -> { 0 }
          | KW_UNSAFE -> { 0 }
          // **And the word this record spends**
          // ([ADR-148](../../../../docs/specification/adr/adr-148.md) D1).
          // `overlap` was reserved before it had a construct; `select` gets
          // both at once, which is the case D3 of
          // [ADR-051](../../../../docs/specification/adr/adr-051.md) asks for.
          // The number that allowed it is **zero**: nothing in `examples/`, in
          // `tests/` or in the three pages writes it as a name.
          | KW_SELECT -> { 0 }
          // **`test`, with its construct**
          // ([ADR-245](../../../../docs/specification/adr/adr-245.md) D1).
          // Measured when the record was written: no name in the tree's
          // `.nika` files is `test`.
          | KW_TEST -> { 0 }


        // The compiler's identifier.
        //
        // The backend's `ident` accepts a **leading digit** - `1` is an
        // identifier to it, and a grammar that wants otherwise says so, which
        // is what this rule is. Without it `1.5` parses as the field `5` of a
        // variable called `1`: harmless while the emitted text happens to read
        // back the same, and wrong the moment there is more after it -
        // `1.5e-4` came out as `1.5e - 4`, and `(2.0).sqrt()` would have been
        // a field access too.
        //
        // `not(digit)` consumes nothing and demands nothing, so it costs a
        // character comparison at the start of every name.
        //
        // **`not(RESERVED)` is the same shape and answers `open-decisions.md`
        // §7.** A reserved word is not a name, so `let fn = 3` - which used to
        // lower to `let fn = 3;` and be refused by `rustc` about the generated
        // file (Part III, C.1) - does not parse as a `let` of a name at all.
        // **A bare `_` is not a name**
        // ([ADR-126](../../../../docs/specification/adr/adr-126.md) D1, D2): it
        // is the ignore pattern, it stands in the three positions that rule names
        // and nowhere else, and `x + _` therefore does not parse. It used to be an
        // ordinary name, which is how `let _ = f()` compiled and lowered to Rust's
        // own `_` - a value *discarded* where the source said *bound*, and for a
        // file handle or a lock guard that is a different program
        // (`open-work.md` found it and the record answered it).
        //
        // `_name`, `_0` and the `_000` of `1_000` stay names, which is what the
        // two lookaheads buy: a bare `_` is one not followed by an identifier or
        // by a digit.
        rule NAME -> Symbol = not(digit) not(RESERVED) not(UNDERSCORE) n:ident -> { n }

        rule UNDERSCORE = "_" not(raw_ident) not(digit)

        rule pattern_lit -> Expr =
            b:bool_lit -> { b }
          | s:str_lit -> { s }
          | c:char_lit -> { c }
          | n:int_lit -> { n }

        rule pattern_path -> Vec<Symbol> =
            head:NAME tail:path_segment* -> {
                let mut path = vec![head];
                path.extend(tail);
                path
            }

        rule ident_list -> Vec<Symbol> =
            head:NAME tail:ident_tail* -> {
                let mut names = vec![head];
                names.extend(tail);
                names
            }

        rule if_expr -> Expr =
            KW_IF cond:head_expr then_branch:block otherwise:else_branch?
            -> {
                Expr::If {
                    cond: Box::new(cond),
                    then_branch,
                    else_branch: otherwise,
                }
            }

        // **`else if` is an `else` whose block holds one `if`**
        // ([ADR-132](../../../../docs/specification/adr/adr-132.md) D1), with
        // that block's braces left out. Nothing is added to the language: the
        // chain *is* an `if` inside an `if`, so every rule of `if` holds at every
        // link - the condition's brace rule, the branches agreeing on one type
        // where the value is taken, a `return` leaving the function.
        //
        // `else if` is two words with whitespace between them and not a keyword,
        // which is what makes this one alternative rather than a word on the
        // reserved list ([ADR-084](../../../../docs/specification/adr/adr-084.md)
        // is what a keyword costs). The braced form comes first, because an
        // `else { … }` whose block *begins* with an `if` is a block and must stay
        // one - `{` cannot begin an `if`, so the order is a statement about
        // reading rather than a trap, and it keeps the plain form the cheap one.
        rule else_branch -> Block @=
            KW_ELSE b:block -> { b }
          | KW_ELSE i:if_expr -> {
                Block { stmts: vec![Spanned::new(Stmt::Expr(i), _span)] }
            }

        // Blocks are expressions (Part I, 3.1).
        rule block_expr -> Expr =
            b:block -> { Expr::Block(b) }

        // Part I 8.1.1: `seq { … }` states an order the compiler cannot see
        // (ADR-033 D7). The statements inside keep the order they were written
        // in, whatever their touch sets say - which is why it is a block and
        // not an attribute on a statement: what it constrains is a *sequence*.
        //

        // Part I 8.1.2: `overlap { … }`, where each statement is a branch
        // (ADR-050 D2). The same shape `seq` has, and deliberately so - both are
        // a keyword and a block, and the difference is entirely in what they
        // mean.
        rule overlap_expr -> Expr =
            KW_OVERLAP b:block -> { Expr::Overlap(b) }

        // Part II 12.4: `select { … }`, where each **arm** is a branch and the
        // first to finish wins ([ADR-148](../../../../docs/specification/adr/adr-148.md)
        // D1).
        //
        // Not a block, which is the difference from `overlap` one rule up and
        // the whole of why this costs a keyword: an arm binds a name and then
        // runs a block, and no function parameter can be given that shape.
        rule select_expr -> Expr =
            KW_SELECT "{" arms:select_arm+ "}" -> { Expr::Select(arms) }


        // `head_expr` and not `expr`, for the reason `match`'s arms give: what
        // follows is `=>`, and a struct literal in the raced expression would
        // swallow the arm's body.
        rule select_arm -> SelectArm @=
            binding:select_binding "=" value:head_expr "=>" body:block ","?
                -> {
                    SelectArm {
                        binding,
                        value,
                        body,
                        at: _span.start,
                    }
                }

        // `_` is [ADR-126](../../../../docs/specification/adr/adr-126.md) D1's
        // ignore pattern: a value **arrives** and is not wanted. It is tried
        // first, because `NAME` would otherwise have to decline it.
        rule select_binding -> Option<Symbol> =
            UNDERSCORE -> { None }
          | n:NAME -> { Some(n) }

        // Part III 15.1: `unsafe { … }`, the one place a call to an `extern`
        // name may stand ([ADR-124](../../../../docs/specification/adr/adr-124.md)
        // D3). A block with a value and no other rule - what is inside is
        // checked exactly as anything else is, and what the word buys is that
        // the boundary is visible *at the call*.
        rule unsafe_expr -> Expr =
            KW_UNSAFE b:block -> { Expr::Unsafe(b) }

        rule struct_lit -> Expr =
            name:type_name "{" fields:field_inits "}" -> {
                Expr::StructLit { name, fields }
            }

        rule field_inits -> Vec<FieldInit> =
            head:field_init tail:field_init_tail* ","? -> {
                let mut fields = vec![head];
                fields.extend(tail);
                fields
            }

        rule field_init_tail -> FieldInit = "," f:field_init -> { f }

        rule field_init -> FieldInit =
            name:NAME ":" value:expr -> {
                FieldInit { name, value: Some(value) }
            }
          | name:NAME -> { FieldInit { name, value: None } }

        // `Summary::new` is a path; `println(...)` a call; `acc` a variable.
        //
        // The trailing lambda belongs here as well as on a method (Kap 5.3):
        // `access_all(a, b) fn(x, y) { … }` (Part II, 12.3) and `task::scope
        // fn(s) { … }` (Part II, 12.7) are calls whose last argument stands
        // outside the parentheses, and without this the lambda is a statement
        // of its own and the call is made with one argument fewer - which
        // type-checks in Rust often enough to be a different program rather
        // than an error.
        //
        // A lambda with no parentheses before it is the *only* argument, so
        // `task::scope fn(s) { … }` is a call even though `task::scope` on its
        // own is a path. Nothing else can follow a path here, so the optional
        // lambda cannot take a `{` that belonged to something else: it has to
        // start with the keyword `fn`.
        rule path_expr -> Expr =
            head:NAME tail:path_segment* args:call_arg_list?
            lambda:trailing_lambda?
            -> {
                let mut segments = vec![head];
                segments.extend(tail);
                let base = if segments.len() == 1 {
                    Expr::Variable(segments[0])
                } else {
                    Expr::Path(segments)
                };
                match (args, lambda) {
                    (Some((mut args, config)), lambda) => {
                        args.extend(lambda);
                        Expr::Call {
                            func: Box::new(base),
                            args,
                            config,
                        }
                    }
                    (None, Some(lambda)) => Expr::Call {
                        func: Box::new(base),
                        args: vec![lambda],
                        config: Vec::new(),
                    },
                    (None, None) => base,
                }
            }

        rule bool_lit -> Expr =
            KW_TRUE -> { Expr::LitBool(true) }
          | KW_FALSE -> { Expr::LitBool(false) }

        // Part I 2.3. Beside `bool_lit` because it is the same kind of thing: a
        // word the grammar knows, which is why it is a reserved word
        // ([ADR-051](../../../../docs/specification/adr/adr-051.md) D1) - read
        // as a name it would be `NK1117`, and read as a name that *is* declared
        // it would be a different program.
        rule null_lit -> Expr = KW_NULL -> { Expr::LitNull }

        // Kap 2.5. `f` before the quote is what makes a string *code* - without
        // it the braces are braces (ADR-035). UPPERCASE, so the `f` and the
        // quote are one token: `f "x"` with a space is the variable `f`
        // followed by a string, and reading it as an interpolation would make
        // whitespace change what a program means.
        rule FSTRING -> String =
            "f\"" parts:STR_CHAR* "\"" -> { parts.concat() }

        // `@=` for the position, as `list_lit` has it and for its reason: what
        // the literal lowers to is its use's answer (ADR-207 D2).
        rule str_lit -> Expr @=
            s:STRING -> { Expr::LitStr { text: s, at: _span.start } }

        // Its own rule rather than an alternative inside `str_lit`, and the
        // reason is the error message: a rule whose body is one sequence
        // reports what it can start with, so `"` stays in the "also possible
        // here" list and `f` joins it. Folded into `str_lit` both disappear,
        // which Part III C.2 would not forgive.
        //
        // **Not reachable from `literal_expr` or `pattern_lit`**, and
        // deliberately: a Kap 5.1 default and a `match` pattern are constants,
        // and `f"…"` is a call to `format!`. The grammar is where that is said.
        rule f_str_lit -> Expr =
            s:FSTRING -> { Expr::LitInterpolated(s) }

        // Kap 2.2. Lexical, and the body is kept as written - a `'\n'` is two
        // characters here and one in the value, and the language below reads
        // the same two. Deciding what they mean would be decoding done twice.
        rule CHAR -> String =
            "'" c:CHAR_BODY "'" -> { c }

        rule CHAR_BODY -> String =
            "\\" c:any -> {
                let mut s = String::from("\\");
                s.push(c);
                s
            }
          | not("'") c:any -> { c.to_string() }

        rule char_lit -> Expr =
            c:CHAR -> { Expr::LitChar(c) }

        // **The four spellings are one rule and it is written in Rust**
        // ([ADR-136](../../../../docs/specification/adr/adr-136.md)): see
        // `number_lit` above for why.
        rule int_lit -> Expr =
            n:number_lit -> { Expr::LitInt(n) }

        // **A tuple's part is a plain run of digits** and takes neither form:
        // `t.0` is a *field name* that happens to be a number
        // ([ADR-098](../../../../docs/specification/adr/adr-098.md)), so a
        // separator in it would be a separator in a name.
        rule digits -> String =
            d:digit1 -> { d.to_string() }

        // A run of decimal digits with D1's separator in it, and the separator
        // is not in what comes back (D3). What `FLOAT` counts in: `1_000.5` is
        // a number and `1_000` beside `.5` is two.
        rule DIGITS -> String =
            head:digit1 rest:DIGIT_GROUP* -> {
                let mut all = head.to_string();
                for group in rest {
                    all.push_str(&group);
                }
                all
            }

        rule DIGIT_GROUP -> String = "_" d:digit1 -> { d.to_string() }
    }
}
