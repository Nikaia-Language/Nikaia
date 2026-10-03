# Findings for `winnow-grammar`

What Nikaia's use of the parser backend turned up, with a reproduction for each.
Recorded here so it is not re-discovered, and so it can be handed over as one
piece. Measured against `2f0d5da` and `024e3d3`.

**Status — handed over.** All four are now with upstream, on
`winnow-grammar` branch `claude/nika-2-branches-offene-aufgaben-kd77u1`:
§1 is implemented there (with the documentation the patch had not carried),
§3 is implemented there, and §2 and §4 are its TODO §5 and §6 with these
measurements. This file stays as the reasoning behind them.

---

## 1. Two expectations beat one, and the one is thrown away

### 1a. Every Nikaia error names whitespace instead of the token that would fix it

```nika
struct S {
    name: &str
    temp i32          // comma missing
}
```

```
expected one of: `//`, whitespace; found unexpected token `temp` at line 3, column 5
in COMMENT / in item 1 / in WS / in struct_item / in item / in program
```

Both expectations are **true** — whitespace and a comment really are accepted
there. But `,` is the only one whose insertion fixes the program, and it is
gone. This is every syntax error in the language, not a corner case.

Cause: `WS = (WSE | COMMENT)*` fails with two expectations, so `merge` raises it
to `PRIO_AGGREGATED (20)`. The real expectation has one, so `PRIO_NORMAL (0)`.
At equal offset `merge` returns the higher-priority side and **discards** the
other; the union happens only on a tie.

Nikaia cannot avoid it: comments need at least two alternatives in `WS`, and
there is no formulation with one. `tests/comment_aware_test.rs` is line for line
what Nikaia writes, so this reaches every grammar that supports comments.

### 1b. It is not about comments, and not about whitespace

No custom `WS`, no comments, no trivia of any kind:

```rust
grammar! {
    grammar G {
        pub rule doc -> usize = xs:item* "." -> { xs.len() }
        rule item -> u32 = n:u32 -> { n } | "#" h:hex_digit1 -> { 0 }
    }
}
// G::parse_doc().parse_test("1 2 x")
```

```
expected one of: `#`, integer literal; found unexpected token `x`
```

**`.` is missing** — the token that ends the document and fixes the input. Here
the repetition's own expectations are the useful ones and must stay, so the
answer cannot be to suppress a repetition's reason for stopping. It has to be
to stop discarding.

The general statement: **any rule with two or more alternatives suppresses every
single-expectation error at the same offset.** In a language grammar that is
most error positions — expressions, statements and items all have many
alternatives. The implicit whitespace skip is only the most frequent instance,
because the code generator puts it between every pair of tokens.

### A shape that fits both

The criterion is not taste. **An expectation is worth reporting only if
satisfying it can move the parse past this position.** Whitespace fails it: the
skip is greedy and has already taken everything available at this offset, so
inserting more produces the same failure at offset + n. Telling the reader
"expected whitespace" asks them to make an edit that cannot work.

That criterion has a mechanical form, and the code already draws the line —
`repeat_recording_bounded`, one branch:

```rust
Err(ErrMode::Backtrack(e)) => {
    let e = e.item(items.len() + 1);
    if items.len() < min {
        return Err(ErrMode::Backtrack(e));   // required: the error itself
    }
    e.record(&mut input.state);              // optional: merely recorded
    input.reset(&cp);
    break;
}
```

An error that is **returned** is a requirement: the grammar demanded something
here. An error that is **recorded** by a repetition that has met its minimum,
or by `opt_recording`, is an **optional continuation**: the grammar would have
accepted more and does not insist. `x+` below its minimum returns, so it stays
a requirement — which is why nothing here suppresses a repetition.

So the ranking needs no new classification, no declaration and no magic rule
names:

1. **Headline** — the requirements at the furthest offset.
2. **Note** — the optional continuations.
3. **Dropped** — an optional continuation that is whitespace, by the criterion
   above. (A *required* whitespace — `space1` below its minimum — is returned,
   so it is a requirement and stays.)

And the two changes are still independent: **stop discarding** (union at equal
offset instead of returning one side, with `fail(…)` and labels staying
authoritative), and **rank by requirement rather than by how many alternatives
an error happens to carry**.

Worked through:

| input | today | proposed |
| :--- | :--- | :--- |
| `temp i32` in a struct | `expected one of: '//', whitespace` | `expected '}'` — note: also possible: `,`, `//` |
| `1 2 x` against `item* "."` | `expected one of: '#', integer literal` | `expected '.'` — note: also possible: `#`, integer literal |
| ` /broken comment` | whatever is required, with `//` discarded | `expected …` — note: also possible: `//` |

The struct line is the honest one rather than the ideal one, and worth saying
plainly: at that position the grammar does **not** require a comma. It requires
`}` or accepts another field, and a parser cannot know the author meant the
second. `expected '}'` with `,` in the note is true and complete; guessing the
intent is not on offer.

This also subsumes "deprioritise comments for now": a comment is an optional
continuation, so it lands in the note by construction, with no special case for
it and no list of rule names anywhere.

A near-miss — the found input sharing a prefix with a trivia expectation, `/`
against `//` — would promote that expectation to the headline and is the one
place a heuristic would earn its keep. It is a refinement of this, not a
prerequisite.

**What it costs to implement.** `state.furthest` is one `ParseError`, so keeping
requirements and optional continuations apart needs either a flag per
expectation or a second slot. That is the whole of it; the classification itself
is free, being a branch the runtime already takes.

### Against the existing tests in `tests/diagnostics.rs`

| | why it is unaffected |
| :--- | :--- |
| p06 `let ?;` → `expected one of: number, string` | the label is authoritative; nothing unions into it |
| p07 `let "abc;` → the label's own message stays | the two errors are at **different offsets** — the string alternative made progress — so the offset comparison decides before priority is consulted |
| p08 `fail(..)` wins on a tie | authoritative; `headline()` returns `message` |
| p09 progress beats `fail(..)` | offset first, unchanged |

**Not verified against the full suite.** Cargo fingerprints a git dependency on
its commit, so editing the vendored checkout has no effect, and this is a design
plus two reproductions plus a reading of four tests — not a tested patch.

---

## 2. The word-at-a-time class scan loses below ~6-8 characters

ADR 23's changelog entry says of the eight-bytes-at-a-time scan: *"on the short
runs a real format has it is smaller but never negative."* On 1BRC's
`digit{1,2}` and `digit` it is negative.

**The mechanism, before the numbers.** `AsciiClass::out_mask` costs a fixed
~13 operations per 8-byte word for a one-range class (`digit`): build `guarded`
(2), two `splat`+`wrapping_sub`+`&HIGH` (~8), combine (3). With `run`'s loop
head, `try_into`, `trailing_zeros`, the divide, and `class`'s `as_bstr` and
`next_slice`, a call is **~30 instructions regardless of run length**. A
per-character loop costs ~4-5 *per character*. Break-even is therefore around
six to eight characters, and a one-character run cannot come out ahead: a
13-operation word test is not cheaper than a 1-operation character test.

**Then the numbers**, `examples/1brc.nika --profile lite`, callgrind over
200 000 rows, same source, same flags, identical output. All four are simulator
outputs, so they do not depend on the machine:

| | `2f0d5da` | `024e3d3` | Δ |
| :--- | ---: | ---: | ---: |
| instructions | 131.2 M | 143.9 M | +12.7 M |
| branches | 16.49 M | 17.25 M | +0.76 M |
| **mispredicts** | 653 026 | 638 959 | **−14 067** |
| D1 misses | 207 908 | 209 753 | +1 845 |
| LL misses | 55 723 | 55 187 | −536 |

The scan does what a word scan is supposed to do — it buys fewer mispredicts
with more instructions. It is the size of the trade that is wrong: 14 067
mispredicts at ~20 cycles is ~0.28 M cycles won, against 12.7 M instructions at
a generous IPC of 4, ~3.2 M cycles paid. **About elevenfold against.** Both
approximations lean the same way: IPC 4 is an upper bound so the cost is a lower
bound, and a simulated predictor is simpler than a real one so the saving is if
anything overstated.

Instruction counts alone would have been the wrong measurement here, and for
exactly this kind of change — that is why the branch and cache columns are in
the table.

The scan path itself: 40 → 106 instructions per row (`take_till0` → `rt::class`),
which is the whole of the +63 per-row difference.

This is a threshold, not a regression to undo: ADR 23's own +9 % on
identifier-heavy grammars is entirely plausible, identifiers being 5-15
characters. The code generator knows `{1,2}` at compile time, so an upper bound
below break-even could take the character path. Where the crossover sits is a
measurement for the machines that matter, not for a VM.

---

## 3. `dec(p)` and `text(p)` cannot appear inside a `#[frame]`

`builtin_may_consume` has no arm for them, so `_ => true` says they consume
anything, and the frame check rejects:

```
error: the built-in `dec` in rule `TENTHS` can consume the boundary "\n" of
frame `MEASUREMENT`
```

`intern` is already excepted, with the reasoning that applies unchanged to
both:

> `intern(p)` consumes exactly what `p` consumes and nothing besides: it is a
> map over its argument, and the argument is checked on its own in the
> `RuleCall` arm above.

`text(p)` is `.take()` over its argument and `dec<T>(p)` is a `try_map` over the
same sequence; neither consumes anything its argument does not.

Reproduction: `examples/1brc.nika` with `whole:dec[i32](digit{1,2})`. It compiles
and runs correctly under `@frame(…, unchecked)`, which is how the measurement in
[ADR-296](../specification/adr/adr-296.md) was taken.

---

## 4. The intern cache is sized for identifiers, not for aggregation

Not a defect — an assumption worth making settable. `InternCache` is 512
direct-mapped slots and says so plainly: *"There is no collision handling
because there is nothing to handle - a displaced entry is simply interned
again."* Right for a compiler's identifier stream, where a few words are hot.

1BRC has 413 distinct keys, so displacement is the common case and every miss
falls through to `ThreadedRodeo` — a shard lock and a hash, ~2.5x a cache hit.
Measured, keying the station table by `Symbol` and indexing a `Vec` instead of
hashing a `&str`:

| keys | intern cache + `Vec` index | plain map, fast hash |
| ---: | ---: | ---: |
| 15 | 507 instructions/row | 512 |
| 413 | **617** | 514 |

The map is flat and the cache degrades. Making `InternCache::BITS` settable per
context would close it: how many distinct keys a parse will see is the caller's
knowledge, not the library's.
