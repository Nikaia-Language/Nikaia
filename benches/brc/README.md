# What the 1BRC grammar costs against the same loop written by hand

[`examples/1brc.nika`](../../examples/1brc.nika) claims that a `grammar` which
describes a whole file is worth writing instead of the loop you would write by
hand. The claim is worth exactly as much as the loop it is measured against, so
this is that loop — twice, because beating a straw man proves nothing.

| shape | what it is |
|---|---|
| `naive` | what a competent Rust programmer writes first: `read_to_string`, `str::lines`, `split_once`, `HashMap<&str, _>` with the default hasher |
| `tuned` | what a 1BRC entry looks like before it is parallelised: `mmap`, raw bytes with **no UTF-8 validation**, one `memchr` searcher over the whole file, the separator found by walking back from the end of the line, integer temperatures, a word-at-a-time hash |

Both are a **pair** with the example in the sense the other benches here use:
everything differs except the aggregation and the output, and the output is
compared byte for byte before any clock is read — so a difference in speed is
never a difference in what was computed.

```sh
benches/brc/brc.sh                  # the table: 8M rows, best of 5
benches/brc/brc.sh 2000000 3        # smaller and quicker
```

The script generates its own input, because the benchmark's own 13 GB file is
not something to keep in a repository and a measurement without one is not
reproducible. **413 stations**, which is the count the real file has: at fifteen
the table lives in L1 and every hash looks free. About a quarter of the names
carry non-ASCII characters, as the real ones do — an all-ASCII file lets a UTF-8
validator run eight bytes at a time, and the compiler's side of the comparison
would look better than it is ([ADR-016](../../docs/specification/adr/adr-016.md) §3).

## How it is measured

Read this before quoting a number from this page. Every figure below says which
of these it is.

**The box.** 4 cores, Intel Xeon @ 2.10 GHz, 8 MiB L2 per core, Linux
6.18.44, rustc 1.97.0, valgrind 3.22.0. `brc.sh` prints the machine with its
table, and so should any quotation of it.

**The input.** `handwritten gen`, 413 stations, about a quarter of the names
non-ASCII: 8 million rows (123 MB) for the clock, 1 million for callgrind. Every
program's output is compared byte for byte with the others before anything is
counted.

**The builds.** The two Rust halves are `cargo build --release`: `opt-level =
3`, 16 codegen units, no debug assertions, no overflow checks. Nikaia's is
`nikaia build` with what `brc.sh` writes: `opt-level = 3`, `incremental =
false`, and its own optimizations on. Its generated profile has no debug
assertions (since 0.0.419; ADR-002 D5), keeps Cargo's 256 codegen units, and
checks overflow in the program's crates, because Part I says an overflow
aborts. **Its optimizations on or off make no difference here**: the program
has no index, and no arithmetic the prover can show stays in range, so both
settings emit the same Rust and link the same binary. Figures taken before
0.0.419 carried Cargo's `dev` defaults instead - debug assertions on, and with
them the standard library's precondition checks in every inlined function -
and are only comparable with each other.

**Instructions** are callgrind's total over 1 million rows divided by the row
count, start-up included (under one a row); **mispredictions** are its
`--branch-sim`. They are deterministic, and they are what this page uses to
say *where* time goes.

**The clock** is taken two ways. `brc.sh` runs each program five times,
unpinned, and keeps the best; Nikaia's runs go through `nikaia run`, which
re-checks the project first, so its rows include that. For a comparison of two
builds of one program, the variants are interleaved, pinned to one core
(`taskset -c 2`), best of 25, and the binary is run directly.

**What the clock can and cannot say on this box.** A difference under about
10 % is not a result. Round after round, the same binaries have swapped places:
`tuned` measured 0.225 s in one interleaved round and 0.259 s in the next, and
two builds 0.343 s and 0.415 s in one round came out the other way round in
the following. `docs/history/runtime-cost.md` §6.3 has absolutes moving by
1.4–1.9× from one day to the next. **Only the ratios travel, and only the large
ones.** Where a change is smaller than that, its instruction and misprediction
counts are the measurement, and the clock is quoted only to say that it did not
move.

## The table

`brc.sh`, 8 million rows, best of 5, Nikaia 0.0.419:

| | cores | best | against `tuned` |
|---|---:|---:|---:|
| Rust, `naive` | 1 | 0.74 s | 3.4× |
| **Nikaia, `user-parallelism = "no"`** | 1 | **0.49 s** | **2.2×** |
| Rust, `tuned` | 1 | 0.22 s | 1.0 |
| Nikaia, `user-parallelism = "yes"` | 3 | 0.31 s | 1.4× |

The same programs pinned to one core, binaries run directly, interleaved, best
of 25 - so without `nikaia run`'s check, and without the cores `fs::map`
validates on at either setting:

| | instructions per row | mispredictions per row | one core |
|---|---:|---:|---:|
| Rust, `naive` | 862.6 | 8.74 | 0.668 s |
| **Nikaia** | **390.1** | **4.28** | **0.371 s** |
| Rust, `tuned` | 312.7 | 3.61 | 0.234 s |

Four runs, one output. That the two Nikaia rows of `brc.sh` agree is
[ADR-296](../../docs/specification/adr/adr-296.md) D10's whole claim — the chunk
count does not change the result — checked rather than asserted, and the script
fails if it ever stops being true.

### What the rows say

**Against the loop you would actually write, the grammar wins**: 0.371 s
against 0.668 s on one core, and less than half `naive`'s instructions. That is
the row the example's claim turns on, and it is why both halves are here:
measured against `tuned` alone the grammar looks 1.6× slow, and measured
against `naive` alone it looks 1.8× fast. Neither number means anything
without the other.

**The rest against `tuned` is not all parser overhead.** `tuned` works on bytes
and makes text only of the 413 distinct names, once each, when it prints them;
a stray byte in a temperature it reads as a wrong number without noticing.
`std::fs::map` checks the whole file, because every `&str` view a grammar cuts
out of a mapping depends on it
([ADR-016](../../docs/specification/adr/adr-016.md) D1: without the check, the
program prints a station name that is not text). Where the rest of it is, line
by line, is in the sections below.

**The parallel row is not a 4× speed-up over the sequential one**. The input is
read once either way and the UTF-8 check is already spread across cores at both
settings, so what the four cores divide is the parse and the fold, not the
whole program.

## A lowercase `par_fold` rule, and what its whitespace cost (0.0.388)

`examples/1brc.nika` writes its entry rule `entry rule file` - lowercase, so
syntactic. Its entry point skipped no whitespace (ADR-296 D10), but the rule
itself still skipped at its start and before every item of the fold, until
`winnow-grammar` 28cf576. That broke the one thing `par_fold` promises: at
`user-parallelism = "no"` a station ` b` lost its leading blank and a blank
line between rows passed, while at `"yes"` the blank stayed and the blank line
failed. And on every valid row the skip ran and found nothing.

Callgrind over 1 000 000 rows of `handwritten gen` (413 stations), every
program printing the same:

| | instructions per row |
|---|---:|
| Rust, `naive` | 860 |
| Nikaia, before the fix | 658 |
| **Nikaia, with it** | **587 (−10.8 %)** |
| Rust, `tuned` | 312 |

On the clock it is smaller - 0.51 s to 0.49 s over 8 million rows, best of 5 -
because a skip that always finds nothing is a branch that is always predicted.
The rows above, 8 million, same box: `naive` 0.91 s, Nikaia 0.49 s, `tuned`
0.33 s, Nikaia on four cores 0.23 s.

What is left between the grammar and `tuned` is the parser itself: per row
about 205 instructions in the rule, the temperature read a character at a
time and `until(";" | frame_end)` searched line by line, against `tuned`'s one
`memchr` over the file and fixed-width indexing. The hash table is the
cheaper of the two (about 85 against 124). That is the next question for the
grammar, and it is a decision of its own.

## The temperature by index, and one search per line kept (0.0.418)

Both shapes named above were built in `winnow-grammar` and measured one at a
time; the record is its
[ADR 24](https://github.com/keywan-ghadami/winnow-grammar/blob/main/docs/adr/adr24-frames-and-bounded-formats.md)
§8–§10:

- **`"-"? digit{1,2} "." digit` is matched by index.** A run of literals,
  `digit`s and bounded `digit{m,n}` in a lexical rule is a few byte
  comparisons and one advance in the fast pass, where it was four parser
  calls and a repetition with a checkpoint per digit. A failed match, and the
  diagnosing pass, run the elements as before, so no error changes.
- **One `memchr` over the file instead of one `memchr2` per line was slower**:
  706 against 608 instructions per row upstream and +17 % on the clock,
  because a line of fifteen bytes was then searched twice instead of once.
  `tuned`'s walk back from the line's end to the `;` would only pay without
  the forward search, and it finds the *last* `;` where `until(";" |
  frame_end)` means the first - it accepts `a;b;1.0`, which the grammar
  rejects. It measured 649.

Here it was 587 → 566 instructions per row, with the profile Nikaia then
inherited (debug assertions on), and nothing on the clock.

## No debug assertions, `dec[i32]`, a separator as a byte (0.0.419)

Three changes, each measured on its own with callgrind over 1 million rows,
Nikaia's optimizations on and `incremental = false` throughout:

| | instructions per row | mispredictions per row |
|---|---:|---:|
| 0.0.418, as `cargo` defaulted the profile | 516 | 4.22 |
| **no debug assertions** in the generated profile (ADR-002 D5) | 438 | 4.55 |
| and **`whole:dec[i32](digit{1,2})`** in `TENTHS`, instead of the text and a `chars()` loop | 417 | 4.13 |
| and **a one-byte literal compared as a byte** (`winnow-grammar` bbc8683, ADR 24 §8a) | **390** | **4.28** |

`incremental` on costs this program 50 instructions a row (566 against 516,
taken with the debug assertions still on) and saves a third of a second on a
rebuild after an edit (1.56 s against 1.90 s); it stays on by default and
`brc.sh` turns it off. `codegen-units` at 16 or 1 instead of 256 moved nothing
(437–439).

`dec[i32](…)` was made for exactly this field. Folding it further into the
indexed match - the value accumulated while the digits are matched - was built
and measured upstream, as a loop and unrolled, and came out even with `dec`'s
own `FromStr` once mispredictions are counted; it is not in (ADR 24 §8b).

## Where the rest is, against `tuned` (0.0.419)

Callgrind, 1 million rows, per row; Nikaia attributed by the source file each
instruction came from, `tuned` by its own lines:

| what | Nikaia | `tuned` | Nikaia − `tuned` |
|---|---:|---:|---:|
| finding the `;` and the line's end | 57 instr, 0.00 mispred. (`memchr2` per line) | 93, 1.23 (`memchr_iter` 46 + the walk back 47) | **−36** |
| the rest of the parse: temperature, separators, the fold's loop, the stream | 134, 0.88 | ~50, 0.26 | **+84** |
| hash table and key compare | 137, 2.48 | 131, 1.69 | +6, **+0.79** |
| updating the stats | 23 | 11 | +12 |
| UTF-8 validation (`fs::map`) | 35.5, 0.89 | 0 | +35.5 |
| **total** | **390, 4.28** | **313, 3.61** | **+77** |

- **Finding the separator is not where `tuned` wins.** One `memchr2` per line
  is cheaper than its one `memchr_iter` plus the walk back, and predicts better.
- **The parse's 134** are `str` slicing and its character-boundary checks
  (about 27), the arithmetic and digit tests of `core` (24), the fold's loop in
  `winnow-grammar` (21), the generated rules and the action (17) and winnow's
  stream (13).
- **The hash** costs about what `tuned`'s does, but mispredicts more: the
  tail of `nikaia_std::hash::FxHasher` branches on the key's length three
  times, where `tuned` copies the tail into a word.
- **UTF-8** is the check `tuned` does not make and `fs::map` must (ADR-016 D1).

The compiler itself does not move with any of this: `nikaia lower` on
`crates/nikaia-std/src/tools/ty.nika`, the largest Nikaia source in the tree,
went 382.54 M → 382.56 M instructions with the same Rust out across
`winnow-grammar` 28cf576 → 9446f25.

## The UTF-8 check with SIMD (0.0.424)

The check `tuned` does not make was the largest single thing left: 35.5 of 390
instructions a row and 0.89 of 4.28 mispredictions. `checked-text` now checks
each piece with `simdutf8`, which answers as the standard library does, verdict
and offset ([ADR-016](../../docs/specification/adr/adr-016.md) D4):

| | instructions per row | mispredictions per row | one core | unpinned | all-ASCII twin, one core |
|---|---:|---:|---:|---:|---:|
| `std::str::from_utf8` | 390.0 | 4.28 | 0.332 s | 0.305 s | 0.301 s |
| **`simdutf8`** | **364.9** | **3.42** | **0.305 s** | **0.278 s** | 0.302 s |

Same output, and a file with a stray byte is refused with the same message at
the same offset. The clock columns are under this box's 10 %, and quoted for
what they rule out: on no input, pinned or not, is the check slower than it
was. The all-ASCII twin is the case where the standard library's own fast path
was already good, and there the two are the same.

What is left of the check is about 10 instructions a row. Making it nothing,
as `tuned` does, would take the grammar reading bytes and the text check moved
to what becomes text - here only the names, and of those only each distinct one
- which ADR-016 §4 defers as a decision about what a view is.

## The hash of a name (0.0.430)

`nikaia_std::hash::FxHasher` finished a key with three branches on what was
left of its length, and a station name's length changes from row to row. Text
is now folded into one word by the byte hash of `rustc-hash` 2 first, which
reads a key of up to 16 bytes as two overlapping words; numbers keep the Fx
step they had ([ADR-316](../../docs/specification/adr/adr-316.md) D5).
Callgrind, output compared first and the same everywhere:

| `Trusted` hash of text | 1BRC, per row | `k-nucleotide`, whole run | `nikaia lower ty.nika` |
|---|---:|---:|---:|
| Fx, three tail branches (before) | 364.9 instr, 3.42 mispred. | 1292 M, 3.26 M | 383.4 M, 3.20 M |
| Fx with an overlapping tail of its own | 340.2, 1.57 | 1228 M, **4.47 M** | |
| `foldhash`, fixed seed | 356.7, 1.63 | **1347 M**, 1.27 M | |
| `rustc-hash` 2, whole | 345.8, 1.62 | 1208 M, 1.32 M | |
| **`rustc-hash` 2's byte hash, Fx for numbers** | **344.1, 1.58** | **1204 M, 1.29 M** | **383.3 M, 3.20 M** |
| the same, 4-16 bytes read without a branch (wyhash's offsets) | 371.2, 1.28 | 1452 M, 1.48 M | |
| …and no second multiply after the byte hash | 370.0, 1.28 | 1441 M, 1.35 M | |

`k-nucleotide` is the second program because its keys are text of one length
per table, where the old branches were always predicted; the copy the
measurement used maps `dna.fasta` instead of reading standard input, since
valgrind does not run the original to its end. Its keys are text of a length
the compiler does not know, the same shape as 1BRC's names, so one function
serves both and both got better. The two tuned rows were tried for 1BRC
alone: reading every key of 4 to 16 bytes without a branch takes 0.30
mispredictions a row off, and adds 27 instructions and a fifth to
`k-nucleotide`, so they are not in.

## The fold's step in place (0.0.432)

The step `fn(acc, m) { acc.record(m) }` was lowered as `|mut acc, m| {
acc.record(m); acc }`: the accumulator, a table, handed into the step and back
for every row, a copy each way that the optimiser does not remove once
`record` takes it by `&mut`. `winnow-grammar` 761bc1a runs a step written
`|acc: &mut _, m| …` with a fold that never moves it (its ADR 24 §11), and the
emitter now writes that form. Callgrind, 1 M rows, output the same:
344.1 → **337.1** instructions a row, mispredictions 1.58 either way. What is
left of the fold's loop per row is 8 instructions; the checkpoint
winnow-grammar#22 suspected costs nothing, held in a register until a failure
needs it.

## Fewer cuts of the text (0.0.433)

Every cut of the input as `&str` is checked for a character boundary, and on
1BRC those checks were about 34 instructions a row. `winnow-grammar` 3ca83d3
(its ADR 24 §12) makes fewer cuts and lets LLVM see the rest hold, without
`unsafe`. Callgrind, 1 M rows, each step against the last, output the same and
mispredictions 1.58 throughout:

| step | instructions a row |
|---|---:|
| before (0.0.432) | 337.1 |
| `"."` in `TENTHS`, bound to nothing, matched and not sliced | 335.1 |
| `until(";" \| frame_end)` takes its hit as the end, without two cuts to look back for a `\r` | 326.1 |
| the cut at the hit follows a compare of its byte, and LLVM drops the boundary check | **319.1** |

Measured and not kept: the same compare after `TENTHS`'s indexed run (no
change), and `dec[i32](digit{1,2})` matched inside the run (385.9, 2.07
mispredictions). What is left of the checks is about 15 instructions a row.
`nikaia lower ty.nika`, the same main against the previous `winnow-grammar`,
three runs each: the same Rust, and the same instructions in every function
(384.7-384.9 M in all). Its mispredictions read 3.18 → 3.23 M, but they move
between functions that run the same instructions as before and have nothing to
do with a scan (`parse_catch_expr_inner` +8 k, `String::clone` −9 k, `memcmp`
−13 k): callgrind's predictor is a table indexed by code address, and a change
to the crate moves the compiler's code. **A misprediction count that moves
where the instructions do not is layout, not work**, and is not read as an
effect here.

## What is left to build

Against `tuned`, as of 0.0.433: 319 against 313 instructions a row, 1.58
against 3.61 mispredictions, taken as the tables above say. In the
order they are to be done; the figures are what callgrind attributes today,
not what a change has been measured to save.

| | what | today | where | issue |
|---|---|---|---|---|
| 1 | updating the stats: the `and_modify` / `or_insert_with` chain, beside overflow checks that are the language's | 23 against 11 | the lowering | [#420](https://github.com/Nikaia-Language/Nikaia/issues/420) |
| 2 | ADR-314, decided and not built: what a grammar matched and what a callee promises prove `TENTHS`'s overflow checks away at `aggressive` | a few instructions a row | the prover | [#421](https://github.com/Nikaia-Language/Nikaia/issues/421) |
| 3 | last: a grammar over bytes, with only what becomes text checked as UTF-8 (ADR-016 §4) | ~10 instructions a row | language, `winnow-grammar`, `std` | [#422](https://github.com/Nikaia-Language/Nikaia/issues/422) |

Not on the list, because measured or reasoned out of it: the hash of a name, done in 0.0.430 (#419); the fold's loop, done in 0.0.432 (winnow-grammar#22); boundary checks on cuts, done in 0.0.433 (winnow-grammar#23); finding the `;` and the
line's end, where one `memchr2` a line already beats `tuned`'s search and walk
back (ADR 24 §9); accumulating `dec`'s value inside the fixed-width match,
which came out even with `dec` as it is (ADR 24 §8b); and the mispredictions
the data itself makes - a sign or none, one whole digit or two, a name's
length - which `tuned` pays as well.

## What this does not claim

Nothing about the 1BRC leaderboard. The entries there are not parsers: they are
SWAR over raw bytes, hash tables sized to the key count with the keys stored
inline, and no UTF-8 anywhere. `tuned` is a single-threaded, readable version of
that idea and stops well short of it. The question this bench answers is the one
the example asks — whether a declarative grammar costs you the loop — and the
answer on this workload is that it costs less than the loop most people write.

## The files

* `src/bin/handwritten.rs` — both halves and the input generator. The binary is
  `handwritten` rather than `brc`, because the example is built as a project
  called `brc` and two binaries of that name in one comparison is one too many.
* `brc.sh` — builds all three, checks they agree, times them, prints the
  machine.

The two hand-written halves are the originals from the measurements behind
[ADR-015](../../docs/specification/adr/adr-015.md) and
[ADR-016](../../docs/specification/adr/adr-016.md), which had lived outside the
repository until now; the numbers in those ADRs were taken on a different box
and against a different backend revision, and do not match the table above.
