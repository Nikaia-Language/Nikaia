# The error corpus

Twenty-six broken `.nika` files and what the compiler says about each. It exists
to be argued with: a message is only wrong against a claim about what a reader
needed, and this is where those claims are written down before anything is
changed. Every change to how a parse error is said is measured against all
twenty-six at once.

**The message a reader gets** has one layout since 0.0.266 (Part III C.2,
rule 5): the headline as a sentence, `-->` and the place, the source line with
the place underlined, then `= note:` and `= help:`.

```text
error: A `,` is missing before `temp`.
  --> src/main.nika:2:21
   |
 2 |     name: ref String
   |                     ^
   |
   = help: Separate `String` and `temp` with a comma.
```

`tests/errors/EXPECTED.txt` records the headline and the position, which are
what the columns below quote. `✅` says what a reader needs · `⚠️` does not ·
`○` does not fail at all.

## How a message finds the mistake

A parse stops where the input stopped making sense, and that is often a line
after the mistake: a missing `,` fails at the next field, a missing value at
the `}` below it, an unclosed `{` at the end of the file. What the parser
*expected* there is true and says little. So a failed parse is read once more
(`a_better_reading` in `crates/nikaia/src/parser/mod.rs`), for the mistake a
reader made:

* **A reading that proposes a change is offered only where the program,
  changed that way, parses further.** A missing `,` is named when inserting it
  gets the parse past the token it failed at; `==` is proposed for `if a = b`
  only when the comparison parses. No guess is said that the parser would
  refuse.
* **The caret goes where the mistake is**: after the element the `,` belongs
  to, on the `=` that has no value, on the `{` or `"` that was never closed.
* **A reserved word is said to be one only where a name was wanted** — again
  by trying a plain name in its place.

Where no reading applies, the message is still *Expected X here, but found
Y.*, made from what the grammar required at that position.

---

## A. A separator or terminator is missing

| # | input | the reader needs | today |
| :-- | :--- | :--- | :--- |
| A1 | `struct S { name: ref String` ⏎ `temp: i32 }` | the `,`, after `String` | ✅ *A `,` is missing before `temp`.* at the end of line 2 |
| A2 | `fn f(a: i32 b: i32) {}` | the `,`, after `i32` | ✅ *A `,` is missing before `b`.* |
| A3 | `fn f() {` ⏎ `let x = 1` | where the `{` was | ✅ *This `{` is never closed.* at the `{` |
| A4 | `let xs = [1, 2` ⏎ `}` | where the `[` was | ✅ *This `[` is never closed.*, with *a `}` closed something else first* |
| A5 | `struct S { a: i32,, b: i32 }` | the doubled comma | ✅ *There are two commas here.* |

## B. An operand is missing

| # | input | the reader needs | today |
| :-- | :--- | :--- | :--- |
| B1 | `let y = ` | the `=` with nothing after it | ✅ *`=` has nothing after it.* on the `=` |
| B2 | `let y = 1 + ` | the `+` with nothing after it | ✅ *`+` has nothing on its right.* on the `+` |
| B3 | `if { }` | the missing condition | ✅ *`if` needs a condition before its `{`.* |
| B4 | `f(1, )` | an expression | ✅ *Expected expression here, but found `)`.* |
| B5 | `let x: = 1` | a type | ✅ *Expected type here, but found `=`.* |

B1 and B2 used to fail at the `}` on the next line; the reading moves the
caret onto the operator whose operand is missing.

## C. The wrong token where the grammar knows what belongs

| # | input | the reader needs | today |
| :-- | :--- | :--- | :--- |
| C1 | `struct S { name ref String }` | `:` | ✅ *Expected `:` here, but found `ref`.* |
| C2 | `rule A -> i32 = n:digit+ -> { n }` | *the arrow is gone* | ✅ *An action doesn't take an arrow.*, naming `pattern { action }` |
| C3 | `fn main( {` | `)` or a parameter | ✅ *Expected `)` here, but found `{`.* |
| C4 | `let 5 = x` | a name | ✅ *`let` names what it binds, but `5` is a value.* |
| C5 | `impl S { struct T {} }` | a method | ✅ *An `impl` holds methods, and a `struct` can't be declared inside one.* |

C1 is the best row in the corpus and the model for the rest. It carried a
note calling `ref` a reserved word until the note was asked whether a name was
wanted there; it was not, and the note is gone. So is C5's.

C2's arrow is refused by name, the reading ADR-120 D2 made necessary: the
block after a pattern *is* the action, and a program written the old way is
told the new one.

## D. Almost the right token

| # | input | the reader needs | today |
| :-- | :--- | :--- | :--- |
| D1 | `/ a broken comment` at top level | `//` named | ✅ *A single `/` is division, and nothing stands before it to divide.*, help *A comment starts with `//`.* |
| D2 | `if a = b { }` | `==` | ✅ *`=` gives a name a value; a condition compares with `==`.* |
| D3 | `a:B -> C { 1 }` where `=>` was meant | the cut is `=>` | ✅ *A pattern can't contain `->`* … *write `=>`.* |

D3 used to share C2's message, and following it — delete the arrow — gave
`a:B C { 1 }`, which parses as a plain sequence: a program that silently meant
something else. An arrow followed by a block is C2's; an arrow anywhere else in
a pattern is D3's.

## E. Inside a `grammar` block

| # | input | the reader needs | today |
| :-- | :--- | :--- | :--- |
| E1 | `d:digit{1, -> { 1 }` | a number or `}` | ✅ *Expected `}` here, but found `{`.* |
| E2 | `d:nosuchbuiltin` | the backend rejects it | ○ parses, as intended |
| E3 | `par_fold(M, init)` | the arity | ✅ *`par_fold` takes four arguments.*, at the `)`, with the form written out |

E3 was *Expected `->` or `{`* at the rule's end: the fold failed, and the
reading of `par_fold` as a rule's name got a line further. `par_fold(` and
`fold(` now commit to a fold.

## F. The cause is far from the symptom

| # | input | the reader needs | today |
| :-- | :--- | :--- | :--- |
| F1 | `let s = "unterminated` | the **opening quote** | ✅ *This string is never closed.* at the `"` |
| F2 | a stray `}` at top level | the `}` named | ✅ *Didn't expect `}` here.* |
| F3 | one unclosed `fn` | where the `{` was | ✅ *This `{` is never closed.* at the `{` |

The opening delimiter is found by reading the file the way the lexer does —
strings and `//` comments skipped — and taking the innermost bracket still
open.

## G. Not ASCII

| # | input | the reader needs | today |
| :-- | :--- | :--- | :--- |
| G1 | `let x = “hi”` | the quote named, column right | ✅ *`“` isn't a quote the language reads.*, help naming `"hi"` |
| G2 | `let café = 1` | accepted | ○ parses |

Offsets are counted in characters, so G1's column is right.

---

## How it got here

**Before a word of the messages changed**, six changes in `winnow-grammar`
took the parser's own machinery out of them: an expectation ranked by whether
the grammar *required* it (winnow-grammar#4), a rule that names itself
(`expression`, `type`, #5), an element that began counted as a requirement
(#6), a losing alternative's error kept (#8), a failing lookahead not an
expectation (#10), and the source line with a caret (#11).

**The reserved-word list** ([ADR-051](specification/adr/adr-051.md)) closed
three rows that did not fail at all: `if { }`, `let 5 = x` and `if a = b { }`
were read with a keyword as a variable — programs that meant something other
than what was written, with no diagnostic of any kind.

**0.0.266** gave every message one layout and plain words, and dropped the
`note: also possible here: …` line — which A1, A2 and A5 had leaned on, so they
became wrong. **0.0.272** is the reading above: all twenty-four failing rows
say what the reader needs.

## Keeping this honest

The inputs are `tests/errors/*.nika`, and `tests/errors/EXPECTED.txt` holds
what each produces today:

```bash
cargo run -p nikaia --example errors > tests/errors/EXPECTED.txt
```

`crates/nikaia/tests/errors.rs` compares the whole file in one assertion. A
change that moves a message makes it fail, which is the point: the diff is the
change, in the reader's terms. Read it, then regenerate. A row struck or added
here is a file added or removed there.
