# Does `from(f)` generalise? The worked cases for `throws` and `touches`

**Date:** September 12, 2026 — a laboratory record, and a snapshot of that day.

> **Read as a snapshot.** The lambdas on this page are written in the form
> [ADR-049](adr/adr-049.md) withdrew — `sort_by_key fn { a }`, with
> the argument read off the body — and **no sample here compiles as written**; the
> current spelling names its arguments, `sort_by_key fn(x) { x }` (Part I, 5.3).
> The analysis each section records is unaffected: what a lambda's arguments are
> called changes nothing about when it runs or what it touches, which is what this
> page is about. Transcribe the reasoning, not the code.

[ADR-029](adr/adr-029.md) D3 added a fourth `sync` state to the
ledger, `sync = "from(f)"`, and rested it on one sentence:

> A caller reads it as "this call adds no pausing of its own." That is sound for
> exactly one reason: the lambda runs **during** the call, so its body is part of
> the function that writes it, and that function's own analysis has already
> counted its calls. `from` adds nothing because there is nothing left to add.

The question this page answers: does that argument also hold for `throws`
([ADR-023](../specification/adr/adr-023.md) D1) and for `touches`
([ADR-033](adr/adr-033.md) D2), so that "immediate function types
for user code" is **one** inference rule rather than three?

Nothing here is normative. The answer is one sentence per effect in ADR-029 D3;
this is the programs, the ledgers they produced and the reports they printed.

---

## 1. What the argument actually requires

Read closely, D3's sentence has **two** premises, and only the first one is the
one D4 bounds:

1. **Immediacy.** The lambda runs before the call returns, so its body is
   lexically inside the function that writes the call. D4 is this premise, and
   `a_detached_lambda_may_not_use_from` is how `std` is held to it.
2. **Scope match.** The analysis that consumes the contract asks a question
   whose scope is *the whole function body*. Only then does "its calls are
   already counted" mean they are counted **where the contract is being read**.

Premise 2 is never stated in D3, because for `sync` it is invisible: `sync` *is*
a property of a function, so counting an effect anywhere in the body is counting
it in the right place. It is the premise that decides the other two effects.

Mechanically, premise 1 is one match arm. `contracts::sync`'s `visit_expr_blocks`
descends into `Expr::Closure { body }` wherever a closure is an argument, and
skips `spawn`:

```rust
Expr::Block(block) | Expr::Seq(block) | Expr::Closure { body: block, .. } => f(block),
```

`contracts::throws` imports that same walk. `contracts::order` does not have it.
That one difference is the whole answer.

---

## 2. `sync` — the control

The program is ADR-029 §3's own, re-run here so the other two have something to
be compared against.

```nika
fn ordne(xs: Vec[i64]) -> i64 { xs.sort_by_key fn { a } return 1 }
fn im_lock(xs: Vec[i64]) -> i64 sync { return ordne(xs) }
```

`nikaia.contracts`:

```toml
[fn."im_lock"]
sync = true
signature = "(xs: Vec[i64]) -> i64"

[fn."ordne"]
sync = "inferred"
signature = "(xs: Vec[i64]) -> i64"
```

`sort_by_key` says `sync = "from(f)"`, `ordne` earns `sync = "inferred"` through
it, and nothing is reported. Now the negative half, which is what proves the
counting rather than the permission:

```nika
use std::io

fn ordne(xs: Vec[i64]) -> i64 throws {
    xs.sort_by_key fn { io::read_to_string().len() }
    return 1
}
```

```toml
[fn."ordne"]
throws = ["?"]
signature = "(xs: Vec[i64]) -> i64"
```

No `sync` key, which is `Sync::No`: the pausing call **inside the lambda** took
the claim away. `from(f)` did not launder it, because the enclosing function's
own walk reached into the lambda and found `io::read_to_string`. That is D3's
sentence, executing.

---

## 3. `throws` — the same walk, and nothing left to buy

```nika
enum LeereZeile { Leer }

fn pruefe(n: &i64) -> i64 throws {
    if n == 0 { throw LeereZeile::Leer }
    return 1
}

fn sortiere(xs: Vec[i64]) -> i64 throws {
    xs.sort_by_key fn { pruefe(a) }
    return 1
}

fn ohne_lambda(xs: Vec[i64]) -> i64 throws {
    xs.sort_by_key fn { a }
    return 1
}
```

```toml
[fn."ohne_lambda"]
sync = "inferred"
throws = ["?"]
signature = "(xs: Vec[i64]) -> i64"

[fn."pruefe"]
sync = "inferred"
throws = ["LeereZeile"]
signature = "(n: &i64) -> i64"

[fn."sortiere"]
sync = "inferred"
throws = ["LeereZeile"]
signature = "(xs: Vec[i64]) -> i64"
```

`LeereZeile` is in `sortiere`'s set, and `ohne_lambda` — the same call with a
lambda that cannot fail — has nothing in its set at all (its `["?"]` is the
declaration's own floor, which a declared `throws` never drops below). So the
named error came from the lambda's body and from nowhere else: the caller's own
analysis already counts what the lambda throws, at the point of the call. Both
premises hold, for the same reason they hold for `sync`, and `throws` is a fact
about a function in exactly the way `sync` is.

*(These two sets are what the run gives **after** §6's second finding was fixed.
Before it, `sortiere` read `["?", "LeereZeile"]` and `ohne_lambda` got its `"?"`
from the method call rather than from the declaration, because
`contracts::throws` answered every method call with `"?"` instead of asking the
type checker. The argument below does not depend on which it was.)*

`throws` has a second, independent reason to gain nothing from a `from` key: a
`throws = "from(f)"` read as "adds nothing" could only ever **remove** an error
from a caller's set, never add one — and the direction that removes is the one
[ADR-010](../specification/adr/adr-010.md) D1 forbids. What the caller needs from a
higher-order call is that the lambda's failures be **added**, and the walk
already adds them. A `throws` column behaves exactly as `from` would want it to
and has no key to add.

### The `and_modify` chain, and the precision it used to lose

[ADR-031](adr/adr-031.md)'s live example, with a failing `add`:

```nika
enum ZuVoll { Voll }

pub struct Stats { n: i64 }

impl Stats {
    fn add(&self, v: i64) throws {
        if self.n > 100 { throw ZuVoll::Voll }
    }
}

fn record(m: HashMap[&str, Stats], v: i64) throws {
    m.entry("x").and_modify fn { a.add(v) }
}
```

```toml
[fn."Stats::add"]
sync = "inferred"
throws = ["ZuVoll"]
signature = "(&Stats, v: i64)"

[fn."record"]
sync = "inferred"
throws = ["ZuVoll"]
signature = "(m: HashMap[&str, Stats], v: i64)"
```

**`record` used to get `["?"]` and not `["ZuVoll"]`.** The lambda *was* walked —
that is where the `"?"` came from — but `a.add(v)` is a *method* call, and
`contracts::throws` did not ask the type checker what a method resolves to the
way `contracts::sync`'s `reach_of` does with ADR-028's `MethodCalls`. So the
chain `HashMap[&str, Stats]` → `fn(&$V)` → `a.add(v)` → `Stats::add` was
followed for `sync` and not for `throws`, and the fix was the one argument this
page predicted: `throws::infer` is handed the same `resolved` map, and merges it
in the same place, by the same rule.

It was a **precision** gap and not a soundness one — `"?"` is the widest claim
the column can make, and a caller reads it as "it fails". Which is why closing
it has to be argued in the other direction, and here is that argument, because
the change makes an answer *narrower* and that is normally the forbidden way:

* every contribution that went is a contribution from a call the compiler
  **named**. A receiver of unknown type and a method no ledger describes both
  still arrive as `"?"`, from `MethodCalls::unresolved`, so "I cannot see it" is
  still never read as "it does not fail";
* what replaces the `"?"` is a *written-down* fact. `Vec::sort_by_key` carries
  no `throws` in `std.contracts`, and an absent `throws` there means "it cannot
  fail" — reviewed like code, and stated as such in that file's own header,
  which contrasts it with an absent `touches` deliberately;
* it is the same trust `sync` already places in the same map through the same
  resolution, which is ADR-028's whole point: **one** answer to what `a.add(v)`
  goes to, read by both walks. Two answers is what that record refused.

`from` still has nothing to add afterwards.

---

## 4. `touches` — where the argument breaks, and it is the polarity

Premise 1 holds here too: `and_modify`'s lambda runs before the call returns.
**Premise 2 does not.** `contracts::order` does not ask "what does this function
reach"; it asks "what does *this statement* reach", because the rule it serves is
about two adjacent statements. A touch set that is complete for the function and
incomplete for the statement is worth nothing to it.

And nothing walks a lambda for an effect. `contracts::order`'s `walk` reduces a
statement's value to the calls it performs, and a closure is not one of the
shapes it reads — so the lambda's body is never reached, and the statement is
refused instead. The file is already half-aware of the asymmetry: `names_in`,
which finds data dependencies, *does* descend into `Expr::Closure`. The names
count; the effects do not.

### The contrast that settles it

A `catch` handler is the other piece of code that runs during a statement, and
ADR-033 §8.3 made its effects part of the statement's touch set. So the two
cases can be put side by side. With `--overlaps --user-parallelism yes`:

```nika
use std::fs

fn main() {
    fs::write("a.txt", "x") catch { println("gescheitert") }
    println("danach")
}
```

```text
main:
    in order  fs::write + println / println - both reach stdout, and one writes it
```

The handler's `println` is **named as a callee of the first statement** —
`fs::write + println` — and `stdout` is what the pair meets on. Counted.

```nika
fn lauf(xs: Vec[i64]) {
    xs.sort_by_key fn { println("aus dem lambda") return a }
    println("danach")
}
```

```text
lauf:
    in order  println - nothing says what `sort_by_key` reaches, so it reaches everything
```

The lambda's `println` appears nowhere. The pair is refused, correctly, but for
**ignorance about `sort_by_key`** and not because anybody counted the write. So:

> Where a handler's effect is *counted into the statement*, a lambda's effect is
> *stepped around*. "Already counted at the call site" is true for `sync` and for
> `throws` and false here.

### The counterfactual, measured

Run the same program against `std`'s ledger with one line added —
`Vec::sort_by_key` claiming `touches = []`, which is the permissive entry a
`from`-style reading would amount to:

```text
lauf:
    in order  println - `xs` is not a literal, and only literals are sent to another thread
```

Still refused, and **not** because of the lambda. Today a trailing lambda on a
method call is refused two or three times over, and the other refusals are the
ones the records call liftable:

| refusal | what would lift it |
| :--- | :--- |
| nothing says what `sort_by_key` reaches | a `touches` line — the thing `from` would be |
| `xs` is not a literal | ADR-033 D9's table: "an implementation limit, not a language question" |
| what `Vec::new` hands back may not cross a thread | a `crosses` line for the type (ADR-005 §1 Group B) |

Each of those is a gap somebody is meant to close. Close all three and the only
thing left between a permissive higher-order entry and an overlap is whether the
analysis reads the lambda. Before this change nothing in the repository said that
it must not; the refusal was a consequence of `walk`'s `_` arm. It is a named arm
now, with the reason written where somebody will be standing, and a test —
`a_lambdas_own_effects_are_not_in_the_statements_touch_set` in
`crates/nikaia/tests/touch_sets.rs` — that holds it. With a bare lambda statement,
where no other limit fires first, the arm is visible:

```text
lauf:
    in order  println - one of them holds a lambda, whose body this analysis does not read
```

### Why this is a soundness question and not a speed one

`touches` is fail-closed by [ADR-033](adr/adr-033.md) D4 — an
unknown touch set is *everything* — and the reason is
[ADR-010](../specification/adr/adr-010.md) D1's: an analysis that fails open is a
vulnerability generator. `sync`'s `from` is a **permissive** reading, and that is
affordable there because the permission it grants is paid for by a count that
really happened somewhere else. Here there is no count, so the permission is
unfunded, and what it buys is not speed:

```nika
xs.sort_by_key fn { println("aus dem lambda") return a }
println("danach")
```

Two touch sets that meet on nothing may be reordered. If `sort_by_key` said
"adds nothing" and the analysis believed it, these two lines would become a
group, and the two lines of output would interleave differently from run to run.
That is ADR-033 D1 broken by a resource nobody checked — the identical shape as
the `stdout`/`stderr` mistake ADR-033 §6 caught (`println` / `eprintln` /
`println` interleaving under `2>&1`) and as the `catch` hole §8.3 closed. **A
soundness hole here changes what a program prints.**

So `touches` does not want `from`'s reading. What it would want, if a Nikaia
function could ever take a lambda, is the **opposite** instruction: *add* what
the lambda reaches to this statement, and where the lambda cannot be read, the
statement reaches everything. That is the polarity `walk_block` already gives a
`catch` handler, and it is a union rather than an omission.

Which also means the spelling must not be reused. One word meaning "skip, it is
counted elsewhere" in the `sync` column and "descend, nothing else counts it" in
the `touches` column is two rules wearing one name, and the `stdoutt` lesson of
`contracts::touch::KINDS` is what a vocabulary whose mistakes are permissions
costs. If the column ever needs it, it needs a word that says descend.

---

## 5. The answer

**One rule, not three — and `touches` is not the third.**

| effect | does the caller's analysis already count the lambda, at the call? | what `from` needs |
| :--- | :--- | :--- |
| `sync` | **yes**, by `visit_expr_blocks`' `Expr::Closure` arm | the rule ADR-029 D3 built |
| `throws` | **yes**, by the same walk — and what it cannot resolve is still `"?"` | **nothing**, and no key either |
| `touches` | **no** — the consumer asks per *statement*, and no walk reads a lambda for an effect | not `from`'s reading; a *union* instruction under a different word |

The rule, stated so that it says what it covers:

> A ledger may say "the lambda decides, and this call adds nothing of its own"
> for an effect that is (a) carried by an **immediate** parameter, and (b)
> consumed as a fact about a **whole function**. Where the consumer needs the
> effect attributed to a smaller scope than the function — a statement, a pair —
> "already counted" is a claim about the wrong place, and the entry must make the
> caller *add* the lambda's effect instead of omitting it.

So a function type for user code costs **one** inference rule, the one that
exists. `throws` rides on the same walk and needs no entry. `touches` needs no
entry either, as long as nothing ever reads a higher-order call permissively —
which is now a named refusal rather than a `_` arm.

---

## 6. Incidental findings

All three are **fixed now**, and each is kept here with what was probed, because
that is what this page is for.

* **A call that can fail, in a function that does not declare `throws`, was not
  reported by Nikaia.** `fn ruft() -> i64 { return pruefe(0) }` where `pruefe`
  is `throws` lowered without a diagnostic and recorded `ruft` as unable to
  fail; the emitted Rust was `fn ruft() -> i64 { pruefe(0) }`, which `rustc`
  rejects. `NK2601` and `NK2701` are both about *implicit* calls
  ([ADR-025](../specification/adr/adr-025.md) D1), and the explicit case had no
  code. Nothing to do with lambdas — the same program without one behaved the
  same way — but it is the reason §3's programs all declare `throws`.

  It is `NK2605` now, and §7 is the whole probe: the ledger it published, the
  second half nobody had noticed (D8's propagation was not lowered at all), and
  why the answer is to refuse the program rather than to infer the keyword.
* **`contracts::throws` did not use ADR-028's method resolution**, where
  `contracts::sync` does. §3's `and_modify` chain is what showed it: `ZuVoll`
  became `"?"`. Fail-closed, so not urgent, and a one-argument change — which
  is what it turned out to be. §3 now records the run both ways and the three
  reasons the narrowing is a written-down fact rather than an assumption.
* **Two leftover mentions of the withdrawn path** outside the files
  `docs/README.md` allows them in: `docs/foreign-runtime.md` (line 42) called
  `rustc 1.94.0-nightly` "the toolchain this repository named", which it no
  longer is, and [ADR-036](adr/adr-036.md) §2 names the same
  nightly as its measurement machine.

  The wording is fixed in the two notes and the **version is kept**: a note that
  records which compiler a number was taken on is correct, and rewriting the
  number would falsify the measurement. ADR-036 §2 is unchanged, and
  deliberately: it names the compiler beside its instruction counts and makes no
  claim about what this repository uses, so there is no tense in it to correct —
  and an ADR is written once.

---

## 7. `NK2605`, end to end

The reproduction is the two lines from §6, with `std` in place of a local
`throws` function so that nothing about it depends on this file:

```nika
fn liest() -> String throws { return fs::read_to_string("x.txt") }
fn ruft() -> String { return liest() }
```

### What it published

Before: it lowered, silently, and `nikaia.contracts` said

```toml
[fn."liest"]
throws = ["?"]
signature = "() -> String"

[fn."ruft"]
signature = "() -> String"
```

An absent `throws` in that file means **it cannot fail** — the same file's
`std` counterpart says so in its header, and contrasts it with an absent
`touches` on purpose. So the committed, shipped, `--locked`-compared artifact
carried a wrong fact about the one thing a caller cannot see: whether the
callee can fail. That, and not the missing caret, is what made this the serious
one of the four.

### The second half, which the probe found on the way

The emitted Rust was

```rust
fn ruft() -> String { liest() }
```

…and adding the `throws` the rule asks for gave

```rust
fn ruft() -> Result<String, Box<dyn std::error::Error>> { Ok(liest()) }
```

which `rustc` rejects too. **[ADR-023](../specification/adr/adr-023.md) D8's
propagation was not lowered at all.** Every fallible call in `examples/` has a
`catch` beside it — all eight programs that can fail do, and `tally.nika`'s one
propagation is a *loop's* step, which had its own `?` from
[ADR-025](../specification/adr/adr-025.md) — so nothing in the corpus ever asked
for this one, and no test did either. A
diagnostic whose `help:` produced that second failure would have moved
Appendix C.1's violation one step later instead of removing it, so the `?` is
emitted now for a call **by name**, and `a_written_call_propagates_its_failure`
compiles and runs the result.

A **method** call still does not take one: the emitter has no receiver types,
which is exactly ADR-028's division of labour, and handing it the checker's
answers is the shape `fallible_loops` already has. `NK2605` refuses a fallible
method call all the same — the checker *does* know — so on a method the way out
that lowers today is `catch`. Part I 7.1's Status note says so.

### Which answer, and why not the other one

The ledger stops publishing the wrong fact because **the program is refused**,
before `Ledger::render` is reached. The alternative — infer `throws` for a
function that never wrote it, the way `sync` is inferred — was rejected on
three counts:

1. [ADR-023](../specification/adr/adr-023.md) D1 derives the error **set** and
   leaves the declaration in the source: "in source, `throws` is bare". The
   keyword is not derived truth; what it can fail *with* is.
2. [ADR-025](../specification/adr/adr-025.md) D1 has already decided the case, in
   words: "the function **must** declare `throws`, and the compiler says which
   implicit call is the reason". Inferring it instead would be a different
   decision, and would need a record.
3. It would publish a **new** false fact. `contracts::throws`'s walk does not
   know about `catch`: `fn f() { g() catch { … } }` cannot fail, and an
   inference blind to that would record that it can. Trading a false negative
   for a false positive in a file other programs read is not an improvement —
   ADR-027 D3's argument for why `Sync` has three states is the same argument.

### The polarity, asked the way ADR-027 D4 asks it

Can a declared `throws`, or its **absence**, outrank what the body does? Both,
and they fail in opposite directions:

| | what the source said | what the inference does | which way it fails |
| :--- | :--- | :--- | :--- |
| `throws` written, body cannot fail | it fails | keeps `["?"]` | **safe** — over-claims, a caller handles an error that cannot arrive |
| `throws` written, body names errors | it fails | replaces `["?"]` with the names | exact; the names come from the body |
| `throws` **absent**, body can fail | it cannot fail | *nothing* — no entry is made | **open**, and this was the defect |

The third row is ADR-027 D4's hole mirrored. There, an **assertion** survived a
body that contradicted it and kept `sync = true`. Here the **absence** of an
assertion survived a body that contradicted it and kept "cannot fail". Same
shape — the source's word beating the body — and opposite sign: an over-claim
for `sync`, an under-claim for `throws`. Both fail open, which is why both had
to go. The first two rows are the direction that is allowed to survive, and
they still do.
