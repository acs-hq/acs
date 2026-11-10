# 0030 Session records, log, blame, undo, verify and stats (Phase 4)

Date: 2026-11-10. Work packages WP4.1–WP4.5.

## Context

Phase 4 turns what the hooks store into things a person can use: a ledger record per session
(`acs log`), line attribution (`acs blame`), recovery (`acs undo`), tamper checks (`acs verify`)
and counts (`acs stats`). The hook budget rules out git work at session end (harnesses give that
hook about a second), records are pushed to the remote, and some harnesses send no session end at
all. Three Sonnet subagents built it in parallel worktrees after the orchestrator froze the shared
surface.

## Decision

**Contracts (orchestrator, before fan-out).**

1. The five commands are registered in `cli.rs` with their argument structs in their own files.
   Agents may run `log`, `blame`, `stats`, `verify` and `undo` without `--apply` (`READ_ONLY` in
   `acs-core/src/engine/protect.rs`); `undo --apply` rewrites files and makes commits, so it is the
   pilot's, and `undo` is also in `STATEFUL` so `xargs acs undo --apply` is caught.
2. `SyncStorage::ledger_pilots` lists every pilot with a ledger here, written or fetched.
   `runtime::records::ledger_records` is the only way commands read records: every pilot's ledger,
   the record appended last per `session_id` wins, invalid entries are skipped and listed.

**Records (WP4.1).**

3. **Records are written by `acs sync`, never by the hook.** Sync's first step, under the sync
   lock, appends every due record ("finalize"), also with no remote. The session-end hook only
   starts a detached `acs sync --wait`, and now does so in any repository with `.acs/policy/`,
   remote or not. `acs log` and `acs stats` finalize first when the lock is free, so a crashed
   session gets its record on the next `acs` run. A record that cannot be written is a warning in
   sync's report, never a failed sync: claims must still reach other machines.
4. **Due = ended, or silent for an hour.** Migration 0007 adds `session_records` (one row per seat:
   the `last_seen_at` it was recorded at, the ledger commit, whether signed). A seat is due when it
   has ended or has been silent for `CRASH_AFTER` (1 hour, `ACS_CRASH_AFTER_SECS` overrides it for
   tests), and has no row or has been seen since its row was written. A crashed seat is not ended;
   its claims lapse by TTL. A seat that comes back gets a second record with the same `session_id`
   (SPEC section 4.3: readers take the last one).
5. **Nothing to record, no record.** Human seats never get records. A seat with no intent, claims,
   changed files, deny or ask decisions, overrides, lore or commits gets a table row without a
   ledger commit, so read-only sessions do not fill the ledger and are not revisited.
6. **Privacy.** `seat.worktree` holds the folder name only (absolute paths carry user names).
   Intent and every `path_or_command` are redacted with the hook's secret rules
   (`hook::redact::redact_text`); intent is capped at 500 characters, notable decisions at 50
   entries of 200 characters. A claim conflict's notable lists paths, not the message naming the
   holder.
7. **Commits** come from one `git log --all --grep` since the earliest due seat's start minus an
   hour, each hit confirmed with `trailers::session_ids`; if git fails the record is written
   without them.
8. **Other pilots' ledgers are pushed too.** A seat started under another git identity writes to
   that pilot's ledger; `acs sync` reconciles and pushes those after the normal run.
9. **`acs log`** filters by pilot (substring of the email, or the pilot id), harness, `--since`
   (on `ended_at`), path (changed or claimed), newest first, `--limit` 20. It shows the overrides a
   session used, which completes WP7.1's acceptance ("every override appears in `acs log` and in
   the session record").

**Blame (WP4.2).**

10. `git blame --porcelain` on the work-tree file, all commit messages in one `git log --no-walk`.
    A committed line's sessions are its `ACS-Session` trailers, resolved through the ledger, then
    the local seat with that session id (`StateDb::seat_by_session_id`, "no ledger record yet"),
    then "unknown". Uncommitted lines are given to the seat that last changed the path
    (`path_attributions`); a person's later edit of the same file cannot be told apart. Lines
    without trailers show "no session". Renames are not followed.

**Undo (WP4.3).**

11. **Local only.** Undo needs this machine's snapshots. `<session>` is a `ses_` id or a unique
    prefix.
12. **Three states per file**: before the session (its first pre-image), after it (its last
    post-image), and now. In order: held by another live seat → left alone; no pre-image →
    unknown, left alone; the session left it as it found it, or it is already back → already
    undone; as the session left it → restore, delete (the session created it) or recreate (it
    deleted it); otherwise `git merge-file -p current post pre`, and a conflict leaves the file
    alone. A file tool takes no pre-image of a file that does not exist yet, so a path whose first
    attribution is a tool write without `pre_hash` counts as created; without this, create-then-
    edit would restore the intermediate version.
13. **A live seat is not undone.** `--apply` refuses a seat with no end that was seen within
    `claim_ttl_secs`; the dry run works with a warning. Apply re-hashes each file just before
    writing and skips one that changed since the plan.
14. **History is never rewritten.** Restored paths whose session changes were committed (a commit
    with the trailer on `HEAD`'s history, and the file equal to `HEAD` before the restore) go into
    one new commit, `Undo session ses_x`, made with `git commit --only` so other staged work stays
    staged. User hooks run; if one rejects the commit the files stay restored and the exit code is
    1. Exit 1 also for any conflict or failed write; unknown and held files are reported but are
    not failures.

**Verify (WP4.4).** SPEC section 8.1 has the rules.

15. Signature states come from git's `%G?` together with whether the commit has a `gpgsig`
    header, because git prints `N` both for unsigned commits and for SSH-signed ones when no
    allowed-signers file is set. `G` and `U` are good (`U` is every GPG signature without a trust
    database); `B`, `X`, `Y`, `R` fail; `E`, or `N` with a header, are "unchecked". Unsigned and
    unchecked fail only with `--require-signed`.
16. **Remembered tips** live in `cache/verified-ledgers.json` (`local/<id>` and `fetched/<id>`),
    written only for chains with no problems. `--accept-history` re-remembers the present tips
    after a deliberate rewrite. `signing::ledger_tips` builds the fetched ref name itself (there is
    no tip accessor on `SyncStorage`); a change to the fetch layout must change both.

**Stats (WP4.5).**

17. Counts come from ledger records, team-wide, per pilot and in total: collisions prevented
    (`claim_conflict`), destructive actions blocked (`banned_command`, `protected_branch`,
    `delete_outside_claims`) and asked (`destructive_budget`, `destructive_unknown_targets`),
    sealed-path denials, other denials (`owned_territory`, `path_escapes_repo`), lore served,
    sessions, files changed. Time waiting on claims is not in records, so it is computed from this
    machine's events (from the first claim-conflict denial until the seat next changed or was
    allowed to write one of those paths, or ended) and labelled "this machine". Queued edits wait
    for Phase 8.

## Consequences

- The state schema is 7: **reinstall `acs` after merging**; older builds then report the state
  unavailable.
- The first sync after the reinstall writes records for every past session in the local state that
  has something to record, and the next sync with a remote pushes them to
  `refs/acs/ledger/<pilot-id>`.
- `FileChange` still cannot tell "pre-hash unknown" from "did not exist" (HANDOFF section 4); the
  first attribution's `pre_hash` is used as is. Undo does not depend on it (it reads snapshots).
- A seat resumed after its record and then quiet for under an hour is not re-recorded until it
  ends or crashes.
- An unsigned ledger rewritten before this machine ever verified it cannot be detected; signing
  closes that.
- Undo cannot help with files over the 32 MB snapshot limit, shell-changed untracked files that
  existed before (no baseline), or overlapping later edits (conflict, left alone).
- Dogfooding found that commits from a subagent's worktree are rejected for the agent's own claims
  (`docs/security/overrides.md`), open.
