# checked-text

Bytes checked as UTF-8 once and handed out as text from then on, without
checking them again. That is the point: a 13 GB mapping is checked once,
not once per view. Optional features:

* `rayon`: a large input is checked in chunks across the rayon pool, cut
  only where a character begins, and reports the same first bad byte a serial
  check would.
* `map`: a file mapped and checked in one step.
* `simd`: each piece is checked with `simdutf8`, which gives the standard
  library's verdict and the same offset for a bad byte; a test holds it to
  `std::str::from_utf8` across the vector check's block edges.

No dependencies without features.

```rust
use checked_text::CheckedText;

let text = CheckedText::check(b"Oslo;3".to_vec()).expect("text");
assert_eq!(&*text, "Oslo;3");
assert_eq!(CheckedText::check(b"ok\xFFno".to_vec()).err(), Some(2));
```

## Every `unsafe`, and why it is sound

| where | what | why it holds |
| :--- | :--- | :--- |
| `as_str` (so `Deref`) | `str::from_utf8_unchecked` | `check` validated the bytes before the value existed, and they are reached only through `&self`: nothing hands out a way to change them |
| `into_string` | `String::from_utf8_unchecked` | validated in `check`, owned here since |
| `map` (feature `map`) | `memmap2::Mmap::map` | a private, read-only mapping. **One caveat is accepted, not ruled out**: another process that truncates or rewrites the file while it is mapped changes what this value reads. That is true of every memory map, and it is why a program that maps a file must own it for the duration. |

The chunked check's correctness is part of the argument. A cut walks forward
off at most three continuation bytes, so every chunk begins where a character
begins, and checking the chunks separately is the same check as checking the
whole. Tests compare it with a serial check at every cut.

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

Miri runs the small tests. The megabyte inputs and the mapping are skipped
there: Miri cannot map a file and would take hours on megabytes.
