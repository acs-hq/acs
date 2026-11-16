# 0044 — Write-paths audit: five small fixes before first testers

Date: 2026-11-16. Status: accepted.

Decided by: Claude, on the human's brief to audit every place ACS writes to a person's work
tree, git history, git config, hooks or remote, and to fix only what is small and clearly wrong.
The findings, the tests and what is still open are in `docs/security/write-paths.md`.

## Context

First testers forgive a bypass, not lost work or an unexpected publication. The audit found nine
problems (W1 to W9) and reproduced each with a test in `crates/acs-cli/tests/write_paths.rs`.
Five had a fix small enough to make inside the audit.

## Decision

1. **Undo calls a file "created by the session" only from an absent pre-image (W1).** This
   amends decision 0030 item 12, which also counted a first tool attribution without a
   before-hash as a creation, and decision 0009 item 8, which skipped missing files when taking
   pre-images. The pre-tool step now records a requested file that does not exist yet as absent;
   without a pre-image a file is unknown and undo leaves it alone. The old rule could not tell a
   creation from a change whose pre-tool step never ran (the hook failed open or timed out, the
   state was busy, `history` was switched on in between), and undo then deleted the person's file.
   Cost: such a file is now reported as unknown instead of deleted, and sessions recorded before
   this change lose the creation rule (no tester has any).
2. **Undo asks git whether a file equals `HEAD` (W3)**, through `core.autocrlf` and clean filters
   (`git ls-tree`, then `git diff --quiet HEAD`), instead of comparing raw bytes. On Windows,
   restored files were otherwise never committed.
3. **A hook wrapper takes the original's executable state (W5).** A non-shell hook the person had
   switched off stays off, like a shell hook already did (decision 0015).
4. **Redaction finds known token prefixes after `:`, `=`, quotes, `@`, `(` and `,` inside a word,
   and knows more prefixes (W7)**: `sk_live_`, `sk_test_`, `rk_live_`, `rk_test_`, `whsec_`,
   `npm_`, `pypi-`, `hf_`, `shpat_`, `dop_v1_`, `glptt-`, `xoxs-`, `xapp-`. It stays a heuristic
   (decision 0030 item 6); a word starting with one of these is masked even when it is not a
   secret.
5. **`acs lore review --accept` refuses when `acs/lore-<id>` is checked out in any work tree
   (W8)**, instead of moving that branch under its index and files (decision 0029 item 6 says the
   checked-out branch is never touched).

## Consequences

- W2 (undo resolves paths in the wrong worktree), W4 (a rejected undo commit leaves files
  staged), W6 (a newline added to a hook) and W9 (a stale commit marker alters or rejects a
  person's commit) stay open; their tests fail on purpose until they are fixed.
- W5 is Unix only; its test does not run on Windows.
