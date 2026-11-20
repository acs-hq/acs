# 0049 — The hook is benchmarked in a repository with lore; no cache of parsed lore yet

Date: 2026-11-20. Status: accepted.

Decided by: Claude, on issue 57's brief: which cases are measured, which carry a budget, and not
building a cache of the parsed entries now. The human has not seen these yet. Issue 81 holds
what is left open.

## Context

Issue 57: in a repository with files under `.acs/policy/lore/`, the pre-tool hook does work that
no benchmark measured. Every pre-tool call reads and parses every lore file (there is no compiled
cache, unlike the policy files). The first call of a session in a scope measures the staleness of
the entries it has not seen yet, cached per `HEAD` in `cache/lore-stale.json`. Reads have their
paths resolved, which the gate alone does not need. The issue asks for lore cases in
`benches/hook_paths.rs`, and for a cache of the parsed entries if the per-call load misses the
budget.

Decision 0048 added a hook at the end of every agent turn (`turn_end`, the harnesses' `Stop`). It
runs once per turn, not per tool call, and was not measured either.

## Decision

1. **Six cases in a repository with 50 lore entries** (`lore_calls` in `benches/hook_paths.rs`).
   One entry covers `src/**` and 49 cover `src/m<i>/**`, so a file in a module is in the scope of
   two entries:

   | Case | Budget |
   | --- | --- |
   | Pre-tool edit in a module whose entries were already shown | Pre-tool, 15 / 40 ms |
   | Pre-tool read in such a module | Pre-tool, 15 / 40 ms |
   | Pre-tool edit outside every scope (`docs/`) | Pre-tool, 15 / 40 ms |
   | First read of a new session in a module, staleness already cached for this `HEAD` | None |
   | The same right after a commit (staleness measured again) | None |
   | `turn_end` of a session that was already asked for lore | None |

   The three warm calls are what an agent pays on almost every call, so they carry the plan's
   pre-tool budget (section 4.7), and CI gates them on Linux like the other budgets. The first
   touches happen once per session and scope, or once per commit, so they are measured to be
   watched, not judged. Before measuring, the run checks that each first call really showed the
   entry's text, so a broken setup cannot pass by measuring nothing.
2. **`ACS_PERF_LORE_ENTRIES`** changes the number of entries (default 50), to see how the cost
   grows.
3. **No cache of the parsed entries for now.** The issue's rule is "if the per-call load misses
   the budget". On Windows every path misses its budget with or without lore (issue 2), so the
   rule says nothing there. What lore adds at 50 entries is 6 to 8 ms per call, and on Linux, where
   CI gates, the pre-tool path runs in about 6 ms in total. A cache keyed by a hash of the
   files, as `acs-policy` does, would also not remove the main cost: computing the hash still
   opens and reads every file, and only the parsing would be saved. So the cache is left to issue
   91, with the numbers below, to be built against a measured split between reading and parsing.

## Measured

`ACS_PERF_NO_ASSERT=1 cargo bench -p acs-cli --bench hook_paths` (release build, the bench
profile), on the human's Windows machine (Intel Core i7-1165G7, 4 cores / 8 threads, 16 GB,
NVMe SSD, Windows 11 Home build 26200, Avast on, git 2.45.2, rustc 1.97.0), at `3549f90` plus
this change, on 2026-11-20 with no other work running, 100,000 files for the post-tool path. p50 / p99 in ms,
process start included.

| Call | Calls | p50 / p99 |
| --- | --- | --- |
| Pre-tool edit, warm, no lore (for comparison) | 300 | 48.8 / 67.2 |
| Pre-tool read, every switch at its default, no lore (for comparison) | 200 | 35.1 / 65.5 |
| Pre-tool edit, 50 entries, already shown | 200 | 54.7 / 124.3 |
| Pre-tool read, 50 entries, already shown | 200 | 42.6 / 79.7 |
| Pre-tool edit, 50 entries, none in scope | 200 | 55.7 / 78.3 |
| First read of a session in a scope (two entries), `HEAD` cached | 100 | 83.8 / 103.0 |
| First read of a session in a scope (two entries), new `HEAD` | 100 | 436.9 / 1193.8 |
| `turn_end`, already asked | 200 | 23.0 / 28.3 |
| Process start alone (`acs --version`) | 200 | 15.9 / 18.6 |

The same run's other budgets: post-tool shell with 100,000 files 120.7 / 146.0, `acs status`
66.2 / 76.5. A trial run with 500 entries (2,000 files for the post-tool path, otherwise the same)
gave a warm read of 114.5 ms p50 against 34.3 without lore, and a warm edit of 132.6 against 50.5.

- **Every call pays about 0.15 ms per lore entry on this machine:** 6 to 8 ms at 50 entries,
  about 80 ms at 500.
- **The first read of a session in a scope costs about 40 ms more than a warm read** when the
  staleness is cached. That includes a new session's first call; one `git rev-parse HEAD`,
  which runs on every first touch, costs roughly that much on Windows on its own.
- **Right after a commit, the first touch costs about 350 ms more for two entries** (p99 1.2 s).
  The issue expected two `git diff` runs per entry; there are four git processes per entry
  (`rev-parse` of `written_against`, `hash-object` of the empty tree, and two `diff --numstat`),
  plus the `rev-parse HEAD`. On Windows each costs about 40 ms. Only one of the four depends on
  `HEAD`, yet all four run again after every commit.
- **The staleness cache holds one `HEAD`, and every worktree of a repository shares it**
  (`repo_home` is per repository, not per work tree). Agents in worktrees on different commits
  therefore reset each other's cache. This was read in the code, not measured.
- **`turn_end` costs as much as a call that ends early** (decision 0035): about 7 ms more than
  starting the process, once per turn.

## Consequences

- CI's Linux perf job now also gates the three warm lore calls. Its numbers come with the next
  push; the Windows and macOS jobs only report (decision 0023).
- `ACS_PERF_BASE_BIN` must be a build that knows the `turn_end` phase (decision 0048 or later),
  or the run stops at the `turn_end` case. CI compares with the commit before, which is new
  enough.
- Issue 81 holds the work these numbers point to: fewer git processes after a commit, a
  staleness cache that worktrees do not reset, and a cache of parsed entries once a reading
  versus parsing split is measured.

## Not done, not checked

- No Linux or macOS numbers for the new cases yet (they come from CI after the push).
- The 40 ms of a first touch with the cache warm was not split between lore and the new session's
  first call.
- Repositories with lore files much larger than these one-line entries were not measured.
