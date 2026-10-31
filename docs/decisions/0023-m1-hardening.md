# 0023 — M1 hardening: collisions in status, benchmarks, scenarios (WP10.1, WP10.2)

Date: 2026-10-31. Status: accepted for the code; **the Windows budgets are open for the human**.

## Context

The M1 gate says every collision is visible in `acs status`, but `acs status` listed live claims
only, so a collision left no trace once its seats ended. WP10.1 needs the budgets of plan section
4.7 checked in CI, and they had never been measured on Windows. WP10.2 lists scenarios from several
milestones.

## Decision

- **`acs status` shows collisions.** A collision is a refused edit, a refused `acs claim` or
  `acs_claim`, or a commit the backstop stops, because another seat holds a file. It goes in the
  event log with reason code `claim_conflict`, keeping the message the seat was told, because that
  names the holder. `acs status` shows the last 24 hours, and `--json` gives up to 200 entries plus
  `collisions_total`. Migration 0004 adds a partial index, so the cost does not grow with the log.
  Collisions are this machine's only; a team's arrive with the ledger (WP4.1).
- **M1's scenarios are in `tests/e2e/`:** acceptance, sealed paths, an offline team, husky, and the
  junction delete being stopped. Recovery and interleaved undo need `acs undo` (M2); queued edits and
  interface notices are M5. The acceptance test proves its own check can fail: with the gate off, the
  same writes lose a line.
- **Benchmarks are `benches/hook_paths.rs`,** with process start included. Criterion reports means,
  so the p50 and p99 come from the individual calls. It replaces `crates/acs-cli/tests/perf.rs`.
- **The regression check runs both builds on one machine, turn about,** each with its own `ACS_HOME`.
  A baseline from another run would compare two runners; one file of numbers would compare runners
  and go stale. A failure has to happen twice, because shared runners have bad minutes.
- **In CI, Linux is the gate; macOS and Windows report only,** until the budgets below are settled.
- **`real/three_sessions.sh` is the release check for real agents.** `real/check.mjs` judges it from
  Claude Code's logs, not ACS's own records. It compares added lines: an edit's text repeats its
  neighbours, and a later session may rightly insert between them, which first looked like an
  overwrite. A run needs the ACS tools withheld (`ACS_REAL_NO_MCP=1`) to hit the hooks at all: agents
  that can read `acs_status` plan around each other.

## Windows measurements (open)

Release build, Windows 11, Avast running, repositories under `AppData\Local\Temp`:

| Path | Budget p50 / p99 | Measured p50 / p99 |
| --- | --- | --- |
| Pre-tool edit, warm | 15 / 40 ms | 57.2 / 156.4 ms |
| Post-tool shell, 100k files | 60 / 200 ms | 173.4 / 268.7 ms |
| `acs status`, 50 claims and 50 collisions | 50 / 150 ms | 101.5 / 181.6 ms |

Process start alone is over the first budget here: `acs --version` p50 18 ms, `whoami` 22 ms, `git
--version` 33 ms. One warm pre-tool call of 49 ms split into 18 ms outside `main`, 13 ms closing the
two SQLite databases, 6 ms updating the seat, 7 ms taking pre-images and about 5 ms for the rest.

An experiment with `SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE` on both connections brought that call to 35 ms.
It was reverted, because it was run with one process at a time and never against the test suite.

This is plan section 9.2's risk that process-per-hook startup misses the budgets on Windows. Its
fallback, an optional warm server, is the human's decision and was not built. How much of the cost
is Avast and how much is this machine is not known. The Linux figures date from Phase 2, so the
first CI run of the perf job is also the first check of this code on Linux.

## Consequences

- A slow hook does not change a decision: both real runs behaved correctly at these timings. The
  budgets are about the agent's waiting time, so this does not block the zero-overwrite gate.
- `acs status --json` and the event `detail` of a collision carry the holder's email, which the
  claims already expose. Records must not copy it further than they copy claims (WP4.1).
- A collision older than 24 hours leaves `acs status`; `acs stats` (WP4.5) is its long-term home.
