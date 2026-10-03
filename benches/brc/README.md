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

## The table

8 million rows (123 MB), 413 stations, best of 5, on a 4-core box. The script
prints the machine with the table and so should any quotation of it:
`docs/history/runtime-cost.md` §6.3 has absolutes on this kind of box moving by
1.4–1.9× from one day to the next, and **only the ratios travel**.

| | cores | best | against `tuned` |
|---|---:|---:|---:|
| Rust, `naive` | 1 | 0.75 s | 3.1× |
| **Nikaia, `user-parallelism = "no"`** | 1 | **0.46 s** | **1.9×** |
| Rust, `tuned` | 1 | 0.24 s | 1.0 |
| Nikaia, `user-parallelism = "yes"` | 4 | 0.29 s | 1.2× |

Four runs, one output. That the two Nikaia rows agree is
[ADR-009](../../docs/specification/adr/adr-009.md) D2's whole claim — the chunk
count does not change the result — checked rather than asserted, and the script
fails if it ever stops being true.

### What the rows say

**Against the loop you would actually write, the grammar wins: 0.46 s against
0.75 s, one core each.** That is the row the example's claim turns on, and it is
why both halves are here: measured against `tuned` alone the grammar looks
1.9× slow, and measured against `naive` alone it looks 1.6× fast. Neither number
means anything without the other.

**The remaining 1.9× is not all parser overhead.** `tuned` never validates
UTF-8, and `std::fs::map` must — the `&str` views a grammar cuts out of a
mapping depend on it
([ADR-016](../../docs/specification/adr/adr-016.md) D1, where that check is
measured and divided across cores rather than skipped). `tuned` also knows the
temperature is four characters or three and indexes it directly, where the
grammar states `"-"? digit{1,2} "." digit` and a general repetition walks it.
The first is a guarantee the compiler cannot drop; the second is a bound it
could in principle use and does not.

**The parallel row is not a 4× speed-up over the sequential one** (0.46 → 0.29,
about 1.6×). The input is read once either way and the UTF-8 check is already
spread across cores at both settings, so what the four cores divide is the parse
and the fold, not the whole program.

## A lowercase `par_fold` rule, and what its whitespace cost (0.0.388)

`examples/1brc.nika` writes its entry rule `pub rule file` - lowercase, so
syntactic. Its entry point skipped no whitespace (ADR-009 D2), but the rule
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
