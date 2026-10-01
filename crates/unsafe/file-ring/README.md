# file-ring

Files read and written by the kernel through `io_uring`, with **owned**
buffers, and an `eventfd` bell (`Bell`) that wakes a thread parked on the
ring without taking its lock. Linux only. Depends on `io-uring` and `libc`.

## The soundness rule for the buffers

A buffer handed to the kernel must stay alive and unmoved until the completion
for it arrives. **A slot, and the buffer in it, is freed only by the code that
has reaped its completion.** Three things make that hold, not just hope for
it; the crate documentation lists them:

* the buffers live in the ring, not on a caller's stack;
* a slot is released on a count of outstanding submissions, not on a return;
* the next operation reconciles before it submits.

## Every `unsafe`, and why it is sound

| where | what | why it holds |
| :--- | :--- | :--- |
| `Ring::open` | `libc::eventfd` | takes a count and flags, borrows nothing, returns a descriptor or `-1`, which is checked |
| `Ring::open` | `OwnedFd::from_raw_fd` | a descriptor `eventfd` just made, which nothing else holds |
| `arm_bell` | submission of a poll | a poll borrows no buffer; the bell's descriptor is owned by the ring and outlives the poll |
| `submit` (read/write) | building and pushing the entry | the buffer lives in the slot and is released only after its completion is reaped (the rule above); each submission covers a disjoint range of exactly one slot's buffer |
| `reap` (read) | `Vec::set_len` after a completion | a read's buffer is offered to the kernel from its length to its capacity, unzeroed; the completion says how many bytes the kernel wrote there, and the length grows by exactly that many, so every byte below it is initialised |

Ringing and draining the bell are ordinary `std` reads and writes on a
`File`, and take no `unsafe`.

## How it is checked

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

The tests run on the real kernel, and a machine without `io_uring` skips the
ones that need a ring. **Miri cannot run them**, since it has no `io_uring`;
the buffer rule is upheld by the slot bookkeeping above, which the tests
exercise.
