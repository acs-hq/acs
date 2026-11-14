# 0037 — Snapshot cleanup runs in `acs sync`, with a fixed 500 MiB budget

Date: 2026-11-14. Status: accepted. Decided by: Claude.

## Context

ACS keeps a copy of each file before and after a session changes it, so `acs undo` can restore
it. `SnapshotStore::gc` (unreferenced blobs first, then least recently used, never those of live
sessions) existed but nothing outside tests called it, so the store only grew. A test confirmed
it (`crates/acs-cli/tests/snapshot_cleanup.rs`): after sessions that edit files and an
`acs sync`, the store stayed at 4931 bytes under a 1-byte budget. Decision 0030 makes undo depend
on the copies of ended sessions, so any cleanup limits how far back undo reaches (issue 68).

## Decision

1. **Budget: 500 MiB per repository**, a constant (`runtime/cleanup.rs`), not a policy setting,
   because a setting would change the frozen policy types. `ACS_SNAPSHOT_BUDGET_BYTES`
   overrides it for tests, like `ACS_SCAN_MAX_ENTRIES`. A policy setting is a possible follow-up.
2. **Cleanup runs inside `acs sync`**, under the sync lock, right after session records are
   written and before the team-sync early returns, so it runs with `sync` off too. Hooks start
   `acs sync` detached (including at every session end with `history` on); cleanup never runs in
   a hook. Errors are ignored like the other best-effort steps and never fail the sync.
3. **Only with `history` on.** With it off nothing new is captured, so the store does not grow,
   and the copies already there keep working for undo.
4. **Live sessions** are seats that have not ended and were seen within the crash window
   (`ACS_CRASH_AFTER_SECS`, one hour). A crashed seat never ends, so counting it live would pin
   its copies forever.
5. **`acs doctor`** reports the store's size against the budget (`snapshots` check) and warns
   when over. It does not clean and has no `--fix` hook for this: `acs sync` is the one place.
6. **`acs undo`** says a file's copy "was removed to save space" when the store reports it
   evicted. A file the session created can still be deleted by undo after eviction, since that
   needs no copy.

## Consequences

- Undo reaches back as far as 500 MiB of copies allows, oldest ended sessions going first.
- A store can stay above budget while a long session runs; doctor warns about it.
- docs/HANDOFF.md section 3 stays true: no `drop_session` at session end, eviction by budget only.
