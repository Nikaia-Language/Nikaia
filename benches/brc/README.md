# The One Billion Row Challenge: `examples/1brc.nika` against Rust

[`examples/1brc.nika`](../../examples/1brc.nika) describes the whole input file
as a grammar and aggregates it with a fold. This bench measures it against the
same aggregation written in Rust, twice:

| program | what it is |
|---|---|
| [`naive.rs`](src/bin/naive.rs) | what a competent Rust programmer writes first: `read_to_string`, `str::lines`, `split_once`, a `HashMap` with the default hasher |
| [`tuned.rs`](src/bin/tuned.rs) | what a 1BRC entry looks like before it is parallelised: `mmap` (`unsafe`), raw bytes with **no UTF-8 validation**, one `memchr` searcher over the whole file, the separator found by walking back from the line's end, integer temperatures, a word-at-a-time hash |

`tuned` is the comparison that counts. `naive` is there so that neither number
is read alone.

## How it is measured

```sh
benches/brc/brc.sh            # the table below, 1 000 000 rows
benches/brc/brc.sh 200000     # quicker
```

* **Counts, not a clock.** Instructions and mispredicted branches per row,
  counted by callgrind (`--branch-sim=yes`) over the whole process. On a shared
  machine the same binaries swap places from one timed round to the next; a
  count does not move.
* **The same answer first.** Every program's output is compared byte for byte
  with `naive`'s, including Nikaia's at `user-parallelism = "yes"`, before
  anything is counted.
* **Release builds on both sides.** The Rust programs are `cargo build
  --release` (`opt-level = 3`, no overflow checks). Nikaia's is a project built
  with `nikaia build` at `opt-level = 3`, its own optimizations on, no debug
  assertions; it keeps the overflow checks the language promises. **Neither side
  uses link-time optimisation.** Each Rust program is a binary of its own: in one
  binary with the others, LLVM did not inline `tuned`'s hash-table entry and the
  count rose by 40 a row.
* **Nikaia without overflow checks** is the same project built with
  `RUSTFLAGS="-C overflow-checks=off"`, to say what the checks cost.
* **Length in tokens**, by [`scripts/tokens.py`](../../scripts/tokens.py), the
  same rule for both languages: identifiers, keywords, numbers, literals and
  operators count, comments and whitespace do not, `use` lines do. Each file is
  counted whole.
* **The input** is `gen`'s: 413 stations, the count the real file has, about a
  quarter of the names non-ASCII, from a fixed seed.

## The table

Nikaia 0.0.459, rustc 1.97.0, Intel Xeon @ 2.10 GHz; callgrind, 1 000 000 rows,
per row:

| | instructions | mispredictions | tokens |
|---|---:|---:|---:|
| Rust, `naive` | 860.6 | 8.62 | 486 |
| Rust, `tuned` | 272.2 | 3.70 | 809 |
| **Nikaia** | **300.1** | **1.58** | **618** |
| Nikaia, overflow checks off | 298.2 | 1.58 | |

Against `tuned`, Nikaia runs a tenth more instructions and less than half the
mispredictions, from a quarter fewer tokens, with no `unsafe`, with UTF-8 and
every overflow checked; the same file also runs on every core. The overflow
checks cost 1.9 instructions a row. `naive` is the shortest program and the
slowest by far.

## Where Nikaia's instructions go

Callgrind's attribution by source file, per row, for the table's Nikaia row:

| | instructions | mispredictions |
|---|---:|---:|
| the hash table (`hashbrown`, through `std`'s `HashMap`) | 57.4 | 0.04 |
| finding the `;` and the line's end (`memchr`) | 40.0 | 0.00 |
| comparing a name with the one in the table | 33.8 | 0.19 |
| the temperature's digits and arithmetic (`core::num`) | 29.1 | 0.26 |
| cutting the text (`core::str`) | 27.9 | 0.00 |
| the grammar's runtime (`winnow-grammar`, `winnow`) | 22.9 | 0.48 |
| the generated rules and actions | 22.0 | 0.01 |
| hashing a name (`nikaia-std`'s byte hash) | 17.2 | 0.49 |
| the UTF-8 check (`simdutf8`) | 2.4 | 0.09 |
| the rest, SIMD intrinsics of `memchr` and the UTF-8 check among it | 43.3 | 0.02 |

`tuned` is one function after inlining, so callgrind cannot split it the same
way.

## What is left to build

| what | where | issue |
|---|---|---|
| updating the stats: the `and_modify` / `or_insert_with` chain through the hash table | the lowering | [#420](https://github.com/Nikaia-Language/Nikaia/issues/420) |
| what a grammar matched and what a callee promises, to prove the temperature's overflow checks away (at most 1.9 a row) | the prover | [#421](https://github.com/Nikaia-Language/Nikaia/issues/421) |
| a grammar over bytes, with only what becomes text checked as UTF-8 | language, `winnow-grammar`, `std` | [#422](https://github.com/Nikaia-Language/Nikaia/issues/422) |

## What this does not claim

Nothing about the 1BRC leaderboard. The entries there are SWAR over raw bytes,
hash tables sized to the key count with the keys stored inline, no UTF-8
anywhere, and every core. `tuned` is a single-threaded, readable version of that
idea and stops short of it. The question this bench answers is whether a
grammar that describes the file costs you the loop a specialist writes.

## The files

* `src/bin/naive.rs`, `src/bin/tuned.rs` - the two Rust programs, each a binary
  of its own.
* `src/bin/gen.rs` - the input: `gen <rows> <file>`.
* `brc.sh` - builds everything, checks the outputs agree, counts, prints the
  table.
