# Write paths: where ACS changes your repository

Audit of 2026-11-16, before first testers. Testers forgive a bypass; they do not forgive ACS
losing their work or publishing something they did not expect. This page lists every place where
ACS itself (not an agent) writes to a person's work tree, git history, git config, hooks or
remote, what was checked, and what was found.

Every finding has a test in `crates/acs-cli/tests/write_paths.rs`, named after it (`w1_...`).
Tests of open findings fail on purpose. Fixes made during the audit are in decision 0044; W4,
W6, the link check (issue 77) and the sync notice (issue 78) were solved the same day in
decision 0045. Open now: W2 and W9.

## Summary

Severity: **data loss** (content the person cannot get back), **publication** (something leaves
the machine that the person did not expect), **commit** (a person's commit is rejected or
altered by mistake), **surprising** (harmless, but not what the person expects).

| Id | Surface | What happens | Severity | State |
| --- | --- | --- | --- | --- |
| W1 | `acs undo --apply` | A file that existed before the session, of which ACS took no pre-image, is taken for a file the session created and **deleted**, untracked content included | data loss | Fixed (decision 0044) |
| W2 | `acs undo --apply` | A session that edited files in a linked worktree is undone in the work tree `acs undo` runs in: it reports "already undone", or merges into the wrong copy, and the worktree keeps the change | surprising (wrong target) | Open (issue 74) |
| W3 | `acs undo --apply` | With `core.autocrlf` (the Git for Windows default) committed session changes are restored but never committed, and the output says nothing | surprising | Fixed (decision 0044) |
| W4 | `acs undo --apply` | When a hook of the person rejects the undo commit, the restored files are left staged | surprising | Solved (decision 0045, issue 75) |
| W5 | hook install | A non-shell hook the person had switched off (`chmod -x`) gets an executable wrapper whose `exec` fails: **every commit is rejected** (Unix) | commit | Fixed (decision 0044) |
| W6 | hook install and removal | A hook whose last line has no newline gets one, and keeps it after removal | surprising | Solved (decision 0045, issue 76) |
| W7 | `acs sync` (session records) | Redaction of the intent misses Stripe keys (`sk_live_...`) and tokens after `user:` (`curl -u sara:ghp_...`), and the record is pushed | publication | Fixed (decision 0044); still a heuristic |
| W8 | `acs lore review --accept` | Accepting onto an `acs/lore-<id>` branch that is checked out moves that branch under its index and files | surprising | Fixed (decision 0044) |
| W9 | `pre-commit`, `prepare-commit-msg` | An agent command containing `git commit` that never ran (`cargo test && git commit ...` with failing tests) leaves a marker; the person's next commit within 10 minutes gets the agent's `ACS-Session` trailer, and a sealed path in it is rejected as an agent's | commit | Open (issue 38) |

Tests that fail on purpose today: `w2_undo_acts_on_the_worktree_the_session_changed`,
`w9_a_commit_the_agent_never_ran_does_not_mark_the_persons_commit`,
`w9_a_stale_marker_does_not_reject_a_persons_commit_of_a_sealed_path`.
`w5_a_disabled_non_shell_hook_stays_disabled_after_install` is Unix only and does not run on
Windows.

## 1. `acs undo --apply`

**What it writes:** the files of one session (restored, deleted, recreated or merged), one new
commit `Undo session ses_...` made with `git commit --only` for files the session had committed,
and the index entries of those files (`git add -A -- <paths>` before the commit).

**Checked and sound:** a file is re-hashed just before it is written and skipped if it changed
after the plan; files claimed by another live seat are left alone; a live session is refused;
overlapping later changes are a conflict and left alone; a directory in the way is a conflict;
history is never rewritten; other staged work stays staged and out of the commit (existing test
`committed_changes_are_undone_with_a_new_commit`); a rejected undo commit leaves the index as it was (W4); a path
that is a link, or lies behind one, is never written (issue 77); restored files get their recorded mode bits
on Unix (only the read-only flag on Windows); the three-way merge works on bytes, so line endings
inside a file are kept. Undo of an undo: running it again reports "already undone"; an undo is
not a session, so it is undone with git, not with `acs undo`. A file created and then edited is
deleted (`w1_a_file_the_session_created_and_then_edited_is_still_deleted`). Renames by a file
tool or a predictable shell command are a deletion plus a creation, each undone; an unpredictable
shell command that creates an untracked file gets a baseline only when the file was born after
the command started, otherwise it is unknown and left alone.

**W1 (data loss, fixed).** The planner took any first tool change without a before-hash for a
creation, and `--apply` deleted the file. That is also what a change looks like when the
pre-tool step took no image: the hook failed open or timed out (Claude Code then runs the tool
anyway), the state database was busy, or `history` was switched on between the two events. An
untracked file was then gone for good. Fix: the pre-tool step records a requested file that does
not exist yet as absent, and only that pre-image makes a file "created"; without one the file is
unknown. Test: `w1_undo_never_deletes_a_file_that_existed_before_the_session`; unit test
`a_tool_change_whose_pre_tool_step_never_ran_is_unknown_not_a_creation`.

**W2 (wrong target, open).** Since decision 0032 a session started in the main checkout that
edits a file in a linked worktree (`.claude/worktrees/wp/src/lib.rs`) records it as `src/lib.rs`,
the worktree's own name for it. Attributions and snapshots do not record which work tree a path
belongs to, and undo resolves every path against the work tree `acs undo` runs in. Run from the
main checkout (where the session ran, and where `--last` is natural), it reads the main copy of
`src/lib.rs`: "already undone", and the worktree keeps the agent's change. When the main copy
differs, the reverse change is merged into a file the session never touched (a clean merge
usually leaves it as it was; content equal to the session's result is restored or deleted). No
case was found where content that exists nowhere else is lost, but the person is told the undo
happened. Run from the worktree, it works. **Proposed fix:** store the work tree root with each
attribution and snapshot entry (a migration), plan each path in its own work tree, and refuse
`--apply` for paths whose work tree no longer exists. Until then, `acs undo` could warn when the
repository has linked worktrees. This is the dogfooding table's "paths claimed relative to the
wrong worktree" (row of 2026-11-10), reaching undo.

**W3 (surprising, fixed).** Whether a restored file can go into the undo commit was decided by
comparing raw work-tree bytes with the `HEAD` blob. On a CRLF checkout they never match, so on
Windows committed changes were restored in the work tree only, and the output did not mention a
commit. Fix: `acs_git::merge::worktree_matches_head` asks git (`ls-tree`, then
`diff --quiet HEAD`). Test: `w3_committed_changes_get_an_undo_commit_under_autocrlf`.

**W4 (surprising, solved).** `commit_only` runs `git add -A -- <paths>` before `git commit --only`
(needed for a file the session had deleted and committed). If a hook rejected the commit, the files
stayed restored, as decision 0030 says, but also staged, and the person's next plain `git commit`
took them along. Fix (decision 0045): the paths' index entries are saved before the add
(`git ls-files -s -z`) and put back (`git update-index --force-remove`, then `--index-info`) when
the commit fails. Test: `w4_a_rejected_undo_commit_leaves_the_index_as_it_was`.

**Links (issue 77, solved).** `read_file` follows a symlink, and the write renames a temporary
file over the path: undoing a change made through a symlinked file would have replaced the link
with a regular file, and a path through a folder that became a junction or symlink after the
session was written through it, outside the repository included (reproduced on Windows with a
junction). Fix (decision 0045): a path whose file or any folder on the way is a link is a conflict
and left alone, at planning and again before the write; the output names the link. Tests:
`undo_never_writes_through_a_folder_that_became_a_link`, and
`undo_never_replaces_a_symlink_with_a_file` (Unix only; it has not run yet, CI is its first run).

**Not reproduced, from reading the code:**

- **A staged-only version.** If the person staged a version of a committable file and then put
  the work-tree file back to `HEAD`, the `git add` replaces the staged version. Rare.
- **The undo commit's own trailer.** The person-commit trailer rule counts an attribution that is
  not older, to the second, than the last commit of the path. An undo commit made in the same
  second as the session's commit can get the undone session's `ACS-Session` trailer. Rare outside
  tests.

## 2. Git hook install and removal

**What it writes:** `pre-commit`, `prepare-commit-msg` and `pre-push` in the hooks directory
(`.git/hooks`, a `core.hooksPath` folder, or husky's `.husky/<hook>`), a block between marker
lines inserted after the shebang of a shell hook, or a wrapper with the original moved to
`<hook>.acs-orig`.

**Checked and sound:** a second install changes nothing; an outdated block is replaced in place;
removal cuts the block and restores a CRLF hook byte for byte
(`w6_removing_the_block_restores_a_crlf_hook`); a wrapped hook moves back; husky 9's generated
stubs are never edited; the pre-commit framework's script is treated as a shell script and keeps
working; a non-executable shell hook stays non-executable. `hooks::uninstall` **is** called now,
by `acs uninstall` (decision 0039).

**W5 (commit, fixed).** The wrapper for a non-shell hook was always made executable. A Python or
Node hook the person had switched off was then run by git, and the wrapper's `exec` of the
non-executable original failed with 126, rejecting every commit. Unix only (Git for Windows does
not look at the executable bit). Fix: the wrapper takes the original's executable state, also
when an outdated wrapper is updated. Test: `w5_a_disabled_non_shell_hook_stays_disabled_after_install`
(Unix).

**W6 (surprising, solved).** When the line before the block (usually the shebang) has no newline,
install adds one, and removal could not know it was added: `acs init` then `acs uninstall` left
`#!/usr/bin/env bash` plus a newline. Fix (decision 0045, the human's choice in issue 76): install
marks the block (`v1+nl` in the begin line) when it added the newline, and removal drops it again.
A hook that is only `#!/bin/sh` without a newline is still deleted, the exception decision 0039
states. Test: `w6_removing_the_block_restores_a_hook_without_a_final_newline`.

**Worth knowing:** with a global `core.hooksPath` (one hooks folder for every repository) ACS
installs into that folder, outside the repository (decision 0015). In repositories without
`.acs/policy` the block is silent, but `acs init` should say plainly that it edited a folder
shared by all repositories. With husky, ACS edits `.husky/<hook>`, which is committed and reaches
the team with the adoption commit.

## 3. `acs init`

**What it writes:** missing files in `.acs/policy/` (opened with `create_new`, so an existing
file is never overwritten, even one created during the run); `territory.toml` is replaced only
while it is still the untouched template, re-checked right before the write; the hooks (section
2); and, only when the repository switched team sync on, the fetch refspec
`+refs/acs/*:refs/remotes/<remote>/acs/*` on every remote (`git config --add`, skipped when
present). It commits nothing and switches no branch. A second run changes nothing. In a
repository with no commits it writes the same files; the repository id is then derived from the
path (decision 0003).

**Found, low:** the refspec puts ACS refs under `refs/remotes/<remote>/acs/`, the same place a
remote branch named `acs/...` goes. ACS's own branches (`acs/lore-<id>`) do not collide with
`acs/claims/` and `acs/ledger/`, but a branch named `acs/claims` or `acs/ledger` on the remote
would make every `git fetch` fail with a file/directory conflict. **Proposed fix:** fetch into
`refs/remotes/<remote>-acs/*` or `refs/acs-remote/<remote>/*`; not urgent. A remote added after
`acs features sync on` gets no refspec (only plain `git fetch` needs it; sync fetches with its own
refspec).

## 4. `acs sync`

**What it pushes:** to `origin` (else the first remote by name), `refs/acs/claims/<pilot-id>`
(forced; a commit without parents holding the pilot's claims) and `refs/acs/ledger/<pilot-id>`
(fast-forward only; one commit per session record), with `--no-verify --no-follow-tags`. Nothing
else: no branch, no tag, no other ref. Fetches use `--no-write-fetch-head`.

**Nothing is pushed before the repository adopted ACS:** team sync is off by default (decisions
0031 and 0033); with it off, `acs sync` and the hooks fetch and push nothing (they only write
local session records), and `acs init` adds no refspec. Hooks sync only where `.acs/policy/`
exists.

**What a record carries, and what is redacted:** the pilot's git email, harness, the harness's
session id, the work tree's folder name (not its path), branch and target branch, the intent
(redacted, at most 500 characters), claimed and changed paths with content hashes, counts of
decisions and up to 50 notable ones (text redacted), commit ids, lore ids, and overrides used
(pattern, mode, the email of who granted it, paths). The claims ref carries the email and the
claimed paths. **Not redacted:** branch names, the harness session id, override patterns and
paths. A repository that sets `sync = true` makes every pilot who works there with the plugin
publish these on hooks, with no question to that pilot; on a public repository or fork they are
readable by anyone who fetches `refs/acs/*`. That is the design (decision 0020). **Solved (issue 78, decision
0045, the human chose "tell each pilot once"):** `acs init` and `acs features sync on` print what
sync publishes and who can read it, and each pilot is told once per repository and machine, by
the first `acs sync` they run or by the agent of their first session. Nothing about what is
published changes. Test: `a_pilot_is_told_once_what_sync_publishes` (`tests/sync.rs`).

**W7 (publication, fixed).** See the summary. Redaction stays a heuristic: a secret with no known
shape (an AWS secret key, a password in prose) is not caught. Test:
`w7_the_record_that_is_pushed_carries_no_tokens_from_the_intent`.

## 5. `acs lore review --accept`

**What it writes:** blobs and a commit object, and the branch `acs/lore-<id>` with `update-ref`
(its old value given, so a concurrent change is refused). The temporary index lives in the git
directory and is removed afterwards. The work tree, the real index and `HEAD` are not touched
(existing test `review_accepts_onto_a_branch_and_leaves_the_work_tree_alone`). Commit hooks do not
run (`commit-tree`); signing follows the person's git configuration.

**W8 (surprising, fixed).** If the reviewer had `acs/lore-<id>` checked out (to edit the entry
before merging) and accepted a second proposal for the same entry, `update-ref` moved their
current branch: the index and files stayed on the old commit, so the new entry showed as a staged
reversal, and the next `git commit -a` would have undone it. Fix: accept refuses while the branch
is checked out in any work tree. Test: `w8_accepting_lore_never_moves_a_checked_out_branch`.

## 6. The `prepare-commit-msg` trailer and the pending-commit marker

**What it writes:** an `ACS-Session` trailer into the commit message file (never by running
git; comment lines, scissors and CRLF respected), and nothing else. The pre-commit hook writes
nothing but can reject the commit.

**W9 (commit, open; issue 38).** The pre-tool step leaves a pending-commit marker whenever an
allowed agent shell command contains `git commit`, and only a commit uses it up (or a rejected
agent commit). When the commit never runs, which is the common case for
`cargo test && git commit -m ...` with failing tests, the marker stays for ten minutes. The
person's next commit in that work tree is then taken for the agent's: it gets the agent's
`ACS-Session` trailer (the commit is altered and attributed to an agent in `acs log` and
`acs blame`), and sealed paths in it are rejected with "Sealed paths can only be changed by a
human ... ask your pilot to commit them", said to the pilot. Issue 38 names the rarer trigger
(another hook rejecting the agent's commit). **Proposed fix:** use up the commit and push markers
in the post-tool step of the shell command that left them: the command has finished, so any
commit it ran has already used its marker. A command that commits in the background (`&`, a
harness's background run) would then commit unmarked and be judged as a person's (sealed paths
not checked, the agent's own claims allowed); that trade-off is the human's call. Tests:
`w9_a_commit_the_agent_never_ran_does_not_mark_the_persons_commit`,
`w9_a_stale_marker_does_not_reject_a_persons_commit_of_a_sealed_path`.

## Not covered

- File symlinks in undo on Windows (they need developer mode; junctions are covered), and any
  undo test with real file modes on Windows beyond the read-only flag.
- `acs uninstall` was not re-audited beyond its hook removal (decision 0039 has its own
  byte-identical tests).
- Real husky and pre-commit installations: the layouts are fixtures written from their sources
  (decision 0015).
