# What the solver is asked, and where its time goes

**Date:** October 2, 2026
**Status:** a measurement. The numbers that decided something are quoted in
[ADR-267](specification/adr/adr-267.md) D1; this file is the method and the
full tables behind them.
**Related:** [ADR-265](specification/adr/adr-265.md) (the solver's layers and
its determinism rule), [ADR-264](specification/adr/adr-264.md) D9-D10 (what
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
coefficient, or tightening by the gcd first - answers all 92; ADR-267 D12
makes them step 1's criterion.

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

## 7. What this does not measure

* **SMT-LIB.** The QF_LIA and QF_LRA benchmark sets are on Zenodo, which the
  container this ran in could not reach; every statement about them in
  ADR-267 is from the literature and marked so.
* **Threads.** Everything ran on one thread.
* **The frontend's own cost** beyond the `prove()` total of §5.
