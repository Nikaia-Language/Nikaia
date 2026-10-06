# What a memory block per task costs

[ADR-327](../../docs/specification/adr/adr-327.md) D4-D8 has a program allocate
from a block per task, with a full block ending the task. This counts what that
costs against the system allocator, on many short tasks that each read a
request and write a response, as a server's do (#490, step 2).

Three binaries, one file each, the same workload (`src/workload.rs`); each
differs from the others only in its `#[global_allocator]`:

| binary | allocator |
|---|---|
| `global` | the system allocator (`malloc`) |
| `budget` | the system allocator, with every task's bytes counted against a budget of 64 KiB (D5's check without a block) |
| `block` | a block of 64 KiB per task: an allocation moves a pointer, nothing is given back one by one, the block is empty again when the task ends; a full block ends the task |

```sh
benches/taskblock/taskblock.sh          # 100 000 tasks
benches/taskblock/taskblock.sh 20000    # quicker
```

The script builds with `--release` (`opt-level = 3`, no LTO), checks that all
three print the same bytes, counts each with callgrind
(`--branch-sim=yes`), and subtracts a run of no task, so what is left is
the tasks' own.

## The table

100 000 tasks; a task makes 22 allocations and 7 reallocations.

| allocator | instructions / task | mispredicted / task |
|---|---:|---:|
| `global` | 17410 | 180.3 |
| `budget` | 17810 | 197.5 |
| `block` | 10188 | 94.8 |

- **A block is cheaper than `malloc`, not dearer**: 41% fewer instructions and
  half the mispredicted branches. What it saves is `malloc`'s and `free`'s own
  work (about 7 200 instructions a task, `callgrind_annotate`); taking from the
  block and emptying it costs a few instructions per allocation.
- **Counting a budget over `malloc` costs 2.3%** (400 instructions a task):
  the thread-local count on every allocation, reallocation and free.

## Reading it

- **`global` declares the system allocator as the global one**, as the other
  two declare theirs. Without a declaration every allocation goes through
  `std`'s default shims (`__rdl_alloc`, `__rdl_realloc`), which are not inlined
  and cost about 420 instructions a task more - a difference of the
  declaration, not of a strategy.
- **`malloc`'s count depends on where the heap starts**: the same binary called
  by another path (`./target/release/global` against its absolute path) moves
  `_int_malloc` and `_int_free` by about 177 instructions a task, because the
  argument vector shifts the heap. The script always calls the same way.
- **What is not measured here**: a block that is given back to the system when
  a task ends (ADR-327 §6), a task that holds more than one block, values that
  cross from a task into a shared value, and fallible growth of the
  collections themselves (#490 step 1).
