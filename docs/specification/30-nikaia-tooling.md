# Nikaia Language Specification
**Part III: Tooling, Ecosystem & Interoperability**
**Version:** 0.0.433 (Draft)
**Date:** 2026-10-04

---

## Chapter 13: The Toolchain (CLI)

Nikaia provides one command-line interface, `nikaia`. It builds and runs a project, manages its dependencies, formats its source and runs its tests.

**Prerequisites.** A Nikaia installation requires a **stable** Rust toolchain and nothing else. The compiler emits stable Rust and hands it to `cargo`. It uses no unstable compiler feature and no `-Z` flag. A Nikaia installation, `std` and the emitted Rust require Rust **1.88 or newer**. The emitted Rust is Edition 2024 ([ADR-305](adr/adr-305.md)). The compiler writes that number as `rust-version` into every generated `Cargo.toml` and compares it with `rustc --version` before `cargo` runs. An older toolchain is refused before the backend runs.

### 13.1. Project Structure
`nikaia new my_project` generates the following structure:

* `nikaia.toml`: the **manifest**. It describes the project, its authors and its dependencies.
* `nikaia.lock`: the **lockfile**. It records everything that determines the build and is also the **cache key**.
    * **Asset hashing:** where a grammar reads an external file (`asset("schema.sql")`), the compiler records the file's SHA256 hash here. The read itself is permitted by an allowlist file put in effect with `--allow-read-from-list=…` (Part II 10.2, [ADR-310](adr/adr-310.md)); the lockfile records it after the fact. A build given no allowlist reads nothing at compile time.
    * **Source hashing:** the SHA256 of each `.nika` source that took part. Every file is still parsed, once, because the parse is what says which files the program is; where none changed, the checks, the expansion, the lowering and the inference are served from the cache ([ADR-286](adr/adr-286.md) D8).
    * **Resolved versions:** the exact dependency versions, the toolchain version used, and the Nikaia compiler's own version. The compiler's version is part of the key. The dependency versions are recorded without being hashed into the key; Cargo's own fingerprinting covers them.
    * **Declaration and record:** `nikaia.toml` states what the project requires; `nikaia.lock` records what was resolved and used.
    * **Not in the lockfile:** the build options of Part I 1.2, `opt-level` and the backend. They are hashed into the cache key and never written.
    * **Incremental builds:** where the hashes on disk are unchanged, the compiler skips re-processing and reuses the artifact from the content-addressed store under `target/nikaia/cache/`. The store is git-ignored. The lockfile holds inputs and the store holds outputs. Keys are per translation unit, so one changed asset invalidates that unit and not the project.
* `nikaia.contracts`: the **borrow contract ledger**. It is generated and committed like the lockfile. It records the borrow contracts the compiler inferred for the program's functions and the tether relationships of its structs. It is an incremental-build cache and the basis of the compiler's contract-change diagnostics (13.5).
* `nikaia.proofs`: the **proof file** ([ADR-270](adr/adr-270.md) D4, D19-D23). It is generated and committed like the lockfile. It holds one entry per query the prover asks: the SHA-256 of the query in normal form and its answer - `proved` with its certificate, `refuted` with its model, or `unknown`. Every entry is checked when read, so a wrong entry is ignored, never believed. A build uses only what the file holds; the solver's search, which may be timed, parallel and cached, only adds to it. Long certificates live in `nikaia.proofs.d/`. *Decided, not built yet.*
* `src/`: the source. A source file is at most 4 GiB; a longer one is refused where it is read ([ADR-294](adr/adr-294.md) D7).
    * `main.nika`: the entry point.

### 13.2. Core Commands
* `nikaia build`: compiles the project.
* `nikaia run`: compiles and executes. `nikaia run <file>.nika` compiles and executes one file. Inside a project the file has to be that project's entry point. Outside any project it is built as a project of its own, kept in the user's cache directory and keyed on the file's path, through the same Cargo workspace `nikaia build` makes. The interpreter is not what `run` uses.
* `nikaia test`: runs the package's `test` blocks, each in a process of its own, and its output tests, `tests/NAME.stdout` (14.1). `--both-settings` runs each at `user_parallelism = no` and `yes`.
* `nikaia bench`: runs performance benchmarks.
* `nikaia fmt`: formats the source.
* `nikaia describe <crate>`: writes the draft ledger for a Rust crate the program calls, from the crate's own `pub` signatures (15.2). `--project` names the project directory. The default walks up from the working directory to the nearest `nikaia.toml`.
* `nikaia bind <language>`: writes a binding for a library build from the ledger: `python` over `ctypes`, `js` for a WebAssembly build (15.1).
* `nikaia lower-std`: re-lowers `std`'s Nikaia half (13.2b).

Three commands take a single `.nika` file:

* `nikaia lower <file>.nika`: lowers the file to Rust and stops. It writes `<file>.rs`, or the path `--output` names, and the ledger beside it (13.5). It does not call `rustc`.
* `nikaia interpret <file>.nika`: runs the file in the interpreter. Nothing is written and nothing is compiled.
* `nikaia explain <file>.nika`: reads `rustc --error-format=json` on stdin and reports each message against the `.nika` source instead of the emitted Rust.

A file inside a project is a file of that project: the nearest `nikaia.toml` above it decides its `[build]` settings and its dependencies, exactly as `nikaia build` reads them. A file outside any project is standalone. It has the default settings and the command-line switches, and no dependencies.

**Backends.** `nikaia build`, `nikaia run` and `nikaia lower` compile through the `rust` backend. It is the only code generator, and it is part of every installation. `nikaia interpret` runs a program instead of producing one. There is no `--backend` switch: each command says which job is asked.

### 13.2b. Where `std` Comes From (the Sysroot)

`std` is not resolved by a registry and is not a `type = "rust"` dependency (13.3). A generated project depends on it by path into a **sysroot**: a directory that travels with the compiler and holds `std`'s sources. `NIKAIA_SYSROOT` names the sysroot. By default it is the source tree the compiler was built from.

* **`std` ships as sources, with its Nikaia half already lowered.** The parts of `std` written in Nikaia are lowered to Rust at release time, and the `.rs` sits beside the `.nika` it came from; the toolchain's own Nikaia, `src/tools`, is one package lowered into one `package.rs` ([ADR-294](adr/adr-294.md) D18). Building `std` needs only `rustc`; the compiler is not in a project's build graph. `nikaia lower-std` re-lowers it by invoking the compiler binary.
* **`std` is compiled once per machine, not once per project.** The compiled `std` lives in the user's cache directory, in an entry keyed by the compiler's own fingerprint, the toolchain, the `target` and the codegen flags of 13.3's `[build.<target>]` table. Two builds that differ in any of those keep their own entry. `NIKAIA_CACHE_DIR` moves the cache. `CARGO_TARGET_DIR` disables it.
* **The newest three entries are kept; idle entries below that are removed.** Each entry is a whole Cargo target directory. A tree written to within the hour is never removed.
* **`user-parallelism` is not one of those keys.** The build option reaches `std` as a value its runtime is started with, never as a compile-time condition. One compiled `std` serves both values.
* **Nothing in `std`'s Nikaia half may lower differently per build option.** The Nikaia half is lowered at one value of every build option. `Shared` is ruled out of it, since the emission of a `Shared`'s count follows `user-parallelism`. `Locked` is ruled out likewise (Part II, 12.2). The toolchain lowers every module at both values; the bytes must agree, and a difference fails the toolchain's own build.
* **`std`'s ledger travels inside the compiler.** `std.contracts` (13.5) is part of the compiler, not of the sysroot copy it is read from.

### 13.3. Manifest Configuration (`nikaia.toml`)
The manifest defines the project's metadata and the build options of Part I 1.2.

The build options live in `[build]`. `--target` and `--user-parallelism` override `target` and `user-parallelism` for a single build. `reentrancy-check` is read from the manifest. A key `[build]` does not know fails the build.

`optimization` names optimizations that change nothing a program means, as `NAME:LEVEL` words separated by commas. `--optimization=NAME:LEVEL` overrides it for a single build, and may be given more than once; a later word for a name wins. The first name is `remove-bounds-checks` ([ADR-306](adr/adr-306.md)): at `basic`, an index is written without its check inside a loop over the list's own length whose body cannot change that length; at `aggressive`, also wherever the solver proves `0 <= i < xs.len()` from the facts that hold there, with a certificate its checker accepts. The other is `remove-overflow-checks` ([ADR-306](adr/adr-306.md)), whose one level besides `off` is `aggressive`: a `+`, `-` or `*` of one whole-number type is written without its overflow check where the solver proves it stays inside that type. Both read what every write into a local list puts in it as a bound on its values, `x % n` as below `n`, and the length a loop that pushes once per turn leaves. The default is `off`. No level drops a check that is not proved, so a program stops at an index or an overflow at every level where it stops at `off` (A.2). It is part of the build's cache key; `std` is lowered at `off`.

The manifest carries what the **compiler** must know. How the program behaves on the machine it runs on — the number of I/O workers, the size of the pool for user code, the I/O mechanism, the shutdown drain — is runtime configuration (13.3b), read at startup by whoever runs the program.

`reentrancy-check` is a build option. It changes what the compiler emits and is a cache-key dimension like `target` and `user-parallelism`. `refuted-claims` is one too ([ADR-269](adr/adr-269.md) D8): at `error`, the default, a claim the compiler shows false with values (`NK1207`, 14.2) refuses the program; at `warn` the program builds, each such claim a warning and a check where it is reached. `--refuted-claims` overrides it for a single build. It is the bypass for the day a better prover refutes what an older one built. `cleanup-deadline` is runtime configuration. It changes what a running program waits for; the compiler does not read it.

```toml
[package]
name = "hyper-core"
version = "0.1.0"
authors = ["dev@nikaia.org"]

[build]
# The machine the program is built for. `wasm32-unknown` has no threads and
# traps rather than unwinding; the target decides the panic strategy and what
# `std` offers.
target = "x86_64-linux"

# What is made: a program (the default), or a library other languages call.
# `artifact = "c-library"` exports every `pub extern "C" fn` with a body and
# generates the header. With `target`, it decides whether foreign code may
# call in.
# artifact = "program"
# The prefix of every exported symbol and constant. The default is the package
# name with `-` written `_`.
# symbol-prefix = "hc"

# Whether user code may run concurrently. A permission, not a count; how many
# threads serve a "yes" is the runtime's decision.
#   "no"  (default) - no two pieces of user code are ever in flight at once
#   "yes"           - user code may run concurrently
# The option bounds user code, not the compiler or the runtime.
user-parallelism = "no"

# Optimizations that change nothing a program means (ADR-306). "off" by
# default; "basic" or "aggressive" drop an index's or an overflow's check where
# it is proved.
# optimization = "remove-bounds-checks:aggressive,remove-overflow-checks:aggressive"

# `cleanup-deadline` is runtime configuration (13.3b). A manifest that carries
# the key compiles, with a note naming where it went.

# A manifest that carries `ordering` fails the build, with a message naming
# `overlap { … }` (Part I 8.1.2).

# Whether the compiled program notices a lock taken while a lock is held.
#   "on" (default) - the check is emitted, and a violation panics where it happens
#   "off"          - the check is not emitted
# Taking a lock inside a lock is refused at build time (Part II 12.3). Every
# program the refusal accepts behaves the same at both values. The option
# decides only whether a violation the refusal missed is noticed.
reentrancy-check = "on"

[dependencies]
# A Nikaia package by path. The key is the name a `use` writes; the path,
# relative to this file, says where the package comes from. The package is
# built as a Rust crate of its own, and its files are read for its ledger:
# `pub` is what it offers, its own `[build]` is ignored with a note (a package
# is built with the options of the program that uses it), and the overflow
# checks of A.2 reach it.
http = { path = "../http" }
# A Nikaia package by version resolves through Cargo: on crates.io it is the
# crate `nikaia_http_server`, and the generated `Cargo.toml` writes the rename,
# so the prefix never reaches a `.nika` file. `"1.2"` is Cargo's semver; a
# `git` table with a `tag` is the same arm without an index. The key is an
# identifier, because it is the `use` name. *Decided, not built yet:* a bare
# version is refused.
http_server = "1.2"
# A native Rust crate. The entry reaches Cargo with only `type` removed and a
# relative `path` made absolute, and Cargo resolves, fetches and links it.
regex = { type = "rust", version = "1.5" }

# Code generation, per target. These keys decide output size and speed and
# change nothing a program means. The table of the chosen target becomes the
# generated `Cargo.toml`'s profile. The panic strategy follows from `target`,
# and a build never has debug assertions (ADR-002 D5).
[build.wasm32-unknown]
opt-level = "z"     # Optimize for binary size

[build.x86_64-linux]
opt-level = 3       # Maximize throughput
lto = true          # Link Time Optimization
incremental = false # Compile as one piece: slower rebuilds, faster program.
                    # Absent, it is on (ADR-002 D5).
```

### 13.3b. Runtime Configuration (`nikaia-runtime.toml`)
The runtime configuration is set by whoever runs the program and is read when the program starts. It has four keys:

```toml
# The number of I/O threads the runtime runs. One always runs. It runs `std`'s
# own code and never user code, so it exists at `user-parallelism = "no"` too.
# More than one lets a pair of operations overlap on a machine with no
# completion queue.
io-workers = 1

# The size of the pool for user code at `user-parallelism = "yes"`. "0" means
# as many threads as the machine has. The key is read at both values of the
# build option and used at one; a key that is present is never dropped
# silently.
user-pool = 0

# The mechanism that serves a file.
#   "auto"     (default) - the kernel completes the operation where the machine
#                          can, and the blocking path serves it where it
#                          cannot. Decided when the program starts, never when
#                          it was compiled.
#   "uring"              - pinned to completion. A machine without it refuses
#                          to start; there is no silent fallback.
#   "blocking"           - pinned to the blocking path, whatever the machine has.
io-method = "auto"

# How long the runtime waits at program end for pending resource cleanups
# (flushes, rollbacks, connection shutdowns; Part I 6.4). The default is "30s".
# On expiry the remaining cleanups are cancelled (their synchronous fallback
# runs), and the program ends with exit status 70 and a message naming every
# resource that did not finish cleanly, on stderr and through the panic hook,
# never on stdout and never as a 0. "0" disables draining. The deadline always
# ends the wait: the timer runs in the runtime, and cancelling a cleanup always
# terminates.
cleanup-deadline = "30s"
```

The file has these four keys and no other. Any other key fails, as under `[build]`. *Decided, not built yet:* two keys for the `http` package's file bodies (17.1), `send-file` (`auto` by default, pinnable to `never` or to a size) and `static-cache` (`0` disables it) ([ADR-289](adr/adr-289.md) D24, D29). The file is read from the working directory. `NIKAIA_RUNTIME_CONFIG` names a file outright. Without a file, the defaults above apply.

A build option is not runtime configuration: `target` and `user-parallelism` change what the program means and stay in `nikaia.toml`.

### 13.4. Build Scripts (`build.nika`)
A project that requires custom build steps, such as compiling C code or generating protocol files, places a `build.nika` file in its root. The script is compiled and executed **before** the main build.

The script has access to the `std::build` API, which emits instructions to the compiler.

```nika
// build.nika
use std::build

fn main() {
    // Compile a local C library
    build::cc("src/native/mylib.c")
    
    // Link against a system library
    build::rustc_link_lib("z") // links libz
}
```

### 13.5. The Borrow Contract Ledger (`nikaia.contracts`)

Nikaia source contains no lifetime annotations (Part I, 6.5). The compiler infers a **borrow contract** for every function whose signature involves references, such as *the result of `longest(a, b)` borrows from `a` or `b`*, by analysing all `.nika` sources of the package as one graph. The result is written to a generated, human-readable file in the project root, **`nikaia.contracts`**: the ledger.

**File format (illustrative):**

```toml
# AUTO-GENERATED by `nikaia build`. Commit this file like a lockfile.
# Do not edit by hand — it is regenerated on every build.
version = 3

[fn."text::longest"]
pub = true
sync = "inferred"
signature = "(a: ref String, b: ref String) -> ref(a | b) String"

[fn."http::Request::header"]
pub = true
signature = "(ref self, name: ref String) -> ref(self) String?"

[type."parser::Token"]
pub = true
fields = ["text: ref String", "at: i64"]
tethered = ["text"]
```

**What a contract was derived from is a file of its own**
([ADR-251](adr/adr-251.md) D1): `nikaia.derived`, beside it, generated and
committed with it.

```toml
# AUTO-GENERATED by `nikaia`, beside the contract it describes.
version = 3
toolchain = "nikaia 0.1.0"
inference = "stage0-signatures+sync-bodies+throws-bodies+sharing-bodies"

[sources]
"text.nika" = "9f3a…"
"main.nika" = "41c0…"
```

The contract says what a caller may rely on; the record says which compiler
wrote it, by which inference, from which sources. A compiler upgrade or an
edited comment in a source changes the record, and a diff of the contract is a
contract that moved. A description written by `nikaia describe` has the same
pair, `contracts/<crate>.contracts` and `contracts/<crate>.derived`.

**Build semantics.** On every build the compiler infers fresh contracts and diffs them against the ledger:

1. **Unchanged:** the fast path. Callers of unchanged contracts are not re-checked; the record beside the ledger is the incremental-compilation cache key. The lock is consulted before any work and decides whether a build starts at all. The ledger is compared after inference and decides which callers are re-checked. The two are separate files: the ledger ships with a published package and a lockfile does not. `--locked` means regenerate-and-compare for the ledger and do-not-re-resolve for the lock.
2. **Changed, all callers still valid:** the ledger is updated and the build proceeds. The change is noted in the build output.
3. **Changed, and a caller breaks:** the compiler uses the diff to narrate the cause chain:

```text
error[NK2401]: a change in `longest` broke its caller `report`
  --> text.nika:8 (the change)
   |
 8 |     return b.trim()
   |            ^^^^^^^^ the result of `longest` now borrows from `b`
   |                     (previously: only from `a`)
  --> main.nika:14 (the breakage)
   |
14 |     let best = longest(title, subtitle)
15 |     drop(subtitle)
16 |     println(best)
   |             ^^^^ `best` may point into `subtitle`, which is already gone
   |
  help: either keep `subtitle` alive until after line 16,
        or return an owned copy from `longest`: `return b.trim().clone()`
  note: contract change recorded in nikaia.contracts (line 12) — if the
        change was unintentional, this diff shows exactly what to revert.
```

**Trait methods.** Dynamic dispatch requires one contract per trait method. The ledger stores the join of all implementations. An implementation that broadens the contract produces a ledger diff and, where callers break, the same narrated error.

**What the ledger records.** The ledger records what a caller must know about a body it cannot see: borrows, whether a function may pause, and whether it may fail.

| key | on | meaning |
| :--- | :--- | :--- |
| `pub` | fn, type | reachable from outside the unit that declares it |
| `sync` | fn | Part II 12.1: it never pauses; what it reaches is answered by `touches`. `true` where the source asserted it, `"inferred"` where the body implies it, `"sync(f)"` where the lambda it is given decides - the source's own `sync(f)` ([ADR-288](adr/adr-288.md) D31) |
| `throws` | fn | Part I 7.1: it may fail, and **with what**: `throws = ["ConfigError", "IoError"]`, the set inferred over the call graph. The set names error **types** and never one of their variants. A member written `"?"` is *something this compiler cannot name* — an unresolved call, or a call into code no ledger describes. **`std`'s own entries all name what they throw**: `io::IoError` for the seven that read, write or check text, `Overtaken` for the lock's two doors. Where the set has exactly one member and the member is a type **any ledger describes** — the unit's own, or `std`'s — the **failure channel is that type**; where it has two or more, the channel is a **generated sum** over them. Either way a `catch` matches on the members' variants. A set with a `"?"` in it travels in one opaque error |
| `returns` | fn | what the result may point into, `returns = "ref(a \| b)"`, **only where the result has no `ref` to write it on** (a `Seq[char]` read out of a text). Everywhere else it is in the signature: `-> ref(a \| b) String` |
| `keeps` | fn | the parameters written with a plain type that the body **keeps** past the call - stores, returns, hands to a task or to a callee that keeps it ([ADR-094](adr/adr-094.md) D2): `keeps = ["value"]`. A caller hands such an argument over; every other one is lent. **Absent on a present entry means it keeps nothing** |
| `mutates` | fn | the call changes the value it is made on. Written in the signature, as the receiver `ref mut self`; the key stands only on an entry with no receiver to write it on |
| `signature` | fn | its parameters, its **options** and its result, as the source writes them: `"(path: ?, data: ?; append: bool = false, create: bool = true)"`. An option carries its default (Part I, 5.1). **It is spelled as Nikaia spells a declaration** ([ADR-251](adr/adr-251.md) D4). A method's receiver is the first parameter, written `self`, `ref self` or `ref mut self`, and `self: ref T` (or `mut self: ref T`) where its type is not the entry's own or carries arguments. **Where the result points** is written at each `ref` of the result, `-> ref(self) String`, `-> Seq[ref(self) T]`. **A type variable is declared in brackets before the parameters**, as `fn hand[T](x: T)` declares one, and a caller binds it from what it passes and reads the result off the same signature: `hand` records `"[T](x: T) -> T"`, and a call that passes an `i64` gets one back. `Self` is `?`. **A bound stands where the declaration writes it**, `"[H: Handler](h: H) -> String"`, and the trait is spelled the way the *declaring* package writes it. Shared mutable state is written `SharedMut[T]` |
| `fields` | type | every field of a `struct` with its type, `pub` where another package may read it: `["pub name: ref String", "temp: i32"]` |
| `variants` | type | every case of an `enum` and what it holds: `["Quit", "Write(String)", "Move { x: i32, y: i32 }"]`, so a `match` in another package is checked for completeness |
| `tethered` | type | the fields that hold a view, directly or through another type that does |
| `compares` | type | two values of it can be compared with `==` (`true`), for a type whose parts are the library's; a Nikaia type compares where its fields or variants do. **Absent means it does not** (`NK1188`) |
| `iterates` | type | a `for` over a value of it can **fail**, so the function holding the loop declares `throws` ([ADR-025](adr/adr-025.md) D6) |
| `ends_by_length` | fn | a sequence entry's result can be walked from the back only where every input's length is known (below) |
| `threads` | fn | a described foreign call **may start threads of its own** (`true`), **may not** (`false`), or nobody said (absent); `NK2502` fires on the claim, never on its absence ([ADR-290](adr/adr-290.md) D6) |
| `provenance` | fn | the function is a **source**: its result is bytes that entered the program from outside, and this names who chose them ([ADR-010](adr/adr-010.md) D2). Absent means *not a source*, which is what almost every function is |
| `trait."…"` | table | a trait, with its methods as ordinary `fn` entries (signature, `sync`, `throws`) and no `fields`; a bound and an `impl … for` must name one. The table carries the name and nothing else. The methods are the `fn` entries beside it, under `Handler::handle`, the key shape `NK1130` compares against. |
| `impl."A for T"` | table | an `impl`, in the ledger of the package that wrote it. Whether `T` implements `A` is the union over every ledger a program reads plus its own; no ledger claims completeness. A consumer reads it under both spellings, the package's and the qualified one. An extra spelling can only make the check fail **open** (C.4); it never removes `NK1164`. |
| `crosses` | type | a value of this type **may cross a thread** (`true`), **may not** (`false`, which the crossing refusals `NK25xx` fire on), or nothing was recorded (absent). Written by `nikaia describe` from a foreign type's fields — `false` for an `Rc` or a raw pointer, `true` where every field is plainly sendable, nothing where it cannot tell — or by hand, and never inferred. It answers only for a type whose parts this compiler cannot walk; a Nikaia `struct` records its `fields` and the check walks those. Absence means *nobody said*, which is not permission: the compiler does not put such a value on a thread of its own choosing |
| `touches` | fn, type | which resources it reaches and whether it reads or writes them: `["file(path) write", "stdout write"]`. **Absent means it touches everything**, so a function nobody has described orders against everything and stays where it was written |
| `locks` | fn | whether the body may **acquire a lock** anywhere it reaches; this decides whether a call may appear inside an open lock. Propagated over the same call graph as `sync`. `locks = "?"` is *this compiler could not tell*, which reads as `true`. **An absent entry means it touches a lock**, the one inverted key in this file (below). The key says *a lock*, never which lock, so it never answers an ordering question; that is `touches`'s |
| `sharing` | fn | which of its `Shared` positions are one allocation, and which reference count each of those classes gets: `["counts \| <result>: plain", "hits: atomic"]`. `counts` and the result are **the same allocation**, so they have the same count whichever side of the call decides it. The count is `atomic` unless the compiler proved that nothing crosses a thread with the class; `word` is the atomic count where the class is a `SharedMut` of a word-sized value whose crossing was proved and no `access_all`/`update_all` holds it, and its lock is a compare-and-swap (ADR-281). **Absent means the function has no `Shared` position** |
| `views` | fn | which of Part I 6.6's states each view in its signature solved to: `["data: borrowed", "<result>: tethered"]`. A view in a parameter **borrows**. A result borrows where a receiver or a parameter carries a view, and **tethers** where the buffer is one the body made. `Owned` never appears. **Absent means the signature holds no view.** `--tethers` (on `nikaia build` and `nikaia lower`) prints what it solved |

`sharing` and `views` are the function keys whose content cannot make a program wrong. The other function keys are read as permissions: a caller that trusts a wrong `sync` puts a pausing body inside a lock, and one that trusts a wrong `locks` puts a second lock inside the first. `sharing` records an **optimisation** on a floor that is already safe: the worst a class can be given is the atomic count. A ledger that says nothing, or says `atomic` about everything, still describes a correct program. The classes compose across a call: a build that can read a dependency's sources continues the analysis through its published functions, and the two reference counts never become two types.

`signature` and `fields` make a type check possible across a boundary whose bodies are not visible; the `NK1xxx` diagnostics of C.3 are answered from them. A `?` in either is **the absence of a claim**. A checker reports a mismatch only where both sides are written down, so a contract that says less makes the compiler quieter and never wronger.

Only what is true is written. **An absent `sync` on an entry means not `sync`.** An absent *entry* means nothing is known, and a caller may not assume.

`locks` is the one key whose absence is the restrictive answer written the other way round. **An absent `locks` means the function touches a lock.** A function that reaches no lock says `locks = false`.

**The contract carries no prose** ([ADR-251](adr/adr-251.md) D2). A `///` in front of a declaration is the source's (Part I), and the compiler does not copy it into the ledger. `std.contracts`, which is written by hand, keeps its sentences as `#` comments above the entries they are about.

**`sync` is written in two forms.** `sync = true` is a promise the source made; a body that contradicts it is refused with `NK2202`. `sync = "inferred"` is an observation about the body: nothing the function calls can pause, so today the function cannot pause.

**Inside a package a caller does not distinguish the two forms.** Both mean *this cannot pause*, and both satisfy the `sync` half of what `access` and `par_iter` require.

**A caller in another package reads only the promise** ([ADR-288](adr/adr-288.md) D28). Across a package boundary, `"inferred"` means *may pause*: a change inside the body may withdraw it, and nobody promised it. A `sync` function, a lock's `update` or `access`, a `par_iter` lambda or a grammar's action that calls such a function is refused, and the message names the word to write in the other package (`sync`, or `sync(f)`). Only the check reads it so. The call is lowered by what the body is, and a function that cannot pause is not awaited (D3). A caller whose only pausing call is such a function may pause itself, to its own consumers.

**A build names what its author could promise** ([ADR-288](adr/adr-288.md) D29). `nikaia build`, `run`, `test` and `lower` end with a note listing the package's own `pub` functions that never pause and do not say `sync`, and those that pause only where the lambdas they are given do and do not say `sync(f)`, each with the word to write:

```text
note: 2 pub functions never pause and do not promise it: `parse`, `total`
  help: write `sync` after the result, and callers in other packages can rely on it
```

A dependency's functions are not named in its consumer's build.

**A build warns where a `pub` function lost what it had** ([ADR-288](adr/adr-288.md) D32). Where the committed ledger says a `pub` function's `sync` is `"inferred"` and the build finds that it can now pause, the build warns (`NK2211`) at the statement that made it, naming the calls that lead there. Committing the new ledger acknowledges it, and the warning is not repeated.

A diff distinguishes the two forms: the narration of a withdrawn asserted `sync` differs from that of a lost inferred one.

`sync` is one of two conditions for a body inside a lock. A body goes inside a lock only if it cannot pause and reaches no lock of its own; the first condition is `sync`, the second is `locks`.

**`throws` is a set.** The diff says which error appeared and which `catch` stopped covering it. The narration is `NK24xx` (Appendix C), the same machinery a changed borrow contract uses.

**A function that runs a caller's lambda says so.** `xs.map fn(n) { n + 1 }` cannot pause and `xs.map fn(n) { io::read()… }` can, and both call the same `map`.

`sync = "sync(f)"` names the parameter that decides, as the source's `sync(f)` does ([ADR-288](adr/adr-288.md) D31). A caller reads it as *this call adds no pausing of its own*. The lambda's body counts as part of the function that writes the lambda.

`sync(f)` is for an immediate lambda only. A parameter the callee stores or spawns (`@detached`, Part I 5.4) may not be named by it. The rule is held by a check over `std`'s own entries.

`locks` has no `sync(f)` form. The check reads a lambda passed in a call directly from the caller's body. The premise is the same immediate lambda.

**A signature may name its receiver's type arguments.** `HashMap::entry` is written `[K, V](mut self: ref collections::HashMap[K, V], key: ?) -> Entry[V]`: the result holds whatever the map holds. At a call site the receiver's actual type binds the variables, and they are substituted away: `HashMap[ref String, Stats]` makes the result an `Entry[Stats]` and `and_modify`'s `fn(ref V)` a `fn(ref Stats)`, which gives the `s` in `fn(s) { s.add(t) }` its type.

**A sequence has a name in the ledger, and a container keeps its own.** `Seq[T]` is elements of `T` produced step by step; `keys()`, `chars()`, `io::lines()` and `xs.map fn …` hand one back, and `sync`, `pauses` or `throws` after it say what one step may do, as after a function type. The first two are **three states and not two**: `sync` promises the step does not pause, `pauses` warns that it does, and neither is *nobody said*. A `for` over a sequence that says `pauses` gives its thread up instead of holding it; one that says nothing keeps the blocking step. The eager consumers `collect`, `count`, `nth` and `join` also read `pauses`. `collect` builds a list, or the map, set or `String` that the `let`, field or result it goes into declares, where its items fit ([ADR-293](adr/adr-293.md)). A `Seq` is consumed by walking it, so a second walk is refused with `NK2702`, wherever the first one took it: a `let` that names it again, an argument, a loop or a lambda that would take it on its next turn. **Three more words say what a sequence is as a whole**: `ends` — it can be walked from the back; `sized` — its length is known before it is walked; `replays` — walking it does not use it up, which is true of a range and of nothing produced. On a receiver they are a demand (`rev` writes `Seq[T] ends`, and a sequence without the word is `NK2703`); on a result they pass through, and `ends_by_length = true` says the result has a back end only where every input's length is known. A walk of one whose step **pauses** is a `for` or one of the eager consumers; `map`, `filter`, `take`, `skip`, `step_by` and `zip` over one make another such sequence; `rev` is `NK2703`. A list's `windows` and `chunks` hand out `ref Array[T]`. A `Vec[T]` or a `HashMap[K, V]` is a container, walked by view and as often as a program likes. `Par[T]` is what `par_iter()` hands back, and a lambda handed to it must be `sync` (Part II 12.6). Neither `Seq` nor `Par` is in a program's type grammar. **`Seen[T]` is what a lock hands out**: `get` is `-> Seen[T]`, the result of `access` is a `Seen`, the stamp sticks through arithmetic and through calls to entries whose `touches` names no lock, and a `set` given one is refused. A `Seen[i64]` is emitted as an `i64`. `Seen` is in a program's type grammar at two places: a struct field, and a parameter of a function that touches a lock.

**An unbound variable becomes `?`, never a name.** A variable is bound and replaced, or it is the absence of a claim. A map built by `HashMap()` says nothing about what it holds and binds nothing, and the chain stops there.

**A variable says what flows out; `?` stays for what flows in.** A variable may appear in a result and in a lambda's parameter type, and never in an argument of a `std` entry. Binding is narrow: from the receiver, by position, one pattern; a free function's arguments bind its own written type parameters ([ADR-288](adr/adr-288.md) D19-D20).

The type a lambda parameter names is a **function type**, `fn(ref Stats)`. It says what the lambda is handed, so that the `s` in `fn(s) { s.add(t) }` has a type and the call on it resolves. A program writes the same type in its source, `fn(i64) -> i64 sync` (Part I 5.4 C).

The check on an asserted `sync` and the inference of `sync` share one resolution rule and differ in what an unresolvable call means ([ADR-288](adr/adr-288.md) D2):

* The **check** refuses what it can see. It reports the calls that will pause, and a call by name that no ledger knows (`NK2202`), because an asserted `sync` ships and nothing could check it; a method call is answered by the type checker, and one it cannot resolve is not refused.
* The **inference** is conservative in the restrictive direction. It claims `sync` only where every call resolves and every callee is `sync`; an unresolvable call costs the function its claim.

**What counts as resolvable is the type checker's answer.** A call by name, such as `helper(x)` or `io::read_to_string()`, is looked up directly. A method call needs the receiver's type. The type checker records where each method call went, and the `sync` inference reads that record.

Where the inference does not earn `sync`, a program writes `sync` by hand, and the body is checked against it.

A method with no entry is an unknown, and an unknown costs every function that calls it its inferred promise. A library that ships thin contracts makes its consumers' code unusable inside `access` and `par_iter`, however pure that code is.

> Writing a signature down is what lets a library's callers be `sync`.

**Which inference wrote it.** The record beside the ledger, `nikaia.derived`, carries `inference`, naming what the ledger was derived from. The borrow contract is the widest one the signature supports: a result that is a view may point into any view it was given. `sync`, the errors a `throws` names and `sharing` are read off the body. A ledger regenerated by a compiler that reads one more body shows a change in the record, and a change in the contract only where an answer moved; the change is narrated. The `toolchain` recorded is Nikaia's version, not `rustc`'s.

**Distribution.** A published package ships its ledger, so a downstream project builds against stable contracts and receives the same diff-based explanation when a dependency upgrade changes one. `std` ships `std.contracts`; it is the file a program's compiler reads when the program calls `io::…` or `fs::…`. A library whose implementation is partly in another language cannot have all of its contracts inferred. Those contracts are written in the ledger, marked as such, and reviewed like code. The ones that can be inferred are regenerated and checked against the sources by the library's own tests.

**A consumer reads a dependency's ledger; it never derives a dependency's contracts itself.** A package's ledger is written by the package's own build and read by every consumer, as for `std`. The inference runs over the package as one graph, so a call from one file of a package to another resolves. A call into a dependency is answered from that dependency's ledger. Only a call into code no ledger describes is unresolved, and it fails closed.

**A ledger is believed only while the sources it came from are unchanged.** The record beside it, `nikaia.derived`, holds per unit the SHA-256 of the file the entries were derived from, the digest `nikaia.lock` uses. A ledger with no record beside it is not believed. At a consumer's build a dependency whose sources hash as recorded is believed, and nothing is inferred. A dependency whose sources changed has its ledger derived again and written, and the difference is narrated. A dependency with a ledger and no sources is believed. A dependency is never believed against its own sources. The build is ordered by the dependency graph, so a ledger exists before its consumer is checked. A mismatch the backend reports at a package boundary is reported as *the ledger of that package does not match its sources*.

**Version control.** `nikaia.contracts` and `nikaia.derived` are committed. A merge conflict resolves like a lockfile conflict: accept either side and run `nikaia build` to regenerate. Where a toolchain upgrade, and not user code, changed the inference results, the build output states that the contract changes were caused by the toolchain update.

**Determinism guarantee.** The ledger and its record are a **pure function of (source tree, toolchain)**, where the source tree includes the committed `nikaia.proofs` ([ADR-270](adr/adr-270.md) D4): the solver's search may differ between machines, and a build uses only the answers that file records; the same sources and the same pinned toolchain produce a byte-identical `nikaia.contracts` on every machine, every run, with any thread count. A violation is a compiler bug. Two consequences:

* There is exactly **one** ledger per project, valid at every value of every build option. Borrow contracts and tether relationships do not depend on a build option. A check that does, such as a thread-safety rule, is performed by the compiler directly and is never recorded in the ledger.
* Ledger stability is **not** promised across toolchain upgrades. The recorded toolchain and the narration explain such a diff.

**Verification mode (`--locked`).** `nikaia build --locked` verifies instead of updating: the compiler regenerates the contracts in memory, the program's and those of every path dependency it has sources for, and compares each byte for byte against its committed `nikaia.contracts` and `nikaia.derived`. Any difference fails the build with the narrated contract diff (`NK2401` above). This is the one place contracts are compared rather than hashes; a development build compares hashes and derives only what changed. A CI build with `--locked` is equivalent to `git diff --exit-code nikaia.contracts` after a regular build. `--locked` also searches no proof: every query the build asks must have a checking entry in `nikaia.proofs`, and every entry must be asked; a difference fails the build with the query's position and *run `nikaia build` and commit `nikaia.proofs`* ([ADR-270](adr/adr-270.md) D22; decided, not built yet).

---

## Chapter 14: Testing and Quality Assurance

Testing and verification are part of the language.

### 14.1. Unit Tests (`test`)
*[ADR-269](adr/adr-269.md) D1 and D12. Built.*

A `test "name" { … }` block checks specific inputs. It stands where a `fn` may, in any file of the package, and sees what the package sees, private names included. Only `nikaia test` compiles it: `nikaia build` and `nikaia run` leave it out, unchecked.

```nika
test "a blank line is counted" {
    let (lines, blank) = count("a\n\nb")
    assert(lines == 3)
    assert(blank == 1; message: "an empty line is blank")
}
```

A test's body may pause and may fail. A test **fails** on an error that leaves its body, which is printed in its long form (the site and the chain), on a false `assert` (14.2), or on a `panic`. `nikaia test` builds the package once with its tests compiled in and runs **each test in a process of its own**, so a test that stops the program does not stop the others. It prints a line per test, the output of each failing one, and a summary, and it exits unsuccessfully where any test failed.

Each test runs under the `user_parallelism` the project names. `nikaia test --both-settings` runs each at `no` and at `yes`, and a test whose outcome differs between the two fails with both outcomes shown: a program means the same at both settings (Part I 1.2), so a difference is a fault of the compiler.

**An output test is a few files and no code** ([ADR-269](adr/adr-269.md) D13, [ADR-247](adr/adr-247.md)). In a package's `tests/` directory, `NAME.stdout` or `NAME.out/` makes `NAME` a test of the package's program. `nikaia test` builds the program as `nikaia build` does and runs it **in a fresh, empty directory of the test's own**, removed afterwards, so nothing it writes reaches the package. `NAME.in/`, where there is one, is copied into that directory first; the arguments are the lines of `NAME.args` and the input is `NAME.stdin`, each empty where the file is absent. The test passes where the program ends successfully, prints exactly `NAME.stdout` (where there is one) and leaves every file `NAME.out/` names with exactly its bytes; a file it writes that `NAME.out/` does not name is not compared. A failing one is reported as a line difference, `-` expected and `+` produced. Output tests run beside a package's `test` blocks, and at both settings under `--both-settings`.

**`nikaia test --bless`** writes the expectations from what the program did: its standard output into `NAME.stdout` where the test has one, and each file it wrote into the `NAME.out/` entry of that name. Only a run that ended successfully is blessed, and at `--both-settings` only one whose two runs agree. A new output test starts as an empty `NAME.stdout`, or the empty `NAME.out/` entries it should compare, and is blessed; the change is reviewed in version control.

**A `test` block runs in the package's directory**, so it reads what the package holds, and writes under **`fs::scratch()`**: a fresh, empty directory handed back as an `fs::Root::Dir`, inside the test's own directory and removed with it.

**`--asserts`** ([ADR-269](adr/adr-269.md) D7), beside `--overlaps`, `--sharing`, `--tethers`, `--trust` and `--comptime` on `nikaia build` and `nikaia lower`, prints every `assert` of the program by line and how it is held (14.2): proved while the program was built, a precondition its callers prove, checked at run time - with the reason the prover gave up -, or checked by a test when it runs; and how many are checked at run time. It is how a reader asks where the unproved claims are.

### 14.2. Assertions: a contract the compiler proves where it can
*[ADR-269](adr/adr-269.md) D2-D11. Built: `assert`, the MVP prover, the check at run time for a claim it does not prove, a precondition's check at the call, and `--asserts`. D8's refutation: built, for a claim ruled out every time it is reached and for one false for a value what is known shows reaches it, refused unless `refuted-claims = "warn"`.*

`assert(cond)` and `assert(cond; message: "…")` claim that `cond` holds whenever the line is reached. `assert` is a prelude function the compiler knows, not a statement: it needs no `use` and costs no reserved word, and a program that declares its own `fn assert` calls its own.

**The condition changes nothing.** It may not pause, may not fail, and may not touch or change anything: every call in it has to be `sync`, throw nothing, touch nothing (`touches = []`) and change nothing, and a call nothing describes is not known to. A condition that could is refused where it is written (`NK1194`), because a condition with an effect would make the program depend on whether the check is performed, and a condition that reads the world has no value a compiler could prove. `assert` takes one `bool` and one option, `message:`, which is text (`NK1195`).

**An `assert` is a contract: proved where the compiler can, checked where it cannot** ([ADR-269](adr/adr-269.md) D4). A claim the compiler proves while the program is built has no check emitted, so it costs nothing when the program runs. A claim it does not prove is checked where it is reached, and a false one stops the program as `panic` does (A.2). The claim means the same either way - a proof only removes its cost - and no build option chooses between the two, as none switches off an overflow check (Part I 2.2).

**An `assert` is not a guard.** A guard - `throw … if c`, `return … if c` - handles a case the program expects to meet. An `assert` says the case cannot occur, and that if it does, the program is wrong.

**What the prover reads** (ADR-269 D9). Comparisons of whole numbers built with `+`, `-` and a `*` by a constant, joined with `&&`, `||` and `!`, over bindings that do not change, and the `len()` of a list or text that does not change; it knows the branch an `if` is in, what a jump with its condition after it has ruled out (`return 0 if b == 0` leaves `b != 0`), a `for` over a range, a `let`'s value and every claim proved before. It may fail to prove a true claim and never proves a false one (D10); a claim it cannot prove is checked when the program runs. A claim that what is known before it **rules out** - false every time it is reached, proved so - is an error, `NK1207`, with values for its names from a model of what is known (D8): a program the compiler has shown wrong does not build. `refuted-claims = "warn"` (13.3) builds it, the claim a warning and a check. So is a call that breaks its callee's precondition every time it is reached. **So is a claim false for some of the values that reach it**, where what is known is exactly what reaches - no parameter, no call's result, no condition the prover cannot read, no loop that may end early stands between the values and the claim - so that a value what is known allows is one the program reaches: `for i in 0..<10 { percent(1, i) }` is told *when `i` is 0*. A claim that is only not proved is not refused: elsewhere a value what is known allows need not be one the program reaches.

**A claim about parameters is the caller's** (ADR-269 D5). An `assert` at the top of a function that names only its parameters, and that the body cannot prove, is the function's precondition: the body assumes it, a call that proves it costs nothing, and a call that does not carries the check, so that a failure names the caller. `divide` below asks each caller to show `b != 0`. [ADR-269](adr/adr-269.md) D14-D21 generalise this, within a package and across packages, and it is built: a claim is **carried back to the entry** - a `let` is substituted, a parameter changed before the claim included, a branch becomes an implication, a guard weakens it - and where it then reads only the parameters it is a precondition, `path → claim`: `let y = x - 1` and `if mode == 1 { assert(y > 0) }` make `mode == 1 → x - 1 > 0`. **A call is carried back too**: a callee's precondition the body cannot show, with the arguments in place of the parameters, is the function's own, as an `assert` of it at the call would be - so the caller that passes the value is the one told, and the calls in between need no check. A call that can come back to the function, directly or through others, is not carried back. A loop, a lambda, a `match` arm or a call's result stops it, and the claim stays a check. A computed precondition is proved at each call or checked in the body, and a call that breaks it is told the condition and the `assert` it came from. **A claim about the value returned, shown at every exit, is a postcondition**, and a caller knows it of the result: after `let c = clamp(n)`, `clamp`'s `result >= 0` holds of `c`. **The ledger publishes both** - `requires`, `ensures`, and the `assert` each came from - so a caller in another package proves the one and relies on the other; a guard is only observed and is not published. **A function with a precondition has two entries**: `f` checks it and calls the body, `f__unchecked`; a call that proves it or checks it where it stands calls the second, and every other caller - a function value, a method call, another package, the language below - reaches the first. A method of a trait's implementation has no precondition. **A contract changed in the breaking direction** - a precondition stronger, a postcondition weaker than the committed ledger's, decided by the solver - warns its author (`NK1208`) until the ledger is committed.

**A claim about data from outside the program is refused** (`NK1202`, ADR-269 D6): it is not a defect when such data is wrong, so it is checked where it arrives, with a guard the program handles (ADR-010); after the guard, the claim is proved.

**In a test, a claim is the test's verdict** and is checked when the test runs. A false one stops the test, as `panic` does (A.2), and says what only the compiler knew: the line, the message, the claim as written, and the value of each operand of its comparison - a literal's value is already on the line, and an operand whose type has no printed form is left out.

```nika
fn divide(a: i64, b: i64) -> i64 sync {
    assert(b != 0; message: "division by zero")
    return a / b
}
```

```text
src/math.nika:2: the program stopped: assertion failed: division by zero
    assert(b != 0; message: "division by zero")
    b is 0
```

### 14.3. Property-Based Testing (Fuzzing)
Fuzzing generates random data to find crashes. The test runner does this for a test that declares parameters.

**Automatic data generation**
A test that declares parameters is given generated inputs.
* **Primitives:** random integers, strings, bools.
* **Structs:** data is generated recursively for every field.

```nika
struct User { name: String, age: i32 }

// Nikaia automatically creates random 'User' structs here
test "User Validation" (u: User) {
    assert(u.age >= 0) // Might fail if fuzzer generates -1
}
```

**Custom generators (`impl Generator`)**
Where random data does not fit the type, such as a field that must hold a valid email address, the type implements the `Generator` trait.

```nika
impl Generator for User {
    // 1. How to generate standard random samples
    fn arbitrary() -> User {
        User { 
            name: String::random_alphanumeric(10), 
            age: i32::random_range(0, 100) 
        }
    }

    // 2. Defining "Edge Cases" (Values likely to break things)
    // The fuzzer will ALWAYS try these values first.
    fn edge_cases() -> Vec[User] {
        [
            User { name: "", age: 0 },         // Empty/Zero
            User { name: "A".repeat(1000), age: -1 } // Overflow/Negative
        ]
    }
}
```

### 14.4. Benchmarking (`bench`)
A `bench` block measures the speed of user code.

**Regression detection**
`nikaia bench`:
1.  executes the block thousands of times;
2.  calculates the average time and the standard deviation;
3.  compares the result against the last recorded run.

Where the new version is more than 5% slower, the CLI prints a warning:
> **Performance regression:** 'Sorting' is 12% slower than commit 8f3a2c.

**Result storage**
Results are stored in `.nikaia/benchmarks.json`. The file records:
* the timestamp
* the git commit hash
* the function name
* nanoseconds per operation

```nika
bench "Sorting" {
    let mut list = [5, 2, 9, 1, 6]
    list.sort()
}
```

**Viewing history**
`nikaia bench --history` shows the recorded history.

---

## Chapter 15: Interoperability (FFI)

A Nikaia program calls C and Rust, and a Nikaia library is called from C and from every language that speaks C.

### 15.1. C Interoperability
A call into C is written inside an `unsafe` block.

```nika
extern "C" {
    fn strlen(s: ref Array[u8]) -> usize
}

fn main() {
    let text = "hello\0"
    let n = unsafe { strlen(text) }
    println(f"{n}")
}
```

A C string ends at a zero byte, and the caller puts it there.

`extern` and `unsafe` are reserved words with constructs. An `extern "C"` block lowers to Rust's own, and the call lowers to Rust's own `unsafe`. A call to an `extern` name outside an `unsafe` block is refused with `NK1143`. A declaration in an `extern "C"` block is `sync` and carries no `throws` without writing either. Its `touches` and `locks` are fail-closed, as a described crate's are.

**What the boundary lends is a view.** `ref T`, `ref mut T`, `ref Array[T]` and `ref mut Array[T]` in an `extern` declaration are the pointer C wants, and each lives **for the call**. Nothing is stored and nothing escapes. A `ref Array[T]` takes whatever lends a run of elements — a `Vec[T]`, an `Array[T, N]`, and text where the element is `u8` — and the call makes the address.

**A length beside a view is checked at the call.** A `ref Array[T]` followed by a `usize` is one fact: the pointer says where and the count says how far. A call whose count cannot be shown to fit the buffer is refused with `NK1159`. Two shapes are accepted: the buffer's own `len()`, and a constant a known length covers, which is an `Array[T, N]`. The *type* `usize`, not the parameter's name, marks the length. A `usize` in a declaration names C's `size_t`; a caller passes an `i64`, and the conversion is emitted.

**`ref mut` is the boundary's and nowhere else's.** Away from it, a parameter the callee may change is written `mut name: T`, and a `ref mut` outside an `extern "C"` declaration is refused with `NK1158`. A `ref Array[T]` is a view of a run everywhere; only in a declaration does it lower to an address beside a count.

**A handle a library hands out is `opaque`:**

```nika
extern "C" {
    opaque type FILE released by fclose
    fn fopen(path: ref Array[u8], mode: ref Array[u8]) -> FILE
    fn fclose(f: FILE) -> i32
}
```

An opaque type is an address the language **never dereferences**. It is moved and stored like any value. It has no fields, no indexing and no constructor; a field access, an index or a call of it is refused with `NK1160`. Its release is a `cleanup` the compiler runs at the end of its scope (Part I 6.4), so a handle cannot be forgotten. A C function that keeps the address past its own call is not checked.

A handle is **lent** to every declaration but its release: `fileno(f)` reads it and `f` is still the caller's to close, while `fclose(f)` takes it and *is* the cleanup. None of `opaque`, `type`, `released` and `by` is a reserved word; each means something only in this position and is a name everywhere else.

**A handle that may be absent is a `T?`**, as in Part I 2.3: `??` and `?.` get past it. A handle holds a **non-null** address and `T?` is the absence of one, so a nullable handle is one machine word, and `ref mut sqlite3?` is `sqlite3 **` as C writes it. This makes an **out-parameter** work:

```nika
extern "C" {
    opaque type Block released by free
    fn posix_memalign(out: ref mut Block?, alignment: usize, size: usize) -> i32
    fn free(b: Block)
}

fn main() {
    let mut room: Block? = null
    let rc = unsafe { posix_memalign(room, 64, 128) }
    println(f"{rc}")
}
```

At this boundary a `?` on a `ref mut T` or a `ref Array[T]` belongs to **what it points at** and not to the view: `ref mut sqlite3?` is a slot that holds a handle or nothing. A plain `ref T?` is 2.3's own nullable view.

A declaration that does **not** say `?` is a **claim**. Where C hands back nothing, the program aborts with a message naming the declaration ([Appendix A.2](#appendix-a-error-hierarchy)). A `T?` reaches the `T` a parameter wants through `?? throw`: `slot ?? throw Refused::NoDatabase` is a `sqlite3`.

**Text a C library hands back is a `CStr`, and `std` copies it:**

```nika
use std::foreign

extern "C" {
    fn getenv(name: ref Array[u8]) -> foreign::CStr?
}

fn main() throws {
    let home = unsafe { getenv("HOME\0") }
    println(home?.to_string() ?? "none")
}
```

A `CStr` is an address the caller does not own, whose lifetime is the library's, and which ends at a zero byte. It is an opaque handle with **no** cleanup. `to_string` copies it into a `String`; the walk to the zero byte is written once, in `std`. `to_string` **fails** where the bytes are not UTF-8. A declaration says `-> CStr?` where the library may hand back no text, and the program writes `??`.

**There is no raw pointer, and `Pointer[u8]` is not a type.** `Pointer[u8]` is refused with `NK1135`. Memory the language indexes arrives with a length it knows. A program that needs a buffer makes one in Nikaia and lends it.

**A build that lets C call in is a target.** An exported entry point may be called twice at once from threads the caller owns; `user_parallelism` does not bound those. Such a build is one artifact, safe at its boundary: the entry points and everything they reach take the safe shape, and the rest of the library keeps the per-value answer.

**Providing a library to C, and to everything that speaks it.**
A `pub extern "C" fn greet(name: ref String) -> String { … }` with a body is an **entry point**. A build
with `artifact = "c-library"` in `[build]` makes a shared and a static library of the package and
generates `<package>.h` from the ledger. The caller owns the memory, and a call never surprises.

* every entry point returns a status (`0` is `<PACKAGE>_OK`, the error variants are numbered per
  library, seven negative codes are the boundary's own) and a value travels in an out-parameter;
  `<package>_last_error` renders the failure with its site and its `secondary` list, per thread;
* a text or byte result is written into the **caller's buffer** (`out, cap, written`; `NULL` asks
  the size; too small is a status with the size that would do), and the caller may supply the
  **allocator** for everything the library holds across calls, before `init`;
* an exported `struct` is an opaque handle with `_new`, `_free`, a getter per `pub` field and its
  `extern` methods; text and bytes go in as pointer and length; `T?` is `NULL` or a status;
* a function that may pause is exported **blocking** and **`_async`** with a callback on a library
  thread; a `sync` one only blocking;
* a panic is caught at the boundary, returns a status, and poisons the library until
  `shutdown` and `init` have run; a handle carries a lock, and a re-entrant call on it from a
  callback is a status rather than a deadlock;
* everything that enters is `Untrusted`, the header carries the ledger's hash, and `--locked`
  refuses an ABI change nobody committed;
* a **`pub extern "C" struct`** has C's layout and crosses **by value** — numbers, `bool`, `char`,
  payload-free enums, other such structs and an `Array[T, N]` of any of them as fields, every field `pub`, anything else `NK1145`
  with the handle named as the shape; a `Vec` of them is an array in and the caller's buffer out;
* the symbol prefix is the package's name, or the one `symbol-prefix = "…"` the build sets; no
  declaration renames itself;
* the `_async` form takes a **ticket** (`<package>_op**`, or `NULL`) that `<package>_cancel` cancels
  at the task's next pause point, with `cleanup` run and `done` called exactly once, `E_CANCELLED`
  if the cancellation came first; **many results** are a function taking `fn(item) -> bool sync`,
  whose `false` stops the stream;
* on `target = "wasm32-unknown"` the same declarations make a `.wasm` with a generated `.js` and
  `.d.ts`: the host takes its buffers from `<package>_alloc`, there is no blocking form and no
  `set_allocator`, a pausing entry point is a Promise, and there is **no `extern "wasm"`** — the
  convention is `"C"` on every target;
* `nikaia bind python` writes a `ctypes` binding from the ledger — exceptions for statuses,
  `str`/`bytes` for buffers, classes for handles, generators for streams, awaitables for `_async` —
  and `nikaia bind js` is the WebAssembly build's `.js`; there is no second artifact.

### 15.2. Rust Integration (Deep Integration)
The compiler verifies safety properties at the boundary with a Rust crate.

**A value handed to a Rust function may reach a thread that function owns.** A call whose body the compiler cannot see may put what it is given on a thread of its own. A value may cross into a foreign thread only if it may cross any thread. The crossing the compiler can decide about is refused with `NK2502` (C.5), at both values of `user_parallelism`.

The rule reaches as far as the Rust signature is true. Where the compiler reads the signature, a value is crossable by declaration. A narrowing shim is reviewed like the boundary it is.

**A call into foreign code is judged by what its arguments can reach.** Where no lock is reachable from the arguments, transitively and through the fields of a struct, the call is allowed. Otherwise the call is refused with `NK2503` (C.3, worked through in C.6); the way out is to keep the lock out of the call's reach and hand over a copy of what it needs. A lambda among the arguments is its captures, so whether it touches a lock is read off its body (Part II, 12.3). A foreign function a ledger describes is not subject to this rule.

**"No lock reachable" is an answer, not the absence of one.** Where nothing written down says what a value contains, the question is undecided: C.5's third answer, which is not permission and is handed on. A ledger entry with an empty field list, which is what a type whose fields are Rust has, means *nothing recorded* and not *nothing inside* (13.5).

**Mapping Types**
* Rust `i32` -> Nikaia `i32`
* Rust `i64`, `u8` -> Nikaia `i64`, `u8` — the rest of the numeric surface (Part I, 2.2)
* Rust `&str` and Rust `String` -> Nikaia `String`, whose state the compiler
  picks: a Nikaia `String` crosses to Rust `&str`
  for free, and to Rust `String` only by a `.clone()` the program writes
* Rust `Option<T>` -> Nikaia `T?` (Nullable)
* Rust `Vec<T>` -> Nikaia `Vec[T]`, and `HashMap<K, V>` -> `HashMap[K, V]`
* Rust `Rc<T>` **or** `Arc<T>` -> Nikaia `Shared[T]`. The compiler decides per
  value which it becomes. A `Shared[T]` handed to a call whose body the compiler
  cannot see is refused; a lock is refused by the same rule. The way across is
  what is inside: a view or a copy.

**A crate is described before it is called.** A call into a crate the manifest
declares with `type = "rust"` that no `contracts/<crate>.contracts` describes is
refused with `NK2504`, once per crate, with the command in the message. A written
type from such a crate counts as a call; a name the build did not declare is left
alone (C.4). `nikaia describe <crate>` reads the crate's `pub` signatures, from
rustdoc-JSON where the toolchain offers it and from the sources where it does
not, and writes a draft entry for every function the program calls and the types
those signatures name, translated by the table above. The draft names what it
could not answer. A version dependency is refused with its reason. The draft is
committed as `contracts/<crate>.contracts` and reviewed like code. It is believed
while the crate's version and source hash hold. A file that hashes differently is
`NK2505`, and the way out is the same command and another review. What a
signature cannot say (`touches`, `locks`) is written fail-closed, and what
neither reader can read is written `?`. The description's entries are read: the
signature types the call and what it hands back, `throws` makes it a place that
can fail, and `sync` makes it one a `sync` function may not make. A described
crate is a package, and nothing about it is imported. Where the backend
disagrees at a call into a described crate (*not a future*, a `?` on what is
not a `Result`, a `Result` where none was expected), the build says *the
description of `X` does not match the crate*, keeps the backend's words as a
note, and the way out is the same command and another review
([ADR-290](adr/adr-290.md) D21).

**Thread Safety (Send/Sync)**
Whether a value may cross into foreign code is decided from the Nikaia type of
the argument. No crate metadata is read.

* A type the compiler knows may cross is allowed in a `spawn` task and in a
  foreign call.
* A type it knows may not, such as a `Shared[T]`, is refused with `NK2502`
  (C.5); where what it may not cross with is a **lock**, the refusal is about
  the call and the code is `NK2503` (C.6). Either diagnostic names the Nikaia
  type.
* Every other type is C.5's third answer, undecided.

```toml
# nikaia.toml: a Rust crate
[dependencies]
image = { type = "rust", version = "0.24" }
```

```nika
// In code
use image

fn process() {
    // This is safe because the 'image' crate implements proper locking
    let img = image::open("test.png")
}
```

### 15.3. WebAssembly (WASM) Synergy
WebAssembly runs in a single-threaded host with a linear memory. `user_parallelism = no` matches that host.

**Zero overhead.** `nikaia build --target=wasm32-unknown` produces compact
binaries. The runtime a `no` build starts is the I/O worker and nothing else,
and no OS-level mutex is generated. At `user_parallelism = no` every
`Shared[T]`'s owner count is the cheap count. The verdict on a crossing does not
move with the build option: a call whose body the compiler cannot see is refused
a lock and a `Shared` at both values.

**A library for the web** is the same build with `artifact = "c-library"`
(15.1): the `pub extern "C" fn` declarations become the module's exports, a
generated `.js` and `.d.ts` stand where the header would, the host obtains its
buffers from `<package>_alloc`, and a function that may pause is a Promise
driven from the host's event loop. There is no `extern "wasm"`.

**JavaScript Interoperability (`dsl js`)**
Nikaia does not map the DOM to Nikaia structs. A program embeds JavaScript with the `dsl` keyword (Part II, 10.5).

```nika
// main.nika
fn main() {
    let message = "Hello from Nikaia!"

    // The 'js' grammar parses the code. ':msg' is a parameter hole -
    // a deferred parameter, not string interpolation, so the value cannot
    // be spliced into the source text and change its meaning.
    let script = dsl js {
        document.querySelector("#submit").addEventListener("click", () => {
            window.alert(:msg);
        });
    } eod

    // Subject: none ; Config: msg
    script.exec(msg: message)
}
```

---

## Chapter 16: Hardware Instructions (via DSL)

Hardware instructions are **not** part of the Nikaia core language. They are provided by
library-defined DSLs, `dsl backend::x86`, `dsl backend::arm64` and `dsl backend::wasm`, each
of which validates its own operands.

### 16.1. Why not a core construct

The core language has no `asm` block and no register constraints. Each backend DSL defines the
operand model its hardware has and the grammar that validates it.

### 16.2. Usage

Assembly uses the standard `dsl` syntax. The assembly DSL uses **immediate capture**
(`meta::capture`, Part II 10.5): it binds variables from the current scope and injects machine
code at the call site. In an assembly block, `val` is the `val` in the enclosing scope.

```nika
use std::backend::x86

fn fast_add(val: i64, ptr: ref i64) -> i64 {
    let mut result: i64 = 0

    // The grammar parses the bindings and resolves 'val', 'ptr' and 'result'
    // from the enclosing scope.
    dsl x86 {
        // 1. Binding header - syntax defined by the x86 grammar
        $v = in(reg) val
        $p = in(mem) ptr
        $r = out(reg) result

        // 2. Instructions
        mov $r, $v
        add $r, $p
    } eod

    return result
}
```

The constraint vocabulary (`reg`, `freg`, `mem`, `imm`, clobber declarations) belongs to the
`x86` grammar and is documented with it. A stack-machine backend declares
a different vocabulary: `dsl wasm` has locals and a value stack, not registers.

### 16.3. Consequences

*   **Portability:** the language core makes no assumption about the target's execution model.
*   **Validation:** the DSL parser checks instruction operands at compile time and reports
    errors through the same diagnostics contract as the rest of the compiler (Appendix C).
*   **Optimization:** a backend DSL can emit target-specific or SIMD instructions without any
    change to the language.
*   **`unsafe`:** `unsafe { … }` is a construct, and the word is on Part I 2.1's list. 15.1
    writes it for a call across the C boundary.

---

## Chapter 17: The Standard Library ("Batteries Included")

The standard library consists of universal modules, with the same API on every target, and target-specific capabilities.

### 17.1. Universal Modules
These modules rely on the unified types and behave identically at every value of every build option. Their implementation differs to match the runtime model.

**`std::io` — standard input**

A stream is not a file: the surface has no `map`, no `seek`, no length, and no second read of the same
bytes. Standard input's bytes do not exist until they are read, so a program that wants views into its
input owns the buffer first.

```nika
pub fn read_to_string() -> String throws   // all of it, UTF-8 validated
pub fn read() -> Bytes throws              // all of it, as bytes
pub fn lines() -> Lines throws             // one line at a time
pub fn bytes() -> ByteStream throws        // chunks as they arrive
```

**A step of `lines` can fail, and the failure leaves the function.** Nothing marks the loop. The compiler requires the enclosing function to declare `throws`:

```nika
use std::io

fn tally() -> i64 throws {          // NK2701 without the `throws`
    let mut n = 0
    for line in io::lines() { n += 1 }
    return n
}
```

A failed read is never indistinguishable from the end of the input. Part I 6.4 refuses that at the closing brace of a block; this is the same refusal at the top of a loop.

The surface is `std::fs`'s shape minus what a stream cannot keep. A read looks blocking and is not: no `async` on the signature, no `await` at the call. A `for` over `lines()` is the same: each step may pause, nothing in the loop says so, and the thread is given up between lines. On a single-threaded runtime the event loop runs another task while the pipe is empty; with threads the read may resume on a different one. A `sync` function cannot call it (Part II, 12.1), so a `par_iter` body cannot wait on a pipe.

`lines()` yields **owned** text, where a file's lines are views into the mapping they came from
(`fs::map(path, root)` and `.lines()`, below). An iterator may hand out views into a buffer it does
not own, and never into one it does.

There is one standard input, so these are functions rather than a handle. Reading it a second
time yields nothing.

**Provenance** follows the rule files follow: standard input is **Trusted**. A request body
arriving on standard input is the uploaded-file case and is read with
`io::read_to_string(trusted: false)`.

**Writing output**

`println(text)` writes a line to standard output, `print(text)` writes without the newline, and
`eprintln` / `eprint` are the same two on standard error. They are in the prelude rather than in
a module.

The argument is an ordinary interpolated string (Part I, 2.5), so a hole is written where the
value goes and `{{` is a literal brace:

```nika
print(f"{name}: ")
println(f"{count} rows")
```

`print` is for output composed piece by piece, where a newline after every fragment would be wrong.

**`http` — a package, not a module of `std`**
An HTTP/1.1 server: `GET` and `POST`, bodies by `Content-Length`, one answer per connection. Keep-alive, chunked bodies, TLS, HTTP/2 and a client are not built. `http` is not part of `std`: it is a package reached by path, `http = { path = "../http" }`.

* **At `user_parallelism = no`:** the server runs on a single-threaded event loop.
* **At `yes`:** the server runs on a multi-threaded work-stealing executor.

```nika
use http

fn main() throws {
    // Starts a server on 127.0.0.1:8080.
    // The code looks the same, but the runtime behavior follows `user_parallelism`.
    // The handler is a trailing lambda, outside the parentheses. There is one,
    // and it looks at the path and decides.
    http::listen("127.0.0.1:8080") fn(request) {
        if request.path() == "/" {
            return http::Response::text("Hello World")
        }
        let who = request.query("name") ?? "world"
        return http::Response::text(f"Hello, {who}")
    }
}
```

**The handler and the request.** A handler is a lambda, so its arguments follow Part I 5.3: the
first and only one is the request. It returns an `http::Response`, a value with named fields built
where it is returned: `http::Response { status: 400, content_type: "text/plain", body: "id is
required" }`. `http::Response::text(body)` is a 200 in plain text, and `http::Response::not_found()`
a 404. The limits are options after the `;` with defaults, and a program names only what it
changes: `body_cap`, `head_cap`, `head_wait` and `connections` ([ADR-289](adr/adr-289.md) D13).
`examples/hello-http/` is a server built this way.

**Decided, not built yet** ([ADR-289](adr/adr-289.md) D14, D17-D19, D22-D29): what follows, to the
end of this module's description, except the last paragraph on the request's views. A server is
`http::Server()` with a handler per path, and a handler that does not need the request names
nothing:

```nika
http::Server()
    .route("/")         fn { "Hello World" }                    // names none, takes none
    .route("/hello")    fn(request) { "Hello, " + (request.query("name") ?? "world") }
    .route("/fortunes") fn(request) { render(request) }
    .listen("127.0.0.1:8080")
```

What a handler returns is what answers the request:

| returns | becomes |
| :--- | :--- |
| `String` | 200, `text/plain; charset=utf-8` |
| `html::Raw` | 200, `text/html; charset=utf-8` |
| `Bytes` | 200, `application/octet-stream` |
| `Response` | itself |
| `T throws` | the value on success; on failure **500 with a generic body**, the error logged |

A status code, a header or a body of the handler's own is a `Response`, built where it is
returned: `http::Response { status: 400, body: "id is required" }`, with headers a field like the others.

**A body need not be bytes the program allocated.** `Bytes` is the shared buffer of Part I 6.6
and `Mapped` derefs to it, so a page mapped once, outside the handler, is a body that costs a
reference count per request. The handler answering with it duplicates the handle rather than
moving it:

```nika
use std::fs
use http

fn main() throws {
    let page = fs::map("index.html", fs::Root::Dir("site"))

    http::Server()
        .route("/") fn { page }
        .listen("127.0.0.1:8080")
}
```

**A response may be a file the program never read:** `http::File("index.html", fs::Root::Dir("site"))` is a body whose
bytes go from the page cache to the socket without entering the process. Its `Content-Type`
follows the extension. Its length, and any failure to open it, are settled before the status line.
Whether the transfer is `sendfile(2)`, a mapping or an ordinary read is the library's choice at
run time. `http::File` is unavailable under TLS and under HTTP/2.

**When the request names the file, the name and its root arrive together.** A download route:

```nika
// The request chose this name; the call says which directory it may not leave.
http::File(request.query("file") ?? "", fs::Root::Dir(store))
```

Every `std` function that takes a path takes its root right after it, with no default:
`http::File` like `fs::map`, `fs::read` and `fs::write`. The root is an `fs::Root`: `Dir(store)`,
under which the joined name is resolved and compared component by component, or `Anywhere`, which
performs no check and is listed per site by `--trust` (on `nikaia build` and `nikaia lower`). A name that leaves its `Dir` is
`io::IoError::Outside`, and the handler answers 404. The call refuses; it does not rewrite. Nothing
is inferred about where the name came from; a `../../etc/shadow` is stopped at the call, before
the headers are written. `trusted: false` on `fs::map` is about the file's content and is a
different question.

What the library keeps between requests is bounded, dropped when the file's identity or
modification time changes, and sized by the operator rather than the program. A page that must be
held for certain is mapped by the program itself, outside the handler, as in the mapped-page example
above.

The request's strings are **views** into the bytes the connection read: `path()`, `header(name)`
and `query(name)` yield `ref String`, so a parameter used inside the request's scope costs nothing and
one kept past it has to be owned (Part I, 6.6). `query` and `header` return the nullable type of
Part I 3.5 rather than an empty string, and `method()` returns an enum rather than a string.

A handler does I/O, so it is not `sync`. It carries no `async` marker and no `await`; the build
option chooses the executor and nothing else.

**`std::html`**

HTML escaping, on which a template's contract rests.

```nika
pub fn escape(text: ref String) -> String        // for a text node or a quoted attribute
pub struct Raw                             // "this is already markup"
pub fn Raw::new(markup: String) -> Raw     // the audit point, and the only constructor
```

A template grammar escapes **every hole, unconditionally**. No flag at a hole turns it off, and
trusted provenance exempts nothing. The one way to say a value is already markup is to give it
the type `Raw`, so the decision is made where the value is built rather than where it is used.

`escape` handles the five characters that change what HTML means in a text node or a quoted
attribute value, `&`, `<`, `>`, `"` and `'`, and returns its input unchanged when none of them is
present. It does **not** make text safe inside `<script>`, inside CSS, in an unquoted attribute or
in a URL. A hole in one of those positions is a compile error naming the position.

**The template, and where it is compiled.** `dsl html { … } eod` is compiled where it is
written: the body is split into literal markup and holes at compile time, and the output is plain
string building. The escaping is a compile-time property.

```nika
fn row(name: ref String, shade: ref String) -> String {
    return dsl html {
        <tr class="{shade}"><td>{name}</td></tr>
    } eod
}
```

`{{` is a literal brace, the same rule an interpolated string follows. The framing whitespace —
the newline after `{` and the indentation before `} eod` — is not markup and is removed;
whitespace inside the body is kept exactly.

What may go in a hole is decided by the **type**, through the `Render` trait: a `Raw` renders
itself, text renders escaped, and a type with no impl cannot be placed in a template. The
compiler emits the same call for every hole; the type chooses. The rule holds on every hole,
including one whose type the checker does not know.

**Control flow is written as an element:**

```nika
<table>
<for row in :rows><tr><td>{row.id}</td><td>{row.message}</td></tr></for>
</table>
```

`:rows` carries the colon because it is **captured from the enclosing scope**: the colon is
where the template's names end and the program's begin. The loop runs over the captured
collection and borrows rather than copies. The position check runs through a loop's body; a hole
in a `<script>` is refused inside a loop too.

**`std::fs` (Compiler Magic)**
File system access looks **blocking**. The compiler transforms each call into a **non-blocking** state machine backed by the runtime's reactor. User code never blocks the thread and never writes a callback.

Every function below may fail for environmental reasons, so every one of them `throws` (Appendix A.1). **What they throw is `io::IoError`**, with `NotFound`, `PermissionDenied`, `NotText`, `Outside` — where a name would leave the `fs::Root` it was given — and `Other`. A handler that only passes the failure on names nothing; one that takes it apart writes `use std::io` and `io::IoError::NotFound(p)`. None of them takes an `async` marker, and none is awaited.

**Whole-file access**

```nika
// Subject: the path, and the root it may not leave ; Config: options
pub fn read(path: Path, root: Root) -> Bytes throws                  // whole file, as bytes
pub fn read_to_string(path: Path, root: Root) -> String throws       // whole file, UTF-8 validated
pub fn walk(path: Path, root: Root) -> Vec[String] throws           // every file under a directory
pub fn write(path: Path, root: Root, data: ref Array[u8]; append: bool = false, create: bool = true) throws
```

**Every path names its root.** `root` is an `fs::Root`. `Dir(store)` resolves the name under
that directory and throws `io::IoError::Outside` where it would leave it; `Outside` is a case of
`io::IoError`, not an error type of its own. `Anywhere` performs no check. `--trust` (on `nikaia build` and `nikaia lower`) lists
every site that writes `Anywhere` or `Dir("/")`. There is no default and no exception for a
literal: a relative name is resolved against the working directory. A call that leaves the root
out is refused with `NK1101`, whose help names both forms.
`fs::map(ref path, fs::Root::Anywhere)` is the form for a command-line program whose path the
operator typed.

**A directory of one's own.** `pub fn scratch() -> Root throws` makes a fresh, empty directory and
hands it back as `Dir`, so what is written under it is checked to stay there
([ADR-247](adr/adr-247.md) D5). Under `nikaia test` it is inside the test's own directory and is
removed with it (14.1); elsewhere it is a new directory under the system's temporary directory.

**A directory walk.** `walk(path, root)` hands back every file under the directory `path`, as
names relative to it with `/` between the parts, sorted, so two runs over one tree agree
([ADR-290](adr/adr-290.md) D14). Directories are not listed, and each is visited once, so a link
back up the tree ends the walk. Under `Dir(store)` the root holds for every name the walk finds:
an entry that leads out of it, a symlink pointing away, is left out rather than failing the walk.
It throws where the directory itself cannot be read, and `NotText` where a name under it is not
UTF-8. A name it hands back opens under the same root: `fs::read_to_string(ref name, root)`
after `fs::walk(".", root)`.

**The reactor.** `read`, `read_to_string` and `write` go through the runtime's reactor. Where the
machine has a completion queue, the kernel performs the operation and reports when it is done;
otherwise the runtime's own I/O thread performs it. Which one serves is decided when the program
starts, never when it is compiled, and an operator may pin it (13.3b). The reactor runs before
the program's first statement, so an operation costs no thread start and no thread wake-up.
`map` is not on the reactor: it hands back pages the operating system owns. A read is a slot the
caller polls rather than a call that blocks its thread, and the executor runs something else
while it is in flight. Two reads are put in flight together by `overlap { … }` (Part I 8.1.2) or
a `spawn`, never by the compiler.

`read` returns **`Bytes`**, not a `Vec[u8]`: one shared buffer, and slices that outlive its scope are tethered to it (Part I 6.6). A parser can therefore hand back thousands of names that all point into a single allocation.

`Bytes` is a language type, written bare, and is one reference-counted buffer. A tethered slice keeps its buffer alive in whatever keeps the slice; nothing is written for it.

**Reading a large file: `map`, and the grammar**

There is no `fs::lines` and no `fs::bytes`. A program maps the file and walks the mapping's lines. `fs::map(path, root)` owns the pages and `.lines()` borrows views of them: tethered `ref String`, no allocation per line, constant memory.

```nika
let data = fs::map(ref path, fs::Root::Anywhere)
for line in data.lines() { … }
```

A file that is a **record per line** is read with the grammar protocol: `@frame(boundary: "\n")` says so, and the grammar drives itself over the pages, in parallel where `user_parallelism` allows (Part II, 10.7).

`map` is a compile error on `wasm32-*`.

**Handles**

```nika
pub fn open(path: Path, root: Root; write: bool = false, append: bool = false,
            create: bool = false, truncate: bool = false) -> File throws
```

`File` implements `Cleanup` (Part I, 6.4): the compiler flushes and closes it at the end of the scope, on the normal path and while an error is bubbling up, and a flush that fails surfaces as an error. A program calls `close()` explicitly where it handles that error at a precise point.

```nika
impl File {
    pub fn read(ref mut self, into: ref mut Array[u8]) -> i64 throws
    pub fn write(ref mut self, data: ref Array[u8]) -> i64 throws
    pub fn flush(ref mut self) throws
    pub fn seek(ref mut self, to: Seek) -> i64 throws   // Seek::Start(n) | Current(n) | End(n)
    pub fn len(ref self) -> i64 throws
    pub fn close(self) throws                        // explicit opt-in; otherwise Cleanup does it
}
```

**Memory mapping**

```nika
pub fn map(path: Path, root: Root) -> Mapped throws     // read-only memory map
```

`Mapped` derefs to `Bytes`, so a mapped file is a tethered buffer like any other. The pages are the buffer, and nothing is copied. *Decided, not built yet:* today `Mapped` derefs to text ([ADR-283](adr/adr-283.md) D22).

**Retention.** A slice that escapes the mapping's scope tethers to it, and a tether keeps the whole map alive. The compiler warns where a small extract outlives a large buffer and suggests `.clone()` (*decided, not built yet*: today `--tethers` names the cost, [ADR-283](adr/adr-283.md) D6). The mapping is released once the last tether is gone, which may be later than the end of the block that created it. Slices that never leave that scope cost nothing and hold nothing. At process exit a read-only mapping with nothing observable attached to it is left to the operating system rather than unmapped page by page.

**Availability is a property of the target, and of nothing else.** `fs::map` is available at every value of every build option on any target whose platform provides memory mapping: a single-threaded program compiled for Linux, macOS or Windows maps files exactly like a parallel one. On `wasm32-*` `fs::map` is a **compile-time error**. What `std::fs` offers on `wasm32-*` in place of `fs::map` is decided with that target (17.2).

**Metadata and directories**

```nika
pub fn exists(path: Path, root: Root) -> bool throws
pub fn metadata(path: Path, root: Root) -> Metadata throws   // len, is_dir, is_file, modified
pub fn read_dir(path: Path, root: Root) -> DirEntries throws
pub fn create_dir(path: Path, root: Root; recursive: bool = false) throws
pub fn remove(path: Path, root: Root; recursive: bool = false) throws
pub fn rename(from: Path, to: Path, root: Root) throws       // both names under one root
pub fn copy(from: Path, to: Path, root: Root) -> u64 throws
```

**Availability by target**

Every value of every build option has the same `std::fs` surface; only the target changes it.

| API | Native | `wasm32-*` |
| :--- | :--- | :--- |
| `read`, `read_to_string`, `write` | yes | yes — backed by OPFS |
| `open` | yes | yes — backed by OPFS |
| `map` | yes | **compile error** — the platform has no memory mapping |
| `metadata`, `read_dir`, `create_dir`, `remove`, `rename`, `copy` | yes | yes — OPFS, within the origin's sandbox |

`std::thread` (17.2) is barred by **`user_parallelism = no`**, so manual threading is a compile error even on a native target that has threads. `fs::map` is barred only by the **target**.

**`std::collections` — and where keys come from**

Every input carries a **provenance**, because the compiler knows where a buffer came from (Part I 6.6):

* **Untrusted**: a remote peer chose these bytes. HTTP requests, sockets, IPC, rows read back from a database.
* **Trusted**: the operator chose these bytes. Files, command-line arguments, the environment, anything compiled in.

A value inherits the provenance of the buffer it comes from, and a collection takes the most cautious provenance of everything put into it. Where the compiler cannot tell, across a dynamic call or from a foreign library, the answer is Untrusted.

The hasher follows from the provenance: untrusted keys get a keyed hash with a per-process random seed, and trusted keys get a fast one. Within that, the keys' shape picks the function: a fixed-width type, text of a known length, or text of unknown length. A program may name its own with `hasher:` where it makes the map, and `--trust` lists the choice for every map ([ADR-316](adr/adr-316.md)). *Decided, not built yet.* Nothing else about the map changes: same table, same API, and keys are always compared in full.

**User code has the last word, at the place the data enters:**

```nika
// A service that processes files uploaded by strangers:
// a local path, but bytes nobody vetted.
let data = fs::map(path, fs::Root::Anywhere; trusted: false)
```

The reverse, `trusted: true`, exists for the case where the program knows the peer. Both are recorded in the ledger. A grammar for a wire format can pin the floor for everyone who uses it, `@untrusted grammar HttpHeaders`, so that no application can lower it ([ADR-010](adr/adr-010.md) D3-D4). *Decided, not built yet:* the `trusted:` argument and `@untrusted grammar`.

`--trust` (on `nikaia build` and `nikaia lower`) prints where the program's bytes came from, which source said so, and which hasher its maps got:

```text
$ nikaia lower 1brc.nika --trust
input provenance: trusted
    cli::args is trusted
    fs::map is trusted
hash for a map keyed by the input: fast, fixed seed - no adversary chooses these keys
```

An untrusted map is seeded randomly, so **its iteration order is not stable between runs**. A program that needs an order asks for it explicitly.

**Other key modules:**
* **`std::text`**: what text says, written in Nikaia (`std/text.nika`).
  `text::parse_i64(s)` is the whole number `s` writes - an optional sign and
  digits - or nothing where it writes none, a number too large for an `i64`
  included: `text::parse_i64(s) ?? 0`. `text::digit_value(c)` is one digit's
  value. A text whose shape has to be explained when it is wrong is a grammar's
  to read, with `dec[T](p)` (Part II 10.8).
* **`std::json`**: serialization using compile-time code generation, with zero-allocation parsing where possible.
* **`std::cli`**: parsers for command-line arguments, environment variables and ANSI terminal colours.
* **`std::net`**: low-level TCP sockets for building custom protocols — `listen`,
  `connect`, `accept`, `read`, `write`. Everything that waits gives the thread up,
  so one thread serves many connections at either setting of `user_parallelism`.
  **What comes off a socket is `untrusted`.**

* **`std::http1`**: HTTP/1.1's **text half** — where a head ends, what its lines
  say, which header the client sent, what the query string says, and where the
  body starts. It reaches no socket, so everything in it is `sync`: a program
  reads bytes with `net` and hands them over. It is not a `std` HTTP module;
  **the protocol is the `http` package's**. It is named `http1` because a
  program reaches a *package* by the word `http`.

### 17.2. Availability by Target and by `user_parallelism`
Some modules are available, or behave restrictively, depending on the target and on whether user code may run concurrently.

**A target without an operating system** is one more column of this section. `user_parallelism` is `no` on it. `fs`, `net`, `process`, threads and memory mapping do not exist on it, and neither does the `http` package, which is built on `net`. Everything the language itself is carries over: `overlap`, `spawn` as a coroutine, the four doors, `Cleanup`, `Seen`, grammars, `comptime` and Chapter 16's assembly. The emitted Rust is `no_std`, the executor is the target's with interrupts as wakers, an interrupt handler is a `fn() sync` that touches no lock, a lock is a critical section the length of its block, and a build may forbid allocation after start.

* **`std::process`**: starting another program and waiting for it
  ([ADR-243](adr/adr-243.md)). `process::run(program, args; dir: …)` runs it to
  the end and hands back its exit `code`, whether it was `ok`, and its
  `stdout` and `stderr` as text. A non-zero exit is an answer and not a
  failure; what throws `io::IoError` is a program that cannot be started, or
  output that is not UTF-8. The wait gives the thread up, so other tasks run
  while the program does.
* **`std::thread` / `spawn`**:
    * **At `user_parallelism = yes`:** full concurrency. The primary mechanism is `spawn`.
        * **Strict implicit move:** ownership of the ordinary data used inside a `spawn` block is transferred to the new task. A handle on a shared value is the one exception and is duplicated instead, so the name outside keeps working (Part I 6.2).
    * **At `user_parallelism = no`, and on `wasm32-*` whatever it says:** direct use of `std::thread` is a **compile-time error**. The runtime is share-nothing under that value.

At `no` the runtime starts its I/O workers and nothing else, so no thread carries user code; at `yes` a pool for user code starts with it, sized by `user-pool` (13.3b). At `no` a task interleaves in the executor, and no user code runs concurrently; at `yes` a task runs on a thread of the `user-pool` pool.

> Interleaved is not concurrent.

**`std::db` (the protocol, and nothing else)**
`std::db` holds what two database drivers must agree on without depending on one another: the `Connection` and `Transaction` traits, the `Statement` protocol a `dsl` block's prepared statement speaks, and the values a row may carry: the language's numbers, `bool`, `String`, `Bytes`, and `T?` for `NULL`. **No SQL, no grammar, no dialect**: the compiler knows none. A dialect is a grammar in a **driver package** (`sqlite`, `postgres`, a vendor's own), which also brings the DDL grammar for the schema, the connection and the target adapter.

* **The driver checks the query while the program is built.** `dsl sqlite(schema: app) { SELECT name, email FROM users WHERE age >= :min_age } eod` hands the statement and a schema file (a `comptime` asset) to the driver's grammar. A missing table or column is a build error at the query, every `:hole` is a typed named parameter, and the grammar declares the result columns with `meta::column`, from which the compiler derives a **row type** with named, typed fields (Part II 10.5). No live database is opened while building; whether the one opened at runtime still matches the file is the driver's check at `open`.
* **Zero-blocking guarantee:** database operations are implicitly asynchronous. They never block the event loop, nor the compute scheduler where there is one.
* **Architecture adapter**, the driver's: on native targets the runtime's own I/O thread carries the blocking calls; on the web a **Web Worker** with **OPFS** (Origin Private File System) does, so a persistent database runs in the browser without freezing the UI thread.
* **No expression capture and no object-relational mapper**: a query in memory is the `Seq` combinators, the row type is the mapping, a migration is a SQL file. Dynamic SQL is a driver's `raw(text)` with untyped, `Untrusted` rows.

```nika
use sqlite                                    // a driver package, not `std`

let app = comptime asset("schema.sql")        // the schema, read while building

fn query_data() {
    // Transparently starts the required sidecar (thread or worker). The
    // connection is typed by the schema it was opened for (ADR-299), so a
    // statement checked against another schema is refused at `execute`.
    let db = sqlite::open("app.db"; schema: app)

    // The driver's grammar checks the statement against the schema while
    // the program is built, and declares the columns; the row has fields.
    let active_users = dsl sqlite(schema: app) {
        SELECT name, last_login FROM users WHERE last_login > :since
    } eod

    for u in active_users.execute(db; since: 0) {
        println(u.name)
    }
}
```

---

# Appendix A: Error Hierarchy

Nikaia distinguishes between errors caused by the environment, which are recoverable, and bugs in the program logic, which are not.

### A.1. Recoverable Errors (`throws`)
Errors arising from external circumstances, such as a missing file or a network timeout.
* **Mechanism:** declared in the function signature with `throws`.
* **Handling:** enforced by the compiler through `catch{}` blocks or propagation.

### A.2. Unrecoverable Errors (`panic`)
Errors indicating an inconsistent program state: an index out of bounds on a **list** (a map read through the brackets is a `T?` and never panics); division by zero; **arithmetic overflow**; **a conversion whose value does not fit**; a handle a C declaration claimed and C did not hand back (15.1); and an explicit `panic()`.

An overflow is in this list at **every** build. Where a program means to wrap or to stop at the limit, it says so by name (Part I, 2.2). A conversion is checked in the emitted code at every build; `truncating_i32` is how a program asks for the low digits.

A constant that cannot fit the type it is given is refused with `NK1116`, and a division whose divisor is a constant zero is refused with `NK1118`. Neither takes a case out of the list: a divisor the compiler cannot evaluate is divided by at run time, and a zero there is unrecoverable. The compile-time refusal replaces the run-time message only where no program had to run to know it (C.1).

A panic depends on **two** of the three build options (13.3), `user_parallelism` and `target`:

| `user_parallelism` | Panic Behavior | Consequence |
| :--- | :--- | :--- |
| **`no`** | **Abort** | The process terminates immediately. The stack is not unwound. |
| **`yes`** | **Task Poisoning** | Only the affected task is terminated. The worker thread catches the panic (fault isolation). Resources (`SharedMut[T]`) held by the task are marked poisoned, so no other thread reads state a half-finished task left behind. |

**One end that is not a panic and is not success either.** A `cleanup-deadline` that expires ends the program with **exit status 70** (`EX_SOFTWARE`) and a message naming every resource whose cleanup was cut off, delivered on the panic path: standard error and the panic hook. No build option and no runtime configuration turns that into a `0`. `cleanup-deadline = "0"` does not drain and therefore never expires.

The `target` decides independently: on `wasm32-unknown` a panic is a **trap** and the
module is done, whatever `user_parallelism` says. A build with `artifact = "c-library"` unwinds instead (15.1): every entry point catches the panic and returns `E_PANICKED`, and the library is poisoned until `shutdown` and `init` have run. *Decided, not built yet.*

**The third build option changes nothing on this page.** `reentrancy-check` (13.3) decides whether a compiled program notices a lock taken while a lock is held. Taking one is refused at build time, so in a program the compiler accepted the check cannot fire; if it fires, the refusal has a hole. The re-entrancy panic Part II 12.2 describes at `user_parallelism = no` is that check, so the table above has no row for it. Poisoning happens only in the `yes` row: at `no` a panic is an abort, and no task survives to be poisoned.

On **every** panic path, including the abort and the WASM trap, the application's **panic hook** runs first (Part I, 7.2): one global `sync` handler receiving message, location and stack trace, for crash dumps and reports. It runs before aborting even under `panic = abort`.

**The abort names the `.nika` line.** Every program carries a table of
generated line to Nikaia file and line, and the hook looks the site up before it
prints anything. An overflow, a conversion that does not fit, an index out of
bounds and a written `panic()` all read
`src/main.nika:2: the program stopped: attempt to multiply with overflow`, and
never name a generated file. A location the table does not know, such as a panic
inside `std`'s own Rust or a foreign crate's, is left in the words of whoever
wrote it.

# Appendix B: Compiler Internals & Annotations

To enforce the contextual capture rules (Part I 5.4) without hard-coding function names into the compiler, Nikaia uses internal attributes. They belong to the standard library.

`@detached` is a ledger fact and not a word a program writes: whether a function-typed parameter is run or kept is inferred from the body (Part I, 5.4 C).

### B.1. Capture Attributes

| Attribute | Internal Name | Default | Description |
| :--- | :--- | :--- | :--- |
| None | `capture_mode = "immediate"` | Yes | The lambda executes within the caller's stack frame. Captured variables are **Borrowed** (`ref T`). Used by `map`, `filter`, `lock.access`. |
| `@detached` | `capture_mode = "detached"` | No | The lambda escapes the current stack frame (stored, spawned, or deferred). Captured variables are **Moved** (Owned). Used by `spawn`, `defer`. |

### B.2. Standard Library Signatures

Common standard library functions are annotated internally as follows:

```nika
// std::collections::List
// Standard immediate execution
pub fn map[U](self, op: fn(T) -> U) -> Vec[U]

// std::task (Global Spawn)
// Detached execution: Must take ownership of environment
pub fn spawn(task: @detached fn() -> T) -> TaskHandle[T]

// std::task
// Scope is immediate because it waits for completion
// Where they run in parallel, a scope's child tasks must be 'sync' (see Part II, 12.7)
pub fn scope(f: fn(Scope))
```

# Appendix C: The Diagnostics Contract

Nikaia compiles through the Rust toolchain. The backend's error messages, about lifetimes, borrow traits and generated code, never reach a Nikaia program's author. Diagnostic quality is a **testable requirement**.

### C.1. The Iron Rule

> **An untranslated backend (rustc) error reaching the user is a Nikaia compiler bug.**

The compiler reads every backend diagnostic from `rustc --error-format=json` and places it on the `.nika` span that produced it ([ADR-300](adr/adr-300.md) D1, D7). Each borrow, ownership and lifetime error class is mapped to a Nikaia diagnostic with Nikaia vocabulary, `.nika` spans and a concrete fix-it. An unmapped error is reported as an internal error, never as normal output. The error catalogue below is also a test suite: every entry has a minimal `.nika` reproduction that must produce the documented message.

### C.2. Requirements for Every Diagnostic

1. **No prior knowledge assumed.** The message is understandable without Rust or systems-programming background. The terms "lifetime" and "borrow checker" and Rust error codes never appear.
2. **Always say what to do next.** Every error names at least one concrete way out (clone, use `Shared`, use `retain`, mark a function `sync`, move the I/O out of the lock, …), as paste-ready code where possible.
3. **Narrate cause chains.** An error caused by a change (through the ledger, 13.5) shows both sides: the edit that changed the contract and the caller that broke.
4. **Positive guarantees over prohibitions.** Where the language removes a danger structurally (tethered slices, scope waiting), the documentation and the messages state the guarantee, such as *the buffer cannot die while a token lives*, not the forbidden thing.
5. **Spoken to the reader, in plain sentences, at the places it is about.** A message says what *you* wrote, why it cannot work, and what to write instead, in the words a colleague would use: *You're changing `count`, but it wasn't declared as mutable.* Not *`count` is changed, and a `let` that is changed says `mut`*, which is correct and reads like a rule being recited. The source is shown at every place the explanation needs, each underlined whole and named: `^` under the place the error is, `-` under a place that explains it (*declared here without `mut`*). References to the specification and the decision records stay in the documentation, not in the message. **Every message has one layout**, whichever part of the compiler says it — a parse error, a checker finding, a refusal of the lowering, a relayed `rustc` message: the headline as a sentence, `-->` and the place, the source line with the place underlined, then `= note:` and `= help:`. A message with no code of its own (a parse error) is headed `error:` alone. A message that names no word of its own underlines its statement's first line:

   ```text
   error[NK1139]: You're changing `count`, but it wasn't declared as mutable.
     --> app.nika:4:9
      |
    2 |     let count = 0
      |         ----- declared here without `mut`
     ...
    4 |         count += n
      |         ^^^^^ changed here
      |
      = help: Add `mut` where it's declared: `let mut count`.
   ```

### C.3. Error Code Catalogue (NK codes)

| Range | Domain | Examples defined so far |
| :--- | :--- | :--- |
| `NK1xxx` | Syntax & types | `NK1101` a call passes the wrong number of arguments. `NK1102` an argument is not what the parameter takes. `NK1103` a `let` says one type and is given another. `NK1104` a `return`, or a body's last expression, is not what was declared. A body whose last statement is a `while true` that no `break` leaves is not asked ([ADR-276](adr/adr-276.md) D6). `NK1105` an assignment is not what the target holds. `NK1106` a struct literal gives a field the wrong type. `NK1107` a field that is not there. `NK1108` a condition that is not a `bool`. `NK1109` a call names an option the callee does not have (Part I, 5.1). The same code carries a `dsl` statement's parameters handed to a callee that is not a **driver**, one declared with `...args: Self::dsl`: nothing else has a place for them to arrive (Part II, 10.5). `NK1110` a call reaches an item, or a field, another **package** keeps private (Part I, 9.2): *`secret` is private to `http`*, and *`http::Request.method` is private to `http`* for reading such a field and for giving one a value in a struct literal. A field's `pub` is in the ledger. `NK1101`–`NK1110` are answered from the ledger (13.5), so a call into a library is checked against the contracts the library ships. `NK1111` (**warning**) a plain string holds what looks like a hole, or a doubled brace; the fix is `f"…"`. `NK1112` a call does not pass a parameter the DSL statement it is given declares; `NK1113` a call names one that statement does not have (Part II, 10.5). Both are answered from the statement's body, where a `:name` is written. `NK1114` is **retired** and not reused; a body that reaches for `a`, `b` or `c` meets `NK1117`. `NK1115` a call wants a shared value and is given a plain one (Part I, 6.2). The way out is an explicit wrap at the call, `Shared(db)`, never an implicit one. `NK1116` a constant does not fit the type it is given (Part I, 2.2): at an annotated `let`, a `return` against a declared result, an argument whose parameter says what it takes, a bare `let` where an operand's declaration pins the type, a constant no type holds, a constant reached through a name, **an operation over names whose type is pinned, wherever it stands** - an `f"…"` hole, a condition, a list, a receiver - once, at the innermost operation that overflows, and a number its uses typed (Part I, 2.4) that is given one the type does not hold, with a note naming the use. A literal standing with nothing beside it keeps its type-less reading (Part I, 2.4), and an expression between those cases is widened rather than refused. The refusal replaces the backend's message (C.1). `NK1117` a statement is one name and nothing declares it. A word the scannerless grammar has no rule for is read as a name, and a name on its own is a legal statement; `assert c` and a leading `_000` are refused here. `1_000` is the number `1000`. The help explains a word that is not reserved: `loop { … }` is answered with *write `while true`*, `const X = …` with *write `comptime`*, and `macro` and `quote` with *Nikaia has no macros*. Four things declare a name: a local or parameter in scope, a function either ledger describes, a type declared here, and a module of this program. A name this compiler cannot see is not refused (C.4). **The same code carries a call by a bare name** nothing declares — *nothing declares a function `dobled`*, with *did you mean `doubled`?* where a function of this program or a bare `std` name is near. A bare name has nowhere else to come from: a `use` of a module brings none in, so a package's function is written with the package's name in front (Part I, 9.1). A name a `use` item ends in, or renames to, counts as declared. **The same code carries the prelude's two rules**: a `std` name that lives in a module written **without** its prefix — *`std` has `text::digit_value`* — and a module used **before** it is introduced — *`fs` is used here and introduced nowhere*. Both hand over the line to add. Both are asked only where `std`'s ledger has exactly one entry for the name, and never about a module of this program or a package. What needs no `use` is the list in Part I 1.3: the entries `std`'s ledger keys **bare**. `NK1118` a division, or a remainder, whose divisor is a constant zero. A division by zero at run time stays where A.2 puts it. `NK1116` and `NK1118` read the same constant fold: a literal, an immutable `let` whose value folded, `+ - * / %` and a negation, in a magnitude and a sign (65 bits), each step held to the type a declared operand gives it; nothing is claimed about a divisor the fold cannot evaluate. `NK1119` a `let`, a `for` binding, a lambda's argument or a struct field called `self`. `self` is the one reserved word that is a name; every other reserved word does not parse in a name position. A parameter named `self` is refused by the grammar, because the receiver takes the word. `NK1120` is **unused** and the number is not reused. `NK1121` a `?.` reaches through a value that cannot be absent; the way out is the plain `.`. Asked only where the receiver's type is known (C.4). `NK1122` an `as` names a type Part I 2.2 does not offer - `n as usize`, `n as u128` ([ADR-285](adr/adr-285.md) D14). `NK1123` a hull written a second way, or a hull of a hull: `Shared[Locked[T]]` is `SharedMut[T]`, and the message carries the replacement; `Shared(x)` where `x` is already a handle adds a second count around one value. `NK1125` a member reached off a `T?` with a plain `.`, `NK1121`'s mirror. `a?.b.c` guards `a` and nothing else, so the unguarded `.c` reaches into a `T?`; a `T?` is a type of its own (Part I, 2.3), and a member of `T` is not a member of it. The way out is the guarded form, or `??` and then the plain `.`. `NK1124` a door over several locks written wrong: handed something that is not a lock, handed one where several are wanted, or given a block that does not name one value per lock. A number is refused there although a literal carries no type (Part I, 2.4). `NK1126` a field or a method reached on a **type parameter** that no bound of it declares. A `T` with no bound can be moved and passed and nothing else. The message says *no bound* rather than *no such method*. `NK1127` a `comptime` binding this compiler cannot evaluate while it builds. A `let` may fold; a `comptime` must. What may be evaluated is Part II 10.2's list: an integer, a float, a `bool`, text, a list, a pair, a struct or an `enum` variant, and a **call** to a function or method of this program whose body is made of those. The message names the way out: `let`. `NK1128` a name the **language below** reserves and cannot escape: `crate`, `super` and `Self`. Every other such name is written escaped and stays a name, so a field called `type` is accepted. Asked at every position that declares a name, including an item's own name. `NK1129` a trait's method that an implementation **pauses** in where the declaration says `sync`. Without the word a method may pause. `NK1140` the same comparison for `throws`: a body that fails under a declaration without the word. `NK1130` an `impl` and the `trait` it names disagree about **which methods exist**: one the trait does not declare, or one it declares that the `impl` leaves out; two messages under one code. An `impl` of a trait this unit does not declare is not checked. `NK1131` a field of a **borrowed** subject handed out by value, such as `return self.username` out of a `ref self` method whose result is declared by value ([Part I 6.8](10-nikaia-light.md)). The compiler writes no `.clone()`; the message names both ways out, and at a `return` a third: declare the result `ref T`, and the view is what the line means ([ADR-202](adr/adr-202.md)). Asked at a `return`, a `let` and either kind of call argument, and only where the field's type is known and does not copy: a number, a `bool`, a `char` and a view take nothing away, and nothing is claimed about a field this compiler cannot type (C.4). `NK1132` a `break` or a `continue` with no loop to act on (Part I 3.3): there is no loop, or a function boundary stands between (a lambda, a task, an `overlap` branch or a DSL fold's step), and a jump does not leave a function. A `catch` handler is not such a boundary and is not refused. `NK1133` a statement after a `break` or a `continue`, in the same block. `break` carries no value, so a value written after it parses as a statement of its own. `NK1134` a `catch` over an expression that **cannot fail**. An expression whose failure could not be looked up is not refused. `NK1135` a **type** nothing declares, or one written **without its module**: *`HashMap` is written without its module*, with `use std::collections` and `collections::HashMap` handed over. A type that lives in a module is reached through it, as a package's is; what needs no prefix is the list in Part I 1.3, the names `std`'s ledger keys **bare**. Only `std`'s modules say `use std::…`; a package's prefix is the package's name. The known set is Part I 2.2's own types, the ledger's `types` map and the declaration's own type parameters; a qualified name is left alone. A bound naming a trait neither this unit nor a ledger declares is the same refusal, with the message saying *trait*. `NK1136` a `let` that binds several names and is given a type: the names are taken apart by position, and one written type cannot say which of them it is about. `NK1137` a `ref` written where the **compiler** lends anyway, in either position it lends: a `for` lends the place it is given, and at a call the argument is lent wherever the callee's `keeps` column says the parameter is only read. A `ref` in a declaration is untouched. A `ref` in a position the callee keeps, in front of a copy type, at a method call, or in front of a value whose type this compiler could not work out is left alone (C.4). Asked after the fit, so a `ref i64` handed to a `ref Request` stays `NK1102`. `NK1138` a parameter a body **changes** where the declaration does not say `mut`. `fn fill(mut out: Vec[i64])` is where in-place change is written, and the caller's value is what changes. Raised only where the change is certain: an assignment into the parameter or into a place rooted at it, or a method every candidate entry marks `mutates`. A name a `let` has bound is no longer the parameter, so `let mut v = x` is a way out, and a method no ledger describes is not refused on (C.4). Said once per parameter, with the caret on the declaration. For a fold's accumulator it is asked at a door and nowhere else: `par_fold(…, fn(acc, m) { acc.record(m) })` compiles. `NK1139` a **`let`** whose value is changed, and no `mut` on it ([Part I 2.1](10-nikaia-light.md)). It is separate from `NK1138` because a parameter's `mut` also decides what the caller sees, where a `let`'s is only about this body. Both read the binding in scope, so an inner block's `xs` stops being it when the block closes and an outer `mut xs` is it again. `NK1141` an `update` block that hands a value back. The block takes `mut v` and changes it, and the change is the result. Asked of the shape only: a last statement that can only be a value, or a `return` carrying one. A last statement that is a call is left alone (C.4). `NK1143` a call to a name an `extern "C"` block declares, **outside** an `unsafe` block (15.1). Only a name this file declared `extern`. `NK1144` a `let _ = expr`: `_` is the ignore pattern and stands where a name would be bound (a tuple position, a parameter, a part of a `match` pattern), never as a whole binding. The message says to write the expression as a statement or bind it to a name. `NK1145` a field of an `extern "C"` struct that is not a C value: text, a list, an optional, a lock, a `Shared`, a function or a struct without the word. The message names the handle as the shape to use. `NK1152` a build-time body the rule forbids: a callee that can **pause**, or whose touch set is anything but the build's own parameters. Distinct from `NK1127`, which says *this compiler cannot evaluate it*. The rule is two ledger columns and not a list of allowed functions. The same code carries the **call depth**: there is no step budget, and a recursion without a base case is refused. `NK1153` an **empty list** whose element type nothing ever says: `[]` carries none, so it takes one from the first use that needs one. Asked once the whole body has been walked, and only where nothing at all uses the name; a use this checker cannot read a type out of is left to the language below (C.4). `NK1154` two elements of one list literal that do not agree. The message names the two types, or the kind (*a number* beside *text*) where an element has no type yet. Once per literal. `NK1155` the alternatives of an `|` pattern bind different names. Every alternative binds the same set. Recursive, so an `|` inside a tuple's part is the same refusal one level down. `NK1156` a `use std::…` whose last segment is a **type**: a `use` names a module and brings no name in, for `std` as for a package. Where the type lives in a module the help names that module — `use std::fs` and `fs::Mapped` — and a type keyed without one needs no line at all (Part I, 1.3). A ledger key is a type's when its entries are called on a value (`self`), and a module's when they are not. A module nothing describes is `NK1186`'s question. `NK1186` a `use std::…` whose module **`std` does not have**. The accepted modules are what `std`'s ledger declares, joined with the modules this specification names without a ledger entry — `db`, `json`, `process`, `thread`, `panic`, `build`, `task`, `backend`. The prefixes the ledger keys that are **not** modules are subtracted: `str::len` and `list::ListExt::map` sit beside `fs::read`, and a primitive and a trait need no line (Part I, 1.3, 2.2). Only the **second** segment is asked, so `use std::db::postgres` is answered by the module it starts at. A module `nikaia-std`'s crate has and `std` does not offer gets its own message: `tools` holds the Rust-signature grammar, which the compiler calls and a program cannot reach. `NK1187` a **grammar element** the page names and this engine does not have: `tag` and `digit1`, each answered with the spelling to use — a string literal and `digit+`. `NK1188` `==` or `!=` on a type **two values of which cannot be compared**. A type compares when every part of it does, walked structurally: the language's own types from a list, a container exactly when what it holds does, a declared `struct` or `enum` from its parts, and a type whose parts are Rust from the ledger's `compares` column, whose absence is **no**; a lock, a mapping, a socket, a task's handle and a channel's ends do not compare. The help is one the source can take (C.2). Asked of one side and only where its type is known (C.4); the two sides disagreeing stays `NK1102`. `NK1158` a `ref mut` written outside an `extern "C"` declaration. It exists for the C boundary only: a parameter this language may change is written `mut name: T`. `NK1159` a length handed to an `extern "C"` declaration beside a buffer it cannot be shown to fit. A `ref Array[T]` and a `usize` right after it are one fact in C. Two shapes are accepted: the buffer's own `len()`, and a constant a known length covers — an `Array[T, N]` carries its length and a `Vec[T]` does not; a zero is covered by every buffer. Nothing is claimed where the callee is not a foreign declaration, or where the buffer's type is unknown (C.4). `NK1161` a `throw` of something that is not an error (Part I 7.1). What is thrown implements `Error`, and the `impl` line says so; a number, a `bool`, a character and text are Part I 2.2's own types and never do. Only those are refused: a type this file declares may have its `impl` in another file of the same package, a package's type is not this compiler's to answer for, and a caught error re-thrown is a `?` (C.4). The **kind** is asked and not the type, because a bare `3` fits every numeric type and arrives as `?` (Part I, 2.4). `NK1160` a field, an index or a **call** reaching past an `opaque` handle. A handle is an address this language never dereferences, and nothing here constructs one. `NK1157` a list literal standing where an `Array[T, N]` is wanted, with a different number of elements. The message names **both** numbers. The literal takes the array type from its use; an element that is not what the array holds is refused under the code of the `let`, the argument or the field. `NK1151` a `match` that does not cover every case. An **enum** is complete when every variant is named; anything else needs `else`, and `bool` is complete with `true` and `false`. A bare name catches and covers. Where the scrutinee's type is not known, nothing is claimed (C.4). `NK1150` a `match` arm written `_`: the catch-all arm is `else`, and `_` is the **ignore pattern**, for a value that arrived. Raised by the parser. `NK1149` a type's constructor written `Type::new`: a type is constructed the way a `.nika` file declares one, `Vec()`, `String()`, `HashMap()`, `Stats(first)` (Part I 4.2). Asked of the name and not of the position, so a constructor handed over as a value goes with it: `par_fold(M, Summary, …)`. A qualified name is left alone, as for `NK1135`. `NK1148` a name declared twice in one file: a `fn`, a `struct`, an `enum`, a `trait` and a `grammar` declare a name; a **method** belongs to its type and a **rule** to its grammar, and neither declares one. Across files the same rule is the module layer's message; `nikaia lower` outside a project skips the module layer. The caret is on the second declaration, and the message names both kinds. `NK1147` a grammar's rule reached through a **dot**: a rule of a grammar is a qualified name, `Json::value(input)`, and the dot is for a value's members. Asked only where the receiver is a grammar of this file and the name is one of its rules; the message carries the whole rewrite. `NK1146` a struct literal written like a **call**: `Name { field: value }` is the literal, and `Name(first)` calls the constructor, so an options-only call is unambiguous. Asked where the name is known to denote a type, before the call resolves (C.1). A name nothing declares is not this: `Widgit(size: 3)` is `NK1117`'s call to a function nothing declares, and the brace form keeps `NK1135`'s claim. `NK1142` is **retired** and not reused: a function type stands in a field, a result and a `let` (Part I 5.4). `NK1162` a compound assignment on a **map** slot: `m[k] += 1` reads the slot as well as writing it, and a map read is a `T?`, so the line has to say what an absent key counts as — the message hands over `m[k] = (m[k] ?? 0) + 1`. A **sequence** is untouched, because `xs[i]` is a `T`. Asked only where the read is nullable, so nothing is claimed about a container this compiler cannot type (C.4). `NK1163` a name that needs **no** `use`, written with a `std` module in front of it: `io::println("x")`. It is `NK1117`'s rule read the other way. Both halves are required: the module has no such name **and** the bare one is a name `std` keys. `NK1164` a call passes a type that does not answer for the bound its parameter carries: a type with no `impl` of the trait (the help writes the `impl`), a parameter of the caller's own whose bound lacks it (the help widens the bound), or a type the declaration does not make what a shape bound (`Struct`, `Enum`) asks for, which no `impl` answers. `NK1165` an index into an `Array` a `comptime` computed, past its length: the read is folded while the program builds, so it would fail there. The help gives the indexes there are. `NK1166` a `comptime` whose value is not the type it declares; an `Array[T]` with no length is one, and the help writes `Array[T, N]` with the length the build computed. `NK1167` a `comptime` whose value owns memory one level in: a field, or what a variant carries, that is a `Vec` or a `String`. The message names the field, because the struct around it is fine. `NK1168` a `comptime` that depends on itself; the message names the chain. `NK1169` a `Fixed` map (Part II 10.2) with one key twice; the message names both pairs. `NK1170` a `Fixed` map whose keys are not text. `NK1171` a `Type::member` that names nothing `::` reaches: no such variant, a field (read from a value with a dot), or a field or variant reached by a type's name where a bound (`[T: Struct]`, `[T: Enum]`) is what reaches it. `NK1172` one field set twice, in a struct literal or a `with`. `NK1173` a `with` on what is not a struct value it may copy: an `enum`, another type, a view of a struct, or a value whose type could not be worked out. `NK1174` a `with` that changes no field, which is only a copy. `NK1175` an `asset` this build may not read: no list of readable files is in effect (`--allow-read-from-list=…`), the path is not in it, the path is absolute or climbs out with `..`, the listed file cannot be read, or it is not UTF-8. Each reason carries its own way out. `NK1176` an `asset` whose path is computed rather than written out, so the list cannot be checked against the line. `NK1177` an `asset` outside a `comptime`; the help offers `comptime` or `fs::read`. `NK1178` a grammar this compiler could not run while it built: the parser refused its input (its diagnostic is relayed whole), the result has no build-time form, the grammar would run inside another still running, there is no directory to compile the parser in, or the generated parser did not compile or its result could not be read. The last two are this compiler's defect and say so. `NK1179` a field declared as a view of a list something else keeps, given a list this body owns: a parameter lends a list on its own, a field does not. `NK1180` a member of a reflected field from `T::fields` other than `.name` and `.of(value)`, or of a variant from `T::variants` other than `.name` and `.is(value)`; what a variant carries is read with a `match`. `NK1181` a path whose head is none of the five things a word in front of `::` may be: a type, a package `use` brings in, a package `nikaia.toml` declares, a Rust crate a description covers, or a `std` module; the help is a `use` (with a `[dependencies]` line or `nikaia describe` where the head is not yet declared), never a new file; the refusal fails open (C.4). `NK1182` an `Array[T]` with no length as a field or a result: only a parameter takes its length from the call. The help offers `Vec[T]`, `ref Array[T]` or `Array[T, N]`. `NK1183` a `let` that declares an owned `T` for the element a `for` lends; the help removes the type, or writes `.clone()` for a copy. `NK1184` a `\` in a literal that the escape set does not name. The set is Part I 2.5's: `\n \r \t \0 \\ \' \" \xNN \u{…}`. Asked of a `"…"`, an `f"…"` and a `'…'` alike (C.1, C.2). The refusal reads the same table the build-time decoder does, and the help is read off the escape: a malformed `\x` or `\u{…}` is told the form it should have had, and anything else is a backslash to double. `NK1185` a `??` whose left side is a **view** and whose fallback **owns**. `user?.name` is a view of `user` where `name` does not copy, and `?? "nobody".clone()` asks for one value that is both. The help is `?? "…"`, a text literal, which is already a view (C.2). `.clone()` is read **by name**; `.to_string()` of text is the text itself and stays a view ([ADR-282](adr/adr-282.md) D8), and only of a number or a value whose type is not known is it owned. Both sides have to be known (C.4). The same code carries a **map's value that is not text**: `m[k] ?? []` reads a view of the list the map keeps, beside a fallback of its own, and the help hands over `m[k]?.clone() ?? …`, the copy written where one is wanted. `m.get(k)` is the same read, and so is a name a `let` bound to either: `let found = m.get(k)` is a view of what the map keeps, and `found ?? []` is refused as `m[k] ?? []` is. A map's text beside a name that is a view of text is a view whichever side answers. `NK1189` a copy written `.to_owned()`: a copy is `.clone()`, for text and for everything else. `.to_string()` is the text form of a value and stays. `NK1190` a type called like a function with nothing to call: a `std` type whose ledger describes no constructor, or a struct of this program with no anonymous constructor, whose help hands over the literal. `NK1194` an `assert` whose condition could pause, fail, touch anything or change a value (14.2): every call in it has to be `sync`, throw nothing, touch nothing and change nothing, and one nothing describes is not known to. `NK1195` an `assert` of another shape: other than one `bool`, an option other than a text `message:`, or the name alone. `NK1196` an `impl` method that takes its `self` another way than the trait declares, asked of a trait of the same file and of `Error`, `Drop` and `Cleanup`. `NK1197` a bit operator written beside a comparison without parentheses: this language binds `&` tighter than `==` and C binds it looser, so the line reads two ways; the help hands over `(a & mask) == 0` ([ADR-285](adr/adr-285.md) D10). `NK1198` `&`, `|`, `^` or a shift on something that is not an integer; on a `bool` the help names `&&`, `||` or `!=`. `NK1199` two number types in one operation - `u64 + i64` - with the conversion to write; a shift's count is the one side that may be any integer. Each is asked only where both sides are known (C.4). `NK1200` one number without an annotation, two uses that ask two types of it - handed to an `i32` and to an `i64` - naming both, with the annotation to write (Part I, 2.4). An index asks for an `i64` only where nothing else asks. `NK1202` an `assert` about data from outside the program, which a guard checks instead ([ADR-269](adr/adr-269.md) D6). `NK1207` an `assert`, or a call to a function with a precondition, the compiler shows false with values - every time it is reached, or for a value it shows reaches it - and a warning only under `refuted-claims = "warn"` (13.3) ([ADR-269](adr/adr-269.md) D8). `NK1208` (a warning) a `pub` function's precondition is stronger, or its postcondition weaker, than the committed ledger says ([ADR-269](adr/adr-269.md) D19). `NK1203` and `NK1204` are retired: a call that does not prove its callee's precondition checks it, and a function handed on as a value checks it in its body (ADR-269 D5). `NK1205` a `match` over a `T?` whose arm names a variant of `T`: the pattern says nothing about `null`. The help hands over `??` first, and a `match` of what it gives. `NK1206` a `for` over a `T?`: a `for` walks a list and not the maybe of one, and the lowering walked the option, once, with the whole list as the binding. The help hands over `?? []`; for a map read, `NK1185` then hands over the copy, `m[k]?.clone() ?? []`. `NK1191` an arithmetic operator, or its compound assignment, on something that has none: a list, a map or a set (Part I 4.5), or a `struct` or an `enum` this unit declares - there is no overloading and no trait that gives a type an operator. `-=` is asked as `-` is, and `+` or `+=` onto text stays a concatenation. Where a struct has exactly one numeric field and the side is a name, the help names the field: *Did you mean a field, like `a.balance -= 30`?* Asked only where a side is known to be one (C.4). `NK1201` a list, a map or a set in an `f"…"` hole. A hole is written as its value's text (Part I 2.5), and a collection has no text form; the help joins a list's elements first, `let text = data.iter().join(", ")`. Asked only where the hole's type is known to be one (C.4), and not of a hole whose `:` asks for a form with a `?`. `NK1193` a pattern that looks inside a part that holds its own type further than the lowering reaches: one such part per arm is looked into (the arm is then taken only where the part has the shape, and covers no variant on its own for `NK1151`), but not a part inside that part, and not in one alternative of an `|` ([ADR-246](adr/adr-246.md) D5). A guard may read a name bound in there, and a part bound whole out of a box: it reads them through the box. |
| `NK21xx` | Running at once, and capture | `NK2101` a task takes ownership of a variable still used afterwards (Part I, 8.3). Raised only where the type is known and a move takes it away: a number, a `bool`, a `char` and a view are copied, so the name keeps working; a handle on a `Shared[T]` is duplicated, the exemption Part I 8.3 states; nothing is claimed about a type nothing describes (C.4). An assignment between the task and the later use clears it. `NK2102` scoped tasks must be `sync` where they run in parallel (Part II, 12.7). `NK2103` a `spawn`'s lambda names an argument, and a task is handed nothing (Part I, 8.2): `spawn` starts a body, it does not call it. `NK2104` two branches of an `overlap { … }` cannot run together, and the message names what they meet on; the touch sets decide whether the branches are independent. The same code refuses a branch that **binds** a name, because the block's value already carries every branch's result. A branch this compiler cannot account for is not refused (C.4). An `overlap` is not a task. `NK2105` data used after it was handed to something that keeps it: an argument the callee keeps, a key or a value written into a container, a field or an element of a literal, a `let` that renames it, an assignment. Inside a loop or a lambda the hand-over itself counts; a second use in the same statement, and a use of the part handed over or of the whole it was part of, count too. `NK2106` a part of a value that is only lent here — a `ref` parameter, a `for` binding over a list, a `let` over a place — handed to something that keeps it: the owner still has it. `NK2107` a lambda handed to a `par_iter()` walk changes a name bound outside it — an assignment, or a method that changes what it is called on: every core would write it at once (Part II, 12.6). Asked at both settings of `user_parallelism`; the way out is a `map` and a `collect`, or a `SharedMut` changed through `update`. |
| `NK22xx` | Locks & suspension | `NK2201` I/O while holding locked data (Part II, 12.2). Reading a `fs::Mapped` is a **page fault**, a disk read that neither suspends nor takes a lock, so `NK2202` and `NK2203` say nothing about it; inside a door it is a disk read with the lock held. Both read shapes are asked — a method on it and an index of it — and the claim is the **type's own** `touches` column, so a second such type is a line in a ledger. A type whose column says nothing is not refused (C.4). The way out is reading what the block needs before the door. `NK2202` a `sync` function called something that can pause (Part II, 12.1), answered from the ledger (13.5); and a call that can pause, or a `for` over a sequence whose step does, inside a door's block, which would hold the lock across the wait (Part II, 12.2). A **free** call is resolved by name, and a **method** call by the type checker. `NK2203` a lock taken while a lock is held: written one inside the other, reached through a chain of calls, or opened as a scope inside the block, because a scope's tasks run during the call. The `locks` column propagates *touches a lock* over the call graph `sync` uses, with a `spawn`'s body excluded and a trailing lambda's counted. A `println` takes standard output's own lock and counts. Asked on `Holds` and never on the column's third answer, where the runtime check applies (C.4). `get` and `set` hold nothing open, so neither is asked. The way out is asking for both at once: `access_all(a, b) fn(x, y) { … }` (Part II, 12.3). `NK2204` an assignment to a `SharedMut` directly; the message names the door, `kasse.set(42)` for `kasse = 42`. `NK2205` a `set` given a value that was **seen** in a lock, or standing under a condition that was: what a lock hands out is a `Seen[T]`, the stamp travels with the value, and the message names `update` and `set(…; after:)`. The `get` written inside the `set` is the smallest case. `set(…; after: seen)` is not asked: if the lock still holds what was seen, every decision taken on it still holds. `NK2207` an `update` or `update_all` block that assigns to its `mut v` without reading it: a `set` through the back door, refused as one. `NK2209` a call that can **pause**, inside a grammar's action, or inside a fold's `init`, `step` or `merge`, which is action code too. The same demand holds for a `par_iter` lambda (Part II 12.6); an `overlap` branch may pause (Part I 8.1.2). Asked where the free call and the method call meet, and only where the ledger answered; a callee nothing describes is not refused (C.4). The message names the rule. `NK2210` `sync(f)` names what is not code the call runs: a name that is no parameter, a parameter that is not a function, or one the function keeps (Part II 12.1, [ADR-288](adr/adr-288.md) D31). `NK2206` a lambda that **pauses**, handed to a parameter whose function type says `sync`: `NK2202`'s shape one level over. A lambda that does less fits a type that allows more, never the other way round. Asked of what the body's calls say in the ledger; a callee nothing describes leaves it unasked (C.4). `NK2208` `set_after` written by hand on a lock: it is how `set(…; after: …)` is lowered, and writing it directly would drop a failure nothing in the contracts describes (C.1). Worked through in C.6. `NK2211` (**warning**) a `pub` function the committed ledger says could not pause (`"inferred"`) can now, at the statement that made it and with the calls that lead there ([ADR-288](adr/adr-288.md) D32); committing the ledger acknowledges it. |
| `NK23xx` | Aliasing | `NK2301` cannot change a collection while looping over it (Part I, 6.8). `NK2302` a parameter written `ref String` is kept past the call it was given in, and nothing names the buffer it views (Part I, 6.6). A view inside a struct carries the buffer it points into and a naked one does not, so the message names the struct form as the way out. Reported where the destination names no buffer: a field of a subject that holds no view, a result that may point into another buffer as well, a view given to a task, fields of two struct parameters. Where the destination is a field of a subject that holds a view, a field of one struct parameter that holds views, or a struct literal handed back as the result, the program is lowered instead, with the parameter written as a view of that buffer. `NK2303` is **not a refusal**: a view of a buffer a body made may be handed out of it, and the buffer lives in the keep of whatever keeps its views. `NK2304` what the tether cannot lower: one buffer handed both to a task and out of the function, which has no one owner; and a container of **structs** holding views that drops entries inside a loop, where the container is not a list or a view in the struct is not text (a list of structs of text views is held, one handle per buffer). In that last case a call on the subject is treated as keeping what it is given, which narrows the signature rather than refusing. |
| `NK24xx` | Contract changes | `NK2401` a borrow contract change broke a caller, narrated from the ledger diff (13.5). `NK2402` (**warning**) an error that **newly reaches a `catch`**, narrated from the same diff: a handler takes everything that reaches it, so nothing is refused. The warning is at the call rather than at the `catch`, once, and the committed ledger diff is the acknowledgement. Reserved: the same over a boolean. `NK2403` (**warning**) a parameter whose `keeps` changed since the committed ledger, on a type whose teardown does something (`Drop` or `Cleanup`, on it or on a field): the call that hands it a name is told where that name's cleanup runs now, once. **A backend error at a boundary** — *is not a future*, a `?` on a value that is not a `Result`, a `Result` where none was expected — is reported as *the ledger of `X` does not match its sources*, or *the description of `X` does not match the crate*, with the backend's words as a note (ADR-100 D6). |
| `NK25xx` | Portability | The `Send` rules that parallel code needs, decided the same way at **both** values of `user_parallelism`, so that a library built at one stays usable at the other, and asked of a value **and a destination**, so the two codes below may answer differently: a lock goes into a task of the program's own and not into code nothing describes. `NK2501` a value that may not cross a thread is used by a task (Part II, 11.2): an **error** at `user_parallelism = yes` and a **lint** at `no`. `NK2502` a value that may not cross a thread is handed to a call this compiler cannot see the end of: an error at both values. Worked through in C.5. `NK2503` a call into foreign code from which a **lock** is reachable through its arguments, transitively and through the fields of a struct. A call that can reach no lock is allowed without a word, and the way out of one that can is keeping the lock out of its reach (15.2). Worked through in C.6. `NK2504` a call, or a written type, reaching into a **Rust crate no ledger describes**. Once per crate, and only where the manifest declared the crate with `type = "rust"` (C.4). `NK2505` a described crate whose **sources have moved** since the description was reviewed: the description is reviewed again, not derived again. Once per crate, and only where a recorded hash disagrees; a crate declared by version, and a description that recorded no hash, answer *nothing moved*. |
| `NK26xx` | Failure declaration, resource cleanup & crash path | `NK2601` an `impl Cleanup` that is not the one method `fn cleanup(ref mut self)`: another method, another shape, or a `drop`, which the message points at its own `impl Drop` (Part I, 6.4). `NK2602` is retired: a value with a pausable cleanup dying where nothing may pause is the pause rules' own case — `NK2202` in a door, `NK2209` in a `par_iter` lambda, `NK2206` in a lambda handed to a `sync` function type — named as *the cleanup of `f`* ([ADR-297](adr/adr-297.md) D3). `NK2603` is **retired** and not reused: an expired `cleanup-deadline` is exit status 70 with the message on the panic path (Appendix A, [ADR-297](adr/adr-297.md) D7-D8). `NK2604` only the application may set the panic hook, and the hook must be `sync` (Part I, 7.2). `NK2606` a lambda that can **fail**, handed to a parameter whose function type does not say `throws`: `NK2605`'s shape one level over, reported alone, without `NK2605` beside it. Where the type does say `throws`, the failure travels to the caller and `NK2605` applies. `NK2605` a **written** call that can fail, in a function that does not declare `throws` (Part I, 7.1): answered from the ledger (13.5), so it says which contract it read. The cleanup the compiler writes where a value dies is a call too, and a failing one is `NK2605`'s, named as *the cleanup of `f`* (Part I, 6.4). The same rule as `NK2701`. |
| `NK27xx` | Implicit calls and sequences | `NK2701` a loop whose step can fail, in a function that does not declare `throws`. The same rule as a cleanup's `NK2605`: where the language performs a call nobody wrote, a failure of it fails the enclosing function. `NK2702` a sequence used after something took it — a walk, a `let`, an argument, or a loop or lambda that takes it again on its next turn. `NK2703` a sequence asked for what it is not: walked from the back where it has no back end, or its length asked where it is not known. |

Every NK code the compiler emits has a reproduction test and a worked example in the relevant chapter.

A code specified ahead of its check has no reproduction test.

`NK2605` is reported for a call whose callee a ledger describes: a function in this program, one in another module of it, or one of `std`'s, by name or as a method on a receiver whose type is known. A call nothing describes is silence rather than approval, which is C.4's property for every check here.

### C.4. What a Type Error Looks Like

Two of the `NK1xxx` family, on a file that says `io::read_to_string("input.txt")` and puts a view of text it was handed in a published `String` field that text of its own also flows into:

```text
error[NK1106]: `Reading.name` holds `String`, but you're giving it `ref String`.
  --> app.nika:8:5
   |
 8 |     return Reading { name: city, temp: 12 }
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: `city` is declared `ref String`, so the text belongs to the caller, who still has it.
   = note: `Reading` keeps its `name` after this line, so it needs text of its own.
   = note: Nikaia never copies text behind your back: a copy costs as much as the text is long, so it's written where it happens.
   = help: Declare `city: String` so the caller hands its text over, or write `city.clone()` to copy it here.
error[NK1101]: `io::read_to_string` takes 0 arguments, but you passed 1.
   --> app.nika:12:5
    |
 12 |     let text = io::read_to_string("input.txt")
    |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    |
    = note: It's declared as `io::read_to_string() -> String`.
    = help: Call it without arguments: `io::read_to_string()`.
```

**The note is the contract**, quoted from the ledger: the compiler shows the caller what the callee promised. **The underline is on the statement**, not the expression: this checker and `NK2202` report at statement granularity, and a message that knows the word it is about underlines that word. **Where the compiler could have written the fix and did not, it says why**: the text refusal names whose text it is, what keeps it, and what a copy made on its own would cost, and leads with the answer that copies nothing. **The help is paste-ready**, as C.2 requires: `.clone()` for text, `as i64` between numbers, and the nearest existing field when a name is close to one that exists.

A message appears only where **both** sides are written down. Where a type is not known, such as a method on a receiver `std` has no signature for, or what a `?` unwraps, the compiler says nothing, which is not the same as approving. The checker never rejects a program that is correct.

**A hole is code, and is checked like code.** The expression inside `f"total is {stock::total(items)}"`,
and inside a `dsl html` template's `{…}`, goes through the same path as a statement, so
`NK1101` and the rest say the same thing about it that they would say about the same expression
written on a line of its own. The `sync` analysis reads holes too, in both directions: a pausing
call inside one costs an inferred `sync` and contradicts an asserted one. A hole whose text does
not parse is a parse error in an `f"…"` ([ADR-309](adr/adr-309.md) D8) and is reported by the emitter in a template, and the checker raises no second error for it.

### C.5. What a Crossing Refused Looks Like

The `NK25xx` codes cover the places a value the program wrote reaches another thread. One question decides them, **may a value of this type go to this destination?**, asked of the value's type and of where it is going. Each destination's answer does not depend on which build this is.

The codes do not share one verdict. Into a task of the program's own a lock may go, at both values. Into code nothing written down describes it may not, at both values. `NK2501` therefore refuses nothing a Nikaia program can write, and `NK2502` refuses the shared value — a `Shared[T]`, whose count is chosen per value and so has no one shape a foreign signature could name, and a described type that says `crosses = false`. **A lock is the third, and it has a code of its own**: the refusal there is about the call and not the value, so it is `NK2503` and C.6 (15.2). One walk answers all three, and the reason it returns picks the code: a `Shared[T]` handed to an undescribed call gets `NK2502`, and a `SharedMut[T]` or a `Locked[T]` gets `NK2503`. No type a program can write reaches `NK2501`'s shape: the `Held` below is a type nothing describes.

A task runs somewhere else, so everything it uses goes with it:

```text
error[NK2501]: This task uses `counts`, which can't be passed to a task.
  --> app.nika:7:5
   |
 7 |     spawn fn { total(counts) }
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: A task runs on its own thread, so everything it uses has to be able to move between threads.
   = note: `Held[i64]` is described as a value that can't move to another thread.
   = note: This is checked even when the program runs on one thread, so code stays correct when `user-parallelism` is turned on.
   = help: Keep it here, and pass the call only what it needs from it.
```

A lock goes into a task of the program's own, and everything else a program can write goes with it. The shape above is what a type that answers *may not* gets.

A call whose body the compiler cannot see may start a thread of its own (15.2):

```text
error[NK2502]: `counter` can't move to another thread, and `hyper_shim::across_a_thread` might move it.
   --> app.nika:14:5
    |
 14 |     let crossed = hyper_shim::across_a_thread(counter)
    |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    |
    = note: The compiler knows nothing about `hyper_shim::across_a_thread`, so it has to assume it might start a thread of its own.
    = note: `Shared[i64]` is shared, and how a shared value is counted is decided per value, so code outside Nikaia can't be written to handle it.
    = note: This is checked even when the program runs on one thread, so code stays correct when `user-parallelism` is turned on.
    = help: Pass what the shared value holds, a view or a copy, instead of the shared value itself.
```

**The way out is one line.** The caller passes what is inside — a view of it or a copy — so the foreign function sees an ordinary value (Part I 6.2). The same shape is the way out of `NK2503` (C.6). The message states that the refusal is deliberate.

The pair follows four rules.

**The rule is structural and transitive.** A `struct` with one field that may not cross may not cross, and the note names the field that decided it rather than the struct. The fields come from the ledger (13.5), so the rule reaches a type declared in another file.

**Three answers.** A type may cross, may not, or **nothing written down says**. The third is not permission and not a refusal (C.4). An undecided crossing is handed on: `rustc` still type-checks the emitted crate, and the trait-bound error it raises is reported against the `.nika` line by the translation C.1 requires. A library may settle one of its own types with a line in its ledger. The same third answer governs whether a call into foreign code can reach a lock (15.2): an undecided type is not a type with no lock in it, and a call is allowed silently only when its arguments reach nothing.

**One verdict per destination, two severities.** At `user_parallelism = no` no user code runs concurrently, so the task above does not run and `NK2501` is a lint, with a note saying which of the two it is. `NK2502` is not downgraded, because a foreign runtime's threads run whatever the build option says. Each destination's verdict is the same at both values; only the severity differs.

**The crossing the compiler chooses for itself gets no diagnostic.** Statement overlapping puts each of a pair inside a closure that runs elsewhere, so what the pair hands back crosses a thread. Where that cannot be shown, the statements keep the order they were written in, and `--overlaps` lists it among the other refusals. That closure is the program's own code, the same destination a task is: a result that holds a lock overlaps.

### C.6. What a Refused Lock Looks Like

Five refusals come with the rule that a lock may not be taken while a lock is
held, with the doors shared mutable state is reached through, and with a foreign
call that could reach a lock. Each names a way out, as C.2 requires, and each
way out is one line of code. The shapes below are what the codes print (C.3).

A lock taken inside a lock, which is what `access_all` exists for (Part II, 12.3):

```text
error[NK2203]: `update` takes a lock, but you're already holding one here.
  --> main.nika:9:19
   |
 9 |         account_b.update fn(mut to) { to.balance += 100 }
   |                   ^^^^^^ takes a second lock
   |
   = note: This block runs while a lock is held, so taking a second lock inside it can deadlock.
   = help: Take both locks at once with `access_all(a, b) fn(x, y) { … }`, or compute the value before the block and pass it in.
```

A chain reads the same, with one note more: it names the call, and the function
inside it that takes the lock. A scope opened inside the block is the same
refusal: the scope waits for its tasks, so a task waiting for the held lock
waits for the block that is waiting for it.

The two that come with the doors:

```text
error[NK2204]: You can't assign to `kasse` directly: it holds shared mutable state.
  --> main.nika:4:5
   |
 4 |     kasse = 42
   |     ^^^^^^^^^^
   |
   = note: Changing shared state goes through a method that takes the lock.
   = help: Write `kasse.set(42)`.
```

```text
error[NK2205]: This `set` stores a value that was read from `kasse` earlier, so the lock is taken twice.
  --> main.nika:6:5
   |
 6 |     kasse.set(kasse.get() + 100)
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: A value read from a lock is out of date as soon as the lock is released: anything can change between the read and the store.
   = note: This is found wherever the value was read: on the line above, in another function, or in another request.
   = help: Write `kasse.update fn(mut v) { … }`, which reads and changes the value while holding the lock.
```

The second one reads the stamp a value carries out of a lock: the inline `get`
is its smallest case, and the same pair spread over two lines, two functions or
two requests is the same refusal, with the note naming where the value was seen.

The lowering of a door, written as a door. `set_after` is a function in the
language below; written directly it would take the program's two arguments,
drop the failure nobody declared, and leave the backend to report against a
generated file (C.1):

```text
error[NK2208]: `kasse` has no `set_after`. Write `set(…; after: …)` instead.
  --> main.nika:5:5
   |
 5 |     kasse.set_after(stand + 1, stand)
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: `set` with `after:` compares and stores while holding the lock once.
   = note: Written like this, a failure would be silently dropped.
   = help: Write `kasse.set(neu; after: seen)`, where `seen` is the value you read from the lock.
```

The foreign call, judged by what its arguments can reach (15.2):

```text
error[NK2503]: `hyper_shim::render` could reach a lock through `state`.
   --> main.nika:12:5
    |
 12 |     hyper_shim::render(state)
    |     ^^^^^^^^^^^^^^^^^^^^^^^^^
    |
    = note: The compiler knows nothing about `hyper_shim::render`, so it can't see what it does with what you pass it.
    = note: `state.counts` is a `SharedMut[i64]`, and code that might keep a lock could deadlock.
    = note: This is checked even when the program runs on one thread, so code stays correct when `user-parallelism` is turned on.
    = help: Pass it a copy of what it needs instead of the container:
           hyper_shim::render(state.counts.get())
```

The note names the **field** that decided it rather than the struct, as
`NK2501`'s does: the fields come from the ledger (13.5), so the rule reaches a
type declared in another file. A call whose arguments can reach no lock gets no
diagnostic and no note. It is allowed silently, which is not the same as a call
whose contents nobody wrote down (C.5).

The walk is `NK2502`'s, asked once per argument. The way out prints the path it
found — the argument's own name with the field that decided it behind it — where
the argument is a name or a chain of fields, and states the general form
otherwise.
