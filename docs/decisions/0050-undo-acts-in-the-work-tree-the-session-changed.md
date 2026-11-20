# 0050 — Undo acts in the work tree the session changed

Date: 2026-11-20. Status: accepted.

Decided by: Claude, on issue 74's proposed fix: the storage (a column in both stores, `NULL` for
old rows), how a recorded work tree is checked before undo writes there, the output for files in
other work trees, and one undo commit per work tree. The human has not seen these yet.

## Context

Finding W2 of the write-paths audit (`docs/security/write-paths.md`). Since decision 0032 a
session started in the main checkout that edits `.claude/worktrees/wp/src/lib.rs` records it as
`src/lib.rs`, the worktree's own name for it. Neither the attributions (state database) nor the
snapshot entries (snapshot index) said which work tree a path belongs to, and `acs undo` resolved
every path in the work tree it ran in. Run from the main checkout, where the session ran and where
`acs undo --last` is natural, it read the main copy: "already undone", and the worktree kept the
agent's change. When the main copy equalled what the session left, undo "restored" a file the
session never touched.

## Decision

1. **Both stores record the work tree.** State migration 0008 adds `attributions.work_tree`;
   snapshot index migration 2 adds `entries.work_tree`. The value is the absolute root of the work
   tree the hook judged the event in (`roots::for_event`, canonical as `acs_git::discover` names
   it). Rows written before are `NULL` and belong to the work tree undo runs in, which is what
   undo did before, so old sessions undo as they did. A root that is not UTF-8 is stored as
   `NULL` too.
2. **The hook looks up images per work tree.** `pre_image_in` / `post_image_in` ignore entries of
   other work trees and count `NULL` ones, so a session that spans the upgrade keeps its first
   pre-images, and a session that changes `src/lib.rs` in two checkouts has two histories.
3. **Undo plans each file in its own work tree.** Files are grouped by (work tree, path); those of
   the work tree undo runs in come first. A recorded root other than the current one is used only
   when `discover` on it finds a work tree of this repository with exactly that root. Anything
   else (removed, a plain folder or another repository in its place) makes the file `unknown`,
   with the reason, and nothing is written for it, in that place or any other. This is the
   issue's "refuse `--apply` for paths whose work tree no longer exists"; like every `unknown`, it
   does not change the exit code.
4. **Output.** A file of another work tree is shown with its work tree in front, relative to the
   one undo runs in when it is inside it (`.claude/worktrees/wp/src/lib.rs`). JSON files gain
   `work_tree` (absolute) and `elsewhere`. Restored committed files are committed in their own
   work tree, one commit per work tree; JSON gains `commits` (each with its `work_tree`), and
   `commit` keeps its old shape: the first failed commit, else the first made.

## Consequences

- The W2 test passes, with two more: undo run inside the worktree, and undo after the worktree
  was removed (where the main copy holds what the session left, so the old code wrote it).
- **Reinstall `acs`** (`cargo install --path crates/acs-cli --locked`). The state moves to schema 8
  and older builds then report it unavailable. Sessions recorded before the reinstall carry no work
  tree: a worktree session from before still undoes only when `acs undo` runs inside that
  worktree.
- `acs blame` of uncommitted lines still reads attributions by path alone, so a change in a
  worktree can be named for the same path in the main checkout. Not part of W2; it is the same
  root cause as issue 69 (claims made relative to the wrong worktree).
