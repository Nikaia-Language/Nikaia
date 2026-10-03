# Nikaia Project Status & Roadmap

This document outlines the current status of the Nikaia compiler and the toolchain, and lists the necessary steps to reach a fully functional and stable v1.0 release.

## Where it stands, in numbers

**Counted from the boxes below, and from nothing else.** Every item on this page is `[x]` done,
`[~]` part-built or `[ ]` open; a part-built one counts a half, and what makes it a half is
written in the box. That is the whole method, and it is here so the number can be **checked**
rather than believed — a percentage nobody can re-derive is a mood.

**What a percentage here does *not* mean** is how much work is left. The four areas are not the
same size and were never meant to be: *the language* is most of the decisions and nearly all of
the compiler, and *extended targets* is four artifacts that each need one thing built and then
work. Read the areas, not the average.

| Area | Done | Boxes | What is in it |
| :--- | ---: | :--- | :--- |
| **The language**, incl. build-time | **95.5 %** | 21 of 22 | Control flow, structs, enums, `impl`, modules, generics and traits, the type checker, `sync` inferred per function, `throws` with named channels, the execution model, tests and build-time-proved `assert`s, and compile-time I/O with a grammar run over what it read. Open: the last half of *what a `pub` function promises about pausing* ([ADR-288](specification/adr/adr-288.md)) and of *the last copies and the last `unsafe`*. **Tier-1 staging is withdrawn** ([ADR-178](specification/adr/adr-178.md)) and has no box |
| **Libraries** — `http`, `std::db`, `std` | **25 %** | 1 of 4 | The runtime under them **is** built: the executor at both settings, `spawn`, `overlap`, the I/O layer. `std` exists in the narrow sense the examples need, and since 0.0.166 that includes a **socket** and HTTP/1.1's text half, with a **server** over them in `examples/http/`. Open: the parser moved into Nikaia, `rustls`, HTTP/2, the database protocol, the query DSL that checks the SQL while the program is built |
| **Tools** — `build`, `describe`, `test`, `fmt`, `doc`, LSP, self-hosting | **50 %** | 4 of 8 | `nikaia build`/`run` through Cargo, incremental compilation keyed on content, `nikaia describe` for a foreign crate's boundary, and `nikaia test`. Open: `nikaia fmt`, `nikaia doc`, the LSP, and **self-hosting** — 22.7 % of the toolchain is Nikaia ([ADR-294](specification/adr/adr-294.md)) |
| **Extended targets** — wasm, Python, C library, bare metal | **0 %** | 0 of 4 | Each is one artifact from the **same declarations**, which is the point of them: a Nikaia library does not ship twice ([ADR-284](specification/adr/adr-284.md), [ADR-284](specification/adr/adr-284.md), [ADR-284](specification/adr/adr-284.md), [ADR-119](specification/adr/adr-119.md)). `x86_64-linux` and `aarch64-linux` are the ones that work |
| **Overall** | **68.4 %** | 26 of 38 | |

*Two things the count deliberately does not flatter.* A `[x]` is **built and tested**, not
specified: the specification is far ahead of the compiler, and every box here is about the
compiler. And an area with one big open item reads worse than one with five small ones — which
is correct, because the big one is what is actually in the way.

*The version number is a different thing.* `0.0.NN` counts **change packages**, one per
[CHANGELOG](../CHANGELOG.md) heading, and says nothing about how far along anything is.

## Where the project actually stands

*In plain words: what works today, what does not, and where a bug report helps most.*

**Pre-alpha, as of 0.0.289.** The table above shows 68.4 % —
that counts *areas of scope* built, and the language area alone reads 95.5 %. Neither number says
how close you are to writing the program you have in mind. This section does, in plain words.
Every wall and risk below has an entry of the same subject in
the [issue tracker](https://github.com/Nikaia-Language/Nikaia/issues), with the evidence and the record behind it.

### What you can do today

Single programs and small multi-file projects on Linux: structs, enums, `impl`, `match`,
generics with trait bounds, modules beside the entry file, `f"…"` strings, `throws`/`catch`
with typed errors, maps and vectors, reading files and standard input, writing files, `spawn`
and `overlap`, `Shared`/`Locked` with `access_all`, views that outlive the buffer they point into — a
function that reads a file and hands back slices of it, with nothing written for it —
`test` blocks with `nikaia test` and `assert`s proved while the program is built,
grammars and `dsl` blocks, a minimal
HTTP/1.1 server, calling C through `extern "C"`, and calling a Rust crate once it is described
(`nikaia describe`). Both `user-parallelism` settings. Most mistakes are refused **in Nikaia's
own words** with an `NK`-code and a help line, and the ones that are not still point at your
`.nika` line.

### The walls you will hit

| You try to… | What happens | Because |
| :--- | :--- | :--- |
| depend on a Nikaia package by version | refused | there is no registry yet; `path = "…"` dependencies only |
| use a crate from crates.io | refused until you run `nikaia describe <crate>` and commit what it writes | foreign code is described before it is called; works, but it is a step |
| format, get completion, generate docs | nothing | no `nikaia fmt`, no LSP, no `nikaia doc`. [`editors/vscode`](../editors/) has syntax highlighting only |
| put a **view** of text into a **published** (`pub`) field or result that also gets text of its own | refused, and the message says why and what copies nothing | every user of a published field or result reads and builds its representation, which is fixed before they exist. A published *parameter* takes either kind from its own package and text of its own from others ([ADR-282](specification/adr/adr-282.md)). Everywhere else a declared `String` or `String?` — field, result, parameter, `let`, the elements of a `Vec` or a map, from inside an `f"…"` hole too — becomes a view, or either per value, by what flows into it ([ADR-282](specification/adr/adr-282.md), [282](specification/adr/adr-282.md), [282](specification/adr/adr-282.md)) |
| put your own modules in subdirectories (`src/a/b.nika`) | not found | modules are one level: a `.nika` file beside `main.nika` |
| talk to a database | not possible | `std::db` and the SQL DSL are specified, not built — which is why `fortunes` doesn't run |
| serve HTTPS or HTTP/2, or many connections at once | not possible | the server handles one connection at a time, HTTP/1.1, no TLS |
| build for anything but `x86_64-linux` or `aarch64-linux` | refused | wasm, C library, Python binding and bare metal are all specified and unbuilt |
| supervise tasks | not possible | no supervisor |
| find a function you'd expect in `std` | often missing | `std` holds what the examples needed; the *surface* is the open part |

Also expect: every file access names where it may reach (`fs::read(path, fs::Root::Anywhere)`)
— that is by design, not a gap; some errors and warnings still come from `rustc` in Rust's words (mapped to your
line, but in Rust's vocabulary); syntax and
diagnostics change between releases with no migration; `cargo test` over the **whole
workspace** fails some project tests for reasons in Cargo's package cache —
use `-p nikaia`.

### The areas the design stands on

These carry the central promises. If something is wrong with Nikaia's design, it shows up
here — so **a bug report in any of these areas is the most valuable kind**, whether the area is
finished or not.

| Area | Promise | Built | Open |
| :--- | :--- | :--- | :--- |
| **The tether** ([ADR-283](specification/adr/adr-283.md)) | a view may outlive its buffer, with no annotation and no copy | ✅ all of it: the buffer lives in the caller's frame, in a handle a task carries, or one handle per buffer where a cache drops entries — for text and for a list of structs of views alike; `nikaia --tethers` shows which | a buffer handed both to a task *and* out of the function; a *map* of structs holding views that drops entries — both refused with an explanation |
| **Text as one type** ([ADR-282](specification/adr/adr-282.md), [282](specification/adr/adr-282.md)) | text is `String`, and you never convert by hand | ✅ literals work wherever a `String` is wanted; a view handed to a function that only reads needs nothing; every declared `String` or `String?` a view flows into — field, result, parameter, `let`, a list's element, a map's key — becomes a view, or either per value, and only that position pays ([ADR-282](specification/adr/adr-282.md), [282](specification/adr/adr-282.md), [282](specification/adr/adr-282.md)) | a view for a *published* `String` field or result that also gets text of its own needs `.clone()` |
| **Functions have no colour** ([ADR-055](specification/adr/adr-055.md)) | no `async`/`await`; any function may pause | ✅ inferred everywhere, including tasks, `overlap`, and a lambda handed to `std` — `map` and `filter` then make a sequence that pauses at each step, `sort_by_key` and the map entries await it ([ADR-233](specification/adr/adr-233.md)) — and every lazy walk of `io::lines()` ([ADR-234](specification/adr/adr-234.md)); a `par_iter()` lambda is refused if it pauses (`NK2209`) | — |
| **Locks without deadlocks** ([ADR-281](specification/adr/adr-281.md)) | `access_all` takes locks in one order; a lock is never held across a pause | ✅ the lock, all its doors, and `access_all`; a call that may pause inside a door is refused (`NK2202`); a counter that crosses a thread is a compare-and-swap rather than a lock ([ADR-281](specification/adr/adr-281.md)) | the refusals themselves, `NK2201` and `NK2203` (a lock taken while one is held), are not written. What stood in their way was noise, and it is gone: the analysis they read answers *undecided* for **1 of 61** functions in the examples, a call through a handler parameter, where it was 12 before 0.0.247 (the 24 once written here was older still) |
| **SQL checked at build time** ([ADR-299](specification/adr/adr-299.md)) | a misspelled column is refused while the program is built | — | not started: `std::db`, the driver, and the query DSL |

**Just a lot of work** — decided, well specified, low risk of surprising anyone: the package
registry, the C library and everything built on it (wasm, Python, bare metal), HTTP/TLS/HTTP/2,
`fmt`/`doc`/LSP, supervision, and filling out `std`.

**No question is waiting on a decision** right now ([`open-decisions.md`](open-decisions.md)).

### So, as a tester

Expect to write small, self-contained CLI programs — parsers, log crunchers, number crunching,
a toy server — and to hit a refusal every few dozen lines. That is the useful part: a refusal
that is **wrong**, a message that doesn't tell you what to write instead, a `rustc` error that
leaks through, or a program the spec says should work and doesn't — those are exactly the
reports this stage needs. Don't bring a production service, a database-backed app, or anything
that needs a library ecosystem.

---

## What is built

*The long history of each finished box is in the [CHANGELOG](../CHANGELOG.md) and the ADR it links. A
finished box here keeps what it is, the record, and what it leaves open — nothing else.*

**The vertical slice.** `nikaia lower hello.nika` lowers the AST to Rust in one pass
(`crates/nikaia/src/emit`, [ADR-004](specification/adr/adr-004.md) D1) with the source map
[ADR-300](specification/adr/adr-300.md)'s diagnostics need, `rustc` compiles it and the binary runs;
`nikaia build`/`run` do the same through Cargo. The subprocess plus the round trip through text is
**0.64 %** of compiling the largest program the corpus lowers ([`subprocess-cost.md`](subprocess-cost.md)),
so the text interface is paid for. Measured on the way: input provenance picks the hasher
([ADR-010](specification/adr/adr-010.md), 26 % on the flagship), the generated 1BRC parser costs 659
instructions a row against 688 hand-tuned ([ADR-015](specification/adr/adr-015.md)), and `fs::map`'s
UTF-8 check is chunked per core ([ADR-016](specification/adr/adr-016.md)).

### Phase 0: The execution model

*   [x] **A program says where an overlap may happen, and the touch sets check it** ([ADR-292](specification/adr/adr-292.md), [ADR-292](specification/adr/adr-292.md)): `overlap { … }` runs its branches together, every other statement runs in written order, and a branch that touches what another touches is `NK2104`. The automatic reordering, `seq` and `ordering` are **withdrawn** (127 adjacent pairs in `examples/`, zero overlapped).
    *   *Open*: a method call's touch set, `touches` inferred from a Nikaia body, `task::scope`/`select`.
*   [x] **A pause is a suspension point, not a blocked thread** ([ADR-055](specification/adr/adr-055.md)): the lowering is implicitly `async` over our own executor — one thread at `user_parallelism = no`, a pool of futures at `yes`. `async fn`/`.await` come off the ledger's `sync` column, `std`'s pausing entries suspend on the ring or an I/O worker, and `spawn`, `TaskHandle`, `.join()` and `overlap` are built. Four tasks of one size: **1.58 s** at `no`, **0.65 s** at `yes`. Lambdas that pause, recursive pausing methods and standard input are built too ([ADR-277](specification/adr/adr-277.md), [ADR-277](specification/adr/adr-277.md), [ADR-233](specification/adr/adr-233.md), [ADR-234](specification/adr/adr-234.md)).
    *   *Measured* ([#106](https://github.com/Nikaia-Language/Nikaia/issues/106), `docs/history/runtime-cost.md` §7): against the blocking lowering it replaced, an `.await` costs a program that never overlaps nothing measurable (3-25 instructions a read, three `.await`s deep); the read's road through the runtime's I/O costs about 3,100-3,500 instructions a read more than a `read(2)` on the caller's thread. *Decided* ([ADR-263](specification/adr/adr-263.md)): a file operation on a runtime with nothing else in flight runs on the calling thread - after the normal path stopped zeroing the buffer and building an error text nobody reads (0.0.329: 3,100-3,500 → 2,500-2,600 instructions a read), and only if it then measures.

### Phase 1: Language completeness

*   [x] **Control flow**: `if/else`, `while`, `for`, `break`, `continue` ([ADR-276](specification/adr/adr-276.md)), `match`, ranges; a jump may carry its condition after it ([ADR-276](specification/adr/adr-276.md)). `loop` is **withdrawn** ([ADR-276](specification/adr/adr-276.md) D1) — `while true` is the spelling.
*   [x] **Data structures**: `struct`, `enum`, a copy with fields changed (`p with { x: … }`, [ADR-118](specification/adr/adr-118.md)). A type that holds itself gets its box from the compiler ([ADR-246](specification/adr/adr-246.md), D1–D4).
*   [x] **Methods & `impl` blocks** ([ADR-013](specification/adr/adr-013.md)). *Open*: operators as methods.
*   [x] **Generics and traits** ([ADR-295](specification/adr/adr-295.md), [ADR-295](specification/adr/adr-295.md), [ADR-295](specification/adr/adr-295.md), [ADR-295](specification/adr/adr-295.md)): a type parameter is written, not erased; a bound is enforced and names a path; who implements what is a ledger table. *Open*: a bound on an `impl`'s own parameter, `[T: A + B]` with one method declared twice.
*   [x] **Modules & imports** ([ADR-286](specification/adr/adr-286.md)): `use utils` brings in `utils.nika` beside the entry; `pub` is enforced by the language below. *Open*: nested module paths, a grammar across a module boundary, per-module incremental compilation.
*   [x] **`f"…"` interpolates, `"…"` is text** ([ADR-309](specification/adr/adr-309.md)), and **a hole is code every analysis sees** ([ADR-309](specification/adr/adr-309.md)). *Open*: holes are still parsed in the emitter, not in the AST.
*   [x] **A failure has a type, and the channel carries it** ([ADR-280](specification/adr/adr-280.md)–[280](specification/adr/adr-280.md), [ADR-280](specification/adr/adr-280.md), [ADR-280](specification/adr/adr-280.md)): `throws` is a set of error types; a `catch` matches variants; an error keeps its site and its secondary list through the box. *Open*: one `"?"` — a grammar's entry rule.
*   [x] **A key may be absent, so a map read is a `T?`** ([ADR-293](specification/adr/adr-293.md), [ADR-293](specification/adr/adr-293.md)); `??` lends its left side where the answer is only read ([ADR-279](specification/adr/adr-279.md), at a call's argument).
*   [x] **What needs no `use` is a list, enforced both ways** ([ADR-313](specification/adr/adr-313.md), [ADR-313](specification/adr/adr-313.md)); `Bytes` is the language's ([ADR-283](specification/adr/adr-283.md)); `u32`/`u64` and bit operators ([ADR-285](specification/adr/adr-285.md)); an unannotated number is typed by its uses ([ADR-285](specification/adr/adr-285.md)).
*   [x] **Tests and proofs** ([ADR-269](specification/adr/adr-269.md), [ADR-247](specification/adr/adr-247.md)): `test` blocks, `assert` whose failure names its operands' values, output tests with `--bless`, `fs::scratch`, `nikaia test --both-settings`; outside a test an `assert` is a contract, **proved while the program is built** where the MVP prover can (`prove.rs`) and checked when the program runs where it cannot, a precondition at the call that does not prove it; `--asserts` lists which and why. A written `assert` is the function's contract (ADR-269 D14-D21): carried back to the entry as a precondition or forward to the exits as a postcondition, published in the ledger, proved across packages, and checked at a second entry for every caller that does not prove it. The prover may fail to prove a true claim, never proves a false one (ADR-269 D10).
*   [~] **What a `pub` function promises about pausing** ([ADR-288](specification/adr/adr-288.md), [#92](https://github.com/Nikaia-Language/Nikaia/issues/92)): a `pub` function that loses an inferred `sync` breaks its consumers and nothing says so. **Half**: D4 (`sync(f)` in the source) is built at 0.0.255; D1/D3 (a consumer reads `"inferred"` as *may pause*), D2 (the note) and D5 (the warning against the committed ledger) are not.
*   [~] **The last copies and the last `unsafe`** ([#96](https://github.com/Nikaia-Language/Nikaia/issues/96), [#84](https://github.com/Nikaia-Language/Nikaia/issues/84), [#97](https://github.com/Nikaia-Language/Nikaia/issues/97)): a `ref` written at a `return` is still a second spelling ([ADR-202](specification/adr/adr-202.md)); generated programs still call `tether::forever` in an `unsafe` block for a task's keep ([ADR-218](specification/adr/adr-218.md) D4); a pattern inside a boxed part is `NK1193` ([ADR-246](specification/adr/adr-246.md) D5). `??` lends at every reading position since 0.0.294. **Half**: the mechanism of each is built; the last position of each is not.

### Phase 2: Compiler robustness

*   [x] **Error reporting** ([ADR-300](specification/adr/adr-300.md), [ADR-171](specification/adr/adr-171.md)): every refusal names its `.nika` line with a caret, a reason and a way out (Part III C.2). *Open*: expression-level spans.
*   [x] **`Subject ; Config`** (G18): positional data before the `;`, named options with defaults after it, filled from the callee's ledger entry. `NK1109`.
*   [x] **The type checker** ([ADR-024](specification/adr/adr-024.md)) runs before a line of Rust is emitted; `?` is part of the type language and means *no claim*, so a correct program is never rejected. Its database is the ledger ([ADR-288](specification/adr/adr-288.md), [ADR-288](specification/adr/adr-288.md) — `$V` names a receiver's type argument).
*   [x] **`sync` is earned, not entered** ([ADR-288](specification/adr/adr-288.md), [ADR-288](specification/adr/adr-288.md)): the ledger records `sync` for every function that provably cannot pause, `sync = "from(f)"` for a higher-order one. All 18 hand-written `sync`s are derivable.
*   [x] **A written call that can fail is reported** ([ADR-023](specification/adr/adr-023.md) D8, `NK2605`) **and a loop can fail** ([ADR-025](specification/adr/adr-025.md), `NK2701`): a ledger never publishes that a function cannot fail when it can. `fs::lines`/`fs::bytes` are removed; `io::lines()` streams in constant memory.
*   [x] **The build-time evaluator** ([ADR-287](specification/adr/adr-287.md), [ADR-287](specification/adr/adr-287.md), [ADR-287](specification/adr/adr-287.md)): there are no macros ([ADR-298](specification/adr/adr-298.md)). [`build_time.rs`](../crates/nikaia/src/build_time.rs) evaluates a `comptime` that calls this program's functions and methods — loops, `push`, text, tables, `Fixed` maps, across files ([ADR-311](specification/adr/adr-311.md), [ADR-311](specification/adr/adr-311.md)) — bounded by two ledger columns (`sync`, `touches`), not a sandbox. `[T: Struct]`/`[T: Enum]` and the walk over `T::fields` and `T::variants` are built ([ADR-304](specification/adr/adr-304.md), [ADR-304](specification/adr/adr-304.md), [ADR-217](specification/adr/adr-217.md)); `--comptime` prints what was unrolled.

### Phase 3: Tooling & ecosystem

*   [x] **Orchestrator (Cargo wrapper)** ([ADR-002](specification/adr/adr-002.md), [ADR-286](specification/adr/adr-286.md)): `nikaia.toml` becomes a Cargo workspace under `target/nikaia/build/`, one crate per Nikaia package, and `RUSTC_WORKSPACE_WRAPPER` points at `nikaia` itself, so a `.nika` file arrives in a `rustc` command line and is lowered there. Path dependencies all the way down; what Cargo resolved is recorded in `nikaia.lock`. *Missing*: a dependency by **version** — no record names a registry.
*   [x] **Incremental Compilation** ([ADR-021](specification/adr/adr-021.md)): SHA-256 keys per unit, `nikaia.lock` committed, a content-addressed store under `target/nikaia/cache/`; on by default, `--no-cache` opts out; a cache that fails degrades the build and never fails it. *Missing*: the test harness's `rustc` calls are still uncached.
*   [x] **Compile-time I/O** ([ADR-310](specification/adr/adr-310.md), [ADR-177](specification/adr/adr-177.md)): `comptime X: ref String = asset("f")` reads a file named in **three places** (flag, list, literal; `NK1175`/`NK1176`), a build with no list reads nothing, and a **grammar runs over what was read** by compiling the parser it generates. *Open*: no corpus grammar produces a result that crosses yet ([ADR-311](specification/adr/adr-311.md) D1 stops an `enum`).
*   [x] **`nikaia describe <crate>`** ([ADR-290](specification/adr/adr-290.md)): a `path` dependency's `pub` signatures become `contracts/<crate>.contracts`, to be reviewed; believed while the sources hash as recorded (`NK2505`). *Open*: the rustdoc-JSON reader, `cargo metadata`, a version dependency's sources ([#124](https://github.com/Nikaia-Language/Nikaia/issues/124), [#100](https://github.com/Nikaia-Language/Nikaia/issues/100)).
*   [x] **`nikaia test`** ([ADR-269](specification/adr/adr-269.md), [ADR-247](specification/adr/adr-247.md)): see *Tests and proofs* above; every runnable program in `examples/` is a package that tests itself.
*   [ ] **`nikaia fmt`** ([ADR-132](specification/adr/adr-132.md) D2, [#101](https://github.com/Nikaia-Language/Nikaia/issues/101)): no `.nika` file has ever been formatted by a tool. Born with one rule: `} else if cond {` on one line.
*   [ ] **`nikaia doc`** ([ADR-307](specification/adr/adr-307.md), [ADR-251](specification/adr/adr-251.md), [#102](https://github.com/Nikaia-Language/Nikaia/issues/102)): `std`'s hundred and eight entries carry prose held there by a test; a **field's** and a **variant's** prose have nowhere to go.
*   [ ] **LSP Server**: built on the parser and AST; once the tree is Nikaia ([ADR-294](specification/adr/adr-294.md)) the server can be written in the language it serves.
*   [ ] **Self-hosting — Nikaia compiling Nikaia** ([#91](https://github.com/Nikaia-Language/Nikaia/issues/91), [ADR-001](specification/adr/adr-001.md) D4, [ADR-294](specification/adr/adr-294.md)): **22.7 %** of the toolchain is Nikaia (13 203 of ~57 900 lines, `scripts/self_hosting.py`, redrawn into the README badge). **Stage 1** is reached when `main`'s pipeline — read, check, lower — runs Nikaia end to end, with Rust left only as adapters to crates (`winnow`, `clap`, `toml`, the file system); **Stage 2** when the lowered compiler lowers its own sources to byte-identical Rust and a test holds it. The road is a module at a time, leaves first, and **every move is a test of the compiler** — each has found defects the suite had not, fixed in the compiler in the same package (D3), never bent around in the `.nika`.
    *   *Moved*: *did you mean*, a `dsl` body's parameters, `describe`'s reader, `Fixed`'s hash, the HTML template scan, the ledger's line reader, what `nikaia.toml` may say, **the whole syntax tree** ([ADR-294](specification/adr/adr-294.md): a `Span` is two `u32`, `Spanned[T]` is generic, 0.0.275), the constant fold (0.0.278), `--trust`'s text, and the checker's types `Ty`/`Shape`, every record a ledger holds, reading a ledger back, and what the checker asks of a type (`fits`, `bind`, `substitute`, …) — all of [ADR-294](specification/adr/adr-294.md) (0.0.285–295).
    *   *Moved since*: the trait check, and the decisions of `throws`, `locks`, `touch` and the call resolution they share (0.0.320–325, [#125](https://github.com/Nikaia-Language/Nikaia/issues/125)); their **walks** stay Rust until the holes of an `f"…"` are in the tree, which [ADR-309](specification/adr/adr-309.md) decides: the grammar parses them.
    *   *Next ring* (ADR-294's road): ADR-309, then the walks of `sync`, `throws`, `touch`, `locks`; the modules that read only the tree and the types (`dsl`, `types`, the rest of `contracts::*`, `views`, `diagnostics`), and last the ones that read everything: `check` (21 k lines), `emit` (13 k), `parser` (3.9 k), `project` (3.2 k).
    *   *Costs the move measured* (ADR-294): reading `std.contracts` was read **seven times** per run, and is read once since 0.0.296 (an empty program 93 M → 23 M instructions); resolving an interned name is 572 M of 2 918 M on `n-body`, not decided.
    *   *Counted `[ ]` and not `[~]`* although it is under way: at 5 % the half would flatter, and the road is 95 % ahead.

The remaining big pieces are in **the owner's order**: the database first, the C library second, the
query DSL third, the bare-metal target fourth, the HTTP server last — which went first in its minimal
form. Self-hosting runs beside all of them: the owner's instruction is to take it as far as it goes.

*   [ ] **A database is reachable — `std::db`** ([ADR-299](specification/adr/adr-299.md)): the **protocol** only — traits, a statement, a row's values — each dialect a package beside it, because the compiler knows no SQL. First of the five, since every later demo stores something in it. A statement runs on a connection it names as its subject, and the connection's type carries its schema ([ADR-299](specification/adr/adr-299.md), decided, not built).
*   [ ] **A library for other languages** ([ADR-284](specification/adr/adr-284.md), [#88](https://github.com/Nikaia-Language/Nikaia/issues/88)): `artifact = "c-library"` — header generated off the ledger, the caller owning the memory, every call answering with a status. The direction [ADR-302](specification/adr/adr-302.md) and [ADR-302](specification/adr/adr-302.md) do **not** face: those let a program *call* C, and both are built.
*   [ ] **The query DSL checks the SQL while the program is built** ([ADR-299](specification/adr/adr-299.md), [#89](https://github.com/Nikaia-Language/Nikaia/issues/89)): a dialect is a grammar in the driver package, a misspelled column is refused at the query, the row type comes from the columns the grammar declares. Not an ORM. Its first step needs a grammar run while the program is built — built since [ADR-177](specification/adr/adr-177.md).
*   [ ] **A WebAssembly library** ([ADR-284](specification/adr/adr-284.md)): `target = "wasm32-unknown"` with `artifact = "c-library"` makes `.wasm`, `.js` and `.d.ts` from the same declarations; a pausing entry point becomes a Promise with an `AbortSignal`.
*   [ ] **A Python binding** ([ADR-284](specification/adr/adr-284.md)): `nikaia bind python` writes a `ctypes` binding from the ledger over the C library — no second artifact, no native add-on.
*   [ ] **A target without an operating system** ([ADR-119](specification/adr/adr-119.md)): `no_std` emission, `user_parallelism` pinned to `no`, interrupts as wakers, a handler checked as `fn() sync` touching no lock. After the C library; it reuses its allocator and baked settings.
*   [~] **The runtime's second half, and the HTTP server** ([ADR-303](specification/adr/adr-303.md)). **Built**: the runtime before the first statement, files on `io_uring`, sockets with kept registrations ([ADR-303](specification/adr/adr-303.md)), a minimal HTTP/1.1 server (0.0.166), its parser written in Nikaia as a grammar (0.0.248), `std::process` whose wait gives the thread up ([ADR-243](specification/adr/adr-243.md)). **Open**: `rustls` (D2), HTTP/2, many connections at once, route hashing; [ADR-289](specification/adr/adr-289.md) D18's `String`/`html::Raw` bodies and [ADR-289](specification/adr/adr-289.md)'s `Bytes`/mapping/`http::File` bodies, which `examples/fortunes.nika` waits on. `nikaia serve` is **cut** ([ADR-289](specification/adr/adr-289.md) D12). `mmap` per request is 2.5× worse than plainly reading at 4 KiB ([`zero-copy-send.md`](history/zero-copy-send.md)).
*   [~] **Standard Library**: the **shape** is settled — half Rust, half Nikaia ([ADR-014](specification/adr/adr-014.md)), every entry says what it throws ([ADR-280](specification/adr/adr-280.md)) — and the *surface* is what is left. The share written in `.nika` grows with the self-hosting road above. *Open questions on its surface* are in [`open-decisions.md`](open-decisions.md) (`Path`, `fs::exists`, `html::Raw::new`).

### Phase 4: Backend optimization

*   [x] **Cranelift / LLVM (investigation)** ([ADR-021](specification/adr/adr-021.md) D9): Cranelift is compatible and buys ~1 s on an incremental rebuild, nothing on a full one; a supported option, not the default. There is no `--backend` switch since [ADR-260](specification/adr/adr-260.md) D3: each single-file job is a command of its own, so there is no name to give an unbuilt backend.

Tier-1 staging is **withdrawn** ([ADR-178](specification/adr/adr-178.md)) and has no box, because the absence is the decision: every candidate in [`staging-candidates.md`](history/staging-candidates.md) closed, three against their own prediction. What would reopen it is a candidate with a **measured crossover**. The callgrind harness stays (`crates/nikaia/tests/measure.rs`).

---

## What is next

This file is the long view. **What to do next is one list, and it is not here**:
the [issue tracker](https://github.com/Nikaia-Language/Nikaia/issues) holds every decided-and-unbuilt item, with
type (`Bug`, `Feature`, `Task`), area labels, and **Priority** and **Effort** fields for the order to take them. The unchecked boxes above are *scope*; an entry there has a record behind it and only
*when* is open. Scope becomes work by being decided — that is [`open-decisions.md`](open-decisions.md).

What moved since the last roadmap pass (0.0.266 → 0.0.289), so it is on this page and not only in
the CHANGELOG:

*   **Self-hosting became a road** ([ADR-294](specification/adr/adr-294.md), [294](specification/adr/adr-294.md), [294](specification/adr/adr-294.md)) — the syntax tree is Nikaia; the checker's types are next.
*   **`nikaia test` and the prover** ([ADR-269](specification/adr/adr-269.md), [247](specification/adr/adr-247.md)) — built.
*   **`std::process`** ([ADR-243](specification/adr/adr-243.md)), **`u32`/`u64`** ([ADR-285](specification/adr/adr-285.md)), **typed numbers** ([ADR-285](specification/adr/adr-285.md)), **a list has no `+`** ([ADR-253](specification/adr/adr-253.md)) — built.
*   **New decided-and-unbuilt**: the connection is the statement's subject ([ADR-299](specification/adr/adr-299.md)); what a `pub` function promises about pausing ([ADR-288](specification/adr/adr-288.md)); the rest of a self-holding type ([ADR-246](specification/adr/adr-246.md) D5).
*   **How a design question is prepared for the owner** — four weights ([ADR-258](specification/adr/adr-258.md)).
