# 0045 — Write-paths follow-up: four fixes after the audit

Date: 2026-11-16. Status: accepted.

Decided by: the human for items 2 and 4 (they took the recommended option of issues 76 and 78);
Claude for items 1 and 3, on the human's request to fix the small open findings of
`docs/security/write-paths.md`. W2 (issue 74) and W9 (issue 38) stay open by the human's choice.

## Context

Decision 0044 fixed five findings of the write-paths audit and left W2, W4, W6 and W9 open, plus
an unchecked concern about links in undo (issue 77) and a question about what team sync tells a
pilot (issue 78).

## Decision

1. **A failed undo commit leaves the index as it was (W4, issue 75).**
   `acs_git::merge::commit_only` saves the index entries of its paths (`git ls-files -s -z`)
   before `git add`, and puts them back (`git update-index --force-remove`, then
   `--index-info`) when `git commit` fails. The files stay restored in the work tree, as decision
   0030 says, but nothing is staged that was not staged before. If the index cannot be put back,
   the failure text says the files are still staged.
2. **Hook removal takes away the newline install added (W6, issue 76).** When the line before
   the block has no newline, install adds one and writes the block's version as `v1+nl`; removal
   drops that newline again. Blocks without the mark stay valid and are not rewritten. The one
   exception of decision 0039 (a hook holding only `#!/bin/sh` is deleted) is unchanged.
3. **Undo does not write through links (issue 77).** A path whose file, or any folder on the way
   to it, is a symbolic link or a junction is reported as a conflict and left alone, at planning
   and again just before the write. Sessions record files under their real paths, so a link there
   appeared after the session or the change went through it unseen; writing would replace the link
   with a regular file or land where the link points, outside the repository included. Cost: a
   file reached only through a link is not undone by `acs undo`; the output names the link.
4. **Each pilot is told once what team sync publishes (issue 78, option 1).** One text
   (`runtime::sync::publishes_text`: the email, claimed paths, session records, that anyone who
   can fetch can read them, and how to switch it off) is printed by `acs init` and
   `acs features sync on` where sync is on, and once per repository and machine by the first
   `acs sync` a person runs or, failing that, handed to the agent of the first session to pass on
   (`publishes_text_once`, state flag `sync_publishes_told`). Nothing about what is published or
   when changes: the first sync is not held back for the notice. Option 2 (sync stays off for a
   pilot until they confirm) was not taken; the human may revisit it if testers are surprised.

## Consequences

- Tests: `w4_a_rejected_undo_commit_leaves_the_index_as_it_was` and
  `w6_removing_the_block_restores_a_hook_without_a_final_newline` now pass;
  `undo_never_writes_through_a_folder_that_became_a_link` (runs on Windows with a junction),
  `undo_never_replaces_a_symlink_with_a_file` (Unix only, first run is CI) and
  `a_pilot_is_told_once_what_sync_publishes` are new.
- Agent-facing text changes with a switch (decision 0034's rule): the notice is said by `acs`,
  only where sync is on and the repository has a remote.
- Reinstall `acs` after merging; no schema change.
