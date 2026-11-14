# 0035 — `acs hook` ends early when no switched-on feature needs the call

Date: 2026-11-14. Status: accepted. Decided by: Claude. The task from the orchestrating session
set which calls end early (items 1 and 2 as listed there) and that hook registration is not
touched; Claude decided the keep-alive and its five minutes (item 3), that sync counts after a
read (item 2, second row), and that session end still ends the seat (item 5). The human has not
seen these yet.

## Context

Decision 0033 made seven features switchable. A switch skipped work inside the hook, but the
hook still resolved the seat, wrote to the state database and recorded an event for every tool
call, also where nothing that is switched on used any of it. Issue 70 asks that a repository
with features off pays less; its comment recommends this early exit and recommends against
hook registration that follows the switches, which is still not decided.

The switches are part of the policy, so the exit can only come after the repository is opened
(state database opened, policy loaded from its cache).

## Decision

1. **A tool event that no switched-on feature needs ends early** (`hook::unneeded`, handled by
   `hook::skip`). No seat is created and no event is recorded.

   | Event | Ends early when | What needs it otherwise |
   | --- | --- | --- |
   | Before a read | `lore` is off | Lore is shown with the first read in its scope. A read is never denied and claims nothing. |
   | After a read | `claims` and `sync` are off | Claims refresh their heartbeat; sync checks whether it is due. |
   | After a write, a delete, a move or a shell command | `history`, `claims` and `sync` are off | History attributes the change; claims and sync as above. |

   Example: with `lore = false` and `claims = false`, an agent that reads `src/lib.rs` leaves
   nothing in the state database, before or after the read.
2. **Never skipped:** the event before a write or a shell command (ACS's protection of itself
   is not a feature), session start (the status message), and a call whose tool ACS does not
   know.
3. **A skipped event keeps a live session alive.** With `lore` and `claims` off and `history`
   on, a session that only reads would have nothing marking its seat as seen. After an hour
   `acs sync` would record it as crashed (`runtime::records::CRASH_AFTER`), and the MCP server
   would stop taking it for the most recently active session (`commands/mcp.rs`, `bind`). Both
   read `last_seen_at`; this was checked in the code and the test below fails without the
   fix. So a skipped event reads the seat and marks it as seen when it has been silent for
   five minutes (`hook::keep_alive`), not every time: a write to the state database costs most
   of what skipping saves (see the numbers). The seat's `last_seen_at` can therefore be up to
   five minutes behind for such a session, against an hour before it counts as crashed.
4. **The marker of a running command is still used up** after a skipped write or shell event,
   so `acs watch` and the next command never see a call that has ended as still running.
5. **Prompt submit creates no seat.** A session without a seat has nothing waiting for it. A
   live seat is kept alive as in item 3. Nothing in ACS produces notices yet, so today this
   event never has anything to deliver.
6. **Session end always ends the seat,** and records its event only where `claims`, `history`
   or `sync` is on. The task asked to skip session end entirely with those three off. That
   would leave every finished session live for ever, and `acs_status`, `acs_intent` and
   `acs_why` would then pick among finished sessions when the harness does not say which
   session is calling. Ending the seat is one write, once per session.

## Measured

`cargo bench -p acs-cli --bench hook_paths` with `ACS_PERF_FILES=2000`, on the human's Windows
machine, release build, process start included. "Before" is `main` at `9497dd8`, run in turns
with this branch through `ACS_PERF_BASE_BIN`. p50 / p99 in milliseconds, 200 calls each.

| Call | Before | After |
| --- | --- | --- |
| Before a read, `lore` and `claims` off | 31.6 / 57.7 | 19.6 / 26.6 |
| After a read, `lore` and `claims` off | 31.1 / 40.1 | 20.1 / 27.5 |
| After a shell command, `history` and `claims` off | 31.6 / 39.6 | 20.0 / 26.1 |
| Before a read, every switch at its default (not skipped) | 32.0 / 100.6 | 32.1 / 71.0 |
| After a read, every switch at its default (not skipped) | 31.1 / 97.6 | 31.5 / 77.5 |
| Process start alone (`acs --version`) | 14.4 / 17.0 | 14.5 / 17.2 |

- **Saved: about 12 ms of 32 per skipped call (a bit over a third).**
- **Left: about 20 ms, of which 14.5 ms is starting the process** and about 5 ms is finding
  the repository, opening the state database, loading the policy and reading the seat. The
  process start is the part only issue 70's other half can remove.
- The three paths with budgets did not change (pre-tool edit 48.8 against 48.4 ms p50,
  post-tool shell 179.5 against 182.4, status 68.9 against 68.0). They miss their budgets on
  Windows as before (issue 2).

## Consequences

- With every switch at its default nothing changes except prompt submit, which no longer
  creates a seat or writes on every call.
- With features off, the event log has no rows for skipped calls. Nothing reads them: `acs
  log`, `acs stats` and `acs_why` use denied and asked calls and calls with paths, which are
  never skipped.
- With features off, a session's seat is created by its first event that is not skipped. In
  Claude Code that is session start, as before. In a harness without session start it is the
  first write or shell command, which is what the MCP server already tells an agent that has
  no session yet. SPEC section 4 says "a seat is created on the first event of a session";
  the spec is frozen and was not changed. The orchestrator should decide whether that sentence
  needs "the first event that needs one".
- A new feature that needs an event `unneeded` skips must be added there. The test
  `a_read_is_recorded_only_where_a_feature_needs_it` lists the cases.
- `benches/hook_paths.rs` measures the calls above on every run, without budgets, so CI's perf
  job takes about a minute longer.
- Tests: `crates/acs-cli/tests/features.rs`, six new ones. Four fail without the exit and
  `a_session_whose_events_are_skipped_stays_alive` fails without the keep-alive.

## Not done

- **The process still starts for every tool call.** Issue 70's "done when" asks that it does
  not; that needs a change to hook registration, which is not decided. Issue 70 stays open.
- **The state database is still opened for a skipped call.** Opening it only when needed
  could save part of the 5 ms that is left. Not tried.
- **No real-agent run.** The tests send events to the built binary; no Claude Code or
  Antigravity session was run.
