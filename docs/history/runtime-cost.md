# What an operation costs on a runtime that is already running

**Date:** September 11, 2026; §6 added September 12
**Status:** measured; the numbers it produced are [ADR-038](../specification/adr/adr-038.md) §4.3's
evidence, and §6's are [ADR-033](../specification/adr/adr-033.md) D10's — including what a re-run of
§2 on the same box a day later says about how far an absolute microsecond here travels
**Related:** [ADR-038](../specification/adr/adr-038.md) D3 (the I/O split) and D4 (the runtime starts
before `main`), [ADR-033](../specification/adr/adr-033.md) §8.4 (the 46 µs per-pair wake-up this is
measured against), §8.5 (the prediction this settles) and D10 (the lowering §6 measures),
[ADR-037](../specification/adr/adr-037.md) D2 (whose thread the I/O thread is)

[ADR-033](../specification/adr/adr-033.md) §8.5 wrote down a prediction and said in the same
sentence that there was nothing to time: *"the sidecar thread is already running; two overlapped
reads are two messages, not two thread starts… **This is a prediction, not a measurement** —
there is no event loop, no sidecar and no async lowering yet."* All three now exist - the
async lowering last, in [ADR-055](../specification/adr/adr-055.md) §6, whose step 3 also moved
the *waiting* out of the operation and into the executor. The numbers below were measured
before that and are about the mechanism rather than about who waits for it: the syscalls
are the same syscalls, and what changed is which frame is parked in them.
This is the measurement, its method, its machine, its spread, and the two things it found that
were not predicted.

**The conclusion, first.** The prediction is right, and it is right for a narrower reason than
it gave. A pair of operations on the pre-started runtime costs **nothing measurable** — but only
on the completion path. The blocking fallback, on the same pre-started threads, pays **~38 µs a
pair**. "The runtime is already running" removes thread *creation*; it does not remove thread
*wake-up*, which is what §8.4 said the floor was. What removes the wake-up is that `io_uring`
involves no thread at all — which is
[ADR-038](../specification/adr/adr-038.md) D3's argument, now with a number under it.

---

## 1. The machine and the method

Intel Xeon @ 2.10 GHz, **4 vCPU**, 15 GB RAM, Linux 6.18.44 x86_64 — a shared virtual machine
with no `cpufreq` governor exposed, and a box that other work was using throughout: the load
average was **1.5 to 2.1** before and after every block, and it is printed by the script beside
every table rather than assumed away. `rustc 1.94.0-nightly (8d670b93d 2025-12-31)` — the
compiler the block was run under, kept for that reason and not because this repository names it:
since [ADR-001](../specification/adr/adr-001.md) D1 the channel is stable. `--release`. `io_uring` is available on this
kernel and `io-method = "auto"` chose it, which the binary prints before it measures anything.

Same machine class as [ADR-033](../specification/adr/adr-033.md) §8.4's "4 vCPU container", so the
46 µs it measured and the 59 µs measured here are comparable rather than merely both being
numbers.

**The program** is `benches/overlap/src/bin/runtime.rs`, run by
`benches/overlap/runtime.sh`. It reads the same two files three ways and times each:

| shape | what it is |
| :--- | :--- |
| `seq` | `fs::read(a)` then `fs::read(b)` — the sequential baseline |
| `join` | `task::both(\|\| fs::read(a), \|\| fs::read(b))`, which is `rayon::join` — §8.4's vehicle |
| `both` | `fs::read_both(a, b)` — both in flight on the pre-started runtime |

**All three go through the same `std::fs`, and therefore through the same mechanism.** That is
what makes the subtraction meaningful: `join − seq` and `both − seq` are the cost of the
*vehicle* and of nothing else, because the reads underneath them are the same reads. The absolute
columns are not compared across mechanisms — see §4 for why that would be wrong.

**The runtime is started once**, by `rt::start`, exactly as the `fn main` the emitter writes
starts it. Nothing timed below pays for starting it; that is the whole of what D4 claims.

**Warm-up** is 64 pairs of each shape before the first repeat, so no measurement includes a cold
page cache, a cold `rayon` pool or the first `io_uring_enter`.

**Repeats.** Nine, each 20 000 pairs (4 000 at 256 KiB, 1 000 at 1 MiB). Every repeat is printed
by the binary; the mean, the range and the standard deviation below are over those nine. Files
are `base64` of `/dev/urandom` truncated to the exact size — incompressible, and warm in the page
cache after the warm-up.

**The two mechanisms are both measured, not one measured and one assumed.** The script runs the
whole table twice: once with `io-method = "auto"` (which chose completion here) and once with
`io-method = "blocking"` pinned, which is the route an older kernel or a sandbox that forbids the
syscalls would take. `io-workers = 2` in both, so the fallback has a thread to overlap a pair
*with*; with one worker a pair on the fallback runs one after the other, which is the honest
answer for a machine that has neither a completion queue nor a thread to spare.

---

## 2. What a vehicle adds per pair

The number that answers D4. Mean over nine repeats, with the range and standard deviation.

### Completion (`io-method = auto`, which chose `io_uring`)

| bytes/file | `seq` | `join − seq` | `both − seq` |
| ---: | ---: | ---: | ---: |
| 5 | 3.34 µs | **+59.17** [57.81, 61.53] sd 1.36 | **−0.25** [−1.29, 0.63] sd 0.65 |
| 4 096 | 3.75 µs | **+60.28** [58.71, 63.29] sd 1.74 | **−0.48** [−1.23, 0.37] sd 0.63 |
| 65 536 | 10.39 µs | +67.26 [58.62, 75.43] sd 4.64 | −0.61 [−2.17, 0.86] sd 1.22 |
| 262 144 | 40.71 µs | +95.85 [82.80, 108.38] sd 7.23 | −0.86 [−7.25, 9.11] sd 4.58 |
| 1 048 576 | 400.66 µs | +291.00 [245.12, 349.67] sd 38.18 | +31.22 [−87.88, 109.02] sd 61.89 |

### Blocking fallback (`io-method = blocking`, pinned)

| bytes/file | `seq` | `join − seq` | `both − seq` |
| ---: | ---: | ---: | ---: |
| 5 | 2.77 µs | +51.91 [43.65, 55.47] sd 3.83 | **+37.87** [36.84, 39.53] sd 0.89 |
| 4 096 | 2.93 µs | +48.15 [41.25, 51.37] sd 2.93 | **+37.72** [31.96, 39.66] sd 2.33 |
| 65 536 | 8.02 µs | +52.66 [49.88, 55.73] sd 1.64 | +37.32 [35.72, 38.53] sd 1.11 |
| 262 144 | 139.91 µs | −56.79 [−71.31, −30.95] sd 12.52 | **−77.90** [−91.70, −55.52] sd 10.90 |
| 1 048 576 | 893.52 µs | −669.46 [−777.54, −592.99] sd 69.59 | −704.48 [−813.83, −572.12] sd 91.63 |

The rows to read are the first two of each table: at 5 bytes and 4 KiB the payload is small
enough that the vehicle *is* the answer. The 1 MiB row of the completion table is the noisiest
thing here (sd 62 µs on a mean of 31) and nothing is concluded from it — at that size a single
read is 200 µs and a neighbour's build moves it.

---

## 3. What the numbers say

**§8.4 reproduces.** `join − seq` is 59 µs where §8.4's `fixed` said 46. The difference is that
`fixed` timed a pair with *nothing in it*; here each closure also performs a read, so the stolen
closure's wake-up sits on the critical path of real work rather than beside it. Same floor, same
shape, a little more of it. The number is stable — sd 1.4 µs over nine repeats of 20 000 pairs —
so it is the vehicle and not the weather.

**D4's claim holds, and the answer is zero.** `both − seq` on the completion path is **negative**
at every size up to a quarter megabyte, with a spread that straddles zero: −0.25, −0.48, −0.61,
−0.86 µs. There is nothing to find. The kernel performs both reads, one `io_uring_enter`
collects them, no thread is started and none is woken — and the ring was already there, because
the runtime started before `main`. ADR-033 §8.5's "none in principle" is "none, measured".

That the mean is very slightly *below* zero is not a discovery, it is arithmetic: two reads in
one `io_uring_enter` is one syscall where `seq` pays two, so `both` saves a syscall and loses
nothing. The saving is smaller than the noise, which is why the honest statement is "zero".

**The finding nobody predicted: the fallback does not inherit D4's zero.** `both − seq` on the
blocking path is **+37.9 µs**, and it is *flat* — 37.87, 37.72, 37.32 µs at 5 bytes, 4 KiB and
64 KiB, sd under 2.5 µs throughout. A fixed cost that does not move with the payload is a
wake-up, and that is what it is: the pair is one message to a thread that is already parked, and
parking and unparking a thread costs what parking and unparking a thread costs however early it
was started.

So "the runtime starts before `main`" is **necessary and not sufficient**. D4 removes the thread
*start*; only D3's completion path removes the thread. Put the two tables side by side and
ADR-038's §1 is no longer an argument from the literature: *that pool **is** the 46 µs* is now a
measurement of this repository's own fallback.

**And the crossover is where ADR-033 put it.** On the fallback `both − seq` turns negative
between 64 KiB (+37 µs) and 256 KiB (−78 µs) — [ADR-033](../specification/adr/adr-033.md) §8.2's
"crossover sits near **256 KB** of payload per operation", found again by a different vehicle on
a different mechanism, which is the kind of agreement that makes a number worth trusting. The
completion path has no crossover, because a fixed cost of zero has nothing to amortise.

---

## 4. One confound, named rather than hidden

The `seq` columns of the two tables **are not comparable to each other**, and nothing above
compares them. At 1 MiB the fallback's baseline is 894 µs and the completion path's is 401 µs —
a factor of 2.2 that is far too large to be the mechanism.

Part of it is known and is not about `io_uring` at all: `std::fs::read` sizes its buffer from
`stat` and then calls `read_to_end`, which finds the buffer full, **grows it** and reads again to
see the end of the file — so a 1 MiB read reallocates and copies a megabyte before it returns.
The ring path allocates `size + 1` and so reaches end-of-file without growing. The rest is not
attributed, and on a 4-vCPU box under load it is not worth attributing.

**It does not touch the conclusion**, because every number the conclusion rests on is a
difference *within* one mechanism: `join − seq` and `both − seq` compare vehicles over identical
reads. A cross-mechanism claim would need a quiet machine and an instruction count, which is the
standard `docs/staging-candidates.md` §2 sets and which this measurement deliberately does not
claim to meet. What it claims to meet is the standard `benches/overlap/README.md` sets: the
machine, the method, the repeat count and the spread, all stated.

---

## 5. How to run it again

```sh
benches/overlap/runtime.sh                     # the whole table, both mechanisms
benches/overlap/runtime.sh /tmp/somewhere 9    # …in a named directory, nine repeats

# one size, one mechanism, by hand
cd /tmp/somewhere
printf 'io-method = "blocking"\nio-workers = 2\n' > nikaia-runtime.toml
NIKAIA_RUNTIME_CONFIG=$PWD/nikaia-runtime.toml \
  cargo run -p overlap-bench --release --bin runtime -- 20000 9
```

The script prints the machine, the load average before and after each block, and the runtime's
own report of what it started on — `io-workers=2 io-method=auto (chose completion) user-pool=4
cleanup-deadline=30s user-code=concurrent` — so a table can always be read back to the
configuration that produced it.

---

## 6. The same thing end to end, and what the re-run found

**Date:** September 12, 2026 · the numbers [ADR-033](../specification/adr/adr-033.md) D10 and §8.5.1
rest on

§1 to §5 measure `nikaia_std` from Rust. That answers what the *function* costs and not what the
*lowering* costs, and ADR-033 D10 is a lowering: at `user_parallelism = no` a pair of `std` file
reads is emitted as `nikaia_std::task::read_pair`. So the same question was put to a `.nika`
program through the whole pipeline — the ordering analysis, the emitter, `cargo`, the runtime.

### 6.1 The method

`benches/overlap/pair.nika`, built twice by `benches/overlap/lowered.sh` from **one source**: once
at `ordering = "effects"`, where the two reads lower onto the completion pair, and once at
`ordering = "strict"`, where they are two statements one after the other. `user_parallelism = no`
in both, which is the point — before D10 there was no overlap to measure there at all. The script
greps the emitted Rust for `task::read_pair` and prints what it found, so a measurement of two
identical binaries cannot pass for a result.

The program reads the same two files in a loop and sums their lengths; nothing else in the loop
body performs an operation, so the pair is the pair and the loop is the repeat count. **200 000
pairs per run, nine repeats, timed as whole-process wall clock** — the two binaries start the same
runtime and differ only in the pair, so process start cancels in the difference and is 0.01 µs a
pair besides. A 64-pair warm-up precedes every block. Both of
[ADR-038](../specification/adr/adr-038.md) D3's mechanisms are measured, because D10's decision is
about the second one.

**The machine:** the same class as §1 — Intel Xeon @ 2.10 GHz, 4 vCPU, 15 GB RAM,
Linux 6.18.44 x86_64, a shared VM with other work on it. Load average **1.0 to 1.7** through every
block, printed by the script. `rustc 1.94.0-nightly (8d670b93d 2025-12-31)`. `io-workers = 2`.

One thing to know about the build: a Nikaia project has **one** profile (`dev`, with `opt-level`
from `[build.x86_64-linux]`), so a lowered program runs with `debug_assertions` on, where §2's
figures are `--release`. Turning them off with `CARGO_PROFILE_DEV_DEBUG_ASSERTIONS=false` moves the
baseline from 5.70 to 5.38 µs a pair and leaves the difference at −0.80 µs (sd 0.20): it is a tax on
both binaries and not on the lowering. The headline rows below are what `nikaia build` actually
produces.

### 6.2 What the lowering costs

Mean over nine repeats of 200 000 pairs, 5 bytes a file, with the range and standard deviation.

| mechanism | `strict` baseline | `effects − strict`, per pair |
| :--- | ---: | ---: |
| completion (`io-method = auto`) | 5.70 µs | **−0.87** [−1.05, −0.62] sd 0.14 |
| blocking fallback (pinned) | 4.01 µs | **−0.05** [−0.23, +0.07] sd 0.10 |

Both rows are D10 working as written. The overlap is **free and slightly better than free** where
the kernel performs the pair — one `io_uring_enter` where the sequential program pays two, the
saving §3 already explained as arithmetic. And it is **exactly nothing** on the fallback, because
there the lowering does not overlap at all: D10 refuses the fallback's per-pair wake-up rather than
paying it, and the measurement is what says the refusal is in the code.

What makes that second row worth the run: `fs::read_both`, which *does* overlap on the fallback,
costs **+53.98 µs** a pair on this box (§6.3). The same pair, the same mechanism, through the
function the compiler chose instead: −0.05 µs. That is the difference between the two `std`
functions being a decision rather than a naming accident.

### 6.3 The re-run, which is the finding

The numbers above do not match §2's, and the gap is **the machine and not the lowering**. The
control is the same binary §2 was measured with, run on the same box within the hour:

| `benches/overlap/runtime.sh`'s binary, 20 000 pairs × 9 | §2, Sept 11 | §6, Sept 12 |
| :--- | ---: | ---: |
| `seq` baseline, completion | 3.34 µs | 5.3 µs |
| `both − seq`, completion | −0.25 (sd 0.65) | **−0.98** [−1.37, −0.70] sd 0.23 |
| `join − seq`, completion | +59.17 (sd 1.36) | **+111.81** [+107.50, +115.31] sd 2.08 |
| `both − seq`, blocking | +37.87 (sd 0.89) | **+53.98** [+50.43, +55.10] sd 1.43 |
| `join − seq`, blocking | +51.91 (sd 3.83) | **+92.31** [+84.32, +99.35] sd 4.71 |

Everything on this box-day is 1.4 to 1.9 times §2's figure, the baseline included. A shared virtual
machine is not the same machine from one day to the next, and §4 already said a cross-mechanism
claim here would need a quiet box and an instruction count.

**So the end-to-end number and the `std` surface agree**: −0.87 µs a pair for the lowering against
−0.98 µs for the function it calls, measured hours apart on one box. There is no cost hiding in the
emitter, which is what the end-to-end run was for. What there is instead is a reminder with a
number on it: **§2's absolute microsecond figures are not reproducible on a different day on this
hardware, and its conclusions are — because every one of them is a sign, a flatness, or a ratio.**
`both` is zero on completion and a flat fixed cost on the fallback; `join` is two orders of
magnitude worse than either; the fallback's crossover is where ADR-033 §8.2 put it. None of that
moved.

A number quoted from §2 in an ADR should therefore be read as "about this, on a 4-vCPU shared VM",
which is how [ADR-033](../specification/adr/adr-033.md) D10 quotes it.

### 6.4 How to run it again

```sh
benches/overlap/lowered.sh                          # both mechanisms, 200 000 pairs, 9 repeats
benches/overlap/lowered.sh /tmp/somewhere 20000 3   # …in a named directory, quicker
benches/overlap/runtime.sh /tmp/elsewhere 9         # the control above
```

---

## 7. What being `async` costs a program that never overlaps

**Date:** October 1, 2026 · [#106](https://github.com/Nikaia-Language/Nikaia/issues/106)

Everything above measures the **mechanism**, and was taken before the waiting moved into the
executor ([ADR-055](../specification/adr/adr-055.md) §6). What had not been measured is what the
`async` lowering costs the program that never uses it: one that reads, then reads again, and
never has two operations in flight.

**The method.** `benches/awaits.nika` reads one small file `n` times, one read after the other,
each three `.await`s deep. Three variants, one tree and one emitter
(`crates/nikaia/tests/measure.rs`, `what_an_await_costs_a_program_that_never_overlaps`):

* **blocking** — the emitted Rust with `async`, `.await` and `block_on` taken out and the read
  made `std::fs::read_to_string`: what the lowering wrote before ADR-055;
* **async, the read blocking** — `async` and the executor kept, only the read made blocking: the
  futures without the hand-off;
* **async (as lowered)** — the program as it is.

Instructions retired under callgrind, every thread counted; the runtime is started in all three.
Kernel work is not in an instruction count, so the syscalls a read makes are in none of them.

| `user_parallelism` | reads | blocking | async, the read blocking | async (as lowered) |
| :--- | ---: | ---: | ---: | ---: |
| `no` | 100 | 665,003 | 665,337 (+0.1 %) | 1,020,613 (+53.5 %) |
| `no` | 1,000 | 2,404,726 | 2,430,260 (+1.1 %) | 5,936,428 (+146.9 %) |
| `yes` | 100 | 778,621 | 767,875 (−1.4 %) | 1,086,714 (+39.6 %) |
| `yes` | 1,000 | 3,070,124 | 2,815,235 (−8.3 %) | 6,185,811 (+101.5 %) |

**What it says.**

* **An `.await` costs nothing measurable.** Three of them per read, and the state machines and
  the executor's polling come to **3 to 25 instructions a read** at `no` — inside the noise at
  `yes`, where the thread interleaving moves the totals by more than that. The `async` lowering
  itself is free for a program that does not overlap.
* **The read's road through the runtime is not.** About **3,100 to 3,500 instructions a read** at
  both settings: handing the read to the runtime's I/O and taking the answer back, against a
  `read(2)` on the caller's own thread — roughly 2.8 times what the whole blocking read costs in
  user space. It is the same at `no` and `yes`, so it is the I/O path and not the pool.

**What it decided.** That road is what makes two reads in an `overlap` cost nothing (§2), and a
program that never overlaps pays it on every read. [ADR-263](../specification/adr/adr-263.md)
answers it: on a runtime with nothing else in flight, a file operation runs on the calling thread
- once the normal path is cheapened (§7.2) and if it still measures.

### 7.2 Where the instructions go

Both profiles of the 1,000-read run at `user_parallelism = yes` (2,819,075 instructions blocking,
6,140,340 as lowered: **3,321 a read**), compared function by function under
`callgrind_annotate`. What the lowered program spends a read on that the blocking one does not:

| what | instructions a read | where |
| :--- | ---: | :--- |
| the ring: submit, wait, reap, poll its slot | ~1,030 | `file_ring::Ring::{submit_one, reap, poll_slot, begin}`, `Submitter::submit_and_wait`, `rt::io::poll`, `park_for` |
| allocation and copying | ~830 | `malloc`/`free`/`calloc`, `memset`, `memcpy`, `finish_grow`: the read's buffer is **zeroed** before the kernel fills it (`vec![0u8; size + 1]`), and copied on its way out |
| the executor parking and waking | ~450 | `exec::block_on` and its queue, polled again after every pause |
| the text of an error nobody reports | ~425 | `Utf8Chunks::next`: `fs::read_to_string` writes the path into a `String` on **every** call, for the message it would give if the read failed |
| a lock taken under contention | ~80 | `Mutex::lock_contended` |

About 2,800 of the 3,321 are named; the rest is spread too thinly to name. Two of the five rows are not
the hand-off at all - the zeroed buffer and the eager error text - and cost the same whichever way
the read is made; [ADR-263](../specification/adr/adr-263.md) D3 removes them first, so that what
D1 of that record saves is measured on its own.

### 7.3 After the normal path stopped doing what it need not

[ADR-263](../specification/adr/adr-263.md) D3, measured with the same harness on the same tree
(October 1, 2026): the ring's read buffer is offered to the kernel as capacity rather than zeroed
first, and `fs` writes a path into the text of an error only when there is an error.

| `user_parallelism` | reads | blocking | async (as lowered) | for being async, a read | before (§7) |
| :--- | ---: | ---: | ---: | ---: | ---: |
| `no` | 1,000 | 2,402,439 | 5,016,835 (+108.8 %) | **2,614** | 3,531 |
| `yes` | 1,000 | 2,663,838 | 5,156,686 (+93.6 %) | **2,492** | 3,115 |

About 900 instructions a read, a quarter of what a program that never overlaps paid, gone without
changing what any operation does. What is left is the ring, the executor and the copy out of the
buffer - the hand-off itself, which is what D1 of that record is measured against.

### 7.4 How to run it again

```sh
cargo test -p nikaia --release --test measure what_an_await -- --ignored --nocapture
```
