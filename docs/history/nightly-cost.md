# What the pinned nightly was for, and what it cost

**Date:** September 12, 2026
**Status:** measured, and kept. Everything this file measures — the pinned
nightly, the Bridge-IR backend, the `rustc_ast` path and `-Zpolonius=next` — has
since been **withdrawn** from the project. The withdrawal was decided *without a
further measurement*, and every number below is the number it was decided on.
Nothing here is revised: this is the notebook page, and a notebook page is not
edited when the experiment ends. What happened, and why, is
[`../CHANGELOG.md`](../../CHANGELOG.md) and
[`withdrawn-one-way-down.md`](withdrawn-one-way-down.md).
**Related:** [ADR-001](../specification/adr/adr-001.md) D1 (the toolchain, stable,
as this file's third conclusion wanted), [ADR-005](../specification/adr/adr-005.md)
D2 (Group B.2, now the frontend's desugaring),
[ADR-021](../specification/adr/adr-021.md) D9 (the precedent for refusing a backend
by name), [`subprocess-cost.md`](../subprocess-cost.md) (the same machine, the same
method, the adjacent question)

At the time of writing, the project pinned one exact nightly `rustc` and justified
it with two things: `rustc_private`, and `-Z` flags. This file asks what each is
actually for, what it buys, and what it costs in the three terms a person
installing a compiler feels — how big, how long, and which tools they must
already have.

**The three conclusions, first.**

1. **`rustc_private` is the whole of it.** Two `#![feature(…)]` attributes exist in the
   workspace and both are `rustc_private`; the Rust that Stage 0 emits uses no unstable
   feature at all. Everything except the Bridge-IR backend builds, tests and runs on a
   **stable** toolchain, and does so in CI now.
2. **`-Z` buys nothing, because no `-Z` flag is passed.** `-Zpolonius=next` is named as a
   premise by three records and has never been on a command line. Measured across the
   thirty programs this repository lowers, it changes **zero** verdicts, and it would cost
   **+7.1 %** of the instructions a full build of the largest of them retires.
3. **For a *user*, the nightly costs almost no bytes and all of the flexibility.** A
   minimal stable toolchain is 577 MiB and a minimal pinned nightly is 579 MiB — two
   megabytes apart. What the pin costs a user is not size: it is that a bridge-enabled
   `nikaia` binary links `librustc_driver-<hash>.so` by **absolute path**, so it runs only
   beside that one nightly, installed where the build machine had it.

---

## 1. The machine and the method

Intel Xeon @ 2.80 GHz, 4 vCPU, 15 GB RAM, Linux 6.18.44 x86_64 — a **shared virtual
machine** with no `cpufreq` governor exposed, and the same box
[`subprocess-cost.md`](../subprocess-cost.md) used. `rustc 1.94.0-nightly (8d670b93d
2025-12-31)`, which is the `nightly-2026-01-01` `rust-toolchain.toml` named at the time;
stable is `1.94.1 (e408947bf 2026-03-25)` for
the builds and `1.98.1 (48a229cea 2026-09-01)` for the installs, because an install measures
what a person downloads *today* and a build has to match the rlibs already on disk.

`docs/staging-candidates.md` §2 sets this repository's standard: *wall-clock on a shared
machine is not a measurement; an instruction count is deterministic and diffable*. So §3's
Polonius decision rests on callgrind, and the wall-clock beside it says what those counts
feel like. Where a duration is quoted it is the **median of three runs from nothing** —
`rm -rf target` before each build, a fresh `RUSTUP_HOME` before each install — with the
spread given, because a median of three on this box is worth about ±5 %.

**What "the corpus" means here** is wider than in `subprocess-cost.md`, because a borrow
checker has to be offered everything that has a body. Every `.nika` file the bootstrap
compiler can lower: **ten** of the eleven `examples/*.nika` (all but `fortunes.nika`, whose
`dsl postgres` block it refuses), the three files of `examples/inventory`, the four
`examples/foreign-runtime` programs (`overlaps.nika` and the `crossing`, `serve` and
`smuggled` projects), three `tests/samples/`, `crates/nikaia-std/src/text.nika`, four
`crates/nikaia/tests/fixtures`, and the five of the twenty-six `tests/errors/` files that
get past the parser — **thirty** lowered crates. Of those, 24 reach the borrow checker and
6 stop earlier, at a parse or resolution error, which is what they are in the corpus for.

---

## 2. What nightly is needed for, by name

Two `#![feature(…)]` attributes in the workspace, and both are the same one:

| where | why |
| :--- | :--- |
| `crates/rustc-executor/src/lib.rs` | `#![feature(rustc_private)]` — the crate built `rustc_ast` and printed it |
| `crates/nikaia/src/main.rs` | the same attribute and `extern crate rustc_driver`, so the binary links the compiler's dylib set rather than a second copy of `std` |

**No `-Z` flag is passed anywhere in a build.** The only one in the tree was
`-Zls=root` in `crates/nikaia/tests/common/mod.rs`, which read an rlib's dependency list to
pick matching copies out of the deps directory — a *test harness* dependency on the
nightly, not a compiler one, and the thing that made a stable-toolchain run of the suite
impossible. The same question is now answered without a flag, by asking which candidate
rlib's filename hash occurs in the anchor's metadata, so the workspace passes no `-Z` at
all.

**The Rust that Stage 0 emits uses no unstable feature.** All thirty lowered crates compile
on the stable toolchain, which `cargo test -p nikaia --no-default-features` on stable is
what proves (§5).

### What the second `#![feature]` really was

`main.rs` carried the attribute unconditionally, and `[[bin]] name = "nikaia"` carried
`required-features = ["rustc-backend"]`, so there was no binary without the bridge and the
whole compiler needed the nightly. Neither was a technical constraint: `rustc_executor` is
named in exactly one place in `main.rs`. Both are now conditional on the feature, the
`required-features` line is gone, and `cargo build -p nikaia --no-default-features`
produces a compiler on stable. `rustc_private` stays confined to `rustc-executor` — the
frontend never gained any.

---

## 3. `-Zpolonius=next`: what it would buy, and what it would cost

[ADR-005](../specification/adr/adr-005.md) D2 names one situation — conditional return of a
borrow, the `get_or_insert` shape — as rejected by the stable borrow checker and accepted by
`-Zpolonius=next`. Three records rested on the flag at the time — the toolchain pin, ADR-005
§1's Group B.2 and the [ADR index](../specification/adr/README.md) — while ADR-005 §5 already said
the case is **not built**. What nobody had done is compile something twice and count.

### 3.1 The positive controls, because a harness that finds nothing must first find something

Four hand-written Rust programs, compiled with `--emit metadata` (which runs borrowck) on
the pinned nightly, with the flag and without:

| control | NLL | `-Zpolonius=next` |
| :--- | :--- | :--- |
| conditional return out of a `HashMap` — D2's own `get_or_insert` | `E0502` | **accepted** |
| the same shape on a `Vec` | `E0502` | **accepted** |
| a borrow live on one path and dead on the other, returning the other input | `E0503` | **accepted** |
| `fn last(&mut Node) -> &mut Node`, the linked-list walk | accepted | accepted |

So the flag does what D2 says, on the toolchain D1 pins, and the harness can see it. Three
of the four flip; the fourth is a case NLL already solved, which is worth keeping in the
table because it is the shape people still quote as needing Polonius.

### 3.2 The corpus: thirty programs, zero differences

Every lowered crate from §1, compiled `--crate-type lib --emit metadata` twice:

| | programs |
| :--- | ---: |
| accepted by both | **24** |
| rejected by both, with byte-identical error codes | 6 |
| accepted by `-Zpolonius=next` and rejected by NLL | **0** |
| accepted by NLL and rejected by `-Zpolonius=next` | **0** |

The six rejected by both are rejected before borrowck ever runs — `expected expression,
found ;` three times, an undefined grammar rule, a `until(…)` that runs through its
boundary, and a missing `digit_value` import — which is what four `tests/errors/` files and
two fixtures are in the corpus to do.

**Zero is the finding, and the reason it is zero is in the corpus itself.** `examples/1brc.nika`
and `examples/access-log/src/main.nika` both want exactly D2's situation — look a key up, add it if
it is not there, keep using it — and both write it as
`.entry(k).and_modify fn { … }.or_insert_with fn { … }`. That is the **entry-style API**
D2 names as its own fallback ("if the alpha regresses, the fallback is desugaring the known
patterns onto entry-style APIs in the frontend"). The fallback is not a contingency; it is
what the corpus already does, and the language's own `HashMap` surface is what makes it the
natural way to write it.

### 3.3 But the shape is reachable from Nikaia, and that is the other half

A thirteen-line Nikaia program reaches it:

```nikaia
impl Cache {
    fn first_or_push(&mut self, v: i64) -> &i64 {
        let found = self.entries.get(0)
        if found.is_some() {
            return found.unwrap()
        }
        self.entries.push(v)
        return self.entries.get(0).unwrap()
    }
}
```

Stage 0 lowers it, NLL rejects the emitted Rust with `E0502`, and `-Zpolonius=next` accepts
it. So Group B.2 is a real situation in this language and not only in Rust's issue tracker —
it is simply not in anything the repository contains. Two neighbouring shapes are *not*
reachable, and the reasons are worth writing down because they are both defects rather than
design:

* the same thing over a `HashMap[&str, …]` is rejected by **both**, with `E0621`: Stage 0
  elides the key's lifetime in `fn get_or_insert(&mut self, key: &str)` where the field it
  inserts into is `HashMap<&'a str, …>`. The program never reaches the borrow checker's
  flow analysis, because elision gets there first. **Still open, and §3.3.2 says why it is
  a decision and not a patch;**
* writing the branch as a `match` with `return x` in an arm emits `x` — Stage 0 **drops the
  `return`** from a match arm whose body is a value, turning a control-flow statement into
  the arm's value. Here it surfaces as `E0308` (incompatible arm types); in a function
  returning nothing it would surface as a silently different program. `return` with no
  value in an arm is kept, so the bug looked narrow. **Fixed, and it was not narrow:
  §3.3.1.**

### 3.3.1 The dropped `return`: what it actually was, and the four shapes it reached

The cause was not in `match`. One `bool` named `tail` carried two questions at once through
`crates/nikaia/src/emit/mod.rs`: *is this statement the block's value* — which decides the
semicolon, and is a fact about Part I 3.1 — and *is this block's value the function's return
value*, which is what licenses writing `fn f() -> T { return x }` as `{ x }`. The second is
true of a function body. It is false of every block that is a value handed to the expression
around it, and `expr()` handed all of them `tail = true` on the way in, because at that point
it had only the one flag to hand them.

So the rewrite fired everywhere a block sat in value position, and the same one line of
`Stmt::Return` accounts for four shapes. Probed one at a time, before and after:

| where the `return` is | emitted before | emitted after |
| :--- | :--- | :--- |
| the end of a `match` arm | `1 => { 10 }` | `1 => { return 10; }` |
| the end of an `if` branch whose value is taken | `let x = if c { 1 } else { 2 };` | `let x = if c { return 1; } else { 2 };` |
| the end of a `catch` handler | `Err(error) => { 0 }` | `Err(error) => { return Ok(0); }` |
| the end of a block or `seq` block used as a value | `let x = { 1 };` | `let x = { return 1; };` |
| the end of a loop body, a `for` or a `while` | `return i;` | unchanged |
| the end of a function body, or of a lambda | `{ x }` | unchanged |

The last two rows are the point of the table: a loop body was never in value position, and a
function body's tail is the one place where the value *is* the function's, so the rewrite that
was right stays. Fixing the cause is one three-state `Tail` in place of the `bool` —
`Statement`, `Value`, `Return` — and the rewrite is allowed only at `Return`.

**The `catch` row is the one to read twice.** [ADR-034](adr/adr-034.md) D1 is
about exactly that handler: one that can `return` makes the next statement conditional on the
guarded operation having succeeded, so the two may not be overlapped, and
`contracts::order`'s `diverts` counts any `return` anywhere in the handler when it refuses. It
was counting a `return` the emitter then deleted — an analysis reasoning about a control flow
the emitted program did not have. It is conservative in the safe direction (it refuses an
overlap the program did not need) so it produced no wrong schedule, but the two halves of the
compiler disagreed about what the handler did, and only one of them was right.

**Why it was silent, which is the part the `E0308` above understates.** The failure mode
depends on whether the two readings happen to agree on a type, not on whether the function
returns anything:

```nika
fn announce(n: i64) {
    match n {
        0 => { return note(n) }
        _ => { println(f"n is {n}") }
    }
    println("checked")
}
```

`announce(0)` printed `checked`. Nothing in rustc has anything to say about it, because both
readings of the arm are `()`; the only witness is the output. The same holds in a
value-returning function whose arms agree on a type, and in the `catch` handler above whose
fallback has the type the read has. `E0308` is what you get when the types happen to
disagree — the loud minority of the cases, and the reason this was recorded here as narrow.

`crates/nikaia/tests/returns.rs` is that program and five others, lowered, compiled as Rust,
run where there is something to run, and compared against what the source means. All six fail
on the commit before the fix, and **four of them fail as a wrong printed answer** and not as a
compile error: the `match` in a function returning nothing printed `checked` as well, the
`match` in one returning a value answered `30 30` for `10 30`, the `if` answered `101 102` for
`1 102`, and the `catch` handler answered `read 7 bytes` for `missing`.

**One shape's verdict changes, and it is worth naming.** A `let` bound to a block that returns
unconditionally — `let x = { return 1 }` — used to compile, because the lowering quietly turned
it into `let x = { 1 }`. It now lowers to `let x = { return 1; };`, whose binding has no value
and which rustc says so about. That is an honest complaint about dead code in place of a
different program, and nothing in `examples/`, `tests/` or `crates/nikaia-std` writes it: the
whole corpus is unchanged at both switches, on the pinned nightly and on stable, at dev and
release, and `tests/errors/EXPECTED.txt` does not move. What *does* change in the corpus is
`examples/json/src/main.nika`'s `longest`, whose four arms each end in a `return` and now say so; it
compiles and prints what it printed.

### 3.3.2 The elided key lifetime: not fixed, because the fix is a decision

The `E0621` above is one line to make go away and the wrong line to write. Stage 0 spells a
view's lifetime by **position** and by nothing else
([ADR-011](adr/adr-011.md) D6, which is
[ADR-008](adr/adr-008.md) in the only form a bootstrap compiler can express it):
named inside the grammar module and on the structs it builds, elided in a free function's
signature, and — in a method of an `impl` whose receiver holds views — elided on the `&` while
every named type takes `'a`. That third spelling is what makes `examples/1brc.nika` work:
`fn record(&mut self, m: Reading)` becomes `m: Reading<'a>`, and the key reaches
`HashMap<&'a str, Stats>` inside the struct it came in.

It has no answer for a **bare `&str` parameter**, because that parameter is two different
things and the signature cannot say which:

* a key that will be **stored** — `fn get_or_insert(&mut self, key: &str)` inserting into
  `HashMap<&'a str, …>` — needs `&'a str`, and gets `E0621` without it;
* a view that will only be **inspected** — `fn note(&mut self, probe: &str)` — must *not* have
  `'a`. Checked rather than assumed: with `&'a str` on that parameter, a caller that builds
  the probe locally and hands the receiver back to its own caller is refused with `E0515`,
  and the same program with the lifetime elided compiles and runs. A shared-reference method
  is not affected either way, because `&Cache<'a>` is covariant in `'a` and `&mut Cache<'a>`
  is not — so the shape that breaks is precisely `&mut self` plus a view parameter that is
  never stored.

One spelling cannot serve both, and the source writes no lifetime to choose with
([ADR-005](../specification/adr/adr-005.md) D1, ADR-008 D1) — so the **use** has to decide, and
deciding it per view is ADR-008 D2's tether lattice, whose "least state that makes the program
valid" is the same question one level up and which that record's own status line says is not
built. Two things are the owner's:

1. whether the answer comes from the beginnings of that solver in Stage 0, or from a fourth
   positional rule — and if the latter, which programs it is allowed to start refusing, since
   `'a`-everywhere refuses the `E0515` shape above and elision refuses the `E0621` one;
2. if a fourth positional rule, whether the method declares a lifetime of its own —
   `fn get_or_insert<'k>(&mut self, key: &'k str)` with `'k: 'a` — which makes the signature
   carry a lifetime relation the source never wrote, and is what ADR-008 D9 rejected explicit
   regions for.

A partial fix is worse than none here: it would make the storing shape compile and leave the
inspecting one failing differently, in a compiler whose whole claim about lifetimes is that
the position decides and the author never writes one.

**And `E0621`'s *text* is not translated.** [ADR-005](../specification/adr/adr-005.md) D7 now
enumerates it — E0382, E0499, E0502, E0505, E0506, E0597, E0716, E0621, plus E0277 added after
the foreign-runtime experiment — and when this was written it did not, which is what this
paragraph found. Being in the enumeration is worth exactly what it is worth:
`crates/nikaia/src/diagnostics` translates the *place* for every code, so the message already
landed on the `.nika` line and still does. The **text** does not, and here the text is
`help: add explicit lifetime 'a to the type of key`, whose every noun is something ADR-008 D1
says the author never writes. That is D7's own `E0277` failure at a second code — the half it
records as open — and it is a second reason this is a record's business and not a patch's.

### 3.4 What it would cost

`valgrind --tool=callgrind`, one run each, deterministic:

| | Ir, NLL | Ir, `-Zpolonius=next` | delta |
| :--- | ---: | ---: | ---: |
| `hello_world.rs` (589 B), `--emit metadata` | 25 838 798 | 25 924 473 | +0.33 % |
| `json.rs` (6 393 B), `--emit metadata` | 818 392 914 | 942 296 839 | **+15.1 %** |
| `json.rs`, **full debug compile** | 1 731 303 418 | 1 854 471 352 | **+7.11 %** |

The full-compile row is the one to quote, and it cross-checks
[`subprocess-cost.md`](../subprocess-cost.md) §4, which measured the same file's full debug
compile at 1 719 M Ir — 0.7 % apart, a day and a flag apart.

Wall-clock beside it, median of nine after two warm-ups, milliseconds:

| program | NLL | polonius | delta |
| :--- | ---: | ---: | ---: |
| `hello_world` | 26 | 26 | 0 |
| `tally` | 38 | 38 | 0 |
| `k-nucleotide` | 124 | 130 | +6 |
| `1brc` | 128 | 134 | +6 |
| `json` | 246 | 268 | +22 |

So: **+7.1 % of a build, on every program, to accept a program nobody in this repository has
written.** And it would cost one more thing that is not a percentage: the flag is `-Z`, so
passing it would put *every user's* build on the pinned nightly, where today only the
bridge backend's own compilation is. That is the trade the records should be read against,
and it is why the correction in ADR-005 D2 left the classification where D2 put it rather than
reversing it.

---

## 4. What nightly costs, as the three things a person installs

Fresh `RUSTUP_HOME` each time, three runs each, `du -sm` of the whole rustup home.

| what | median install | spread | on disk |
| :--- | ---: | ---: | ---: |
| stable, `--profile minimal` | **11.05 s** | 10.85–11.35 | **577 MiB** |
| the pinned nightly, `--profile minimal` | 11.37 s | 10.80–11.81 | 579 MiB |
| stable, minimal + `rustfmt` + `clippy` | 12.30 s | 11.96–13.51 | 602 MiB |
| the pinned nightly, all four components `rust-toolchain.toml` names | **24.73 s** | 24.72–26.54 | **1 420 MiB** |

The gap between the last two rows is **+12.4 s and +818 MiB**, and it is almost entirely one
component. Measured by summing the on-disk size of the files each component's own manifest
lists:

| component | files | on disk |
| :--- | ---: | ---: |
| `rustc-dev` | 3 085 | **654.3 MB** |
| `rustc` | 41 | 388.6 MB |
| `llvm-tools-preview` | 16 | 194.6 MB |
| `rust-std` | 35 | 174.8 MB |
| `cargo` | 43 | 42.1 MB |
| `clippy-preview` | 5 | 21.2 MB |
| `rustfmt-preview` | 5 | 9.0 MB |

Two notes on that table. **`rustc-dev` is 654 MB and not the 931 MB that had been quoted** —
931 MB is `lib/rustlib/x86_64-unknown-linux-gnu`, which holds `rust-std`'s rlibs too. Of the
654, **41 MB is `rustc-src`**: 2 829 of `rustc-dev`'s 3 085 files are rustc's own sources,
which `rustc-dev` installs and nothing in this workspace reads. And
**`llvm-tools-preview`'s 194.6 MB is almost all `libLLVM.so`, which the `rustc` component
ships anyway** (the two are hardlinks); its own contribution is the fourteen `llvm-*`
binaries, about 8 MB, and nothing in the repository invokes one. It is declared in
`rust-toolchain.toml` and is not obviously earning its line — small enough that this file
records it rather than acting on it.

### 4.1 The compiler's needs and the user's are not the same thing, and this is the crux

**What *builds* `nikaia` needs**, per `rust-toolchain.toml`: 1 420 MiB and 24.7 s, of which
`rustc-dev` is 654 MB — and it is needed only to compile `rustc-executor` against
`rustc_ast`.

**What *running* a `.nika` file needs** is a different list, because the emitted Rust is
ordinary stable Rust and a project build hands it to `cargo`:

| the binary they were given | what the user must have | install | on disk |
| :--- | :--- | ---: | ---: |
| built `--no-default-features` (no bridge) | **any** recent stable toolchain | 11.05 s | 577 MiB |
| built with the bridge | `nightly-2026-01-01`, **at the path the build machine had it** | 11.37 s | 579 MiB |

Two megabytes apart, which is the opposite of the way the question is usually framed. The
real cost is in the second column's small print, and `readelf`/`ldd` are where it shows:

```
$ ldd target/release/nikaia                      # built with the bridge
    librustc_driver-bcf6c5c396c0890f.so => /root/.rustup/toolchains/nightly-2026-01-01-…/lib/…
    libLLVM.so.21.1-rust-1.94.0-nightly          => …
$ readelf -d target/release/nikaia | grep RUNPATH
    Library runpath: [/root/.rustup/toolchains/nightly-2026-01-01-x86_64-unknown-linux-gnu/lib]

$ ldd target/release/nikaia                      # built --no-default-features
    libgcc_s.so.1, libc.so.6, ld-linux-x86-64.so.2
```

So a bridge-enabled `nikaia` is **not a relocatable binary**: the soname carries a
per-build hash (a stable install has `librustc_driver-83018425804cb0fc.so`, which is the
wrong file), and the runpath is an absolute path into whoever built it. A compiler without
the bridge links libc and nothing else, and 4 037 608 bytes of it is the whole thing against
3 830 568 with the bridge — it is *larger*, because it carries its own `std` instead of
borrowing rustc's.

### 4.2 Building the compiler, timed both ways

Clean `release` build of the `nikaia` binary, `rm -rf target` first, three runs:

| | median | spread | binary |
| :--- | ---: | ---: | ---: |
| nightly, with the bridge | 39.13 s | 38.90–43.19 | 3 830 568 B |
| stable, `--no-default-features` | **38.02 s** | 37.76–38.76 | 4 037 608 B |

They are the same build, inside the noise of this box. The first measurement taken put the
stable one at 52.6 s, and repeating it is the only reason this table does not say that the
bridge makes the build faster. **Build time is not a term in this decision**; install size
and toolchain identity are.

---

## 5. A stable-only installation, and what was needed to make one

`[[bin]] name = "nikaia"` had `required-features = ["rustc-backend"]`, so a build without the
bridge produced a library and no compiler. Four changes, none of them in a frontend:

1. `main.rs`'s `#![feature(rustc_private)]` became `#![cfg_attr(feature = "rustc-backend",
   feature(rustc_private))]`, and `extern crate rustc_driver` and the Bridge-IR frontend
   are now behind the same `cfg`;
2. the `required-features` line is gone, and the bridge arm of the backend match is two
   functions, one per `cfg`;
3. the absent one **fails by name** — [ADR-021](../specification/adr/adr-021.md) D9's rule about
   `cranelift`, applied to a backend that was configured out rather than never written —
   and the message says which feature, which toolchain component, and roughly what it
   weighs. `crates/nikaia/tests/backend_absent.rs` asserts that, and asserts that the
   `rust` backend still lowers, so the first test cannot pass on a compiler that refuses
   everything;
4. `crates/nikaia/tests/common/mod.rs` no longer passes `-Zls=root` (§2), which is what let
   the suite run at all on stable.

The result, on this box:

| | |
| :--- | :--- |
| `rustup run stable cargo build -p nikaia --no-default-features --release` | 38.02 s, 4 037 608 B, links libc only |
| `rustup run stable cargo test -p nikaia --no-default-features` | **all green**, 0 failures |
| the same `--release` | all green, 0 failures |
| `rustup run stable cargo clippy -p nikaia --no-default-features --all-targets -- -D warnings` | clean |
| `--backend bridge` on that binary | refused by name, exit non-zero, nothing compiled |

The green suite is the corpus passing through a stable build: `examples.rs` lowers every
runnable example at **both** settings of `user_parallelism`, compiles the emitted Rust with
the `rustc` that built the test — here a stable one — runs the binary and compares what it
printed; `one_brc.rs`, `modules.rs`, `project.rs`, `ledger_determinism.rs` and
`foreign_runtime.rs` do the same for their own programs. A second CI leg runs exactly that,
so the claim cannot rot quietly.

What a stable build did **not** have is the bridge backend, which was the default
backend when this was measured. So `nikaia --input x.nika` with no `--backend`
failed there and the user had to say `--backend rust`. Whether that was
acceptable — whether the default should resolve per build, or whether a stable
installation is a `--backend rust` installation — was left open here and not
implied either way.

> **What happened.** The question answered itself twice over. First the default
> became `rust` everywhere, which made a stable build a whole installation. Then
> the bridge, the nightly and the `rustc_ast` path were withdrawn, so there is one
> code generator, one toolchain and nothing to select between: the "stable-only
> installation" this section had to build on purpose is the only kind there is.
> The measurements are unchanged.

---

## 6. What became of all this

> **Written before the decision, and kept as written.** This section recorded
> what the *measurements* changed, which was the evidence in two records rather
> than the records themselves. What followed was not a further measurement but a
> decision on top of these: the nightly toolchain, the Bridge-IR backend, the
> `rustc_ast` path and `-Zpolonius=next` are **withdrawn**. Nothing in §1–§5 is
> revised.

Read against that outcome, the three conclusions at the top of this file each
turned out to be one half of it:

* **`rustc_private` was the whole of the pin**, and the pin was paid for by one
  optional backend. When that backend went, the pin had nothing left to buy.
* **`-Z` bought nothing**, because no `-Z` flag was ever passed. Group B.2 is
  answered by the frontend desugaring this file found the corpus already writing
  ([ADR-005](../specification/adr/adr-005.md) D2), and the flag was never picked up.
* **The pin's cost to a user was not bytes but identity** — a non-relocatable
  binary tied to one rustup home. That is the measurement that did the most work
  and the one nothing argued back at.

What the records say now: [ADR-001](../specification/adr/adr-001.md) D1, the
toolchain is stable and one file names it; [ADR-004](../specification/adr/adr-004.md)
D1, there is one lowering and it emits Rust source text;
[ADR-005](../specification/adr/adr-005.md) D2, Group B.2 is the frontend's to
desugar. The full account, including what each withdrawn thing was for, is
[`../CHANGELOG.md`](../../CHANGELOG.md) and
[`withdrawn-one-way-down.md`](withdrawn-one-way-down.md).

One question this file handed on is still open and is unaffected by any of it:
whether the compiler should invoke the rustup shim or a named compiler
([`subprocess-cost.md`](../subprocess-cost.md) §5) — *which* `rustc` a Nikaia
installation is entitled to assume.
