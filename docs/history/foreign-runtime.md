# A Nikaia program that starts `hyper` — what ADR-038 D7's two rules cost today

**Date:** September 11, 2026
**Status:** the experiment ran; the finding is evidence under
[ADR-038](adr/adr-038.md) D7 and changes no decision
**Related:** [ADR-038](adr/adr-038.md) D7 (the two rules under test) and §4
(which asked for this early), [ADR-002](../specification/adr/adr-002.md) D1 (the crates.io promise),
[ADR-037](../specification/adr/adr-037.md) D2/D3 and §3 (`user_parallelism`, `Shared`, the structural
`Send` check), [ADR-005](../specification/adr/adr-005.md) D7 and §1 Group B (where that check was
decided), [ADR-006](adr/adr-006.md) D3 (a cancelled cleanup parks with *our*
runtime), [ADR-033](adr/adr-033.md) D4 (fail-closed `touches`)
**The programs:** [`examples/foreign-runtime/`](../../examples/foreign-runtime), driven by
`crates/nikaia/tests/foreign_runtime.rs`

> **Read as a snapshot.** Where this page reasons about what follows from
> `user_parallelism`, it predates [ADR-037](../specification/adr/adr-037.md) D6: the
> owner count of a `Shared[T]` is atomic at **both** settings now, because the
> runtime touches it as well as your code, so a count keyed on the switch would
> race with the machinery under it. Any sentence here that has `no` avoiding an
> atomic is that sentence and not a claim about the compiler today.

[ADR-038](adr/adr-038.md) §4 called D7's two rules "the exception worth checking
early rather than late, because a program that starts `hyper` is the cheapest test of whether
[ADR-002](../specification/adr/adr-002.md) D1's crates.io promise and this record can hold at the
same time". This is that program, and what it found.

**Three conclusions, first.**

1. **It works, and it needed a shim.** A Nikaia program starts a `hyper` server, serves one
   request and prints the body. Twenty-six crates resolve from crates.io, none of them reaches
   the Nikaia pipeline, and the whole graph compiles in eleven seconds. Nothing had to be changed
   anywhere in the language or the toolchain.
2. **D7's first rule is not enforced by this compiler, and today cannot be violated by a Nikaia
   value.** There is no structural `Send` check — it is decided in three places and built in none.
   What refuses an illegal crossing is `rustc`, and the refusal reaches the user as `E0277` against
   a generated file, which Part III C.1 calls a bug in this compiler. It is not *unsound*: the
   values Nikaia can build today are all `Send`, `Shared` is unbuilt, and every safe route to
   another thread carries a `Send` bound. It becomes unsound the day `Shared` lands with nothing
   more than this in front of it.
3. **D7's third paragraph — the part that needs no rule — holds, and was measured.** A foreign
   call has no ledger entry, so it reaches everything and orders against everything. That is the
   one claim here that is asserted on every `cargo test`.

---

## 1. The machine and the method

Intel Xeon @ 2.10 GHz, 4 vCPU, 15 GB RAM, Linux 6.18.44 x86_64 — a shared virtual machine.
`rustc 1.94.0-nightly (8d670b93d 2025-12-31)` — the compiler these programs were built with on
the day, kept because a measurement is worth what the compiler under it was. It is **not** a
toolchain this repository names: since [ADR-001](../specification/adr/adr-001.md) D1 the channel is
stable, and `rust-toolchain.toml` says so and nothing else. crates.io was reachable;
every dependency below was fetched, not vendored.

Four programs, all in [`examples/foreign-runtime/`](../../examples/foreign-runtime):

* `serve/` — starts the server, serves one request, and sends a `String` to a foreign thread.
* `crossing/` — sends a value that may not cross a thread. Expected to be refused.
* `smuggled/` — the same, through a foreign API that says `unsafe impl Send` about a type that is
  not. Expected to be accepted, which is the point.
* `overlaps.nika` — four adjacent pairs, three with a foreign call in, for `--overlaps`.

No timing here is load-bearing, so none is repeated: the numbers are there to say what the shape
feels like, and the findings are all yes/no.

---

## 2. Question one: does it work, and in what shape?

**It works.** `nikaia run --project examples/foreign-runtime/serve`:

```text
served: GET /hello on tokio-rt-worker
crossed: a String is Send on tokio-rt-worker, called from main
```

Both lines are produced on a thread `tokio` created. The Nikaia source is four statements and
says nothing about any of it:

```nika
fn main() {
    let served = hyper_shim::serve_once(18080)
    println(f"served: {served}")

    let crossed = hyper_shim::across_a_thread("a String is Send")
    println(f"crossed: {crossed}")
}
```

### 2.1 The shape: a shim was needed, and the reason is not FFI

**A thin Rust shim crate, and nothing else would have worked.** The reason is worth being precise
about, because "Nikaia cannot call Rust" would be the wrong lesson — it can, directly, with no
annotation and no `extern` block at all. `hyper_shim::serve_once(18080)` lowers to
`hyper_shim::serve_once(18080)`, because Stage 0 lowers name for name
([ADR-011](adr/adr-011.md) D2) and a qualified path is emitted as written. The
generated Rust is an ordinary crate in an ordinary Cargo package, so any Rust function whose
signature Nikaia can *spell* is callable.

What Nikaia cannot spell is `hyper`'s API. Serving one request needs an `async` block, a generic
`service_fn` over a future, `Response<Full<Bytes>>`, a trait bound satisfied by
`hyper_util::rt::TokioIo`, and a `tokio::runtime::Builder` chain. None of `async`, generics over
futures, turbofish, trait bounds or closures-returning-futures exists in the language. So the
shim is not a bridge across a language boundary; it is a **narrowing of a generic Rust API down
to a signature made of `i64` and `String`** — exactly the same work a C binding does for a C++
library, and for the same reason.

That is the interop finding, and it has a consequence D7 does not mention: **the shim is where the
soundness of D7's first rule actually lives.** The rule constrains what may cross into a foreign
thread; the thing that decides whether anything crosses at all is the shim's signature, which is
hand-written Rust that no Nikaia analysis reads.

### 2.2 What the passthrough did without being asked

The generated manifest is ADR-002 D1's, unchanged. Two things are worth recording because neither
was predicted:

* **A path dependency works exactly as a version does.** `[dependencies]` strips `type = "rust"`
  and renders the rest verbatim, so `{ type = "rust", path = "../../../../shim" }` reaches Cargo as
  `{ path = "../../../../shim" }`. The path is relative to the *generated* `Cargo.toml`, which
  lives at `<project>/target/nikaia/build/`, which is why it climbs four levels and not one. That
  is not documented anywhere and is the sort of thing that will be discovered by someone the hard
  way.
* **A proc-macro dependency in the graph is a non-event.** `tokio` pulls `tokio-macros`, and with
  it `syn`, `quote`, `proc-macro2` and `unicode-ident`. `RUSTC_WORKSPACE_WRAPPER` applies to
  workspace members only, so all twenty-six are compiled by the real `rustc`. The generated
  package's lockfile names twenty-eight packages: itself, the shim by path, and those twenty-six.
  `NIKAIA_WRAPPER_TRACE` over the whole build contains one line:

  ```text
  foreign_runtime_serve lowered
  ```

  ADR-002 D2 is about Nikaia's own metaprogramming not costing what proc-macros cost. It says
  nothing about a *user's* Rust dependency having one, and nothing needed to.

### 2.3 The two false starts

Both were in the shim, both were HTTP or Rust rather than Nikaia, and both are recorded because
the next person writing one will hit them.

* **HTTP/1.1 keep-alive deadlocks a one-request server.** The first shim had the client send
  `GET /hello HTTP/1.1` with no `Connection: close`, then `read_to_end`. The server has no reason
  to close a keep-alive connection, so the client waits for a close and `serve_connection` waits
  for the client — a hang that looks exactly like a toolchain problem and is not. The build had
  already succeeded; `ss` showed the socket established and the process parked in `futex_wait`.
  Naming it here because it is also a small demonstration of ADR-038 D6's warning: HTTP/1.1
  framing has more edges than it looks like it has.
* **Edition 2021 closure capture defeated the `unsafe impl Send` demonstration.** `smuggled/`
  needs a wrapper that lies about `Send`; the first version wrote
  `move || smuggled.0.describe()`, and RFC 2229's disjoint capture captured the *field* — the
  non-`Send` `T` — rather than the wrapper, so `rustc` refused it and the lie was never told. The
  fix is a method taking `self`. Worth recording because it is a way of accidentally *not*
  reproducing an unsoundness, which is the worst kind of experimental error.

---

## 3. Question two, first rule: a value crossing into a foreign thread

> **A value may only cross into a foreign thread if it may cross any thread.** […] The structural
> `Send` check ADR-037 §3 names is what decides it.
> — [ADR-038](adr/adr-038.md) D7

### 3.1 The structural `Send` check does not exist

It is decided in three places:

* [ADR-005](../specification/adr/adr-005.md) §1 Group B — "`Send`-ness checked **structurally in the
  frontend** at *every* setting of `user_parallelism`, so a library written at `0` cannot turn out
  un-compilable at `auto`" (the `0`/`auto` spelling predates [ADR-037](../specification/adr/adr-037.md)
  D2's `no`/`yes`, and is stale in that record).
* Part III Appendix C.3 — `NK25xx`, Portability: "**Reserved**: the `Send` rules that parallel code
  needs, reported at `user_parallelism = no` as a lint, so a library built there stays usable at
  `yes`."
* [ADR-037](../specification/adr/adr-037.md) §3 and D2's list of what the switch governs.

In `crates/` the word appears four times, all of them the `Send` bounds on
`nikaia_std::task::both`, plus one unrelated comment. There is no analysis, no `NK25xx`, and
nothing that walks a type. The verdict is entirely `rustc`'s, inherited through
[ADR-002](../specification/adr/adr-002.md) §3 ("Rust's own guarantees are inherited rather than
rebuilt").

### 3.2 An `Rc` cannot reach a foreign thread today — and the reason is not a Nikaia check

The task this experiment was set for asked to say plainly and prominently whether a Nikaia `Rc`
can reach another thread with nothing complaining. The honest answer has two halves.

**No, and twice over** — *as of the day this was written; both halves have since gone stale and
the correction is below.* First, **Nikaia has no `Rc` to send.** `Shared`, which
[ADR-037](../specification/adr/adr-037.md) D3 lowered to `Rc` at `user_parallelism = no`, was not
implemented: there is no `Rc::new` or `Arc::new` anywhere in the emitter, and the only `Shared` in
`crates/` is a word inside a comment about a type's generics. Every value a Nikaia program can
build today — `i64`, `f64`, `bool`, `String`, `List`, `HashMap`, a struct of those — is `Send`.
Second, **the emitted Rust contains no `unsafe`**, and the one thread-crossing vehicle the emitter
names, `nikaia_std::task::both`, carries `Send` on both closures and both results. So the frontend
being silent costs nothing yet: `rustc` type-checks the generated crate like any other, and there
is nothing for it to refuse.

**But the check that was supposed to decide this is not there, and the failure mode it was decided
for is the one that will arrive first.** ADR-005 Group B's stated purpose is that the verdict be
*the same at both settings of `user_parallelism`*. Once `Shared` is built, the same source will
compile at `yes` (where `Shared` is `Arc`) and fail at `no` (where it is `Rc`) — which is exactly
the "a library written at one setting turns out un-compilable at the other" that Group B, `NK25xx`
and ADR-037 §3 all exist to prevent, arriving through a foreign call rather than through a
`par_iter`. Nothing in the compiler will notice.

---

> **Both halves above went stale, and the correction is kept beside them rather than written over
> them**, because what this page is for is the finding and the finding includes how it was reasoned.
>
> **`Shared` is built.** [ADR-064](adr/adr-064.md) gave the shared mutable type its
> name and constructor, and Part II 12.2's counter compiles and runs at both settings. So *"Nikaia
> has no `Rc` to send"* is no longer the reason the answer is no.
>
> **And the failure mode this paragraph predicted cannot happen**, because the premise it rests on
> was taken away deliberately. [ADR-037](../specification/adr/adr-037.md) D6 narrowed D3: `Shared` is
> an **atomic** count at *both* settings, so there is no `yes`/`no` split left to be inconsistent
> about — and D6's own argument is this paragraph's, reached from the other side. D7 then allows
> `contracts::sharing` to take the atomic away for a value it can prove crosses nothing, which is
> an optimisation over the floor and never a second answer.
>
> **What replaced it is a refusal.** D7's per-value inference means a `Shared[Conn]` is an `Rc` for
> one value and an `Arc` for another in the same program, and no foreign signature can name both —
> so [ADR-061](adr/adr-061.md) D1 refuses a `Shared` handed to code nothing describes,
> the same refusal a lock already had and for the sentence that was already the lock's reason.
> `contracts::send`'s `CHOSEN` is where that lives. The way out is to pass what is inside.
>
> **So the experiment's own value stopped being a stand-in.** `shim`'s `LocalHandle` was introduced
> as *"what `Shared` would be at `no`"*; a Nikaia `Shared` cannot be written into this shape at all
> now, so the foreign value is the only way to build it and the question is answered with a value
> that exists rather than a stand-in for one.

### 3.3 What the refusal looks like when there is something to refuse

`Shared` being unbuilt, `crossing/` borrows a non-`Send` value from the foreign crate instead — a
struct holding an `Rc<String>`, which is what `Shared` *is* at `no`. The question is unchanged:
what happens when a value that may not cross a thread is handed to a thread the foreign runtime
owns. `nikaia build --project examples/foreign-runtime/crossing`:

```text
error[E0277]: `Rc<String>` cannot be sent between threads safely
   --> …/crossing/target/nikaia/gen/foreign_runtime_crossing.rs:7:47
    |
  7 |     let crossed = hyper_shim::across_a_thread(handle);
    |                   --------------------------- ^^^^^^ `Rc<String>` cannot be sent between threads safely
    |
    = help: within `LocalHandle`, the trait `Send` is not implemented for `Rc<String>`
```

It is refused. Four things about *how* are the finding:

1. **It is a raw `rustc` error reaching the user.** Part III C.1's Iron Rule: "an untranslated
   backend (rustc) error reaching the user is a Nikaia compiler bug." This one is a plain
   `error[E0277]`, not even reported as an internal compiler error as C.1 requires of an unmapped
   class.
2. **The span is the generated Rust**, `target/nikaia/gen/foreign_runtime_crossing.rs:7:47`, which
   the build wrote and the author has never read. The `.nika` line the error is about is line 14 of
   a file nobody mentions.
3. **The vocabulary is Rust's.** `Rc<String>`, `Send`, `E0277`, `LocalHandle`. `Rc` is not a word
   in Nikaia — it appears only in the records, as an implementation detail of `Shared` (ADR-037 D3,
   narrowed by D6 and made per-value by D7). C.2's first requirement ("terms like *lifetime*, *borrow checker*, or Rust error codes
   never appear") is not met, and neither is its second: nothing says what to do next.
4. **`E0277` is not in the catalogue at all.** ADR-005 D7 item 1 enumerates the classes that have
   translations — E0382, E0499, E0502, E0505, E0506, E0597, E0716 — every one of them a
   borrow/ownership/lifetime error. A trait-bound failure is a *different* class, and the record
   that promises coverage does not name it.

### 3.4 The machinery to fix half of it already exists and is not wired in

`crates/nikaia/src/diagnostics/translate` maps a byte offset in the emitted Rust back to the
`.nika` span that produced it, through `emit::SourceMap`
([ADR-012](adr/adr-012.md)). Fed the same `rustc` JSON by hand, it does the right
thing:

```text
error: examples/foreign-runtime/crossing/src/main.nika:14:5: `Rc<String>` cannot be sent between threads safely
  14 |     let crossed = hyper_shim::across_a_thread(handle)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
     = within `LocalHandle`, the trait `Send` is not implemented for `Rc<String>`
```

The right file, the right line, the statement under the caret. Two things keep that from being
what a user sees:

* **It is only reachable through `--explain`**, which reads `rustc --error-format=json` on standard
  input. `nikaia build` and `nikaia run` shell out to `cargo`, whose stderr goes to the terminal
  untouched. The project path has no interception at all.
* **The text is still `rustc`'s**, by deliberate design: the module's own comment says it "does not
  rewrite the message" because the lowering is name for name, so a message about the generated Rust
  is already a sentence about the Nikaia source. That is true of the grammar backend's frame
  checks, which is what it was built for. It is **false of a trait-bound error**, whose every noun
  is a Rust type the author did not write.

### 3.5 Where the rule genuinely has nothing behind it

`smuggled/` is `crossing/` with one name changed, calling a foreign function whose author wrote
`unsafe impl<T> Send for Smuggled<T>`. It builds, runs, and reads an `Rc` refcount on a `tokio`
worker thread:

```text
crossed: not Send (refcount 1) on tokio-rt-worker, called from main
```

Nikaia contributes nothing to this outcome, and nothing to the previous one either: the whole of
D7's first rule is, today, whatever `Send` bound the foreign crate's author happened to write.
[ADR-033](adr/adr-033.md) D4 already names this case for the *ordering* question —
"anything reached through `unsafe`" — and it is the same case here. What is worth being explicit
about is that **a structural `Send` check in the frontend would not have caught this one either**:
the value it would check is `Send`-by-declaration at the point Nikaia can see it. Only the callee's
body is wrong, and the callee's body is Rust. So D7's first rule is enforceable exactly as far as
the foreign crate is honest, and a note to that effect belongs next to it.

---

## 4. Question two, second rule: a `Cleanup` owned by a foreign task

> **A `Cleanup` may not be owned by a foreign task.** ADR-006 D3 parks a cancelled cleanup with
> *our* runtime, and a foreign runtime's shutdown will not drain that queue. A `Cleanup` handed
> across is a cleanup that may never run.
> — [ADR-038](adr/adr-038.md) D7

[ADR-006](adr/adr-006.md) is unbuilt, so this cannot be run. The argument below is
the deliverable. It concludes that **the rule is right and its stated reason is the least of five**,
and that a cheaper alternative to refusal exists which ADR-038 did not consider.

### 4.1 What goes wrong, in order of severity

**(a) On the ordinary path, `cleanup()` is never called at all — and that is worse than the
cancellation case D7 names.** [ADR-006](adr/adr-006.md) D1 inserts `cleanup()` on
every exit path *of a Nikaia block*, during lowering. A value handed to a foreign function leaves
Nikaia's control flow at that call: the frontend sees a `let` and a call, and whatever eventually
destroys the value is Rust the frontend never lowered. So there is no inserted `cleanup().await`,
no queue entry, nothing late — the pausable half simply does not exist in the emitted program. Only
Rust's `Drop` runs, which is `drop()`, the last-resort synchronous fallback. Part I 6.4's promise
("cleanup happens, you cannot forget it") silently degrades to path 3 behaviour on path 1. D7's own
reason — the parked queue not being drained — is about *cancellation*; this is the normal case and
it fails earlier and more completely.

**(b) D5's shutdown drain cannot even report it.** [ADR-006](adr/adr-006.md) D5 waits
at exit for "parked cleanups and detached tasks", and on expiry warns, "naming every resource that
did not finish cleanly" (`NK2603`). A value owned by a foreign task is neither a parked cleanup nor
a detached task. The drain is satisfied, the deadline is not exceeded, the program exits `0`, and
the warning that exists precisely to prevent silent data loss has nothing to name. **The failure is
invisible by construction**, which is the property that makes it worse than a hang.

**(c) The `throws` obligation is accounted for at a scope exit that never happens.**
[ADR-006](adr/adr-006.md) D4 makes the inserted `cleanup()` a real call site: a
`cleanup()` that declares `throws IoError` gives the enclosing function a `throws` it did not write
(`NK2601`), and the ledger records it. Hand the value to a foreign call and the enclosing function
keeps the obligation in its signature while the call that justified it is gone. Worse, if the
foreign crate's drop glue is what runs the teardown, the error cannot be expressed at all:
`Drop::drop` returns `()`. D4's parked path sends such an error to the runtime's error hook and
calls that consistent with a cancelled task's result being discarded — there is no equivalent story
here, because the runtime was never told.

**(d) `cleanup()` may pause, and a foreign executor cannot drive it.** This is the one place where
the two records collide mechanically rather than in bookkeeping. `cleanup()` is a pausable Nikaia
function, so in the lowered program it is a future whose wakers come from *our* runtime — and
[ADR-038](adr/adr-038.md) D3/D4 put those completions on our I/O thread, on
`io_uring` where the machine has it. A `tokio` task polling that future is a second runtime nested
inside the first. [ADR-006](adr/adr-006.md) D5's own "honest limit" already names the
shape of what follows: *"FFI that blocks the thread rather than pausing will block a single-threaded
event loop and with it the deadline timer."* A cleanup awaiting our runtime from inside a foreign
task is that hazard with the arrow reversed, and the thing it can stall is the timer that is
supposed to bound it.

**(e) Cancellation, which is the case D7 states, and which is symmetric.** If the foreign runtime
cancels its own task, `tokio` drops the future on its own thread; our queue is never told and
D3's adoption never happens. And in the other direction, our runtime cancelling a Nikaia task that
has already handed a value to a foreign task cannot reclaim it — the value is gone, and the
cancelled task's cleanup list does not contain it.

### 4.2 What the compiler would have to refuse

The rule that follows is broader than D7's wording and easier to state:

> A value whose type implements `Cleanup` may not be passed to, returned from, or captured by a
> closure passed to, **a function whose body this compiler cannot see**.

Three notes on building it:

* **The polarity is already computed, for a different question.** `contracts/sync.rs` has
  `Reached::Opaque` — "a call whose target this compiler cannot name and nobody else can either" —
  and the `sync` *inference* fail-closes on it. That is the same predicate this rule needs, and it
  costs no new analysis to ask.
* **But the `sync` *check* fail-**opens** on the same value, deliberately**, and a `Cleanup` check
  built on that walk would inherit the hole. `called()` returns `None` for
  `Reached::Method | Reached::Opaque`, and the comment says why: `NK2202` puts a caret under one
  call, the resolver answers per function, and *"something in here pauses" is not a message Part III
  C.2 allows*. Measured, and it is visible from the outside: a function **declared** `sync` whose
  body calls `hyper_shim::serve_once` compiles today and is recorded in `nikaia.contracts` as
  `sync = true`. So the precondition for this rule is expression-level spans
  ([ADR-024](../specification/adr/adr-024.md) D7), which the code comment already names as the
  blocker. A `Cleanup` rule that shrugged where `sync` shrugs would be a rule in name only.
* **It has to be structural and transitive**, like the `Send` check ADR-005 Group B names, and for
  the same reason: a struct with one `Cleanup` field, a `List` of them, a closure capturing one. No
  analysis in the frontend walks a type for a marker today. The ledger's `signature` and `fields`
  ([ADR-024](../specification/adr/adr-024.md)) are the raw material and nothing reads them that way.

The diagnostic belongs next to `NK2602` and has a way out that already exists —
[ADR-006](adr/adr-006.md) D4's explicit `close()`, which consumes the resource and
returns the error normally:

```text
error[NK26xx]: `conn` has cleanup that can pause, and this call cannot run it
  --> app.nika:14:5
  14 |     hyper_shim::serve(conn)
           ^
     = `hyper_shim::serve` is a call this compiler cannot see the end of, so the
       cleanup `conn` needs would never run
     help: write `conn.close()?` first, which runs the cleanup and hands you the
           error, or pass something that needs no cleanup
```

### 4.3 The alternative ADR-038 did not name

Refusal is not the only sound answer, and the other one is buildable.

**Wrap it instead of refusing it.** Hand the foreign crate not the value but a guard, whose Rust
`Drop` pushes the due `cleanup()` onto our runtime's parked queue. `Drop::drop` is synchronous and
must not pause — pushing onto a queue does neither, so this is exactly what D3's adoption already
does for a cancelled task, reached from a different direction. The pausable half then runs on our
runtime, where its wakers come from, which disposes of (d); D5's drain covers it, which disposes of
(a) and (b); and the guard must be `Send` to cross at all, so rule 1 is doing the work it was
already doing.

**What it costs** is (c) and nothing else: the cleanup's error goes to D4's error hook instead of
being thrown at a call site, and the enclosing function's `throws` obligation becomes a promise
about a hook rather than about a statement. That is a real semantic loss and the source cannot see
it. It is also precisely the loss D4 already accepted for the parked path.

So the choice between "refuse" and "wrap" is a decision about whether a `Cleanup` crossing into a
foreign task should be an error or a silent demotion to the parked-path error semantics. D7 chose
refuse without naming the alternative, and refuse is the more conservative choice — but the
alternative is cheap enough, and close enough to D3's existing mechanism, that it should be written
down as rejected rather than unconsidered.

---

## 5. Question three: what needs no rule, confirmed

> What needs no rule: the ledger knows nothing about a foreign crate's effects, so `touches` is
> absent, so it reaches everything and orders against everything
> ([ADR-033](adr/adr-033.md) D4). Fail-closed polarity means interop cannot silently
> break the ordering guarantee — it only makes programs that use it slower.
> — [ADR-038](adr/adr-038.md) D7

**Confirmed, empirically, and it is the one claim here asserted on every `cargo test`.**
`nikaia --input examples/foreign-runtime/overlaps.nika --overlaps --user-parallelism yes`:

```text
control:
    together  fs::read_to_string / fs::read_to_string - they meet on nothing
    in order  fs::read_to_string - one of them holds text with code in it, whose holes this has not parsed
foreign_against_foreign:
    in order  … - nothing says what `hyper_shim::serve_once` reaches, so it reaches everything
    in order  … - nothing says what `hyper_shim::serve_once` reaches, so it reaches everything
foreign_against_a_read:
    in order  fs::read_to_string - nothing says what `hyper_shim::serve_once` reaches, so it reaches everything
    in order  fs::read_to_string - one of them holds text with code in it, whose holes this has not parsed
a_read_against_foreign:
    in order  fs::read_to_string - nothing says what `hyper_shim::serve_once` reaches, so it reaches everything
    in order  … - nothing says what `hyper_shim::serve_once` reaches, so it reaches everything
```

The control pair overlaps, so the report is not merely a build with overlapping switched off. Every
pair with a foreign call in it is kept in order, and the reason given is the absent `touches` and
not something that happens to coincide with it. The emitter agrees: exactly one overlap in the
lowered program, inside `control`.

*(Updated for [ADR-033](adr/adr-033.md) D10. That overlap was a `task::both` when this
was written and is now `task::read_pair` — the control is two file reads, and two reads are carried
by the runtime with no thread of the program's in them. Which also means the `--user-parallelism yes`
in the command above is no longer needed to see it: the assertion on every `cargo test` runs at both
settings, and finds no `task::both` at either.)*

The `nikaia.contracts` a foreign-calling project writes mentions the foreign crate nowhere at all,
which is D4 working as designed rather than a gap — the entry's absence *is* the conservative
answer.

One thing to note beside the confirmation: the ordering analysis is fail-closed here for a reason
that has nothing to do with foreignness. `hyper_shim::serve_once` is refused because no ledger
knows the name, which is the same refusal a call to an unwritten Nikaia function gets. There is no
analysis anywhere that knows a call leaves the language. If a future `nikaia.contracts` could carry
a hand-written contract for a foreign crate — and nothing forbids it — then a wrong `touches` there
would break the ordering guarantee, and there is no mechanism that would distinguish it from a
wrong contract for a Nikaia function. That is not today's problem; it is the shape of tomorrow's,
and it is the reason the fail-closed polarity should not be read as "interop is safe" but as
"interop is not described".

---

## 6. What the repository owner has to decide

Nothing here changes a decision. Four things are open, in the order they will bite:

1. **Does `Shared` land before or after `NK25xx`?** Section 3.2 is the whole of it: today's
   silence costs nothing because Nikaia has no non-`Send` value. `Shared` at `user_parallelism =
   no` creates one, and a program that hands it to a foreign call will then be accepted at `yes`
   and refused at `no` — the exact asymmetry ADR-005 Group B, `NK25xx` and ADR-037 §3 were all
   written to prevent — with a raw `E0277` as the only signal. The cheap version is Group B's own
   phrasing: run the check at both settings and report at `no` as a lint.
2. **Does the project path get diagnostic interception, or does `cargo`'s stderr stay the user
   interface?** Part III C.1 says the second is a bug, and §3.4 shows the mapping already works
   when it is fed. Wiring `cargo --message-format=json` through `diagnostics::translate` is a
   contained change. Rewriting the *text* of a trait-bound error into Nikaia vocabulary is not, and
   is a second decision.
3. **`E0277` is not in ADR-005 D7's enumerated classes.** That list is borrow, ownership and
   lifetime errors. Trait-bound failures are how `Send`, `sync` and every future marker rule will
   surface, and the record that promises coverage should say so.
4. **Is D7's second rule "refuse" or "wrap"?** §4.3. Both are sound; they differ in whether the
   cleanup's error reaches a call site or an error hook.

And one thing to add rather than decide: D7's first rule is enforceable only as far as the foreign
crate is honest (§3.5). A structural `Send` check would not have caught `smuggled/`, and the record
reads as though it would.

---

## 7. Postscript — what §6 asked, and what has since been decided

**Date:** September 12, 2026. Added to the record rather than edited into it: the
experiment above is what was measured on the 11th, and the four open items are
what it handed over. Three are now answered in the records that own them, and
this note is where a reader of §6 finds out.

1. **`Shared` lands *after* `NK25xx`.** The structural `Send` check is built —
   [ADR-005](../specification/adr/adr-005.md) §5 — and §3.2's asymmetry cannot
   arrive unannounced: a value that may not cross a thread is refused as
   `NK2501` into a task and `NK2502` into a call this compiler cannot see the end
   of, in Nikaia words and in the `.nika` file, with the same verdict at both
   settings of `user_parallelism`. The "cheap version" §6 proposed is what was
   built, including Group B's own phrasing: at `no` the task crossing is a lint,
   because the task does not run there.
2. **The project path gets the interception.** §3.4's two blockers are both gone:
   `nikaia build` reads `cargo --message-format=json` through
   `diagnostics::translate`, so the caret lands on the `.nika` line. `crossing/`
   is still refused by `rustc` rather than by the frontend — its value's type
   comes from the foreign crate, so the structural check's answer is *undecided*,
   which is neither permission nor a refusal — but the refusal now names the line
   the author wrote. The **text** is still Rust's, which is the second decision
   §6 separated out and which is still open.
3. **`E0277` is in [ADR-005](../specification/adr/adr-005.md) D7's enumerated
   classes.** §3.3's fourth finding, recorded where the promise lives.
4. **D7's second rule — refuse or wrap — is still open.** §4.3 is unchanged and
   still the argument; [ADR-006](adr/adr-006.md) is unbuilt, so
   nothing has been built that would decide it.

And the addition §6 asked for rather than a decision: D7's first rule now says
that it reaches only as far as a foreign crate is honest, with §3.5 as its
evidence.

**One thing this experiment could not have predicted, and which the check
produced.** A value that may not cross at `no` may not cross at `yes` either,
because the verdict is not allowed to consult the switch — so once `Shared`
exists, a `Shared` will not cross a thread at all, and Part II 12.2's shared
counter is the program that wants to. [ADR-037](../specification/adr/adr-037.md) D3
already names the way out and leaves it open ("whether the *choice between `Rc`
and `Arc`* could be made per value rather than per build"). That is now the
question standing directly in front of `Shared`, and it was not on §6's list.

---

## 8. Second postscript — the frontend refuses both crossings now, and `unsafe impl Send` does not help

**Date:** September 22, 2026. Added rather than edited in, for §7's reason: what
is above is what was measured, and this is what changed.

[ADR-193](adr/adr-193.md) D1 gave a description a `threads`
column — three values, hand-written, the absence meaning *nobody said* — and D2
made `NK2502` ask a **described** call where the word says `true`. Before that,
the refusal asked its question only of a call *nothing* describes, which is
[ADR-038](adr/adr-038.md) D7's own wording, so a crate that
answered every other question honestly turned the check off by being described.

`hyper_shim::across_a_thread` and `across_a_thread_unchecked` both build a
`tokio` runtime and spawn, and `serve_once` builds a multi-threaded one. All
three now say `threads = true` in the three ledgers under this directory, which
is what a reviewer reading the crate would write.

**§3.3's question — what the refusal looks like when there is something to
refuse — is answered for `crossing/`:**

```text
error[NK2502]: `handle` may not cross a thread, and `hyper_shim::across_a_thread` may put it on one
  --> examples/foreign-runtime/crossing/src/main.nika:25:5
  25 |     let crossed = hyper_shim::across_a_thread(handle)
           ^
     = `hyper_shim::across_a_thread` is described as starting a thread of its own (`threads = true`), so what it is given may be looked at from one (Part III, 15.2)
     = `hyper_shim::LocalHandle` is written down as a value that may not go to another thread, so this compiler does not send it to one - at either setting of `user_parallelism`
     help: keep it where it was made and hand the call what it needs from it instead
```

Nikaia's code, Nikaia's vocabulary, the author's line. §7's item 2 recorded that
the **text** was still Rust's; for this program it is not any more.

**And `smuggled/` is refused too, which §3.5 did not expect.**

That section says *a structural `Send` check in the frontend would not have
caught this one either: the value it would check is `Send`-by-declaration at the
point Nikaia can see it*. **That sentence is still true, and the check that
catches it is not the one it is about.** A structural check reads what the
language below says about a type; this one reads what a **person wrote down**
about it — `crosses = false` on `hyper_shim::LocalHandle`, because the type
holds an `Rc`. An `unsafe impl Send` cannot change that line, because the line
never asked Rust.

So the experiment's own finding moves, and the new one is sharper than the old:

* **D7's first rule is enforceable exactly as far as the foreign crate is
  honest** — unchanged, and now with a second axis. Honesty about the *type*
  (`crosses`) and honesty about the *call* (`threads`) are two claims a person
  makes, and either one alone is enough here: the program is refused because the
  type says it may not cross and the function says it may thread.
* **What `unsafe impl Send` defeats is `rustc`**, not a description. A crate
  that lies in `unsafe` Rust still has to lie in its `.contracts` file to get
  past this, and that file is committed and reviewed like code
  ([ADR-104](adr/adr-104.md) D5) — which is a different thing to
  ask of an author than a bound the compiler infers.
* **What is *not* claimed**: none of this is soundness. A description that says
  `crosses = true` about a type holding an `Rc` gets exactly as far as it did
  before. The column moves who has to be honest, and says so out loud
  ([ADR-193](adr/adr-193.md) D3).

**And §2.2's sentence about the path is superseded**, which is worth saying
because it is the reason none of this could be measured until now. That section
records `{ path = "../../../../shim" }` as *relative to the generated
`Cargo.toml`*, and it was, until [ADR-053](adr/adr-053.md) D1 gave
every package a member directory of its own and left every one of those `..`
one short. All three projects here were unbuildable from that day until
[ADR-197](adr/adr-197.md), and nothing said so: the tests that
build them fetch `hyper` from crates.io and are `#[ignore]`d, and CI does not
run ignored tests. A `path` is relative to `nikaia.toml` now, so the three
manifests say `../shim`.

**A second defect, and it was the reason the result took two tries.** A change to `contracts/<crate>.contracts` did not reach the build
cache's key, so a hand edit — which D5 *expects* — took effect only after the
build directory was thrown away. It failed **open**: dropping a claim was seen,
because a refused build records nothing, while adding one hit an entry recorded
before the word was there. `Choices::describes` is the dimension that closes it
([ADR-021](../specification/adr/adr-021.md) D7's *every dimension in one place*).
