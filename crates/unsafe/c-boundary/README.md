# c-boundary

What C hands a library's entry point, and what the entry point hands back
([ADR-284](../../../docs/specification/adr/adr-284.md) D5-D7). A C caller
passes a run of values as an address and a length, a place for one result as
an out-parameter, room for a text or bytes result as a buffer it owns:
`out`, `cap`, `written`, and a value it holds as a handle, whose lock each
call takes (D11). Each is a raw pointer, and reading or writing one is
`unsafe`. This crate is the one place that does it, so the wrapper a Nikaia
library's entry point is lowered to writes nothing but calls into here.
No dependencies but `std`.

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
| `read` (an `unsafe fn`) | a read through `at` | the caller's contract: `at` is null, refused, or one initialised `T` nothing writes during the call - a struct C keeps and lends to a method as `self`. |
| `put` (an `unsafe fn`) | a write through `out` | the caller's contract: `out` is null or a place for one `T`. Null writes nothing. |
| `shared`, `exclusive` (`unsafe fn`s) | `&*at` | the caller's contract: `at` is null, refused as `E_ARGUMENT`, or a handle `handle` made with `Box::into_raw` and `free` has not freed. The value is reached only through its `RwLock`, held for the call; a thread that holds it already is `E_REENTRANT` rather than a deadlock. |
| `free` (an `unsafe fn`) | `Box::from_raw` | the caller's contract: a handle `handle` made and nobody uses any more. Null frees nothing; a handle this thread holds is `E_REENTRANT` and stays. |
| `Sent` (`unsafe impl Send`), `sent` (an `unsafe fn`) | moves raw addresses to the library thread that runs an `_async` call | `sent`'s contract, which is the C caller's: what the addresses point at stays alive and untouched until `done` is called. The one thread that uses them is that call's. |
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
