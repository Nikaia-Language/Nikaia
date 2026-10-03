# What an overlap costs

ADR-292 lets two statements that touch nothing in common run at the same time.
This measures what that is worth, and it is the evidence §8.2 and §8.4 rest on.

Four binaries over the same two reads — the `TWO_READS` program in
`crates/nikaia/tests/ordering.rs`, reduced to plain Rust so there is nothing
between the measurement and the mechanism:

| binary | what it is |
|---|---|
| `strict` | the sequential lowering — the baseline |
| `scope` | the overlap's first lowering, `std::thread::scope` |
| `join` | what the emitter writes today, `nikaia_std::task::both` = `rayon::join` |
| `fixed` | both vehicles with no work in them at all — the floor |

Run it:

```sh
benches/overlap/run.sh            # the table, over seven file sizes
cargo run -p overlap-bench --release --bin fixed -- 20000
```

Keep these in step with the emitter by hand. They exist to be read next to the
generated Rust, not to be generated from it.

## …and what it costs on the runtime instead

[ADR-038](../../docs/specification/adr/adr-038.md) D4 starts the runtime before
`main`, and [ADR-292](../../docs/specification/adr/adr-292.md) predicted
that a pair of operations on an I/O thread that is *already running* carries no
per-pair wake-up at all. `runtime` is that measurement, and unlike the four
above it goes through the **real** `nikaia_std` rather than a reduction of it:

| binary | what it is |
|---|---|
| `runtime` | `seq`, `join` and `fs::read_both` over the same two files, on the pre-started runtime |

```sh
benches/overlap/runtime.sh            # the table, both of D3's mechanisms
cargo run -p overlap-bench --release --bin runtime -- 20000 9
```

It measures **both** mechanisms — `io-method = "auto"` and `io-method =
"blocking"` pinned — because the answer differs between them, and that
difference is the finding. The numbers, the method and the confound are in
[`docs/history/runtime-cost.md`](../../docs/history/runtime-cost.md).

## …and what the *lowering* costs, which is not the same question

Everything above measures `nikaia_std` from Rust.
[ADR-292](../../docs/specification/adr/adr-292.md) D6 is a **lowering**: at
`user_parallelism = no` a pair of `std` file reads is emitted as
`nikaia_std::task::read_pair`. `lowered.sh` measures that, through the whole
pipeline and not a reduction of it:

| what | how |
|---|---|
| `pair.nika` | two reads in a loop, built **twice from one source** — `ordering = "effects"` against `ordering = "strict"` |

```sh
benches/overlap/lowered.sh                          # both mechanisms, 200 000 pairs, 9 repeats
benches/overlap/lowered.sh /tmp/somewhere 20000 3   # …quicker, in a named directory
```

It greps the emitted Rust and prints which lowering each binary got, because two
identical binaries would measure a very stable nothing. Both mechanisms again,
and here the fallback's column is the *decision* rather than the finding: D10
performs the pair in written order where overlapping is not free, so that column
should be zero and not +38 µs. The numbers are `docs/history/runtime-cost.md` §6.
