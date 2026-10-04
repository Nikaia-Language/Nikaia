# Nikaia Examples

Fourteen of the fifteen programs here compile, run, and are checked by `cargo test`. The
fifteenth is written at specification level — it shows what Nikaia 0.0.7 is meant to look like,
and what it needs is listed under *Gaps* below.

Two of them are a **pair**: `http/` is a package and `hello-http/` is a program that reaches it
by a path ([ADR-289](../docs/specification/adr/adr-289.md) D1). Everything else here is a program
on its own, so the pair is the only place in this directory where the dependency arm is written
down rather than described.

**Eleven of them are packages that test themselves**
([ADR-269](../docs/specification/adr/adr-269.md) D13, [ADR-247](../docs/specification/adr/adr-247.md)):
`calc/`, `access-log/`, `config/`, `json/`, `n-body/`, `k-nucleotide/`, `escaping/`, `trend/`,
`tally/`, `report/` and `inventory/` each hold `nikaia.toml`, `src/` and a `tests/` directory:
`NAME.stdout` is what the program must print and `NAME.out/` the files it must write;
`NAME.in/` is copied into the directory it runs in, and `NAME.args` and `NAME.stdin` are what it
is given. Each test runs in a fresh directory of its own. In one of them:

```text
nikaia run -- "2 + 3 * 4"      # in examples/calc
nikaia test --both-settings    # every output test, and calc's `test` blocks, at both settings
nikaia test --bless            # write the expectations from what the program did; review the diff
```

`crates/nikaia/tests/examples.rs` runs `nikaia test --both-settings` in a copy of each.

| | what it is | runs |
| :--- | :--- | :--- |
| [`1brc.nika`](1brc.nika) | the One Billion Row Challenge: a frame, a parallel fold, a billion rows | ✅ `crates/nikaia/tests/one_brc.rs` |
| [`calc/`](calc/) | a four-function calculator: the grammar protocol at its smallest | ✅ `nikaia test`: its own `tests/` |
| [`access-log/`](access-log/) | a web log summarised: several fields per line, a report at the end | ✅ `nikaia test`: its own `tests/` |
| [`config/`](config/) | an INI file with comments: a grammar that defines its own whitespace | ✅ `nikaia test`: its own `tests/` |
| [`json/`](json/) | a JSON document: a tree of unbounded depth, and where zero-copy stops | ✅ `nikaia test`: its own `tests/` |
| [`n-body/`](n-body/) | the CLBG benchmark: arithmetic in a loop, and no grammar at all | ✅ `nikaia test`: its own `tests/` |
| [`k-nucleotide/`](k-nucleotide/) | the CLBG benchmark: FASTA on standard input, counted | ✅ `nikaia test`: its own `tests/` |
| [`escaping/`](escaping/) | an HTML table: the template escapes, the type says what is markup | ✅ `nikaia test`: its own `tests/` |
| [`report/`](report/) | a stock file in, an HTML page **written to disk**: the first result that is a file | ✅ `nikaia test`: its own `tests/`, the file it writes included |
| [`trend/`](trend/) | ten days of readings, looked at **from the end**: a pipeline turned round without a copy, and a range walked twice | ✅ `nikaia test`: its own `tests/` |
| [`tally/`](tally/) | a pipe read line by line in **constant memory**, and a loop that can fail | ✅ `nikaia test`: its own `tests/`, the file it writes included |
| [`inventory/`](inventory/) | `report.nika` again, in **three files**: what a module boundary buys and costs | ✅ `nikaia test`: its own `tests/`, the file it writes included |
| [`fortunes.nika`](fortunes.nika) | the TechEmpower benchmark: a SQL DSL and an HTML template DSL in one handler | ❌ needs G6 and G7 |
| [`http/`](http/) | the `http` **package**: what a handler is given and gives back, and **the server** — one connection at a time, with the caps as options | ✅ through its consumer |
| [`hello-http/`](hello-http/) | a **server**, and a program that reaches `http` **by a path**: one address and one function that decides | ✅ `crates/nikaia/tests/project.rs`, over a real socket |
| [`sqlite/`](sqlite/) | a real **C library**, end to end: a buffer, two handles with their `cleanup`, an out-parameter and text the library owns | ✅ `crates/nikaia/tests/foreign_pointers.rs`, skipped where the machine has no `libsqlite3` |

Each of the twelve programs - eleven of them packages now - is compiled and run **at both
settings**, and their output must be identical — that is the claim the switches rest on, and a test is where it belongs
rather than in a paragraph. The pair is built once, through its consumer, because what it is
there to show is the dependency arm rather than a body of code. Each is the real file: the
tests read the examples rather than a copy, so an example cannot drift from what is checked.

They are deliberately different shapes.

* **`1brc.nika`** is the protocol at scale — `@frame`, `par_fold`, a memory-mapped file, one
  accumulator per core ([ADR-014](../docs/specification/adr/adr-014.md): 8 million lines
  over every core, identical output), and one function it calls for every digit is
  written in Nikaia and compiled into `std` by the compiler itself.
* **`calc.nika`** is the same protocol with none of that: no frame, no fold, no I/O, one
  `entry rule` that returns a number — recursion and precedence, which is what a grammar can say
  and a chain of combinators cannot.
* **`access-log.nika`** is the shape most real work has: a line with several fields of
  different kinds, a record built from them, a report at the end — including what a *rejected*
  line looks like.
* **`config.nika`** is the one whose result is a *tree* rather than a number or a tally, and
  the one that defines its own `WS` so that `#` comments are legal everywhere a blank is
  without another rule mentioning them. It is also the counterpart to 1BRC's fold: `setting*`
  collects, which is right for a configuration file and wrong at a billion rows.
* **`escaping.nika`** is the one about a *contract* rather than a shape: the `html` template
  escapes every hole, the type `html::Raw` is the only way to say a value is already markup, and
  a hole in a position escaping cannot make safe is a compile error naming the position. It is
  the smallest program that shows all three.
* **`inventory/`** is the one made of **more than one file** ([ADR-286](../docs/specification/adr/adr-286.md)) - the same program `report.nika` is, split into the parse, the page and the program that joins them. Both are here on purpose: what a module boundary buys and what it costs, on something small enough to hold in the head. What it buys is that `stock.nika` keeps `weight` to itself and that neither file has to be read to use the other; what it costs is visible too, because a grammar is not visible across a boundary and `stock.nika` had to wrap its own in a `pub fn read`.
* **`report.nika`** is the one whose result is a **file**. Every other example here ends at
  `println`, and a program that cannot produce a file is not a tool: this one reads a stock
  list, orders it, renders a page with the `html` template and writes it with `fs::write`
  (Part III, 17.1). It is also `escaping.nika`'s claim tested against data that was not written
  to make the point — the `&`, the `"` and the `<` come out of the *input file*, which is where
  they come from in every program that has ever had an escaping bug. The test reads the page
  back rather than the line the program printed, because the page is what it produced.
* **`tally.nika`** is the one whose **memory does not grow with its input**. `k-nucleotide.nika`
  also reads standard input and reads *all* of it, which is right for FASTA and wrong for a log
  that never ends; this one holds one line at a time. It is also the smallest program that shows
  the rule of [ADR-025](../docs/specification/adr/adr-025.md): each turn of `for line in
  io::lines()` reads, a read can fail, nothing marks it, and the failure leaves the function —
  which is why `throws` is on the signature and why the compiler puts it there
  (`NK2701`).
* **`trend.nika`** is the one that walks a pipeline **from its back end**
  ([ADR-293](../docs/specification/adr/adr-293.md)). Every answer is the newest few of
  something - averages over a window, day-over-day changes, every third day - and none of them
  reverses a copy: `windows` hands out views, `map` computes one value at a time, and `rev`
  walks the result backwards, which the compiler allows because it knows the list has a back
  end and that `map` keeps it. The days are a range kept in a name and walked twice.
* **`k-nucleotide.nika`** is the one that reads **standard input**, and the difference a pipe
  makes is the point of it. `1brc.nika` maps its file and copies nothing at all; here there are
  no pages to point at, because the bytes do not exist until they are read — so the program owns
  its buffer, and everything after that first copy is the same. The fragments it counts are
  thousands of views into that buffer, not strings.
* **`n-body.nika`** is the one with **no grammar in it at all**. Five programs in a row that
  all begin with a DSL would say Nikaia is a parser generator; this one is arithmetic in a
  loop — `sync` methods, `ref mut self`, indices and floats — and it is the only example here
  whose numbers are directly comparable against other languages, because the CLBG publishes
  the same program in some thirty of them with an exact expected output. Ours matches it to
  the digit.
* **`json.nika`** is the first input whose shape the *grammar* does not fix. `config.nika`'s
  tree is two levels deep because the grammar says so; a JSON value contains values, to
  whatever depth the document happens to go. An `enum` is that tree — six variants, and every
  walk over it is a `match` the compiler checks for completeness — and this is also the example
  where zero-copy stops and says so: a JSON string is not a slice of the input, so `Text` holds
  the **raw** body and decoding waits until a program asks about it.

**One grammar that was here is not any more, and where it went is the point.**
`rust-signatures.nika` read a Rust file's public surface, and in 0.0.157 it moved to
`crates/nikaia-std/src/tools/rust.nika` - the reading half of `nikaia describe`
([ADR-290](../docs/specification/adr/adr-290.md) D13), lowered ahead of time and reached by the
compiler as an ordinary Rust module ([ADR-290](../docs/specification/adr/adr-290.md) D16). It is
not an example any more because it is a tool, and a copy of it here would be a second grammar to
keep in step with the first. `crates/nikaia-std/tests/rust_signatures.rs` is what runs it.

An example that is added has to be declared: either it runs and says what it prints, or it is
specification-level and its gaps are here. `crates/nikaia/tests/examples.rs` fails on a file
that is neither.

What the bootstrap compiler handles: functions and methods, `impl` blocks, `struct` and `use`
items, `let`, assignment, `for`, `if`, `return`, calls, field access, indexing, casts, struct
literals and constructors, lambdas, operators, `throws`/`catch`/`??`, string interpolation — and,
since [ADR-296](../docs/specification/adr/adr-296.md), the whole `grammar` construct: rules,
patterns, `@frame`, `fold`/`par_fold`, and `dsl … from …` with the driver `user_parallelism` asks for.
Errors are reported on the `.nika` line that caused them
([ADR-300](../docs/specification/adr/adr-300.md)), and since
[ADR-024](../docs/specification/adr/adr-024.md) types are checked before any Rust is emitted -
everything the ledger writes down, and nothing it does not. Every file here is part of that
checker's guard: the build fails if any of them produces a finding.

That is deliberate, and it is what these files are *for*. Writing a real program against the
spec is the cheapest way to find out which parts of the spec are underspecified. The gaps each
example exposed are listed below; they read as a roadmap.

`tests/samples/` holds smaller programs that only have to *parse* — one construct each, checked
by `crates/nikaia/tests/samples.rs`. An example graduates the other way: when the compiler
catches up with one, it stays here and gains a row in `RUNNABLE`, because what makes it worth
having is that it is a whole program.

---

## Language benchmarks worth targeting

There are three established suites plus one modern challenge that fit a systems language.
They test different things, and only one of them matches what Nikaia currently claims to be
good at.

### 1. One Billion Row Challenge (1BRC) — **recommended first target**

Read a 13 GB text file of `station;temperature` lines, aggregate min/mean/max per station,
print them sorted. One input file, no dependencies, a one-paragraph spec.

It is the best fit because it stresses exactly the three things 0.0.7 asserts:

* **Scannerless parsing** (ADR-296 D1) — a billion lines of a tiny custom format.
* **Zero-copy / tethered slices** (Part II, 10.6) — station names must point into the input
  buffer; allocating a billion strings loses by an order of magnitude.
* **`par_iter` and the `sync` rule** (12.1, 12.6) — the aggregation is pure computation, so
  `user_parallelism = yes` can use every core, and the compiler can prove no task pauses.

If Nikaia is slow here, the grammar protocol's performance argument is wrong. That makes it a
useful benchmark rather than a demo. See `1brc.nika`.

### 2. Computer Language Benchmarks Game (CLBG) — **best for comparable numbers**

Ten programs with exact output specifications and reference implementations in ~30 languages,
so results are directly comparable against Rust, C and Go.

| Program | What it exercises in Nikaia |
| :--- | :--- |
| `n-body`, `spectral-norm`, `mandelbrot` | `sync` functions, `par_iter`, parallelism |
| `binary-trees` | allocation and deterministic teardown (`Drop` / `Cleanup`, ADR-297) |
| `fannkuch-redux` | scoped tasks (12.7) |
| `reverse-complement`, `k-nucleotide` | **stdin/stdout IO** plus hashing |
| `regex-redux` | the `grammar` construct against a real regex workload |
| `pidigits` | bignum FFI (Chapter 15) |

Small, self-contained, no external services. `n-body` is the usual first one because it is
~100 lines and purely numeric.

### 3. TechEmpower Web Framework Benchmarks — **what `user_parallelism = no` is actually for**

`plaintext`, `json`, `db`, `queries`, `fortunes`, `updates`, `cached-queries`.

This is the benchmark that matches Nikaia's pitch: implicit async, IO density, share-nothing,
WASM-compatible single-threaded build. `fortunes` in particular exercises the 0.0.7 DSL protocol
end-to-end — a SQL DSL with deferred parameters *and* an HTML template DSL with capture holes,
in one request handler. See `fortunes.nika`.

The cost is real: it needs an HTTP server, a Postgres driver and the Docker harness. This is a
target for after self-hosting, not before.

### 4. Are We Fast Yet? — **for judging codegen later**

Marr et al.'s cross-language suite (DeltaBlue, Richards, Havlak, CD, Json). Deliberately
written using only constructs every language shares, so it measures the *compiler* rather than
library tricks. No IO. Worth revisiting once there is a real backend to judge.

**Not applicable:** DaCapo and Renaissance (JVM-only), SPEC CPU (licensed, C/Fortran),
CoreMark (embedded C microbenchmark).

### Suggested order

1. `1brc` — validates the 0.0.7 claims, needs only file IO. ✅ [`1brc.nika`](1brc.nika)
2. CLBG `n-body` — first comparable number against other languages. ✅
   [`n-body.nika`](n-body/src/main.nika), matching the published output for n = 1000 to the digit.
3. CLBG `reverse-complement` and `k-nucleotide` — stdin/stdout IO with exact expected output.
   ✅ [`k-nucleotide.nika`](k-nucleotide/src/main.nika); `reverse-complement` needs binary output, which
   `std::io` specifies as `bytes` and does not yet have.
4. TechEmpower `fortunes` — once an HTTP stack and a DB driver exist.

---

## Gaps these examples exposed

Writing the two programs surfaced spec questions that a real implementation must answer.

### Resolved

**G16 — a type could not be named by a path.** `Shared[postgres::Connection]` did not parse:
a type was a name with optional arguments, and `postgres::Connection` is a name with a path in
front of it. Found by `fortunes.nika` the moment its template stopped being the first thing that
failed. The whole path is interned as one name, because that is what the name *is* to a compiler
that lowers name for name (ADR-296 D17) — nothing here resolves a module, and a path can therefore
never collide with a struct the file declares, which is correct.

**A hole in an interpolated string could not hold a string literal.** `"{f(\"a\")}"` handed the
parser `f(\"a\")`, which is not an expression. A hole is Nikaia source that was written *inside*
a string literal, so the escaping it carries is that literal's: the two characters the enclosing
string had to escape are undone before the hole is parsed, and every other escape keeps its
meaning.

**G18 — a function could not take named arguments with defaults.** *Implemented.* Part I 5.1
specifies the *Subject ; Config* protocol — a `;` in a signature, positional data before it and
named options after it — and nothing parsed it. Found by `fs::write`, whose specified surface is
`write(path, data; append: bool = false, create: bool = true)`: the two options were missing from
`std` because there was no way to write a call that passes one.

Both sides parse now, and the lowering is the interesting part. The language below has neither
named arguments nor defaults, so an option becomes an **ordinary parameter in declaration order**
and a call fills in whatever it left out — which means the expansion needs the *callee's*
declaration, not the call. That is what the ledger already records (Part III, 13.5), so
`signature` gained the options with their defaults and `fs::write` is checked and expanded from
the file `std` ships. Every configuration parameter has a default, because that is what makes it
an option; a default is a literal, because deciding when an expression would be evaluated is worth
doing when something needs it. `NK1109` is a call that names an option the callee does not have,
with the one that was probably meant.

`examples/tally/src/main.nika` uses both halves: it declares one option and passes one to `fs::write`.

**G17 — a stream of lines that can fail while it is being read.** *Decided and implemented*
([ADR-025](../docs/specification/adr/adr-025.md)). Found by trying to build `fs::lines` and
`io::lines`, and it turned out to be two problems rather than one.

The **failure** half: `for line in io::lines()` performs a call per turn that nobody wrote and no
signature declares, so there was nowhere for `throws` to attach — and pressing three answers (a
line, the end, *I could not find out*) into an iterator's two makes a failed read look like the
end of the stream. A truncated file becomes a shorter file, silently. Part I 6.4 already refuses
exactly that at the *closing* brace of a block, where a resource's cleanup can fail, and calls it
"a decades-old bug class in other languages". So the rule was already written and was never
stated generally: **an implicit call that can fail fails the enclosing function, and the compiler
makes you declare it.** Cleanup was its first case, iteration is its second, and `NK2701` is what
the compiler says. Nothing marks the loop, because [ADR-023](../docs/specification/adr/adr-023.md)
D8 leaves three of the language's four invisible control-flow events unmarked and marking the
fourth would imply the other three were absent.

The **ownership** half is why `fs::lines` is *gone* rather than waiting: `lines(path)` was to open
the file and yield tethered `ref String`, so the returned value would own the buffer and hand out views
into itself — the one thing an iterator may not do, and the reason the language below allocates a
string per line when it offers the same function. The shape that works is two calls,
`fs::map(path, root)` + `.lines()`, and that separation *is* the model Part I 6.6 rests on. For a
record-per-line file the language has something better than a sequence of lines anyway:
`@frame(boundary: "\n")`, which is what `1brc.nika`, `access-log.nika` and `config.nika` all use.
None of them iterates lines.

`io::lines()` exists, because a pipe is genuinely different — its bytes do not exist until they
are read, so the failure cannot be moved to the call — and `examples/tally/src/main.nika` is it running.

**G1 — `std::fs` named no functions.** The module was described ("looks blocking, is async")
but had no surface. Now specified in Part III, 17.1: whole-file (`read`, `read_to_string`,
`write`), streaming (`lines`, `bytes`), handles (`open` → `File`, which implements `Cleanup`
so a failed flush surfaces as an error instead of being swallowed), memory mapping (`map`,
a **compile error on `wasm32`** because WASM has no mmap and degrading it to a full read would
turn a constant-memory program into one that allocates its whole input), metadata and
directory calls, and an availability table.

**G2 — a grammar rule that folds instead of collecting.** An earlier draft of
this note claimed a gap around "applying a grammar per line". That was wrong,
and it misread what the language offers: you do not run a grammar per row. You
write down what the *file* looks like and get a parser for it, which drives
itself over the bytes — paying the DSL protocol once instead of a billion times.

The real gap was narrower and only appeared at the entry rule:
`rule file -> Vec[Reading] = measurement*` collects, and a billion `Reading`s do
not fit in memory. `fold` now exists in `winnow-grammar` (upstream, with
`SYNTAX.md` documentation and tests), so the entry rule threads an accumulator
and never builds a collection:

```nika
entry rule file -> Summary =
    fold(measurement, Summary::new, fn(acc, m) { acc.record(m) })
```

**G3 — tethered slices in user structs.** Specified for tokens a parser yields (Part II,
10.6), but not for a slice stored in a struct of one's own. `Reading.name` and the `HashMap`
key in `1brc.nika` are exactly that, and it is the difference between one allocation and a
billion.

Writing the example did more than expose the gap — it showed the existing rule was stated over
the wrong event. "Stored in a struct ⇒ tethered" (Part I 6.6, as of 0.0.7) puts a shared handle
on `Reading`, which this program builds a billion times; with parallelism that is a
billion atomic increment/decrement pairs on one refcount word shared by every worker. Nothing
in the program actually escapes, so the right answer costs nothing at all.

[ADR-283](../docs/specification/adr/adr-283.md) settles it: view types stay (`ref String` is a view
marker, not a lifetime), the rule is restated over **escape** rather than storage, a view has
three inferred states (Borrowed ⊑ Tethered ⊑ Owned), the shared handle sits on the *container*
rather than on each slice, and `.clone()` is never inserted for you. **A struct writes nothing
to stay a plain reference**, and the word that would let one keep its buffer alive is `@tethers`
([ADR-283](../docs/specification/adr/adr-283.md) D4) — not built, because the state it permits is
not. Under those rules the program allocates nothing per row and does no refcount work in the
parallel section.

**G4 — chunking a buffer for `par_iter`.** `par_iter` is specified over a collection.
Splitting a buffer at line boundaries into one chunk per core, with each chunk a slice of the
original, had no spelled-out form.

[ADR-296](../docs/specification/adr/adr-296.md) closes it by **dissolving** it rather than
specifying `split_aligned`. A chunking helper asks the user to restate as an argument what the
grammar already says — that a measurement ends at `"\n"` — and then to hand-write the
split/parallel/merge pipeline that follows from it. Instead the grammar carries both halves:
`@frame` marks a rule as a resynchronization unit — and the compiler *verifies* it, and never
rewrites the grammar to make it so: what a frame can reach is either safe or rejected with the
rule and pattern named (a literal containing the boundary — CSV with quoted newlines — `any`,
`multispace0`, a syntactic rule whose implicit whitespace eats newlines, an `until` that does not
cover the boundary, `recover`). The flagship `NAME` therefore *says* `until(";" | frame_end)`,
where `frame_end` names the boundary of the enclosing frame; an intermediate design that silently
bounded `until(";")` was rejected in review because the same rule text then parsed differently
depending on what reached it. `par_fold(rule, init, step, merge)` supplies the monoid, and its
parser skips no whitespace at its entry, so pieces and the sequential parse agree on every input,
rejections included. All of it is in `winnow-grammar` `main` (`#[frame(boundary = …)]`,
`frame_end`, `par_fold`, `unchecked` for the formats a byte-string boundary cannot cut — its
ADR 24 names them), with `frames_<RULE>`, `merge_<RULE>` and the driver
`parse_<RULE>_pieces(input, ctx, Parallelism)` generated; Nikaia chooses the `Parallelism` from
the switch and the executor. The blind split, the
seam repair, the per-core accumulators and the reduce are then generated; `1brc.nika`'s `main`
is down to `let totals = Measurements.file(data)`.

The same ADR settles what the compiler may then do with the format the grammar states:
word-at-a-time scanning as a specified complexity rather than a hoped-for optimization
(portable SWAR baseline, SIMD only as a target-gated layer, so a single-threaded build and `wasm32` keep the same
story), hashing a view from its first bytes while equality stays full-content, and skipping the
unmap of a large read-only mapping at process exit. Parallelism stays opt-in: a plain `fold` is
never parallelised behind your back, because a merge over floats would make the answer depend on
the core count.

**G8 — the default hasher.** Raised by ADR-296, which framed it as a question about the build mode and was
wrong to. A switch answers "which runtime", never "who supplied these bytes": a single-threaded
single-threaded server hashing attacker-supplied header names is exactly as vulnerable as a parallel one,
and a compute job over operator-chosen data has no adversary in either.

[ADR-010](../docs/specification/adr/adr-010.md) decides it on the axis that matters, **provenance**,
and at the level where the user actually knows the answer — the place the input enters. Sources are
classified by `std` (network, IPC and database rows untrusted; files, argv, env and compile-time data
trusted), the state travels the edges ADR-283 already tracks and lands in the same Ledger, joins
conservatively, and fails safe at `dyn`/FFI barriers. The user overrides it at the source
(`fs::map(path, root; trusted: false)`) and a DSL for a wire format can pin an `@untrusted` floor its
callers cannot lower. Only then does the compiler pick an implementation: keyed hash with a random
seed for untrusted keys, fast hash for trusted ones — the switch enters as *how*, never as
*whether*. 1BRC keeps the fast path without a word about hashing; `fortunes` gets hardened without
anyone remembering to ask.

**G9 — bounded repetition (`digit{1,2}`) in the grammar protocol.** ADR-296 D13: fixed-width
numeric parsing is only sound where the grammar states the width bound. Delivered upstream —
`p{n}`, `p{n,}`, `p{n,m}`, together with a single-digit `digit` terminal that turned out to be
missing too (only the greedy `digit1` existed, which would have swallowed the run before a bound
could count anything). `1brc.nika`'s `TENTHS` now states its width and rejects a three-digit
temperature.

**G5 — ordered iteration over a map.** 1BRC's output must be sorted by station name, and
`access-log.nika` wants its paths by hit count; neither an ordered map nor a way to say an order
over a list was specified. ADR-010 D6 is why it cannot be left to the map: an untrusted one is
seeded randomly, so its iteration order differs between runs. The answer is the explicit form,
and now it is written down — Part I 4.5 specifies `sort()` and `sort_by_key`, and says both are
**stable**, which is what lets two passes state a compound order without a comparator:
`names.sort()` then `names.sort_by_key fn (name) { -report[name].hits }` is hits descending, ties by name.
`access-log.nika` prints that way.

**G10 — no tuple.** Found by `calc.nika`, which has to carry an operator alongside the operand
it applies to (`3 * 4`, where the `*` must survive until the fold). A pair of values of
different types with no name for the group is a *tuple*, and Stage 0 had no way to write one:
a type was a name with optional generic arguments, so the example declared a two-field
`struct Step` to say what one line of a tail rule should have said — a struct pretending to be
a type. Tuples now exist: `(A, B)` as a type, `(a, b)` as a value, `t.0` to read a part
(Part I, 4.5). The parts live where a named type's arguments live, so everything that already
walked a type's arguments — the view analysis of ADR-283 among them — walks a tuple's parts
without knowing about tuples. `mul_tail` yields `(ref String, i64)` and `struct Step` is gone.

**G12 — no sum type.** `calc.nika` has exactly two operators to carry, and an `enum` is what
says that. Part I 4.4 specified enums and the bootstrap compiler did not lower them, so the
example carried the operator as a `ref String` and compared it — one typo away from a bug the
compiler cannot see. Enums and `match` now lower: the three variant shapes (`Quit`,
`Write(String)`, `Move { x, y }`), and five pattern shapes, each one the language below spells
the same way so the lowering stays a transcription. `calc.nika`'s `mul_tail` yields
`(Op, i64)`. An enum that carries a view takes the input lifetime exactly as a struct does,
which is what lets one appear in a grammar rule's return type.

**G15 — a program could not read standard input.** Part III 17.1 specified `std::fs` down to
`seek` and `Cleanup` and said nothing about the input a program in a pipeline has. Decided and
implemented: [ADR-019](../docs/specification/adr/adr-019.md). A stream is not a file, and the
surface says so — no `map`, no `seek`, no length, no second read — because `fs::map`'s zero-copy
story rests on pages that exist before the program asks, and a pipe's bytes do not. The rest is
the rule `std::fs` already had: no `async`, no `await`, and **a `sync` function cannot call it**,
which is what keeps a `par_iter` body from waiting on a pipe. Provenance is Trusted, as for a
file, with `trusted: false` at the entry for the request-body case. `read_to_string` and `read`
are implemented; `lines` and `bytes` are specified and are not, exactly as `fs::open` is.

**The bug it found, and this one was silent.** An `if` in *statement* position had its branches
emitted as tails — right for `let x = if c { a } else { b }`, wrong for a guard. The last
statement of a branch was written as the branch's *value*, so `if n < k { return counts }` came
out as `if n < k { counts }` and the function carried on. Here the types happened to disagree and
`rustc` said so; in a function returning nothing it would have compiled and quietly done the
wrong thing.

**G14 — arithmetic did not read like arithmetic.** Found by `n-body.nika`, which is nothing
but arithmetic and so found four things at once. **A range was not an expression**: Part I 3.3
shows `for i in 0..5` and the bootstrap compiler could not parse it, so an index loop had no
way to be written. `..=` comes with it, and a range binds looser than the arithmetic in it —
`0..n - 1` ends at `n - 1`. **A float could not carry an exponent**, so
`9.54791938424326609e-04` had to be spelled out in zeroes, which is how a digit gets lost.

The two beneath those were bugs rather than gaps, and both were **silently wrong**. A group is
not a node — the parser drops it, because that is how the tree was written rather than part of
it — so `(a as f64).sqrt()` came out as `a as f64.sqrt()`, a cast to a type nobody named. And
the compiler's identifier accepted a **leading digit**, because the backend's `ident` does and
a grammar that wants otherwise has to say so: `1.5` parsed as the field `5` of a variable
called `1`. That one printed back identically, which is exactly why it survived — the emitted
text read the same right up until there was more after it, and `1.5e-4` was where it stopped.

**G13 — no character literal, and three things that followed.** Found by `json.nika`, whose
`unescape` has to ask what a character is: `'n'` was not an expression the language had, and
`char` was not in the table of primitive types (Part I, 2.2) either — the type was reachable
only because a type is a name. Decoding an escape is exactly the shape a `match` over
characters has, so the literal is a **pattern** as well as an expression (3.4), and the body is
kept **as written**: the language below spells `'\n'` the same way, so nothing decides twice
what it means.

Writing the same function found the two beside it. A string could not hold a `\u{…}` escape —
the interpolation scanner read the `{` as a hole and emitted a `format!` with an argument
nobody wrote — because a string's body reaches the emitter as it was typed and the scanner did
not know an escape when it saw one. And output could not be composed piece by piece: `println`
was the only way to write, so a pretty-printer that indents a tree had no way to put a fragment
on a line without ending it. `print` and `eprint` are now what `println` and `eprintln` always
were, minus the newline (Part III, 17.1). Part I gains **2.5** while it is at it: string
interpolation was in every example and in no chapter.

**G11 — a rejected parse said what was expected and never where.** Found by writing
`access-log.nika`, whose `catch` prints the failure: the message read
``expected a digit; found unexpected token ` ` `` with the rule stack under it, and no
position at all. A `ParseError` carries an offset and not the text it came from, so only the
code that has both can turn 1042 into *line 2, column 26* — and that is the driver
`dsl … from …` lowers to, which is the one place holding the input and the error together. It
now binds the input and renders against it, so the same message reads
``expected a digit; found unexpected token ` ` at line 2, column 26``.

Worth keeping apart from the two things it resembles. `docs/error-corpus.md` is about the
compiler's messages for `.nika` source, which have carried a line and a column all along; its
closing finding is that none of them shows the *line itself*, with a caret, the way a rustc
diagnostic routed through `--explain` does. This was smaller and worse: a program Nikaia
generates had no position at all.

The same example found the bug beside it: `dsl … from …` emitted its own `?`, so a `catch`
next to it was handed the value it was meant to inspect and nothing compiled. Both are pinned
in `crates/nikaia/tests/grammar_lowering.rs`.

### Open

**G6 — the HTTP handler cannot see the request.** *Decided*
([ADR-289](../docs/specification/adr/adr-289.md)), *not yet implemented* — it waits on the runtime
binding. The request is the handler's **first argument**, under the rule Part I 5.3
already has: a lambda's arguments are the ones it names, so `fn { "Hello World" }` is a handler
that does not need the request and `fn (request) { request.query("name") }` is one that does.
Nothing is added to the language. A handler *returns* what answers the request — a `String` is 200 text/plain, an
`html::Raw` is 200 text/html (ADR-017 D2 read from the other end: the type that says "this is
markup" is the type that may be sent as markup), a `Response` is itself, and a `throws` that fails
is 500 with a **generic** body and the error in the log, because an error message is written for
the operator. The request's strings are views into the connection buffer (ADR-283), so a parameter
used inside the request's scope costs nothing and one kept past it has to be owned.

**G7 — HTML escaping belongs in the template grammar's contract.** *Decided and now
**enforced*** ([ADR-017](../docs/specification/adr/adr-017.md)). Every hole is escaped,
unconditionally — no flag at the hole, and no exemption for "trusted" data, because provenance is
evidence about where bytes came from and one wrong `trusted: true` upstream becomes an XSS hole
downstream (ADR-010 D8). The one way to say "this is already markup" is the type `html::Raw`,
because a flag is a property of the call site while a type travels with the value and `Raw::new`
is one line to grep for. And a hole is only legal in a position the grammar can escape *for*: a
hole inside `<script>` or in a URL is a compile error naming the position, because a promise that
holds only in some positions would have to be qualified everywhere.

`dsl html { … } eod` is compiled where it is written: the body is split into literal markup and
holes at compile time, every hole goes through `html::Render`, and a hole in a position escaping
cannot make safe is refused with the position named. The type decides what a hole may hold —
`Raw` renders itself, text renders escaped, a type with no impl cannot go in a template — which
is ADR-017 D2 put where `rustc` can act on it - the Nikaia compiler emits the same call for every
hole and does not have to know which case it is. `examples/escaping/src/main.nika` is the whole of it in one page.

**Control flow is there too**: `<for row in :rows> … </for>`, written as an *element* because
the file is markup and an editor that highlights it keeps working, with `:rows` captured from the
enclosing scope (ADR-296 D4). The position check runs through a loop's body, so nothing becomes
safe by being repeated. `fortunes.nika`'s `render` lowers and runs today; what that file still
waits on is the `postgres` block and **G6**'s runtime binding — since
[ADR-277](../docs/specification/adr/adr-277.md) removed the `fn:` form, the whole file parses.


