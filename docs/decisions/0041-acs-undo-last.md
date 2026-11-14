# 0041 — `acs undo --last`

Date: 2026-11-14. Status: accepted. Decided by: Claude, on the human's request to build issue 64
(the issue set the flag, the dry run that names the session, `--apply` with it, and stopping when
the most recent session is still running).

## Context

`acs undo <session>` needs an id copied from `acs log`. The common case is "undo what that last
agent session did". Which session counts as "the last" is not obvious from the issue: a session
that only read files, a person's own seat and a session that is still open all exist in the
state database next to the one the person means.

## Decision

1. **Which session.** `--last` picks, among this repository's seats on this machine, the agent
   seat that changed files (it has at least one attribution), most recent first by when it ended,
   or, for a seat that never reported its end, when it was last seen
   (`StateDb::seats_that_changed_files`). Sessions that changed nothing are skipped, because
   undoing one would do nothing; seats of the `human` pseudo-harness are skipped, because they
   are a person's own claims. Seats are not filtered by work tree, as `acs undo <id>` is not.
2. **A running session stops it, dry run included.** When the picked seat is live (not ended and
   seen within `claim_ttl_secs`, the rule `--apply` already used), `acs undo --last` exits 1 with
   `session_live`, names the session, its harness and intent and when it was seen, and names the
   most recent session that has ended, with its `acs undo <id>` line. It does not fall back to an
   older session by itself: the person asked for "the last", and the last one may still write.
   A live session that has changed nothing does not stop it.
3. **No session to pick** exits 1 with `no_session`.
4. **Output.** The header says `Undo plan for the last session, <id>` (or `Undo of ...` with
   `--apply`), followed by the harness, pilot, `ended <age> ago (<UTC minute>)` and an
   `Intent:` line (`none given` when there is none). Undo by id shows the same two lines now,
   with `last seen` for a live seat. The dry run's last line names the id
   (`acs undo <id> --apply`), not `--last`, so a session that ends in between cannot take its
   place. `--json` adds `picked_by` (`last` or `id`), `intent`, `ended_at` and `last_seen_at`.
5. **Flags.** `SESSION` and `--last` are a required clap group: one of them, never both (exit 2
   with clap's usage error).
6. **Agents.** Self-protection already denied `acs undo` with `--apply` anywhere in its words, so
   `acs undo --last --apply` is denied and `acs undo --last` allowed, like the id forms; the
   protect tests and the adversarial suite now say so.

## Consequences

- A person who exits their agent and runs `acs undo --last` gets that session. One who keeps the
  agent open is told to end it first, or to name a session.
- A session that crashed (silent past the TTL without an end) counts from when it was last seen,
  so it can be picked.
