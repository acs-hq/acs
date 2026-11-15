# History and undo

ACS keeps a record of every agent session in your repository: who ran it, what the agent said it was doing, which files it held and changed, and what ACS stopped it from doing. From that record you can see who wrote a line, undo one session's changes without touching anyone else's, and check that nobody has tampered with the history.

## How it works, by example

Sara asks her agent to fix rounding in refunds. While it works, Omar's agent edits the same repository on another task.

1. **Sara's session ends.** ACS writes a record of it. With a git remote, the record is shared with the team on the next sync, like claims are.
2. **Next morning Sara runs `acs log`.** She sees her session: when it ran, that the agent said "Fix rounding in refund totals", the four files it changed, and that ACS once stopped it from editing a file Omar's agent held.
3. **A test fails in `payments/refund.py`.** `acs blame payments/refund.py` shows, line by line, which session last changed each line, whose agent it was and what it said it was doing. Lines a person wrote without an agent show "no session".
4. **Sara decides the session's changes were wrong.** It was the last agent session on her machine that changed files here, so she runs `acs undo --last` (for an older one she would give its id from `acs log`: `acs undo ses_4k2...`). It names the session it picked, `ses_4k2...`, with "Fix rounding in refund totals" and "ended 14h ago", so she can see it is the one she means, and shows what it would do: put back three files as they were, delete one file the agent created. Omar's agent changed one of those files afterwards, in another place, so that file is merged: Sara's agent's change is taken out and Omar's stays. Nothing is touched yet.
5. **She runs `acs undo ses_4k2... --apply`,** the line the dry run printed (`acs undo --last --apply` works too). The files are restored. Two of them had already been committed, so ACS makes one new commit, "Undo session ses_4k2...", with just those files. It never rewrites history.

What `acs undo --last` prints:

```text
Undo plan for the last session, ses_4k2rmb7q3x...
  claude-code sara@example.com, ended 14h ago (2026-11-13 18:02Z)
  Intent: Fix rounding in refund totals

  payments/refund.py                                                     restore
  payments/tests/test_rounding.py                                         delete
  payments/totals.py                                                       merge
  payments/util/money.py                                                 restore

Summary: 2 to be restored, 1 to be deleted, 1 to be merged.
This is a dry run. Run `acs undo ses_4k2rmb7q3x... --apply` to carry it out.
```

## Commands

| Command | What it does |
| --- | --- |
| `acs log` | Lists past sessions, newest first. Filter with `--pilot`, `--harness`, `--since 2d`, `--path 'payments/**'` and `--limit`. |
| `acs blame <file>` | Shows which session last changed each line, and what its agent said it was doing. |
| `acs undo <session>` | Shows what undoing a session would do. Add `--apply` to do it. |
| `acs undo --last` | The same for the last agent session on this machine that changed files in this repository. Add `--apply` to do it. |
| `acs verify` | Checks that the session records are complete and unaltered, and that signatures are valid. |
| `acs stats` | Counts what ACS prevented: collisions, dangerous commands, writes to protected paths. |

Every command takes `--json`. Agents may run all of them except `acs undo --apply`, with an id or with `--last`, which is yours.

## When records are written

- A session is recorded when it ends. Claude Code tells ACS when a session ends.
- A session that never says it ended (the terminal was closed, the machine slept) is recorded as crashed once it has been quiet for an hour.
- A session that only read files, was never stopped and was shown no lore is not recorded.
- Records contain file paths, content hashes, counts, the agent's own description of its task and your git email. They never contain prompts or file contents, and only the name of your working folder, not its full path.

## Undo, in detail

For each file the session changed, ACS compares three versions: before the session, after the session, and now.

- **Unchanged since the session:** the file goes back to how it was before. A file the session created is deleted, and one it deleted comes back.
- **Changed by someone since:** the session's change is taken out and the later changes are kept. If they overlap, the file is left alone and reported as a conflict.
- **Held by another session right now:** left alone. Run undo again once it is released.
- **No copy from before the session:** left alone and reported. This happens for files over 32 MB, for files a shell command changed that git did not track, and when ACS's check before the agent's edit did not run (for example it timed out). ACS never deletes a file it has no copy of.

- **The copy was removed to save space:** left alone and reported. ACS keeps copies of up to 500 MiB per repository. Past that, `acs sync` (which hooks start in the background, including when a session ends) removes the copies of the oldest ended sessions first; those of a running session are never removed. `acs doctor` shows how much is used.

Undo works only on the machine where the session ran, because that is where the copies are kept. It refuses to apply while the session is still running.

### Which session `--last` picks

`--last` picks the agent session on this machine that most recently changed files in this repository, by when it ended. Sessions that changed no files are skipped, and so is the seat that holds your own claims (`acs claim`, `acs watch`, the VS Code extension): `--last` only picks agent sessions. It prints the session's id, what its agent said it was doing and when it ended, so you can check it is the one you mean before you apply.

If that session may still be running (it has not ended and was active within the claim time-to-live, 15 minutes by default), `--last` says so and stops, for the dry run too, and names the most recent session that has ended. An agent that is still open can keep changing those files, so end it first (exit the agent), or name a session by its id. The dry run tells you to apply by the id rather than with `--last` again, so a session that ends in between cannot take its place.

## Signing

If your git signs commits (`commit.gpgsign = true`), ACS signs its records the same way, with no extra setup. `acs verify` checks those signatures against your git setup: for SSH keys, `gpg.ssh.allowedSignersFile` must list your teammates' keys. Without signing, `acs verify` still notices records that were changed or removed after it last checked them on your machine. Use `acs verify --require-signed` to treat unsigned records as a failure.
