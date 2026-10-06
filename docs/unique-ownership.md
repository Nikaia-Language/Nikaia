# Unique ownership: a door with no lock in it

**Date:** September 22, 2026
**Status:** **an idea, recorded.** Nothing here is measured, decided or built,
and no record promises it. It is written down because it is the next door over
from one this notebook already walked up to, and because the shape of the work —
and of what would kill it — is worth having on paper before anyone starts.
**Whose:** the owner's, in this session.
**Related:** [`lock-free.md`](lock-free.md) §4–§6 (the neighbouring question,
measured), [ADR-281](specification/adr/adr-281.md) (the door it leaves open),
[ADR-037](specification/adr/adr-037.md) D7 (the closest thing to this that is
already built), [ADR-312](specification/adr/adr-312.md) D1 (what a handle does
when it is handed on), [ADR-281](specification/adr/adr-281.md) (the doors),
[ADR-010](specification/adr/adr-010.md) D1 (the polarity this would have to
meet)

## 1. The idea

Nikaia already has a ledger that infers over the whole call graph. Extend it
with **isolation**:

> Where the compiler can prove that a mutable value is handed **between** tasks
> but is reachable from exactly **one** task at any moment, no lock is needed —
> not even at `user_parallelism = "yes"`. The doors (`update`, `access`) over
> such a value could lower to plain, lock-free moves.

## 2. Why it is in character rather than exotic

**The precedent is exact and it is built.** [ADR-037](specification/adr/adr-037.md)
D7 already decides `Rc` against `Arc` **per value**, from an analysis that proves
a value never crosses a thread — one written type, two machine representations,
chosen by proving a *reachability* property. This idea is that same sentence
with *never crosses* weakened to *is never reachable from two at once*, and the
prize one step larger: not the atomic count, the lock.

**And the pattern the language already steers towards is this one.**
[`lock-free.md`](lock-free.md) §6 found it by looking rather than by assuming:
the one genuinely parallel program in `examples/`, the One Billion Row
Challenge, *shares nothing at all* — `par_fold` splits the input, folds each
piece into its own accumulator and merges them. The good pattern is unique
ownership. Today it is rewarded by having no doors to pay for; under this idea
it would also be rewarded where a program does use one.

**And a door is not free.** After the 3.7× defect `lock-free.md` §3 found and
fixed, the shipped crossing door is still work that a provably isolated value
does not need.

## 3. What it would have to answer — the part worth writing down

None of this is a reason not to do it. It is what the first afternoon would run
into, so that nobody rediscovers it.

* **Every column in the ledger today is a per-entry summary; this is not.**
  `keeps`, `touches`, `locks`, `views` and `sync` each say something about *a
  function*, and the fixpoint composes them over the call graph. *Reachable from
  exactly one task at a time* is a property of a **point in the program's
  execution**, not of an entry. It is a different kind of analysis wearing the
  same file's clothes, and the first question is whether it can be made into an
  entry-shaped claim at all.
* **A hull is a handle, and handing one on **duplicates** it**
  ([ADR-312](specification/adr/adr-312.md) D1) — deliberately, so that a cleanup
  point does not move. So *handed between tasks* is not a move in this language
  today; it is a second owner. Unique ownership would need a way to say **this
  is the only handle**, which is either a new shape or a move of the hull, and
  `NK2101` is the refusal that currently stands where that would go.
* **The polarity is the worst one in the tree.** A wrong *isolated* is a data
  race, not a slow program and not a refused one.
  [ADR-010](specification/adr/adr-010.md) D1's rule — *a wrong value is worse
  than a missing one* — means this analysis has to be **sound**, not merely
  right in practice. That is a higher bar than any column in the ledger has ever
  had to clear: `keeps` may say *kept* on doubt and cost an owned argument,
  `touches` may over-approximate, and the tether state errs wide. None of those
  have a wrong answer that corrupts memory.
* **The ledger is per package and this property is whole-program.** A value
  handed across a package boundary would need the barrier
  [ADR-283](specification/adr/adr-283.md) D5 already names for the tether: at a
  `dyn` boundary or a published API, widen to the safe answer.
* **The doors are not only locks.** `update` may run more than once
  ([ADR-281](specification/adr/adr-281.md) D14) and `access` holds the door open
  while user code runs ([ADR-281](specification/adr/adr-281.md) D10). What a
  lock-free lowering means for each is its own question, and they may not have
  the same answer.

## 4. What would decide it, and what would not

**The same gate [`lock-free.md`](lock-free.md) §6 set**, for the same reason:
the question is not *how much would it save* but **does a real program share a
mutable value across threads?** No `.nika` file in this repository uses `spawn`,
`Locked`, `Shared` or `SharedMut`; the concurrency half of the language is
exercised by Rust test snippets only. Writing a contending program to justify
this would be manufacturing the evidence.

**And the warning from that note carries too.** A server will not settle it by
throughput — a request costs tens of microseconds and a door costs nanoseconds.
What a real program settles is the **shape**: whether its data is isolated at
all, which is a fact about how it is written and not about how fast it runs.

**What would kill it** is finding that the isolation claim cannot be made
entry-shaped and sound at once — that every honest version of it is either a
whole-program analysis the ledger cannot carry, or a claim weak enough that the
door has to stay. That is a thing to find out by trying to state the rule, not
by measuring anything.

## 5. What this is not

It is not a plan, not a promise and not an issue —
an issue is for what a record decided and the compiler does not do yet, and no record decided this. It is not a `decision` issue either: those are for questions that
are **answerable**, with options and a recommendation, and this one has neither
yet. It is a note, which is what this directory is for.
