# 0055 — The CI perf job runs only on request

Date: 2026-11-22. Status: accepted.

Decided by: the human, that the perf job stops running on every push and runs only when a change
could affect hook latency (2026-11-22, waiting on CI for issue 79). Claude for how: the triggers
and the rule for sessions in `CLAUDE.md`.

## Context

The `perf` job of `.github/workflows/ci.yml` builds the commit before and runs
`cargo bench -p acs-cli --bench hook_paths` on Linux, macOS and Windows, twice when the first try
fails (plan WP10.1, decision 0023). A CI run waits about 35 minutes for it, while fmt, tests, deny
and the release plan finish well before. Most changes (docs, tests, the updater, commands that no
hook runs) cannot move hook latency, and every one of them still waited for it.

## Decision

1. **The perf job runs on a release tag, on a manual run of the CI workflow, or on a push whose
   commit messages contain `[perf]`.** Every other run skips it; the other jobs are unchanged. The
   budgets and the 20% regression rule are unchanged when it runs. A pull request gets it by a
   manual run on its branch (`gh workflow run CI --ref <branch>`).
2. **Sessions decide when it is needed** (rule in `CLAUDE.md`): a change to what a hook call runs
   (the hook pipelines, `runtime`, the engine, the store, paths, shell, policy, lore, git, OS
   crates, the adapters), to the processes a hook path starts, to dependencies or build profiles,
   or to `benches/`. Such a change is merged with `[perf]` in a commit message, and sessions run the
   local benchmark only for those changes too.

## Consequences

- A latency regression in a change nobody marked shows up at the next marked push or at the release
  tag, compared then with the commit before that push, not with the commit that caused it.
- A run without the perf job is green sooner, so `main` can be pushed and tagged without the wait.
