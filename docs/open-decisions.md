# Open decisions — the questions that need the owner

## The shape this page is for

The entries here are written to be *answerable* — what is blocked, the options, a
recommendation, and what either direction costs if it is wrong.

An answer is an [ADR](specification/adr/), and
the moment a question is answered its entry leaves this file rather than
staying with a note on it. What is merely **unbuilt** is in
[`open-work.md`](open-work.md). Each entry says what the
question is, why it is the owner's, and what this file recommends.

**What is here is what has been asked**, which is not the same as what is
unsettled: an entry is here because a piece of work was blocked by it and somebody
noticed, so an empty page means nothing is blocked that anyone has written down.
The way to refill it is the paragraph above — the moment a piece of work is
blocked by a question, the question comes here in that shape.

## Open

### How a schema is bound while the program is built

**What is blocked.** The examples of [ADR-143](specification/adr/adr-143.md)
D3 and Part III 17.1 bind the schema a statement is checked against with
`let app = comptime asset("schema.sql")` at the top of a file. That line is
not the language as the rest of the specification writes it: a `let` is a
statement, not an item, and a value computed while the program is built is
declared `comptime NAME: T = …` (Part II 7, [ADR-116](specification/adr/adr-116.md)).
[ADR-254](specification/adr/adr-254.md) D5 makes the schema's *name* decide
which connection a statement may run on, so the spelling now matters.

**Why it is the owner's.** Two accepted texts write the same line, and the
language's own rules refuse it; either the rule or the two texts change.

**The options.** (1) `comptime APP: db::Schema = asset("schema.sql")`, the
item form, and the examples change; (2) `let … = comptime …` becomes a form of
the language for a build-time value in function scope, and the examples move
it into a function.

**What this file recommends: (1).** It is the form the language has; (2) adds
a second spelling for one thing. **Costs:** (1) wrong is two examples written
again; (2) wrong is a spelling kept for one use.

### What `Path` is

**What is blocked.** Part I 7.1's `enum ConfigError { NotFound(Path), … }`
and every signature of Part III 17.1's `std::fs` write `Path`, and nothing
declares it: `std` publishes no `Path`, and the compiler refuses the example
(`NK1135`). [ADR-096](specification/adr/adr-096.md) §4 left it to the page.

**Why it is the owner's.** It is a type in `std`'s surface, or it is not.

**The options.** (1) `std::fs::Path` is a type, a checked path; (2) a path is a
`String`, and the pages write `String`.

**What this file recommends: (2) for now.** `fs::map(path, root)` already
takes the text and a `Root` that bounds it; a `Path` type is worth its name
the day it checks something a `String` cannot. **Costs:** (1) wrong is a type
nobody needs; (2) wrong is every signature written again later.

### Whether `fs::exists` takes a root

**What is blocked.** Part I 7.1 writes `if !fs::exists(path) { … }`, one
argument. Part III 17.1 lists `pub fn exists(path: Path, root: Root) -> bool
throws`, two. `std` has no `exists`, so the compiler cannot say which is
right: the call is undescribed and passes the check.

**Why it is the owner's.** Every other `fs` call takes a `Root`
([ADR-108](specification/adr/adr-108.md)); an exception is a decision.

**What this file recommends: the root, as Part III has it**, and Part I's
example writes it. **Costs:** small either way; this is one example.

### Which of two `Shared`s Part I 6.2 means

**What is blocked.** Part I 6.2 shows the three places a shared value is
made: `keep(Shared(connect(url)))`, `Pool { db: Shared(connect(url)) }` and
`fn connect(url: String) -> Shared[Connection] { return Shared(…) }`. With
`connect` returning a `Shared`, the first two wrap it twice, which the
compiler refuses (`NK1123`). Only the compiler says so; the page does not say
which line is the mistake.

**The options.** (1) `connect` returns a plain `Connection`, and the three
lines are three separate examples; (2) the first two lines drop the outer
`Shared(…)`.

**What this file recommends: (1).** The paragraph is about where a `Shared` is
written, and (2) removes two of the three places it shows.

### How a package's DSL is named after `dsl`

**What is blocked.** Part II 10.5 writes `use nikaia_sql` and then
`dsl mysql { … } eod`: the grammar is the package's, but the block names it
without the package. Part I 9.2 says a name from a package is reached through
the package's name, and `dsl nikaia_sql::mysql {` does not parse
(*Expected `from` or `{` here*). [ADR-143](specification/adr/adr-143.md) D1
writes `dsl sqlite` for a package called `sqlite`, where the two names happen
to coincide.

**Why it is the owner's.** It is the spelling every program with a DSL from a
package writes.

**The options.** (1) `dsl package::grammar`, as every other name from a package
is written; (2) a package's grammar is named bare, and the package's name is
its grammar's name where it has one; (3) `use` brings the grammar's name in,
the one exception to 9.2.

**What this file recommends: (1).** It is 9.2's rule without an exception.
**Costs:** (1) wrong is a longer line; (3) wrong is the exception 9.2 was
written to avoid.

### Whether Part III lists `html::Raw::new`

**What is blocked.** Part III 17.1's `std::html` listing names
`pub fn Raw::new(markup: String) -> Raw`, *the only constructor*. A program
writes `html::Raw(built)` (`examples/escaping`), and a written `Type::new` is
`NK1149` (Part I 4.2). Part I 4.2 also says *the ledger writes `Type::new`*,
so a listing in the ledger's words is not wrong by that rule alone.

**The options.** (1) the listing is in a program's words, `Raw(markup)`;
(2) it stays in the ledger's words and says so.

**What this file recommends: (1).** A reader copies what the page shows.

### What Part III 15.2's `image` example says about locking

**What is blocked.** The example's comment says *this is safe because the
'image' crate implements proper locking*. The paragraph above it says whether
a value may cross into foreign code *is decided from the Nikaia type of the
argument. No crate metadata is read.* Only the page's own text disagrees with
it; nothing compiles the comment.

**What this file recommends:** the comment says what 15.2 says — the call is
allowed because of the types it is handed, not because of the crate.

### Whether a call through a name nothing declares is refused here

**What is blocked.** `let t = nowhere::wobble(1)` passes the check and is
refused by `rustc` (*unresolved module*), which is Part III C.1's class — the
language below speaking about a file nobody wrote. `Foo::Baz`, a value through
the same head, is already `NK1181`. A fix refusing the call too was written
and taken back, because it contradicts Part III C.4 as the checker practises
it: *a call nothing describes says nothing here*, `rustc` still checks the
emitted crate, and ADR-005 D7 reports its refusal against the `.nika` line
(`typecheck.rs`, `a_callee_no_ledger_describes_is_not_guessed_at`).
`examples/fortunes.nika`'s `env::var(…)` relies on that silence today.

**Why it is the owner's.** C.1 and C.4 disagree about this one case, and each
is a rule the specification states.

**The options.** (1) Refuse it with `NK1181` wherever the checker knows every
package and crate the program may name — a project build — and stay silent in
a loose file; (2) keep C.4's silence, and make ADR-005 D7's translation of
`rustc`'s message the answer.

**What this file recommends: (1).** In a project the set of heads is known, so
refusing an unknown one is not a guess; `fortunes.nika` would write
`use std::env`.

