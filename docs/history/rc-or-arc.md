# `Rc` or `Arc` — per build, or per value?

**Date:** September 12, 2026
**Status:** the experiment ran; the finding was evidence under
[ADR-037](../specification/adr/adr-037.md) D3, and the owner has since decided with
it — see §11. This page keeps the **method, the machine and the false starts**,
which is what a note is for; what was decided, and the one number it was decided
on, are in the record.
**Related:** [ADR-037](../specification/adr/adr-037.md) D3 (the open question) and D2
(the switch), [ADR-005](../specification/adr/adr-005.md) §1 Group B and §5.3 (the check this
would change), [ADR-006](adr/adr-006.md) (cleanup, whose observability is
the gate in §2), [ADR-010](../specification/adr/adr-010.md) D1 (the polarity), [ADR-027](adr/adr-027.md) D1
(the fixpoint this one is compared against), [ADR-029](adr/adr-029.md) (the walk
through a struct's fields), [ADR-038](adr/adr-038.md) D7 (the foreign crossing),
Part II 12.2 (the counter that wants to cross)
**What ran:** `benches/refcount/` (the measurement),
`crates/nikaia/src/contracts/sharing.rs` and `crates/nikaia/tests/sharing.rs`
(the prototype), `nikaia --sharing` (what it prints)

> **Read as a snapshot.** The lambdas on this page are written in the form
> [ADR-049](adr/adr-049.md) withdrew — `sort_by_key fn { a }`, with
> the argument read off the body — and **no sample here compiles as written**; the
> current spelling names its arguments, `sort_by_key fn(x) { x }` (Part I, 5.3).
> The analysis each section records is unaffected: what a lambda's arguments are
> called changes nothing about when it runs or what it touches, which is what this
> page is about. Transcribe the reasoning, not the code.

[ADR-037](../specification/adr/adr-037.md) D3 expands `Shared` from `user_parallelism` —
`no` gives `Rc`, `yes` gives `Arc` — **per build**, and then says of itself:
*"Whether the choice between `Rc` and `Arc` could be made per value rather than
per build is a real question and is not answered here."* This is the experiment
that question was worth, and what it found.

**Four findings, first.**

1. **`Rc` and `Arc` differ in nothing a program can observe except speed and
   thread-crossing** — which is the gate the experiment was set to check, and it
   opens. Six assertions in `benches/refcount/src/bin/refcount.rs` say so
   on every `cargo test`. One difference is real and is *not* speed: with an
   atomic count the **last handle may be dropped on another thread**, so a
   cleanup runs there. It can only happen to a value that crosses — where the
   inference has no freedom anyway (§2).
2. **The atomic costs ~9 ns per clone-and-drop pair** on this box, a fixed cost
   and not a proportional one: ×3.40 where the loop is *only* the count, ×1.01
   where ~70 ns of other work sits between the clone and the drop, and **×1.00
   where the handle is read and never cloned**. Contended — four threads on one
   count — it is between ×11 and ×17, the least reproducible row here. But that
   case is exactly the one where a plain count is *impossible*, so the inference
   cannot reach it (§3).
3. **The inference's whole benefit is at `user_parallelism = yes`**, and it is
   bounded above by that 9 ns. At `no` it can only lose: per-build gives every
   `Shared` a plain count with no analysis, and a per-value inference that may
   not fail open gives an atomic one to everything it cannot see past. **The one
   `Shared` in this repository is such a value**: `examples/fortunes.nika`'s
   `db`, which the prototype makes atomic where the build makes it plain (§6).
4. **It does not stretch far enough to reach the program it was wanted for.**
   Part II 12.2's counter is `Shared[Locked[i32]]`, and `Locked` expands from the
   same switch. `Arc<RefCell<i32>>` is not `Send` — `RefCell` is not `Sync` — so
   inferring the *count* per value does not make that counter crossable; the
   **lock** would have to be inferred too. And `RefCell` against `Mutex` is not
   the observationally-equivalent pair `Rc` against `Arc` is: it differs in what
   a reentrant `access` does and in what a panic inside one leaves behind (§7).

---

## 1. The machine and the method

Intel Xeon @ **2.80 GHz**, 4 vCPU, 15 GB RAM, Linux 6.18.44 x86_64 — a shared
virtual machine with no `cpufreq` governor exposed. `rustc 1.94.1 (e408947bf
2026-03-25)`, stable, which is the only toolchain this repository has
([ADR-001](../specification/adr/adr-001.md) D1). `--release`. The load average is printed by
the script before and after every table rather than assumed away: **0.24 → 1.95**
across the first block and **0.83 → 1.34** across the second, the rise being this
measurement's own threaded rows.

Not the same box as [`runtime-cost.md`](runtime-cost.md) §1's — that one is a
2.10 GHz Xeon of the same 4-vCPU class — so nothing here may be compared against
a number there.

**Read the ratios and the signs, not the absolutes.**
[`runtime-cost.md`](runtime-cost.md) §6.3 is the standing warning and it has a
number on it: re-running an earlier measurement on that box a day later moved
every absolute figure by **1.4 to 1.9×, the baseline included**. Every finding
below is a ratio, a flatness or a sign, and §3.1 shows the two runs of this table
agreeing on all of them.

**The program** is `benches/refcount/src/bin/refcount.rs`, run by
`benches/refcount/refcount.sh`. Six shapes, each a **pair** whose two halves
differ in one thing — the reference count — so the difference is the count and
nothing else:

| shape | what it is |
| :--- | :--- |
| `clone+drop` | a handle cloned, handed to an opaque call and dropped there: one increment, one decrement |
| `read` | the value read through the handle, no handle made. **The control**: no count is touched, so the pair must tie |
| `clone+work(k)` | the same clone and drop with `k` multiply-adds between them — the count as a *share* of a loop that does something |
| `counter` | Part II 12.2's `Shared[Locked[i32]]` increment: `Rc<RefCell<i32>>` against `Arc<Mutex<i32>>` |
| `N threads, a handle each` | the same atomic with no sharing |
| `N threads, one handle` | four threads on **one** count — the cache line that makes an atomic expensive rather than merely atomic |

**Warm-up** is a tenth of a run before the first repeat. **Repeats** are nine,
each of 50 000 000 operations (fewer for the work shapes, so each row takes
comparable time); every repeat is printed by the binary, and the mean, range and
standard deviation below are over those nine.

**Nothing is elided, and that took two tries** — see §8 for the first run, which
measured a loop the optimiser had removed. The clone and the drop are separated by
an `#[inline(never)]` call that takes the handle by value, which both prevents the
pair from being folded away and is *also* the shape the emitter would write for a
`Shared` handed to a function.

---

## 2. Question zero: do they differ in anything observable?

The reason [ADR-037](../specification/adr/adr-037.md) D3 gives for `Shared` being written by
hand is that *sharing changes when a value is cleaned up, and that is observable*
([ADR-006](adr/adr-006.md)). If the same were true of the choice *between* the
two counts, the language's own rule against inferring cleanup timing would forbid
this inference and there would be nothing to build. So it was checked first.

**It is not true.** `benches/refcount/src/bin/refcount.rs`'s `mod observable`
asserts, on every `cargo test`:

| what | `Rc` against `Arc` |
| :--- | :--- |
| the size of a handle, and of a weak handle | equal, and one pointer |
| the owner count at every step of a clone-and-drop script | equal, step for step |
| **when the value is cleaned up** | the same point in the program, asserted against a written log |
| when a weak handle goes stale | the same point |
| whether exclusive access and unwrapping succeed | the same answers |
| `Send` | `Arc<T>` is where `T: Send + Sync`; `Rc<T>` never is |

Two differences are real and neither is in that table.

**The thread a destructor runs on.** With an atomic count the last handle may be
dropped on a thread other than the one that built the value, so a `Cleanup`
([ADR-006](adr/adr-006.md)) runs *there* — which is observable, and which
`docs/foreign-runtime.md` §4 already found to matter. It is not a counter-example
to the gate: it can only happen to a value that crosses a thread, and a value
that crosses is one the inference has no freedom about — it is atomic by the
polarity. **The two counts differ only in the case where the analysis has no
choice**, which is the sharpest form of the equivalence and the reason the gate
opens.

**The abort threshold on count overflow.** `Rc` aborts when the strong count
would pass `usize::MAX`, `Arc` when it would pass `isize::MAX`. Reaching either
needs 2⁶³ live handles, so no program distinguishes them; it is recorded because
"nothing observable" should be said with the exception named rather than without.

---

## 3. What the atomic costs

Mean over nine repeats, in nanoseconds per operation, with the range and the
standard deviation. **The `difference` column is the finding**; the absolutes are
this box on this day.

| shape | plain count | atomic count | difference | ratio |
| :--- | ---: | ---: | ---: | ---: |
| `clone+drop` | 3.759 [3.739, 3.794] sd 0.018 | 12.788 [12.732, 12.885] sd 0.043 | **+9.028** | ×3.40 |
| `read` *(control)* | 0.634 [0.622, 0.677] sd 0.018 | 0.629 [0.623, 0.643] sd 0.006 | **−0.006** | ×0.99 |
| `clone+work(4)` | 4.953 sd 0.035 | 15.489 sd 0.179 | +10.536 | ×3.13 |
| `clone+work(40)` | 7.155 sd 0.425 | 16.062 sd 0.594 | +8.907 | ×2.24 |
| `clone+work(400)` | 72.872 sd 0.241 | 73.736 sd 0.352 | **+0.864** | ×1.01 |
| `Shared[Locked[i32]]` counter | 4.403 sd 0.017 | 25.170 sd 0.060 | **+20.768** | ×5.72 |
| 4 threads, a handle each | 3.513 sd 0.309 | — | — | — |
| 4 threads, **one** handle | — | 60.405 sd 1.350 | **+56.892** | ×17.19 |

Five things in it.

**The control ties.** −0.006 ns, ×0.99, with standard deviations of 0.018 and
0.006 — so the harness resolves a difference of about 0.02 ns, and every
difference above is far outside that. It also says something about the language
rather than the harness: **a `Shared` that is read and never cloned pays nothing
at all.** The atomic is a per-clone cost, not a per-use one.

**+9 ns is a fixed cost.** At 2.80 GHz that is about 25 cycles for an atomic
increment and an atomic decrement — the right order for a pair of `lock`-prefixed
read-modify-writes on a line already exclusive in L1, which is the sanity anchor
this measurement has instead of an instruction count. The ratio is what moves,
because the ratio is a fraction whose denominator is the rest of the loop.

**And it stops being additive once there is independent work to hide under.** At
`work(400)` — a loop body of ~70 ns — the whole 9 ns is gone: +0.864 ns, ×1.01.
The work loop is not a serial dependency chain (the compiler strength-reduces it
to about 0.17 ns per unit), so this is out-of-order execution overlapping the
atomic's latency with independent instructions. The mechanism is **inferred, not
measured**: this box has no performance counters to hand and §1 forbids trusting
an absolute here anyway. What is measured is the collapse, twice (§3.1).

**The contended row is the one that would justify machinery, and the inference
cannot reach it.** Four threads hammering one count cost ×17.19 here and between
×11 and ×17 across three runs (§3.1) — tens of nanoseconds an operation, which is
the cache line moving between cores and not the atomic instruction. But a count that four threads touch is a value on four threads, and
such a value **must** be atomic: a plain count there is the data race the whole
question is fenced against. So the expensive case is exactly the case where there
is nothing to infer, and the case the inference *can* decide is the ×3.40 one,
whose absolute is 9 ns.

**The counter row is §7's finding and not this one's.** It moves both halves of
the expansion at once, which is the point.

### 3.1 The two runs

The whole table was taken twice within the hour, at 50 000 000 and 20 000 000
operations per repeat, to see which of it travels.

| difference per operation | run 1 (50 M) | run 2 (20 M) |
| :--- | ---: | ---: |
| `clone+drop` | +9.028 (×3.40) | +8.958 (×3.37) |
| `read` *(control)* | −0.006 (×0.99) | +0.001 (×1.00) |
| `clone+work(4)` | +10.536 (×3.13) | +10.451 (×3.11) |
| `clone+work(40)` | +8.907 (×2.24) | +9.566 (×2.38) |
| `clone+work(400)` | +0.864 (×1.01) | +0.899 (×1.01) |
| counter | +20.768 (×5.72) | +20.781 (×5.67) |
| 4 threads, one handle vs a handle each | +56.892 (×17.19) | +56.395 (×17.44) |

Every row agrees to within 1 %, except `work(40)` at 7 % — the noisiest row in
both runs, and the one whose two operands are closest together. That is the
within-the-hour spread and it is not the day-to-day spread
([`runtime-cost.md`](runtime-cost.md) §6.3 measured that at 1.4–1.9× on the other
box); nothing here should be quoted as an absolute for a different day.

**One row does not travel even that far, and it is named rather than averaged
away.** A third run — five repeats, taken an hour later while the box was busier
— reproduced every single-threaded row (`clone+drop` +9.348 ×3.47, control +0.015
×1.02, `work(400)` +0.444 ×1.01, counter +20.916 ×5.72) and gave **×11.33** for
the contended one against ×17.19 and ×17.44 before it. Four threads on four vCPUs
of a shared virtual machine are measuring the neighbours as much as the cache
line. What travels there is the **order of magnitude** — contended is ten to
twenty times uncontended — and nothing finer should be quoted from it.

---

## 4. What was built, and what was done by hand

**Real, and asserted by `cargo test`:**

* `benches/refcount/` — the measurement above, and the six equivalence
  assertions of §2.
* `crates/nikaia/src/contracts/sharing.rs` — the analysis. It runs over the real
  AST and the real ledgers, it is 14 tests' worth of behaviour in
  `crates/nikaia/tests/sharing.rs`, and every source those tests use is a real
  `.nika` source that really parses.
* `nikaia --sharing` — what the analysis would choose for one program, printed.
  Like `--trust` and `--overlaps`, it explains and changes nothing.

**Not real, and not simulated either — simply absent:**

* **No emission.** `Shared` is unbuilt: there is no `Rc::new` or `Arc::new`
  anywhere in the emitter, so there is nothing for a decision here to change and
  nothing was wired into a build. `sharing::rust_name` names the two Rust types
  the emitter *would* write, and is called by nothing.
* **No constructor, so no allocation site.** A `Shared` can only reach the
  analysis as a **declared type** — `fn zaehle(counts: Shared[Vec[i64]])`, which
  is also how `tests/send.rs` reaches the same type. This is the prototype's
  biggest limit and it is structural: the count belongs to the *allocation*, and
  the analysis has never seen one. Everything below about allocation classes is
  therefore about classes whose root is a parameter, which is the case where the
  answer is hardest (§5.1) rather than the case where it is easiest.
* **`Locked` is unbuilt too**, so §7's half of the question is argued from the
  specification and from one measured row, not from an implementation.

**What carried weight from what the repository already had**, since the task
asked which did and which did not:

* **`contracts::send`'s enumeration of the crossings carried all of it.** ADR-005
  §5.2's table — `task::both`, `spawn`, a scope's tasks, a value handed to a call
  whose end this compiler cannot see — *is* the seed list, translated from "which
  types may not cross" to "which values must be atomic". `send::names_used` is
  reused unchanged for the names a `spawn` body mentions.
* **[ADR-029](adr/adr-029.md)'s walk through a struct's `fields` carried §5.2's
  case**, and carried it in the opposite direction from `send.rs`: there, a field
  that may not cross makes the struct refuse; here, a struct that crosses makes
  the field's count atomic. Same ledger, same walk, reversed arrow.
* **The lattice-fixpoint house style did *not* carry weight, and that is worth
  saying.** `Plain ⊑ Atomic` is the right lattice and the fixpoint over it is
  trivial — see §4.1.
* **The whole-program ledger carried the interprocedural half** and is why a call
  *inside* one unit costs nothing: the callee's parameter is a slot the caller's
  handle is joined to, and both ends get one answer. It is also exactly what
  stops at the package boundary (§5.1).

### 4.1 The fixpoint degenerates, and the reason is the finding

The task asked whether this is a least or a greatest fixpoint and for the
justification. The honest answer is that **on this lattice the question has no
bite**, and working out why was more useful than the answer.

A count belongs to the **allocation**, not to the handle. Two handles on one
`Shared` share one count, so they cannot disagree about whether it is atomic. The
relation between handles is therefore an **equivalence**, not an ordering, and the
analysis is union-find over handles plus a single colouring pass. Every clause is
a Horn clause — *if this handle crosses, its class is atomic*; *if these two
handles are one allocation, they agree* — and the only sources of `Atomic` are the
seeds. So the least fixpoint of "is atomic" is precisely the complement of the
greatest fixpoint of "stays plain": the two framings compute the same set, and
they reach it in one step.

That is unlike `sync` ([ADR-027](adr/adr-027.md) D1), which genuinely needs a
greatest fixpoint, because its constraint is *conjunctive* over a call graph that
can cycle — `f` is `sync` if everything it calls is, and two mutually recursive
functions have to be allowed to assume each other. Nothing here is conjunctive.

**What does carry the weight is which crossings are seeds**, and that list is
ADR-005 §5.2's list. The lattice is not where the risk lives.

---

## 5. The three hard cases

### 5.1 Across a library boundary — a column does not suffice, and what would depends on a decision no record makes

**What the prototype does:** a `Shared` in a `pub fn`'s signature — parameter or
result — comes out **atomic, as undecided**, and says so. The callers are in a
unit this build never sees, so nothing here can establish that none of them
crosses.

**Why a column is not the answer, and why the shape is *not* `sync = "from(f)"`.**
`sync` is a **claim about behaviour**: the callee's compiled code is identical
whichever answer it carries, and `from(f)` merely tells the caller to compute the
answer from its own lambda. A count is a **representation**. `Rc<T>` and `Arc<T>`
are two different Rust types, so `pub fn zaehle(counts: Shared[Vec[i64]])` is two
different functions, and a published artifact has already chosen which one it is.
The three ways out, and what each costs:

1. **A column that records the choice** (`shared = "atomic"` on the entry). The
   library commits, and a consumer that needs the other count cannot call it.
   This re-creates ADR-005 Group B's own failure mode in a new place: "a library
   written at one setting cannot turn out un-compilable where it is used" becomes
   "a library whose `Shared` was inferred plain cannot be called by a program that
   crosses". A column is enough to *record* and not enough to *serve*.
2. **A representation variable plus two instantiations** — monomorphisation over
   a new axis. Every public function mentioning `Shared` doubles, every public
   type holding one becomes two types, and transitively every type holding those.
   It is the answer that works, and `Shared` becomes the first type in the
   language whose *representation* is polymorphic.
3. **The whole-program analysis simply extends across the boundary**, which needs
   the dependency's source to be lowered by *this* build. Then the ledger needs no
   column at all: it needs the **constraints** — per public function, which
   parameters and result are one class, and whether any of them crosses inside.
   That is a sharing *summary*, it composes, and the union-find above already
   computes it.

**Which of the three is available is not decided, and not by this experiment.**
Part III 13.3's manifest says it in as many words: *"How [a Nikaia
package] is resolved is not decided: no record names a registry, a name space or a
distribution format"* ([ADR-002](../specification/adr/adr-002.md) D1 §5 refuses such a
dependency today). If a package ships **source** and is re-lowered per build, (3)
is available and cheap. If it ships a **compiled artifact**, (3) is impossible and
the choice is between (1)'s broken libraries and (2)'s monomorphisation. **So hard
case one is downstream of a decision that has not been made** — which is the most
useful thing the experiment found about it, and a reason not to answer D3 before
that one.

### 5.2 A `Shared` in a struct field, reached by a value that crosses

**What the prototype does:** it follows it, and the test
`a_shared_in_a_struct_field_follows_the_struct_across` is the case. `struct
Counter { hits: Shared[i64] }`, a `Counter` built from a handle, the `Counter`
mentioned in a `spawn` body — and the handle comes out atomic. The mechanism is
ADR-029's walk through the ledger's `fields`, run in reverse, plus one slot per
`<struct>.<field>` that every handle put into that field is joined to. The
converse test asserts the same struct keeps a plain count when nothing crosses,
which is what stops the answer from being "atomic always".

**Two limits, and the second is the interesting one.**

* **It is exactly as good as the ledger's `fields`**, which are empty for every
  type whose parts are Rust — `fs::Mapped`, `Lines`. `send.rs` answers
  `Undecided` for such a type and is safe because `Undecided` is not permission;
  here the equivalent is that the analysis *does not know there is a count in
  there at all*, so it reports nothing rather than reporting atomic.
  `a_type_whose_fields_are_rust_hides_what_it_holds` asserts the silence. It is
  safe only because the `Shared` such a type holds cannot have got there except
  through a Nikaia program putting it there — which is visible, and which is
  where the join happens. **The day a Rust type constructs a `Shared` itself,
  that argument ends.**
* **A struct holding a `Shared` inherits the representation problem.** If the
  count is per value, `Counter` is two types, and so is anything holding a
  `Counter`. Within one unit that is monomorphisation the emitter would have to
  learn; across a boundary it is §5.1's case again, one level deeper. The
  prototype decides the *count* correctly here and says nothing about emitting
  two `Counter`s, because nothing emits one.

### 5.3 A crossing into a body the compiler cannot see — fail-closed, confirmed

**Confirmed.** `a_call_this_compiler_cannot_see_the_end_of_comes_out_atomic`
hands a `Shared` to `hyper_shim::across_a_thread` — the program
`docs/foreign-runtime.md` §3.3 actually ran — and the answer is atomic, marked
undecided, naming the callee. A method whose name no ledger describes is treated
the same way, which is how the corpus's one `Shared` is decided (§6).

**And here the polarity is *cheaper* than it is for `NK2502`**, which is the one
respect in which this analysis has an easier job than the check beside it.
`send.rs` needs three answers, because refusing what it cannot decide would
reject a correct program and that is the one thing this compiler may never do
(Part III C.4) — so `Undecided` can be neither permission nor refusal. This
analysis needs only two, because being wrong in the safe direction costs
**speed**: the program still compiles and still means the same thing. It is
ADR-033 D4's polarity applied where it is affordable.

---

## 6. Every case that came out atomic because nothing decided it

The polarity is non-negotiable — in doubt, atomic — so the list of cases where
"in doubt" applies *is* the cost of the design. There are four, and the prototype
marks every one of them `undecided` rather than letting it pass as a decision.

| case | why nothing decides it |
| :--- | :--- |
| a `Shared` in a **public** signature (parameter or result) | its callers are in a unit this build does not read (§5.1) |
| a `Shared` handed to a **call nothing describes** | the body may start a thread of its own ([ADR-038](adr/adr-038.md) D7) |
| a `Shared` handed to a **method nothing describes** | the same, reached by a name rather than a path |
| a `Shared` in an argument position **no contract covers** | more arguments than the signature has parameters; nothing says where it went |

And the honest fifth, which is not a case but a hole: a `Shared` inside a type
whose `fields` the ledger does not record is not decided *atomic* — it is not
seen at all (§5.2).

### The one `Shared` in the repository

`examples/fortunes.nika` is the only program in this repository that writes the
word, and it writes it once:

```nika
fn fortunes(db: Shared[postgres::Connection]) -> String throws {
    …
    let mut rows = all.fetch(db)
```

`nikaia --sharing --input examples/fortunes.nika` says:

```text
fortunes: `db`: atomic (Shared[postgres::Connection])
  could not decide: nothing written down describes `fetch`, so this compiler cannot see
  the end of it - and starting a thread of its own is among the things it may do (ADR-038 D7)

1 `Shared` value(s): 0 plain, 1 atomic, of which 1 because this analysis could not decide.
```

And the file's own comment, ten lines further down, says the opposite about the
build it was written for:

> *"At `user_parallelism = no` this is a single-threaded event loop, so `Shared`
> costs a non-atomic refcount."*

**Both are right, and that is the finding.** The per-build expansion gives `db` a
plain count because at `no` nothing the program wrote runs concurrently. A
per-value inference that may not fail open cannot give it one, because `fetch`'s
body is the driver's Rust and nothing written down says what it does with what it
is given. Describing `fetch` in a ledger does not rescue it either: `fetch` is a
library function, so its parameter is §5.1's boundary and the answer is atomic
from the other direction.

So on the only `Shared` this repository has, **the inference is not an
improvement on the switch — it is a regression**, and it would take ~9 ns per
request that the switch does not take. Against a database round trip that is
about 0.01 %, which is the other half of the same sentence: the regression does
not matter either.

---

## 7. Where it does not stretch far enough: `Locked`

Part II 12.2's counter is the program D3's open question was wanted for:

```nika
let counter: Shared[Locked[i32]] = ...
counter.access fn { a += 1 }
```

**Inferring the count per value does not make it crossable.** `Locked` expands
from the same switch, and at `no` it is "similar to a `RefCell` with a reentrancy
check" (12.2). `Arc<T>` is `Send` only where `T` is `Send + Sync`, and
`RefCell<i32>` is not `Sync` — so `Arc<RefCell<i32>>` may not cross a thread, and
a value of `Shared[Locked[i32]]` with an inferred atomic count still may not.
`send.rs` gets this right for a reason worth noting: it walks a container's
arguments, so the day `Shared` stops being the `MayNot` case, `Locked[i32]`
becomes the part that answers.

**So the inference would have to reach `Locked` too, and there the gate of §2
closes.** `RefCell` and `Mutex` are not observationally equivalent:

* **Reentrancy.** 12.2's `no` implementation *panics* on a logical deadlock — "Task
  A locks data, waits for network, Task B tries to lock same data -> Panic!". A
  real `Mutex` blocks. A program that re-enters observably does one or the other.
* **Poisoning.** 12.2 names "poisoning on several" as the `yes` safety net. A
  panic inside `access` leaves the lock poisoned there and merely released here,
  so the *next* `access` behaves differently.

12.2 says well-formed code never triggers either, and 12.3 forbids the nesting
that provokes the first — but "never in well-formed code" is a weaker claim than
"nothing observable", and it is the claim an inference of `Locked` would have to
rest on. **That is the second decision D3's question turns out to contain**, and
it is a different decision, with a different answer from the one §2 reaches.

The cost of the pair, measured: `Rc<RefCell<i32>>` against `Arc<Mutex<i32>>`,
**+20.8 ns and ×5.72** per increment — of which about 9 ns is the count and about
12 ns the lock. So the *lock* is the larger half of what Part II 12.2's counter
would pay, and it is the half whose two forms a program can tell apart.

---

## 8. The false starts

**The first run measured a loop the optimiser had deleted.** The `clone+work(k)`
rows printed `0.000` ns for the plain count and a ratio of ×167 403. Two things
had happened at once: `work(k, seed)` was called with a loop-invariant `seed`, so
it was hoisted out of the loop entirely; and a `clone` paired with the `drop` that
matches it, both visible and not atomic, is removable — so the plain row measured
nothing while the atomic row measured something. The fix is in the file: the seed
varies with the iteration, and the handle is handed to an `#[inline(never)]`
function that takes it by value, which is also the shape the emitter would write.
A ×167 403 is not a result; it is a harness reporting that it has stopped
measuring.

**The second version inflated the plain row and therefore understated the
ratio.** Wrapping the handle in `black_box` cost about 0.7 ns on both sides;
because the plain row is small, that moved ×3.40 toward ×2.90. The pair is
symmetric, so the *difference* column survived it unchanged, which is the reason
the difference and not the ratio is the finding above.

**The prototype failed open once, and the direction is the whole point.** A
`<struct>.<field>` slot is *forced* by the function that crosses with the struct
and *joined* by the function that builds one, and nothing says which of the two
the walk reaches first. The first version guarded the forcing on "this slot is
already known", so the same two functions answered `atomic` in one written order
and `plain` in the other — a data race that depended on the order somebody wrote
their functions in. It was found by writing the test for the case rather than for
the program, and the test now runs both orders. What is worth carrying out of it:
a fail-closed analysis fails open through a **guard added for tidiness**, not
through a missing crossing, and the guard read perfectly reasonably. Union-find
does not care about the order; only that guard did.

**The prototype's first corpus test asserted the wrong thing.** It asserted that
no program in the repository writes a `Shared` — and failed, because
`examples/fortunes.nika` does. The test now asserts what is actually true,
including *why* it comes out atomic, and §6 is the finding that fell out of being
wrong.

---

## 9. What the decision hangs on

Not a recommendation. Three options, with what each costs now that there is a
number, and the question each one leaves for the owner.

**A — leave it per build, as D3 has it.** A `Shared` may not cross a thread at
all, because the verdict may not consult the switch (ADR-005 §5.3). Part II 12.2's
counter stays unwritable. Costs nothing new; the price is a specified program the
language cannot express.

**B — infer it per value.** Buys at most 9 ns a clone-and-drop pair, and only at
`user_parallelism = yes`; at `no` it is a regression on the corpus's one `Shared`
(§6). Needs a representation axis in the emitter, an answer to §5.1 that depends
on an undecided distribution format, and `Locked` inferred as well (§7) — where
the observability gate that opened for `Rc`/`Arc` closes. And it would **delete
`NK25xx`'s only `MayNot` case**: `Shared` is the one type the records make
non-crossable (ADR-005 §5.3), and a `Shared` that crosses would be made atomic
rather than refused, so nothing in the language would be refused a crossing any
more. The check built yesterday would keep its `Undecided` arm and lose the arm
it was built for.

**C — one representation: `Shared` is always `Arc`.** D3's per-build expansion
drops for `Shared` only. `Shared` becomes crossable at both settings, the verdict
still never consults the switch, Group B is satisfied, Part II 12.2's counter
becomes writable the moment `Locked` is a `Mutex` — and there is no analysis to
build, no representation axis, no ledger column and no monomorphisation. The price
is the measured one: **9 ns per clone-and-drop pair, ×3.40 on a loop that is only
a count, ×1.01 on a loop with ~70 ns of work in it, and ×1.00 on a handle that is
read and not cloned.** In `send.rs` it is one line — `Shared` moves out of the
`MayNot` case and into `CONTAINERS`, where it is answered by what it holds, which
is what makes `Shared[Locked[i32]]` still refuse at `no` for the *lock's* sake.

**So what it hangs on, in the owner's hands:**

1. Is 9 ns per clone-and-drop pair — ~1 % of a 70 ns loop, 0 % of a handle only
   read — a price worth an analysis, a representation axis in the emitter, and a
   second decision about `Locked`? The measurement's own answer is that the
   expensive atomic (an order of magnitude) is the contended one, which no
   inference can avoid.
2. Is the `Locked` half acceptable? Inferring it is inferring between two
   implementations a program **can** tell apart (§7), which is a different
   question from the one §2 settled, and it is the one that decides whether Part
   II 12.2's counter is reachable at all.
3. Does a Nikaia package ship source or an artifact? §5.1 is not answerable
   before that, and no record answers it.

---

## 10. How to run it again

```sh
benches/refcount/refcount.sh                    # the table, 50 M ops, 9 repeats
benches/refcount/refcount.sh 20000000 9 4       # …quicker, and the second run of §3.1
cargo test -p refcount-bench                    # §2's six equivalence assertions
cargo test -p nikaia --test sharing             # the prototype's 14
nikaia --sharing --input examples/fortunes.nika --output /tmp/out.rs
```

The last one prints the report and then fails to *lower* `fortunes.nika`, because
its `dsl postgres { … }` block is a grammar this compiler does not have. That is
unrelated and pre-existing; the report is printed before the emitter runs, which
is deliberate — `--sharing` answers a question about the program, and a program
that does not lower still has one.

---

## 11. Which option the owner took

**C.** `Shared` is an atomic count at **both** settings
([ADR-037](../specification/adr/adr-037.md) D6), and §9's reason is the one that
decided it: the verdict of `NK25xx` is about a *type* and may never consult the
switch, so a `Shared` whose expansion moved with the switch had to be refused a
crossing at both settings — and Part II 12.2's counter is the program that wanted
one. The price is §3's measured **+9 ns per clone-and-drop pair**, about 1 % of a
loop with real work in it and 0 % of a handle that is read and not cloned; §3's
expensive row is the contended one, and no choice reaches it.

In `send.rs` it was the one line §9 said it would be, and the line cost more
around it than in it: `Shared` moving into `CONTAINERS` deleted the file's only
`MayNot` producer, so `NK2501` and `NK2502` are now built and reached by no
program. That belongs to D6 rather than here — it is a consequence of the
decision and not a finding of this experiment.

**And then B on top of it, narrowed to an optimisation.** The prototype in
`contracts::sharing` became the real thing under one rule
([ADR-037](../specification/adr/adr-037.md) D7): *it may only ever take an atomic
away.* Atomic is the floor, and the analysis may lower a particular value to a
plain count only where it **proves** nothing crosses with it. §2's gate is what
lets it — nothing a program can observe distinguishes the two counts, and the one
thing that does, a cleanup running on another thread, can only happen to a value
that crosses, which is a value the analysis has no freedom about.

So **§6's four cases stop being a cost and become the default**, and what §6
called a regression on `examples/fortunes.nika`'s `db` is now simply the floor
holding: `db` is atomic because nothing describes `fetch`, which is where it would
have been anyway. Two of §6's own limits turned out to be fail-open holes rather
than gaps, and they were found by asking what §8's guard had siblings — a handle
read back *out* of a struct field, a handle captured by a lambda handed to an
unseen call, a handle handed over as an option, a handle assigned into a field,
and a `Shared` in a public *type's* field or behind a public parameter that merely
*holds* one. And **step 1 created a seed that did not exist before**: while a
`Shared` was `MayNot`, §4's decision not to seed `task::both` was safe because the
overlapping analysis never put one on a thread; now it may, so a handle whose
allocation this analysis did not watch being made is atomic.

**What the owner did not take from §9.** There is no way for a programmer to ask
for the cheaper count — [ADR-037](../specification/adr/adr-037.md) D8 enumerates
every fallback and answers "would an override help?" no for all of them, the way
[ADR-033](adr/adr-033.md) D9 did for the ordering analysis. And
**§7's `Locked` half is not decided by any of it**: `RefCell` against `Mutex` is
observable, so it is written rather than inferred, and what an always-`Mutex`
floor would cost against Part II 12.2 is a separate question. **A** was not taken
either, and it is the option that would have left 12.2's counter unwritable.

**§9's three questions, as they stand now.** Question 1 is answered, and the
answer is not the one either half of §9 framed: 9 ns was not worth a
representation *axis*, so what was built is an optimisation on **one**
representation rather than a choice between two — which is what makes §5.1's
library boundary and §5.2's two `Counter`s stop being blocking problems, because
there is only ever one `Counter`. Question 2 is open and is `Locked`'s. Question 3
— source or artifact — no longer blocks anything here: a value the analysis
cannot decide is atomic, which is what a published artifact contains, so a
package that ships compiled code is served by the floor and one that ships source
is served better. §5.1's option (3) is what the ledger's new `sharing` column is
for.
