# Working in this repository

## Trunk-based: everyone pushes to `develop`, `main` only gets what was tested (#437)

* **`main` is the default branch, and what the outside sees: the last tested
  state.** A session starts on `main` or on a branch made from it, so the first
  thing to do is to switch to `develop`:
  `git fetch origin develop && git checkout -B develop origin/develop`.
* **`develop` is the trunk. Every session pushes to it directly, at once.**
  No pull requests, no waiting for CI, no full suite first: build,
  `cargo fmt`, and the tests the change touches are enough. Small commits,
  often.
* **Before pushing, rebase onto the latest `origin/develop`**
  (`git pull --rebase origin develop`). Other sessions push all the time.
  If the push is rejected, rebase again and push again.
* **A change to the language raises the version by one**: the `**Version:**`
  line of the three specification parts and a `CHANGELOG.md` heading, then
  `python3 scripts/badges.py` redraws the README's badges from that line
  (`scripts/pre-commit.sh` does it where it is installed). Never edit an SVG by
  hand; CI checks the badges with `--check`.
* **What raises it, and what does not.** The version is the specification's: it
  moves when what a program means, or what the toolchain does with it, moves.

  | Changed | Raises the version |
  | :--- | :--- |
  | `crates/` (compiler, parser, `std`) | yes |
  | Part I-III of `docs/specification/` | yes |
  | an ADR | only with the specification text it decides |
  | tests | no, unless they belong to a change above, in its package |
  | `guide/`, the notes in `docs/`, `README.md`, the website | no |
  | `examples/`, `benches/`, `scripts/`, CI, `editors/` | no |

  A change that does not raise the version gets no `CHANGELOG.md` heading or
  entry either: its commit message is its record. It touches neither the
  version lines nor the badges, so it does not collide with other sessions.
* **Nobody pushes to `main`.** CI runs everything on each push to `develop`.
  When every job passes, the `promote` job fast-forwards `main` to that commit
  (`.github/workflows/nikaia.yml`) - that commit, not `develop`'s tip, which
  may have moved on during the run. `main` is always a commit that passed.
* **The commit that finishes an issue says `Fixes #N`.** `promote` closes an
  issue only when a commit it brings to `main` names it with `Fixes`, `Closes`
  or `Resolves` (`scripts/fixed-issues.py`); `(#N)` and `Part of #N` close
  nothing. So the commit that builds the last step of a *To build* list
  writes `Fixes #N`, and also names every issue that list absorbed
  (`Fixes #439, #482`). An issue left open on purpose gets a comment saying
  exactly what is still missing. One that no commit can finish - a process
  note, a duplicate, an answered question - is closed by hand, with the reason.
* **Every open issue carries exactly one status label**: `ready`, `decision`,
  `blocked` or `note` (`docs/README.md`). Whoever changes an issue's state
  changes the label: a decision taken makes it `ready`, a step that turns out
  to need the owner makes it `decision`, a blocker built frees what it blocked.
  A new issue gets its label when it is filed.
* **A red `develop` is fixed forward.** Whoever finds it red fixes it with the
  next commit, or reverts the commit that broke it. Until then `main` stays
  where it was, and work on `develop` goes on.
* **Parallel agents in one session: one local branch each, never a shared
  `develop`.** Worktrees of one clone share their branches. If two of them
  (or a worktree and the main checkout) have `develop` checked out, a commit
  in one moves the branch under the others, and their index then shows the
  reverse of that commit as staged changes. Each agent works on its own
  branch (`git fetch origin develop && git checkout -B work-<name> origin/develop`),
  rebases with `git pull --rebase origin develop` and pushes with
  `git push origin HEAD:develop`.

## Decision rounds with the owner

Some sessions only prepare and take decisions; another session builds them.
What the owner expects from such a round:

* **Start every topic with an introduction from zero**, before any table,
  option or error code: what the feature is, a small example, what the
  program or the user sees today, and why a question arises at all. Never
  open with the question or the options.
* **Say where the question comes from**: which issue or record asks it, who
  wrote it, and whether anything concrete needs the answer now (a program,
  a user, a measurement) or it is bookkeeping. A question nothing needs may
  be answered *as it is today, until someone needs more*.
* **Check what is already decided before asking.** Read the records the issue
  cites (they may have been merged into another record since) and the code.
  Do not ask again what a record already settles, and do not take a record's
  *deferred* for the owner's current view either - say which it is.
* **Facts before options, checked in the code**: what the compiler does today
  (try it), what is measured and what is not, what goes wrong if the
  decision is wrong and how badly (a wrong value, a refusal, undefined
  behaviour). Name the risk honestly, also where it argues against the
  recommendation.
* **A topic that is not prepared well enough is not asked yet.** Say what is
  missing and prepare a decision basis first: the problem, today's state,
  how other languages and tools do it, options with their consequences, the
  measurements still owed, and a recommendation.
* **Do not ask which issue to take next**: the order is arbitrary, pick one.
* **What the language offers is decided from what it is for**, the coherence of
  its types and what other languages count as basic - never from what this
  repository's programs happen to call (ADR-288 D11 is about how a `std` entry
  is made, not about what exists). *Waits for a program that needs it* is not a
  reason; say *not decided here* and why.
* **A decision lands in three places, each with its own job: the ADR says
  why, the specification says what, the issue tells the history.** The
  deciding session writes the ADR (a new D, or a new record) and the
  specification's text (including Part III's code table) itself, in the same
  round - not left to whoever builds it. An ADR states the reasons and the
  rejected alternatives, not how the question was found or what was broken
  before; that is the issue's. The specification states the rule and nothing
  else: no status, no history, no reasons, no padding.
* **The issue gets the history and the build order**: how the question came up,
  what was checked, the decision in short with links to the ADR and the
  specification, and a numbered *To build* list with tests that another session
  can build from. Leave the issue open until it is built.

## Measuring performance

* **Count, never time.** Instructions and mispredicted branches, from callgrind
  (`valgrind --tool=callgrind --branch-sim=yes`). On these shared machines the
  same binary moves by 1.4-1.9x from one day to the next; a count does not. No
  figure in seconds goes into a document.
* **Against tuned Rust**, `unsafe` allowed, never against a loop this project
  wrote to lose. Each Rust program is a binary of its own: two programs in one
  binary change what LLVM inlines (it cost `tuned` 40 instructions a row).
* **Release builds on both sides, the same settings**: `opt-level = 3`, link-time
  optimisation on both or on neither, and say which. Nikaia keeps its overflow
  checks; measure them off as a second row (`RUSTFLAGS="-C overflow-checks=off"`)
  rather than leaving them out.
* **The same output first**, byte for byte, or the counts mean nothing.
* **Length in tokens, not lines**: `scripts/tokens.py`, the same rule for both
  languages, whole files, `use` lines included.
* **A benchmark's README holds the current table and the method, not a diary.**
  Re-measuring replaces the table; how a number got there is the commit's and
  the issue's. Quote only that table, and before recommending a change from an
  attribution, check it was taken on today's code.
* **Understand a result before publishing it**: if a number moves, find out why
  (callgrind_annotate) before anything quotes it. `benches/brc/brc.sh` is the
  pattern: it builds, compares the outputs, counts and prints the table.

## Moving Rust into Nikaia (self-hosting, ADR-294, #125)

Rules from the sessions that moved the most; they save tokens, not just time.

* **Pick the slice by what is pure.** A stateful struct (`Checker`) cannot move
  whole: what moves is message-building, small decisions and walks that need no
  state. `grep 'Finding {'` and `format!(` first (about 15 Rust lines become 4
  Nikaia). A walk that needs state takes a callback
  (`fn(ref Expr) -> i64 sync`; see `bounds_basic.nika`). Code keyed on
  expression addresses, or mutating the tree, does not move yet.
* **Copy a pattern, do not invent one.** `libraries.nika` + `libraries.rs`
  (small), `assets.nika`, `parse_numbers.nika` (callbacks), `interpreter.nika`.
  The Rust module stays as the adapter that calls `nikaia_std::tools::<m>::f`.
* **Builds are the cost.** A first release build is 10-30 minutes on these
  shared machines, later ones 2-7. `nikaia lower-std` needs a built
  `target/release/nikaia` and takes seconds. Keep the Rust compiling until the
  binary exists. Loop on `cargo check --release -p nikaia --message-format
  short` (under a minute); build once at the end. Run the touched tests as one
  `cargo test --release -p nikaia --test a --test b` (grep the NK codes you
  touched in `crates/nikaia/tests/*.rs`), in the background. No foreground
  `sleep`; poll with a bounded loop.
* **One namespace for all `tools/*.nika`.** Type and function names are global
  (`Decision` clashed): prefix them with the module.
* **Syntax gaps** (each cost a build cycle):
  * no `if let`: use `x ?? default`, or `starts_with` then `strip_prefix(..) ?? ""`;
  * no `*x`; loop variables over a `ref Vec` are values;
  * `.to_owned()` is `NK1189`: write `.clone()`;
  * a literal is a `ref String`: `let mut s: String = "lit"` before assigning
    an f-string (`NK1105`); a list mixing `f"..."` and a plain literal is
    `NK1154`, so write `f"..."` or `.clone()`;
  * `opt ?? ""` moves the option: `(opt ?? "").clone()` for a second use;
    no method call on a `String?` directly;
  * f-string braces are `{{` `}}`; a plain string takes braces literally;
  * a function parameter is `fn(ref X) -> T sync`; a `Span` that arrives by
    reference is copied as `Span { start: s.start, end: s.end }`.
* **Lowered types:** `ref String` is `&str`, `ref T` is `&T`, `Vec[String]` is
  `Vec<String>` (a `ref` one a slice), `String?` is `Option<String>`. Most Rust
  errors after a move are `&` against owned. Check that `Display` equals
  `Ty::text()` before replacing `{ty}` with it.
* **Delete the dead locals** a replaced block leaves behind (`cargo check` lists
  them as unused), and re-grep function names instead of trusting line numbers
  from an earlier listing: every commit shifts them.
* **Versions and badges:** compute the next version after the rebase. On a
  conflict in the version lines, `CHANGELOG.md` or `assets/badges/*.svg`, take
  the next free number and re-run `python3 scripts/badges.py`; never merge an
  SVG by hand. Add each new `.nika` to the table in `scripts/self_hosting.py`.
  The share counts code lines, not comments.
* **Tooling traps:** complex shell (computed `sed` ranges, `awk -v`, heredocs
  chained with `&&`) is refused in a worktree: write a small Python script and
  run it. Replace blocks bottom-up with exact-string asserts.
* **Known compiler defects, to fix in the compiler** (ADR-294 D3), meanwhile
  bind to a `let` first: `let v = f().trim_start()` drops a temporary (E0716);
  `??` with an index read as fallback emits an ambiguous `.into()`; a `match`
  in a `T?` function mixing optional and plain arms; a `String` reassigned from
  a `strip_prefix` view in a loop lowers as `EitherText`; `mut` parameter plus
  `.drain()`; `Spanned(x, span)` lowers to invalid Rust; "never read" and
  unreachable-code warnings in the generated package.
* **Agents: few, one per large module, none on a file another one edits.**
  Splitting one file into slices costs more in conflicts and repeated context
  than it saves. Leaves first: a consumer moved before its dependencies finds
  its errors late. Record the state in the commit message after each chunk.
