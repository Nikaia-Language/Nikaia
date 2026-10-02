# What the specification promised, and what the compiler answers

The laboratory notebook page behind the **Status** notes added to Parts I–III.
Nothing here is normative: the rule is in the specification, and this is the
evidence that each note says the truth.

## Method

Every row below is a `.nika` file of one or two lines, written to use exactly one
construct and handed to the compiler as

```
nikaia lower <probe>.nika -o <probe>.rs
```

on the release build of the branch's `origin/main` base. The **Result** column
records what happened, in three kinds:

* **parse error** — the file is refused, and the message is quoted.
* **parses, means something else** — the file is *accepted*, and the emitted Rust
  is not what the source says. This is the dangerous kind: no construct in the
  parser is keyword-protected, so a bare keyword becomes an ordinary name and the
  brace-led thing after it becomes a statement of its own.
* **built** — the construct exists and lowers.

Four rows were re-run after the run below, because the parser changed under
them: the three trailing-lambda rows are now **built**, and the row they were
re-run beside — `fn(a) sync { … }` — is unchanged. Those four say "re-run" in
the Result column; every other row is the original run.

## The constructs

| Construct | Specified in | Result |
| :--- | :--- | :--- |
| `trait Name { … }` | I 4.7 | parse error: *expected end of input; found `trait`* |
| `pub trait Name { … }` | I 4.7 | parse error: *expected one of `enum`, `struct`; found `trait`* |
| `impl Name for T { … }` | I 4.7 | **built** |
| `[1, 2, 3]` | I 4.5, II 12.6, III 14.4 | parse error: *expected expression; found `[`* |
| `[]` | II 12.6 | parse error: *expected expression; found `[`* |
| `-> [User]` (list type) | III 14.3 | parse error: *expected type; found `[`* |
| `Vec[i64]`, `xs[0]`, `xs[k] = v` | I 4.5 | **built** |
| `macro Name(…) -> AstExpr { … }` | II 10.3 | **re-run, and withdrawn**: `macro` is a reserved word that is not a construct ([ADR-088](specification/adr/adr-088.md) D7), so the parse error now carries *"`macro` is a reserved word, so it is not a name"*. 10.3 no longer specifies a macro system |
| `quote { … }` | II 10.3, 10.4 | it used to parse as `let q = quote;` then `{ 1 + 1 }`, then was refused by `NK1117` — and is now refused **by the grammar**, which is earlier and better: `quote` is a reserved word that is not a construct ([ADR-088](specification/adr/adr-088.md) D7). It never meant something else in silence again |
| `struct User with Describe { … }` | II 10.3 | **re-run, and withdrawn**: same as the two above — the parse error now names `with` as a reserved word. Nothing is attached to a declaration, because nothing is generated ([ADR-088](specification/adr/adr-088.md) D1) |
| `const NAME: T = …` (item) | II 10.2, I 9.2 | parse error: *expected end of input; found `const`* |
| `const NAME = …` (in a body) | II 10.2 | parses as `const; LIMIT = 10;` — **re-run**, and refused now: `NK1117` on the name |
| `select { … }` | II 12.4 | parse error: *expected expression; found `>`* (at the arm's `=>`) |
| `test "name" { … }` | III 14.1 | **built** ([ADR-264](specification/adr/adr-264.md) D1): compiled only by `nikaia test`, each run in a process of its own |
| `test "name" (u: User) { … }` | III 14.3 | parse error: *expected end of input; found `test`* |
| `bench "name" { … }` | III 14.4 | parse error: *expected end of input; found `bench`* |
| `assert cond` | III 14.1, 14.2 | parses as `assert; b != 0;` — **re-run**, and refused now: `NK1117`, *"nothing declares `assert`, and this statement is just that name"*. The assertion still does not exist; it no longer disappears in silence |
| `assert cond, "message"` | III 14.2 | parse error: *expected `}`; found `,`* |
| `assert(cond)`, `assert(cond; message: …)` | III 14.2 | **built** ([ADR-264](specification/adr/adr-264.md) D2, D3): a prelude function the compiler knows; a condition that could pause, fail, touch or change anything is `NK1194`, another shape `NK1195`, and a false one stops the program naming its line, its claim and its operands' values |
| `extern "C" { … }` | III 15.1 | parse error: *expected end of input; found `extern`* |
| `unsafe { … }` | III 15.1, 16.3 | parses as `unsafe; { puts("hi") }` — **re-run**, and refused now: `NK1117` on the name |
| `break`, `continue` | I 3.3 | **built** ([ADR-073](specification/adr/adr-073.md)): name for name, and the rule that bounds them is `NK1126` — a jump may not leave a lambda, a task, an `overlap` branch or a DSL fold's step, each being a function in the language below. **A row added by a construct arriving rather than by a probe**: before the record the two words parsed as nothing at all, since [ADR-071](specification/adr/adr-071.md) had reserved them, so the probe that belongs beside the others here is `break x` — refused, `NK1127`, because a jump takes no value and a value after one would otherwise be a statement of its own |
| `null` | I 2.3, 3.5 | **built** ([ADR-052](specification/adr/adr-052.md)): a reserved word, lowering to `None` |
| `String?`, `ref String?` | I 2.3 | **built**: `Option<String>`, `Option<&str>` — and the wrap where a plain value stands in a nullable slot is the compiler's to write, in all five positions. `Some(…)` where the type is known, `.into()` where it is not, which is right either way ([ADR-068](specification/adr/adr-068.md)) |
| `a?.b` | I 3.5 | **built** ([ADR-052](specification/adr/adr-052.md) D6): `map` over a plain field, `and_then` over one that is itself a `T?` |
| `a?.m()` | I 3.5 | **built** ([ADR-066](specification/adr/adr-066.md) D1): the section's word is *member* and a method is one. A `match` and not the field's `map`, because a method may pause and may fail |
| `a ?? b ?? c` | I 3.5 | **built** ([ADR-066](specification/adr/adr-066.md) D4): right-associative. It used to be a parse error naming the second `??` |
| `a?.b.c` | I 2.3, 3.5 | **refused**, `NK1125` ([ADR-066](specification/adr/adr-066.md) D6): a `?.` guards its own member and no more, and a `T?` is a type of its own — so the unguarded `.c` is a member the type does not have. `a?.b?.c` is the program |
| `a ?? 1` | I 3.5 | **built**: `a.unwrap_or_else(\|\| 1.into())` |
| `f(a??)` (postfix unwrap) | — [ADR-018](specification/adr/adr-018.md) D3 and III 17.1 write one; **I 3.5 does not define it** | parse error, and since [ADR-089](specification/adr/adr-089.md) D2 it is that record's sentence: *expected one value, or an expression in brackets* — the `??` is read as the infix one missing its fallback, which is what it is. The two examples that write one are on [[#191](https://github.com/Nikaia-Language/Nikaia/issues/191)](https://github.com/Nikaia-Language/Nikaia/issues)'s list, as *two examples write a postfix `??` the language does not have* |
| `Conn(id: 1)` (same file) | I 4.2 | **built** |
| `http::Response(status: 400, …)` | III 17.1, [ADR-018](specification/adr/adr-018.md) D3 | **parses**, and lowers to `http::Response { status: 400 }` — the qualified path used to be the parse error here (*expected one of `)`, `,`; found `:`*) and is not any more. What is missing is `http` itself, which is a library and not a syntax ([[#90](https://github.com/Nikaia-Language/Nikaia/issues/90)](https://github.com/Nikaia-Language/Nikaia/issues), *there is no HTTP server*) |
| `use a::{b, c}` | II 10.5 | **re-run**, and a sentence now: *"names are not brought in; a package is reached through its name. Write `use http`, and `http::Request` where you need it — and `use http as h` if the prefix is long"*, with the caret on the brace ([ADR-046](specification/adr/adr-046.md) D2) |
| `use std::fs`, `use utils` | I 9.1 | **built** |
| `Point { line, .. }` (pattern) | I 7.1 | parse error: *expected `}`; found `.`* |
| `Point { line }` (pattern) | I 3.4 | **built** |
| `_ => throw error` (arm body) | I 7.1 | parse error: *expected one of `(`, `{`* |
| `_ => { throw error }` | — | **built** |
| `fn f(g: fn())` (function type) | I 5.4 C, III App. B.2 | parse error: *expected `)`; found `(`* |
| `fn f(g: @detached fn())` | I 5.4 C | parse error: *expected type; found `@`* |
| `x.m fn { … }` | I 5.3 | **built** |
| `x.m(a) fn { … }` | I 5.3 | **built** |
| `x.m fn(a) { … }` | I 5.3, II 12.3 | **built** (re-run after the trailing lambda took a parameter head): `users.map(\|user\| { … })` |
| `f(a, b) fn(x, y) { … }` | II 12.3 | **built** (re-run): `access_all(a, b, \|x, y\| { … })` |
| `p::q fn(s) { … }` / `p::q fn { … }` | II 12.7 | **built** (re-run): `task::scope(\|s\| { … })` — the callee has no `std` entry, so the *call* is what is built |
| `fn(a) sync { … }` | I 7.2 | parses, means something else: `fn(a); sync { a };` — **re-run, and refused** since `sync` became a reserved word ([ADR-051](specification/adr/adr-051.md)): *expected `{`; found unexpected token `sync`*, at the annotation. What is unbuilt is unchanged — a parameter list is still followed by the body and by nothing else — and the three statements are gone |
| `1_000` | I 2.2 (silent) | parses, means something else: `let x = 1; _000;` |
| `0xFF` | I 2.2 (silent) | parses, means something else: `let x = 0; xFF;` |
| `1i64` | I 2.2 (silent) | parses, means something else: `let x = 1; i64;` |
| `/* … */` | I ch. 2 (silent) | parse error: *expected `}`; found `/`* |
| `/// doc` | I ch. 2 (silent) | parses as an ordinary `//` comment |
| unterminated `"…` | I 2.5 (silent) | the newline is inside the literal; the literal runs to the next `"` or to end of file |

## The commands

`nikaia --help` on the same build offers `build`, `run`, `lower-std` and `help`.

| Command | Specified in | Result |
| :--- | :--- | :--- |
| `nikaia build`, `nikaia run` | III 13.2 | **built** |
| `nikaia lower-std` | III 13.2b | **built** |
| `nikaia new` | III 13.1 | `error: unrecognized subcommand 'new'` |
| `nikaia test` | III 13.2 | **built** ([ADR-264](specification/adr/adr-264.md) D1, D12), with `--both-settings` |
| `nikaia bench`, `nikaia fmt` | III 13.2 | not present |
| `nikaia explain <code>`, `nikaia explain --tethers` | I 6.6, 7.1 | not present: `nikaia explain` takes a `.nika` file ([ADR-260](specification/adr/adr-260.md) D3), so `NK-2C7` is read as a file name |
| `nikaia build --with-asserts` | III 14.2 | **withdrawn** ([ADR-264](specification/adr/adr-264.md) D4): no build removes an `assert`, so there is nothing for the flag to put back |
| `nikaia bench --history` | III 14.4 | no such command |
| `nikaia lower`, `nikaia interpret`, `nikaia explain <file>` | III 13.2 | **built** ([ADR-260](specification/adr/adr-260.md) D3); `--input` and `--backend` are gone (D5) |
| `--locked`, `--overlaps`, `--trust`, `--no-cache`, `--target`, `--user-parallelism` | III 13.2, 13.5, I 8.1.2 | **built** |

Note that `nikaia explain <file>.nika` (read `rustc` JSON on stdin) and Part I's
`nikaia explain <code>` (from an error's site to the line that raised it) are
two different things; only the first exists.

## The diagnostics

Codes the compiler emits: `NK1101`–`NK1113`, `NK2202`, `NK2501`, `NK2502`,
`NK2605`, `NK2701`. Codes Part III C.3 catalogues and nothing raises: `NK2101`,
`NK2102`, `NK2201`, `NK2301`, `NK2401`, `NK2601`, `NK2602`, `NK2603`, `NK2604`.

> **The two lists above are this page's own reading on the day it was written**,
> and they have moved since; Part III C.3's status note is the current one.
> `NK2101` has crossed over — `spawn` lowers, so there is a task to hand data
> to ([ADR-055](specification/adr/adr-055.md) §6 step 4) — and so have several
> `NK11xx`. The lists are left as they were, for the reason the whole page is:
> a page that quietly updates its findings cannot be read against the day it
> was written.

`NK2605` was added after this page was written, for the shape
[`from-for-throws-and-touches.md`](history/from-for-throws-and-touches.md) §6 found: a
written call that can fail, in a function that declares nothing.

## Left for the owner

Three things were **not** decided here and the specification was unchanged at
every one of their sites. The first two have since been decided; the notes are
kept with what they were, because a page that quietly loses a finding cannot be
read against the day it was written.

1. **`spawn`.** The specification writes `spawn fn { … }` twelve times
   — six in Part I (5.4 B, 8.2, and four in 8.3, two of them inside `NK2101`'s
   own text and `help:`) and six in Part II (11.2, 12.5, two in 12.7, and two
   inside `NK2102`'s text and `help:`) — and the parser is `spawn "(" expr ")"`.
   `spawn fn { … }` parsed as `spawn; || { … };`, two statements.

   **Re-run after the trailing lambda reached a path.** It is now *one*
   expression, `spawn(|| { … })`, and `spawn fn(x) { … }` likewise — a call to a
   function named `spawn`, which is what the source says and what nothing
   provides. `rustc` moved from `cannot find value 'spawn' in this scope` to
   `cannot find function 'spawn' in this scope`: the same `E0425`, one noun
   closer to the truth, and still an error. Whether `spawn` is a keyword form
   that becomes a task or a `std` function that takes a lambda is the decision
   left here; nothing in the parser's `spawn` rule changed.

   **Re-run again, after the reserved-word list.** `spawn` is a reserved word
   now ([ADR-051](specification/adr/adr-051.md)), so there is no variable and no
   function of that name for the line to be read as: `spawn fn { … }` is
   *expected `(`; found unexpected token `fn`*, this compiler's refusal at the
   `fn` rather than `rustc`'s about the generated file. Both readings above are
   gone and the decision left here is untouched — what `spawn` **means** is
   still [#219](https://github.com/Nikaia-Language/Nikaia/issues/219)'s `spawn` entry, and the parser's `spawn "(" expr ")"` rule is still
   what it was.

   **The spelling is settled; the rest of this entry is not.** `spawn fn { … }`
   is the one form, in all three Parts — the parenthesised `spawn({ … })` that
   Part III Appendix C.5's `NK2501` example wrote is the thirteenth site this
   page did not count, and it is now `spawn fn { … }` like the other twelve.
   That closes the *disagreement between two normative documents* and nothing
   else: `spawn` is a call whose last argument is a lambda, which is the trailing
   lambda Part I 5.3 already has, and the parser's `spawn "(" expr ")"` rule is
   still what it was. So the compiler-against-specification half of this entry
   stands exactly as written above.

   **Closed** ([ADR-055](specification/adr/adr-055.md) §6 step 4). The decision
   left here — *"whether `spawn` is a keyword form that becomes a task or a
   `std` function that takes a lambda"* — turned out to be answered by the thing
   nobody had asked: what a task **means** at `user_parallelism = no`. Part II
   11.2 says *"interleaved on the same thread"*, two synchronous Rust closures
   cannot interleave, and the emitted Rust contained the word `async` zero
   times. So the lowering moved, and with an executor to run it `spawn` is a
   keyword form: `TaskHandle::start(async move { … })`, with the body an `async`
   **block** because a task's body is a block rather than something called, and
   a closure around it would only be called once. (*Not* because Rust has no
   stable `async` closure — it has one, and this line used to say otherwise:
   [ADR-187](specification/adr/adr-187.md) D1, D2.) What settles it against a
   `std` function taking a lambda is Part II 11.2 above and the parser: the
   rule is now the trailing lambda, so `spawn fn
   { … }` parses as the one form, and the parenthesised spelling says what
   happened to it. Twelve sites, one reading, one rule.
2. ~~**`throws` with a type.**~~ **Settled, and fixed.** Part I 7.1 states that
   `throws` names no types and the parser agrees; the four sites that
   contradicted it — Part I 6.4 once in code, twice in prose and once inside
   `NK2601`'s own `help:`, and Part II 10.2 B's `throws ParseError` — are
   corrected to Part I 7.1's form. `throws IoError` still does not parse, and
   the refusal is now a sentence rather than a parse error at the type name:
   `kw_throws` carries a `fail(…)` citing [ADR-023](specification/adr/adr-023.md)
   D1 and pointing at `nikaia.contracts`, on the precedent of ADR-022's removed
   `fn: …` form in the same file. The `help:` line was the worst of the four,
   because the compiler's own suggested fix told the user to write what the
   compiler refuses.
3. **Where the honest fix is a removal.** ~~Part II 10.3–10.4 is two pages of
   `macro` and `quote` with nothing underneath them~~ — **decided, and it was a
   removal** ([ADR-088](specification/adr/adr-088.md)). The two pages described a
   macro system with three constructs; what they wanted, generating code from a
   type's shape, needs none of them, because `comptime` plus a bound is enough.
   10.3 is rewritten around that and 10.4's hygiene question dissolved with the
   construct it applied to. The three words are reserved and are **not**
   constructs, which is the one thing the pages now promise.
   Part III Chapter 14 is still a whole chapter; it carries a Status note, and
   whether a chapter specified that far ahead should stay is a decision, not a
   documentation defect.
