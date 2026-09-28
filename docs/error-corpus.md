# The error corpus

Twenty-six broken `.nika` files and what the compiler says about each. It exists
to be argued with: a message is only wrong against a claim about what a reader
needed, and this is where those claims are written down before anything is
changed.

It exists for a second reason too. Three typos measured by hand is not a corpus,
and an intermediate attempt at the expectation ranking made one case
much worse — seventeen expectations in the headline, twenty-two in a note, the
position on a token that was correct — which was caught by accident rather than
by a test. Every path taken from here should show its effect on all
twenty-six at once.

**Two columns, because there are two states.** *Before* is `winnow-grammar`
`024e3d3` — what a user got when this corpus was written. *Today* is `9180b3d`
and is what a user gets now. Six changes lie between them, and each was
measured against this file:

1. **an expectation is ranked by whether the grammar required it**
   (winnow-grammar#4) — the whitespace skip stops speaking for the grammar;
2. **a rule may name itself**, `rule expr -> Expr # "expression" = …`
   (winnow-grammar#5) — a dozen token spellings become one word, and Nikaia's
   own grammar labels `expr`, `unary_expr`, `stmt`, `item` and `type_ref`;
3. **an element that *began* is not an optional continuation**
   (winnow-grammar#6) — an unfinished item's missing `}` outranks the
   continuations of the expression before it;
4. **a losing alternative keeps its error, and between two requirements the
   one open longest leads** (winnow-grammar#8) — what a shorter parse
   abandoned is no longer lost, and a guess made a token ago no longer
   outranks the structure the reader is inside;
5. **what fails inside a lookahead is a test that said no**
   (winnow-grammar#10) — a `peek(…)` demands nothing, so its failure is not an
   expectation, and the grammar can use one to tell a repetition bound apart
   from a brace group without the test ending up in every message.
6. **the message shows the line it is about, with a caret under the token**
   (winnow-grammar#11) — the last finding this corpus turned up on its own, and
   the only one that applies to all twenty-six rows at once.

A message today can carry a second line, `note: also possible here: …`, holding
what the grammar would have accepted but did not require. The columns below
quote the headline, which is the part a reader acts on; where the note matters
to a row, the row says so.

`✅` says what a reader needs · `⚠️` does not · `○` does not fail at all

---

## A. A separator or terminator is missing

| # | input | the reader needs | before | today |
| :-- | :--- | :--- | :--- | :--- |
| A1 | `struct S { name: ref String` ⏎ `temp: i32 }` | `,` or `}` | ⚠️ `` `//`, whitespace `` | ✅ ``expected `}` ``, `,` in the note |
| A2 | `fn f(a: i32 b: i32) {}` | `,` or `)` | ⚠️ `` `//`, whitespace `` | ✅ ``expected `)` ``, `,` in the note |
| A3 | `fn f() {` ⏎ `let x = 1` | `}` at end of input | ⚠️ 17 tokens | ✅ ``expected `}` `` |
| A4 | `let xs = [1, 2` | `,` or `]` | ⚠️ `` `//`, whitespace `` | ✅ ``expected `]` ``, `,` in the note |
| A5 | `struct S { a: i32,, b: i32 }` | a field name | ⚠️ `` `//`, whitespace `` | ✅ ``expected `}` ``, identifier in the note |

A1 and A2 are the honest answers rather than the ideal ones, and worth saying
plainly: at that position the grammar does **not** require a comma. It requires
`}` (or `)`) and would accept another field — the `,` is in the note, and
guessing which the author meant is not on offer.

A3 is the row that moved furthest. It was seventeen expectations in the
headline and twenty-two in a note; the brace it needs was in the *note*,
because `program = item*` makes every item optional and the unfinished item's
error was recorded as one more "something else could have gone here". An
element that read four tokens and did not finish is now a requirement, and the
brace leads.

A4 **moved when the list literal was built**
([ADR-135](specification/adr/adr-135.md)). It used to be the clearest remaining
case of the whitespace skip winning on *progress* — tried at the start of every
rule, so at a position no real parser reached it was trivially the furthest
thing that failed — because nothing in the grammar began with a `[` and the
expression rule failed there. Now `list_lit` reads the `[`, gets as far as the
end of the line and requires its `]`, so the requirement leads and the `,` is
in the note beside it: the same shape A1 and A2 have, and for the same reason.

## B. An operand is missing

| # | input | the reader needs | before | today |
| :-- | :--- | :--- | :--- | :--- |
| B1 | `let y = ` | **`expected expression`** | ⚠️ `` `//`, whitespace `` | ✅ `expected expression` |
| B2 | `let y = 1 + ` | `expected expression` | ⚠️ 8 tokens | ✅ `expected expression` |
| B3 | `if { }` | `expected expression` | ○ **parses** | ⚠️ refused, token soup |
| B4 | `f(1, )` | `expected expression` | ⚠️ 8 tokens | ✅ `expected expression` |
| B5 | `let x: = 1` | `expected type` | ⚠️ `` `//`, whitespace `` | ✅ `expected type` |

B2, B4 and B5 are what the label bought. `expr`, `unary_expr` and `type_ref`
name themselves in the compiler's own grammar, so the list of spellings each
could have started with is replaced by the word for what belongs there. B2 needs
*two* labels to come out right and says why: `1 + ` fails inside `add_tail`,
whose operand is a `mul_expr`, so the label on `expr` never sees it — every
operand chain bottoms out at `unary_expr`, and that is where the second label
sits.

B1 does not move, and it is the last member of the trivia group: the failure is
reported at the `}` on the line *after* the missing operand, where the
whitespace skip reached further than any real parser. Progress is decided before
any ranking, so no label helps.

## C. The wrong token where the grammar knows what belongs

| # | input | the reader needs | before | today |
| :-- | :--- | :--- | :--- | :--- |
| C1 | `struct S { name ref String }` | `:` | ⚠️ `` `//`, whitespace `` | ✅ ``expected `:` `` |
| C2 | `rule A -> i32 = n:digit+ -> { n }` | *the arrow is gone* | ⚠️ `` `//`, whitespace `` | ✅ the sentence, naming `pattern { action }` |
| C3 | `fn main( {` | `)` or a parameter | ⚠️ `` `//`, whitespace `` | ✅ ``expected `)` `` |
| C4 | `let 5 = x` | a name | ○ **parses** | ⚠️ refused, ``expected `mut` `` |
| C5 | `impl S { struct T {} }` | a method | ⚠️ `` `//`, whitespace `` | ✅ ``expected `}` ``, `fn`/`pub` in the note, and `struct` named as reserved |

C1 is the best row in the corpus and worth keeping as the example: `expected ':'`
is exactly what a reader can act on.

C2 was a grammar question *and* a message question, and needed both answered.
Its input was `rule A -> i32 = n:digit1 { n }` — an action block whose `->` was
forgotten — and the grammar read the brace as a repetition bound, so `digits`
was true of the parser and no help. A brace group is a bound only when its
content starts with a digit, which is the rule the backend already states for
the same ambiguity, and `peek(("{" digit))` is how a grammar says it. That alone
reported `expected a digit` for every brace group that is not a bound, because a
failing lookahead was recorded like any other error and won on progress - so the
second half is upstream: a lookahead demands nothing, and what fails inside one
is not an expectation (winnow-grammar#10).

**And the input changed at 0.0.169**, which is the one row in this table whose
question moved rather than being answered:
[ADR-120](specification/adr/adr-120.md) D2 makes the block after the pattern
*the* action, so `n:digit1 { n }` is no longer a forgotten arrow — it is the
form. The row asks the same reader question from the other side now: somebody
who writes a program the old way is told what the new one is, by name, instead
of *expected `{`*. The lookahead stayed and got stricter: `G_BOUND_START` is
**lexical**, so `digit{1}` is a bound and `digit { 1 }` is an action, which is
what makes D2's *a bound starts with an integer* true rather than nearly true.

C3 now names `)`. An empty parameter list is a real alternative, and the `(`
was already matched, so the parameter list had begun — its missing `)` is a
requirement and the parameter spellings are the note.

C5 says `}` where the reader arguably wants "a method". Both are true: an
`impl` body is methods until the brace, and the brace is what the parser
requires at that position with `fn`/`pub` in the note. It is the same trade as
A1, and recorded here rather than argued away.

## D. Almost the right token

| # | input | the reader needs | before | today |
| :-- | :--- | :--- | :--- | :--- |
| D1 | `/ a broken comment` at top level | `//` named | ⚠️ 4 tokens incl. `//` | ✅ `expected end of input` |
| D2 | `if a = b { }` | open question | ○ **parses** | ✅ ``expected `{` `` at the `=` |
| D3 | `a:B -> C` where `=>` was meant | the cut is `=>` | ⚠️ `` `//`, whitespace `` | ✅ ``expected `{` `` |

D1 is the case where a comment form *is* the answer, and the decision to keep
trivia out of messages entirely gives it up: `expected end of input` is correct
and unhelpful. Recorded so the trade is visible, not to argue it.

## E. Inside a `grammar` block

| # | input | the reader needs | before | today |
| :-- | :--- | :--- | :--- | :--- |
| E1 | `d:digit{1, -> { 1 }` | a number or `}` | ⚠️ incl. `//` | ✅ ``expected `}` ``, digits in the note |
| E2 | `d:nosuchbuiltin` | the backend rejects it | ○ parses, as intended | ○ |
| E3 | `par_fold(M, init)` | `,` — arity is the backend's | ⚠️ incl. `//` | ✅ ``expected `->` `` |

## F. The cause is far from the symptom

| # | input | the reader needs | before | today |
| :-- | :--- | :--- | :--- | :--- |
| F1 | `let s = "unterminated` | the **opening quote** | ⚠️ EOF, `` `\`, any character `` | ✅ ``expected `"` `` — at EOF, not at the quote |
| F2 | a stray `}` at top level | `unexpected '}'` | ⚠️ 4 tokens | ✅ `expected end of input` |
| F3 | one unclosed `fn` | `}`, and where the `{` was | ⚠️ 17 tokens | ⚠️ ``expected `}` ``, not where the `{` was |

F1 and F3 are a *position* problem, not a ranking one, and no amount of work on
expectations touches them. They need the opening delimiter remembered so the
message can point back at it. Separate mechanism, separate decision, listed here
so the two are not confused.

## G. Not ASCII

| # | input | the reader needs | before | today |
| :-- | :--- | :--- | :--- | :--- |
| G1 | `let x = “hi”` | the quote named, column right | ⚠️ `` `//`, whitespace `` | ✅ `expected expression`, column right |
| G2 | `let café = 1` | accepted | ○ **parses** | ○ |

The column in G1 is right in both states, which is worth knowing: the offsets
are character-counted, not byte-counted. A byte that is not UTF-8 at all belongs
to `fs::map` ([ADR-016](specification/adr/adr-016.md)) and is tested there.

---

## What the corpus turned up on its own

**Five cases did not fail, and three of them were the same defect.** `if { }`
(B3), `let 5 = x` (C4) and `if a = b { }` (D2) were all accepted by the grammar,
and this file said they were *"things the grammar admits that probably should not
be"* without naming what admitted them. **The reserved-word list
([ADR-051](specification/adr/adr-051.md)) closed all three at once**, which
is the evidence that they were one thing:

* `if { }` read `if` as a **variable** and `{ }` as a block — two statements.
* `let 5 = x` read `let` as a variable, then `5 = x` as an assignment — two
  statements.
* `if a = b { }` read `if` as a variable, `a = b` as an assignment, and `{ }` as
  a block — three.

So none of the three was a message bug **or** a grammar-admits-too-much bug: each
was a program that meant something other than what was written, with no
diagnostic of any kind, because a keyword could be a name. That is the worst
class this project names, and it was hiding in this file behind a `○`.

The two that remain are `d:nosuchbuiltin` (E2), where the backend refuses it and
the parser is right not to, and `let café = 1` (G2), which is accepted on
purpose.

**Two of the three are refusals now rather than good messages**, and the row says
so: B3 hands back the token list from the `head_expr` chain, which has no label,
and C4 says ``expected `mut` `` because that is the first thing `let_stmt`
alternates on. Both have the caret in the right place. D2 is the one that came
out well — ``expected `{` `` at the `=` — and it also answers the *"open
question"* that row carried: an assignment is not an expression, so `if a = b`
is refused rather than read as a condition.

**Three tokens joined the lists** and no headline changed:
[ADR-052](specification/adr/adr-052.md) added `null` where an expression may
start, `?` where a type may continue, and `?.` where a postfix may. The rows are
regenerated with them because the file is the compiler's output and not a claim
of its own; that they are the *only* change is what says the record added a type
and an operator and moved no diagnostic.

**Twenty-two of the twenty-four failing rows say what a reader needs** — the
twenty-one that always failed, plus D2. B3 and C4 are the two that do not, and
they are new arrivals rather than regressions: they were silent before.

What A4, B1 and G1 turned out to be is worth keeping, because they were filed
as something else for two rounds. They reported whitespace and were never a
trivia problem: in `let xs = [1, 2` — Nikaia had no list literal then, so
nothing could start at the `[` — `let` and `xs` each parse as an expression
statement, and the alternative that *would* have said `expected expression`
there is abandoned when the shorter parse wins. `alt` drops what a losing alternative
found, so the only error left at that offset was the whitespace skip, which
is then the furthest thing that failed. Two repairs were tried against this
file and reverted before the third worked; upstream `TODO.md` carried all
three, and winnow-grammar#8 is the one that landed.

What closed the twenty, in the order the changes landed: the ranking by
requirement (winnow-grammar#4) took the whitespace skip out of the headline;
the rule label (#5) turned lists of spellings into `expression` and `type`;
"an element that began is a requirement" (#6) let a missing `}` outrank the
continuations of the expression before it — A3, F3, and the second expectation
in A1, A2, C3, C5 and E3; keeping a losing alternative's error (#8) closed
A4, B1, G1 and F1, with the tie-break by *how long each requirement has been
open* keeping A3 and F3 from regressing when it did; and a failing lookahead
no longer being an expectation (#10) closed C2, together with the grammar
saying that a brace group is a bound only when it starts with a digit.

One tick deserves a footnote. **F1** now names the `"` it is missing, which is
what a reader acts on, but the position is still the end of the file rather
than the opening quote. The text is right and the position is not; remembering
the opening delimiter is a separate mechanism and is not done.

**No parse error showed the source line — closed.** Every row above used to be a
one-line headline plus `in <rule>` lines. A rustc diagnostic routed through
`nikaia --explain` got a snippet and a caret
([ADR-012](specification/adr/adr-012.md)); a parse error got `at line 3, column
5` and no line 3. That asymmetry applied to all twenty-six rows and was worth
more than any single group above, because a position a reader still has to go
and look up is half a diagnostic.

`render(source)` now prints the line under the headline with a caret under the
token (winnow-grammar#11, ADR 15 point 13):

```text
expected `}`; found unexpected token `temp` at line 3, column 5
   3 |     temp: i32
           ^^^^
note: also possible here: `,`, `//`
in struct_item
```

It reaches two places at once, because both go through the same `render`: the
compiler's own messages for `.nika` files, which is every row here, and the
errors a *generated* program prints for its own input — `examples/access-log/src/main.nika`
shows the rejected log line and points at the character, and
`crates/nikaia/tests/examples.rs` checks that end to end. The caret is as wide
as the token that was found; a line too long to print is windowed around the
position. The two remaining asymmetries with a rustc diagnostic are that this
one has no file name in front of it (it is rendered by the driver, which knows
the input and not where it came from) and no `= help:` lines.

## Keeping this honest

The inputs are `tests/errors/*.nika`, beside the existing `tests/samples/`, and
`tests/errors/EXPECTED.txt` holds what each produces **today, against the
backend in `Cargo.lock`** — so the suite is green as checked in and the messages
that are still wrong are on the record rather than in a memory.

```bash
cargo run -p nikaia --example errors > tests/errors/EXPECTED.txt
```

`crates/nikaia/tests/errors.rs` compares the whole file in one assertion, the
shape `grammar_lowering.rs` already uses. A backend bump — or a label added to
a rule — makes that test fail whenever it changes a message, which is the
point: the diff is the change, in the reader's terms rather than the parser's.
Read it, then regenerate.

The list itself is still meant to be argued with. A row struck or added here
should be a file added or removed there, and the golden regenerated.
