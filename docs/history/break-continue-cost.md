# What `break` and `continue` cost

**Date:** September 14, 2026; §7 added when the question was answered
**Status:** measured, and the numbers are
[ADR-084](../specification/adr/adr-084.md)'s evidence. **Written before the decision
and left as it was written** — the construct was built to price it, and what §7
records is a question answered *from* this page rather than beside it.
**Related:** [ADR-084](../specification/adr/adr-084.md) (the decision this measured),
[ADR-070](../specification/adr/adr-070.md) D2 (which found the absence and left it
open), [ADR-071](../specification/adr/adr-071.md) (all three words reserved, which
is why nothing here cost a program a name),
[ADR-034](../specification/adr/adr-034.md) (a handler that diverts may not be
overlapped), Part I 3.3

[ADR-070](../specification/adr/adr-070.md) D2 found, while answering a different
question, that a Nikaia loop can only be left by its condition going false or by
a `return` that leaves the whole function. It named the two words that were
missing and did not decide whether they should arrive.
[ADR-071](../specification/adr/adr-071.md) reserved them so the question could stay
open at no cost. This is the measurement that makes it answerable: the construct
is built, the branch is green, and the three questions a keyword has to answer —
*what does it cost at run time, what does it cost to compile, what does it break*
— each have a number or a name under them.
[ADR-084](../specification/adr/adr-084.md) is the answer that came out of it; §7 is
what it took from here and what it did not.

**The conclusions, first.**

1. **At run time it costs nothing and saves a great deal.** The lowering is name
   for name: Nikaia's `break` is Rust's `break` is a jump. What it saves is not
   the jump but the shape that stands in for it — a flag, tested once a turn, and
   in a `for` a loop that cannot stop at all. Measured: **−25 %** on a `while`
   that learns it is finished half way through its body, and **−99.9 %** on a
   `for` over a range that wants to stop early, where the saving grows without
   bound in the number of turns. `continue` is within 1 % of the nesting it
   replaces, which is the honest answer for it: it buys shape, not speed.
2. **At compile time it costs about 665 instructions per block** — per *block*,
   not per statement — which is **+0.13 % to +0.32 %** of a whole compile. That
   number is a consequence of where the two rules stand in the statement
   alternation, and it is the whole of the compile-time question: move them ahead
   of `assign_stmt` and they cost a further **~740 instructions per statement**
   and **+1.28 %** on a statement-dense file.
3. **What it is incompatible with is one thing said four ways: a jump does not
   leave a function.** A lambda, a task, an `overlap` branch and a DSL fold's
   step are each a closure or an `async` block in the language below, so a
   `break` inside one whose loop is outside it is refused (`NK1132`) rather than
   handed to `rustc`. A `catch` handler is **not** on that list and works, because
   it lowers to a `match` arm. Nothing else in the compiler noticed the construct
   at all, and §5 says why that is a property of the language rather than luck.

---

## 1. What is built

435 lines added across ten files of the compiler, plus a benchmark, a
measurement and 18 tests.

| Where | What |
| :--- | :--- |
| `ast/mod.rs` | `Stmt::Break` and `Stmt::Continue` — no payload, no label |
| `parser/mod.rs` | `break_stmt` and `continue_stmt`, last in the `stmt` alternation (§3 says why last) |
| `emit/mod.rs` | `break;` and `continue;` — name for name, [ADR-011](../specification/adr/adr-011.md) D2 — and the backstop of §4.3 |
| `check/mod.rs` | a loop count, the boundaries it restarts at, `NK1132` and `NK1133` |
| six walkers | one arm each, all of them doing nothing (§5) |

The emitter's half of that table is the part worth noticing: the refusal is in
**both** places on purpose, and §4.3 says why one of them is not enough.

**No program in the tree changes**, and no word had to be taken away from anyone:
[ADR-071](../specification/adr/adr-071.md) reserved `break` and `continue` a day
before this was written, so the addition is a rule that widens what parses and
nothing else. That is the reservation paying for itself, and it is worth saying
out loud because it is the part that would have been expensive later.

Two diagnostics are new:

* **`NK1132`** — a `break` or a `continue` with no loop to act on. Two shapes,
  one code: there is no loop, or there is one and a function boundary stands
  between. §4 is about the second.
* **`NK1133`** — a statement after a jump, in the same block. The shape it is
  really about is **`break i`**: Rust's `break` carries a value out of a `loop`
  and this one does not, so a value written after it parses as a statement of its
  own. Without the refusal the program compiles, the value is dropped, and
  nothing says so — the same failure `while_stmt`'s comment records from the day
  `while` was missing: *three statements, no error*.

---

## 2. What it costs at run time

**The method is this repository's** ([ADR-011](../specification/adr/adr-011.md) §4):
instructions retired under callgrind, on one tree, at `-O`. `benches/jumps.nika`
holds both halves of every A/B in one file, so one lowering and one `rustc`
invocation produce both and the only difference in a pair is the construct.
`crates/nikaia/tests/measure.rs` runs it:

```text
cargo test -p nikaia --test measure -- --ignored --nocapture what_a_jump
```

The floor — the same binary started and stopped without running any of the three
loops — is **468,851** instructions, and it is subtracted in the table below so
that the numbers are the loops rather than the runtime starting.

> **The first row of the table below was corrected after it was published, and
> the correction is [ADR-086](../specification/adr/adr-086.md) §3.** It measured
> something real and attributed it to the wrong thing: the `break`-less baseline
> had to nest an `if` inside its loop, because `while i < n && running` did not
> parse — a *second* gap, which §2.1 below reports as a finding without either
> half of this page noticing they were the same finding. With `&&` in the head
> the flag shape costs **6,500,940** against the jump's **6,501,105**: the same,
> to 165 instructions over two million turns. The −25 % was the nesting.
>
> The rest of the page stands, and so does what it decided — §7 says what
> decided [ADR-084](../specification/adr/adr-084.md) was the `for` row, and that
> row is untouched. The tables are left as they were taken, with this note over
> them, because a number edited after the fact is a number nobody can check.

**n = 2,000,000, floor subtracted:**

| the loop | without the jump | with it | change | per turn |
| :--- | ---: | ---: | ---: | :--- |
| a `while` that learns it is finished mid-body | 8,001,165 | 6,001,141 | **−25.0 %** *(but see the note above)* | 4.0 → 3.0 |
| a `for` over a range that stops early | 9,010,308 | 12,404 | **−99.9 %** | 2,000,000 turns → 1,415 |
| a `for` that skips turns (`continue`) | 54,001,109 | 53,472,243 | −1.0 % | 27.0 → 26.7 |

And the same three as the harness prints them, un-subtracted, over three sizes:

```text
a `while` that stops mid-body - n = 20000
  a flag in the head                  578,872 baseline
  `break`                             558,787 -3.5%
a `while` that stops mid-body - n = 200000
  a flag in the head                1,298,910 baseline
  `break`                           1,098,811 -15.4%
a `while` that stops mid-body - n = 2000000
  a flag in the head                8,498,937 baseline
  `break`                           6,498,840 -23.5%

a `for` that stops early - n = 20000
  a flag, and every turn taken        589,532 baseline
  `break`                             499,848 -15.2%
a `for` that stops early - n = 200000
  a flag, and every turn taken      1,401,545 baseline
  `break`                             502,320 -64.2%
a `for` that stops early - n = 2000000
  a flag, and every turn taken      9,507,852 baseline
  `break`                             510,171 -94.6%

a `for` that skips turns - n = 20000
  the body inside an `if`           1,038,700 baseline
  `continue`                        1,033,377 -0.5%
a `for` that skips turns - n = 200000
  the body inside an `if`           5,898,753 baseline
  `continue`                        5,845,401 -0.9%
a `for` that skips turns - n = 2000000
  the body inside an `if`          54,498,781 baseline
  `continue`                       53,965,420 -1.0%
```

**What the three numbers mean, and they mean three different things.**

**The `while`: one instruction a turn.** The flag is a store, a load and a test
that the `break` does not need — the optimiser folds most of it, and what is left
is exactly 1 instruction per turn out of 4. A percentage that grows with `n` is
the fixed cost of starting up falling away, not the saving growing.

**The `for`: everything.** This is the case with no workaround at all. A `for`
over a range cannot be stopped: `return` leaves the *function*, so it is only an
exit where the loop is the last thing the function does, and a flag stops the
body without stopping the loop. So the flag version takes all 2,000,000 turns to
do 1,415 turns' work, and the gap is **not a constant factor** — it is
`n / √n` for this predicate and unbounded in general. This is the number that
would decide the question if one number had to.

**The `continue`: nothing, and that is the useful answer.** 27.0 instructions a
turn against 26.7 — the two lower to the same machine code, because inverting a
condition and nesting the rest of the body is what a compiler does to a
`continue` anyway. So `continue` earns its place on what the source reads like
and not on what it costs, and a decision about it should be made on that ground
rather than this one.

### 2.1 What the baseline shape cost to *write*, which is a finding of its own

The obvious version of the `while` baseline is

```nika
while i < n && running { … }
```

and **it does not parse**. The head of a `while`, a `for` or an `if` is the
brace-free expression grammar (`head_expr`), and `&&` is not in it at any
precedence — `if a && b { … }` is refused the same way. So the second condition
has to become a nested `if` with an `else` that sets the flag, which is why the
baseline in `benches/jumps.nika` is shaped the way it is.

This is pre-existing and has nothing to do with jumps, but it is not unrelated:
the workaround for a missing `break` is *"put the exit condition in the head"*,
and the language cannot express that. It is on [[#206](https://github.com/Nikaia-Language/Nikaia/issues/206)](https://github.com/Nikaia-Language/Nikaia/issues).

**And it is the same finding as §2's first row**, which neither half of this page
noticed while it was being written. The baseline was carrying two handicaps and
the table named one of them. [ADR-086](../specification/adr/adr-086.md) closed the
gap and re-measured: with `&&` in the head, the `break`-less shape costs what the
`break` costs. That is the sharpest lesson this page has to offer about its own
method — **an A/B is about the construct only if the construct is the only
difference**, and here the other difference was written down three paragraphs
away.

---

## 3. What it costs to compile

**The method needs stating, because the first two attempts at it were wrong.**
An instruction count for the whole compiler is only comparable between two
binaries built from *nearly* the same source: adding eighty unrelated lines to
`check/mod.rs` moved these totals by up to 0.8 % through inlining and code
layout alone, in both directions. And an identical binary run twice on the same
input differs by up to **0.11 %** — measured, on two byte-identical copies. So
the A/B below is between two builds whose source differs **in nothing but the two
grammar rules**, and every number is a median of five runs.

| file | without the rules | with them, placed last | change |
| :--- | ---: | ---: | ---: |
| `blocks.nika` (2,000 blocks, 2,000 statements) | 410,934,530 | 412,264,526 | +0.324 % |
| `many.nika` (1 block, 2,002 statements) | 116,047,192 | 116,050,312 | +0.003 % |
| `examples/json/src/main.nika` | 40,833,960 | 40,953,318 | +0.292 % |
| `examples/k-nucleotide/src/main.nika` | 16,589,311 | 16,613,053 | +0.143 % |
| `examples/tally/src/main.nika` | 9,034,400 | 9,046,157 | +0.130 % |

**The contrast between the first two rows is the whole mechanism.** Two thousand
statements in one block cost 3,120 instructions — nothing. Two thousand
statements in two thousand blocks cost 1,329,996, which is **665 instructions per
block**. The rules are last in the alternation, so a statement that matches an
earlier arm never reaches them; what reaches them is the `}` that ends a block,
where `stmt` fails through every arm in turn and each failure is recorded for the
*"also possible here"* note. The cost is per **block end**, and a program has far
fewer of those than it has statements.

### 3.1 The placement is the whole question

Put the two arms where they read most naturally — beside `return_stmt`, ahead of
`assign_stmt` and `expr_stmt` — and the same measurement says:

| file | rules last | rules before `assign_stmt` | change |
| :--- | ---: | ---: | ---: |
| `many.nika` (2,002 assignments) | 116,050,326 | 117,531,152 | **+1.276 %** |
| `blocks.nika` (2,000 `return`s) | 412,383,029 | 412,384,482 | +0.000 % |
| `examples/json/src/main.nika` | 40,962,680 | 41,094,257 | +0.321 % |
| `examples/k-nucleotide/src/main.nika` | 16,620,755 | 16,647,489 | +0.161 % |

1,480,826 instructions over 2,002 statements is **740 per statement** — the cost
of two keyword matches that fail, and of recording that they could have matched.
`blocks.nika` is unmoved because its statements are `return`s, which match
earlier in both orderings; that it moved by exactly zero is what says the reading
is right rather than plausible.

**So the rules are last, and the comment there says why.** It costs nothing in
correctness: both words are reserved
([ADR-071](../specification/adr/adr-071.md)), so `NAME` cannot take one and no
earlier arm can swallow a jump. It costs something in reading order, which is
paid back at 740 instructions a statement.

---

## 4. What it is incompatible with

One rule, and it is not a design choice — it is what a jump *is*. A `break`
compiles to a branch to a label, and a label in another function is not a place a
branch can go. Four constructs in this language are a separate function in the
language below, and inside each of them a loop written outside is unreachable:

| construct | what it is below | what happens |
| :--- | :--- | :--- |
| a lambda — `fn (x) { … }` | a Rust closure | `NK1132` |
| a task — `spawn fn { … }` | `async move { … }` ([ADR-055](../specification/adr/adr-055.md) §6) | `NK1132` |
| an `overlap` branch | one `async { … }` per branch ([ADR-050](../specification/adr/adr-050.md) D2) | `NK1132` |
| a DSL fold's `init`, `step`, `merge` | closures handed to the fold driver | `NK1132` |

```text
error[NK1132]: `break` leaves the innermost loop around it, and the nearest loop is outside this lambda
  --> r2.nika:6:17
   6 |                 break
                       ^
     = a lambda is a function of its own in the language below, and a jump does not leave a function
     help: decide inside the lambda and act on the answer outside it - a `bool` it hands back, tested by the loop
```

The refusal has to be this compiler's. Without it the message is `rustc`'s —
*"`break` outside of a loop"*, on a line of a file nobody wrote — which Part III
C.1 calls a bug in this compiler rather than a bad error message.

**The one that is worth knowing about before writing code** is the lock doors.
`lock.update fn (v) { … }`, `access_all` and `update_all`
([ADR-065](../specification/adr/adr-065.md)) all take the block as a lambda, so a
loop cannot be left from inside one. The way out is the one the help names —
decide inside, act outside — and it is a real restriction rather than an
oversight, because the alternative is leaving a lock held across a jump.

### 4.1 A `catch` handler is not one of them, and that matters

```nika
for p in paths {
    let text = fs::read_to_string(p) catch {
        break
    }
    seen += text.len()
}
```

This works, and `crates/nikaia/tests/jumps.rs` runs it rather than asserting
about the emitted text. A handler lowers to a `match` arm (Kap 7.1), which is not
a function boundary:

```rust
let text = match fs::read_to_string(p).await {
    Ok(value) => value,
    Err(error) => { break; },
};
```

It is worth a test of its own precisely because the four rows above make the
opposite look like the rule.

### 4.2 The fold's lambdas were reachable and unchecked

Building this found a hole that was already there. A fold's `init`, `step` and
`merge` are expressions inside a **pattern**, and the checker walks a grammar
rule's *action block* and nothing else — so

```nika
pub rule file -> i64 = fold(N, zero, fn(acc, m) { break })
```

lowered to `|acc, m| { break; }` and was refused by `rustc`. An undeclared name
in the same position is not refused either, so the gap is older and wider than
jumps. This branch closes **the half a jump can reach**, with a walk that reports
`NK1132` and nothing else; the rest is on [[#237](https://github.com/Nikaia-Language/Nikaia/issues/237)](https://github.com/Nikaia-Language/Nikaia/issues), because
walking those bodies with the whole checker would newly refuse things that have
nothing to do with this construct.

### 4.3 And the emitter refuses it too, which is not belt and braces

The four rows above are found by a **walk**, and a walk can miss a corner. One
did: the block walk that reaches a fold's lambdas deliberately does not descend
into a `spawn`, because a task's body is a detached context everywhere else it is
asked about (Part I 5.4) — so

```nika
pub rule file -> i64 = fold(N, zero, fn(acc, m) { let h = spawn fn { break } })
```

went past the checker. The fix is not a better walk. `Flow` — the emitter's
"what surrounds this" record, which already carries `throws`, `caught` and
`in_lambda` — gained `in_loop`, set on a loop's body and false in
`Flow::PLAIN`, which is where a lambda, a task, an `overlap` branch and a
function body each start. **Every statement is emitted through one place**, so a
jump with no loop cannot be written whatever a walk did or did not reach:

```text
`break` has no loop to act on here, and the language below would refuse the
file this writes (Part I, 3.3)
```

The division is the one [ADR-055](../specification/adr/adr-055.md) §6 already drew
for a lambda that pauses: **the checker is the diagnostic and the lowering is the
guarantee.** `NK1132` is what a program meets, and it is the message worth
writing because it names which construct stands in the way; this is what makes
*"the walk is complete"* something that holds rather than something to hope for.
A program should never see it.

### 4.4 The ordering analysis learns a fourth way to divert

[ADR-034](../specification/adr/adr-034.md) refuses to overlap a statement whose
`catch` handler can `return`: the statement after it is conditional on this one
having succeeded, and [ADR-033](../specification/adr/adr-033.md) D5 forbids running
a conditional operation early. A handler that *jumps* makes the next statement
conditional in exactly the same way, so `contracts::order`'s `diverts` now counts
`break` and `continue` too — and is exact about the one case where it should not:
a jump bound to a loop written **inside** the handler lands in the handler, so the
statement after it is reached either way.

**This is a rule written ahead of the case that needs it.** `diverts` is asked
only about branches of an explicit `overlap`, and a jump in one of those is
refused by §4's table before it gets here. It is in because the analysis should be
right about what a construct means rather than about which constructs happen to
reach it.

### 4.5 What the `while true` shape now does to the generated file

`break` makes `while true { … }` the unconditional loop it was declared to be
([ADR-070](../specification/adr/adr-070.md) D1) — before this, a `while true` could
only be left by `return`, which is why no `.nika` file in the tree writes one.
The emitted Rust for the shape that is about to become common is:

```text
warning: denote infinite loops with `loop { ... }`
  --> t1.rs:22:5
   |
22 |     while true {
   |     ^^^^^^^^^^ help: use `loop`
   = note: `#[warn(while_true)]` on by default
```

A warning naming a line of a file nobody wrote is the same class as an error
naming one. It is pre-existing — `while true` has always lowered to `while true`
— and it is **made common by this branch** rather than caused by it. The fix is
one line in the emitted preamble (`#![allow(while_true)]`) and it is a decision
about every file this compiler writes, so it is named here and not taken.

### 4.6 Two lines of the error corpus

The *"also possible here"* note in a parse error lists what the grammar would have
accepted, so two entries joined it wherever a statement could stand.
`tests/errors/EXPECTED.txt` is regenerated; the diff is two lines and both are
that list. Nothing else in the corpus moved.

---

## 5. What it is *not* incompatible with, and why that is a property of the design

Six walkers in the compiler gained an arm for the two new statements, and **every
one of those arms does nothing**:

| analysis | what it decides | what a jump contributes |
| :--- | :--- | :--- |
| `contracts::sharing` | `Rc` or `Arc`, per value ([ADR-037](../specification/adr/adr-037.md) D6) | nothing — it joins no two slots |
| `views` | where a view of a buffer is stored | nothing — it stores nothing |
| `contracts::sync` | which calls a `sync` function reaches | nothing — it calls nothing |
| `contracts::touch` (via `order`) | what a statement reaches in the world | nothing — it mentions no name |
| `dsl` | which expressions hold a DSL statement | nothing — it holds no expression |
| `check`'s type walk | what everything is | `Ty::Unknown`, as a `return` does |

The reason is one sentence: **all of them are about where a value goes, and a
jump carries none.** That is also the argument that the unlabelled form is the
cheap one — a label would be a name that is not a value, scoped to a construct
rather than to a block, and every one of those six would then have a naming
question it does not have today.

**The compiler named all six itself.** Those matches have no `_` arm — the AST's
own comment on `LitInterpolated` records why, from the day an analysis walked past
a string as if it held no code — so adding a variant turned *"which analyses must
be told about this?"* from a question into a list of compile errors. Six files,
six arms, no thinking required. That is what the totality rule was bought for, and
this is the first construct added since it was paid for.

---

## 6. What is still open

**`break` with a value, and the `loop` keyword with it.**
[ADR-070](../specification/adr/adr-070.md) D2 wrote down the condition under which
the `loop` keyword reopens: *the day a `break` hands back a value*. This does not
meet it — `NK1133` refuses `break i` outright — so D1 stands as written and
`loop` stays reserved and unused, which is what
[ADR-071](../specification/adr/adr-071.md) reserved it for and what
[ADR-084](../specification/adr/adr-084.md) D7 records.

**[#183](https://github.com/Nikaia-Language/Nikaia/issues/183)'s unreachable-`return` entry was invalidated as written** — the one [ADR-093](../specification/adr/adr-093.md) later closed — and this is
the one place where building the construct made something else more expensive.
It has since been rewritten; what follows is the argument that had to go.
The entry proposes a diagnostic improvement — a `while true { … }` cannot be left,
so a function ending in one needs no unreachable `return` — and its argument for
why the work is small here and large in Rust is quoted exactly:

> a `while` whose condition is the literal `true` cannot be left except by
> `return`, because `break` and `continue` do not exist — not in the grammar, not
> in the parser, and not among the reserved words. So the analysis other languages
> need for this question is, here, one test on the condition.

With `break` that is false. The test becomes *"the condition is the literal
`true` **and** no `break` in the body is bound to this loop"*, which is still
small — the loop count this branch already keeps answers it — but it is a walk of
the body rather than a test on the condition.

**Labels.** Out of scope on purpose, and §5 is the argument for keeping them out:
the unlabelled form costs nothing anywhere because it carries nothing anywhere.
The case it does not reach — leaving an outer loop from an inner one — is served
today by a flag or by `return`, and what *that* costs has not been measured.
[ADR-084](../specification/adr/adr-084.md) D2 makes that measurement the condition
for reopening, which is the same shape of answer this page is.

---

## 7. What the decision took from here

[ADR-084](../specification/adr/adr-084.md) is the record, and three things about the
relationship are worth saying, because the order they happened in is the point.

**The construct was built before it was decided**, which is not the usual order
and was the right one here. A keyword is the most expensive thing a language adds
([ADR-070](../specification/adr/adr-070.md) D1), so *"what does it cost"* deserves a
number, and a number cannot be had from a design document. Everything in §4 — the
four boundaries, the `catch` handler that is not one, the fold's unchecked
lambdas, the walk that missed a `spawn` — was found by building it, and none of it
would have been in a record written first.

**What decided it was §2's middle row and not the percentages.** −25 % on a
`while` and −1 % on a `continue` are numbers a language can live without. The
`for` row is different in kind: it is not a slow loop but a **missing
capability**, because a `for` cannot be stopped at all, and the gap is unbounded
in `n` rather than a constant factor. A record leading with the two percentages
would have made a weaker case out of stronger evidence.

**One measurement changed the implementation rather than justifying it.** §3.1's
ordering was not written down and then checked: the rules were placed where they
read best, measured at +1.28 %, moved to the end, and measured again at nothing.
That is why [ADR-084](../specification/adr/adr-084.md) D8 exists as a decision at all
— an ordering with a reason attached survives the next person who tidies it.
