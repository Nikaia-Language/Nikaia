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
* **A change package raises the version by one**: the `**Version:**` line of
  the three specification parts and a `CHANGELOG.md` heading, then
  `python3 scripts/badges.py` redraws the README's badges from that line
  (`scripts/pre-commit.sh` does it where it is installed). Never edit an SVG by
  hand; CI checks the badges with `--check`.
* **Nobody pushes to `main`.** CI runs everything on each push to `develop`.
  When every job passes, the `promote` job fast-forwards `main` to that commit
  (`.github/workflows/nikaia.yml`) - that commit, not `develop`'s tip, which
  may have moved on during the run. `main` is always a commit that passed.
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
