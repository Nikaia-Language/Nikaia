# Open work — what is found, what is decided and unbuilt, what is stale

The running list of what is still open, in three kinds:

* **Defects** — the compiler accepts a program it should refuse, produces a
  different program than the source says, or hands the user something in the
  backend's words. A defect outranks everything below it.
* **Decided and unbuilt** — an ADR says what happens and the compiler does not
  do it yet. Each entry names its record; the record specifies the work, and
  this file says where it stands.
* **Upkeep** — a specification sentence or a notes page that a later decision
  made false.

Rules for entries:

* **A closed entry is deleted**, and so is the answered part of an open one.
  What it was and what closed it is in the CHANGELOG. The numbers of the rest
  do not change; a gap in the numbers is a closed entry.
* **An entry is cited by its subject**, not by its number alone.
* **Every entry carries its evidence**, or is marked as a suspicion.
* **A question that needs the owner goes to
  [`open-decisions.md`](open-decisions.md)**, in the shape that page asks for.
  An entry that names such a question links to it there.

This file is a notes page: nothing here is normative.

---

## 1. Defects

Entries here are found by **running** something: the specification's programs
(`crates/nikaia/tests/specification.rs` takes every `nika` block as far as it
goes and hands the ones that lower to `rustc`), the corpus at both settings of
`user_parallelism`, a multi-file project. An empty section says what has been
run, not that the compiler is correct.

**This section is empty.**

## 2. Decided and unbuilt

Two rules for ordering this section:

* **Checked and unrunnable rots fastest.** A check no program exercises stops
  being true without anything failing, so an entry that says *the check runs and
  the construct does not* is more urgent than its size suggests.
* **A refusal is free before programs exist** and breaking afterwards.

The order:

1. **A server to bind to, and the `postgres` block.**
2. **Supervision.** Nothing else waits on it.

### 2.4. Part II 12.8's supervision syntax

`supervisor::start_link(fn { … }; restart_policy: …)` is specified and there is no
supervisor. Listed so it is not mistaken for something the `spawn` work includes —
it is not.

### 2.5. `fortunes.nika` waits on two runtime pieces and one language question

The template half is built — [ADR-017](specification/adr/adr-017.md)'s `dsl html`
compiles where it is written, every hole goes through `html::Render`, and the
whole file parses since [ADR-022](specification/adr/adr-022.md) removed the `fn:`
form. What is left is machinery, not syntax:

* **The `postgres` block**, a deferred-parameter DSL
  ([ADR-007](specification/adr/adr-007.md) D4): the statement has to reach a
  driver intact, which is different machinery from the template that exists.
* **The runtime binding for a handler**, which is **no longer blocked**: there is
  a server since 0.0.166. [ADR-018](specification/adr/adr-018.md) D1's request
  and D3's response are `examples/http`'s two types, and **D4 is built** —
  `path()`, `target()`, `method()` as the **enum** the record asks for, `body()`,
  `query()` and `header()`. What is left of that record is **D2's table**: a
  handler that returns a bare `String` or an `html::Raw` rather than a
  `Response`, which needs the handler's type to vary and is `NK1142`'s
  neighbourhood.
* **A handler can be received now, through one of the two doors.**
  `.route("/fortunes") fn { fortunes(db) }` needs `route` to declare a parameter
  that is code, and **that door is open**: measured at 0.0.150,
  `fn apply(f: fn() -> String) -> String { return f() }` parses and lowers
  ([ADR-102](specification/adr/adr-102.md) D1, and
  [ADR-192](specification/adr/adr-192.md) D1 for the shape it takes). This entry
  said it was a parse error, which it was when the entry was written.
  **And the second door opened at 0.0.171**:
  `fn tell[T: greet::Speaks](x: T)` was a parse error at the `:` and parses now
  ([ADR-106](specification/adr/adr-106.md) D1), with the ledger carrying a
  package's traits and `impl`s beside it. The *module question* this bullet said
  nobody had decided was [ADR-078](specification/adr/adr-078.md) §4's, and
  ADR-106 had decided it. Either door is enough, so what is left here is `route`
  itself and not the language.

  *And the question this bullet named was never written down.* It said *that is
  **how does a package receive a handler** on
  [`open-decisions.md`](open-decisions.md)* — that page has never held such an
  entry, in its whole history. Citing a question is not asking it, which is the
  head of this file's own rule met from the wrong side.

**Measured, so the order is known.** Given a manifest that depends on
`examples/http/`, the file stops before any of the three: `dsl postgres { … }`
has no hole, and `postgres` is not a grammar this compiler has. So the first
thing fortunes needs is the driver question above, and the handler question is
what it meets after that.

Moved here from [`handoff.md`](handoff.md), which is a guide to the parser backend
and was also carrying open work. One list.

### 2.6. The HTTP server is built, and what waits on it is the parsing moved into Nikaia

[ADR-038](specification/adr/adr-038.md) §4.5. Its D3, D4 and D5 are built — the
runtime is running before `main`, files complete on `io_uring`, sockets signal
readiness — and [ADR-055](specification/adr/adr-055.md) has since put an executor
on top of them at `user_parallelism = no`, so a task can pause and another can
run. **D1's server is built at 0.0.166**; D2's `rustls` and D6's HTTP/1.1 parser
written in Nikaia are untouched. The order that record gives is unchanged except
that it is one step shorter: a socket layer that keeps registrations rather than
answering one readiness question at a time, then a minimal HTTP/1.1 server on it,
then the parsing moved into Nikaia, then `rustls`, then HTTP/2 — with
`nikaia serve` **cut** ([ADR-200](specification/adr/adr-200.md) D1) rather than
waiting at the end of it. The first three steps are done.

What waits inside it:

* [ADR-018](specification/adr/adr-018.md) D1 and D3 — what a handler sees and what
  it gives back — are **built** as `http::Request` and `http::Response` in
  `examples/http/`, and its D2's other two rows (a bare `String`, an
  `html::Raw`) need a conversion that package cannot express yet;
* [ADR-058](specification/adr/adr-058.md) D1's `Bytes` body row, D2's `http::File`,
  D3's mechanism choice and D8's kept mappings, all of which are things to build
  *on* a server ([#45](https://github.com/Nikaia-Language/Nikaia/pull/45));
* [`project_status_and_roadmap.md`](project_status_and_roadmap.md) Phase 3's route
  hashing, which says in as many words that it has no target because there is no
  server.

**The one piece that looked as though it did not wait is answered elsewhere.**
A name the request chose reaching the filesystem is
[ADR-108](specification/adr/adr-108.md): the root is an argument of the call,
`http::File(path, root)` exactly as `fs::map(path, root)`, and there is no
provenance on a path and no refusal to build before the server. **The `fs` half
is built at 0.0.178** — `fs::Root` with its two variants, the comparison by
component, `io::IoError::Outside`, `NK1101` for a call that leaves the root out,
and `nikaia --trust`'s listing — so `http::File` inherits a root that exists the
day it is written.

Nothing of [ADR-058](specification/adr/adr-058.md) is built. What is built is the
bench that decided it (`benches/sendfile/`) and the write-up
([`zero-copy-send.md`](history/zero-copy-send.md)); `send_file` beside the ring could have
been built ahead of the server and deliberately was not, because D3's measurement
makes it the mechanism that loses at the sizes a server sends most.

**The runtime piece underneath it is built**, and this entry said otherwise for
a long time. It read: *`rt::io::wait` — the readiness half this would rest on —
**cannot be awaited**, only blocked on … nothing here can be an `async fn` that
actually pauses until it is answered*, and pointed at a question on
[`open-decisions.md`](open-decisions.md) that page has never held in its whole
history.

[ADR-121](specification/adr/adr-121.md) answered it and is **built**. D1 puts an
always-armed `eventfd` on the ring so a worker's reply completes a ring job and
the park returns; D4 says *the same is true of `rt::io::wait`, which is what the
server's socket layer awaits*; and §3 names this entry's own need outright:
***`rt::io::wait` is awaitable, which is the first thing the HTTP server's
socket layer needs.*** `rt::io::waiting` is the future beside it.

*So the head of §2's order is not blocked on the runtime.* What it was short of
was the socket layer itself — and `worker::poll_one`, which this sentence named,
is gone ([ADR-199](specification/adr/adr-199.md)). What is left is the server on
top of it, which is work in this section and not a question for anybody.

*And the socket is in `std` since 0.0.164* ([ADR-198](specification/adr/adr-198.md)),
with the layer under it since 0.0.165
([ADR-199](specification/adr/adr-199.md)) and the server itself since 0.0.166 —
so nothing is left of the order below. **Step 4 was `nikaia serve` and is cut**
([ADR-200](specification/adr/adr-200.md) D1): it is the half of
[ADR-194](specification/adr/adr-194.md) D2 that nothing on this list waits on,
and what it would have shown about this language is the part this language has
least of.

**And the MVP is decided** ([ADR-194](specification/adr/adr-194.md)), so what is
left here is an order rather than a design:

1. **A socket in `std`** — D1, **built** at 0.0.164
   ([ADR-198](specification/adr/adr-198.md)). `net::listen`, `connect`,
   `accept`, `read`, `write`, `address`, `peer`, `close`: a `.nika` program
   binds a socket and both ends talk, at both settings of `user_parallelism`,
   with the same output from each. Everything that waits gives the thread up on
   [ADR-121](specification/adr/adr-121.md) D4's readiness, and nothing in the
   program says `async`, `await`, `epoll` or `poll`. **And it is the first
   untrusted source `std` has** — [ADR-010](specification/adr/adr-010.md) D2's
   column had nothing to fire on until a socket existed.
2. **The socket layer** — **built** at 0.0.165
   ([ADR-199](specification/adr/adr-199.md)), and **this line was wrong about
   what it was for**. A readiness wait was a worker operation and the worker
   *blocked* in the poller for the whole of it; `io-workers` is **1** by
   default, so a wait that had not answered blocked every other wait in the
   process — a ceiling on the default configuration and not a slowness, and the
   shape a server has exactly. Measured: two waits at once, the second on a pipe
   that already had a byte in it, and the second did not get a turn in two
   seconds.

   *And what this line asked for is the smaller half.* Keeping a registration is
   worth **6.2 µs of 35**; the **hop** was worth 28
   ([ADR-009](specification/adr/adr-009.md) D4, doing its job on a line written
   before anything had been measured). One poller for the process, arming on the
   calling thread, and `rt::io::waiting` goes 35.4 → **6.6 µs**.
3. **A minimal HTTP/1.1 server in the `http` package** — D5, **built** at
   0.0.166. `GET` and `POST`, bodies by `Content-Length`, `Connection: close`,
   no chunked and no TLS, with the caps from the first commit: `body_cap`,
   `head_cap`, `head_wait` and `connections` are options on `http::listen`, and
   `examples/hello-http/` is a real server that
   `crates/nikaia/tests/project.rs` drives over a real socket — port `0`, the
   bound address read off the line the server prints, then the two methods, a
   body by `Content-Length` and each of the six refusals. **The parser's text
   half is Rust in `nikaia-std`** — `std::http1`, whose `Buffer` answers where a
   head ends, what it says, and where the body starts — and moving it into a
   Nikaia grammar is step 4 of
   [ADR-038](specification/adr/adr-038.md) §4.5, which is what is left of this
   entry. **The route it takes when it moves is decided**
   ([ADR-196](specification/adr/adr-196.md) D2): the grammar is lowered ahead of
   time and joins `nikaia-std` as an **ordinary Rust module**, the way
   `std::text` already does — so the Nikaia parser is what a Nikaia program
   *and* a Rust one call, and there is nothing between them to design.

*The MVP's shape is one handler*: `http::listen(at) fn(request) { … }`, with the
handler deciding, which keeps [ADR-194](specification/adr/adr-194.md) D4's rule
— the program says what is exposed, and nothing is derived from `pub`. A
`.route(…)` chain can be written now that a function value can be kept in a
field ([ADR-217](specification/adr/adr-217.md) D2); adding one is the package's
work.

*Two lowerings were wrong about a function-typed parameter*, both found by being
the first program to write one for real, and both fixed at 0.0.166. A parameter
the body hands **on** was moved rather than lent, so a handler passed to an
`answer` inside an accept loop was gone the second time round; and a `mut`
parameter handed to another `mut` parameter gained a second `&mut`, which is not
a `&&mut` that derefs — a `&mut` may only be taken of a binding that is itself
`mut`. Both were `rustc` about a file nobody wrote
([Part III C.1](specification/30-nikaia-tooling.md)).

*And one thing is no longer deferred*: whether a route may be refused by what
its handler **touches**. [ADR-194](specification/adr/adr-194.md) §4 carries it,
the owner asked to be asked again with a fuller write-up when the work reached
it, and the work has reached it. **It is on
[`open-decisions.md`](open-decisions.md) now**, not here.

### 2.7. There is no target that lets foreign code call in, and the record for one is written

[ADR-062](specification/adr/adr-062.md). Nothing of it is built: `extern "C"` is
a parse error (Part III 15.1), `Target` has two values, and this repository has
no notion of a linkable artifact — no `cdylib`, no `staticlib`, no `.so`.

**The owner has since named it as open**, so it is no longer waiting on scope
([`project_status_and_roadmap.md`](project_status_and_roadmap.md)). **Talking
*to* C came first** and is 15.1's, and it is built:
[ADR-124](specification/adr/adr-124.md) reserved `extern` and `unsafe` *with
their constructs* — the number that allowed two words was zero — and an
`extern "C"` block, an `unsafe { … }` and `NK1143` are all there. What that
record left open is `Pointer[T]`, and what **this** one still waits on is a
**target**: `Target` has two values and there is no linkable artifact anywhere in
the repository.

*What it needs, in the record's order:* the target and its artifact kind (§4
leaves both open), and then the analysis taking the exported entry points as
roots seeded at the floor — which is the same order of work as the crossing
roots it already seeds, and which D3 says needs no change to any check.

*Why it is written down anyway:* it is the one direction that touches
`user_parallelism` at its root. A target added without it answers "who owns the
threads" by accident, and the answer is invisible — a caller's second thread and a
plain reference count under it is a data race with nothing to see at the source.

*What it needs, when something asks for it:* the analysis takes the exported entry
points as roots seeded at the floor, the way it already seeds crossing roots. The
checks need nothing — [ADR-045](specification/adr/adr-045.md) D1 kept every verdict
off the switch, so a library is already checked for the world it would enter.

### 2.15. A package is found by version through Cargo, under `nikaia_<name>`

[ADR-103](specification/adr/adr-103.md). `http = "1.2"` becomes
`http = { package = "nikaia_http", version = "1.2" }` in the generated
`Cargo.toml`; Cargo finds and fetches, the compiler reads `src/*.nika` and
`nikaia.contracts` from where the crate landed and emits it as a workspace
member as it does a path dependency; a crate under the prefix without both is
refused by name. **Nothing of it is built**: a bare version is refused with
the old sentence, and a package's own package dependencies are not followed.

*What it needs, in the record's order (§5):* the manifest arm and the rename;
resolution through `cargo metadata` and the not-a-Nikaia-package refusal;
following a dependency's own manifest in both arms; the stub `Cargo.toml`
written by the build so that `cargo publish` is the whole of publishing.

*Evidence:* Part III 13.3's own example, which was a refusal and is now a
dependency; no package in the tree is published yet, so the first one is the
test.

### 2.16. A foreign crate is described before it is called

[ADR-104](specification/adr/adr-104.md). A call into a crate no ledger
describes is refused with the command in the message; `nikaia describe
<crate>` writes `contracts/<crate>.contracts` for the functions the program
calls, from rustdoc-JSON where the toolchain has it and from the crate's
sources where it does not, translated by Part III 15.2's table with
`touches` and `locks` fail-closed; the file is committed, hashed against the
crate's version, and reviewed. **Steps 1, 2 and 3 are built, and step 5 by
hand.**

*The refusal is `NK2504`*, once per crate and with the command in the
message — and only where the **manifest** declared the crate with
`type = "rust"`, because refusing on a qualified name nobody declared is
[Part III C.4](specification/30-nikaia-tooling.md)'s correct program refused.
A written **type** from such a crate counts as much as a call does.
`hyper-shim` in the manifest is `hyper_shim` in a program, which is the crate
name Cargo makes of the key.

*And `examples/foreign-runtime/` is described rather than the fixture*, which
is the half of step 5 that could be done without the command: the three
projects carry a `contracts/hyper_shim.contracts` written by hand from the
crate's `pub` signatures, which is what D5 says to expect. A fixture of the
refusal lives in `crates/nikaia/tests/described.rs` instead, where it needs no
network.

*Step 2 is built too, for a `path` dependency.* `nikaia describe <crate>` reads
the crate's `pub` signatures out of its sources, translates them by D3's table
and writes the draft under a header that says it is to be **reviewed**. It is a
**signature scraper** and not a Rust parser — an item a macro generates is not
in the text, a `pub` item inside a `mod` is read as the crate's own, and a type
it cannot account for is `?`. The command names what it could not answer rather
than leaving a draft that looks complete. A version dependency is refused with
its reason: those sources are in Cargo's registry cache, and guessing at that
cache's shape would be resolving a version, which
[ADR-002](specification/adr/adr-002.md) D1 hands to Cargo.

*The measurement is the experiment's own file.* The draft for each of the three
projects is asserted against the reviewed file, entry for entry and hash for
hash. Two of the reviewed lines are not in the draft and both are D5:
`crosses = false`, which is read from a **field**, and the comments.

*And the entries reach the analyses now*, which was D1's **first sentence** and
not an addition to it: *every analysis reaches to the boundary and reads an
entry there*. For two records the file was read to see whether it **parsed**,
the ledger was dropped, and its only effect was silencing the refusal that had
asked for it — while `NK2504`'s own message promised a reader four answers in
return for writing it. Measured on a three-line project: a description saying
`(a: i64, b: i64) -> i64` left a one-argument call unremarked. Three of the four
are in (the signature, `throws`, `sync`); the fourth is `crosses` and is not a
gap here — see the paragraph below.

*And the hash rule holds* (step 3, D5). A description is believed while the
files it was derived from hash as recorded, and `NK2505` says so where one does
not — the same rule as a stale ledger's with the one difference that matters:
[ADR-100](specification/adr/adr-100.md) D3 derives a ledger again, and a
description is **reviewed** again instead, because what it says is a person's
judgement. Only where there is a hash to disagree with: a crate declared by
version has its sources in Cargo's registry cache, and refusing on that absence
would refuse every crate that comes from a registry.

*What it needs, in the record's order (§5):* the rustdoc-JSON reader behind a
toolchain check; and a way to read a version dependency's sources, which is
what would make the hash rule reach every crate rather than the ones with a
`path`.

*And one line of the merge is decided in the cheapest direction rather than
decided.* A description's names carry the crate word in front of them and
`std`'s carry a module's, so a manifest declaring a crate whose word is one of
`std`'s modules would have two answers for one name. `std` wins today, silently,
which is a silence and not an answer — no manifest in this repository reaches
it, and the day one does it is a refusal to write. A **suspicion** rather than a
defect: nothing reproduces it.

*And the line something else was waiting on is written.* The describer reads a
`pub struct`'s **fields** now ([ADR-123](specification/adr/adr-123.md) D2), so
`crosses = false` on `hyper_shim::LocalHandle` is the command's answer rather
than a hand's — which is what the entry above about the crossing refusals being
unreachable into **our own code** was waiting for: a described foreign type is
the first thing that can answer `MayNot` there, and `NK2501` has something to
say the day a program `spawn`s one. `NK2502` and `NK2503` needed more than that — the call
itself had to be one **nothing** describes — which is
[ADR-193](specification/adr/adr-193.md)'s `threads` column, decided at 0.0.149
and built through 0.0.163: a described call is asked now, and what is left of
that record is the I/O half, §2.44 below.

### 2.21. An `update` block says `mut`, may run more than once, and the compiler picks the lock

[ADR-110](specification/adr/adr-110.md). `update fn(mut v) { … }` is the one
form; `v` is a copy where the value fits a machine word — run the block on the
copy, compare-and-swap, retry on a collision — and the address in the lock
otherwise, where the block runs once; nothing is moved out of the lock and no
slot is ever empty. `update_all` takes one `mut` per lock. `access` reads.
**D1, D4 and D6 are built, and D2's address row with them**: `fn(mut v)`
parses, both doors are handed a view they may write through, the `Option` is gone and with it the
empty slot — and the `emptied()` panic that named that state, because it cannot
occur. **What is left is speed**: a copy and a compare-and-swap where the value
fits a machine word. The block runs exactly once today, which D3 licenses
outright (*may* run more than once is a permission, not a requirement).

*And Part II 12.2's counter runs again.* `counter.update fn(mut n) { n += 1 }`
had stopped lowering the day the page was rewritten to this form;
`tests/specification/COMPILES.txt` carries it as a program rather than a
fragment now.

*One thing the record had not said, and the corpus settled it.* `NK1138` at a
door is D1's, and the first build asked it of **every** lambda parameter, on the
reasoning that one changed without `mut` is already broken Rust. It is not:
`par_fold(…, fn(acc, m) { acc.record(m) })` in `examples/1brc.nika` changes
`acc`, has no `mut`, and compiles — the **emitter** writes the word itself where
it recognises a fold's accumulator, and `and_modify fn(tally) { tally.bump() }`
in `k-nucleotide.nika` is the same shape one library over. So the question is
asked at a door and nowhere else. **Widening it is work of its own**: it means
taking that `mut` out of the emitter and making every such lambda say the word,
which is a corpus migration rather than a rule change.

*What it needs, in the record's order (§5):* the compare-and-swap loop for
word-sized values and the second lowering, with `explain` naming which row a
value fell in; `--sharing` on a large `get`.

### 2.22. A cleanup the deadline cut off names the resource

[ADR-112](specification/adr/adr-112.md). **Steps 2 and 3 are built**: an
expired `cleanup-deadline` ends the program with exit status 70, and the
message goes the panic path — standard error and the program's panic hook,
never standard output — with a test that runs a second process, expires its
deadline and reads both. `cleanup-deadline = "0"` does not drain and
therefore never expires, which is D3 and needed no code.

*What is left is step 1, and it is [ADR-006](specification/adr/adr-006.md)
D3's.* The parked-cleanup queue does not exist, so what expires today is the
drain of pending **I/O operations**: the message counts them rather than
naming the resources whose cleanup was cut off, which is what D2 asks for.
When the queue lands the names go in the same message on the same path, and
nothing about the status changes.

*One limit of the build, named rather than left to be found:* under
`panic = "abort"` the process is gone with the abort's own status before the
exit code can be set. The hook has already run and said what happened, so
that profile loses the status and not the message.

### 2.25. An `overlap` keeps every failure — the cleanup half

[ADR-115](specification/adr/adr-115.md) D1 and D2 are **built**
([ADR-170](specification/adr/adr-170.md)): every error carries the failures
that joined it, an `overlap`'s later failing branches join the winner's list in
written order, and an uncaught failure prints them indented under it. The
question that blocked it — where the list lives when the channel has no
envelope — was answered **A**: a body that joins puts one on.

*What is left, in the record's order:*

* **D3's cleanup attachment.** The shape is there — a secondary with
  secondaries of its own is the tree that record wants — and the attachment is
  not: a cleanup that fails while a branch is already failing should join
  **that branch's** error, and today it is attached below in the language
  below's own way with nothing a program can read.
* **Whether the list survives a hop to a caller with a bare channel of its
  own.** [ADR-170](specification/adr/adr-170.md) D1 covers the body the block
  is written in and says so: the block, its `catch` and the function around
  them, which is where a handler is written. A caller that propagates such a
  failure has a channel of its own, and making the envelope travel is the
  transitive version — a derived column like `locks`, and worth its own
  measurement rather than a guess.

*And one case the list cannot cover, named rather than discovered:* a failure
that joins an error from **below** Nikaia, which has no envelope at all, is
dropped. That is the boxed channel's downcast finding nothing, and it is the
price of the box.

### 2.28. A target without an operating system

[ADR-119](specification/adr/adr-119.md). A bare-metal target with
`user_parallelism` pinned to `no`; `no_std` emission with a target prelude
and abort; the target's executor over described crates with interrupts as
wakers and `irq::on(vector, fn() sync)`; a heap by default and an
`allocation = "startup"` profile over a derived `allocates` column; locks as
critical sections; runtime settings baked at build time. **Nothing of it is
built**: the compiler knows `x86_64-linux`, `aarch64-linux` and `wasm32-unknown`, and `std`
has one Rust half. Scheduled after the HTTP server.

*What it needs, in the record's order (§5):* the target and the pin;
`no_std` emission; the `std` half over described crates; `allocates` and the
profile; build-time settings and the deadline; the availability rows and a
first program.

### 2.30. A function-typed parameter lowers by its type

[ADR-122](specification/adr/adr-122.md). Without `sync` the future shape, run
or kept; with `sync` a plain closure; the refusal of a pausing lambda at a run
parameter goes; the box on the common case is measured before the record is
closed. **D1, D2 and D3 are built.** A parameter whose type may pause is
`impl Fn(A) -> Pin<Box<dyn Future<Output = R>>>`, a call to one carries an
`.await`, and a lambda handed to one is `|a| Box::pin(async move { … })`.

*D1's **whether it runs or keeps it** is gone*
([ADR-192](specification/adr/adr-192.md) D1, answering the question
[ADR-187](specification/adr/adr-187.md) D3 filed). A **run** parameter is
`impl AsyncFn(A) -> R` and only a **kept** one keeps the box, chosen by the
`keeps` column [ADR-102](specification/adr/adr-102.md) D3 already put the answer
in. The reason D1 gave for the box — *Rust has no stable `async` closure* — was
false and was false when it was written.

*The number is §3's:* **15.1 ns per call against 0.33 ns**, about ×45, with the
control tying (`benches/handler`). Large as a ratio and small as a number, and
which of the two matters is D1's whole argument: a handler answering a request
spends microseconds, and a lambda run a million times over a list is what
`sync` is the door out of.

*One thing the record had not said, and the corpus said it in one line.* D3's
*`std`'s own entries are untouched* is a **condition on the check** rather than
a remark: `HashMap::and_modify` describes a *Rust* signature, which takes a
plain closure whatever the ledger's `sync` says, and writing the future shape
for it produced *expected `()`, found `Pin<Box<…>>`* against
`examples/access-log.nika`. The shape is written only where the signature was
declared in this language.

*And the run shape gained a `&` at 0.0.166*: `&impl AsyncFn(A) -> R`, because a
run parameter is **immediate** and borrows (Part I 5.4 C). It was moved before,
which held for as long as no program handed a handler on — and `&F` is a function
too, so the call writes one `&` and nothing else changes.

*What is left is step 4*, `fortunes.nika` as the corpus program. `examples/http/`
declares no `route` yet; the kept lowering it needs is built
([ADR-217](specification/adr/adr-217.md) D2), so a `.route(…)` chain is the
package's to add ([§2.6](#26-the-http-server-is-built-and-what-waits-on-it-is-the-parsing-moved-into-nikaia)).

### 2.32. A library for other languages

[ADR-125](specification/adr/adr-125.md), all of it. A `pub extern "C" fn`
with a body is an entry point of a library, and `artifact = "c-library"` in
`[build]` makes the package one. What a C caller gets is deliberately narrow:
numbers by value, text and bytes in as pointer and length, text and bytes out
into a buffer **the caller owns** (size, capacity, written; a `NULL` buffer
asks for the size), a struct as an opaque handle with `_new` and `_free`, an
enum as numbered constants in declaration order, an optional as `NULL` or the
package's `NONE`. Every entry point returns an `int` status and puts its
values in out-parameters; a `throws` variant is a positive code, the library's
own failures are the seven negative ones, and `<pkg>_last_error` carries the
site and the secondary list. A caller may hand the library its allocator
before `init`. A panic is caught at the boundary and poisons the library until
`shutdown` and `init`. A pausing function is exported blocking and as
`_async`. Every handle carries a lock, and a re-entrant call on the same
thread is a status, not a deadlock. The header is generated from the ledger
and carries its hash.

*What it needs, in the record's order (§5):* the parser and the four refusals;
the artifact and the safe shape at entry points; the wrapper per entry point;
the runtime surface (`set_allocator`, `init`, `shutdown`, `last_error`,
`free`, getters); the blocking and async forms and the handle lock; the
header generator and the naming; a library called from a C program in
`examples/`, and the test that links it.

### 2.33. A struct crosses the boundary by value

[ADR-127](specification/adr/adr-127.md), all of it. `pub extern "C" struct`
has C's layout (declaration order, C padding — `#[repr(C)]` below) and crosses
by value, in, out and as a field; a `Vec` of them is an array in and the
caller's buffer out, counted in elements; `T?` of one is the `NONE` status.
Fields are numbers, `bool`, `char`, payload-free enums and other such structs,
every field `pub`; anything else is `NK1145` naming the handle as the shape.
No lock around its methods — it is the caller's memory. The layout is in the
ledger, so `--locked` catches a change. The same type serves a declared C
function.

*What it needs, in the record's order (§5):* the parser; the field check and
`NK1145`; the emitter's `repr(C)`, passing, array and buffer; the header's
`typedef struct` and the ledger's field record; an example beside the
library's. **And §4's `[f64; 3]` field**, which is
[ADR-152](specification/adr/adr-152.md)'s step 4: `Array[T, N]` is built and
lowers to `[T; N]`, so what is left is a `repr(C)` struct to put one in and the
test that it is laid out as C lays it out.

### 2.34. The symbol prefix is one line in the build

[ADR-128](specification/adr/adr-128.md), all of it. `symbol-prefix = "hc"` in
`[build]`, default the package name with `-` written `_`; a C identifier or
refused. No declaration renames its own symbol.

*What it needs:* the manifest key with its check; the header generator and
the emitter reading it.

### 2.35. An async call can be cancelled, and a stream is a callback

[ADR-129](specification/adr/adr-129.md), all of it. The `_async` form ends with
`<package>_op** op` (or `NULL`); `<package>_cancel` cancels the task at its
next pause point with `cleanup` run; `done` is called exactly once,
`E_CANCELLED` (`-7`) when the cancellation came first; `<package>_op_free`
after `done`. A function producing many results takes `fn(item) -> bool sync`,
whose `false` stops it, and the next item is produced only after the callback
returned; a returned list of text or handles is refused naming that shape.

*What it needs, in the record's order (§5):* the ticket, `cancel`, `op_free`
and the code; the `bool` callback row and the refusal message; a streamed file
and a cancelled fetch in the C example.

### 2.36. A WebAssembly library is the same entry point on another target

[ADR-130](specification/adr/adr-130.md), all of it. `target = "wasm32-unknown"`
with `artifact = "c-library"` makes `<package>.wasm`, `<package>.js` and
`<package>.d.ts` from the same declarations; `extern "wasm"` is refused. The
host takes buffers from `<package>_alloc`/`_free`; no `set_allocator`; a
handle is an offset wrapped in a class with `free()`; a pausing entry point
has only the callback form and the `.js` makes it a Promise with an
`AbortSignal` for cancel, the module's executor driven from the host's event
loop.

*What it needs, in the record's order (§5):* the target check with the two
exports and the absent forms; the `.js`/`.d.ts` generator; the executor
bridge and the Promise form; the library on a page, and the test in Node.

### 2.37. A binding is a generated file over the C library

[ADR-131](specification/adr/adr-131.md), all of it. `nikaia bind python`
writes a `ctypes` binding from the ledger (exceptions per variant, `str` and
`bytes` for buffers, classes with `close()` for handles, `IntEnum`, `None`,
generators for streams, an awaitable for `_async`); `nikaia bind js` is the
WebAssembly build's `.js`. No second artifact, no native Node add-on.

*What it needs, in the record's order (§5):* the Python generator over the
example library; the streamed and async forms; the `js` name; a test that
imports the binding.

### 2.38. `nikaia fmt` does not exist

There is no formatter. Part III's tool page names `nikaia fmt` and the CLI has
`build`, `run`, `lower-std` and `describe`; `cargo fmt` formats this compiler's
own Rust and **no `.nika` file has ever been formatted by a tool**.

*It is born with one rule already written down.*
[ADR-132](specification/adr/adr-132.md) D2: `} else if cond {` on one line, and
never unfolding a chain into nested blocks or folding nested blocks into a
chain. That rule outlived its own record — the rest of D1 is built, `else if`
parses and lowers, and `examples/http`'s `status_line` is one decision rather
than seven `if`s in a row — so what is left is not a step of that record any
more. **The formatter itself is the entry.**

*Why it has sat unnamed:* nothing else in the tree waits on it.

### 2.39. A field's prose and a variant's go nowhere

[ADR-139](specification/adr/adr-139.md) D1 gives a `///` run in front of a
**field** and a **variant** the same meaning it gives one in front of an item,
and the parser keeps neither. Everything else of that record is built: the nine
item positions, the ledger's `doc` column, and `std`'s hundred and eight
hand-written entries, which carry prose held there by a test.

*Why it sits still:* D2 gives the ledger a column for a `fn` and a `type` and
none for a field or a variant, so nothing would **read** what the parser kept.
What would is `nikaia doc`, which is that record's §4 and wants a record of its
own — so this is one piece of work with that one rather than a job waiting on
nobody.

*What it needs:* `doc_here` at a field's and a variant's first byte, which is
the same hand-written parser the items already use; a place on `FieldDef` and on
`EnumVariant` to keep it; and the reader that makes it travel.

### 2.40. The database driver checks the SQL while the program is built

[ADR-143](specification/adr/adr-143.md), all of it. The compiler knows no
SQL: a dialect is a grammar in a driver package. A grammar declares a result
column with `meta::column(name, type)`, the third and last intrinsic of the
hybrid binding, and the compiler derives the statement's row type from the
columns as it derives the parameter type from the holes. A `dsl` block takes
build-time arguments, named, no `;` — `dsl sqlite(schema: app) { … } eod` —
resolved from `comptime` values, and the driver's grammar reads the schema
with its own DDL grammar and refuses a missing column at the query. `std::db`
is the protocol only (traits, statement, row values); `sqlite` and the rest
are packages. No expression capture, no ORM; `raw(text)` for dynamic SQL.

*What it needs, in the record's order (§5):* `meta::column` and the row type;
build-time arguments on a block; `std::db`'s traits; the `sqlite` driver with
both grammars and the schema check; the example with a misspelled column
refused.

**Step 1 is blocked, and the record does not say by what.** For the compiler to
know which columns a grammar declares, the grammar has to **run while the
program is built** — and *running a grammar while the program is built is not
interpretation*, above, says that may only be done by compiling the **generated**
parser and running it. So `meta::column` waits on that entry, and so does
`meta::parameter`: today a `dsl` block's holes come from a **scan of the body
text** (`crates/nikaia/src/dsl.rs`), which that file's own note calls an
approximation, and `dsl html { … }` is the one block this compiler runs at all —
by [ADR-017](specification/adr/adr-017.md), because `html` is the target it
compiles itself.

*And the harness this waited on is built* ([ADR-177](specification/adr/adr-177.md)):
a crate holding the grammar and a `main` that parses the bytes, compiled and run
during the build, keyed on what went into it. What comes **back** is a
build-time value, and the shape this record needs — a flat list of declared
columns and parameters — is one of the five that cross. So what is left here is
the driver's own work rather than the machinery under it.

### 2.41. One name the prelude promises and `std` does not have

[ADR-154](specification/adr/adr-154.md). The rule is built for a function and
for a type; what is left is D1's own list naming **one** thing that does not
exist.

**`assert`** is on the list on Part I 1.3 and is not in `std`. It is not a piece
of work on its own: it belongs with the **testing chapter** (Part III 14), which
is not built either — there is no `nikaia test`, `assert c` parses as two
statements and is refused with `NK1117`, and `assert(c)` is a call to a function
of that name. Whatever `assert` is, it is decided there rather than here.

*The other three are answered.* **`Bytes`** is the language's and it exists
([ADR-156](specification/adr/adr-156.md) D1). **`panic`** exists
([ADR-161](specification/adr/adr-161.md) D3), ends the program with the
program's own words at the Nikaia line, and was built because
[ADR-114](specification/adr/adr-114.md) D1 writes it as the way a program says
it knows a key is present. And **`eprint`** is on the list
([ADR-162](specification/adr/adr-162.md) D1) rather than being a question:
diagnosis is the language's, and what it *does* is a thing only the compiler
knows, because only the compiler knows the target.

*A name on the list that does not exist* is the one direction a prelude can be
wrong in without anybody noticing — nothing refuses it, because nothing reaches
it. **The other direction is now watched too**
([ADR-162](specification/adr/adr-162.md) D3): a test reads the list off the page
and holds it against what `std` keys bare, which is what found `access_all` and
`update_all` sitting there unlisted.

*What stays bare on purpose:* `Shared`, `SharedMut` and `Locked` are the
language's ([ADR-064](specification/adr/adr-064.md)) rather than a module's, and
a `TaskHandle` is what a `spawn` hands back — a program has no reason to write
the name, so moving it would cost a migration and buy nothing.

### 2.44. The describer's remaining half: `cargo metadata`, a directory walk and a subprocess

[ADR-195](specification/adr/adr-195.md) D4 and
[ADR-196](specification/adr/adr-196.md) D4, and what is left of
[ADR-193](specification/adr/adr-193.md) is none of it.

***[ADR-193](specification/adr/adr-193.md) is built.*** The `threads` column
with its three values and a refused third spelling (0.0.160); `NK2502` asking a
**described** call, which is the half of
[ADR-038](specification/adr/adr-038.md) D7 that was left open — a crate that
answered every other question honestly used to turn the check off by being
described; the signature scan and the note it writes (0.0.162); the
`unsafe impl Send` flag, which arrived with the grammar rather than needing work
of its own; and the intra-crate call graph with the `use` table (0.0.163), for
the row where an `unsafe impl Send` took the bound away. **Both crossing
experiments under `examples/foreign-runtime/` are refused by this compiler** —
`NK2502`, in Nikaia's vocabulary, on the author's line — including `smuggled/`,
which [`foreign-runtime.md`](history/foreign-runtime.md) §3.5 did not expect.

***And the parser is built too***
([ADR-195](specification/adr/adr-195.md) D3, [ADR-196](specification/adr/adr-196.md)
D1): `crates/nikaia-std/src/tools/rust.nika` is a Nikaia grammar, lowered ahead
of time, `include!`d as an ordinary Rust module and driven from Rust. Measured
against the scanner it replaced, on one file: the scanner reported **four
functions that do not exist** — one inside a block comment, one on the second
line of a string literal, two inside a private `mod` — and put a fifth at the
crate root instead of under its module. The grammar reports none of them, and
it refuses no re-export, which the scanner refused every one of.

*So what is left is the **I/O half**, and it is not this record's.*
[ADR-195](specification/adr/adr-195.md) D4's order, with the first step done:

1. ~~**the grammar**~~ — **built**.
2. **`fs` gains a directory walk, and `std` a subprocess.** Both are the kind of
   operating-system resource [ADR-194](specification/adr/adr-194.md) D1 put the
   socket in `std` for, and the subprocess is what `cargo metadata` is run
   through — which is the one thing `nikaia describe` still cannot do for a
   crate declared by **version**, whose sources are in Cargo's registry cache.
3. **the command rewritten in `.nika`**, in the sysroot, pre-lowered at release
   by `nikaia lower-std`'s own step — which is the route the grammar already
   took.

*And the staging has a written end and a sign that it has stalled*
([ADR-196](specification/adr/adr-196.md) D4, which takes steps 2 and 3 **off**
the reading half's critical path): the Rust half keeps the I/O and hands the
grammar the text. **The sign is concrete** — `fs` has a directory walk and the
Rust half is still doing the walking — and it is a thing to look for rather than
a gate, because a gate on it would be a gate on work nobody has started.

*One rule of [ADR-193](specification/adr/adr-193.md) is worth keeping here,
because it is the thing a first implementation gets wrong:* the describer
**proposes and never claims**, and it may propose `true` and must never propose
`false`. Nothing a signature can show entails *does not thread* — a function may
spawn something it built itself — and
[ADR-123](specification/adr/adr-123.md) D2's licence to fill `crosses` is
**soundness**, which these indicators do not have.

*And two limits stand that a parser looked as though it would close.* The
**macro** one does: expanding one needs `-Zunpretty=expanded`, which is nightly,
and that is the same [ADR-001](specification/adr/adr-001.md) D1 wall that keeps
rustdoc-JSON out. And a **method** is read and not written down, because what an
`impl`'s `pub fn` is at a foreign boundary is
[ADR-104](specification/adr/adr-104.md) D4's own question.

### 2.45. A `ref` a program writes at a `return` is still a second spelling, and one root still needs it

[ADR-202](specification/adr/adr-202.md) D2 and its §4, which together are
[ADR-094](specification/adr/adr-094.md) D4's staging one position further on.

`return ref self.text` and `return self.text` both work, against a declared
`-> ref String` out of a `ref self` method. The second is what D1 built at
0.0.177; the first is what a program had to write before it, and
[ADR-094](specification/adr/adr-094.md) D1's own sentence — *two spellings for
one thing is the state a reader cannot tell a rule from a habit in* — is what
says one of them has to go.

**And it cannot go yet, because one root still needs it.** D1 writes the `&`
where the place is inside a borrowed **subject**, and not where it is inside a
lent **parameter**:

```nika
fn a(row: ref Row) -> ref String {
    return row.name        // NK1104: this returns `String`, and the function declares `ref String`
}

fn b(row: ref Row) -> ref String {
    return ref row.name    // this is the spelling
}
```

So the two halves are one piece of work: the inference reaches every root, and
then the written word is refused. Refusing it first takes away the only way to say
what `a` means, which is exactly why D4 staged the `let` position too.

*What it needs:* the root's binding has to be a view whose buffer is the
**caller's**, which is a question about the binding and not about the place — a
local bound to a view of something this body owns is not it. The subject is the
one root where that question has a constant answer, which is why it went first.
Then `NK1137` at the position, which is one condition and one message.

*And the corpus says when the refusal can land.* The one place that writes the
word today is `crates/nikaia/tests/borrowed_subject.rs`'s
`a_view_of_the_field_is_the_free_way_out`, which is a test *of* the spelling.

*Why it is here and not in §1:* nothing is miscompiled, and the program above has
a spelling — `b` — so no correct program is refused. What is open is a language
with two ways to say one thing.

### 2.46. The emitted code writes no `unsafe`

[ADR-218](specification/adr/adr-218.md) D4's last step. `nikaia-std` holds no
`unsafe` (`#![forbid(unsafe_code)]`), but generated programs still call
`tether::forever` inside `unsafe { … }` for a task's keep
(`crates/nikaia/src/emit/mod.rs`, ADR-209 D3). **Half done**: the per-view
handle is `Held::new` since [ADR-221](specification/adr/adr-221.md) D1, which
finds the view in the keep by address instead of trusting the caller. The
task's packed values move onto the same shape — `Holding` over the task's
keep. Evidence: `grep -n 'unsafe {{' crates/nikaia/src/emit/mod.rs`.

## 3. Upkeep

A page that says something a later decision made false.

### 3.3. Three corpus files cannot be compiled with `--input`, and none of them is broken

A sweep that runs `nikaia --input` over every `.nika` file in the tree reports
three failures, and **all three are the sweep's method rather than the corpus**.
They are written down here so that the next reader does not measure them again.

* **`examples/hello-http/src/main.nika`** and **`examples/fortunes.nika`** write
  `use http`. A package reached by name is declared in `[dependencies]`, which
  lives in `nikaia.toml`, which `--input` does not read — so the refusal is
  correct and says so. `nikaia build` inside `examples/hello-http` compiles it
  and its `http` dependency and finishes clean; `crates/nikaia/tests/project.rs`
  is where that is a gate.
* **`examples/inventory/page.nika`** is one file of a package whose `Entry` is
  declared in `stock.nika` beside it. Compiled alone it is a file referring to a
  type nothing in it declares, which is `NK1135` doing its job.

*What is actually open about `fortunes.nika`* is neither of these: it is written
at specification level and `examples/README.md` lists its gaps — **G6**, the
runtime binding that lets a handler see the request
([ADR-018](specification/adr/adr-018.md)), and the `postgres` block, which is the
database driver above and is itself blocked. Its `render` lowers and runs today.

*So the corpus check to trust is the test suite*, which builds each project the
way a project is built, and not a loop over every file.

## 4. Where the other lists are

* [`project_status_and_roadmap.md`](project_status_and_roadmap.md) — the phases,
  and what runs today. The long view; this file is the short one.
* [`handoff.md`](handoff.md) — how to work on the **parser backend**: how to test a
  change against Nikaia, what the patch does, and what was tried and must not be
  redone. A guide rather than a list; what was open in it is an entry above.
* [`spec-promises.md`](spec-promises.md) — every construct the specification
  names, probed against the compiler — the place to look before adding an entry
  to §3 here.
* [`error-corpus.md`](error-corpus.md) — twenty-six broken programs and what the
  compiler says about each.
