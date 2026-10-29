# 0009 — Decision engine

Date: 2026-10-29. Status: accepted (WP1.5).

## Context

`acs_core::engine::decide(&GateInput, &dyn PolicyView) -> GateOutput` (WP1.5) applies steps 3–7 of
the pre-tool pipeline (spec section 9) to inputs that decision 0004 defines. Plan and spec leave
some corners open; this record lists how the engine closes them.

## Decisions

1. **Order within the guards.** The first denying step supplies the reason code. Denies are ranked
   `sealed_path`, `banned_command`, `protected_branch`, `delete_outside_claims` (or
   `state_unavailable` for destructive actions), then `claim_conflict` from the claims step. Asks
   are ranked `path_escapes_repo`, `destructive_budget`, `destructive_unknown_targets`. A delete of
   a file another seat claims is therefore reported as `delete_outside_claims`, not
   `claim_conflict`.
2. **Protected branches.** A `push` (and `branch`, `update-ref`) is checked against its explicit
   refs, normalized from refspecs (`+src:refs/heads/dst` is `dst`), or against the seat's current
   branch when it names none. Every other rewrite (`reset`, `rebase`, `commit --amend`, ...) is
   checked against the seat's current branch only: its refs are what it rewrites *to*. Checking
   them would deny `git rebase main` on a feature branch and miss `git reset --hard origin/main`
   on `main` (`origin/main` does not match the glob `main`).
3. **Directories.** A recursive target with an expansion contributes its files to guards, claims
   and delete counts, and its directory to the sealed check only (the directory path and a probe
   path one level below it, so `rm -rf .git` is sealed even if a glob engine does not match
   `.git/**` against `.git`). A recursive delete without an expansion is treated as unknown
   targets (`destructive_unknown_targets`): the pipeline is required to expand recursive targets.
   A delete whose expansion was truncated asks as well if no budget was exceeded.
4. **Links.** `linked_into_repo` paths are treated like in-repository targets with the target's
   operation, for writes as well as deletes (a write through a link reaches the repository too).
5. **Sealed zones** (`territory.toml`) are enforced exactly like sealed globs, including under
   `gate_off` and `state_unavailable`.
6. **Delete accounting.** Distinct repository files deleted by the event, counting link-reached and
   expansion files. "Outside claims" means: not covered by a file claim of the requesting seat,
   unexpired, on the seat's target branch, in the snapshot (a symbol claim does not cover deleting
   the file), and not in an `open` zone. Both limits are exceeded when the count is greater than
   the limit.
7. **`state_unavailable`** still enforces sealed paths, banned commands, protected branches and
   path escapes (none needs the state database). It skips claims and the outside-claims check. A
   destructive action is any delete target, any `delete`/`move`-kind tool, a destructive shell
   analysis or a history rewrite or force push. `gate_off` wins if both flags are set.
8. **Snapshots** are requested for every written or deleted repository path, including `open`
   zones, whether or not it exists; the pipeline skips missing paths. Recursive targets are
   requested once, recursively, instead of per file. Globs without a recursive flag are requested
   per file.
9. **Notices** (context) are delivered for `allow` and `ask`, never for `deny` (the adapter may not
   render context on a block, and the notice must not be lost) and never with `gate_off`. Notices
   already delivered, or for another seat, are skipped. Lore injection (Phase 6) goes at the
   marked seam in `collect_context`.
10. **Remote-stale warning** is emitted when the remote view is stale and the event needed claims
    checked, so reads and open-zone edits stay quiet.
11. **Messages** are one template per reason code in `engine/messages.rs`. Paths are in backticks;
    lists show the first three and then "and N more"; ages are "just now", "N min ago", "N h
    ago", "N d ago", measured from `holder_last_seen_at` or else the claim's `heartbeat_at`. A
    holder without a known harness is described as "another machine" (remote) or "this machine".
    No message suggests bypassing ACS. Tests assert the exact text.
12. **Other phases** return `allow`/`ok` with no requests and no context; `decide` does not look
    at them.

## Consequences

- With the default `max_files_deleted_outside_claims = 0`, an agent can delete only files it
  claimed earlier (by editing them first). That is the rule as specified; it will show up in
  practice and may deserve an `acs_claim` shortcut later.
- The engine relies on the pipeline for: expanding recursive and glob targets, skipping snapshots
  of missing paths, and canonical `RepoPath` case.
- Property tests (`tests/engine_props.rs`) cover the two safety claims; the table tests
  (`tests/engine_table.rs`) cover every reason code the engine returns in Phase 1.
