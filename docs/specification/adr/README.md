# Architecture Decision Records

A record here states one decision **as it stands today**: the rule, the
reasoning that makes it the answer, and the number that decided it where a
number did. It is not a diary. It carries no account of how the decision was
reached, no version that built it and no progress report.

When a later decision changes a record, the record is **updated in place**: the
changed rule is rewritten to say what holds now, and a one-line note under it
names the decision that changed it, *Changed by ADR-MMM Dn.* A decision the
project withdraws altogether is removed from its record, with one sentence
saying what is done instead.

When a group of records is replaced by one record that states all their
decisions in their current form, the old records move to
[`docs/history/adr/`](../../history/adr/) and nothing live cites them
(`scripts/check-adr-refs.py` refuses it). The live record is the one to read and
the one to cite.

**Finding the history.** The archive's
[README](../../history/adr/README.md) maps every archived decision to the live
decision that now holds it. What changed in a live record, and when, is its
`git log`; what changed in the compiler is [`CHANGELOG.md`](../../../CHANGELOG.md);
what was tried and measured on the way is in the [notes](../..).

## What goes where

| | holds | does not hold |
| :--- | :--- | :--- |
| the [specification](..) | what the language *is*: the rule, the syntax, the guarantee | why that rule won, what the alternatives were, what anything cost |
| **an ADR** (here) | one current decision, the reasoning that settles it, and the number that decided it | how it was reached, whether or when it was built |
| the [notes](../..) and `CHANGELOG.md` | what was tried and measured, how, and what changed when | anything normative |
| [`docs/history/adr/`](../../history/adr/) | records replaced by a consolidating record, kept for the route to a decision | anything normative; no live file cites them |

A measurement belongs in an ADR only as *the number that decided it*, with the
method in the notes. A spec section states the rule; a reader who only wants to
write Nikaia never has to open a record. The full rule is
[`docs/README.md`](../../README.md) §2.

## Writing a record

Start from [`TEMPLATE.md`](TEMPLATE.md): one question, how other languages and
tools answer it, every option weighed against the four weights of
[ADR-258](adr-258.md), the decision as numbered rules, and what it costs. The
header is **Status**, **Date**, **Answers** and **Supersedes / Related**; it has
no `Built:` line and no target version. The template also lists what does not
belong in a record: progress, the route to the decision, the state of other
lists.

A change to a decision is made in the record that holds it, as above. A change
that one live record makes to part of another live record is also listed under
[What supersedes what](#what-supersedes-what).

## The index

**Status** is *Accepted* or *Open* (nothing decided yet). **Built** says
whether the compiler does what the record decides: `yes`, `no`, or
`partly: <what is missing>`. An ADR is a decision, and a decision is not an
implementation.

### Compiler and toolchain architecture

| ADR | Decides | Status | Built |
| :--- | :--- | :--- | :--- |
| [314](adr-314.md) | **What a grammar matched and what a callee promises are facts of the walk**. 1BRC's `TENTHS` kept every overflow check: the walk did not enter grammar actions and read nothing across a call (`std` publishes no `ensures`). Others: none derives facts from a text grammar (EverParse, Vest check predicates the author writes); SPARK reads an expression function's body as its postcondition. Measured: 1BRC 4.8 instructions a row (1.0 %) on its old action; in the corpus these facts unblock few of 106 unproved sites, lengths (~25), lists of lists (~28) and fields (~12) many more. Decided, B: D1 actions walked with what each binding matched (length, class, and a `dec[T]` over `n` digits below `10^n`); D2 a `char`'s code is a number; D3 a callee's `ensures` is a fact, and a `sync` function that is one `return e` publishes `result == e`; D4 only at `aggressive` and certified; D5 the criterion. A short loop's turns left open | Accepted | **no** |
| [306](adr-306.md) | An index or overflow check that a proof shows unneeded is not emitted where the build asks for it, and no setting drops a check that is not proved. How hard the compiler proves (`proving`) and whether it drops what it proved (`remove-*-checks`, off by default) are two settings; a check shown to fail is an error; a length whose elements take space is below 2^60. | Accepted | partly: a compound assignment's index, facts across calls; D12-D16 not built (#383, #390) |
| [305](adr-305.md) | The language below is Rust 1.88 or newer at Edition 2024, one floor for the compiler, `std`, the `unsafe` crates and the emitted code, checked by a CI job of its own. | Accepted | yes |
| [290](adr-290.md) | A Rust crate is described before it is called: `nikaia describe` drafts the description, it is committed and reviewed like code, and the describer is a Nikaia program joined to the compiler as an ordinary module. | Accepted | partly: the rustdoc-JSON reader, a version dependency's sources, `cargo metadata`, the rest of the command in Nikaia |
| [294](adr-294.md) | Stage 1 is reached a module at a time, leaves first; the syntax tree and the checker's types move whole, and the toolchain's Nikaia is one package. | Accepted | partly: a unit's own `copies` in its ledger, Stage 1, Stage 2 |
| [270](adr-270.md) | The prover's solver is a frontend, a logic and a CDCL(T) solver written in Nikaia, with one small checked proof format, and a build uses the committed `nikaia.proofs`. Its logic layer is a package any package may ask, also while it is built, answered by the reference solver under a counted budget (D24-D26). | Accepted | partly: the solver in Nikaia (the reference solver is Rust), the proof file, the CDCL(T) engine |
| [269](adr-269.md) | An `assert` is a contract, proved where the compiler can, checked where it cannot and published across packages, and a test is a `test` block. | Accepted | partly: the ledger's `inferred` column |
| [243](adr-243.md) | `std::process::run` starts a program and waits for it on a thread of its own, and a non-zero exit is an answer, not a failure. | Accepted | yes |
| [258](adr-258.md) | A design question is weighed by four weights: the special case pays, the compiler works for the developer, the common case costs nothing, and the language is consistent with itself. | Accepted | yes |
| [001](adr-001.md) | The toolchain is stable Rust named in one file, the parser is generated from a grammar over bytes, and the build is staged with Stage 0 a transpiler. | Accepted | yes |
| [002](adr-002.md) | The CLI wraps Cargo so crates.io works, compile-time code runs in the compiler's process rather than as a proc-macro, and a project reaches `std` through a sysroot of pre-lowered sources compiled into a keyed rlib cache. | Accepted | yes |
| [003](adr-003.md) | The language and the machinery that compiles it are two programs whose interface is Rust source text, and the CLI, cache and Cargo wrapping are generic. | Accepted | yes |
| [004](adr-004.md) | There is one lowering and it emits Rust source text, so the text a person reads is the text that is compiled. | Accepted | yes |
| [021](adr-021.md) | The build cache is ours: a committed lockfile records what determines a build, the key names every dimension, and a backend that is not here is refused by name. | Accepted | partly: `--locked` over the lockfile |

### Ownership, borrowing, cleanup

| ADR | Decides | Status | Built |
| :--- | :--- | :--- | :--- |
| [312](adr-312.md) | A handle on a shared value is duplicated, never moved; it may go into a task of our own but not into code nothing describes, and at one user thread its count is plain. | Accepted | partly: D9's bridge type |
| [302](adr-302.md) | A call into C is written inside `unsafe`, and what crosses is a view for the call or an opaque handle that may be absent, never a raw pointer. | Accepted | yes |
| [281](adr-281.md) | Shared mutable state is `SharedMut[T]`: its shape is chosen per value, it opens through four doors, a lock inside a lock is refused, and what leaves a door is stamped `Seen[T]`. | Accepted | partly: D33 for a stored lambda |
| [297](adr-297.md) | A cleanup runs where ownership says the value dies, settled by the compiler and drained under a deadline, and a cleanup the deadline cuts off is exit 70. | Accepted | yes |
| [282](adr-282.md) | Text is one type, `String`, whose representation is decided per position by what flows in, and a copy is only a `.clone()` the program writes. | Accepted | yes |
| [284](adr-284.md) | A Nikaia library for other languages is one C artifact, safe at its boundary, whose entry points answer with a status into the caller's memory, and every other host is a file generated over it. | Accepted | no |
| [293](adr-293.md) | A map read is a `T?` and a key is not a position, a sequence says what it is as a whole, and what is handed over is gone, part by part. | Accepted | yes |
| [283](adr-283.md) | A view that escapes is tethered: its buffer lives in the keep of whatever keeps the view, the compiler decides where with nothing written, and `Bytes` is the language's buffer. | Accepted | partly: the retention lint (D6), `Mapped`'s deref to `Bytes` |
| [291](adr-291.md) | A `let` takes names or a flat tuple, `_` ignores a value, and a `match` takes nested patterns, ends in `else`, covers every case and lends what an arm only reads; `..` is inclusive and `..<` exclusive. | Accepted | yes |
| [279](adr-279.md) | `??` takes one value as its fallback, has no postfix form, and lends its left side where the answer is read and takes it where it is kept; after a type that is not `T?` it is a warning (`NK1216`), after a list's index refused (`NK1211`); `?.` alike (D13). | Accepted | partly: D12 (#342), D13 not built (#476) |
| [246](adr-246.md) | A type that holds itself holds itself through a box the compiler writes. | Accepted | partly: `Option<Box<T>>` for a nullable field |
| [247](adr-247.md) | An output test runs in a directory of its own with `NAME.in/` and `NAME.out/`, its expectations are written by `nikaia test --bless`, and a `test` block gets a scratch root. | Accepted | yes |
| [235](adr-235.md) | `par_iter()` walks a list on every core, and its lambda may neither pause nor write what is shared. | Accepted | yes |
| [234](adr-234.md) | A lazy walk of `io::lines()` carries the read failure through to the eager walk or the `for`. | Accepted | yes |
| [233](adr-233.md) | A lambda that pauses is handed to `std`'s pausing counterpart of the higher-order entry it is passed to. | Accepted | yes |
| [231](adr-231.md) | A walk over views of numbers, truth values or characters yields the values. | Accepted | yes |
| [230](adr-230.md) | A function field is reached through what is stored in it, so a call through it inside a door is checked as a direct call is. | Accepted | yes |
| [229](adr-229.md) | A value handed over inside an `f"…"` hole is handed over there, and the hole does not make the text around it a position of its own. | Accepted | partly: a template's holes |
| [218](adr-218.md) | Every `unsafe` topic is a small crate of its own under `crates/unsafe/`, with an argument for each `unsafe` and Miri under both aliasing models. | Accepted | yes |
| [217](adr-217.md) | `T::variants` under `[T: Enum]`, a kept function value, a view kept by a struct literal and `fs::read` returning `Bytes` are lowered as the specification states them. | Accepted | yes |
| [204](adr-204.md) | Whether a type compares is decided from its parts, a float makes `Eq` fail, and a type that does not compare is refused with `NK1188`. | Accepted | yes |
| [203](adr-203.md) | An `enum`'s cases are a `variants` column of the ledger, so a `match` over another package's `enum` is checked for completeness. | Accepted | yes |
| [202](adr-202.md) | A view of the subject is handed back without a written `ref`, and the ledger's `borrows` column names the receiver. | Accepted | yes |
| [005](adr-005.md) | Ownership has no lifetime annotations: four groups of borrow situation say which the compiler solves, and borrow contracts are inferred whole-program into the ledger. | Accepted | partly: Group B.2's desugaring (D2), D8's check on a second OS |
| [094](adr-094.md) | The caller writes no `&`: whether a parameter is lent or kept is inferred into the ledger's `keeps` column, and mutation is `mut` in the declaration, never at the call. | Accepted | yes |
| [108](adr-108.md) | Every `std` function that takes a path takes its root right after it, an `fs::Root` checked component by component or `Anywhere`, which `nikaia --trust` lists. | Accepted | yes |
| [118](adr-118.md) | `with` copies a value with named fields changed, moving the other fields and never copying unseen. | Accepted | yes |
| [119](adr-119.md) | A target without an operating system pins `user_parallelism` to `no`, emits `no_std` with abort, makes interrupts wakers, and bakes runtime settings in at build. | Accepted | no |
| [121](adr-121.md) | The ring's park listens on an always-armed eventfd, so every worker reply can wake the executor. | Accepted | yes |
| [123](adr-123.md) | `crosses = false` says a type may not cross a thread, written by the describer from a foreign type's fields, and an absent `crosses` means nothing is recorded. | Accepted | yes |
| [132](adr-132.md) | `else if` is an `else` whose block is one `if`, written without the braces, and is no keyword. | Accepted | yes |
| [133](adr-133.md) | An argument list of options alone writes no `;`, and a leading `;` is refused. | Accepted | yes |
| [135](adr-135.md) | A list literal is `[1, 2, 3]`, the empty one takes its type from its first use or is refused, and a `[` at the start of a line begins a literal. | Accepted | yes |
| [317](adr-317.md) | A call's `(` is on the line of what it calls: a `(` at the start of a line begins what is written there (a tuple, a parenthesised expression), the rule ADR-135 D3 made for `[`. | Accepted | yes |
| [140](adr-140.md) | Each of five constructs has one spelling: the brace struct literal, the anonymous constructor, `::` for a path, `throws` and `sync` after the result type, and a `use` that brings nothing in. | Accepted | yes |
| [141](adr-141.md) | Six spellings the specification used are corrected to ones that exist, and `5.seconds()`, `channel::bounded` and `select` are each decided by a record of their own. | Accepted | yes |
| [149](adr-149.md) | A channel is `std`'s and only bounded: `send` pauses when it is full, and `recv` gives `null` when every sender is gone. | Accepted | yes |
| [150](adr-150.md) | A duration is `std::time::Duration`, written `5.seconds()`, with no suffix literal. | Accepted | yes |
| [152](adr-152.md) | A fixed-size array is `Array[T, N]`, with `N` a `comptime` integer. | Accepted | partly: [284](adr-284.md)'s C field |
| [166](adr-166.md) | Cargo's target-info probe, `rustc -` with a `--print`, is given an empty standard input. | Accepted | yes |
| [171](adr-171.md) | A refusal from the lowering names its line, with a caret, as every `NK…` diagnostic does. | Accepted | yes |
| [172](adr-172.md) | A `for` may iterate a sequence whose step pauses; `Seq[T] pauses` is a third state beside `sync` and nothing said, and the step is awaited. | Accepted | yes |
| [177](adr-177.md) | A grammar that runs while the program is built is run by compiling the parser it generates, not by interpreting the grammar. | Accepted | yes |
| [260](adr-260.md) | A file belongs to its project, and the single-file commands are the verbs `lower`, `interpret`, `explain` and `run f.nika`. | Accepted | yes |
| [263](adr-263.md) | A file operation with nothing else in flight runs on the calling thread once a measurement shows it pays, and the normal path does no work it does not need. | Accepted | yes |
| [188](adr-188.md) | The escape set is the language below's, written down once in Part I 2.5, and an escape outside it is refused with `NK1184`. | Accepted | yes |
| [186](adr-186.md) | A parse keeps its input exactly when its declared result may hold a view into it. | Accepted | yes |
| [185](adr-185.md) | A `let` declaring a struct type over a lent binding is refused with `NK1183`, and a grammar entry in tail position binds its value before the `Ok`. | Accepted | yes |
| [184](adr-184.md) | A view is spelled `ref X` with `ref` reserved, text has one noun, `String`, and `Array[T]` is an array of any length whose position says where the length comes from. | Accepted | yes |
| [182](adr-182.md) | A `for` lends, and a cast, a range index and a typed `let` downstream of the binding read through the view. | Accepted | yes |
| [179](adr-179.md) | `ref Array[T]` is a type of the language and what a run crosses as from build time, and a field filled from a run the body owns is `NK1179`. | Accepted | yes |
| [178](adr-178.md) | Compiler-side staging is not part of the language, and a staging decision enters the compiler only with a measured crossover. | Accepted | yes |
| [080](adr-080.md) | An `impl` owes its `trait` (`NK1130`, `NK1129`), and a write through the brackets is `index::set`, not an index. | Accepted | yes |
| [081](adr-081.md) | A `+` over text is a call to `std::concat`, and an expression has a span to key it by. | Accepted | yes |
| [083](adr-083.md) | A field of a borrowed subject may not be handed out by value (`NK1131`). | Accepted | yes |
| [096](adr-096.md) | A type nothing declares is refused with `NK1135`, against a known set derived from what is declared. | Accepted | yes |
| [056](adr-056.md) | Every name the emitter substitutes is put back in a backend message, and a message that then says the same thing on both sides is an internal error. | Accepted | yes |
| [055](adr-055.md) | The emitted Rust is `async` where a function can pause and a plain `fn` where `sync` says it cannot; the executor is ours, and `user_parallelism` sets only how many threads it has. | Accepted | partly: D6's `Send` refusal is still `rustc`'s |
| [042](adr-042.md) | A view keeps the type it views, and a container whose ledger records a `deref` is seen through once, after a comparison fails. | Accepted | yes |
| [253](adr-253.md) | A list has no `+`, and `extend` is how two lists join. | Accepted | yes |

### Grammar, DSLs, parsing

| ADR | Decides | Status | Built |
| :--- | :--- | :--- | :--- |
| [296](adr-296.md) | A grammar is part of the language: scannerless, entered by a call to any `entry rule`, cut in parallel only where a checked frame says so, lowered name for name, and its actions neither pause nor fail without a name. | Accepted | partly: D5's `args.values()`, D14's prefix hash, D15's skipped teardown, D25's `pub grammar` |
| [016](adr-016.md) | The UTF-8 check is divided across frames, never skipped, and each piece is checked with SIMD (`simdutf8`), answering as the standard library does. | Accepted | yes |

### Diagnostics

| ADR | Decides | Status | Built |
| :--- | :--- | :--- | :--- |
| [300](adr-300.md) | A diagnostic names the file the user wrote: at build time through a source map, at run time through a location table the panic hook reads. | Accepted | yes |
| [015](adr-015.md) | The backend's diagnostics are built lazily. | Accepted | yes |

### Trust and provenance

| ADR | Decides | Status | Built |
| :--- | :--- | :--- | :--- |
| [010](adr-010.md) | Whether input is trusted is a property of its source that the compiler tracks, and what it chooses with it is the hasher. | Accepted | partly: D3's `trusted:` argument, D4's `@untrusted` grammar, D5's refusal without entropy |
| [316](adr-316.md) | A map's hasher is chosen by its keys' trust first and their shape second (a fixed-width type, text of a known length, text of unknown length), never weakening trust. The choice shows only in `--trust` and the ledger, a program may name any hasher with `hasher:` at construction, and a shape gets its own function only once a real program of that shape is measured better. Built: text hashed by `rustc-hash` 2's byte hash, numbers by Fx. | Accepted | partly: the shapes the compiler has to classify, `--trust`'s lines, `hasher:` |
| [017](adr-017.md) | Escaping is the template's contract, not the caller's discipline. | Accepted | yes |

### `std` and the language surface

| ADR | Decides | Status | Built |
| :--- | :--- | :--- | :--- |
| [319](adr-319.md) | A file name is `fs::Path`, the platform's own bytes (Unix raw, Windows WTF-8, UTF-8 where names are Unicode): text stands wherever one is asked, an `f"…"` builds one where its use asks, `to_text()` fails only for a name the operating system handed over, `display()` shows `�` and is not tracked, and a `String` is always valid UTF-8. | Accepted | no |
| [320](adr-320.md) | Text is UTF-8 and is read through a view that names its unit - `s.bytes`, `s.scalars`, `s.graphemes`; there is no `char` (`scalar` and `u8`), no `s.len()` and no `s[i]`; a position the text hands out cuts it; `find` from a position; `'…'` is a `u8` or a `scalar` by use; graphemes are `std`'s; `==` compares bytes. | Accepted | no (#453) |
| [318](adr-318.md) | An option's default is a build-time value, evaluated once where the function is declared, recorded in the ledger as its value and written into the call in the form a `comptime` crosses in; a `std` type the compiler cannot see into crosses through its `constant` column, and a `std` function runs at build time when it is written in Nikaia. | Accepted | no |
| [311](adr-311.md) | A build-time value crosses into the program in its view form, a map as a fixed table, and the lookup is the compiler's. | Accepted | yes |
| [310](adr-310.md) | A build reads a file only when the code (`asset("…")`), a committed list and the invocation all name it, and the path stays in the project. | Accepted | yes |
| [309](adr-309.md) | Only an `f"…"` has holes, the grammar parses them as code, and every analysis sees them. | Accepted | yes |
| [304](adr-304.md) | Reflection is data: `T::fields` is a list a bound makes exist, a loop over it is unrolled once per type used, and there is no macro system. | Accepted | yes |
| [313](adr-313.md) | The prelude is a short closed list, everything else is written with its module, and each wrong spelling is refused. | Accepted | yes |
| [307](adr-307.md) | A comment is `//` or a nesting `/* … */`, and a run of `///` before an item is its documentation, kept by the parser and not carried by the contract. | Accepted | partly: a field's and a variant's `///` |
| [301](adr-301.md) | A condition is an ordinary expression: the head of an `if`, `while` or `for` parses the body's language minus the forms a `{` begins, which take parentheses. | Accepted | yes |
| [298](adr-298.md) | A word the language uses is reserved and only for a construct, and a word the language below reserves is escaped where it is written. | Accepted | yes |
| [299](adr-299.md) | A database statement is SQL a driver's grammar checks at build time against a schema, run on a connection typed by that schema; a block may name the struct its rows are (`-> T`), matched by name, strictly, a `NULL` only into a `T?`. A grammar from a package is named `package::Grammar`, always (D20). | Accepted | no |
| [292](adr-292.md) | Statement order is the written order; a program asks for concurrency with `overlap`, which keeps every result and failure, or `select`, which keeps the first. | Accepted | yes |
| [285](adr-285.md) | An integer is one of five types and aborts where a value does not fit, and a number without an annotation is typed by its uses; one integer type takes another without `as` where no value can be lost (D32). | Accepted | partly: `u8`'s arithmetic names; D32 not built (#439) |
| [315](adr-315.md) | Arithmetic and a conversion that may not fit can be asked whether they do: `checked_add`, `_sub`, `_mul`, `_div`, `_rem`, `_neg`, `_abs` and `checked_i32()` … `checked_u64()` answer `T?`, `null` where there is no answer (a division by zero included); no `checked_shl`/`_shr`. | Accepted | yes |
| [277](adr-277.md) | A lambda is `fn(name) { … }` and takes the arguments it names, and a parameter that is code has the type `fn(A) -> R`, lowered by that type and by whether the body keeps it. | Accepted | yes |
| [329](adr-329.md) | A reflected field also answers `.ty`, its type as a `meta::Type` (a struct as a `Record` of its fields), and `.doc`, its `///` text: an API description is built from what the program says; a route's facts are its options; annotations are #496's. | Accepted | no |
| [330](adr-330.md) | A type is passed where a function's parameter list holds one (`schema(CreateUser)`); it answers `::kind`, a `meta::Type` whose parts are types (`List(A)`, `Struct(T)`, `Opaque(T)`), `::access` (`Owned`, `Ref`, `Shared`, `SharedMut`), `::doc`, `::name`; a `match` on a kind checks each arm under the kind it names; `.ty` is the field's type; a grammar names no type of the program (`Named` gone); `f[T]()` is allowed with the warning `NK1233`, asking for its use case. | Accepted | no |
| [328](adr-328.md) | Supervision: `supervisor::run` runs until it gives up; children in dependency order; `OneForOne`, `OneForAll`, `RestForOne`; restarts by an exchangeable `Restart` policy with a fixed-size history; a child keeps no changeable state across a restart; `join` on a crashed task throws `task::Crashed`. | Accepted | no |
| [327](adr-327.md) | A value a crashed task left is never read: a door to it panics, and a `SharedMut::supervised` value is rebuilt. Direction: Nikaia allocates its own memory, every task and shared value from a block that is its budget, sized by a strategy; an owned `Vec` toward Rust is a copy. | Accepted | no |
| [326](adr-326.md) | `net::serve` accepts connections, a task each, below every protocol; no connection count is configured, an exchangeable `net::Admission` strategy decides beside the hot path; a panic ends only its task, at both settings. Timers: `head_timeout` 10 s, `body_bytes_per_second` 500, keep-alive with `idle_timeout` 75 s, closed early only after a response (D6-D9). | Accepted | no |
| [325](adr-325.md) | The tools below Nikaia read no configuration from the project: Cargo and `rustc` start in the compiler's own directory outside it, with a configuration the compiler writes from `nikaia.toml`; the user's own still applies. | Accepted | no |
| [324](adr-324.md) | A package links a C library by name: `extern(library: "x")` in the code, `[library.x] pkg-config = "…"` in the manifest, found only through `pkg-config` with listed linker flags; `extern` is written without `"C"`; there is no build script, and a package brings no C sources yet. | Accepted | no |
| [323](adr-323.md) | A grammar describes a type as data, `std::meta::Type` (scalars, `Maybe`, `List`, `Record`; `Named` gone by ADR-330 D7), passed to `meta::column` and `meta::parameter`; the compiler writes the block's row and parameter types from it, and a message about such a type names the block that declared it. | Accepted | no |
| [322](adr-322.md) | A decimal whole number may end in a scale, `K M G T P` decimal and `Ki Mi Gi Ti Pi` binary, which is a spelling of its value; an exponent is written with a lower-case `e`. | Accepted | no |
| [321](adr-321.md) | Build-time code is compiled and run, not interpreted: against the package's dependencies through one generated dynamic library, one crate per package compiled by `rustc` directly, results cached, run beside the rest of the build, and bounded by a step budget counted in loop turns and calls (10 billion) and a memory bound counted by its allocator (4 GiB), each raised at the `comptime` that needs it; a dependency's function runs only on ledger entries this build derived from its sources, and no Rust code but `std`'s runs then. A grammar calls its package's Nikaia; the build-time program holds no foreign code, not even linked, and nothing lets it run (D13-D15). | Accepted | no |
| [287](adr-287.md) | A `comptime` binding is evaluated while the program is built, where an item or a statement stands, by a body that is `sync` and touches at most the build's parameters; at item level in any order, in a body in written order. The word marks the name and is never an expression; an integer `comptime` is an open number each use takes in its own type. | Accepted | partly: the build's parameters in the cache key; D19's lone `NK1117` (#380) |
| [286](adr-286.md) | A package is a directory and a crate, `use` names one and brings no name in, and a dependency is found by path or by version through Cargo. | Accepted | partly: a dependency by version |
| [289](adr-289.md) | An HTTP server is the application's own: `std` lends the socket, the `http` package speaks the protocol, and a handler is a lambda over the request. | Accepted | partly: bare `String`/`html::Raw`/`Bytes` results, `.route(…)`, file bodies, `--trust`'s wide-bind listing |
| [295](adr-295.md) | A generic is checked at both ends: a type parameter is a type in its body and a variable at the call, a bound names a declared trait, and the ledger carries the trait, each `impl` and the bound. | Accepted | partly: D14's call to the implementing type's own entry |
| [278](adr-278.md) | `T?` is a type the compiler wraps into, and `?.` reaches a member through a view of its receiver. | Accepted | partly: D13 as changed by ADR-279 D13 not built (#476) |
| [276](adr-276.md) | `while true` is the unconditional loop, `break` and `continue` leave the innermost loop and carry nothing, and a jump is a never-typed expression that may carry its condition. | Accepted | yes |
| [013](adr-013.md) | Stage 0 infers only what it states it infers, `std` is a crate, and methods resolve through the receiver. | Accepted | yes |
| [014](adr-014.md) | `std` is written in Nikaia where it can be, `fs::map` maps, and `par_fold` runs on rayon. | Accepted | yes |
| [019](adr-019.md) | Standard input is a stream, read like everything else. | Accepted | yes |

### The contract ledger and the type checker

| ADR | Decides | Status | Built |
| :--- | :--- | :--- | :--- |
| [288](adr-288.md) | `sync` means a function never pauses: it is inferred from the body, decided by a lambda where one runs, and promised across packages only where written. | Accepted | yes |
| [020](adr-020.md) | The ledger records what the compiler knows, and a library brings its own. | Accepted | yes |
| [100](adr-100.md) | A consumer reads a dependency's ledger and derives it only where its source hashes do not match, and the inference graph is the package. | Accepted | yes |
| [251](adr-251.md) | The ledger is the contract, what it was derived from lives beside it in `nikaia.derived`, and it is written in Nikaia's spelling. | Accepted | yes |
| [105](adr-105.md) | The ledger says `Seq[T]` for what is produced step by step and `Par[T]` where the steps run at once, and a `Seq` is walked once. | Accepted | yes |
| [024](adr-024.md) | The type checker says only what is written down, and `?` is the absence of a claim. | Accepted | yes |

### Errors

| ADR | Decides | Status | Built |
| :--- | :--- | :--- | :--- |
| [308](adr-308.md) | An unread error is bound as `_error`, a `catch` over what cannot fail is refused (`NK1134`), and an error that newly reaches a handler is named once (`NK2402`). | Accepted | yes |
| [280](adr-280.md) | A failure travels as its error type in a one-word envelope; the site is free, a trace is asked for, and the failures that joined it are shown, not read. | Accepted | yes |
| [023](adr-023.md) | `throws` carries no type list and the error set is inferred into the ledger; `throw` exists, an error knows its site, and there is no postfix `?`. | Accepted | yes |
| [025](adr-025.md) | A loop's step can fail, and the enclosing function gains a `throws` for it. | Accepted | yes |

### The runtime

| ADR | Decides | Status | Built |
| :--- | :--- | :--- | :--- |
| [303](adr-303.md) | The runtime is Nikaia's own: it starts before `main`, is tuned when the program starts, completes files and registers every readiness wait; the HTTP server is ours and TLS is `rustls`. | Accepted | partly: `rustls` (D2), HTTP/2 (D6), D7's rule for a cleanup owned by a foreign task |

### Build switches

| ADR | Decides | Status | Built |
| :--- | :--- | :--- | :--- |
| [037](adr-037.md) | Two switches: `target` names the machine and `user_parallelism` says whether the user's code may run concurrently; the compiler's own threads are not the user's. | Accepted | partly: the `wasm32-unknown` target |

## What supersedes what

A decision that changes another one is written into the record that holds the
changed rule, with its *Changed by* note. When the change comes from a second
live record that decides more than this, the pair is listed here, so a reader of
either finds the other. Records replaced as a whole are not here: they are in
[`docs/history/adr/`](../../history/adr/).

| Changed | By | What holds now |
| :--- | :--- | :--- |
| [329](adr-329.md) D1 | [330](adr-330.md) D5 | `.ty` is the field's type, not a `meta::Type` |
| [323](adr-323.md) D1 | [330](adr-330.md) D3 | a `meta::Type`'s parts are types; it exists only at build time |
| [323](adr-323.md) D1, D3 | [330](adr-330.md) D7 | there is no `Named`; a grammar names no type of the program |
| [323](adr-323.md) §3 | [330](adr-330.md) D9 | type arguments at a call, `f[T]()`, are allowed, with a warning |
| [304](adr-304.md) D6 | [330](adr-330.md) D1 | a shape walk's type may come from the call |
| [100](adr-100.md) D3 | [251](adr-251.md) D1 | the source hashes live in `nikaia.derived` beside the ledger, not in its header; the ledger is believed while they match |
| [094](adr-094.md) D2 | [184](adr-184.md) D1 | the assertion that a parameter is a view is spelled `ref T` |
| [002](adr-002.md) D2 | [177](adr-177.md) D1 | a grammar that runs while the program is built is compiled from the parser it generates; other `comptime` code is evaluated in the compiler's process |
| [002](adr-002.md) D3 | [021](adr-021.md) D9 | Cranelift is an option for development builds, not the default |
| [203](adr-203.md) D2 | [204](adr-204.md) D1 | a unit-only `enum` compares by the structural walk that decides every type, not by a rule of its own |
| [083](adr-083.md) D2 | [202](adr-202.md) D1 | a field handed back where the result is declared a view is lent, not refused; `NK1131` stands where a field is bound or passed |
| [013](adr-013.md) D4 | [140](adr-140.md) D2 | a type is built by its anonymous constructor, not `new` |
| [013](adr-013.md) D5 | [277](adr-277.md) D2 | a lambda takes the arguments it names |
| [014](adr-014.md) D1 | [002](adr-002.md) D4 | `std`'s Nikaia half is lowered at release time and the `.rs` is committed |

## Reserved numbers

A number is **taken when it is claimed here**, not when the record lands, so two
branches writing records at the same time do not take the same number.

| ADR | Claimed for | Where |
| :--- | :--- | :--- |

A row here is a claim and nothing else: it says the number is spoken for, not
what the decision is. Delete the row in the same commit that adds the record.
A claim whose branch is abandoned is deleted by whoever notices.

**When a record has to move to another number, its citations move with it.**
`check-adr-refs.py` catches a citation of a decision that does not exist and a
number held by two records, but not a citation that still resolves and now
points at the record that took the old number. Every sentence that cited the old
number is checked by hand.

## Writing a new one

Take the next free number (free means neither a file above, nor an archived one
in [`docs/history/adr/`](../../history/adr/), nor a row under **Reserved
numbers**) and claim it there first. Copy [`TEMPLATE.md`](TEMPLATE.md); its
header block is, in this order:

```markdown
# ADR-NNN: A title that states the decision, not the topic

**Status:** Accepted | Open
**Date:** <day the decision was taken>
**Answers:** <the question, with the issue that asked it>
**Supersedes / Related:** <records this changes or leans on, one line each>
```

There is no `Built:` line and no `Target Version:`; whether a decision is built
is the **Built** column above. Then: the question, how others answer it, the
options weighed, the decisions numbered `D1…Dn` so they can be cited, and the
consequences. A decision nobody can cite by number is a decision that gets
re-litigated. If the new record changes a decision in another live record,
update that record in place with its *Changed by* note, and add the pair to
[What supersedes what](#what-supersedes-what) when the change is partial.
