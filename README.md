<div align="center">
  <img src="nikaia-logo.jpg" alt="Nikaia Logo" width="300" height="164" />
  <h1>N I K A I A</h1>
  <p><strong>Good wins.</strong></p>

  <p>
    <a href="#-why-this-exists">Why</a> •
    <a href="#-what-nikaia-does-differently">What's different</a> •
    <a href="#-a-language-for-the-age-of-generated-code">AI-generated code</a> •
    <a href="#-what-its-good-at--and-what-it-isnt">Good for</a> •
    <a href="#-how-it-compares">Comparison</a> •
    <a href="#-two-switches-one-language">Switches</a> •
    <a href="#-getting-started">Getting started</a> •
    <a href="#-where-the-project-actually-stands">Status</a> •
    <a href="docs/specification">Specification</a> •
    <a href="https://nikaia-lang.org/">Documentation site</a> •
    <a href="https://gemini.google.com/gem/1T8viw7ZHA0TwDZDhr6h1mgRBVnw3aTNP?usp=sharing">Gemini explains Nikaia</a>
  </p>

  <img src="assets/badges/version.svg" alt="Version" />
  <img src="assets/badges/status.svg" alt="Status" />
  <img src="assets/badges/license.svg" alt="License" />
  <a href="https://nikaia-lang.org/"><img src="assets/badges/docs.svg" alt="Documentation site" /></a>
  <a href="https://github.com/Nikaia-Language/Nikaia"><img src="assets/badges/github.svg" alt="GitHub repository" /></a>
</div>

---

## ⚡ Why this exists

Writing fast, correct software today means paying a tax that has nothing to do with your
problem.

You want to read a file and answer a request. Instead you decide whether the function is
`async` or not — and that decision infects every caller. You want to keep a name that points
into a buffer you already have. Instead you write lifetime annotations, or you allocate a
copy you didn't need. You want a value two tasks can see. Instead you pick `Rc` or `Arc`
*by hand*, and if you pick wrong the code stops compiling three modules away. You take two
locks and hope everyone else in the codebase takes them in the same order.

None of that is your program. All of it is bookkeeping — and bookkeeping is exactly the kind
of work a compiler is good at.

**Nikaia is a bet that the whole tax is compiler work.** You write straight-line code that
says what should happen. The compiler decides the concurrency model, the pointer types, the
lock order, the lifetimes, and the state machines. Not by guessing, and not by adding a
garbage collector — by inference over a program it can see all of.

### Why now

The bet would have been a harder sell fifteen years ago, because for fifteen years nobody had
to take it. Efficiency in software has gone through three eras:

1. **Constraint.** Small machines and small budgets meant you had to understand the machine —
   memory layout, I/O cycles, what the cache actually does. Every line had physical weight.
2. **Subsidy.** Cheap cloud capacity and cheap capital turned horizontal scaling into the
   answer to every performance question. Hardware quietly paid the bill for inefficient
   software, and the bill compounds: a home computer of the constraint era ran its operating
   system, an editor and a game inside 64 KB, while a chat window today asks for a few hundred
   megabytes — four orders of magnitude to show a list of messages. A CRUD service came to need
   an orchestrator to stay up.
3. **The wall.** Training and inference now compete for the same power, silicon and datacenter
   capacity as everything else, and competition prices things. "Throw more servers at it" is no
   longer the cheap answer to a design problem — and in some regions there are no more servers
   to throw.

Nikaia is a synthesis, not a rollback to era one. You keep the ergonomics the subsidy bought —
no manual thread management, no callback hell, no lifetime bookkeeping — and the compiler pays
for them **once, at build time**, instead of the runtime billing you for them **per request,
forever**. That trade is the entire economic argument for the project.

The same technology is behind both halves of it: AI is why a growing share of code is no longer
typed by a human — and a generator needs *guarantees*, not comfort — and it is also why the
compute that code runs on stopped being cheap. That gets
[its own section](#-a-language-for-the-age-of-generated-code) below.

The language is named after my daughter **Nika**. Her name carries more than one lineage, and
the deeper one is not Greek: in Persian, *nik* means good — virtuous, good the way a person is
good and not the way a product is. The motto is just the name with a verb: **good wins**.

Not over anyone. There is no war here between people and their tools, and least of all one
where the human beats the machine. It says the thing this project refuses to accept: that good
and fast are opposites, and that being decent to the person writing the code has to be paid for
at runtime. Here it is the other way round. Because the language never makes you write `Arc`,
or a lifetime, or a lock order, those decisions belong to the compiler — and only because they
belong to it can it pick `Rc` where nothing of yours runs at once and `Arc` where it does, order the locks, and infer
borrow contracts across a whole program. A stricter language would have to take your word for
it instead. Good is not the price of fast here; it is the reason for it.

Fittingly, the ancient city the name also points to is remembered for a council, where a schism
ended in consensus rather than in one side defeating the other. The rest of the story is in the
[**Manifesto**](manifesto.md).

---

## 💎 What Nikaia does differently

The switches are the *packaging*. These are the actual claims — each one is specified, and
each one links to the decision record that argues it:

**1. Functions have no colour.**
There is no `async` and no `await`. Any function may pause on I/O; the compiler builds the
state machine. This is not "async made easier" — it deletes the split of a language's
ecosystem into a sync half and an async half, where every library has to exist twice.
→ [Spec Part I](docs/specification/10-nikaia-light.md)

**2. Ownership without lifetime annotations.**
Nikaia compiles through the Rust toolchain, so it inherits the borrow checker's guarantees —
but you never write `'a`. Borrow contracts are inferred whole-program and written to an
auditable ledger; the cases that are genuinely inexpressible in safe Rust (a slice stored in
a struct, a task borrowing from its parent) get real language constructs instead of a lecture.
→ [ADR-005](docs/specification/adr/adr-005.md), [ADR-008](docs/specification/adr/adr-008.md)

**3. One source, every runtime.**
You write `Shared[T]`. What it becomes underneath is the compiler's: the atomic reference count
is the floor, and a value it can **prove** never crosses a thread gets the plain one instead —
decided per value rather than per build ([ADR-037](docs/specification/adr/adr-037.md) D7). The
lock in `Locked[T]` is decided the same way and off the same answer
([ADR-057](docs/specification/adr/adr-057.md)), and so is where `spawn`'s tasks run.
Your source file does not encode the deployment decision, so changing it is a line in
`nikaia.toml`, not a refactor.

**4. Deadlocks removed by construction.**
Multiple resources are requested together — `access_all(a, b)` — and the runtime always takes
them in address order. A deadlock cycle between two `access_all` callers is not unlikely; it
is unconstructible. The lambda must be `sync` (provably non-pausing), so a lock can never be
held across an I/O suspension point.
→ [Spec Part II](docs/specification/20-nikaia-advance.md)

**5. Grammars are part of the language, not a preprocessor.**
`dsl` is an expression. A scannerless grammar can be parsed at compile time *or* at runtime
with the same syntax, embedded DSLs get their own real syntax (SQL, JavaScript, x86) instead
of stringly-typed interpolation, and a grammar rule marked `@frame` can be folded in parallel
with `par_fold` — with the compiler *verifying* the resynchronization property rather than
trusting your word for it. This is why parsing benchmarks are a first-class target, not a demo.
→ [ADR-007](docs/specification/adr/adr-007.md), [ADR-009](docs/specification/adr/adr-009.md)

**6. The compiler tracks where your data came from.**
Trust is a property of the *source*, not of how you build: bytes off a socket are untrusted,
your own config file is not, and that provenance travels with the value. The compiler then
picks a DoS-resistant hasher exactly where it matters and a fast one everywhere else,
instead of making every program pay for the worst case — or, worse, making you remember.
→ [ADR-010](docs/specification/adr/adr-010.md)

**7. No garbage collector.**
Deterministic teardown via ownership and RAII, including under implicit async, where "when
does this file close" is otherwise a genuinely hard question.
→ [ADR-006](docs/specification/adr/adr-006.md)

### How is that even possible?

Nikaia is not a new backend. The frontend lowers a `.nika` file to **Rust source text**, and
`rustc` compiles that like any other Rust. That is what makes points 1, 2 and 3 tractable:
the hard safety machinery already exists and is battle-tested — Nikaia's job is to stop
making humans operate it by hand. Keeping the generated Rust readable is deliberate, not a
stopgap: it is the text that is compiled, so it is how a compiler bug stays inspectable.

**What you need installed is an ordinary stable Rust toolchain, and nothing else.** The Rust
that comes out uses no unstable feature, no `-Z` flag is passed anywhere, and no crate in the
workspace declares a `#![feature(…)]`. A `nikaia` links `libc` and nothing exotic, so the
binary goes where you put it.
→ [ADR-003](docs/specification/adr/adr-003.md), [ADR-001](docs/specification/adr/adr-001.md) D1,
[ADR-004](docs/specification/adr/adr-004.md) D1,
[Toolchain architecture](docs/toolchain_architecture.md)

---

## 🤖 A language for the age of generated code

There is an obvious objection to launching a systems language in 2026: if a model writes the
`if err != nil` chains and argues with the borrow checker on your behalf, who cares how
ergonomic the syntax is? "Nicer to type" is a shrinking argument.

The objection is correct, and it points straight at the stronger one. When a machine writes the
code, the question stops being *how pleasant is this to write* and becomes **what can the
compiler still prove about code that no human wrote?** Every claim above changes meaning under
that question:

* **Boilerplate costs context, not just keystrokes.** A model has a finite window and loses the
  thread as it fills. Ceremony — `async`/`await` plumbing, lifetime annotations, `Arc::clone`
  dances, error-propagation chains — is budget spent on machinery instead of on your problem.
  Nikaia is dense on purpose. What used to be a comfort argument for humans is now a
  *capability* argument for the generator.
* **Concurrency is where generated code fails silently.** Models are very good at plausible code
  and weak on the memory model. A hallucinated lock order or a shared mutable capture is not a
  compile error in Go or C++; it is a bug that appears in production, under load, once. In
  Nikaia a deadlock between `access_all` callers is unconstructible, a lock cannot be held
  across a suspension point, and data races are rejected at compile time however much runs at once.
  The compiler is a merciless reviewer for exactly the class of defect human review is worst at.
* **Fewer decisions to get wrong.** `Rc` or `Arc`? The sync or the async variant of this API? Is
  this future `Send`? Each is a coin flip a generator can lose, and losing it surfaces as an
  error three modules away, in code the author has never read. In Nikaia these decisions are not
  in the source at all — the build switches settle them at build time.

This is also the honest answer to a new language's chicken-and-egg problem. Nobody has to learn
Nikaia to get something out of it: hand a model the specification and your requirements, and let
a compiler that rejects deadlocks, races and leaks decide whether what comes back is sound. A
generated program that runs fast and refuses to race is a better first contact with a language
than a tutorial is.

**The honest caveat:** no model has Nikaia in its training data, so generating it means putting
the specification in context. That is a hard constraint on this repository, not an afterthought
— the spec is written to be precise and small enough to fit, and a
[ready-made prompt bundle](#roadmap-to-010) is on the roadmap. Until then, `docs/specification/`
plus `examples/` is the bundle.

---

## 🎯 What it's good at — and what it isn't

**Use Nikaia when:**

* You are writing **I/O-dense services** — HTTP APIs, proxies, edge workers, CLI tools that
  talk to the network — and you want Go-like ergonomics with no GC pauses and no `async`
  colouring.
* You are chewing through **data in a custom or semi-structured format** — logs, telemetry,
  columnar dumps, protocol frames. The grammar protocol plus zero-copy tethered slices is
  the part of Nikaia that is most obviously not just "Rust with fewer keystrokes".
* You need **CPU-bound throughput** — simulation, image and signal processing, aggregation
  over huge inputs — and you would like the parallelism to be checked, not hoped for.
* One team owns both halves and is tired of maintaining **a fast core in one language and a
  service layer in another**, with a serialization boundary in the middle.
* You intend to **generate most of the code anyway** and would rather have the guarantees
  checked by a compiler than by a code review — see the [section above](#-a-language-for-the-age-of-generated-code).
* **Your compute bill is a line item somebody looks at** — density and no GC mean the machine
  you already pay for does more, instead of the cluster growing to cover the runtime's habits.

**Don't use Nikaia when:**

* You need something you can ship next quarter. Read the [status](#-where-the-project-actually-stands)
  section: today this is a specification and a bootstrap compiler.
* You want a REPL-first scripting language, a mature package ecosystem, or ten years of
  Stack Overflow answers. Python and Rust respectively are better at being Python and Rust.
* Your bottleneck is a **runtime**, not a library. A C-ABI dependency is fine in principle —
  Chapter 15 specifies `extern "C"` with `unsafe` at the boundary — but PyTorch, Spark or a
  vendor's JVM SDK are not libraries you link against, they are runtimes you would have to host.
  Embedding one means carrying its GIL or its garbage collector inside your process, which is
  usually the thing you came here to avoid. Nikaia does not dissolve that trade; it only changes
  where you make it.

---

## 📊 How it compares

| | Python / TS | Go | Rust | **Nikaia** |
| :--- | :--- | :--- | :--- | :--- |
| Async in the type system | colours functions | invisible (goroutines) | colours functions | **invisible** |
| Memory management | GC | GC | ownership, manual annotations | **ownership, inferred** |
| Pause times | GC pauses | GC pauses | none | **none** |
| Thread-safe vs single-thread types | n/a | n/a | you choose `Rc`/`Arc` | **one type, `user_parallelism` decides** |
| Data-race protection | none / GIL | detector at runtime | compile time | **compile time** |
| Deadlock protection | none | none | none | **`access_all` ordering** |
| Embedded DSLs | strings / metaprogramming | none | proc macros over Rust tokens | **first-class scannerless grammars** |
| Single-core / multi-core | one runtime | one runtime | you build it | **one switch** |
| Maturity | ✅ production | ✅ production | ✅ production | ⚠️ **specification** |

The last row is the honest one, and it is the only one where Nikaia loses on purpose.

---

## 🎛 Two switches, one language

Nikaia adapts to your problem, not the other way around. There is one language, and two lines
in your `nikaia.toml` decide how it is built — never what it means.

### `target` — which machine
`x86_64-linux` by default. The machine decides what `std` can offer and what a panic does: an
orderly unwind where the machine unwinds, a trap where it traps. `wasm32-unknown` is named and
**refused**, with the gap said out loud rather than code emitted for a different machine — what
`std::fs` offers where there is neither a memory mapping nor a thread is undecided.

### `user_parallelism` — how much of *your* code runs at once
* **`no`** (default) — your code runs on **one** thread, over an event loop. **Data races are
  impossible**: two pieces of your code are never in flight together, so nothing you write needs
  a lock. Microservices, web servers, CLI tools, edge workers — where you would reach for
  Node.js or Go.
* **`yes`** — multi-threaded work-stealing runtime, every core busy, thread safety proven by
  the borrow checker. HPC, game engines, heavy backends — where you would reach for Rust or C++.

**This is not the one thread you know from Python.** There the single thread is the whole
machine and the other cores stand idle. Here it bounds **your instructions** and nothing else:
the I/O runs on threads of the runtime's own, and where the kernel offers a completion queue it
does not need a thread to wait on one. What `std` itself does uses the machine whatever this
switch says — a file's text is validated in chunks across every core, **63.7 ms down to
16.5 ms** ([ADR-016](docs/specification/adr/adr-016.md)). Your code stays a straight line; the
machine does not stand still.

It is a permission, not a count — in both directions. *How many* threads serve a `yes` is the
runtime's to decide, because the right answer belongs to the machine and not to the source file.
And on a machine with no threads the runtime has none either: that is why `wasm32-unknown` is
not yet a target a program can be built for, and the compiler says so with a reason rather than
pretending otherwise.

**The word *your* is load-bearing.** It bounds your program, not the compiler: reading a file
may still validate its text on four cores at `no`, because that is not code you wrote and it
changes nothing your program prints.

Because the difference lives in the compiler rather than in your source, a library built at `no`
is checked against the rules parallel code needs too — you cannot accidentally ship something
that only works single-threaded.

→ [ADR-037](docs/specification/adr/adr-037.md)

---

## 🚀 Getting started

Linux on x86_64 is the one machine this works on today. macOS and Windows are untested.

**1. A Rust toolchain.** Stable, nothing else — `rustup` reads the channel from
`rust-toolchain.toml` and installs it on first use. **Rust 1.88 or newer** is required:
the compiler, `std` and the programs it generates are tested on 1.88.

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**2. Check out and build the compiler.**

```sh
git clone https://github.com/Nikaia-Language/Nikaia.git
cd Nikaia
cargo build --release -p nikaia        # about a minute; the binary is target/release/nikaia
export PATH="$PWD/target/release:$PATH"
```

The compiler finds its `std` in the checkout it was built from, so **leave the checkout where
it is**. If you move it, point `NIKAIA_SYSROOT` at its `crates/` directory.

**3. A project folder.** A project is a `nikaia.toml` and a `src/main.nika`:

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

**4. Run it.**

```sh
cd hello
nikaia run              # or: nikaia build
```

The first build compiles `std` and its dependencies (≈15 s) and caches them in
`~/.cache/nikaia`; later builds reuse that. Arguments go after `--`: `nikaia run -- a b c`.
The build writes `nikaia.contracts` (the inferred borrow ledger — commit it) and `nikaia.lock`
beside the manifest. There is no `nikaia new` yet; create the two files by hand.

**Where to go next:** copy any file from [`examples/`](examples/) into `src/main.nika` and
run it — [`calc.nika`](examples/calc.nika) (`nikaia run -- "2 + 3 * 4"`) and
[`tally.nika`](examples/tally.nika) are small; [`hello-http/`](examples/hello-http/) is a
project with a dependency. The language itself is [Spec Part I](docs/specification/10-nikaia-light.md).
Working on the compiler: `cargo test -p nikaia` (see [the status](#-where-the-project-actually-stands)
for why not the whole workspace).

---

## 💻 Programs that run

Real programs live in [`examples/`](examples/): the One Billion Row Challenge, a four-function
calculator, a web access log summarised, an INI file with comments, a JSON document, two
Computer Language Benchmarks Game programs, an HTML table that cannot be made to leak markup, a
stock list rendered to a page **on disk**, a pipe tallied in constant memory, the same program
split across three files, an HTTP server, a real C library called through `extern "C"`, and
the TechEmpower `fortunes` benchmark. **All but `fortunes` compile, run, and are checked by
`cargo test`** — the single-file ones at either setting, with their output required to be
identical; `fortunes` is still written at specification level.

They are there because writing a real program against a spec is the cheapest way to find out
what the spec forgot, and [`examples/README.md`](examples/README.md) lists exactly which gaps
each one exposed and which are still open. `tests/samples/` holds the smaller programs the
bootstrap compiler can already parse.

---

## 🚦 Where the project actually stands

**Pre-alpha, as of 0.0.217.** The [roadmap](docs/project_status_and_roadmap.md) shows 74 % —
that counts *areas of scope* built, and the language area alone reads 100 %. Neither number says
how close you are to writing the program you have in mind. This section does, in plain words.
Every wall and risk below has an entry of the same subject in
[`open-work.md`](docs/open-work.md), with the evidence and the record behind it.

### What you can do today

Single programs and small multi-file projects on Linux: structs, enums, `impl`, `match`,
generics with trait bounds, modules beside the entry file, `f"…"` strings, `throws`/`catch`
with typed errors, maps and vectors, reading files and standard input, writing files, `spawn`
and `overlap`, `Shared`/`Locked` with `access_all`, views that outlive the buffer they point into — a
function that reads a file and hands back slices of it, with nothing written for it —
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
| write tests | nothing to run them with | no `nikaia test` and no `assert` — Part III 14 is not built. Compare output instead |
| format, get completion, generate docs | nothing | no `nikaia fmt`, no LSP, no `nikaia doc`. [`editors/vscode`](editors/) has syntax highlighting only |
| put a **view** of text into a **published** (`pub`) field or result that also gets text of its own | refused, and the message says why and what copies nothing | every user of a published field or result reads and builds its representation, which is fixed before they exist. A published *parameter* takes either kind from its own package and text of its own from others ([ADR-232](docs/specification/adr/adr-232.md)). Everywhere else a declared `String` or `String?` — field, result, parameter, `let`, the elements of a `Vec` or a map, from inside an `f"…"` hole too — becomes a view, or either per value, by what flows into it ([ADR-222](docs/specification/adr/adr-222.md), [223](docs/specification/adr/adr-223.md), [224](docs/specification/adr/adr-224.md)) |
| do I/O inside a lambda handed to `std` (`map`, `filter`, …) | refused — write a `for` loop | `std`'s entries take synchronous Rust closures; a lazy walk of a pausing sequence has no shape yet |
| put your own modules in subdirectories (`src/a/b.nika`) | not found | modules are one level: a `.nika` file beside `main.nika` |
| talk to a database | not possible | `std::db` and the SQL DSL are specified, not built — which is why `fortunes` doesn't run |
| serve HTTPS or HTTP/2, or many connections at once | not possible | the server handles one connection at a time, HTTP/1.1, no TLS |
| build for anything but `x86_64-linux` | refused | wasm, C library, Python binding and bare metal are all specified and unbuilt |
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
| **The tether** ([ADR-209](docs/specification/adr/adr-209.md)) | a view may outlive its buffer, with no annotation and no copy | ✅ all of it: the buffer lives in the caller's frame, in a handle a task carries, or one handle per buffer where a cache drops entries — for text and for a list of structs of views alike; `nikaia --tethers` shows which | a buffer handed both to a task *and* out of the function; a *map* of structs holding views that drops entries — both refused with an explanation |
| **Text as one type** ([ADR-207](docs/specification/adr/adr-207.md), [208](docs/specification/adr/adr-208.md)) | text is `String`, and you never convert by hand | ✅ literals work wherever a `String` is wanted; a view handed to a function that only reads needs nothing; every declared `String` or `String?` a view flows into — field, result, parameter, `let`, a list's element, a map's key — becomes a view, or either per value, and only that position pays ([ADR-222](docs/specification/adr/adr-222.md), [223](docs/specification/adr/adr-223.md), [224](docs/specification/adr/adr-224.md)) | a view for a *published* `String` field or result that also gets text of its own needs `.clone()` |
| **Functions have no colour** ([ADR-055](docs/specification/adr/adr-055.md)) | no `async`/`await`; any function may pause | ✅ inferred everywhere, including tasks and `overlap` | a lambda that pauses, handed to `std` (`map`, `filter`), and a lazy walk of a pausing sequence — refused, write a loop |
| **Locks without deadlocks** ([ADR-057](docs/specification/adr/adr-057.md)) | `access_all` takes locks in one order; a lock is never held across a pause | ✅ the lock, all its doors, and `access_all` | the analysis that would refuse misuse answers *undecided* for 24 of 59 functions in the examples, so those refusals are not switched on |
| **SQL checked at build time** ([ADR-143](docs/specification/adr/adr-143.md)) | a misspelled column is refused while the program is built | — | not started: `std::db`, the driver, and the query DSL |

**Just a lot of work** — decided, well specified, low risk of surprising anyone: the package
registry, the C library and everything built on it (wasm, Python, bare metal), HTTP/TLS/HTTP/2,
`fmt`/`doc`/LSP, `nikaia test`, supervision, and filling out `std`.

**No question is waiting on a decision** right now ([`open-decisions.md`](docs/open-decisions.md)).

### So, as a tester

Expect to write small, self-contained CLI programs — parsers, log crunchers, number crunching,
a toy server — and to hit a refusal every few dozen lines. That is the useful part: a refusal
that is **wrong**, a message that doesn't tell you what to write instead, a `rustc` error that
leaks through, or a program the spec says should work and doesn't — those are exactly the
reports this stage needs. Don't bring a production service, a database-backed app, or anything
that needs a library ecosystem.

---

### Where to start reading

| If you want to… | Read |
| :--- | :--- |
| know why the project exists at all | [manifesto.md](manifesto.md) |
| learn the language | [Spec Part I](docs/specification/10-nikaia-light.md) |
| see the concurrency and parallelism model | [Spec Part II](docs/specification/20-nikaia-advance.md) |
| understand a design decision | [the ADR index](docs/specification/adr/README.md) |
| see real programs | [examples/](examples/) |
| let a model write Nikaia for you | [the spec](docs/specification) + [examples/](examples/) in one context |
| work on the compiler | [toolchain architecture](docs/toolchain_architecture.md) |

Contributions, objections and "this cannot work because…" arguments are all welcome —
especially the third kind. An unimplemented specification is the cheapest possible moment to
be told you are wrong.

---

## ❤️ Dedication — for Nika

**Nika is my daughter, and I love her more than anything I will ever build.**

Everything in this repository — the specification, the compiler, every argument above — sits
downstream of one simple wish: that the world she grows into has fewer walls in it than the one
I found. The language carries her name because she is the reason it exists at all.

*Because the future belongs to those who build it.*

We are already standing in tomorrow's past: everything that will be ordinary in twenty years is
being decided right now. Everyone decides, and everyone prioritises — and it does not have to be
the whole machine. One gear, turning, is enough to move the ones around it.

That is one half of how I think about Nikaia. It is not mainstream. It takes a different route
and argues with settled consensus in several places. That is the part of me swimming against the
current: one person deciding to turn, and accepting that turning alone is slow.

The other half is knowing I am made of the river. Nikaia was written with the help of AI, which
stands on the accumulated knowledge of everyone who ever wrote a compiler, a paper, a textbook,
a Stack Overflow answer. It rests on Rust's borrow checker, on decades of runtime research, on a
language nobody in this repository invented. It was built with money, electricity and machines
that the flow provided. Not one idea in it is uncaused. Nothing here was taken from nowhere.

It also cost less than it should have. Without AI, this project would have lost the
prioritisation to the people I love — and rightly so. What AI changed is the price: it made
things move fast enough that Nikaia could exist without taking its time out of theirs. Money,
yes. Evenings, far fewer than it would have cost a few years ago.

Both are true at once, and the contradiction is not a flaw in the story — it is the story.
Swimming against the stream is still swimming in it. This is a solo effort that only makes sense
as a shared work: it exists to show what a single person can still put into the world, and in
the same breath to admit how much of that was already lying there, done by others, waiting to be
picked up.

So: to everyone whose work is upstream of this one — the named and the uncounted, the people who
built the tools and the people who wrote down why — thank you. Most of you never heard of this
project. And to whoever is downstream, who will take this further or take it apart and build
something better: this was always meant to end up in your hands.

And to Nika: whatever becomes of it, I would rather hand you a world shaped a little less by
walls and a little more by the freedom to build. ❤️

---

## ⚖️ Intellectual property & governance

Nikaia introduces the **Unified Core Architecture**, a novel approach to compile-time
orchestration, deterministic concurrency (`access_all`), and switch-based runtime
transformation.

**For corporate entities & implementers:**
This repository establishes public **prior art** for these architectural concepts, ensuring
they remain unencumbered for the open-source ecosystem.

While Nikaia is released under the **Apache 2.0 License**, the project is designed with
long-term governance in mind to prevent proprietary fragmentation. We actively invite
organizations interested in adopting, extending, or standardizing these concepts to join us as
**Founding Partners** rather than attempting parallel implementations.

The architecture is complex; let's build the standard together.

---

## 📄 License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](LICENSE) file for details.

<div align="center">
  <sub>For Nika. For everyone downstream.</sub>
</div>
