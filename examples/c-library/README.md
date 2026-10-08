# c-library

A Nikaia package **C calls** ([ADR-284](../../docs/specification/adr/adr-284.md)):
`artifact = "c-library"` in `nikaia.toml` makes a shared and a static library
and `wordtally.h`, and every `pub extern fn` is an entry point.

```text
nikaia build
cc -I target/nikaia/c-library use.c -L target/nikaia/c-library -lwordtally -o use
LD_LIBRARY_PATH=target/nikaia/c-library ./use
```

```text
9 words, the longest "quick" (status 0)
the first two words of "Hello NIKAIA from c":
  Hello (mixed)
  NIKAIA (upper)
stopped after 2
```

What crosses, in `src/main.nika` and `use.c`:

* `Tally` is a `pub struct`, so C holds it as a **handle**:
  `wordtally_Tally_new`, `wordtally_Tally_add`, a getter per `pub` field
  (`wordtally_Tally_words`, `wordtally_Tally_longest`) and
  `wordtally_Tally_free`. Each call holds the handle's lock.
* Text goes in as an address and a length. It comes back into the **caller's
  buffer** (`out`, `cap`, `written`).
* `Case` is an `enum` without payload, so it is a C `enum`.
* `each_word` takes a **callback**: a function pointer and a `void *ctx`. The
  callback's `bool` is the caller's stop.
* Every function returns a status, and its value comes back through the last
  parameter.

`crates/nikaia/tests/c_library.rs` builds it in place under `--locked`,
compiles `use.c` against it, and checks that output.
