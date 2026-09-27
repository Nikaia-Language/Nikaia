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

### Does a consumer rely on an inferred `sync`?

**What is blocked.** [ADR-244](specification/adr/adr-244.md) D1 and D3; D2 and
D4 of the same record are accepted and do not wait on it.

**The question.** A `pub` function whose body never pauses is `sync =
"inferred"` in its ledger. A consumer in another package reads that as a
promise today, and a later line in the body can withdraw it; the consumer finds
out at upgrade, refused at its own line (ADR-244 §1 measures where).

**Options.**

1. **D1 as proposed:** across a package boundary `"inferred"` reads as *may
   pause*; only `sync` and `sync(f)` are promises. Safe and simple to explain;
   costs a caller in another package the use of every function its author has
   not marked, until the author follows D2's note.
2. **Keep reading `"inferred"` as a promise**, and rely on D2's note to get
   authors to write the word. Nothing changes for consumers; the withdrawn
   promise stays possible for every function whose author ignored the note.
3. **Keep reading it as a promise, and tell the author when a `pub` entry loses
   it** — at the change, in the author's build, naming the line that made it
   pause. Moves *who learns it* to the author without narrowing what consumers
   may use; the withdrawal is still published if the author commits it.

**Recommendation: 1**, for the reason ADR-244 D1 gives: a promise is what the
source wrote. **If it is wrong,** the cost is some `sync` words written in
libraries that would have been inferred anyway; option 3 can be added on top of
either answer.

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
