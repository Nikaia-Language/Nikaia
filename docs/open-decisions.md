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

### How a type holds itself

**What is blocked.** Every recursive data structure, and with it **writing
Nikaia's own parser in Nikaia** — the compiler's parser is already written in
the grammar language Nikaia's `grammar` blocks lower to (275 rules in
`crates/nikaia/src/parser/mod.rs`), but its actions build a syntax tree, and
`Expr` holds `Expr`. `enum Expr { Add(Expr, Expr) }` and
`struct Node { next: Node? }` have no size, and nothing in the language says
how a type holds another of itself. Measured at 0.0.234: `Vec[Expr]` in place
of `Expr` works and is what `NK1192` now names; `Shared[Expr]` does not (the
field and the value disagree on the count, `open-work.md` §1.24); a written
`Box[T]` does not exist — Part I 4.6's `Box[T]` is an example struct, not an
indirection.

**Why it is the owner's.** It decides whether a program ever sees the heap
indirection a tree needs, and what it is called if it does.

**The options.**

1. **The compiler puts the indirection in.** A field whose type holds itself
   inline — directly, nullable, through a tuple or another type — lowers to a
   `Box` below, and a construction and a `match` over it are written as if it
   were not there. `enum Expr { Add(Expr, Expr) }` is a program as written.
   `NK1192` is the analysis that finds those fields, already built.
2. **A written indirection type in `std`**, as Rust has: `Add(Heap[Expr],
   Heap[Expr])`, built with `Heap(e)` and read through. Explicit, and it needs
   a name (`Box` is Part I 4.6's example's).
3. **No new form**: a type holds itself through a list or a `Shared`, and
   `NK1192` stays a refusal with that help.

**What this file recommends: option 1.** It is the rule the language already
follows for references — *the `&` is the compiler's to write*
([ADR-094](specification/adr/adr-094.md) D4) — applied to the one other thing
Rust makes a program spell for the machine's sake. A tree reads as the tree,
which is what a parser's actions are made of.

**What each costs if it is wrong.**

* **Option 1**: an allocation per recursive field that the source does not
  show. Part II's cost rule wants what runs to be visible; the answer is that
  the allocation is where the recursion is, which the declaration shows. If it
  proves wrong, option 2 can be added beside it without breaking a program.
* **Option 2**: every recursive type, and every construction and pattern over
  one, carries a word that says nothing about the program's meaning.
* **Option 3**: a syntax tree whose children are `xs[0]` and `xs[1]`, checked
  by nobody — the self-hosted compiler would be written in a workaround.

### Whether `+` joins two lists

**What is blocked.** Nothing hard: `a + b` on two lists is refused
(`NK1191`, 0.0.234) and `a.extend(b)` is the way that works. Before that it
reached `rustc`.

**Why it is the owner's.** It is an operator on a type Part I 4.5 gives none.

**The options.** (1) `+` on two lists of one element type makes a new list,
as `+` on text makes new text (ADR-081); (2) no operator on a list, and the
refusal stays.

**What this file recommends: (2) for now.** `+` on text is there because text
is built up by hand constantly; a list is built with `push` and `extend`, and
`+` would allocate a third list where a program usually wants to grow one.
**Costs:** (1) wrong is a quietly quadratic loop `xs = xs + [x]`; (2) wrong is
a refusal lifted later, which breaks nothing.

### Where the connection goes when a checked statement runs

**What is blocked.** [ADR-143](specification/adr/adr-143.md), all of it
([`open-work.md`](open-work.md) §2.40, *the database driver checks the SQL
while the program is built*). Its D2 says a statement's value *is a prepared
statement whose `execute` takes the parameters by name and returns
`Seq[Row]`*, and its D3 example writes `by_age.execute(min_age: 18)` — with no
connection anywhere. Part II 10.5's example does the same. The record's step 1,
the row type, cannot be run without `execute`, and `execute` cannot be written
until the connection has a place, so steps 1 and 3 (`std::db`'s protocol) are
one piece of work and wait on the same answer.

**Why it is the owner's.** It is the line every program that talks to a
database writes, and it decides what a driver package is: whether the compiler
or each driver turns a row of values into the typed row, and whether `std::db`
holds a trait a program implements.

**The options.**

1. **The connection is the subject.**
   `let rows = by_age.execute(db; min_age: 18)` — the statement is the
   receiver, the connection stands before the `;` and the parameters after it,
   as Part I 5.1's *Subject `;` Config* reads anywhere else. `db` is any value
   whose type implements a `std` trait, `db::Connection`, with one method: run
   this text with these named `db::Value`s, hand back rows of `db::Value`. The
   **compiler** writes the conversion into the row type, so a driver never
   names a per-statement type and the row type stays *not a struct the program
   can declare a second time* (D2).
2. **The connection's method, as ADR-007 D5 has it.**
   `let rows = db.execute(by_age; min_age: 18)` — the driver writes
   `execute(ref self; ...args: Self::dsl)` by hand, as
   `tests/fixtures/sql_statement.nika` does today. It needs a **new spelling**
   for the per-statement row type in the driver's signature (`Self::row`, or
   similar), and every driver converts values to rows itself.
3. **Bound at the block.** `dsl sqlite(schema: app, on: db) { … } eod`, then
   `by_age.execute(min_age: 18)`, which is the example as written. The
   connection becomes an argument of the block beside `schema`.

**What this file recommends: option 1.** It keeps the example's
`by_age.execute(…)` shape and adds only the subject; it keeps the conversion,
which is the part that has to agree with the derived row type exactly, in the
one place that derived it; and `std::db` becomes what D4 says it is — the
traits two drivers agree on, and nothing else.

**What each costs if it is wrong.**

* **Option 1**: a trait in `std` that programs implement, which `std` has
  none of yet (0.0.231 made an `impl` of an undeclared trait a refusal, so the
  trait has to be declared where the checker reads it). If drivers later need
  control over conversion — a database type with no `db::Value` — the trait
  grows a method, which is an addition rather than a break.
* **Option 2**: a spelling in the language (`Self::row`) that exists for one
  use, and a conversion written once per driver that can disagree with the row
  type the compiler derived — the disagreement lands at runtime, which is what
  D6 promises does not happen.
* **Option 3**: D3 says a block's arguments are **build-time** values resolved
  from `comptime`; a connection is a runtime value. Taking it would give the
  block two kinds of argument with one spelling, and a statement would be tied
  to one connection for its whole life, which a prepared statement reused
  across a pool is not.
