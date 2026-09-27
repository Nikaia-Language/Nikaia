# Getting Started

From nothing to a running Nikaia program, and what to try after that.

Nikaia is **pre-alpha**. Expect small programs to work and larger ones to hit a missing
piece; the [roadmap](../docs/project_status_and_roadmap.md#where-the-project-actually-stands)
lists exactly which.

## What you need

Linux on x86_64 or on 64-bit ARM (aarch64). CI runs the whole suite on both. macOS and
Windows are untested.

Nikaia compiles to Rust and lets the Rust compiler build the binary, so the one thing to
install is **a stable Rust toolchain, 1.88 or newer**. Nothing else: no nightly, no
extra system libraries.

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

`rustup` reads the channel from `rust-toolchain.toml` and installs it on first use.

## 1. Build the compiler

```sh
git clone https://github.com/Nikaia-Language/Nikaia.git
cd Nikaia
cargo build --release -p nikaia        # about a minute; the binary is target/release/nikaia
export PATH="$PWD/target/release:$PATH"
```

The compiler finds its standard library in the checkout it was built from, so **leave
the checkout where it is**. If you move it, point `NIKAIA_SYSROOT` at its `crates/`
directory.

## 2. Create a project

A project is a `nikaia.toml` and a `src/main.nika`. There is no `nikaia new` yet, so
create the two files by hand:

```text
hello/
├── nikaia.toml
└── src/
    └── main.nika
```

```toml
# nikaia.toml
[package]
name = "hello"
version = "0.1.0"

[build]
user-parallelism = "no"    # the default; "yes" for the multi-threaded runtime
```

```nika
// src/main.nika
fn main() {
    println("Hello, Nikaia!")
}
```

## 3. Run it

```sh
cd hello
nikaia run              # or: nikaia build
```

The first build compiles the standard library and its dependencies (about 15 s) and
caches them in `~/.cache/nikaia`. Later builds reuse that. Arguments for your program go
after `--`: `nikaia run -- a b c`.

The build writes two files beside the manifest:

* `nikaia.contracts` records what the compiler inferred about your functions: what they
  borrow, whether they can fail, whether they can pause. **Commit it.** A change in it is
  a change in your program's behaviour you can review.
* `nikaia.lock` pins your dependencies.

## 4. When the compiler says no

Most mistakes are refused in Nikaia's own words, with a code and a line that says what to
write instead:

```text
error[NK1139]: `x` is changed, and a `let` that is changed says `mut`
  --> src/main.nika:2:5
   2 |     let x = 5
           ^
     = this assigns to it - a binding changes only where it says so (Part I, 2.1), and this one does not
     help: write `let mut x`
```

Some errors and warnings still come through from the Rust compiler in Rust's vocabulary.
They point at your `.nika` line, but the wording is Rust's. Reporting one of those is
useful: it is a message Nikaia should have written itself.

---

## Two switches, one language

There is one language, and two lines in `nikaia.toml` decide how it is **built**, never
what it **means**. You do not rewrite code to move between them.

### `target`: which machine

`x86_64-linux` by default, `aarch64-linux` too. The machine decides what the standard
library can offer and what a crash does: an orderly unwind where the machine unwinds, a
trap where it traps. `wasm32-unknown` is named and **refused**: the compiler says what is
missing rather than emitting code for a different machine.

### `user_parallelism`: how much of *your* code runs at once

* **`no`** (the default): your code runs on **one** thread, over an event loop. **Data
  races are impossible**, because two pieces of your code are never in flight together.
  This is the setting for web services, CLI tools and edge workers, where you would reach
  for Node.js or Go.
* **`yes`**: a multi-threaded work-stealing runtime keeps every core busy, and thread
  safety is proven at compile time. This is the setting for heavy computation, where you
  would reach for Rust or C++.

**This is not the single thread you know from Python.** There the single thread is the
whole machine, and the other cores stand idle. Here it bounds **your instructions** and
nothing else. Waiting for I/O happens on the runtime's own threads, and what the standard
library does uses the whole machine whatever this switch says: a file's text is checked
in chunks across every core, **63.7 ms down to 16.5 ms**
([ADR-016](../docs/specification/adr/adr-016.md)). Your code stays a straight line, and
the machine does not stand still.

It is a permission, not a count. *How many* threads serve `yes` is for the runtime to
decide, because the right answer depends on the machine and not on the source file.

Because the difference lives in the compiler rather than in your source, a library built
at `no` is still checked against the rules parallel code needs. You cannot accidentally
ship something that only works single-threaded.
→ [ADR-037](../docs/specification/adr/adr-037.md)

---

## Programs to try

Real programs live in [`examples/`](../examples/). Copy one into `src/main.nika` and run it:

* [`calc.nika`](../examples/calc.nika): a four-function calculator built from a grammar.
  `nikaia run -- "2 + 3 * 4"`
* [`tally.nika`](../examples/tally.nika): counts the lines piped into it, in constant
  memory.
* [`access-log.nika`](../examples/access-log.nika): a web access log summarised.
* [`json.nika`](../examples/json.nika), [`config.nika`](../examples/config.nika): a JSON
  document and an INI file with comments, parsed.
* [`1brc.nika`](../examples/1brc.nika): the One Billion Row Challenge.
* [`hello-http/`](../examples/hello-http/): a project with a dependency, serving HTTP.

There are also two Computer Language Benchmarks Game programs, an HTML table that cannot
be made to leak markup, a program split across three files, and a C library called
through `extern "C"`. **All of them except `fortunes` compile, run, and are checked by
`cargo test`**. The single-file ones are run at both `user_parallelism` settings and must
print the same output. `fortunes` (the TechEmpower benchmark) is still written ahead of
the compiler, because it needs a database.

[`examples/README.md`](../examples/README.md) lists which gaps in the language each
example exposed, and which are still open.

## Where next

* [A tour of the syntax](syntax.md): the language on one page, for readers coming from
  other languages.
* [Specification, Part I](../docs/specification/10-nikaia-light.md): the full rules.
* [Specification, Part II](../docs/specification/20-nikaia-advance.md): concurrency,
  parallelism and grammars.
* Working on the compiler itself: [toolchain architecture](../docs/toolchain_architecture.md),
  and `cargo test -p nikaia` (not the whole workspace, see the
  [roadmap](../docs/project_status_and_roadmap.md#the-walls-you-will-hit)).
