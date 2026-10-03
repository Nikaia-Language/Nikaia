# The language, reviewed — where the syntax is weak, where people will curse, where data is at risk

An honest read of the surface language as specified in Parts I–III and as
written in `examples/`, done in September 2026 against the compiler at
`61849da`. It is a notes page: nothing here is normative, and every claim below
was either taken from the specification's own text or reproduced with a probe
program handed to `nikaia --input <probe>.nika`. Where a probe was run, the
result is quoted.

The brief was: *be honest, and do not let the size of a change stop the
recommendation.* So the list is ordered by how much it matters, not by how cheap
it is, and the last section says what turning the language inside out would
actually look like.

What this review does **not** relitigate: the colourless-function bet, the
ledger, the scannerless grammar protocol, provenance, the diagnostics contract.
Those are the reasons the language exists, they are coherent, and each is
argued in a record. The problems below are almost all in the layer *around*
them — the part a person types.

---

## 1. Where the language contradicts its own promise

The README says the tax of a systems language is bookkeeping and the bookkeeping
is compiler work. Four places in the surface language still make the person do
it, and they are the ones a reader coming from Go, Python or TypeScript will hit
in their first hundred lines.

### 1.1 Ownership leaks through every call site

The specification promises *no lifetime annotations, ever*, and keeps it. What it
does not remove is the thing that makes lifetimes necessary in the first place:
the caller has to decide, at every call, whether a value is lent or given away.

Across the 913 non-comment lines of `examples/`:

| what the caller had to write | count |
| :--- | ---: |
| `&x` at a call or loop head | 42 |
| `.to_string()` on a literal or a view | 13 |
| `as i64` / `as f64` / `as i32` | 9 |

That is one ownership or width decision every fourteen lines, in programs that
the README presents as "straight-line code that says what should happen".

Three shapes carry most of it:

* **`&` at the call.** `fs::map(&path)`, `serve(&db)`, `into.merge(&stats)`,
  `for e in &entries`, `fn total(entries: &Vec[Entry])`. The callee's signature
  already says the parameter is a view; the caller repeats it. Nikaia has no
  overloading, so there is never a second function that the `&` would pick
  between: it is redundant at every call site, and the compiler can write it
  (the way it already auto-references a method's receiver).

* **A `for` consumes.** `for e in entries { … }` followed by `entries.len()` is
  accepted by the checker and refused by `rustc` with *use of moved value* — in
  Rust's words, about a file nobody wrote, which Part III C.1 calls a compiler
  bug. Reproduced with a nine-line probe. `examples/report/src/main.nika` even carries a
  comment explaining that `count` has to be read *before* `page(entries, total)`
  because rendering "consumes" the entries. A reader from any other language
  will write it the other way round, every time.

* **`String` versus `&str`.** `Response(status: 200, content_type:
  "text/plain".to_string(), …)`. `NK1106` refuses the literal and tells you to
  allocate. The language already has the machinery to not need this: Part I
  6.6's three-state view (borrowed, tethered, owned) is the compiler choosing
  the representation of text per use — and `Bytes` is already a tethered
  buffer. Text is the one type where that choice is still the programmer's.

**Recommendation (the biggest single change in this review):** finish the
"compiler decides" principle for ownership. *Decided since:*
[ADR-094](specification/adr/adr-094.md) takes points 1 and 2 below, records
how the `&` got there in the first place, and names the one semantic cost;
point 3 is [`open-decisions.md`](open-decisions.md)'s entry on one text type.

1. A parameter written `T` is a **view unless the body keeps it** — stored,
   returned, spawned — and which one it is goes into the ledger beside `sync`
   and `throws`. The caller never writes `&`; a call that hands over ownership
   is one where the ledger says the callee keeps the value, and the diff
   narration of 13.5 already handles a contract that changes. Keep `&` in the
   *type grammar* only where a struct stores a view (that is where the buffer
   is named, and ADR-283 D1 needs it).
2. `for x in xs` lends by default. Consuming iteration is spelled
   (`for x in xs.drain()` or `take xs`), because it is the rare case and it is
   the one that removes a name from scope.
3. One text type, with the borrowed/tethered/owned state chosen by the compiler
   the way 6.6 already chooses it for views. A literal stored in a `String`
   field is a tethered view of static text and costs nothing; `.to_owned()`
   stays for the case where the program wants a copy on purpose. That deletes
   every `.to_string()` above and the `String`/`&str` chapter of the type
   mapping in 15.2.

This is a whole-program inference the ledger was built to carry, and it is the
change that would make the examples read the way the README says they do.

### 1.2 Nobody but `std` can write the APIs the language advertises

The trailing lambda is the language's signature move — `.route("/") fn { … }`,
`counter.update fn(old) { … }`, `access_all(a, b) fn(x, y) { … }`. Part I 5.4 C
then says: *Nikaia's type grammar has no function type, so no `.nika` source
takes a lambda and passes it on.*

Probe: `fn twice(x: i64, f: fn(i64) -> i64)` — parse error at the `fn`. So a
user cannot write `route`, cannot write `retry(fn { … })`, cannot write a
`sort_by`. `examples/http/` is meant to be the package that grows into the
server, and it cannot take a handler. The panic hook, `task::scope`,
`supervisor::start_link` are all functions that take a lambda, and all of them
are things a library author outside `std` would want to write.

The reason given is effect polymorphism: a wrapper's `sync` would have to follow
its parameter's. But 13.5 already solved exactly that for `std` with
`sync = "from(f)"`, and it is a mechanism, not a `std` privilege.

*Decided since:* [ADR-277](specification/adr/adr-277.md).

**Recommendation:** add the function type `fn(A, B) -> R` to the type grammar,
record `from(f)` for any function that runs its parameter during the call, and
reserve `@detached` for one that stores or spawns it. The ledger already has
the column. The same question, measured against `examples/http/`, is
[`open-decisions.md`](open-decisions.md)'s entry on how a package receives a handler.

### 1.3 Traits reintroduce the colour the language removed

Claim #1 in the README: functions have no colour. Part I 4.7 status note and
`NK1129`: a trait's methods are `sync`, *"a word that says one may pause is not
in this language"*.

Probe:

```nika
trait Source { fn load(&self) -> String throws }
impl Source for Disk {
    fn load(&self) -> String throws { return fs::read_to_string(&self.path) }
}
```

```text
error[NK1129]: `Disk::load` can pause, and `Source` declares it as a method that cannot
```

A repository trait backed by a database, a `Handler` trait, a `Storage` trait —
none can be declared. The language that deleted the sync/async split of the
ecosystem has a sync-only trait system. The stated reason (the emitter has no
way to write an `async fn` in a trait) stopped being true with Rust 1.75, which
this toolchain is well past.

**Recommendation:** a trait method may pause unless the declaration says
`sync`. The implementation is checked against the declaration the way `NK2202`
already checks a body against its own `sync`. Emit `async fn` in the trait
where the declaration allows a pause. *Decided since:*
[ADR-288](specification/adr/adr-288.md), with Rust 1.75 as the floor of the
language below.

### 1.4 Two integer widths, and the default is the narrow one

`let count = 42` is an `i32`; `xs.len()` is an `i64`; `1brc.nika` writes `sum:
first as i64` and `self.sum += temp as i64` because a temperature read as an
`i32` meets a sum that is not. The widening rule of ADR-285 makes literals
flexible and then names pin them, so mixed arithmetic through a name needs the
cast every time. Rust made the same choice and Rust programmers complain about
it; Go, Swift, Kotlin and every scripting language default to 64 bits.

**Recommendation:** one integer type in the ordinary program, 64 bits wide
(call it `int` or make `i64` the default of an unconstrained literal). `i32`,
`u8` and the rest stay for layout — a struct that has to match a wire format or
a C header — and the cast is then written where a layout decision is being
made, which is where it belongs.

*Withdrawn after measuring.* The second sentence above is false: a name does
**not** pin a literal against a later use. `let mut total = 0` followed by
`total = total + text.len()` compiles, and the emitter itself writes
`text.len() as i64` with `total` becoming an `i64` from that use — Part I
2.4's rule, *the use is asked first and the size second*, holds. The three
casts in the corpus have other causes: `1brc.nika` **declares** the
temperature and `min`/`max` as `i32` (`rule TENTHS -> i32`, a 24-byte `Stats`),
so the cast stands where a layout decision meets a sum, which is where the
recommendation itself wanted it; `json.nika`'s `count() as i64` is a ledger
gap — `count` has no entry — which `Seq[T]` (ADR-105) closes. What remained
was one sentence of guidance, *"`i32`: used for most numbers"*, and Part I
2.2 now says `i64` for most numbers and `i32` where layout matters. No record
is needed and none is written.

---

## 2. Where data integrity is at risk

These are the places where a program can be *wrong* rather than refused. They
matter more than anything in §3.

### 2.1 `catch { value }` is a catch-all, and the set it catches is open

Part I 7.1 makes three decisions that are each defensible and together produce
a trap:

* nothing marks a call that can fail;
* `throws` names no types, and the set arriving at a `catch` grows whenever a
  callee gains a failure;
* `read_port() catch { 8080 }` handles every error with the same value.

Probe (`catchall.nika`) lowers to `Err(_error) => { 8080 }`: a permission
error, a disk that is full, a file that is a directory and a missing file all
become port 8080. When the callee three modules down gains a new failure, this
`catch` silently absorbs it too. The `NK2401` narration only fires where a
`match` over `error` exists; the one-line form, which is the form the
specification teaches first and every example uses, gets no narration because
it covers everything by construction.

Twenty-two `catch {` handlers in `examples/`, and every one of them is a
catch-all; none matches on `error`.

*Decided since:* [ADR-308](specification/adr/adr-308.md) — the syntax stays,
and a new error reaching any `catch` is named once in the build, with the
ledger commit as the acknowledgement. The recommendation below is kept for the
record and is not the decision.

**Recommendation:** a handler that does not look at `error` is a decision the
source should show. Either

* `catch { … }` without a `match` over `error` is a warning naming the error set
  it swallows (cheap, and the set is in the ledger), or
* a `catch` takes a pattern — `catch ConfigError::NotFound { Config::default() }`
  — and the bare form is spelled `catch _ { … }`, so the catch-all is a thing
  somebody typed.

The second is the honest one. It also gives the ledger's `throws` set a
consumer at the site it was inferred for.

### 2.2 `access` may change the value — or may not; the specification says both

Part I 6.3:

> `access` is for where copying is too expensive — a list of ten thousand
> entries is not copied to append one. It hands your block the value itself.
> `protokoll.access fn(log) { log.add("gebucht") }`

Part II 12.2, ADR-281 D11:

> `access` is for reading in place … and it **may not change it**.

`crates/nikaia-std/src/lock.rs` hands `access` an `&T`. Probe: `xs.access fn(v)
{ v.push(1) }` lowers and would be refused by `rustc` (*cannot borrow as
mutable*) — the Iron Rule again. A reader who trusts Part I writes a change
that never happens, and finds out in Rust's words. The page in Part I is stale
and the example in it is a program that does not compile.

**Recommendation:** fix 6.3 today. Then reconsider whether an in-place *write*
door is needed after all: `update fn(old) { let mut v = old; v.push(x); v }` is
a move in and out of the lock, which is what ADR-281 D12 argues, but it is three
lines for `push` and it moves the value out of the lock for the duration of the
block — a panic inside leaves the lock empty at `no` and poisoned at `yes`.
*Decided since:* [ADR-281](specification/adr/adr-281.md) — `update fn(mut v)`,
one form, copy or address by type, and the retry permitted where the copy is
free.

### 2.3 The lost update has a one-line refusal and a two-line hole

`NK2205` refuses `kasse.set(kasse.get() + 100)`, and the specification says the
check is syntactic: *"it catches what people write on one line and not the same
thing spread over two."* Spread over two:

```nika
let stand = kasse.get()
// a pause, or another task, or another core
kasse.set(stand + 100)
```

is a lost update at `yes` and, across a pause, at `no` too. Nothing refuses it,
nothing lints it, and the one-line refusal teaches people the two-line form.

**Recommendation:** either make it dataflow (a value that came out of `get` on
container `k` may not reach `set` on `k` in the same function — the checker has
the statements and the receiver types), or drop `get` from `SharedMut` and keep
it on `Locked`/copies only, so read-modify-write has no cheap spelling and
`update` is the only door for it.

*Decided since:* [ADR-281](specification/adr/adr-281.md), and neither of the
two: the value is **stamped** `Seen[T]` on the way out, the stamp travels
through calls, fields and time because types do, and a `set` given one — or
under a condition made of one — is refused wherever the read was. No dataflow,
nothing at run time, no way off the stamp.

### 2.4 `overlap` drops the second failure

Part I 8.1.2: *"if two fail the first in written order wins."* The other error
is gone. The language already has the mechanism for exactly this case — a
cleanup failure while an error is unwinding is *attached as a secondary error*
(6.4) — and `overlap` should use it. An operator reading a log wants to know
that two of three loads failed, not one. *Decided since:*
[ADR-292](specification/adr/adr-292.md) — one `secondary` list on every
error, for both cases, in written order, with the block as the place failures
are combined.

### 2.5 `cleanup-deadline` expiry is a warning

13.3b: on expiry *"remaining cleanups are cancelled … and the program exits with
a warning naming every resource that did not finish cleanly."* A cancelled
cleanup is an unflushed file or an unrolled-back transaction; `report.nika`'s
own reasoning — *the page is worth nothing if it did not land* — applies. The
exit status is unspecified, so a supervisor sees success.

**Recommendation:** a non-zero exit, and the warning goes to the panic hook's
path, not stdout. *Decided since:* [ADR-297](specification/adr/adr-297.md) —
exit status 70, the message on the panic path, and no setting that makes it a
`0`.

### 2.6 `?.` consumes its receiver

Part I 3.5 status: *"`?.` takes the value it reaches through, so using the
receiver again is refused by the language below with 'use of moved value'."* In
every language that has `?.` it is non-consuming. `let name = user?.name;
println(user)` failing in Rust's words is both a footgun and a C.1 violation
the specification has written down as a rule.

**Recommendation:** lower `?.` through `as_ref()` (a view) unless the member's
type is a copy; the result is a `T?` of a view, which 6.6 already knows how to
keep alive. *Decided since:* [ADR-278](specification/adr/adr-278.md), as
recommended; ADR-278 D16's translation of the move goes with the move.

### 2.7 A map index that reads panics and one that writes inserts

`scores["Player1"] = 100` inserts; `report.paths[path]` on a missing key
aborts; `.get(k)` hands back a `T?`. Three behaviours behind one bracket. Not
new — Rust and Python do it — but a language that made `null` a type of its own
so that absence is visible at the type has no reason to keep an abort behind
`[]`. Recommend `m[k]` yields `T?` on a map, or is refused in favour of `get`.
*Decided since:* [ADR-293](specification/adr/adr-293.md) — `m[k]` on a map is
a `T?`, a list keeps its abort, and `+=` on a map slot is written out with
`??`.

---

## 3. Where people will curse

Ordered by how early a newcomer meets it. Each was reproduced unless marked
*(spec)*.

### 3.1 Table stakes that are missing

| construct | today |
| :--- | :--- |
| `else if` | **parse error** — `expected '{'; found 'if'`. Every language has it; the examples work around it with nested blocks and sequential `if`s (`http/src/main.nika:status_line`). *Decided since:* [ADR-132](specification/adr/adr-132.md) — an `else` whose block is one `if`, braces left out; not built. |
| `let (a, b) = pair` | was a parse error at `61849da`; **built since** by [ADR-291](specification/adr/adr-291.md), a flat tuple of names. |
| `[1, 2, 3]` | parse error. A list literal is the first thing a scripting-language reader types. `Vec::new()` then `push`, four times, is what `n-body.nika` and `jumps.nika` do instead. *Built since:* [ADR-135](specification/adr/adr-135.md) — `[]` takes its element type from the first use and is refused where none says one, and a `[` at the start of a line begins a literal. `n-body.nika` and `jumps.nika` write the literal now. |
| `1_000_000`, `0xFF`, `0b1010` | `NK1117: nothing declares _000_000`. A language with `u8`, `Bytes`, `wrapping_shl`, an x86 DSL and a benchmark full of physical constants has no hex and no digit groups. *Built since:* [ADR-285](specification/adr/adr-285.md) — the four forms, and the radix is a spelling so `0xFF` is `255`. |
| tuple, or-, range-, guarded, nested patterns in `match` | parse error on `(1, y) =>`. `calc.nika` matches `step.0` because it cannot match `step`. Six pattern shapes, none composable. *Decided since:* [ADR-291](specification/adr/adr-291.md) — the six, a guard is `if`, and `..` in a pattern is inclusive, so `..<` becomes the exclusive range everywhere and `..=` goes. **Built**: the range spelling first, then the six shapes. `calc.nika` matches `step`. |
| `..` rest in a struct pattern | parse error *(spec status)*. *Decided since:* [ADR-291](specification/adr/adr-291.md) D10, with the other five. |
| a bare `throw` as a match arm | parse error; must be `{ throw error }`. *Built since:* [ADR-276](specification/adr/adr-276.md) — `throw`, `return`, `break` and `continue` are expressions of the never type. |
| block comments, doc comments | none. Doc comments matter here more than elsewhere: the ledger ships and the prompt bundle is on the roadmap, and neither has anywhere to take a sentence about a function from. *Decided since:* [ADR-307](specification/adr/adr-307.md) — `/* … */`, nesting, not a doc comment; and [ADR-307](specification/adr/adr-307.md) — a doc comment is a language feature and the ledger carries it in a derived `doc` column. **Both are built** for an item; a field's and a variant's wait on `nikaia doc`. |
| a function that never returns | needed an unreachable `return 0` at `61849da`; **built since** by [ADR-276](specification/adr/adr-276.md). |

None of these needs a decision any more. **Every row has a record**, and the
five that turned out to carry a question in them — what an empty literal is, how
a range pattern is spelled, whether a hexadecimal literal carries a width — were
answered in one round and left `open-decisions.md` with it. What each needs now
is the afternoon, and the issues' Priority says in what order.

### 3.2 Reserved words that block ordinary names

`from`, `in`, `as`, `with`, `quote`, `macro`, `const`, `loop` are reserved
everywhere. Probe: `fn rename(from: i64, to: i64)` — parse error, *`from` is a
reserved word*. Part III 17.1 specifies `pub fn rename(from: Path, to: Path)`
in `std::fs`: the specification's own standard library cannot be written in the
language.

`from` only means something after `dsl X` and inside a `comptime` initialiser;
`with`, `quote` and `macro` are reserved for constructs the language *promises
never to add* (ADR-304). The argument — "a reserved word can be given a
message where a name cannot" — is exactly what `NK1117`'s help text already
does for `assert` and `unsafe`.

**Recommendation:** `from` becomes contextual; `with`, `quote`, `macro` are
un-reserved (the direction ADR-298 says costs nothing); `const` and `loop` stay
only while their reopening conditions are live. *Decided since:*
[ADR-310](specification/adr/adr-310.md) — `from` is a name outright, not
contextual, and the build-time read is `asset("…")`; and
[ADR-298](specification/adr/adr-298.md) frees `loop`, `const`, `macro` and
`quote`, moving their messages to `NK1117`'s help. `with` stays, for the
copy-with-changes construct the language lacks — its own record.
Beside the words, `_`: it was a name the parser knew only in a `match` arm.
[ADR-291](specification/adr/adr-291.md) makes it the ignore pattern — a tuple
position, a parameter, a `match` arm, and nowhere else — and refuses
`let _ = expr` with `NK1144`.

### 3.3 Two spellings for one thing, several times over

* **Struct literal**: `Stats(min: first, max: first)` (Part I 4.2) and
  `Reading { name, temp }` (every example). Both build the struct. And
  `Stats(first)` calls the anonymous constructor. So `Foo(x: 1)` bypasses the
  constructor's invariants and `Foo(1)` goes through them, distinguished by a
  colon. **Pick the brace form for the literal** — it cannot be mistaken for a
  call, and shorthand fields already live there. *Decided and **built**:*
  [ADR-140](specification/adr/adr-140.md) D1, with `NK1146` naming the braces —
  and [ADR-133](specification/adr/adr-133.md)'s call half built on top of it.
* **Constructor**: `pub fn(first: i32)` (anonymous) in user code, `Type::new()`
  in `std` (`HashMap::new`, `Vec::new`, `String::new`, `Raw::new`,
  `Server::new`) — and `1brc.nika` passes `Summary::new` as a value although
  `Summary` declares an anonymous constructor and no `new`. One convention.
  *Decided and **built**:* [ADR-140](specification/adr/adr-140.md) D2 — the
  anonymous one, with `NK1149` on a written `Type::new` in either position. The
  ledger keeps the key, because that is what the lowering writes; a **package's**
  types (`Raw`, `Server`) follow the same rule the day their ledger exists.
* **Path separator**: `Op::Times`, `Summary::merge`, `http::Request` — and
  `Json.value(input)`, `T.fields` (ADR-296, 10.3) with a dot. Same kind of
  thing, two spellings. *Decided and **built**:*
  [ADR-140](specification/adr/adr-140.md) D3 — `::` everywhere, with `NK1147`
  naming it; `T::fields` is a page edit, since that construct is itself unbuilt.
* **`throws` and `sync` on either side of `->`**: `fn f() throws -> String`
  (7.1) and `fn f() -> String throws` (3.3, every example). The parser takes
  both. One order; after the type reads best. *Decided and **built**:*
  [ADR-140](specification/adr/adr-140.md) D4 — after the type, which is the
  order [ADR-277](specification/adr/adr-277.md) D6 already fixed for a function
  *type*; the pre-arrow form is a parse error that names the order.
* **`use`**: for a package *no name is brought in* (ADR-286) — but `use
  std::collections::HashMap` brings in `HashMap`, and `Vec` and `String` need
  no `use` at all. Probe confirms the import works. Two rules for one keyword,
  and the one the spec argues for is the one `std` does not follow.
* **An option without the `;`**: 17.1 writes `io::read_to_string(trusted:
  false)`; 5.1 says a named argument stands after the `;`. *Decided since:*
  [ADR-133](specification/adr/adr-133.md) — 17.1 was right.

*Decided since, all five:* [ADR-140](specification/adr/adr-140.md) picks the
brace literal, the anonymous constructor with `new` gone from `std`'s own types,
`::` everywhere, `throws` and `sync` after the result type, and `use` bringing
nothing in for `std` as for a package. **One record and five migrations**,
because they are one question — *does this language say one thing one way* — and
answering them apart would migrate the corpus five times for one property.
**D1 to D4 are built** and D5 is not; each has its own
issue. D1 went first because
[ADR-133](specification/adr/adr-133.md)'s call half was blocked on exactly the
spelling it frees, and that half is built too.

### 3.4 The subject/config protocol at its worst

`query.execute(; target_age: min_age)` and `script.exec(; msg: message)`. A
leading semicolon in an argument list is a shape no reader has seen. Since
options must be named and subjects must not be, a call with only named
arguments is unambiguous without the `;`: let `f(opt: 1)` mean `f(; opt: 1)`.
*Decided and **built** since:* [ADR-133](specification/adr/adr-133.md) —
`f(opt: 1)` is the one form and the leading `;` is refused, in a signature and at
a call; a mixed call keeps its `;`, and a driver's deferred parameters write none
either, because a receiver stands outside the parentheses. The call half waited
on [ADR-140](specification/adr/adr-140.md) D1, which took the construct it
collided with out of the language.

### 3.5 Things the specification writes that are not in the language

* `neg.is_some()` in `examples/1brc.nika:85` — the flagship example reaches for
  Rust's `Option` surface on a page that says *there is no `Some` in Nikaia*.
  It should be `neg != null`, which works.
* `io::read_to_string("input.txt")?` in Part III C.4's own diagnostic example —
  a postfix `?` that does not exist.
* `List[Token]`, `List[Row]`, `List[Fortune]` (Part I 6.6, Part II 10.5,
  `fortunes.nika`) — the type is `Vec[T]`.
* `panic::on_panic fn(info) sync { … }` — a lambda cannot carry `sync`, so the
  panic hook example is three statements.
* `supervisor::start_link(fn { … }; restart_policy: "always")` — a string where
  Part I 4.4 argues for an enum.
* `TimeoutError("Too slow!")` — an error raised as a positional constructor
  where 7.1 says an error type is an `enum`.
* `5.seconds()`, `channel::bounded(100)`, `select { … }` — unbuilt and
  unspecified in Part I.

*Decided since:* [ADR-141](specification/adr/adr-141.md), and it is **built** —
the six slips are corrected on their pages and in `1brc.nika`, and the three
**constructs** are marked *unspecified* in place rather than removed or decided
in a footnote, because each is a record's worth of question and Part I chapter 8
is where it is taken. The mark has a definition of its own beside the **Status**
note's, in [`specification/README.md`](specification/README.md): a *Status* note
stands under a decided rule the compiler has not caught up with, an *Unspecified*
mark under a construct nobody decided. `List[…]` turned out to be written in four
places rather than three.

### 3.6 The grammar sub-language

It is the most original part of the language and its syntax is the least
designed:

* `->` means *result type* and *action* on the same line:
  `rule NAME -> &str = s:until(";" | frame_end) -> { s }`.
* `=>` is a commit point here and a match arm elsewhere.
* Whether whitespace is skipped between elements depends on the **case of the
  rule's name**. `Hamburg ; 12.0` parses under `measurement` and not under
  `MEASUREMENT`, and nothing on the line says so. Three example files spend a
  comment each explaining it. A word (`token`, `lexical`) would say it.
* `rule WS = "" -> { }` is how a grammar turns whitespace skipping off: a magic
  rule name with an empty body and an empty action.
* `digit1`, `multispace1`, `line_ending`, `raw_ident`, `any`, `not(…)`,
  `until(…)`, `text(…)`, `dec[i64](…)`, `digit{1,2}`, `list(pair, ",")`,
  `recover(…)` — the built-in vocabulary is `nom`'s, and the specification
  documents none of it. Part II 10.1 shows five of these; the examples use
  fourteen.
* `# "expression"` between the type and the `=` for the failure name.

None of this is wrong; all of it is undocumented and looks like the tool it is
built on. It deserves the same pass Part I got: one page, every built-in, one
spelling for actions. *Decided since:* [ADR-296](specification/adr/adr-296.md)
— Part II 10.8 is the page, the action is the block after the pattern, `tag`
and `digit1` go; the case convention (ANTLR's), `WS` (pest's `WHITESPACE`),
`=>` and `# "…"` stay, with precedent on their side.

### 3.7 Smaller things, each real

* `x ?? y` binds so loosely that ADR-279 had to rule the fallback to *one value
  or a bracketed expression* — a symptom of `??` sitting below the range
  operator in the precedence table. Put it where C# and Kotlin put it (just
  above the comparisons) and the rule goes away.
* `while true` as the only unconditional loop is fine; the missing labelled
  `break` is not, once nested loops over a grid appear (`triangle` in
  `jumps.nika` is the shape, and it only works because the inner loop is the
  one to leave).
* `let mut first_even = 0 - 1` in Part I 3.3 and `tests/samples/jumps.nika` —
  `-1` parses; the sample teaches a workaround for a problem that is gone.
* A `spawn` body may `throw`, `join()` "does not fail", and what `join` then
  hands back is unspecified: the language has no generic `enum`, so there is no
  `Result`-shaped value for it to be.
* `catch` binds `error` implicitly; nested handlers shadow it silently. An
  optional binder (`catch e { … }`) costs nothing.
* `SharedMut(0)` at `no` is a `RefCell` whose re-entrancy is a **runtime
  panic**, and the compile-time refusal (`NK2203`) is not built. Today a
  correct-looking program aborts at run time for a rule the specification says
  is checked when you build.
* `f"{{{body}}}"` — three braces to print one braced value. Python has the same
  wart; it is still a wart.

---

## 4. What is good, and should not be touched while fixing the above

Short, because it does not need defending:

* **Colourless functions with a checked `sync`**, inferred from bodies and
  written to the ledger. This is the right design, and the "conservative in
  the restrictive direction when shipped, permissive when checked" split in
  13.5 is exactly right.
* **The ledger as a diffable artifact**, and `NK2401`'s narration of a contract
  change. Nobody else has this.
* **`f"…"` versus `"…"`** — the brace is a brace unless the string says
  otherwise. Correct, and the JSON example shows why.
* **Overflow aborts at every build; narrowing `as` aborts; wrapping is a name.**
  Swift got this right and so does this.
* **`T?` with compiler-written wrapping**, `?.` and `??` — the surface is right
  even where 2.6 says the lowering is not.
* **Provenance and the hasher**, and a path that names its root at the call
  (decided since: ADR-108).
* **Grammars as expressions, `@frame`, `par_fold`**, with the frame invariant
  checked rather than trusted.
* **The diagnostics discipline** — the Iron Rule and the polarity rule
  (*refuse only what is certainly wrong*). The gaps in §1 and §2 are mostly
  places where the rule is stated and the check is not yet there, and the
  discipline is what makes them findable.
* **`overlap { … }` instead of automatic reordering** (ADR-292). Withdrawing
  the inference was the right call.

---

## 5. Turning it inside out — the order

If the answer to "how much work" is "not a reason", this is the order, with what
each step buys.

1. **Table stakes (§3.1), reserved words (§3.2), one spelling each (§3.3), the
   stale pages (§3.5, §2.2).** Weeks, no design fights, and the language stops
   losing readers on page one. Do `else if`, the list literal, `let (a, b)`,
   number literals and `match` patterns first.

2. **`catch` takes a pattern, and the catch-all is spelled (§2.1).** The one
   change here that removes a *silent wrong value* rather than a refusal.
   Small, and it should land before the examples grow further, because every
   example is a catch-all today.

3. **Ownership is the compiler's (§1.1).** Parameters are views unless the body
   keeps the value; the caller writes no `&`; `for` lends; one text type. This
   is the change that makes the README true. It is also the largest, because it
   is an inference into the ledger and a rewrite of every example — and the
   examples are the argument for doing it: 42 `&` and 13 `.to_string()` in 913
   lines is the tax the project set out to abolish.

4. **Function types and pausing trait methods (§1.2, §1.3).** Without them the
   language cannot host a library that takes callbacks or a trait that does I/O,
   which is to say it cannot host `http`. The ledger column exists; the type
   grammar and the emitter's `async fn` in traits are the work.

5. **One integer width (§1.4).** Cheap once 3 has rewritten the examples
   anyway.

6. **The grammar sub-language gets its Part I treatment (§3.6).** Documented
   built-ins, an explicit lexical marker, one arrow for actions.

7. **Lock rules (§2.3), `overlap`'s second error (§2.4), the deadline exit code
   (§2.5), `?.` by view (§2.6), map indexing (§2.7).** Each is a day, each
   closes a way for a program to be wrong.

Everything in 1, 2, 5 and 7 can be done without a new ADR beyond the
one-paragraph kind. 3, 4 and 6 are records worth writing, because each reverses
a sentence the specification currently states as a rule — and the specification
is at its best precisely where it says so.
