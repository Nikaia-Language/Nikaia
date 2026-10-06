# What a compare-and-swap loop would buy, and the defect finding it uncovered

**Two findings, and the second one was not the question.** The question was
whether [ADR-281](specification/adr/adr-281.md)'s open door is worth walking
through: `update` takes a pure function and is therefore repeatable, which is the
route to an implementation that **retries instead of locking** — the route
Clojure's `atom` and Haskell's `TVar` took.

The answer to that is *maybe, and narrowly*. What came out on the way is that the
shipped crossing lock cost **63.5 ns per door** where it should cost 17, and the
reason was in how its owner check was written. That is fixed, and every locked
value in every program that may cross a thread is **×3.7** cheaper for it.

## 1. The machine and the method

4 cores, Linux 6.18.44 x86_64 — a shared virtual machine with no `cpufreq`
governor exposed. `rustc 1.94.1 (e408947bf 2026-03-25)`, `--release`. Load average
0.25 → 0.31 across both runs.

The method is [`mutex-floor.md`](mutex-floor.md) §1's, unchanged, because that
document solved this problem already: **read the ratios, not the absolutes**;
two runs side by side rather than averaged; every repeat printed; a control that
must tie; and the threaded row reported as a **sign** and never as a cost.
`benches/lockfree` is the run.

**What the control says about resolution.** The same shape measured twice came
out ×1.07 and ×1.10 — 7 to 10% apart, where `mutex-floor.md`'s control tied to
within 4%. These rows are 1.4 ns, so a percentage of them is nothing; what it
means is that **a ratio inside ±10% is not a finding here** and the rows below
are read with that in mind.

**It measures what the emitter writes.** `benches/lockfloor` deliberately measured
`RefCell` against `Mutex`, because neither was yet what the compiler emitted; the
two shapes in `nikaia_std::lock` *are* what it emits now
([ADR-281](specification/adr/adr-281.md), [ADR-281](specification/adr/adr-281.md)),
owner check and all. Measuring anything else would be measuring a shape no program
gets.

## 2. The rows

`update fn(alt) { alt + 1 }`, uncontended, one thread, ns per door:

| row | run 1 | run 2 |
| :--- | ---: | ---: |
| `lock::Local<i64>` — the cheap shape | 1.797 | 1.512 |
| `lock::Crossing<i64>` — the crossing shape, **after** §3's fix | 17.340 | 17.953 |
| compare-and-swap loop | 11.663 | 11.317 |
| `fetch_add` | 6.349 | 6.435 |
| control: the cheap shape twice | ×1.07 | ×1.10 |

And the ratios the decision turns on:

| | run 1 | run 2 |
| :--- | ---: | ---: |
| the loop against the **cheap** shape | +9.867 ns, **×6.49** | +9.804 ns, **×7.48** |
| the loop against the **crossing** shape | −5.677 ns, **×0.67** | −6.636 ns, **×0.63** |
| `fetch_add` against the loop | −5.315 ns, ×0.54 | −4.882 ns, ×0.57 |

**Four threads on one value — a sign and not a cost.** Crossing against the loop:
×0.19 and ×0.19 in the first run, ×0.32 and ×0.36 in the second, on absolutes that
moved from 187 ns to 96. The direction reproduces and the number does not, which
is what a 4-vCPU shared box does with four threads
([`mutex-floor.md`](mutex-floor.md) §4.2 is the standing example). What it says is
that contention is where the loop wins, not how much.

## 3. The defect this found

**The first table said the crossing shape costs 63.5 ns.**
[ADR-281](specification/adr/adr-281.md) measures its owner check at **+2.0 ns,
×1.15** against a bare `Mutex` — so either the record was wrong or the
implementation was not the thing the record measured. It was the second.

`Crossing::hold` did three things per door that it did not need to do:

* the owner mark was a **second `Mutex`**, taken and released before the
  acquisition and taken again after it — **three** mutex acquisitions per door;
* who is asking was a **SipHash of the thread id, computed on every call**;
* and the mark was written **before** the acquisition, so while one task waited
  the mark said *its* name although another task held the lock. That is a
  correctness point and not only a cost: the mark did not name the holder.

All three are gone. The mark is an `AtomicU64` written **under** the guard, so it
names the holder; it is still **read** before the acquisition, because once that
blocks there is nothing left to report to — and a racing read says nothing about
*our own* re-entry, which is the only thing it answers, since only we ever write
our own id there. Who is asking is computed once per thread.

**Measured on the same box, within the hour:** 63.5 → 17.3 ns, **×3.7**. And a
hand-written equivalent now ties with the shipped one (×0.96, ×1.08), which is
what says the fix is the fix.

## 4. What the numbers mean for the door ADR-281 left open

**The loop must not replace the cheap shape.** ×6.5 to ×7.5 against it, and that
shape is what *every* value gets at `user_parallelism = no` and what a value gets
at `yes` wherever nothing crosses a thread with it. A borrow flag is one
non-atomic write; a compare-and-swap is a locked instruction with cache-line
traffic. There is no version of this where the loop wins there.

**Against the crossing shape it wins, and by less than it looked.** ×0.63 to ×0.67
uncontended — about **6 ns a door** — and several times that under contention,
where the sign reproduces. Before §3's fix the same comparison read ×0.18, which
would have been an argument; it was an argument about a defect.

**Recognising the operation is worth about as much again.** `fetch_add` is ×0.54
to ×0.57 of the loop. So *"add one"* specifically could be about half of what
*"any pure function"* costs — which is a second, smaller decision and not this one.

**So the shape of the answer is narrow:** a third representation for a
**word-sized** value that may cross a thread, replacing the crossing shape and
never the cheap one. Everything else keeps the row it has today, because the
choice is already made per value.

## 5. What it would still need

* **A block that may be repeated.** `sync` says a block does not *wait*. It does
  not say it has no **effect** — run, not argued: a `println` inside an `update`
  compiles and prints today, so a retry would print twice.
  [ADR-281](specification/adr/adr-281.md) calls the missing property *"an
  additional assurance which does not exist yet"*, and **half of it arrived while
  this was being written**: `touches` is inferred over the call graph since
  [ADR-288](specification/adr/adr-288.md) D22, so a user-written function answers
  what it reaches, and a body that prints inside an `update` reports
  `["lock write", "stdout write"]`. *"Touches nothing, and it is known"* is very
  nearly *"repeating it is unobservable"*; what is left is to say that they are
  the same thing. `get` and `set` need no repetition at all and could take the
  shape first.
* **Not a value in a door over several locks.** `update_all` holds both at once
  ([ADR-281](specification/adr/adr-281.md)), and a retry loop cannot be held. Such
  a value has to keep a lock.
* **Nothing new in the dependency tree.** The atomics are in `std`. And
  `crossbeam-utils`, `crossbeam-epoch` and `crossbeam-deque` are already compiled
  into every generated program by way of the pool — so the machinery a *general*
  `T` would need (safe reclamation: you cannot free the old value while another
  thread may still be reading it, which is what a garbage collector does for
  Clojure and Haskell and what nothing does for us) is present but unused. That
  case is not what these numbers are about.

## 6. What this is, and what it is not

**Built since:** [ADR-281](specification/adr/adr-281.md) is the third
representation §4 asks for: a word-sized `SharedMut` whose crossing is proven
and which no door over several locks holds is `nikaia_std::lock::Word`, and
everything else keeps the row it had.

**Decided since:** [ADR-281](specification/adr/adr-281.md) D14 permits the
retry — an `update` block may run more than once — and D2 says where: on a
copy, for a value that fits a machine word. The block's form is `fn(mut v)`
either way; what changes is what `v` is. The shape question below is unchanged.

**It is a performance idea, and it is not a question waiting on the owner.** It
was on the former `open-decisions.md` for a while and has been taken off
deliberately, because that page is for questions whose answer somebody has to
give before work can continue — and nothing is waiting on this one. The measured
win is 6 ns a door on the values that cross a thread. Nothing is half-built, no
record promises it, and no program is slower for its absence in a way anyone has
shown.

**The reason it has no urgency is worth stating, because it was found by looking
rather than assumed.** No `.nika` file in this repository uses `spawn`, `Locked`,
`Shared` or `SharedMut` — the concurrency half of the language is exercised by
Rust test snippets only. And the one genuinely parallel program in `examples/`,
the One Billion Row Challenge, shares nothing at all: `par_fold` splits the input,
folds each piece into its own accumulator and merges them, which is why it has no
lock to contend. That is the good pattern, and it is the pattern the language
steers towards.

So the question that decides this is not *"how much would the loop save"* — that
is measured, above — but **"does a real program share a word-sized value across
threads?"**. Writing a contending program to find out would be manufacturing the
evidence; the honest route is to wait for a program that needs it. A server is the
likeliest one, and `http` is now a package that can grow into it
([ADR-289](specification/adr/adr-289.md)).

**And a warning for whoever measures it there.** A web server will not settle this
by throughput. A request costs tens of microseconds and a door costs six
nanoseconds — four orders of magnitude apart, under a network stack. What a real
server settles is the **shape** question above, by what its code has to do. The
numbers are already here.
