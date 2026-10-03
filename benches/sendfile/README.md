# `sendfile` — what it costs to answer a request with a file

[ADR-289](../../docs/specification/adr/adr-289.md)'s evidence.
[`docs/history/zero-copy-send.md`](../../docs/history/zero-copy-send.md) is the write-up: the
method, the numbers, what they settled and the two confounds they survive.

```sh
./benches/sendfile/sendfile.sh [repeats]     # default 5
```

Five vehicles serve the same file over one kept-open TCP connection with a
thread draining the far end: `read_each` (open, read, validate, write, per
request — what the README's example did), `cached` (read once, write the
buffer), `mapped` (`mmap` once, write the mapping — what `fs::map` gives),
`sendfile` (`sendfile(2)`; the bytes never enter the process) and `splice`
(`io_uring` `Splice` through a pipe, the two hops linked).

**Read the `machine` column.** The sending thread's own CPU is not the cost:
`io_uring` performs a splice on kernel workers that no per-thread accounting
sees, and reading the thread column instead would report the losing mechanism
as the winner by a factor of twenty. `machine` is every non-idle jiffy on the
box, from `/proc/stat`; run it on a quiet machine and check the load average
the binary prints on either side.

Linux only in its interesting half: `sendfile(2)` and `mmap` are `libc`'s here
and the ring is target-gated exactly as `nikaia-std` gates it.
