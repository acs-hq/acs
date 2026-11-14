# 0040 — `acs init` detects the main branch

Date: 2026-11-14. Status: accepted. Decided by: Claude, on the human's request to build issue 67
(the issue set the detection rule and the doctor warning).

## Context

`default_target_branch = "main"` and `protected_branches = ["main", "release/*"]` are built-in
defaults. In a repository whose main branch is `master` or `develop`, claims are compared under
the wrong target branch and no branch is protected from history rewrites by agents, and nothing
says so (`docs/DEFAULTS.md`).

## Decision

1. **Detection:** the branch `refs/remotes/<remote>/HEAD` points at, trying `origin` first and then
   the other remotes; else the current branch, when it is `main`, `master` or `develop`
   (`crates/acs-cli/src/commands/init/branch.rs`). Nothing else counts as a guess.
2. **When the branch found is not `main`, the policy files `acs init` creates say so:**
   `config.toml` sets `default_target_branch` to it, and `guards.toml` sets
   `protected_branches` to the built-in list plus it (`["main", "release/*", "master"]`). The
   built-in entries stay because setting the key replaces the defaults; `main` stays as well,
   which costs nothing where it does not exist.
3. **Existing policy files are never changed.** When they disagree with the branch found,
   `acs init` warns and leaves them; a second `acs init` therefore changes nothing.
4. **`acs init` always reports the main branch** (a `Main branch` section, `default_branch` in
   `--json`), including when none was found.
5. **`acs doctor` warns** (check `target_branch`) when `default_target_branch` names no local
   branch and no remote-tracking branch. The current branch counts before its first commit. The
   check reads local refs only; it does not ask the remote.

## Consequences

- A clone of a `master` repository is set up correctly by `acs init` with no question asked.
- A repository with no remote on a branch with another name (`trunk`) keeps `main` and is told
  so; `acs doctor` then warns.
- Remote-tracking refs are what `git fetch` last saw, so a branch deleted on the remote still
  counts until the next fetch with pruning.
