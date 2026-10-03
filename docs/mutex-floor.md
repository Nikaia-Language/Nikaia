# What an always-`Mutex` floor for `Locked` costs

**Date:** September 12, 2026
**Status:** an investigation. It **decides nothing**: `Locked[T]`'s
representation is the repository owner's to close, and §6 is the list of what is
left to close. Nothing here reaches the emitter.
**Related:** Part II [12.2](specification/20-nikaia-advance.md) (the lock and the
no-pausing rule) and 12.3, Part I 6.2–6.3 (`Locked` and `.access()`) and 7.2 (the
panic hook), Part III Appendix A.2 (the panic table),
[ADR-005](specification/adr/adr-005.md) D6 (the rule, and the only ADR that names
`Locked[T]` at all), [ADR-297](specification/adr/adr-297.md) D3, D6, D10,
[ADR-288](specification/adr/adr-288.md) D1, D2, D4 (`sync` earned from the
body), [ADR-288](specification/adr/adr-288.md) D15 and D16 (`from(f)`),
[ADR-037](specification/adr/adr-037.md) D2 and D3 (the switch),
[ADR-303](specification/adr/adr-303.md) D4 (whose thread the I/O thread is),
[`rc-or-arc.md`](history/rc-or-arc.md) §7 (where that experiment stopped, and why this one
starts here)
**What ran:** `benches/lockfloor/` (the measurement),
`crates/nikaia/tests/lock_floor.rs` (the question put to the real `sync`
inference), and the seven `.nika` and three `.rs` probes quoted below

> **Read as a snapshot.** The lambdas on this page are written in the form
> [ADR-277](specification/adr/adr-277.md) withdrew — `sort_by_key fn { a }`, with
> the argument read off the body — and **no sample here compiles as written**; the
> current spelling names its arguments, `sort_by_key fn(x) { x }` (Part I, 5.3).
> The analysis each section records is unaffected: what a lambda's arguments are
> called changes nothing about when it runs or what it touches, which is what this
> page is about. Transcribe the reasoning, not the code.

[`rc-or-arc.md`](history/rc-or-arc.md) §7 ended by naming the decision it could not
make: *"`RefCell` and `Mutex` are not observationally equivalent… That is the
second decision D3's question turns out to contain."* Part II 12.2 gives
`Locked[T]` two implementations, one per `user_parallelism`, and if the floor
were a `Mutex` at **both** settings then acquiring a lock could block — while
12.2 forbids the program pausing while something is locked, enforced through
`sync`. This is what that costs.

**Five findings, first.**

1. **Blocking is not pausing, and the specification already says so in the one
   place where it had to choose.** The panic hook *must* be `sync`, and Part I
   7.2 says of it: *"Blocking is allowed here — briefly."*
   [ADR-297](specification/adr/adr-297.md) D10 says it twice: *"Must be `sync`:
   mid-panic there is no runtime to pause on"* and *"**Blocking is exceptionally
   permitted**"*. [ADR-005](specification/adr/adr-005.md) D6 is titled *"No
   suspension point while a lock is held, enforced statically by `sync`"*. So
   `sync` forbids a **suspension point**, and a `Mutex` acquisition is not one.
   An always-`Mutex` floor neither violates 12.2's rule nor sidesteps it (§2).
2. **Whether `access` is still `sync` turns on one line of one ledger entry, and
   not on the primitive underneath it.** Run against the real inference: with
   `sync = "from(f)"` — the spelling [ADR-288](specification/adr/adr-288.md) D15
   built for `and_modify` — 12.2's idiom is `sync = "inferred"` and so is every
   caller above it; with no `sync` line it is not, and
   [ADR-288](specification/adr/adr-288.md) D1's greatest fixpoint takes the claim
   from every caller too. Separately, and for a reason that has nothing to do
   with the representation: **12.2's own idiom is not `sync` today** — nothing
   can resolve `counter.access`, so D2's polarity refuses it (§3).
3. **At `user_parallelism = no` the floor costs 11.3 ns an `access` and no
   syscall at all.** `Mutex<i32>` against `RefCell<i32>`, one thread,
   uncontended: **+11.3 ns and ×6.1** per `counter.access fn { a += 1 }`; the
   ratio falls to ×1.05 with 400 multiply-adds inside the guard, so it is a fixed
   cost and not a proportional one. `strace -c` counts **zero futex calls in
   1 000 000 acquisitions** — uncontended, it really is an atomic and a branch.
   `Mutex<i32>` is also *smaller* than `RefCell<i32>` here: 12 bytes against 16
   (§4).
4. **What the floor actually costs at `no` is a diagnostic, not a nanosecond.**
   12.2's `no` implementation exists to *panic* on a logical deadlock — "Task A
   locks data, waits for network, Task B tries to lock same data -> Panic!". Run
   on one thread: the borrow flag exits 101 with `RefCell already borrowed`; the
   `Mutex` prints "holding; now re-entering" and has to be killed. At `no` there
   is one user thread, so that hang is the whole program, and
   [ADR-297](specification/adr/adr-297.md) D6's *"honest limit"* already names
   this hazard class — a thread that blocks rather than pauses stops the
   cleanup deadline with it. Keeping the panic on a `Mutex` floor is an owner
   check in front of the acquisition and costs **+2.0 ns, ×1.15** (§4.3).
5. **The lead about the panic table is right in substance, wrong about the file,
   and decides more than it was credited with.**
   [ADR-037](specification/adr/adr-037.md) has no panic table and does not mention
   `Locked` at all. The table is **Part III, Appendix A.2**, it is normative, and
   its `yes` row commits `Locked[T]` to poisoning — which only a `Mutex` has. But
   Part II **12.2 already names both implementations outright**, at both
   settings. So the specification has decided this twice over and no ADR has
   decided it once: an always-`Mutex` floor is not filling a gap, it is a change
   to 12.2's `no` bullet, and under `docs/README.md`'s rule that is a record that
   names what it displaces (§5).

---

## 1. The machine, the method, and what could not be run

Intel Xeon @ **2.80 GHz**, 4 vCPU, 15 GB RAM, Linux 6.18.44 x86_64 — a shared
virtual machine with no `cpufreq` governor exposed. `rustc 1.94.1 (e408947bf
2026-03-25)`, stable, the only toolchain this repository has
([ADR-001](specification/adr/adr-001.md) D1). `--release`. Load average printed
by the script before and after every table: **0.48 → 1.73** across the first run
and **1.35 → 3.02** across the second, the rise being this measurement's own
threaded rows.

The same box class as [`rc-or-arc.md`](history/rc-or-arc.md) §1, and not the same box as
[`runtime-cost.md`](history/runtime-cost.md) §1's — so nothing here may be compared
against a number there.

**Read the ratios and the signs, not the absolutes.**
[`runtime-cost.md`](history/runtime-cost.md) §6.3 is the standing warning with a number
on it: re-running an earlier measurement on a box of this class a day later moved
every absolute figure by **1.4 to 1.9×, the baseline included**. Every row below
is reported with the two runs beside each other (§4.2), and the one row whose
ratio did *not* survive the second run is named as such rather than averaged.

### 1.1 Which half is a run and which half is an argument

`Locked[T]` is **unbuilt, in full**, and that bounds this note. The front end
accepts the type name and the method and hands both to the backend verbatim:

```nika
fn log_it(counter: Shared[Locked[i32]]) {
    counter.access fn { fs::write("log", "{a}") }
}
```

```rust
// what the emitter writes for it
fn log_it(counter: Shared<Locked<i32>>) { counter.access(|| { fs::write("log", "{a}", false, true) }); }
```

and `nikaia build` on the same file ends where it has to:

```text
error: .probe/lockproj/src/main.nika:6:1: cannot find type `Shared` in this scope
error: .probe/lockproj/src/main.nika:6:1: cannot find type `Locked` in this scope
```

`nikaia_std::prelude` has neither name. Nor is there anything for `access` to be
a method *of*: no `Locked` entry in `crates/nikaia-std/std.contracts`, no
`NK2201` anywhere in the compiler (`docs/spec-promises.md` lists it among the
codes "Part III C.3 catalogues and nothing raises"), and none of the four
constructs [ADR-288](specification/adr/adr-288.md) names as `sync`'s consumers
is built: `access`/`access_all` do not exist, `par_iter`'s methods have no ledger
entry, `task::scope fn(s) { … }` and `panic::on_panic fn(info) sync { … }` each
parse as two expressions rather than one (`docs/spec-promises.md`).

So:

| question | answered by |
| :--- | :--- |
| what the `sync` inference concludes, for each spelling of an `access` entry | **a run** — `crates/nikaia/tests/lock_floor.rs`, the real `contracts::sync::infer` |
| what the inference concludes about 12.2's idiom today | **a run** — `nikaia --input`, ledgers quoted in §3.1 |
| what an acquisition costs, contended and not | **a run** — `benches/lockfloor/`, `strace -c` |
| what re-entering one lock does in each implementation | **a run** — `reentrant.rs` (§7), both halves |
| whether a blocking acquisition is a "pause" in this language | **the specification's own words**, quoted in §2 |
| what the floor does to a `par_iter` body, a scope's tasks, the panic hook | **an argument** from §3's run, because none of the three is built |

---

## 2. Question one: is blocking on a lock "pausing"?

Part II 12.2's rule is not about acquiring a lock. It is about what happens
while one is **held**:

> **`access` and `access_all` require a `sync` lambda — at every setting.**
> …while you hold locked data, the program provably runs straight through: lock,
> compute, unlock.

The gate is on the *lambda*, and an always-`Mutex` floor does not touch it: the
lambda is the same lambda whatever the primitive is. What the floor adds is an
event at the **acquisition**, before anything is held, and the language has a
word for the event `sync` forbids and it is not "blocking":

* [ADR-005](specification/adr/adr-005.md) D6 is titled **"No suspension point
  while a lock is held, enforced statically by `sync`"**, and gives the Nikaia
  rule as Rust's *"a `MutexGuard` must not live across `.await`"*.
* The panic hook **must be `sync`** — and Part I 7.2 says of it, in bold,
  *"**Blocking is allowed here — briefly.**"*
  [ADR-297](specification/adr/adr-297.md) D10 says the same thing twice: *"Must be
  `sync`: mid-panic there is no runtime to pause on"*, then *"**Blocking is
  exceptionally permitted.** … Where it survives the hook blocks one worker"*.

A language in which blocking were a kind of pausing could not have written that
paragraph. So the answer to the question as posed is the **third** option it
offered: an always-`Mutex` neither violates 12.2's rule nor sidesteps it. It
raises the penalty in the one place 12.2 says its static rule does not reach.

### 2.1 Where the penalty lands, per setting

12.2 is explicit that `sync` is not the whole of the guarantee:

> The runtime checks described above (a reentrancy check on one thread, poisoning
> on several) remain as a safety net for the remaining edge cases — e.g.
> accidentally re-entering the *same* lock through a chain of `sync` calls — but
> well-formed code never triggers them.

That named edge case is exactly where the floor bites, and it bites differently
at the two settings.

**At `no`.** One user thread ([ADR-037](specification/adr/adr-037.md) D2), and
the runtime's I/O thread is not the user's and cannot touch a user lock:
[ADR-303](specification/adr/adr-303.md) D4's boundary is the `Op` enum, and it
still holds — `pub(super) enum Op` in `crates/nikaia-std/src/rt/worker.rs` has
exactly three variants (`Read`, `Readiness`, `Stop`), none of them carrying a
closure, and `on_io_worker()` asserts the other direction at every public entry
of `rt::io`. So a `Locked` at `no` can only ever be contended by *the same
thread*, which means only through reentrancy. There the two implementations
answer differently, and it is not a small difference:

```text
$ .probe/reentrant refcell
holding; now re-entering
thread 'main' panicked at reentrant.rs:13:26:
RefCell already borrowed
exit=101

$ timeout 2 .probe/reentrant mutex
holding; now re-entering
exit=124   (124 = still running when the timeout fired)
```

One user thread blocked is the whole program, forever, with no diagnostic — and
at `no` a panic is an abort (Part III Appendix A.2), so even the panic hook is not
a place the program could say so from. Worse, it takes the shutdown deadline with
it: [ADR-297](specification/adr/adr-297.md) D6's *"This cannot hang"* rests on two
things, a runtime-driven timer and *"locks are never held across a suspension
point"* — and D5's own **honest limit** already names the class the blocking
acquisition joins: *"FFI that blocks the thread rather than pausing will block a
single-threaded event loop and with it the deadline timer."* The floor makes that
hazard reachable from ordinary `Locked` code rather than only from FFI.

**At `yes`.** 12.2 already specifies a real OS `Mutex` here, so nothing about the
floor is new at this setting — it *is* this setting. What the floor would be
doing is exporting this setting's behaviour to the other one.

### 2.2 One thing the floor makes 12.2's rule *more* necessary for

At `yes`, user code runs on a rayon pool (`rt::Runtime::build`,
`UserCode::Concurrent`). A pool thread that blocks on a lock cannot steal work
while it waits, so a pause held across a lock at `yes` parks a worker and every
thread queued behind the lock. That is 12.2's own stated reason ("with threads it
can block a whole CPU core"), and it is the reason the rule has to be static
rather than a runtime check: by the time the acquisition blocks there is nothing
left to report to.

---

## 3. Question two: would `Locked::access` still be `sync`?

This is the load-bearing one, and it has a sharper answer than expected: **the
inference never sees the primitive.** It reads one line of one ledger entry. So
the floor decides nothing here on its own; what decides it is what the entry for
`Locked::access` is allowed to say.

### 3.1 What the compiler says today, run

`nikaia --input`, release build of this branch. 12.2's idiom, as literally as it
can be written:

```nika
fn tally(counter: Shared[Locked[i32]]) {
    counter.access fn { a += 1 }
}

fn asserted(counter: Shared[Locked[i32]]) sync {
    counter.access fn { a += 1 }
}

fn pure(n: i32) -> i32 { n * 2 + 1 }
```

```toml
[fn."asserted"]
sync = true
signature = "(counter: Shared[Locked[i32]])"

[fn."pure"]
sync = "inferred"
signature = "(n: i32) -> i32"

[fn."tally"]
signature = "(counter: Shared[Locked[i32]])"
```

**`tally` has no `sync` line at all.** Not because of anything about locks:
`counter.access` is a method call whose receiver type no ledger describes, so the
type checker cannot resolve it and [ADR-288](specification/adr/adr-288.md) D2
takes the claim away. The same happens to a `par_iter` body — and to a pure one:

```nika
fn brighten(xs: Vec[i32], counter: Shared[Locked[i32]]) -> i32 {
    xs.par_iter().for_each fn { counter.access fn { a += 1 } }
    return 0
}

fn scaled(xs: Vec[i32]) -> Vec[i32] {
    return xs.par_iter().map fn { a * 2 }.collect()
}
```

Neither gets a `sync` line: `par_iter`, `for_each`, `map` and `collect` have no
ledger entries either. And `asserted` keeps `sync = true`, because D4 never
overwrites an assertion and the *check* is deliberately permissive about a method
call (`Reached::Method`, `contracts/sync.rs`) — so today the word can be written
over an `access` and nothing contradicts it.

Two things follow for the floor. The first is that **12.2's idiom does not earn
`sync` today** — it does not reach the backend at all (§1.1), and the ledger it
produces on the way carries no claim — so there is no working behaviour for the
floor to break — what is at stake is what the entry will be allowed to say when `Locked`
is built. The second is that the answer, when it comes, will come from the
ledger, and that is where the floor has to be argued.

### 3.2 The two spellings, put to the real inference

`crates/nikaia/tests/lock_floor.rs` hands `contracts::sync::infer` the same
program, `std`'s ledger with one `Locked::access` entry appended, and the type
checker's answer for `counter.access` — which `infer` takes as a parameter
anyway ([ADR-288](specification/adr/adr-288.md)), so nothing had to be faked
except the resolution `Locked` being unbuilt denies it.

| the entry says | what the inference concludes for `tally` | …and for `outer`, which calls it |
| :--- | :--- | :--- |
| `sync = "from(f)"` | `Sync::Inferred` | `Sync::Inferred` |
| `sync = "from(f)"`, lambda does `fs::write` | `Sync::No` | `Sync::No` |
| no `sync` line | `Sync::No` | `Sync::No` |
| *(today: unresolvable)* | `Sync::No` | `Sync::No` |

Four assertions, all passing, and the middle row is the one that says `from` is
not a hole: the lambda still decides, because it runs during the call and its
calls are counted in the function that writes it
([ADR-288](specification/adr/adr-288.md) D16's bound).

So the consequence of classifying an acquisition as a pause is not "`access`
loses a label". It is that **no function that touches a lock can be `sync`**, and
therefore none can appear in a `par_iter` body, inside another `access`, in a
scope's parallel tasks or in the panic hook — the four constructs
[ADR-288](specification/adr/adr-288.md) lists. 12.2's own idiom would be
uncompilable in 12.6's own vehicle. The same chain, on functions that *are*
built, as a control:

```nika
use std::fs

fn acquire_blocking(n: i32) -> i32 {
    fs::write("lock", "held")
    return n + 1
}

fn acquire_atomic(n: i32) -> i32 {
    return n + 1
}

fn body_over_blocking(n: i32) -> i32 {
    return acquire_blocking(n) + n
}

fn body_over_atomic(n: i32) -> i32 {
    return acquire_atomic(n) + n
}

fn outer_over_blocking(n: i32) -> i32 {
    return body_over_blocking(n)
}
```

`acquire_atomic` and `body_over_atomic` come out `sync = "inferred"`;
`acquire_blocking`, `body_over_blocking` and `outer_over_blocking` come out with
no `sync` line. Assert `sync` anywhere on the blocking chain and the build stops:

```text
error[NK2202]: `par_body_asserted` is `sync`, and `body_over_blocking` can pause
  --> .probe/p5.nika:30:5
  30 |     return body_over_blocking(n)
           ^
     = a `sync` function promises it cannot pause and does no I/O (Part II, 12.1)
```

### 3.3 So what may the entry say?

`sync = "from(f)"`, on the evidence of §2: the property `sync` carries is "no
suspension point", an acquisition is not one, and the language already permits
blocking inside a `sync` context on purpose. The mechanism is built and works on
the one `from(f)` entry a program can reach today (`Vec::sort_by_key`):

```nika
use std::fs

fn pure_lambda(xs: Vec[i32]) -> i32 {
    xs.sort_by_key fn { a }
    return 0
}

fn pausing_lambda(xs: Vec[i32]) -> i32 {
    xs.sort_by_key fn { fs::write("log", "x") }
    return 0
}
```

→ `pure_lambda` is `sync = "inferred"`, `pausing_lambda` has no `sync` line.

**And here is the one place the argument bends,** which is worth writing down
because it is the strongest case against the floor.
[ADR-288](specification/adr/adr-288.md) justifies its polarity like this:

> A wrong `sync` there is a pausing body inside somebody else's lock — a
> **deadlock on one thread**, a stalled core on several, and no diagnostic
> anywhere.

A blocking acquisition inside a `sync` body produces *precisely that outcome* —
a deadlock on one thread, no diagnostic — by a different mechanism. So with an
always-`Mutex` floor, `sync` still means exactly what it says (this body contains
no suspension point) but it stops implying the thing ADR-288 wanted from it
(this body makes progress). The property is unchanged; the *reassurance* is
weaker, and it is weaker only for reentrancy, which is the case 12.2 already
concedes and 12.3 already forbids the syntax for.

---

## 4. Question three: what is free at `user_parallelism = no`?

`benches/lockfloor/` — `RefCell<i32>` against `Mutex<i32>`, five shapes, each a
pair whose halves differ in one thing. 20 000 000 operations, 9 repeats, every
repeat printed by the binary; `benches/lockfloor/lockfloor.sh` prints the machine
and the load.

### 4.1 The floor, uncontended, on one thread

| shape | first half | second half | difference | ratio |
| :--- | ---: | ---: | ---: | ---: |
| `access fn { a += 1 }` | `RefCell` 2.169 [2.117, 2.205] sd 0.026 | `Mutex` 13.406 [13.184, 13.624] sd 0.118 | **+11.237 ns** | **×6.18** |
| the same `RefCell` half twice (**control**) | run A 2.321 sd 0.008 | run B 2.221 sd 0.030 | −0.100 ns | ×0.96 |
| + 4 multiply-adds in the guard | `RefCell` 3.953 | `Mutex` 15.392 | +11.439 ns | ×3.89 |
| + 40 | `RefCell` 6.007 | `Mutex` 16.149 | +10.142 ns | ×2.69 |
| + 400 | `RefCell` 71.623 | `Mutex` 75.357 | +3.734 ns | ×1.05 |

The control ties to within a tenth of a nanosecond, which bounds every other row
from below. The acquisition is a **fixed** ~11 ns: unchanged by 4 or 40 units of
work in the guard, and down in the noise against 400. A `sync` body that does
anything at all does not notice the floor.

**It is an atomic and nothing else.** One thread, one lock, 1 000 000
acquisitions, under `strace -c`: **no `futex` call appears at all** (63 syscalls
in the whole process, every one of them process start-up). Four threads on one
lock for the same million: **2** `futex` calls. So even contended at this hold
length the cost is cache-line traffic and spinning rather than the kernel — the
`yes` penalty is a scheduling cost, not a syscall cost.

And the floor is not a memory cost here either: `std::mem::size_of` says
**`RefCell<i32>` = 16 bytes, `Mutex<i32>` = 12** on this target
(`cargo test -p lockfloor-bench`). The borrow flag is an `isize`; the futex word
is a `u32`.

### 4.2 The two runs

| row | run 1 | run 2 |
| :--- | ---: | ---: |
| `access`, uncontended | +11.237 ns, ×6.18 | +11.403 ns, ×6.06 |
| control | −0.100 ns, ×0.96 | −0.014 ns, ×0.99 |
| + 4 | +11.439, ×3.89 | +11.595, ×3.86 |
| + 40 | +10.142, ×2.69 | +10.456, ×2.60 |
| + 400 | +3.734, ×1.05 | +8.968, ×1.12 |
| owner check (§4.3) | +2.000, ×1.15 | +2.044, ×1.15 |
| 4 threads, one lock against one each | +69.660, ×3.16 | +16.220, ×1.85 |

Every single-threaded row reproduces. The `+400` row's *difference* moved (3.7 ns
against 9.0) while its ratio stayed near one, which is what a 75 ns row with a
0.6 ns standard deviation does on a loaded shared box; the finding there is the
flatness, not the number. **The threaded row does not reproduce and is reported
as a sign only**: four threads on one lock cost multiples of four threads on four
locks (×3.16 then ×1.85, with per-repeat ranges of [90, 116] and [17, 56] ns), and
the uncontended-but-threaded absolute is itself above the single-threaded one
(32.2 then 19.0 ns against 13.4) on a 4-vCPU box running four threads. Nothing
here should be quoted as *the* cost of contention; what it says is that
contention is the expensive case and that it is the only case a `no` build cannot
reach.

That last point is the whole of the answer to question three. **At `no`, a user
lock cannot be contended**: one user thread (D2), and the I/O thread cannot name
a user value (§2.1's `Op`). So the floor's cost at `no` is the uncontended
acquisition and nothing else — 11.3 ns, no syscall, 4 bytes saved — and the
expensive row is unreachable there.

### 4.3 What keeping 12.2's `no` diagnostic would cost

§2.1's hang is the floor's real price at `no`, so the obvious question is what it
costs to keep the panic. `benches/lockfloor`'s `Checked<T>` is the cheapest thing
that does: an owner field read before the acquisition, written after it, and a
panic where it already names the caller.

| | mean | difference | ratio |
| :--- | ---: | ---: | ---: |
| `Mutex<i32>`, no check | 13.404 [13.206, 13.621] sd 0.108 | | |
| `Mutex<i32>` + owner check | 15.404 [15.345, 15.467] sd 0.037 | **+2.000 ns** | **×1.15** |

Reproduced in both runs (+2.000 and +2.044). So the diagnostic 12.2 promises at
`no` is recoverable on a `Mutex` floor for 15 % of the acquisition — which makes
"the floor loses the panic" a choice rather than a consequence. (`Checked` is a
bench type. Nothing proposes it; it exists to put a number on the trade.)

### 4.4 The pair, and `rc-or-arc.md` §7

[`rc-or-arc.md`](history/rc-or-arc.md) §7 measured `Rc<RefCell<i32>>` against
`Arc<Mutex<i32>>` at **+20.8 ns, ×5.72**, and split it by subtraction: *"about
9 ns is the count and about 12 ns the lock"*. This is the lock measured on its
own, with no reference count in the shape, on the same box class: **+11.2 and
+11.4 ns**. The subtraction was right, and the lock is indeed the larger half of
what Part II 12.2's counter pays.

---

## 5. What is already decided, and where

The lead this investigation was given was that
[ADR-037](specification/adr/adr-037.md)'s panic table may already have decided
the representation at `yes`. Checked:

* **ADR-037 has no panic table**, and does not contain the word `Locked`. Its D1
  says only *"what a panic does: unwind where the machine unwinds, trap where it
  traps"*.
* **The table is Part III, Appendix A.2** (`30-nikaia-tooling.md`), it is
  normative, and its `yes` row reads: *"Resources (`Locked[T]`) held by the task
  are marked 'poisoned' so no other thread reads state a half-finished task left
  behind."* Poisoning is a `Mutex` property and a borrow flag has none — asserted,
  not argued: `a_panic_inside_the_guard_poisons_one_and_not_the_other` in
  `benches/lockfloor` panics inside a guard both ways and finds the mutex
  poisoned for every later acquisition and the borrow flag recording nothing.
* **But Part II 12.2 had already decided both settings in plain words** — `no`:
  *"Similar to a `RefCell` with a reentrancy check… It does not use OS
  primitives"*; `yes`: *"A real OS-level **Mutex**"*. Part I 6.2–6.3 names the
  type and `.access()` without saying which.
* **No ADR decides it.** The only ADR that names `Locked[T]` is
  [ADR-005](specification/adr/adr-005.md) D6, which decides the *rule* (`access`
  takes a `sync` lambda at every setting) and mentions the representation only in
  passing — *"shrinks the divergence of `Locked[T]` between settings. The runtime
  check and lock poisoning stay as backstops"*. It records the divergence; it does
  not decide it. [ADR-292](specification/adr/adr-292.md) D3's touch sets name a
  `Locked` value as a resource, which is orthogonal.

So the question is narrower than it looks, but not in the direction the lead
guessed. It is not that `yes` is decided and `no` is open: **both are decided in
Part II 12.2, and neither is decided in an ADR.** An always-`Mutex` floor is
therefore a change to a normative sentence, and `docs/README.md`'s rule for that
is a new record naming what it displaces — not a gap to fill silently. Two
consequences ride along:

* Appendix A.2's `yes` row is a **second, independent** commitment to a
  poisoning primitive at `yes`, reached from the panic behaviour rather than from
  the lock. A floor that is a `Mutex` everywhere satisfies it at both settings;
  the current per-setting split satisfies it only at `yes`, which is consistent —
  at `no` a panic is an abort and there is no surviving task to poison anything
  for.
* `rc-or-arc.md` §7's blocked path opens exactly this far: `Arc<RefCell<i32>>` is
  not `Send`, so a `Shared[Locked[i32]]` can only cross a thread if `Locked` is a
  `Mutex`. An always-`Mutex` floor is the one shape of `Locked` under which
  [ADR-037](specification/adr/adr-037.md) D3's per-value question could ever reach
  12.2's own counter. That is an argument *for* the floor, and it is the only one
  this investigation found that is about capability rather than cost.

*(This bears on [ADR-037](specification/adr/adr-037.md) and is written here
rather than there: that record is being edited on another branch, and a note may
not change a decision anyway.)*

---

## 6. What the owner now has to decide

Three things, in this order. None of them is decided here.

1. **Whether a blocking acquisition counts as a pause.** §2 says the
   specification's own words already answer *no* — the panic hook is `sync` and
   may block — and §3.2 shows what the *other* answer costs: no function that
   touches a lock can be `sync`, and 12.2's idiom cannot appear in 12.6's
   vehicle. If the answer is `no`, `Locked::access` gets `sync = "from(f)"` when
   it is built and the floor is free in the inference.
2. **Whether 12.2's `no` reentrancy panic is a promise or a nicety.** It is the
   only thing an always-`Mutex` floor actually takes away at `no` (§2.1), it
   costs +2.0 ns / ×1.15 to keep (§4.3), and 12.2's own text calls it a safety net
   for code that is not well-formed. A floor that keeps it is a `Mutex` with an
   owner check; a floor that drops it turns a panic into a hang that also stops
   [ADR-297](specification/adr/adr-297.md) D6's deadline.
3. **Whether the floor is worth its one capability.** 11.3 ns and no syscall at
   `no` (§4), against `Shared[Locked[T]]` becoming a thing that can cross a
   thread at all (§5). That is the trade, and the second half is the part
   `rc-or-arc.md` §7 could not reach.

And one thing that is **not** about the floor at all, found on the way: **Part II
12.2's idiom is not `sync` today** and will not be until `Locked` has a type the
checker can resolve `access` through (§3.1). Whatever the representation turns
out to be, that resolution is the work that makes Chapter 12's gate real, and
`sync = "from(f)"` on the entry is what makes the idiom compile.

---

## 7. How to run it again

```sh
benches/lockfloor/lockfloor.sh                    # the table, 20 M ops, 9 repeats
benches/lockfloor/lockfloor.sh 20000000 9 4       # …with the thread count named
cargo test -p lockfloor-bench                     # the four observable differences
cargo test -p nikaia --test lock_floor            # the question, put to the inference
```

The `.nika` probes in §3 are quoted in full above and take one `nikaia --input`
each. The three hand-written Rust probes are below, and take one `rustc -O` each.
None of the ten is committed: a probe worth keeping is a test, and the ones that
were worth keeping are `crates/nikaia/tests/lock_floor.rs` and the four
assertions in `benches/lockfloor`.

```rust
// reentrant.rs — §2.1. `refcell` exits 101; `mutex` never exits.
let which = std::env::args().nth(1).unwrap_or_default();
if which == "refcell" {
    let cell = RefCell::new(0i32);
    let outer = cell.borrow_mut();
    println!("holding; now re-entering");
    let inner = cell.borrow_mut();
    println!("re-entered: {} {}", *outer, *inner);
} else {
    let lock = Mutex::new(0i32);
    let outer = lock.lock().unwrap();
    println!("holding; now re-entering");
    let inner = lock.lock().unwrap();
    println!("re-entered: {} {}", *outer, *inner);
}
```

```rust
// syscalls.rs — §4.1, under `strace -c -f`. No futex call appears.
let lock = Mutex::new(0i64);
for _ in 0..1_000_000 {
    let m = black_box(&lock);
    *m.lock().unwrap() += 1;
}
println!("{}", lock.lock().unwrap());
```

```rust
// syscalls_contended.rs — the same million, four threads on one lock: 2 futex calls.
let lock = Mutex::new(0i64);
std::thread::scope(|s| {
    for _ in 0..4 {
        s.spawn(|| {
            for _ in 0..250_000 {
                *lock.lock().unwrap() += 1;
            }
        });
    }
});
println!("{}", lock.lock().unwrap());
```
