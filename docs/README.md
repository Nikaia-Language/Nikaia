# The documentation, and what belongs in each part

Four kinds of writing live here. They are separated on purpose: a reader who
wants to know what a program *means* should never have to read what a benchmark
did on someone's laptop, and an experiment that later turned out wrong should
never be able to quietly change the language.

## 1. The specification — [`specification/`](specification)

**What the language is.** Normative. If the compiler disagrees with it, one of
the two is a bug and the disagreement gets recorded.

* [Part I — The Language Core](specification/10-nikaia-light.md): the language
  a person learns first.
* [Part II — Concurrency and Metaprogramming](specification/20-nikaia-advance.md):
  concurrency, parallelism, the grammar protocol.
* [Part III — Tooling](specification/30-nikaia-tooling.md): the CLI, the
  ledger, `std`, the diagnostics contract.

The three parts follow one language standard,
[`specification/STYLE.md`](specification/STYLE.md): the rule, then its
consequence — nothing else. A page carries no ADR references, no rationale, no
history and no implementation status; those live in the decision records, the
changelog and the roadmap, and are not repeated there.

## 2. The decisions — [`specification/adr/`](specification/adr)

**Why the language is that way.** One decision per record, numbered `D1…Dn` so
it can be cited, plus the evidence that settled it — including the number, when
a number is what decided it.

Start at the [ADR index](specification/adr/README.md): every record, what it
decides, whether it is built, and what supersedes what.

An ADR is written once. When a later decision **changes** it, the change is a
new ADR that names the section or `Dn` it displaces — never a silent edit. That
is why records marked *superseded* are still here and still cited: a
supersession is **partial**, or a **consolidation** — one record that restates
several whole ones, maps every decision and decides nothing anew. Either way the
old records hold the evidence.

**But a decision that is *withdrawn* is removed, not superseded.** When the
project stops doing something altogether — a dependency it no longer has, a
mechanism it no longer builds, a path nothing takes — the records are rewritten
as though it had never been there, and one sentence says what is done instead.
The ADRs do not owe a reader every loop the project turned; they owe the reason
the current answer is the answer. Where the withdrawn thing went, and why, is
[`../CHANGELOG.md`](../CHANGELOG.md) and these notes — which is what they are
for.

The line between the two: a supersession leaves a **live citation** behind,
because something still rests on the displaced part. A withdrawal leaves none.
If after the rewrite anything still cites it, it was a supersession and the
rewrite was wrong.

## 3. The notes — this directory

**What was tried, measured and learned.** The laboratory notebook. Nothing here
is normative and nothing may depend on it to know what a program means.

* [the issue tracker](https://github.com/Nikaia-Language/Nikaia/issues) — **the running list**: what is found and
  broken, what an ADR decided and the compiler does not do yet, and which
  specification sentences a later decision made false. Every entry carries its
  reproduction or says it has none. The type says what kind it is (`Bug` for
  anything false — in the compiler, a specification page or an ADR — `Feature`,
  `Task`), a label says where (`language`, `libraries`, `tools`, `targets`,
  `selfhosting`, `compiler`, `spec`), and **Priority** and **Effort** give the
  order. Big pieces are issues with sub-issues. A closed issue stays: every entry the old `open-work.md` ever carried, closed ones included, is an issue, and the citations of its `§N.M` numbers in the CHANGELOG, the ADRs and code comments name the issue they mean. "The former backlog file" in old text means that file as a whole.
* [`open-decisions.md`](open-decisions.md) — the questions work cannot settle,
  each with what is blocked by it, the options, and a recommendation. **Only
  what is open**: an answered question leaves this file for its
  [ADR](specification/adr/) rather than staying with a note on it, so the answer
  and its reasoning live in one place.
* [`handoff.md`](handoff.md) — how to work on the **parser backend**: testing a
  change against Nikaia, what the upstream patch does, and what was tried and
  must not be redone. A guide; the open work that was in it is in
  [the issue tracker](https://github.com/Nikaia-Language/Nikaia/issues).
* [`error-corpus.md`](error-corpus.md) — twenty-six broken `.nika` files and
  what the compiler says about each, before and after.
* [`stored-views.md`](stored-views.md) — the `E0621` a stored naked view used to
  produce, the two increments of the analysis that replaced it, and what the full
  tether lattice would have to track that they do not: the seven things it needs,
  the two the compiler has, and the measurement showing that building the
  reachable half first refuses a program in `examples/` that works today.
* [`spec-promises.md`](spec-promises.md) — every construct and command the
  specification names, run against the compiler one probe at a time; the
  evidence behind the **Status** notes in Parts I–III.
* [`language-review.md`](language-review.md) — the surface language read as a
  newcomer would read it: where it contradicts its own promise, where a program
  can be silently wrong, where the syntax will make people curse, and the order
  in which turning it inside out would pay. Every claim reproduced with a probe
  or quoted from the page.
* [`solver-workload.md`](solver-workload.md) — what the prover asks its
  solver and where the time goes: 1 697 queries of at most four variables,
  83 % of them bounds on single variables or differences, 4.2 µs each, and
  the one pattern - an equality with a coefficient - behind every query it
  cannot answer. The numbers [ADR-270](specification/adr/adr-270.md) D9 rests on.
* [`subprocess-cost.md`](subprocess-cost.md) — what invoking `rustc` as a child
  process costs, against the codegen it wraps: 0.64 % of a compile, which is what
  the text interface of ADR-004 D1 is paid for with.
* [`mutex-floor.md`](mutex-floor.md) — what an always-`Mutex` floor for
  `Locked` would cost against Part II 12.2's no-pausing rule: why blocking is
  not pausing in this language, the one ledger line the answer turns on, and
  the 11 ns an uncontended acquisition costs where it can never be contended.
* [`lock-free.md`](lock-free.md) — whether ADR-281's open door is worth
  walking through: what a compare-and-swap loop would buy over the shipped
  crossing door, the **3.7×** defect the measurement uncovered on the way, and
  §6's finding that what decides it is not the saving but whether a real program
  shares a word-sized value across threads at all.
* [`unique-ownership.md`](unique-ownership.md) — **an idea, not a measurement**:
  extending the ledger with a proof that a mutable value is reachable from
  exactly one task at a time, so a door over it would need no lock. Why it is in
  character (ADR-037 D7 already picks `Rc` against `Arc` per value by proving a
  reachability property), the five things the first afternoon would run into,
  and the one that would kill it.
* [`automatic-clone.md`](automatic-clone.md) — why having the compiler insert the
  `.clone()` a detached task makes necessary is not a convenience: it adds an
  owner and so moves a cleanup point, which is the observable change ADR-037 D3
  refused to infer and ADR-005 §3 already rejected outright, so the proposal has
  to answer those records rather than appeal to typing saved.
* [`toolchain_architecture.md`](toolchain_architecture.md) — how the crates in
  the workspace stack up.
* [`project_status_and_roadmap.md`](project_status_and_roadmap.md) — what runs
  today and what is next.
* [`upstream/`](upstream) — findings and patches handed to dependencies.

This is the right home for the detail the specification must not carry: the
method, the machine, the false starts, the measurement that overturned an
intuition.

### Closed notes — [`history/`](history)

**The same notebook, kept but no longer current.** A note moves here once its
question is answered (the ADR it fed now carries the decision) or its subject
is withdrawn — never for age by itself. The method and the measurement are
still true and still cited by ADR number from wherever they are used; only the
question the page was written to answer is no longer open.

* [`staging-candidates.md`](history/staging-candidates.md) — the survey of
  where compile-time staging would pay; every candidate is closed
  ([ADR-178](specification/adr/adr-178.md)).
* [`from-for-throws-and-touches.md`](history/from-for-throws-and-touches.md) —
  whether ADR-288 D15's `sync = "from(f)"` argument carries to the other two
  effect columns; answered, and the syntax it was written against is
  superseded.
* [`std-sysroot.md`](history/std-sysroot.md) — what `std`'s build graph cost
  when `std` had the compiler as a build dependency; that dependency is gone.
* [`nightly-cost.md`](history/nightly-cost.md) — what the pinned nightly
  toolchain cost and bought; withdrawn, per `../CHANGELOG.md`.
* [`runtime-cost.md`](history/runtime-cost.md) — what a pair of operations
  costs on a running runtime; the evidence behind ADR-303 and ADR-292 D6,
  both decided.
* [`break-continue-cost.md`](history/break-continue-cost.md) — `break` and
  `continue`, priced before [ADR-276](specification/adr/adr-276.md) decided
  them.
* [`rc-or-arc.md`](history/rc-or-arc.md) — what an atomic reference count
  costs; the question ADR-037 D3 left open, closed by ADR-037 D6–D8.
* [`fixed-map-lookup.md`](history/fixed-map-lookup.md) — a fixed map's lookup,
  `match` against a perfect hash; the crossover
  [ADR-311](specification/adr/adr-311.md) D9 rests on.
* [`zero-copy-send.md`](history/zero-copy-send.md) — what it costs to answer a
  request with a file; [ADR-289](specification/adr/adr-289.md)'s evidence.
* [`foreign-runtime.md`](history/foreign-runtime.md) — a Nikaia program that
  starts `hyper`; the finding changed no decision under
  [ADR-303](specification/adr/adr-303.md) D7.
* [`technical_notes.md`](history/technical_notes.md) — compiler-internals
  findings from the `rustc_ast` path; that path is withdrawn.
* [`withdrawn-one-way-down.md`](history/withdrawn-one-way-down.md) — what the
  nightly toolchain, the Bridge-IR backend and the `rustc_ast` path were, and
  why they were withdrawn.

## 4. The history — [`../CHANGELOG.md`](../CHANGELOG.md)

**What changed, when, and what it cost.** Per release. A correction to an
earlier claim is recorded here as well as in the record it corrects.

---

## The rule of thumb

> If deleting a sentence would change what a correct Nikaia program is, it
> belongs in the **specification**. If it would only change whether you *agree*
> with the language, it belongs in an **ADR**. If it would only change how much
> you trust the number, it belongs in the **notes**.
