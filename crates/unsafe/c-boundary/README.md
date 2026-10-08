# c-boundary

What C hands a library's entry point, and what the entry point hands back
([ADR-284](../../../docs/specification/adr/adr-284.md) D5-D7). A C caller
passes a run of values as an address and a length, a place for one result as
an out-parameter, and room for a text or bytes result as a buffer it owns:
`out`, `cap`, `written`. Each is a raw pointer, and reading or writing one is
`unsafe`. This crate is the one place that does it, so the wrapper a Nikaia
library's entry point is lowered to writes nothing but calls into here.
No dependencies.

```rust
let numbers = [1_i64, 2, 3];
let run = unsafe { c_boundary::run(numbers.as_ptr(), numbers.len()) };
assert_eq!(run, Some(&numbers[..]));

let mut room = [0_u8; 8];
let mut written = 0;
let status = unsafe { c_boundary::hand_back(b"hello", room.as_mut_ptr(), room.len(), &mut written) };
assert_eq!((status, written, &room[..5]), (c_boundary::OK, 5, &b"hello"[..]));
```

## Every `unsafe`, and why it is sound

| where | what | why it holds |
| :--- | :--- | :--- |
| `run` (an `unsafe fn`) | `slice::from_raw_parts` | the caller's contract, which is the C caller's: `at` points at `len` values it keeps alive for the call. A length of `0` reads nothing, whatever `at` is; a null address with a length is refused rather than read. |
| `text` (an `unsafe fn`) | `run` | `run`'s contract; what is not UTF-8 is refused, not read as text |
| `put` (an `unsafe fn`) | a write through `out` | the caller's contract: `out` is null or a place for one `T`. Null writes nothing. |
| `hand_back` (an `unsafe fn`) | writes through `written`, copies into `out` | the caller's contract: `written` is null or a place for one `usize`, and `out` is null or `cap` bytes of room. At most `cap` bytes are copied, and nothing is where the bytes do not fit. |

## How it is checked

Its own workspace, so its lock file and its checks are its own:

```sh
cargo test && cargo test --all-features
cargo clippy --all-features --all-targets -- -D warnings
cargo +nightly miri test --all-features                       # Stacked Borrows
MIRIFLAGS=-Zmiri-tree-borrows cargo +nightly miri test --all-features
```

`scripts/check-unsafe-crates.sh` runs these for every crate under
`crates/unsafe/`, and CI runs that script.
