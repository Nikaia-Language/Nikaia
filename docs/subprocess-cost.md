# What the `rustc` subprocess costs — and what skipping it would have bought

**Date:** September 11, 2026
**Status:** measured, and kept. The record then said the in-memory exit — feeding
a `rustc_ast::Crate` to `rustc_interface::run_compiler` instead of printing it —
"can be added when the subprocess cost is worth removing". This file put the
number on it. That exit, and the whole `rustc_ast` path it belonged to, has since
been **withdrawn** (§6): the numbers below are unrevised, because they are part of
why. `../CHANGELOG.md` and [`withdrawn-one-way-down.md`](history/withdrawn-one-way-down.md)
carry the account.
**Related:** [ADR-004](specification/adr/adr-004.md) D1 (the one lowering, which
emits text, and whose price §4 is), [ADR-003](specification/adr/adr-003.md) D1
(the text interface), [ADR-300](specification/adr/adr-300.md) (diagnostics read
the generated file)

Nobody had ever put a number on what invoking `rustc` as a child process costs.
This is the number, the method, the machine, and the three things the measurement
found that are not timings.

**The conclusion, first:** the subprocess is not worth removing, and the largest part of what it
costs is not the subprocess.

---

## 1. The machine and the method

Intel Xeon @ 2.10 GHz, 4 vCPU, 15 GB RAM, Linux 6.18.44 x86_64 — a **shared virtual machine**
with no `cpufreq` governor exposed. `rustc 1.94.0-nightly (8d670b93d 2025-12-31)`, which is the
`nightly-2026-01-01` this repository named when the block was run.

`docs/history/staging-candidates.md` §2 sets this repository's standard: *wall-clock on a shared machine
is not a measurement; an instruction count is deterministic and diffable*. So the decision rests
on §4's callgrind counts, and the wall-clock in §3 is there to say what those counts feel like.
Where wall-clock is quoted it is a **median of 25 runs after 3 warm-ups**, with the median
absolute deviation beside it, and every figure is warm — the file is in page cache and the
toolchain's dylibs are resident, which is the case that flatters the subprocess least.

**The corpus** is every `.nika` file in `examples/` and `tests/samples/`, lowered with
`nikaia --backend rust --no-cache` and then compiled with `rustc`: 14 programs from 148 bytes of
generated Rust (`hello_world`) to 5 910 (`json`). `examples/fortunes.nika` is excluded because the
bootstrap compiler cannot lower its `dsl postgres` block. Compiling the *generated* Rust is the
right subject: the subprocess wraps `rustc` on printed text, and what produced the text does not
change what `rustc` pays to read it.

**The decomposition.** Feeding the `rustc_ast::Crate` to `rustc_interface::run_compiler` instead
of printing it removes exactly four things, and nothing else:

| removed | how it was measured |
| :--- | :--- |
| the `rustc` process — `exec`, the dynamic loader, teardown | wall minus rustc's own `-Ztime-passes` `total`, **in the same process**. `total` is started at the top of `rustc_driver_impl::main`, so everything before and after it is the process. |
| `rustc` re-lexing and re-parsing the text | `parse_crate`, from `-Ztime-passes-format=json` |
| `pprust` printing the crate to text | `-Zunpretty=normal` (parse, print, stop) on the file, minus the same on an empty one, minus the parse difference |
| writing the file | 200 writes of the largest generated crate |

Everything downstream — expansion, resolution, type checking, borrow checking, monomorphisation,
LLVM, and the linker, which is its own subprocess either way — is identical on both paths and is
the denominator.

---

## 2. Three false starts, because each one would have changed the answer

**`rustc` on `PATH` is not `rustc`.** The first floor taken was `rustc --version`, at 28.0 ms. That
is `rustup`: `/root/.cargo/bin/rustc` is a symlink to the shim, which starts a 40 MB binary, reads
`rust-toolchain.toml` and *then* `exec`s the real compiler. The real binary answers in 5.5 ms.
Measuring the subprocess through the shim would have credited **21 ms of rustup to rustc's process
start** — and, because the shim is the larger half of the cost, would have made the in-memory exit
look like the fix for a problem that a two-line change fixes (§5).

**`-Ztime-passes` rounds to milliseconds** in its default text format, so `parse_crate` printed as
`0.000` or `1.000` and the parse looked like either free or 0.24 % of a build. It reached the same
conclusion by luck. `-Ztime-passes-format=json` reports microseconds and is what §3 uses.

**Two medians are not a difference.** "Wall minus internal" was first computed from medians of two
different run sets, which put five of the fourteen programs at *negative* process cost. Timing the
wall of the very run that printed the `total` reduced the spread from ±40 ms to a median absolute
deviation of 0.2–0.8 ms.

---

## 3. Wall-clock: what the subprocess costs per invocation

Real `rustc` binary, debug (`opt-level=0`, which is what `rustc-executor` invokes and the case
where a fixed cost is the largest share). Milliseconds, median of 25.

| program | .rs bytes | rustc wall | rustc's `total` | the process (mad) | `parse_crate` | `pprust` |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: |
| `hello_world` | 148 | 50.6 | 43.7 | 7.3 (0.3) | 0.175 | 0.28 |
| `let_assignment` | 181 | 56.7 | 49.3 | 7.3 (0.5) | 0.210 | 0.23 |
| `while_loop` | 386 | 65.5 | 57.6 | 8.1 (0.2) | 0.267 | 0.23 |
| `tally` | 1 205 | 93.1 | 85.8 | 7.4 (0.2) | 0.315 | 0.08 |
| `escaping` | 1 941 | 83.4 | 75.6 | 7.8 (0.5) | 0.342 | 0.27 |
| `calc` | 2 299 | 271.5 | 263.3 | 9.0 (0.6) | 0.403 | 0.12 |
| `config` | 2 608 | 269.6 | 260.6 | 9.2 (0.4) | 0.421 | 0.38 |
| `report` | 3 405 | 242.1 | 233.1 | 9.0 (0.4) | 0.484 | 0.25 |
| `k-nucleotide` | 3 907 | 313.3 | 301.9 | 9.3 (0.8) | 0.538 | 1.54 |
| `inventory/main` | 4 010 | 288.8 | 280.5 | 8.5 (0.5) | 0.473 | 0.77 |
| `access-log` | 4 221 | 468.5 | 460.7 | 10.1 (0.6) | 0.505 | 1.45 |
| `1brc` | 4 328 | 384.2 | 373.0 | 9.5 (0.7) | 0.514 | 0.30 |
| `n-body` | 4 891 | 103.0 | 95.4 | 7.9 (0.4) | 0.494 | 0.18 |
| `json` | 5 910 | 386.7 | 377.3 | 9.5 (0.4) | 0.625 | 0.47 |

Read the columns with their error bars. **The process** is solid: 7.3 ms at 148 bytes rising to
10.1 ms at 4 221, with a median absolute deviation under 1 ms, and about 1.0 ms of it is the
harness's own `fork`/`exec` (measured against `/bin/true`) rather than rustc's. **`parse_crate`** is
solid too, and is 0.18–0.63 ms across a forty-fold range of input. **`pprust`** is a difference of
medians of a 2.6–4.6 ms quantity, so its error bars are the size of the value: all that column
says is *under 2 ms on every file*, which is all that has to be true. Writing the largest generated
crate to disk takes **0.073 ms**.

Release (`-O`) doubles to trebles the denominator and leaves every removable column where it is:
`json` goes from 386.7 ms to 1 079.7, `1brc` from 384.2 to 1 100.9.

So per invocation, everything the in-memory exit removes is **8–11 ms**, of which the parse is
under 0.7 and the print under 2. Against the 386.7 ms it takes to build the largest program in the
corpus without optimisation, that is **2.2 %**; against the 1 079.7 ms it takes with `-O`,
**0.8 %**.

### The rustup shim, measured the same way

`rustc-executor` calls `Command::new("rustc")`, which is the shim:

| program | the process, real binary | the process, via `rustup` | the shim costs |
| :--- | ---: | ---: | ---: |
| `hello_world` | 6.98 (mad 0.22) | 27.55 (mad 1.04) | **20.6** |
| `1brc` | 9.83 (mad 0.77) | 31.97 (mad 2.78) | **22.1** |

A cross-check that timed shim and real binary in separate run sets put it at 15.5 ms, so call it
**15–22 ms, best estimate 20**. Either way: **two thirds of what today's subprocess costs outside
rustc's own work is rustup, not the process.**

---

## 4. Instructions retired — the number the decision rests on

`valgrind --tool=callgrind`, one run each, deterministic:

| | Ir |
| :--- | ---: |
| `rustc --version` — `exec` and the dynamic loader, nothing compiled | 8 304 501 |
| `-Zunpretty=normal` on `fn main() {}` — everything up to the print, no program | 12 664 830 |
| `-Zunpretty=normal` on `hello_world.rs` (148 B) | 12 690 888 |
| `-Zunpretty=normal` on `json.rs` (5 910 B) | 15 436 962 |
| **full debug compile of `hello_world.rs`** | **34 001 700** |
| **full debug compile of `json.rs`** | **1 719 324 739** |

The printed-Rust round trip — lex, parse, and print — is the difference between the two
`-Zunpretty` rows:

| | round trip | full compile | share |
| :--- | ---: | ---: | ---: |
| `hello_world` | 26 058 | 34 001 700 | **0.077 %** |
| `json` | 2 772 132 | 1 719 324 739 | **0.161 %** |

Add the process, taking `rustc --version` as its cost — it is the right order of magnitude for
what it measures, the dynamic loader relocating `librustc_driver` and its dependencies, and it is
an over-estimate in that it also parses arguments and writes to stdout:

| | removable | full compile | share |
| :--- | ---: | ---: | ---: |
| `hello_world` | 8 330 559 | 34 001 700 | **24.5 %** |
| `json` | 11 076 633 | 1 719 324 739 | **0.64 %** |

**That pair is the finding.** The subprocess is a quarter of the cost of compiling a three-line
program and **0.64 % of the cost of compiling the largest program in the corpus**, and three
quarters of even that 0.64 % is starting a process rather than the round trip through text. The
round trip itself — the part an in-memory exit would remove, skipping rustc's parser — is
**0.161 %**.

### A project of many files

It does not get worse; it gets better. `modules::collect` walks from the entry and every `.nika`
file becomes a `mod` at the crate root of **one** Rust crate (Part I 9.1), so a program of any
number of files is still **one** `rustc` invocation. The 8.3 M Ir is paid once per program, not
once per file, and a project ten times this size pays the same 8.3 M against ten times the
codegen. The share falls.

The one thing that would invert this is named so it can be watched: if
[ADR-003](specification/adr/adr-003.md) D2's build graph ever splits a program into several Rust
crates, each crate becomes an invocation and the fixed cost multiplies by the crate count. **That
is the condition under which the fixed cost is worth re-measuring** — not a bigger program, a
differently shaped build.

---

## 5. What the measurement found that is not a timing

Three things, and each matters more than the numbers above.

**The path that holds the subprocess had never run.** `--backend bridge` was the **default**
backend when this was measured — the default changed to `rust` on the strength of this section's
"one of fifteen", and the bridge was withdrawn altogether not long after — and it panicked on
every input, at the first symbol the lowering interned:

```
thread 'main' panicked at scoped-tls-1.0.1/src/lib.rs:168:9:
cannot access a scoped thread local variable without calling `set` first
   1: <rustc_span::symbol::Symbol>::intern
   2: <rustc_span::symbol::Ident>::from_str
   3: rustc_executor::execute
```

`Ident::from_str` reads the interner out of `rustc_span`'s `SESSION_GLOBALS`, and `execute` never
installed one. It compiled, which is why it survived: `cargo check` cannot tell a lowering that
works from one that aborts before it prints a line. **This is why the cost had never been
measured — there was nothing to measure.** Repaired by wrapping the body in
`create_session_if_not_set_then`, five lines, and `crates/nikaia/tests/bridge_backend.rs` now
asserts the whole path: `.nika` in, a binary out, and the binary prints what the program says.

A smaller thing found on the way, reported here and since changed: `execute` invoked `rustc` with
no `--edition`, so the printed crate compiled at **2015**, while every other path in the workspace
writes Rust for 2021 (`crates/nikaia/tests/common/mod.rs` passed `--edition 2021`; 2024 since 0.0.201). Nothing
Bridge-IR can express tells the two apart today, which is why this file recorded it rather than
acting on it — a change to what is compiled wants its own commit and its own check. It has one now:
the interner's edition and `rustc`'s are a single constant in `crates/rustc-executor/src/lib.rs`,
and the change was verified inert rather than assumed inert. The printed crate and the compiled
binary are byte-identical before and after
(`sha256` `01455af3…` and `a474e629…` on `hello_world`), and
`crates/nikaia/tests/bridge_backend.rs` now compiles the same printed text at 2015 and at 2021 and
compares what the two binaries print — so the sentence "nothing Bridge-IR can express tells the two
apart" is an assertion rather than a claim, and stops being true loudly.

**The `rustc_ast::Crate` the bridge lowering built was print-only, so the in-memory exit did not
cost nothing to keep open.**
Every node it creates carries `NodeId::from_u32(0)` — which is `CRATE_NODE_ID` — and `DUMMY_SP`.
rustc's own parser writes `DUMMY_NODE_ID` (`NodeId::MAX`) instead, and macro expansion asserts it
before assigning a real one:

```rust
// rustc_expand::expand, assign_id!
debug_assert_eq!(*$id, ast::DUMMY_NODE_ID);
let new_id = $self.cx.resolver.next_node_id();
```

So handing this crate to `run_compiler` trips that assertion on a debug compiler, and on a release
one silently skips assignment and leaves every node holding the crate root's id. The spans are the
same story from the other end: [ADR-300](specification/adr/adr-300.md)'s contract is that an error
points back at the `.nika` file, and it is kept today by translating `rustc --error-format=json`
about the generated file — a file an in-memory compile does not produce and a stream it does not
emit. Opening the in-memory exit therefore costs a node-id discipline and a `SourceMap` in the
lowering, plus a second route for diagnostics. Not nothing, and not written down anywhere before
now.

**Bridge-IR can carry one of the fifteen corpus programs.** `bridge-ir` is 85 lines:
`Function`, `Struct`, `Let`, `Expr`, `Literal`, `Variable`, `Call`, and no control flow at all — no
`if`, no `while`, no `match`, no binary operators. Of the corpus, `hello_world.nika` reaches the
subprocess; `let_assignment.nika` is refused by name (`println` with a variable argument has
nowhere to go in `BridgeExpr`), and all eleven `examples/` lower through `--backend rust`, whose
output is a `grammar! { }` proc-macro invocation that nothing in Bridge-IR can express. Measured
end to end on the one program that does reach it:

| | ms | share |
| :--- | ---: | ---: |
| `nikaia --input hello_world.nika --backend bridge` | **68.96** | |
| the `rustc` subprocess inside it | 63.47 | 92 % |
| — of which the rustup shim | 15.46 | 22 % |
| — of which the process itself | ~7.0 | 10 % |
| — of which `parse_crate` | 0.18 | 0.3 % |
| the front end, the lowering, `pprust` and the file write together | 5.49 | 8 % |

Even here, where the fixed cost has almost nothing to hide behind, the in-memory exit is worth
**33 %** of the build and **two thirds of that 33 % is rustup**. Twenty-two of those percentage
points are reachable without the in-memory exit at all: naming a compiler instead of the shim, two
lines, no new `rustc_private` surface — `crates/nikaia/build.rs` already records the `rustc` that built the
compiler as `NIKAIA_RUSTC`, and `crates/nikaia/tests/common/mod.rs` already invokes it that way.

**Left undone on purpose, because it is a decision no record has made.** *Which* `rustc` the
executor invokes is not a performance question. The shim is what makes
[ADR-001](specification/adr/adr-001.md) D1's "one file is the single source of truth"
true at run time — it reads `rust-toolchain.toml` on every invocation — and the two answers come
apart for a user whose own project names something else. Choosing between "the toolchain resolved
where the build runs" and "the toolchain that built this compiler" is an ADR's job, not a
benchmark's, so it is recorded here and not acted on.

---

## 6. What was not built, and why that is the result

Nothing consuming `rustc_interface::run_compiler` was written. A feature that removes 0.64 % of
the cost of the largest program in the corpus, on a code path that can compile one program in that
corpus, whose AST would not reach codegen without a node-id and span discipline nobody has built,
is a feature nobody needed.

The record kept the exit open on the strength of "the lowering builds the AST anyway", and the
paragraph above is what took that away: it did not build an AST anything but a printer could use.
Three things would have changed the answer —

1. the orchestrator splitting a program into several Rust crates, so the per-invocation cost
   multiplies ([ADR-003](specification/adr/adr-003.md) D2's build graph);
2. Bridge-IR growing wide enough that a real program could reach the bridge backend at all;
3. the lowering acquiring node ids and spans for some other reason, which would have made the exit
   genuinely free to open, as the record believed it already was.

— and **none of the three happened.** The exit, the Bridge-IR protocol and the `rustc_ast` lowering
are withdrawn; the compiler emits Rust source text and hands `rustc` a file, which is the path §3
and §4 measured and the 0.64 % is the whole price of
([ADR-004](specification/adr/adr-004.md) D1). Condition 1 is still the thing to watch, and now it
is about the fixed cost of an extra invocation rather than about an exit from an AST nobody builds.

And one open question is handed on rather than answered: two thirds of the per-invocation cost is
the rustup shim, and whether the compiler should invoke the shim or a named compiler is a
toolchain-identity decision that belongs to a record, not to this file (§5).
