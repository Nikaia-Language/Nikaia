# What the solver is asked, and where its time goes

**Date:** October 2, 2026
**Status:** a measurement. The numbers that decided something are quoted in
[ADR-270](specification/adr/adr-270.md) D9; this file is the method and the
full tables behind them.
**Related:** [ADR-270](specification/adr/adr-270.md) (the solver's layers and
its determinism rule), [ADR-269](specification/adr/adr-269.md) D9-D10 (what
the prover asks)
**What ran:** `benches/solver-workload/` (the stress families, the analysis,
the fragment count and the run of z3 and cvc5), an instrumented copy of
`crates/nikaia-logic` at 0.0.368 (§1), and a query-dumping hook in
`crates/nikaia/src/prove.rs` that was never committed (§1)

**The conclusion, first:** the compiler asks tiny queries, almost all of them
bounds on single variables, and spends a few microseconds on each; what it
cannot answer today is one pattern, an equality with a coefficient. The
queries say how the fast paths must look and nothing about the core, which
SMT-LIB has to size.

---

## 1. The machine and the method

Intel Xeon @ 2.10 GHz, 4 vCPU, AVX2 and AVX-512 (F, BW, DQ, VL, IFMA, VBMI2,
VPOPCNTDQ) - a **shared virtual machine**. Rust 1.94 stable, `--release`.

* **The queries.** Every call of the reference solver in `prove.rs` wrote its
  query as SMT-LIB 2 (`smtlib::write`) to a directory, named by the answer the
  compiler got. Two sets:
  * **the recorded set**, 353 queries, dumped at 0.0.357 from `tests/prove.rs`,
    `tests/assert.rs` and the tests around them - the set every solver change
    since has been replayed against;
  * **the full suite**, 1 697 queries, dumped at 0.0.368 from
    `cargo test --release -p nikaia`.
* **The replay.** Each file is read back (`smtlib::read`) and answered
  in-process by a copy of `nikaia-logic` with counters: branches of the search,
  eliminations, combinations, the most bounds held at once, non-zeros of every
  derived bound, and the time inside Fourier-Motzkin. Each query is read,
  indexed and solved 20 times; the times are the mean of the 20. A *proved*
  answer's certificate is then checked 20 times.
* **The stress families** (`stress.py`): guards `x != k` for every `k` below a
  bound, unrelated `y != 0` beside a one-line proof, paths of `+1 or +2`
  steps, and `k` numbers that are each `-1` or `1` and sum to `1`.
* **z3 5.1.0 and cvc5 1.4.1** answer the same files through their Python APIs,
  a fresh solver per query (`others.py`). z3 also answered every dumped query
  once more to check the answers (§4).
* **The prover's share of a compile**: `nikaia lower` on `examples/calc` and
  `examples/inventory`, seven runs each, with the time inside `prove()`
  printed by a hook that was removed again.

## 2. The size of a query

| per query | recorded set (353) | | | | full suite (1 697) | | | |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| | median | p90 | p99 | max | median | p90 | p99 | max |
| variables | 1 | 2 | 3 | 4 | 1 | 3 | 3 | 4 |
| atoms `lin <= 0` | 2 | 8 | 10 | 12 | 2 | 5 | 8 | 12 |
| disjunctions | 0 | 1 | 3 | 3 | 0 | 1 | 2 | 3 |
| alternatives in all | 0 | 2 | 6 | 6 | 0 | 2 | 4 | 6 |
| branches searched | 1 | 3 | 5 | 5 | 1 | 2 | 3 | 5 |
| eliminations | 1 | 6 | 9 | 12 | 1 | 3 | 6 | 12 |
| bounds held at once | 2 | 3 | 3 | 3 | 2 | 4 | 4 | 4 |
| bounds derived | 1 | 5 | 6 | 8 | 1 | 3 | 6 | 8 |

* **Non-zeros per atom:** 1.01 (recorded), 1.05 (full suite); at most 1.67.
* **Density** of an atom, its non-zeros over the query's variables: median 1.00
  in both sets, because the median query has one variable. A derived bound has
  almost always eliminated every variable but the last: its density is 0.00 in
  the median and 0.02 on average over 978 derived bounds.
* **No query has more than four variables**, so every one of them fits an
  eight-lane dense kernel.

## 3. Which fragment

An atom is an **interval** atom (one variable), a **difference** atom (two
variables with coefficients `1` and `-1`) or **general**; a query falls in the
hardest fragment any of its atoms does. (Counted per comparison, so `x = y`
counts once.)

| | recorded set | full suite | stress set |
| :--- | ---: | ---: | ---: |
| interval only | 293 (83 %) | 1 153 (68 %) | 18 |
| interval or difference | 313 (89 %) | 1 413 (83 %) | 24 |
| general | 40 (11 %) | 284 (17 %) | 5 |
| coefficients of magnitude 1 | 790 | 3 593 | 892 |
| coefficients of magnitude 2 | 40 | 288 | 0 |
| larger coefficients | 0 | 0 | 0 |

## 4. What is answered, and what is missed

| | recorded set | full suite |
| :--- | ---: | ---: |
| proved | 179 | 649 |
| refuted (with a checked model) | 174 | 956 |
| unknown | 0 | **92** |

z3 agrees with every *proved* (unsat) and every *refuted* (sat) answer in both
sets. **The 92 unknown queries are four shapes**, all an equality with the
coefficient 2:

| times | the query, as asserted | z3 |
| ---: | :--- | :--- |
| 56 | `blank = 2·n`, `blank = 1` | unsat - **a proof missed** |
| 16 | `y = 2·x`, `y > 0` | sat |
| 14 | `four = 4`, `twice = 2·k`, `twice > 2` | sat |
| 6 | `d = 2·c`, `d < 0` | sat |

Fourier-Motzkin eliminates `n` over the rationals and keeps `blank = 1`, which
has a rational solution; the parity of `2·n` is lost with `n`. The satisfiable
ones fail at the model: back-substitution takes `y = 1` and then `x = 1/2`.
Eliminating an equality exactly - solving it for a variable with a unit
coefficient, or tightening by the gcd first - answers all 92; ADR-270 D8
makes them step 3's criterion.

## 5. Where the time goes

Sums over each set, in µs; *solve* includes reading the formula into atoms.

| | recorded set (353) | full suite (1 697) |
| :--- | ---: | ---: |
| SMT-LIB read (not on the compiler's path) | 743 | 3 328 |
| formula into atoms | 480 (26 % of solve) | 2 012 (28 %) |
| Fourier-Motzkin | 1 119 (60 %) | 4 113 (58 %) |
| the search around it | 265 (14 %) | 936 (13 %) |
| **solve, in all** | **1 864** | **7 060** |
| checking certificates | 458 (25 % of solve) | 1 218 (17 %) |
| solve per query, mean / median / max | 5.3 / 2.8 / 34.7 | 4.2 / 2.6 / 34.0 |

The slowest tenth of the queries takes a third (recorded) or 29 % (full
suite) of the time; the slowest are the queries with four variables, twelve
atoms and three disjunctions.

**Inside a compile:** `prove()` - frontend and solver together - took 0.07 ms
of `nikaia lower`'s 7.4 ms on `examples/calc` (1 %) and 0.63 ms of 10.2 ms on
`examples/inventory` (6 %), medians of seven runs. `nikaia lower` stops
before `rustc`, so the share of a build is smaller still.

**Repetition:** 30 of the 353 recorded queries are distinct once variables are
renamed in order of appearance (34 before renaming); 105 of the 1 697 (121).
The test suite compiles similar programs many times, so this is an upper
bound on what a result cache would hit in one project's build.

## 6. The stress families

| family | variables | atoms | disjunctions | branches | eliminations | solve, µs | z3, µs | cvc5, µs |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| guards, 32 | 1 | 68 | 33 | 67 | 67 | 500 | 2 596 | 3 800 |
| unrelated, 32 | 33 | 66 | 32 | 1 | 1 | 33 | 2 390 | 4 830 |
| `+1 or +2` path, 32 | 33 | 195 | 32 | 1 | 33 | 811 | 5 907 | 19 319 |
| parity, 8 | 8 | 34 | 8 | 251 | 2 008 | 15 526 | 3 050 | 9 398 |
| parity, 10 | 10 | 42 | 10 | 923 | 9 230 | 94 602 | 4 688 | 27 012 |
| parity, 12 | 12 | 50 | 12 | 1 025 (budget) | 12 288 | 159 909, unknown | 12 118 | 97 345 |

The z3 and cvc5 times include parsing and a fresh solver; z3 spends about
1 500 µs on a trivial query that way and 140 µs with one solver reused through
`push` and `pop`. On the compiler's queries the comparison is a comparison of
set-up costs; on parity it is a comparison of algorithms, and Fourier-Motzkin
loses by an order of magnitude at ten variables.

## 7. The size of a certificate

What [ADR-270](specification/adr/adr-270.md) D19's committed proof file would
hold, for the certificates of 0.0.368 (a split tree of refutations). *Bytes*
is a compact encoding estimated from the steps - one byte per step kind and
index, the multipliers as variable-length integers - before any text
encoding:

| | proved | steps: median | p90 | max | bytes: median | p90 | max | bytes in all |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| full suite | 649 | 3 | 6 | 6 | 10 | 22 | 22 | 7 073 |
| stress set | 22 | 35 | 455 | 9 702 | 122 | 1 643 | 34 648 | 46 379 |

The 649 proved queries of the suite are fewer than 105 distinct ones (§5), so
a project's file is a few kilobytes; the stress set's largest certificate is
parity with ten variables.

## 8. The solver's kernels, lowered from Nikaia

[ADR-270](specification/adr/adr-270.md) D8 step 1: three kernels a CDCL(T)
solver lives in, in `benches/solver-kernels.nika`, against the same
algorithms written by hand in Rust (`benches/solver-workload/kernels.rs`),
by `the_solver_kernels_lowered_against_rust_by_hand` in
`crates/nikaia/tests/measure.rs` - instructions retired under callgrind, both
halves printing the same checksums. Two further columns come from editing the
lowered Rust by hand, to find where the difference lives: every index helper
replaced by `as usize`, and only `index::at` replaced, `get` and `set` kept.

| kernel | Rust by hand | Nikaia, lowered | `as usize` everywhere | only `at` replaced |
| :--- | ---: | ---: | ---: | ---: |
| combining sparse rows, n = 200 000 | 1 018 M | 1 278 M (+25.5 %) | 1 053 M (+3.4 %) | 1 053 M (+3.4 %) |
| scanning watch lists, n = 2 000 | 10.8 M | 19.3 M (+79.6 %) | 12.6 M (+16.5 %) | 13.8 M (+28.2 %) |
| multiplying big integers, n = 20 000 | 724 M | 3 705 M (+411 %) | 1 132 M (+56 %) | 1 132 M (+56 %) |

**What the lowering has to change**, in the order of what it costs:

1. **`index::at` is the gap.** Every `xs[i]` with an `i64` index becomes
   `index::get(&xs, index::at(i))`, and `at` converts with a check for a
   negative index and a `#[track_caller]` panic path. `get` and `set` cost
   nothing; `at` costs the big-integer kernel a factor of 3.3 and the others
   most of their gap. A loop variable of `0..<n`, and a sum of such, is not
   negative - and where it is not obvious, the prover can show it
   ([ADR-269](specification/adr/adr-269.md)): an index known to be
   non-negative needs only the bounds check `Vec` already makes.
2. **What is left** (+56 % on big integers, +17 % on watch lists) is the
   index type: `i64` loop counters cast to `usize` at every access, where the
   Rust half counts in `usize` and lets the optimiser drop bounds checks; and
   `std` has no `Vec` of a given length (`vec![0; n]`), so the Nikaia half
   pushes zeros in a loop.

**Two programs that did not compile**, found on the way and worked around in
the benchmark so that it measures:

* **An index into a `mut` parameter.** `out[i] = x` where `out` is
  `mut out: Vec[u32]` is lowered as `index::set(&mut out, ...)` on what is
  already a `&mut Vec<u32>`; `rustc` refuses `&mut &mut Vec<u32>: Set`. The
  benchmark returns the vector instead.
* **A number its uses do not type.** `let variables = 2000`, used in a `u64`
  cast and as an index, is lowered as `let variables = 2000;` with no type,
  and `rustc` cannot infer what `watched[lit]` reads. The benchmark annotates
  it `i64`.

### 8.1 Closed, piece by piece (0.0.373)

Every piece of the gap above was traced to its cause and closed in the
lowering; one item of the list above was **misattributed**, and the table above
was taken against a `std` built without optimisation, which is not neutral
either. The corrections first:

* **The +56 % on big integers was not the `i64` counters.** It was `std`
  having no `Vec` of a given length: the Nikaia half grew its product by
  `push`, the Rust half allocated it with `vec![0; n]`. With `resize` in the
  ledger, and `let mut xs = []` followed by `xs.resize(n, v)` written as
  `vec![v; n]`, the kernel is at the Rust half's count.
* **The measurement linked an unoptimised `std`.** The tests find the test
  profile's `nikaia-std` beside themselves, at `opt-level = 0`, and `rustc`
  then uses that crate's unoptimised copies of generic code both crates
  instantiate - `RawVec<i64>::grow_one` among them - in place of its own: 88 M
  instructions on the sparse-row kernel that no build of a program pays. The
  measurement now builds `std` optimised, with the bitcode link-time
  optimisation reads, into a target directory of its own.

What the lowering changed, each found by reading the lowered Rust against the
hand-written one:

| piece | cost before | now |
| :--- | :--- | :--- |
| `index::at`'s sign test on every `xs[i]` | the gap of §8 | a counter that starts at a literal and only grows, and a range's binding, index `as usize`; a comparison with a length is made in `usize` |
| `count::of`'s `try_from` on a length | a branch per call | the same sign test, inlined |
| `Vec` of a given length | a `push` per element | `resize` in the ledger, fused into `vec![v; n]` |
| `&Vec<T>` parameters | a second load for every read | a list the body only reads is a `&[T]`, as text is a `&str` (ADR-282 D5) |
| a runtime started for a `main` that cannot pause | an I/O thread, so `malloc` takes locks | started on demand (`rt::on_demand`) |
| an index into a `mut` parameter; a number its uses do not type | did not compile | lowered (`&mut *out`; the range binding joins its bounds) |
| a literal no `i32` holds in a `Vec[u32]` | did not compile (`4294967295i64`) | the element's type |

**The numbers**, both halves against the same optimised `std`, from
`the_solver_kernels_lowered_against_rust_by_hand`:

| kernel | Rust by hand | Nikaia | Nikaia, `remove-bounds-checks:aggressive` |
| :--- | ---: | ---: | ---: |
| rows, `-O` | 993.3 M | 995.6 M (+0.2 %) | 998.1 M (+0.5 %) |
| rows, `-O`, `lto = fat` | 966.2 M | 959.0 M (−0.7 %) | 955.1 M (−1.2 %) |
| watch, `-O` | 10.56 M | 10.56 M (−0.0 %) | 10.45 M (−1.1 %) |
| watch, `-O`, `lto = fat` | 10.40 M | 10.33 M (−0.7 %) | 10.22 M (−1.8 %) |
| bignum, `-O` | 1 088.3 M | 1 087.9 M (−0.0 %) | 720.8 M (−33.8 %) |
| bignum, `-O`, `lto = fat` | 1 088.1 M | 1 087.6 M (−0.0 %) | 720.4 M (−33.8 %) |

**What is left, and where it lives:**

* **rows at `-O` without link-time optimisation, +0.2 %.** Not the
  lowering: the hand-written Rust with nothing changed but `use
  nikaia_std::prelude::*` at its top retires the same 1.76 M more. Linking
  `std` makes the program use `std`'s own copies of `RawVec<Vec<i64>>::grow_one`
  and `RawVecInner::finish_grow`, where alone it inlines its own; the
  remaining 0.6 M is code layout. Any link-time optimisation removes it -
  `lto = "thin"` gives 956.7 M against the Rust half's 966.2 M (−1.0 %) - and
  Part III 13.3 already names `lto = true` for throughput.
* **Overflow checks.** A Nikaia crate is built with them (Part III A.2), the
  Rust half here without. Measured with `-C overflow-checks=on` and fat LTO:
  rows 955.5 M (−1.1 % against the Rust half without), bignum 1 087.6 M
  (−0.0 %), and **watch 10.44 M, +0.7 %** - which §8.2 traces to one
  addition, `start + at`, not to `2 * v + 1` as first written. The same proof that drops an index check can
  drop an overflow check ([ADR-269](specification/adr/adr-269.md) §6's
  implicit checks); that is [ADR-271](specification/adr/adr-271.md) §6's
  next step.
* **What `basic` buys here: nothing measurable.** Its one shape is the loop
  that LLVM already proves for itself. It exists for a check LLVM does not
  see through - a list behind a parameter, one inlined across a call - and as
  the level that asks no solver.

### 8.2 What a bound on a list's values would buy (0.0.378)

For [ADR-272](specification/adr/adr-272.md). **One correction to §8.1 first:**
the watch kernel's overflow cost is not `2 * v + 1` and `c * size`. Each check
was removed by hand from the lowered Rust (`<u32>::wrapping_*`) and counted
alone, with `-C overflow-checks=on` and fat LTO:

| the watch kernel, n = 2 000 | instructions |
| :--- | ---: |
| as lowered at `aggressive`, overflow checks off | 10.19 M |
| as lowered at `aggressive`, overflow checks on | 10.44 M |
| `2 * v` and `2 * v + 1` unchecked | 10.44 M (±0) |
| `checksum += 1`, `+= 3`, `+= 7` unchecked | 10.53 M (+0.8 %, layout) |
| `start + at` unchecked | 10.11 M (−3.2 %) |

LLVM already drops what the walk of ADR-271 D4 can prove (`v < 2000`, so
`2 * v + 1` fits). The one check that costs reads `start` out of a list
(`let start = list[k]`, `list` one of `watched`), and nothing the walk keeps
says anything about a value read from memory. The same is true of two index
checks in the innermost loop, `value[blocks[k]]` and `value[arena[start +
at]]`: the position is a value out of a list.

**The upper bound of the gain**, by removing from the lowered Rust exactly the
checks that a bound on a list's values would prove, and nothing else - the
method of §8, against the same optimised `std`, fat LTO:

| | checks dropped | overflow checks on | overflow checks off |
| :--- | :--- | ---: | ---: |
| Rust by hand | - | 10.69 M | 10.37 M |
| Nikaia, `aggressive` | - | 10.44 M | 10.19 M |
| **with values' bounds** | `start + at`; `value[…]` at `blocks[k]` and at `arena[…]` | **9.68 M (−7.3 %)** | 9.76 M |
| **and a length from a filling loop** | the above; `arena[start + at]` | **9.41 M (−9.9 %)** | 9.44 M |

The values' bounds are `arena`'s, `< 2 * variables` (each is `… % (2 *
variables)`), which carries over to `blockers[*]`'s (each is one of
`arena`'s), and `watched[*]`'s, `<= (clauses - 1) * size` (each is `c * size`
with `c < clauses`). The second row also needs `arena.len() == clauses *
size`: one `push` per turn of `for _ in 0..<(clauses * size)`. Every variant
prints the same checksum.

**Sites, of the 40 of §8.1:** 22 proved today; values' bounds add 6
(`value[…]` twice, and `watched[first]`, `blockers[first]`, `watched[second]`,
`blockers[second]` at set-up); a filling loop's length adds 3
(`arena[start + at]`, `arena[c * size]`, `arena[c * size + 1]`). Of the 9
left, 5 need `x % n` (ADR-271 §6) and 4 need two lists known to be as long as
each other (`rv[i]` beside `rc[i]`, `blocks[k]` beside `list[k]`). rows and
bignum gain nothing from values' bounds: rows' arithmetic multiplies by a
parameter, bignum's `xi * y[j]` is a product of two unknowns, which no linear
fact bounds.

**What the walk costs today**, as the price a wider one starts from: `nikaia
lower --no-cache`, release build, wall clock over three runs.

| | `off` | `aggressive` |
| :--- | ---: | ---: |
| the 46 files of the repository that lower alone (8 024 lines) | 0.65 s | 0.66 s (+1 %) |
| `benches/solver-kernels.nika`, one lowering | 24 ms | 34 ms (+42 %) |

Wall clock on a shared machine, so the corpus row says "not measurable" and
the kernel row says "index-heavy code pays tens of milliseconds", no more.

### 8.3 Built (0.0.386)

ADR-272 as built, measured as §8.2 was: both options at `aggressive`,
overflow checks on, fat LTO, against the same optimised `std`.

| | `off` | `aggressive` |
| :--- | ---: | ---: |
| rows, n = 200 000 | 948.5 M | 946.7 M |
| watch, n = 2 000 | 10.44 M | **9.41 M (−9.9 %)** |
| bignum, n = 20 000 | 720.4 M | 720.3 M |

The watch row is §8.2's upper bound exactly: every check it removed by hand
is proved - `start + at`, `value[blocks[k]]`, `value[arena[…]]`,
`arena[start + at]` - and 32 index sites are written without their check
(22 under ADR-271 alone). All three print the same checksums at both levels.

**What it costs**, `nikaia lower --no-cache`, release build:

| | `off` | `aggressive` |
| :--- | ---: | ---: |
| the 46 standalone files, three runs | 0.65 - 0.71 s | 0.68 s |
| `benches/solver-kernels.nika`, one lowering | 24 ms | 40 ms |

Two changes brought the kernels there from a first build at 162 ms. A bound
was first found by searching for the least constant the solver proves -
about 30 queries a bound, 85 % of all queries; interval propagation finds the
candidate and the solver confirms each end with one query, proving the same
sites. And `emit::Needs::of`, which builds an emitter only to ask what a
program needs, proved every check a second time; it now builds it with both
options off.

**1BRC gains nothing**, as expected: `examples/1brc.nika` over a million
rows lowers to the same Rust at both levels. Its hot code is the grammar's
actions, which the walk does not enter, and `+=` in `Stats::add`, which keeps
its check (ADR-271 D5). All its overflow checks together cost 1.4 %
(481.4 M against 474.6 M with `-C overflow-checks=off`).

## 9. What this does not measure

* **SMT-LIB.** The QF_LIA and QF_LRA benchmark sets are on Zenodo, which the
  container this ran in could not reach; every statement about them in
  ADR-270 is from the literature and marked so.
* **Threads.** Everything ran on one thread.
* **The frontend's own cost** beyond the `prove()` total of §5.
