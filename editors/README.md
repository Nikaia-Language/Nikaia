# Editor support

One file: [`vscode/syntaxes/nikaia.tmLanguage.json`](vscode/syntaxes/nikaia.tmLanguage.json).

It is a **TextMate grammar** for Nikaia — the format every current syntax
highlighter reads. VS Code, Sublime Text, Zed, GitHub's linguist, `bat`, Shiki
and anything else built on `vscode-textmate` all consume this one JSON file, so
there is one grammar and not one per editor. It lives under `vscode/` because a
VS Code extension is the only consumer that needs files of its own beside it;
every other consumer wants the `.json` on its own and nothing else.

This is a **tool**, not a language decision. It needs no ADR and no change to
the specification: it only describes what is already there. Where writing it
turned up a place where the specification and the parser disagree, that is
recorded below as a defect in one of the two — not papered over here.

## What it covers

Everything in the corpus, and the constructs that are Nikaia's own rather than
Rust's with different words:

| construct | where | what the grammar does |
| :--- | :--- | :--- |
| `f"…"` interpolation | Part I 2.5, ADR-035 | the holes are **Nikaia expressions** and are highlighted as such; `"…"` has no holes at all, so a brace in it is a brace; `{{` is one brace; `\u{0041}` is an escape and not a hole; the first colon outside a call or an index is a format spec, so `{utils::double(21)}` keeps its path and `{Point(x: 1)}` its named argument |
| the `;` config zone | Part I 5.1 | the `;` inside a parameter list or a call gets `punctuation.separator.config`, and what follows it is named arguments; a statement's `;` is a terminator instead |
| `...args: Self::dsl` | ADR-296 D5 | the typed spread, with `Self::dsl` as a type rather than the `dsl` keyword |
| `break`, `continue` | Part I 3.3, ADR-276 | `keyword.control.flow`, with `return` rather than with `while` and `for`: `keyword.control.loop` is the word that **opens** a loop and these two are words that **leave** one. Neither takes a value and neither takes a label, so in each case the word is the whole token and there is nothing after it to scope |
| `throws`, `throw`, `catch`, `??` | Part I 7.1, ADR-023, ADR-025 | `catch` is an ordinary keyword in expression position; `throws` carries no type; `??` is the coalescing operator |
| `sync` | Part II 12.1 | `storage.modifier.sync`, on either side of the return type |
| `fn { … }` | Part I 5.2/5.3, ADR-277 | the one lambda form, including the trailing form outside the parentheses and a chain continuing after its `}` |
| `seq { … }` | ADR-292 D7 | a keyword only where a block follows it — `seq` is also a perfectly good variable name, and `examples/k-nucleotide/src/main.nika` uses it as one |
| `spawn`, `access`, `access_all`, `par_iter`, `await`, `join` | Part II 11–12 | `spawn` as a keyword; the rest as `support.function.concurrency`, because they are methods that demand a `sync` lambda |
| `Shared`, `Locked`, `Cleanup`, `Drop`, `Error` | Part I 6.2/6.4 | `support.class` |
| `Vec[i64]`, `HashMap[&str, Stats]` | Part I 4.6 | square brackets, never angle |
| `grammar Name { … }` | Part II 10.1 | see below |
| `dsl <target> { … } eod` | Part II 10.5 | see below |
| `@frame(boundary: "\n", unchecked)`, `@detached`, `@immediate` | ADR-296 D9 | attributes, with `unchecked` given a scope of its own because it is an assertion you make rather than a question you ask. `@borrowed` was here and is gone ([ADR-283](../docs/specification/adr/adr-283.md) D4); `@tethers` replaces it and is not built (D3), so the grammar takes neither |
| numbers | Part I 2.2 | an integer, or a float with an optional exponent. **Nothing else**: this language has no digit separators, no radix prefixes and no literal suffixes, so `1_000`, `0xFF` and `1i64` are each something other than a number and the grammar colours them as what they are |
| comments | parser `rule COMMENT = "//" until(line_ending)` | `//` to the end of the line, and that is the only comment there is — no block comment, no doc comment |

### How deep the `grammar` body goes

Fairly deep, because a grammar body really is a different language and reading it
as Nikaia would be wrong in a visible way. Inside `grammar Name { … }` the
grammar scopes:

* **Rule names by case.** An uppercase name is *lexical* and a lowercase one
  *syntactic* (Part II 10.1), and they get different scopes — that distinction is
  the whole of Nikaia's lexer/parser split, and it is invisible unless something
  says it. `entity.name.function.rule.lexical.nika` and `…rule.syntactic.nika`.
* **The commit point** `=>` as `keyword.operator.commit`, distinct from a `match`
  arm's `=>`.
* **Built-ins** (`until`, `dec`, `digit1`, `text`, `frame_end`, `par_fold`, …) as
  `support.function.grammar`, including inside nested arguments — `dec[i64](digit+)`
  and `until(";" | frame_end)` read as patterns, not as calls.
* **Bindings** (`name:pattern`) as `variable.parameter`.
* **ALL-CAPS names** as references to lexical rules; CamelCase ones fall through
  to the ordinary type rule, which is what they are. The corpus follows that
  convention everywhere (`NAME`, `TENTHS`, `MEASUREMENT` against `Reading`,
  `Summary`, `Op`).
* **Repetition** `* + ?` and a brace bound, **alternation** `|`, and the
  `# "expression"` label of Part II 10.6.

An **action block** (the `{ … }` after a pattern) is ordinary Nikaia and is
highlighted as such,
which is also what lets the grammar body find its own closing brace: every `{`
inside it is consumed by a matching `}`.

### How `dsl` bodies are handled

The body of a `dsl` block is *foreign* syntax, so highlighting it as Nikaia would
be actively wrong. It is an embedded block, and the target name dispatches:

* **`dsl html { … } eod`** — a small HTML written into this grammar rather than
  delegated to `text.html.basic`. Delegating would have lost the holes: once the
  HTML grammar enters a tag, its own patterns are the only ones active, and
  `<for r in :rows>` and `class="{r.shade}"` both put Nikaia *inside* a tag.
  So tags, attributes, quoted values, entities, comments and the doctype are
  handled here, and `{…}` holes and `:name` captures are live in all three
  positions — text, tag, attribute value.
* **`dsl mysql|postgres|postgresql|sqlite|sql { … } eod`** — the body is marked
  `meta.embedded.sql.nika` (which the VS Code manifest maps to `sql`, so that
  editor's own SQL grammar colours it) and `source.sql` is included for editors
  that resolve it. The `:name` parameters are matched first, so they win over
  whatever SQL makes of a colon.
* **anything else** — `meta.embedded.foreign.nika`, with **no** Nikaia colouring
  at all. Only `:name` is scoped, because the compiler reads those holes off the
  text whatever the grammar is (Part II 10.5).

`{…}` holes are enabled for `html` only. A template's holes need no `f` because
the `dsl html { … }` *is* the mark (ADR-035 D4), but that is the template
grammar's property and not every DSL's — in SQL a brace is a brace, and guessing
otherwise would colour someone's `GROUP BY` as code.

**The terminator.** The parser's rule is
`"dsl" name "{" until("} eod") "} eod"`, so the marker is the literal six
characters `} eod` — with exactly one space. The grammar ends the block on
`(\})[ \t]+(eod)\b`, which accepts more than one space or a tab. That is
deliberate and it is the one place the grammar is knowingly more permissive than
the parser: a stray second space would otherwise colour the whole rest of the
file as a DSL body, which is a worse way to learn about it than a compiler error.

## Trying it in VS Code

```sh
mkdir -p ~/.vscode/extensions
cp -r editors/vscode ~/.vscode/extensions/nikaia-0.0.7
```

Then restart VS Code and open any `.nika` file. `Developer: Inspect Editor
Tokens and Scopes` from the command palette shows which rule coloured what,
which is the fastest way to check a change.

There is no language server here and this is not a step towards one: this
extension contributes a grammar and a language configuration (comment character,
brackets, indentation) and no code at all.

## The tests

A grammar is a program, so it has a test. `editors/test/` runs the grammar
through **`vscode-textmate`** — the reference implementation, the same library
VS Code and Shiki tokenize with — so what passes there is what those editors do.

```sh
cd editors/test
npm install
node scope-test.mjs               # 143 scope assertions + the corpus
node scope-test.mjs --uncovered   # list every unscoped token in the corpus
node scope-test.mjs --no-catchall # the honest coverage number (see below)
node dump.mjs 'let x = f"{a:.1}"' # every token and its scopes
node dump.mjs @../../examples/1brc.nika
```

It checks three things.

1. **Scope assertions.** 143 of them, over the constructs above. Each says that
   a named token in a snippet carries a named scope, so a rule that stops firing
   fails rather than degrades quietly.

2. **Coverage over the corpus** — all 55 `.nika` files in the repository, 10 311
   non-whitespace tokens. **676 of them (6.6%) carry no scope beyond
   `source.nika`**, and every one is a plain local or field name (`i`, `n`,
   `rows`, `days_per_year`) plus three deliberate oddities from
   `tests/errors/`: the two smart quotes of `G1.nika` and the `café` of
   `G2.nika`.

   That number is measured with `--no-catchall`, which removes the grammar's last
   pattern — the one that says only "this is a name" (`variable.other.nika`).
   With that pattern in place the number is **3 of 10 314 (0.03%)**, and it says
   almost nothing, because a catch-all can always be added to make a coverage
   figure look good. Both are reported here because only the pair is honest: the
   first says how much of the corpus carries *meaning*, the second says nothing
   is being dropped on the floor.

3. **That every multi-line construct closes.** For each corpus file the test
   checks the rule stack the tokenizer ends on. A `dsl` block or an `f"…"` hole
   whose end is mis-detected leaves a region open and colours the rest of the
   file as string, and a file ending deeper than the root scope has one. All 49
   well-formed files end at the root; the six that do not are in `tests/errors/`
   and are unterminated on purpose (`A3`, `A4`, `C3`, `E1`, `F1`, `F3`).

   This check is what found the one real bug of the exercise: in
   `f"{emphasised(\"Ada\")}"` the `\"` inside the hole opened a string that never
   closed, and every line after it was coloured as one. The fix is the
   `string-escaped` rule, and the case is now asserted.

The tests are **not** wired into CI. `.github/workflows/nikaia_experiment.yml`
installs no Node toolchain, and a leg that downloads two npm packages on every
push is a change to the pipeline that ought to be made deliberately rather than
as a side effect of adding a grammar. It is four lines (`actions/setup-node`,
`npm ci`, `node scope-test.mjs`) whenever that is wanted.

## How it is maintained

**By hand, against two sources**, and not generated from the parser.

There is no mechanical route from the real grammar to a TextMate grammar, and
pretending otherwise would be the worse lie: `crates/nikaia/src/parser/mod.rs`
is a PEG with commit points, actions, a state-carrying stream and recursion, and
TextMate is a flat list of regular expressions with no stack and no recursion.
Half the language — precedence, recursion, which of two readings a PEG keeps —
cannot be expressed in it at all, and a generator would have to invent the
approximation anyway.

So the grammar was written by reading both authorities:

* [`docs/specification/10-nikaia-light.md`](../docs/specification/10-nikaia-light.md),
  [`20-nikaia-advance.md`](../docs/specification/20-nikaia-advance.md),
  [`30-nikaia-tooling.md`](../docs/specification/30-nikaia-tooling.md) — what the
  language is.
* [`crates/nikaia/src/parser/mod.rs`](../crates/nikaia/src/parser/mod.rs) — what
  the compiler accepts. Rule by rule: the `FLOAT` and `EXPONENT` rules decided
  the number patterns, `STR_CHAR` decided that a string literal spans lines, the
  `interpolation()` function in
  [`crates/nikaia/src/emit/mod.rs`](../crates/nikaia/src/emit/mod.rs) decided
  exactly where an `f"…"` hole ends and what its format spec is.

The corpus is the check: the test runs over every `.nika` file, so a construct
that appears in the repository and is not coloured shows up as a number.

**When the parser changes, this file has to be edited too**, and nothing enforces
that. The test is the thing that notices: add the new construct to a corpus file
or to a scope assertion and the grammar's gap becomes a failure.

## Where the specification and the parser disagree

Found while writing this, reported rather than quietly decided. **None is fixed
here**: the grammar highlights the shape the parser accepts, and marks the
specified-but-unparsed constructs with an extra `.unimplemented` segment in their
scope name — `keyword.declaration.trait.unimplemented.nika`. Scope selectors
match on a prefix, so a theme colours them exactly like their siblings; a scope
inspector, and a theme that wants to, can tell them apart.

### Constructs the specification defines and the parser has no rule for

| construct | specification | parser |
| :--- | :--- | :--- |
| `trait Name { … }` | Part I 4.7, with `impl Summarize for User` beside it | `rule item` has `grammar`, `struct`, `enum`, `impl`, `use`, `fn` — and no `trait`. The `impl … for …` half parses; the trait it implements cannot be declared. |
| list literal `[1, 2, 3]` | Part I 4.5, `let numbers = [1, 2, 3, 4]` | no rule. Already recorded in [`docs/error-corpus.md`](../docs/error-corpus.md): *"Nikaia has no list literal, so nothing can start at the `[`"* — which is why `tests/errors/A4.nika` reports `expected expression` at the bracket. |
| `macro Name(…) -> AstExpr`, `quote { … }`, `struct User with Describe` | Part II 10.3/10.4 | no rule for any of the three. |
| `const NAME: T = …` | Part II 10.2, and it is what makes a `dsl … from …` run at compile time | no rule. The compile-time half of dual-mode parsing has no syntax to be written in. |
| `select { … }` | Part II 12.4 | no rule. |
| `test "…" { … }`, `bench "…" { … }`, `assert` | Part III 14.1/14.2/14.4 | no rule. |
| `extern "C" { … }`, `unsafe { … }` | Part III 15.1 | no rule. |
| `null`, and a nullable type `String?` | Part I 2.3 | no `null` literal, and `rule type_ref` has no trailing `?`. `??` *is* implemented, so the operator that handles null exists while the value it handles cannot be written. |
| `?.` safe navigation | Part I 3.5 | `rule postfix_tail` has `.field`, `.method(…)` and `[index]`, and no `?.`. |
| `use nikaia_sql::{Database, mysql}` | Part II 10.5 | `rule use_item` is `"use" NAME ("::" NAME)*` — no brace group. |
| `ConfigError::BadSyntax { line, .. }` | Part I 7.1 | `rule match_pattern`'s named form takes an `ident_list`, which has no `..` rest. A `match` over an error type cannot bind some fields and ignore the others. |

### Where the two disagree about the *shape* of a construct

These are the ones worth arguing about, because both sides wrote something.

1. **`spawn`.** The specification writes `spawn fn { … }` in fourteen places
   across the three Parts, and that spelling is now the decided one: `spawn` is a
   call whose last argument is a lambda, like the lock's doors, so the
   parenthesised form is out ([ADR-281](../docs/specification/adr/adr-281.md) is
   where the lock's doors are; the `spawn` spelling is Part I 8.3 and Part II
   11.2). The parser still disagrees:
   `rule spawn_expr = "spawn" "(" body:expr ")"` — parentheses required.
   What `spawn fn { … }` does has changed, and in the right direction: since a
   trailing lambda reaches a path, it is **one** expression — a call to a
   function named `spawn` — where it used to be two statements naming a
   variable. So `rustc` says *cannot find function* rather than *cannot find
   value*, and the program is an error instead of quietly meaning something
   else. What is left is making `spawn` the `std` entry the source already
   reads it as. The grammar colours both forms, and
   `keyword.control.spawn` does not depend on what follows.

2. **`throws` with a type.** Part I 7.1 is explicit that *"`throws` names no
   types"*, and the parser agrees: `rule kw_throws = "throws"`. But Part I 6.4
   writes `fn cleanup(&mut self) throws IoError`, and so does the help text of
   `NK2601` (`fn save_report(text: String) throws IoError`). So the specification
   contradicts *itself* here, and the parser follows the one sentence rather than
   the examples. The grammar highlights `throws` with no type; a type written
   after it is coloured as the type it is, rather than as part of the keyword.

3. **A string literal spans lines.** The parser's `STRING` is
   `"\"" STR_CHAR* "\""` where `STR_CHAR` is `"\\" any` or `not("\"") any`, and
   `any` matches a newline — so a literal may contain one, and an unterminated
   literal runs to the next quote or to the end of the file. The specification
   says nothing either way. The grammar follows the parser, which is why
   `tests/errors/F1.nika` ends inside a string region, and there is an assertion
   pinning it so nobody "fixes" it to stop at the line end.

4. **Comments.** Part I never says what a comment is; the parser says
   `rule COMMENT = "//" until(line_ending)` and nothing else. No `/* … */`, no
   `///`. The grammar implements exactly that, so a `/*` in a Nikaia file is a
   division followed by a multiplication and is coloured as one.

## The site

`scripts/site-build.sh` builds the documentation site with Jekyll, which
highlights with **Rouge**, which does not read TextMate grammars; the build then
re-highlights every block with Shiki from `vscode/syntaxes/nikaia.tmLanguage.json`
(`scripts/site-highlight/`). `editors/` is added to
that file's `exclude` list for the same reason `scripts/` is on it: it is
tooling, not documentation.
