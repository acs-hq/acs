# 0039 — `acs uninstall`

Date: 2026-11-14. Status: accepted. Decided by: Claude, on the human's request to build issue 63
(the issue set the scope: what is always removed, what is asked about, `--json`, a dry run, and
byte-identical results).

## Context

Removing ACS took nine manual steps (`docs/releases/manual-phase6-7.md`, section 0). People are
more willing to try a tool that installs git hooks when leaving is one command.
`acs_git::hooks::uninstall` existed but no command called it.

## Decision

1. **Always removed** (unless `--dry-run`): the ACS block in each git hook (a hook ACS wrapped
   moves back from `<hook>.acs-orig`), the `refs/acs/*` fetch refspec of every remote, the local
   `refs/acs/*` refs and their `refs/remotes/<remote>/acs/*` copies (and the folders git leaves
   empty), the two ACS entries in `.claude/settings.json` and `.claude/settings.local.json`
   (`enabledPlugins."acs@acs-plugins"`, `extraKnownMarketplaces."acs-plugins"`), and the
   Antigravity plugin folder `.agents/plugins/acs/`. `acs init` puts all of it back.
2. **Removed only when asked**, each on its own: `--policy` (`.acs/policy/`), `--state` (this
   repository's files in ACS's home folder: its state directory, the directory of its
   path-derived id if one is left, and its identity-cache entry), `--remote-refs` (the
   `refs/acs/*` refs on every remote). `--all` means all three. In a terminal, without the flag,
   `acs uninstall` shows the plan first and asks about each one that has something to remove.
   With `--json` or without a terminal it asks nothing and keeps them.
3. **The always-removed part is not asked about.** It is what the command is for, and `acs init`
   undoes it; the three questions cover what cannot be put back.
4. **Settings files are edited, not rewritten.** The file is read as a list of entries with each
   value's original text, the two entries are dropped, and the rest is written back in the same
   order and layout. An object left empty is dropped; a file left empty is deleted. This needs
   `serde_json`'s `raw_value` feature. The `claude` CLI is not used, because it is not always
   installed and `claude plugin marketplace remove` acts beyond the project.
5. **Refs on a remote are listed with `git ls-remote` only when they are to go** (or in a dry run
   with `--remote-refs`), because listing needs the network. They are deleted with one
   `git push --no-verify <remote> --delete ...`: deleting ACS's own refs is not the push a
   person's pre-push hook (tests, linters) is meant for. `GIT_TERMINAL_PROMPT=0` makes a remote
   that wants a password fail instead of waiting.
6. **Looking leaves no trace.** Finding the state resolves the repository id, which may write an
   identity-cache entry; when that entry did not exist before, it is deleted again and not
   reported.
7. **Agents may not run it**, dry run included: it is not in self-protection's read-only list,
   and `uninstall` is in `STATEFUL`, so `xargs acs uninstall` is denied too.

## Consequences

- `acs init` then `acs uninstall --all` leaves the work tree, `.git` (config, hooks, refs) and
  ACS's home folder as they were, in a plain repository, with husky 9 and with a
  `core.hooksPath` folder (`crates/acs-cli/tests/cli_uninstall.rs`).
- **One case is not byte-identical:** a hook that held only `#!/bin/sh` and a newline before
  `acs init` cannot be told apart from a hook ACS created, so it is deleted. Git skips a missing
  hook and runs an empty one to the same effect.
- The state directory is shared by every clone of the repository on this machine (its id comes
  from the root commits), so `--state` removes it for all of them. The question says so.
- A running Claude Code session, `acs watch` or the editor extension can hold the state
  database open on Windows; deleting it then fails and the output says to close them and run
  `acs uninstall --state` again.
- A user-scope Claude Code plugin is not touched (it gates every repository); the output gives
  the `claude plugin uninstall` command when the user settings enable it.
