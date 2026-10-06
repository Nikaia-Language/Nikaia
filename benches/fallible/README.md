# What fallible collections cost

[ADR-327](../../docs/specification/adr/adr-327.md) D4, D5 has a task's lists,
texts and maps take their memory from a budget, and a full budget ends the task
instead of aborting the process. That needs collections whose growth can
fail: `Vec<T, A>` from `allocator-api2` and `hashbrown`'s map with the same
allocator. This counts what they cost against today's `Vec`, `String` and
`std`'s `HashMap` (#490, step 1).

Two workloads, each written twice, the same steps on both sides:

| binary | what |
|---|---|
| `brc_std`, `brc_fallible` | 1BRC's aggregation: a map from station to min/max/sum/count, then the sorted names written out. The map's table, the list of names and the output text are the collections; the map uses Nikaia's trusted hash on both sides |
| `tasks_std`, `tasks_fallible` | `benches/taskblock`'s workload: many short tasks, each building a request and parsing it into owned parts, then writing a response (22 allocations and 7 reallocations a task) |

The fallible side's allocator (`src/budget.rs`) counts every byte against a
budget and refuses past it; growth goes through `try_reserve`, and a refusal
ends the task. The budget is set so that nothing is refused: what is counted is
the check and the collections, not a refusal.

```sh
benches/fallible/fallible.sh                # 200 000 rows, 100 000 tasks
benches/fallible/fallible.sh 50000 20000    # quicker
```

The script builds with `--release` (`opt-level = 3`, no LTO), checks that both
sides of each workload print the same bytes, counts each with callgrind
(`--branch-sim=yes`) and subtracts a run over nothing, so what is left is per
row or per task.

## The table

| program | instructions | mispredicted |
|---|---:|---:|
| `brc_std` (per row) | 393.8 | 3.51 |
| `brc_fallible` (per row) | 394.4 | 3.67 |
| `tasks_std` (per task) | 17872.4 | 181.12 |
| `tasks_fallible` (per task) | 17882.3 | 193.21 |

- **Instructions are the same within 0.2%** on both workloads.
- **Mispredicted branches rise 5% (brc) and 7% (tasks)**: the budget check on
  every allocation and the `try_reserve` before each growth.

## Reading it

- **`allocator-api2`'s `extend_from_slice` copies one element at a time** on
  stable Rust (`extend(iter().cloned())`, no specialisation). Used as it is,
  it doubled the tasks' count. `budget::extend` copies with one
  `copy_nonoverlapping`, as `std`'s `Vec<u8>` does; a `std` built on the crate
  would have to do the same.
- **The fallible text is a `List<u8>`**, handed out as a `str` without checking
  its UTF-8 again, as `String` does: every byte in it came from a `str`.
  Checked, it cost about 230 instructions a task more.
- **What is not measured here**: the collections behind a feature of
  `nikaia-std` and a program the compiler emits against them (the emitter
  writes `Vec` and `String` today), a refusal and the task it ends, and a
  budget that is a block rather than a count (`benches/taskblock`).
