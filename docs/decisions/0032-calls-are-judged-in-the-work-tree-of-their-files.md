# 0032 — A call is judged in the work tree its files are in

Date: 2026-11-13. Status: accepted. Decided by: Claude, fixing three bugs from the dogfooding table
in `docs/security/overrides.md` (rows of 2026-11-10) at the human's request.

## Context

A harness reports the directory its session runs in, not the directory a call acts on. Phase 4's
subagents ran in the parent's Claude Code session (cwd: the main checkout) and worked in linked
worktrees under `.claude/worktrees/`. Two things went wrong:

- An edit of `.claude/worktrees/wp4-undo/crates/acs-git/src/merge.rs` was claimed under that
  path. Nobody else calls the file that, so the claim protected nothing, `acs_release
  crates/...` found no claim, and the default seals (`.github/workflows/**`, `.acs/**`) did not
  match inside the worktree.
- `cd <worktree> && git commit` left its pending-commit marker under the main checkout's key. The
  hook in the worktree found none, took the commit for a person's, rejected it for the session's
  own claims, and advised `git commit --no-verify`, which agents are denied.

The reverse was broken too: a session started in a worktree that edited a file of the main
checkout was not checked at all (the path counted as outside the repository).

## Decision

1. **One work tree per hook event.** `runtime::roots::for_event` picks it: the work tree of this
   repository that holds everything the call writes, deletes or commits (file tools: the event's
   paths; shell: the write and delete targets, and the directory of each `git commit`-like or
   `git push` command). The whole pipeline (resolver, policy, claims, snapshots, post-tool scan)
   then runs there, so the file is `crates/acs-git/src/merge.rs` everywhere. If the places
   disagree, or a commit's directory is unknown, the session's work tree is kept, as before.
2. **Only work trees of the same repository** (same common git directory). A submodule, or another
   repository nested in the tree, is left as it was.
3. **A worktree's root named directly belongs to the checkout around it.** `rm -rf
   .claude/worktrees/x` is an operation on the main checkout; otherwise other sessions' claims on
   `src/...` would stop the removal of a stale worktree.
4. **The seat keeps the work tree the session runs in** (`RepoContext::session_tree`). The MCP
   server finds its seat by that, and the git hook's rule for a person's commit compares it.
   The seat's branch, used for protected branches, is read from the work tree being edited.
5. **Commit and push markers are keyed by the work tree git runs in.** `GitInvocation` gained
   `dir` and `dir_unknown` (a contract change in `acs-core`; not part of any JSON Schema), which
   `acs-shell` fills from `cd` and `git -C`. One command line can leave markers in several work
   trees.
6. **`acs claim`, `acs release` and the MCP tools split their paths by work tree**
   (`roots::split_by_tree`). Work trees are handled one after the other, so a refusal in a later
   one leaves the earlier ones claimed, and the refusal says so.
7. **A marker-less commit from an agent's shell gets its own advice.** When the harness
   environment (`CLAUDECODE=1`) shows an agent but no marker identifies the session, the rejection
   points at `acs_release` and naming the directory, not at `--no-verify` (issue 59).
8. **Git Bash drive paths.** On Windows `/c/Users/...` in a bash command is read as `C:/Users/...`.
   Without this, an absolute `cd` in Git Bash hid the worktree again.

Rejected: mapping each path to its own work tree inside one context. Snapshots, expansions and the
post-tool scan all turn a repository path back into a file under one root, so that would have
touched every stage for the sake of commands that write in two work trees at once. Also rejected:
letting the hook accept the newest marker of any work tree when `CLAUDECODE=1`; it widens the
stale-marker problem of issue 38.

## Consequences

- Cost: one directory lookup per event in a repository without linked worktrees; with them, a
  second parse of a shell command and a `.git` lookup per distinct directory.
- Claims are per file and target branch, not per work tree, as before. Two sessions in two
  worktrees of one repository now block each other on the same file, which is the rule the spec
  states and the bug hid.
- Not covered, each in issue 69: a command that writes in two work trees
  at once; a commit whose directory the command line does not show (`cd "$X" && git commit`); a
  command with unknown targets run in another worktree (`cd <worktree> && cargo fmt`), whose
  changes the post-tool scan of the session's work tree does not see; and a person committing in a
  linked worktree files that an agent session started elsewhere has claimed.
- Claims made before this change under `.claude/worktrees/...` paths stay until they expire.
- Evidence: `crates/acs-cli/tests/worktrees.rs`, four new cases in `tests/git_hooks.rs` (they
  fail without the change), `crates/acs-shell/tests/api.rs`, and a real Claude Code run on Sonnet
  with `tests/e2e/real/worktree.sh` (2026-11-13, 7 of 7 checks). That run commits with `git -C`,
  because Claude Code itself asks for approval of `cd <dir> && git ...` in a headless session.
- The hooks run the installed `acs`: the human reinstalls after merging
  (`cargo install --path crates/acs-cli --locked`).
