# Handoff — the parser backend: how to work on it, and what was already tried

Written at the end of a session that could not finish, because the change it
depends on lived in a repository that session had no push access to. That
change is merged and Nikaia is on it. What is still open is one entry in
[`open-work.md`](open-work.md); what is here is how to work on this area and
what has already been measured. Read this file first before touching the parser
backend.

---

## 1. The blocked thing — landed

The `winnow-grammar` change is **merged upstream** (winnow-grammar#4, `e1b0e33`)
and Nikaia is on it: `Cargo.lock` names that commit and
`tests/errors/EXPECTED.txt` is regenerated against it, so the corpus is green
as checked in and its rows now describe what a user actually gets.

`docs/upstream/0001-expectations-by-requirement.patch` stays here as the record
of what was handed over — it is the first of the five commits that landed:

* the ranking itself (the patch, unchanged but for a clippy lint in its test),
* SYNTAX.md and CHANGELOG.md, which the patch had not touched — the documented
  order of message selection was still progress-then-priority,
* two documentation errors that had `cargo doc` failing on upstream `main`
  since `54acc33`,
* `text(p)`/`dec<T>(p)` inside a `#[frame]` — §4.2 below, now closed,
* the remaining findings, as TODO items §5 and §6 upstream.

Twelve of the twenty-one failing corpus rows moved. The nine that did not are
§3, and they are the open work on messages.

### To test a further backend change against Nikaia

Append to `Cargo.toml` (and **remove it again before committing** — it must not
be checked in):

```toml
[patch."https://github.com/keywan-ghadami/winnow-grammar"]
winnow-grammar = { path = "/abs/path/to/winnow-grammar" }
winnow-grammar-macros = { path = "/abs/path/to/winnow-grammar/winnow-grammar-macros" }
winnow-grammar-model = { path = "/abs/path/to/winnow-grammar/winnow-grammar-model" }
```

**Do not edit the vendored checkout under `~/.cargo/git/checkouts/` instead.**
Cargo fingerprints a git dependency on its *commit*, not on file contents, so
the edit is silently inert — `cargo clean -p` does not help either. An hour was
lost to this.

---

## 2. What the patch does, and why it is shaped that way

Nikaia's parse errors named the wrong thing: `expected one of: '//', whitespace`
where `expected ','` was meant. True, useless, and it was **every** syntax error
in the language.

Two axes, and both are needed:

* **A requirement outranks an optional continuation.** The runtime already draws
  the line: `repeat_recording_bounded` *returns* the element's error below the
  minimum and merely *records* it at or above, and `opt_recording` only ever
  records. So `ParseContext::record` is by construction the optional path and
  marks what it stores.
* **Among optional continuations, the implicit whitespace skip ranks last.** The
  first axis cannot separate it: where the entry rule is a repetition
  (`program = item*`) everything is optional. The code generator now calls the
  skip through `rt::skip_trivia`, so what it records is marked — no rule names,
  no declaration, and a `WS` a grammar calls itself stays an ordinary rule.

Nothing is discarded by either axis: the loser becomes a `note: also possible
here: …`. Whitespace alone is left out, and that is the criterion the rest
follows from — the skip is greedy, so more whitespace only moves the same
failure to offset + n.

### Two things that were tried and must not be redone

* **Do not give a `*` repetition's stop-reason a lower priority in general.** It
  is often the useful half: `xs:item* "."` over `1 2 x` needs both the `.` and
  the items. This was rejected on review and the corpus row exists to prove it.
* **Do not take trivia out of the *progress* race** (a second `furthest` slot).
  It looks right and it made case B1 far worse: seventeen expectations in the
  headline, twenty-two in a note, and the position on a token that was correct.
  It was reverted. The three rows it was meant to fix (A4, B1, G1) are still
  open and need a different idea.
* `tests/context_reuse_test.rs` catches an `absorbing` that throws away the
  absorbed error's rule stack — that stack carries `in item 4`, which says
  *which* element failed. Keep it.

---

## 3. The corpus — where to start

`docs/error-corpus.md` is 26 broken inputs, what a reader needs from each, and
what the compiler says, in two columns — before the ranking change and today.
`tests/errors/*.nika` are the inputs and `tests/errors/EXPECTED.txt` is what
they produce against the backend `Cargo.lock` names, so the suite is green as
checked in.

```bash
cargo run -p nikaia --example errors > tests/errors/EXPECTED.txt   # regenerate
```

**A backend bump that changes a message makes `errors.rs` fail, and that is the
point:** the diff is the change in the reader's terms. Read it, then regenerate.

**No row is left.** All twenty-one failing rows say what a reader needs.

What closed them, upstream, each measured against this corpus:

| change | what it closed |
| :--- | :--- |
| ranking by requirement (winnow-grammar#4) | the whitespace skip out of the headline |
| a rule may name itself (#5) | `expected expression`, `expected type` |
| an element that *began* is a requirement (#6) | A3, F3, and the second expectation in A1, A2, C3, C5, E3 |
| a losing alternative keeps its error (#8) | A4, B1, G1, F1 |
| a failing lookahead is not an expectation (#10) | C2, with the grammar change below |
| the message shows the line, with a caret (#11) | the closing finding — all twenty-six rows |

Two of them are worth reading before touching this area again. A4, B1 and G1
were filed twice as "trivia wins on progress" and were never that: `alt` drops
what a losing alternative found, so where a *shorter* alternative wins - `let`
and `xs` each parse as an expression statement in `let xs = [1, 2` - nothing
survives at the position the input actually goes wrong except the whitespace
skip. Two repairs were tried against this file and reverted before the third
worked, and `docs/error-corpus.md` keeps all three.

C2 needed a change here as well as upstream: `{ n }` is an action block whose
`->` was forgotten, and the grammar read it as a repetition bound. A brace
group is a bound only when its content starts with a digit - the rule the
backend already states for the same ambiguity - and `peek(("{" digit))` is how
the grammar says it.

**One tick has a footnote.** F1 names the `"` it is missing, and its position
is still the end of the file rather than the opening quote. Remembering the
opening delimiter is a separate mechanism and is not done.

### The label obstacle, and how it was removed

`expected expression` instead of six token spellings needed `# "…"` on a rule
with many alternatives, and the backend could only label a *single* variant.
Both halves are upstream now (winnow-grammar#5):

* a rule may name itself between its return type and its `=`;
* the leading whitespace skip of a labelled rule is hoisted **outside** the
  label, because `ParseError::labelled` substitutes only when the error is at
  the position the label started at - measured from inside the skip, it never
  is.

Nikaia's own grammar labels `expr`, `unary_expr`, `stmt`, `item` and
`type_ref`; `.nika` grammars can use the same syntax (Part II, 10.6), and
`examples/calc/src/main.nika` does. `unary_expr` is labelled as well as `expr` for a
reason worth keeping: `1 + ` fails inside `add_tail`, whose operand is a
`mul_expr`, so the label on `expr` never sees it.

### The finding that was worth more than all of it — closed

**No parse error showed the source line.** Every message was a headline plus
`in <rule>` lines, and a reader of `at line 3, column 5` never saw line 3, while
a *rustc* diagnostic routed through `nikaia --explain` got a snippet and a caret
(ADR-012, `diagnostics::render`). It applied to all 26 rows and nothing else in
this file touched it.

`ParseError::render(source)` now prints the line with a caret under the token
(winnow-grammar#11, ADR 15 point 13). It lands in both places at once, because
both go through the same call: the compiler's messages for `.nika` files, and
the errors a *generated* program prints about its own input — the `catch` in
`examples/access-log/src/main.nika` shows the rejected line and points at the character,
which `crates/nikaia/tests/examples.rs` checks end to end. What is still not
there, against a rustc diagnostic: a file name in front of the message (the
driver knows the input, not where it came from) and `= help:` lines.

---

## 4. Also open

Error messages first, performance after — that was the instruction, and the
messages are done: **all twenty-one failing corpus rows say what a reader
needs**, and they show the reader the line (§3). The two performance items are
closed too, and both closed by measuring rather than by building: one was a
real cost in a place nobody had looked, the other was not a cost at all. What
is left is 3 and 4, and neither is a performance question.

1. **The character-class scan threshold — closed, and not with a threshold.**
   Upstream scans a class eight bytes at a time (ADR 23), and this item said
   that on 1BRC's short runs it loses, with break-even estimated at six to
   eight characters and a threshold in the code generator as the fix. Measured
   with callgrind, the per-character loop costs ~7 instructions a character and
   the word path's fixed cost is 9 above it: **the crossover is at three**, so a
   threshold would buy at most 9 instructions on a one-character run — and a
   class that always matches exactly one is written as `digit`, which is a
   `one_of` and never reaches that code.

   The cost was in the case nobody had measured: a run of **length zero**, which
   cost the same as a run of eight. The implicit whitespace skip runs between
   every pair of elements of every syntactic rule and most of those find
   nothing, so `run` now tests the first byte first (winnow-grammar#12).
   **Nikaia's own compiler parsing 2 000 small functions: 281.6 M instructions
   -> 233.7 M, 17%.** 1BRC over 200 000 rows: 123.6 M -> 120.0 M, against
   119.4 M for a build with no word scan at all.

2. **Sizing the intern cache for 1BRC — closed, and it changes nothing.**
   `ParseContext::expect_distinct_keys(n)` exists upstream and Nikaia does not
   call it; the question was whether the measurement that rejected keying the
   station table by `Symbol` — 617 instructions per row against 514 for a plain
   fast-hashed map — had been an artefact of the *unsized* cache. It was not.

   Callgrind over `intern(until(";"))` on 100 000 rows, sized against unsized:
   at **413** distinct keys, 1BRC's station count, sizing saves **one**
   instruction per row; at 5 000 it saves 60. The default 512 slots already
   holds 413 keys without thrashing. So the rejection stands on its own, Nikaia
   needs no way to say a key count for this program, and the surface that would
   let a `.nika` file say one — an attribute on the grammar, since the count is
   the author's knowledge and not the emitter's — is worth designing when a
   program wants thousands of distinct keys and not before. Upstream
   winnow-grammar#13 records the numbers where the method is documented.

3. **Input provenance (ADR-010) — closed, and it was the biggest one.** A map
   keyed by the input now gets the hash the provenance of that input picks:
   `std`'s ledger says which of its calls are sources and who chose their bytes
   (ADR-020), and `contracts::trust` joins them over the program. Callgrind on
   200 000 rows, the same tree built twice with byte-identical output: **120.0 M
   instructions hardened against 89.1 M chosen — 26 %.** `nikaia --trust` prints
   the choice and what decided it.

   What the analysis is, so a later one is not mistaken for it: **one buffer**,
   because ADR-008 gives a compilation unit one input lifetime. The join over a
   program's sources is not a coarsening of a per-buffer analysis — it is the
   per-buffer analysis, for the one buffer this representation can express.
   `std` has no untrusted source yet; `http`, `net` and `db` arrive with the
   modules that have them, and the lattice is tested against a ledger that has
   one so the join is checked rather than assumed on that day.

4. **What `fortunes.nika` waits on — moved.** G7 is built; the two runtime pieces
   that are left are an entry in [`open-work.md`](open-work.md), because open work
   belongs in the list of open work and this file is a guide to the parser
   backend. Kept below for the reasoning, which is not repeated there.
   [ADR-017](specification/adr/adr-017.md) is implemented: `dsl html { … } eod`
   is compiled where it is written, every hole goes through `html::Render`, a
   hole in a position escaping cannot make safe is refused with the position
   named, and `<for row in :rows>` repeats a body. **The `render` function in
   `fortunes.nika` lowers and runs today**, and `examples/escaping/src/main.nika` is the
   whole contract in one page.

   Since [ADR-022](specification/adr/adr-022.md) removed the `fn:` form, the
   **whole file parses** — the chain in `main` was the last thing the syntax
   was in the way of. What it waits on now is two runtime pieces and no
   language question:

   * **The `postgres` block**, a deferred-parameter DSL (ADR-007 D4): the
     statement has to reach a driver intact, which is a different machinery
     from the template that is built.
   * **G6, the runtime binding.** [ADR-018](specification/adr/adr-018.md)
     decided what a handler *is* — the request as its first implicit argument,
     and what each return type answers with — and nothing of it can be built
     before there is a server to bind to. That is the roadmap line after the
     bootstrap compiler.

### Closed since this file was written

* `dec(p)` and `text(p)` inside a `#[frame]` — upstream, with `tests/ui/frames.rs`
  pinning that an argument which *can* consume the boundary is still rejected.
  `examples/1brc.nika` can drop its `unchecked`.
* **G12, enums and `match`** — Part I 4.4 specified them and Stage 0 could not
  lower them, so `calc.nika` carried an operator as a `&str` and compared it.
  Three variant shapes and five pattern shapes lower now, each one the language
  below spells the same way; an enum carrying a view takes the input lifetime as
  a struct does, so one may be a grammar rule's return type.
* The intern cache's fixed size — `expect_distinct_keys` upstream; see 2 above
  for what is left *here*.
* Tuples (G10), ordered output over a map (G5), and a rejected parse saying
  where it failed (G11).
* **G17, a stream that can fail while it is read** —
  [ADR-025](specification/adr/adr-025.md). It was two problems. The failure half
  was answered by a rule the language already had and had never stated
  generally: Part I 6.4 makes a function declare `throws` because a resource's
  cleanup at the *closing* brace can fail, and a `for`'s step at the top of the
  block is the same implicit call. `NK2701`. The ownership half is why
  `fs::lines` is **gone** rather than waiting — it would have had an iterator
  hand out views into a buffer it owns, and the shape that works is two calls.
  `io::lines()` exists and `examples/tally.nika` runs it.
* **The type checker** — [ADR-024](specification/adr/adr-024.md). Eight `NK1xxx`
  codes, answered from the ledger, before a line of Rust is emitted. The design
  is one value: `?`, the absence of a claim. An error is reported only where
  **both** sides are written down and disagree, so it never rejects a program
  that is correct — which is the only way to build one on a `std` that is half
  Rust. Its first find was real: `fortunes.nika` put a `&str` literal in a
  `String` field, which no `rustc` had ever seen because that file does not run
  yet. What it does not catch — a method on an unwritten receiver, an element
  type, what a `?` unwraps — becomes checkable when a signature is written,
  without the checker changing.

### Do not re-propose these — they were measured and lost

* `dec[i32](digit{1,2})` in 1BRC's `TENTHS`: 693 instructions per row against
  659 for the loop it would replace. `str::parse` does its own scanning, sign
  handling and overflow check.
* Keying the station table by `Symbol` through the intern cache: 617 against 514
  at 413 keys.
* A hardened-vs-fast constructor the user must remember, or a global default
  either way — ADR-010 §4 rejected both before this session started.

---

## 5. The state of the branch

`claude/nikaia-compiler-lowering-zaqa0y` (PR #9), and everything below it is
pushed and green: 28 tests, `cargo fmt --check` clean, clippy clean. This file
is updated on `claude/nika-2-branches-offene-aufgaben-kd77u1`, which is that
branch plus this note.

ADR-009 no longer sits below it: its two commits landed on `main` on their own
(PR #10), so what PR #9 still adds is the Stage 0 lowering and everything
after it.

* `Cargo.lock` is on upstream `024e3d3`. The measurements in ADR-011 §4 were
  taken on `2f0d5da` and say so.
* `examples/1brc.nika` runs and prints what the benchmark asks for.
* The 203 explicit `_sp:skip_ws` bindings are gone (commit `8888c3c`). **That is
  a precondition for the patch to reach `.nika` files**, not a cleanup: an
  explicit `skip_ws` calls `WS` as an ordinary rule, past the mark the code
  generator puts on the whitespace it inserts itself. Do not put them back.
