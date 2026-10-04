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
