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

### How a `pub` function writes that it pauses only when its lambda does

**What is blocked.** [ADR-244](specification/adr/adr-244.md) §3, and with it the
rest of that record: under its D1, a consumer reads a `pub` function as *may
pause* unless the source writes `sync`. A higher-order function that only calls
the lambda it is handed is `sync = "from(f)"` in `std`'s hand-written ledger
([ADR-029](specification/adr/adr-029.md) D3), but a `.nika` source has no way to
say it, so every such `pub` function in a package would read as pausing.

**Why it is the owner's.** It is syntax on a signature, and the language has
been sparing with words there ([ADR-084](specification/adr/adr-084.md)).

**Options.**

1. **`sync(f)` where `sync` stands today**: `pub fn apply(f: fn(i64) -> i64, x:
   i64) -> i64 sync(f)`. Reads as "sync, given `f` is"; it is the ledger's
   `"from(f)"` in the source's own place for the word. Several lambdas:
   `sync(f, g)`.
2. **No syntax; publish `"from(f)"` when inferred.** A body whose only pausing
   calls are to its lambda parameter is `"from(f)"`, and that crosses the
   boundary. Cheapest, but it is exactly what ADR-244 D1 says not to do: an
   inference published as a promise, withdrawn by the next pausing line.
3. **Leave it unwritable.** Such functions read as pausing across the boundary,
   which is safe; a caller in a `par_iter` or a lock cannot use them.

**Recommendation: 1.** It is the only option that keeps D1's rule — a promise is
what the source wrote — and it costs no new word, only a parameter list on one
that exists. **If it is wrong,** the cost is one form to accept and ignore;
option 3 is what happens until it is decided.

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
