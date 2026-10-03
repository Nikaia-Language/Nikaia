# A view kept past its call: what was built, and what a full answer would need

A laboratory note. Nothing here is normative, nothing decides anything, and the
question at the end is the owner's.

It exists because two increments of one analysis are built and a third is not,
and the third is the one worth arguing about before anybody starts it.

---

## 1. The defect, reproduced

A function taking a parameter written `&str` and storing that view was lowered to
Rust that `rustc` refused. The exact output, on the emitted file:

```text
error[E0621]: explicit lifetime required in the type of `name`
  --> naked_form.rs:27:51
   |
27 |     fn record(&mut self, name: &str, temp: i32) { self.stations.insert(name, temp); }
   |                                                   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ lifetime `'a` required
   |
help: add explicit lifetime `'a` to the type of `name`
   |
27 |     fn record(&mut self, name: &'a str, temp: i32) { self.stations.insert(name, temp); }
   |                                 ++
```

The `.nika` it came from:

```nika
pub struct Summary { stations: HashMap[&str, i32] }

impl Summary {
    fn record(&mut self, name: &str, temp: i32) sync {
        self.stations.insert(name, temp)
    }
}
```

Every noun in that `help:` — lifetime, `'a`, explicit — is something Part I 6.6
promises the author never writes, and Part III C.1 calls an untranslated backend
error a bug in this compiler. So the reproduction is the whole reason the work
happened.

**The struct form was never broken**, and that was confirmed rather than assumed.
The same program with the view arriving inside a struct lowers to
`fn record(&mut self, m: Reading<'a>)` and compiles:

```nika
@borrowed
pub struct Reading { name: &str, temp: i32 }

impl Summary {
    fn record(&mut self, m: Reading) sync {
        self.stations.insert(m.name, m.temp)
    }
}
```

That is `examples/1brc.nika`'s own shape, and it is what the refusal's `help:`
now shows.

### 1.1. Three more shapes, and one that is a different defect

Each lowered and handed to the `rustc` that built the test suite.

| shape, inside `impl Summary` where `Summary` holds a view | before any of this work |
| :--- | :--- |
| `fn record(&mut self, name: &str, …)`, `self.stations.insert(name, …)` | `E0621` |
| `fn note(&mut self, name: &str)`, `self.label = name` | `E0621` |
| `fn pick(&self, name: &str) -> &str`, `return name` | `error: lifetime may not live long enough` |
| `fn has(&self, name: &str) -> bool`, `return self.stations.contains_key(name)` | compiles |
| `fn show(&self, name: &str)`, `println(name)` | compiles |

And in an `impl` whose target holds **no** view:

| shape | before |
| :--- | :--- |
| `fn make(&self, name: &str) -> Reading`, `return Reading(name: name, …)` | `error: lifetime may not live long enough` |

**A separate defect, found on the way and deliberately untouched.** A free
function that takes a view-holding struct by value and hands one back:

```nika
fn record(s: Summary, name: &str) -> Summary sync { … }
```

emits `fn record(s: Summary<'_>, name: &str) -> Summary<'_>` and fails with
`E0106`, *whether or not the view is stored* — `fn record(s: Summary, name: &str)
-> Summary { print(name); return s }` fails identically. So it is not a defect
about keeping a view, and `NK2302` would be the wrong code to report for it. What
breaks is elision in the result position when the signature names two buffers.
Nothing in the corpus writes the shape.

---

## 2. What is built

Two increments of one analysis, in `crates/nikaia/src/views.rs`. The question is
the same in both: **where does this parameter's view go?**

**The first** refuses every answer. Four destinations are reported: a field
reached from `self` whose declared type holds a view; a field of a struct or enum
the function builds; the result, where the result holds a view of a buffer that is
not this parameter's; a task's body. Anything else is not a destination — which is
what keeps `examples/k-nucleotide/src/main.nika` out of it (§4).

**The second** looks at what the view is stored *into*. A field is declared, and
its declaration says which buffer it points into ([ADR-283 D1). So where the
destination is a field of a subject that itself holds a view, the buffer is
already named, and the parameter is lowered as a view of **that** buffer —
`name: &'a str` — rather than refused. `NK2302` is what is left where the chain
runs out.

Measured on every `.nika` in the repository, both increments, both directions:

| | files that lower | files that do not | emitted Rust |
| :--- | :--- | :--- | :--- |
| before | 33 | 22 | — |
| first increment | 33 | 22 | identical |
| second increment | 33 | 22 | identical, byte for byte |

The 22 are the 21 deliberately broken files in `tests/errors/` plus
`examples/fortunes.nika`, whose `dsl postgres` has no lowering. Nothing in the
repository has the shape, in either direction: seventeen naked view parameters
stand on fifteen declarations in ten files, and **none of them is a method of a
struct that holds a view**, which is the position where `'a` exists at all. So
the corpus cost of both increments is zero, and the corpus is therefore not
evidence that either increment *works* — the four compiled-and-run programs in
`crates/nikaia/tests/stored_views.rs` are.

### 2.1. What the second increment still refuses

Three places, each a test in that file so that extending the reach is a visible
change:

1. **A buffer the emitter does not have in hand.** `impl Factory` where `Factory`
   holds no view: there is no `'a` on the impl, so even a destination that is a
   field of a view-holding struct (`Reading.name`) has no buffer to be written
   as. Refused.
2. **The result.** Covering it means writing the result's buffer as well, which
   is a second edit to the signature.
3. **A task's body.**

And one case is *accepted without being decided*: where the view is handed to a
call on the subject, nothing written down says whether the callee keeps it —
`HashMap::insert` does, `HashMap::contains_key` does not, and
`crates/nikaia-std/std.contracts` records `key: ?` for the entries it has and no
entry at all for `insert`. It is treated as kept. The cost of being wrong in that
direction is a narrower signature; the cost of being wrong the other way is Rust
that does not compile, which is the C.1 violation the work exists to remove.

---

## 3. What a full answer would be, and what it would have to track

The full rule is not "write the lifetime out further". It is [ADR-283 D2:

> Every view has one of three states… `Borrowed ⊑ Tethered ⊑ Owned`, and
> inference computes the least state that makes the program valid.

Under that rule a stored view is not an error and not a signature change — it is
a **representation** change. The buffer takes a handle (on the container, D4), the
view becomes `(offset, len)`, and nothing is refused at all. What is built today
can only write `'a` or refuse; it has no second answer.

So between here and there, three things have to be tracked that are not:

**(a) Which buffer each view points into, as a value rather than a yes/no.**
Today the analysis asks "does this type hold a view" and the emitter writes one
name, `'a`. `crates/nikaia/src/emit/mod.rs` has a single `INPUT_LIFETIME`, used
for a struct's parameter, an enum's, and an impl's; `contracts/trust.rs` states
the consequence in its own header:

> **One buffer, because Stage 0 has one input lifetime.** ADR-283's model gives a
> compilation unit a single lifetime, so it has a single input buffer… When the
> representation grows more than one, this becomes the join over each.

An analysis that concluded "this view comes from a different buffer than the
subject's" would have **nothing to emit**: there is no vocabulary for a second
buffer. It could only refuse, which is what the second increment already does for
every destination it cannot name.

**(b) The buffer of every local, not only of every field.** A field's buffer is
declared. A local's is not: `let mut counts: HashMap[&str, Tally] = HashMap()`
says the map holds views and says nothing about whose. The second increment
follows carriers **by name** (a name that ever carries the view keeps carrying it)
and never treats a local as a destination, precisely because it cannot answer this.

**(c) A solver, because the answer is a unification and not a reachability.**
`examples/k-nucleotide/src/main.nika` is the case that makes this concrete:

```nika
fn count(seq: &str, k: usize) -> HashMap[&str, Tally] {
    let mut counts: HashMap[&str, Tally] = HashMap()
    …
    counts.entry(fragment)…        // fragment is a view of seq
    …
    return counts
}
```

Views of `seq` are written into a container that outlives every statement that
wrote them, and the program is **correct** — because the container's buffer and
`seq`'s buffer are the *same* buffer, and what says so is the `return`. No amount
of following where the view goes answers that. What answers it is equating two
buffer variables and checking the equation holds, which is the monotone fixpoint
[ADR-283 D5 says the lattice shares with the region solver — and which is not
built.

### 3.1. What the compiler has at the point the analysis runs

Asked concretely, because "it would need more information" is not an answer.

| what C needs | is it there today |
| :--- | :--- |
| which fields of a type hold a view | **yes.** `emit::borrowing_structs` computes it as a fixpoint over field types, and the ledger records it per type as `tethered = […]` |
| which parameters a result may point into | **yes, but from the signature only.** `FnContract::borrows` is `borrows(a \| b)` — every view parameter, whenever the result holds a view. `Ledger::infer` never reads the body for it, so it is an over-approximation and says nothing about *which* one |
| a type for every local | **computed and then thrown away.** `check::Checker` carries `scope: Vec<Vec<(String, Ty)>>` and infers a type for every `let`, but `Checked` exports only `findings`, `fallible_loops` and `methods`. The channel for handing one more answer to the emitter already exists — `emit` asks `check::fallible_loops_against` today — so this is an export, not a new pass |
| which parameter of a `std` call keeps what it is given | **no.** `std.contracts` writes `HashMap::get(&HashMap[$K, $V], key: ?)` and has no entry for `insert` at all. This is the undecided case of §2.1, and it is a file to extend rather than a mechanism to build |
| a buffer variable per view position, and a solver over them | **no.** One `INPUT_LIFETIME`, no variables, no solver. [ADR-283 D5 argues the solver is the *same* monotone fixpoint as the region solver, so the claim is that it is not new work of a new kind — but it is not present |
| a representation other than a native reference | **no.** Nothing emits a buffer handle plus `(offset, len)`; `tethered` in the ledger is "which fields hold a view", not a solved state, and D7's "solved state and buffer-table shape" is nowhere |
| `nikaia explain --tethers`, which D6 calls the inverse tool | **no.** `docs/spec-promises.md` records `error: unrecognized subcommand 'explain'` |

Two of the seven are there, one is there and discarded, four are not.

---

## 4. What a half-built C does wrong — measured, not imagined

The obvious next step is (b) alone: let a local whose declared type holds a view
be a destination. It was built as a throwaway probe and run over the corpus.

**It refuses `examples/k-nucleotide/src/main.nika`:**

```text
lowered: 32      (33 before)
refused: 23      (22 before)
FAIL ./examples/k-nucleotide/src/main.nika :: error[NK2302]: `count` keeps `seq` past this
     call, and `seq: &str` does not say which buffer it views
```

One of the ten programs in `examples/`, correct today, refused — because the
probe can see that the view is stored and cannot see that it is stored into the
buffer it came from. The probe was reverted and the corpus is back to 33/22.

That is the whole argument about sequencing, and it generalises: **(b) without
(c) is strictly worse than nothing.** Every step that widens *where the view is
seen to go* without also answering *which buffer the destination is* converts
working programs into refusals — and a refusal of a correct program is the one
thing this compiler may not do (Part III C.4). The order is forced: (a) and (c)
first, or not at all.

A second half-built shape is worth naming because it fails more quietly. Suppose
(a) and (c) are built but the emitter still writes one `'a`. The solver would then
be able to prove two buffers distinct and the emitter would conflate them, so a
program the analysis accepted would come back from `rustc` refused — an
untranslated error about a generated file, which is the defect this work started
from, moved one layer down and made harder to find.

---

## 5. Is it possible in this compiler

**Yes, and not as an extension of what is built.** The pieces the full answer
needs are of two kinds, and only one kind is an extension.

Extensions: exporting the local type map the checker already builds, and writing
down which `std` parameters keep what they are given. Both are small, both are
useful on their own, and neither changes any program's meaning by itself.

Not extensions: a buffer variable per view position, a solver over them, and a
representation that is not a native reference. Those are [ADR-283 D2, D11, D5,
none of which is built, and the last of them is the one with real surface — a
container that holds a buffer table, `Eq`/`Hash` on a view that is content-based
rather than identity-based, and `Cleanup` running when the last tether dies
rather than at the end of the mapping scope (D8). That is a body of work with its
own decisions, and nothing in `views.rs` is in its way or a head start on it.

Meanwhile, what is built stops the defect: the program is refused in Nikaia's own
words, with the shape that works written out, and the cases where the buffer is
already named are lowered rather than refused.

### Recommendation

Stop here, and take the two small exports next if anything. `views.rs` closed the
C.1 hole and costs the corpus nothing, and the second increment turned the
fail-closed cost from a refusal into a narrower signature — so the remaining
refusals are a documented limit rather than a bug. The next increment that would
pay is *not* more reach: it is the local type map the checker already computes
(one export, no new analysis) and the missing `std` entries for which parameters
keep what they are given (one file), because each removes a guess without
widening what the analysis claims.

The full answer should not be started as a continuation of this work. It is
[ADR-283 D2's lattice and D4's buffer table, which need a representation the
emitter does not have, and §4 measured what building the reachable half first
does: it refuses a program in `examples/` that works today. If the lattice is
wanted, it wants its own decision, and the one thing to carry across from here is
that the reach must not arrive before the solver.

[ADR-283]: specification/adr/adr-283.md

---

## Postscript, 2026-09-23: `@borrowed` is gone, and the word is `@tethers`

Nothing above is withdrawn, and nothing above is edited — the transcripts and the
measurement stand as they were taken. One spelling in them has since left the
language, which is worth saying here so a reader copying a block out of §1 does
not get a parse error for it.

`@borrowed` above a struct asserted *no value of this type ever tethers*, and it
is removed ([ADR-283 D4). The word that stands there now is `@tethers`, and it
is the opposite: a **permission**, written where a struct may keep its buffer
alive, with an error where it is missing. Three things made the swap:

* **A reader could not check it.** Its whole job was to make a *silent* state
  change loud, so its presence and its absence looked identical on the page —
  *why is this here, is it needed, is it missing somewhere else* had no answer in
  the source.
* **The polarity was backwards.** The expensive state was the default. This
  note's own §1 is the shape: a view kept past its call is the thing that costs,
  and it is the thing silence used to allow.
* **It forbade nothing**, which is this note's finding read from the other end:
  the lattice is not built, so every program that would tether is refused
  already. The assertion had no transition to refuse.

The two blocks in §1 lower today with the line taken off; `@tethers` is not in
the grammar either ([ADR-283 D4), because the state it permits does not exist
yet. **The recommendation at the end of this note is unchanged**: the reach must
not arrive before the solver, and neither must the word.

