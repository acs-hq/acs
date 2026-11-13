# 0033 — Feature switches

Date: 2026-11-13. Status: accepted. Decided by: the human for the idea, the seven switches and
their place in `config.toml` (their suggestion, taken as written); Claude for what each switch
covers, what can never be switched off, and the `acs features` command.

## Context

Until now a repository took ACS whole. `acs off` exists, but it is one machine's temporary switch
and it only stops claims. The human asked for every feature to be switchable on its own ("turn
off claims and leave territories on"), and suggested a `[features]` table in `config.toml`.
Decision 0031 had already agreed that team sync is off until a repository opts in, and left a
switch for the `ACS-Session` commit trailer open (issue 61).

## Decision

1. **The switches are a `[features]` table in `.acs/policy/config.toml`:** `claims`, `guards`,
   `territory`, `lore`, `history`, `commit_checks`, `sync`. Each is `true` or `false`. A missing
   key takes its default, so existing repositories need no change. An unknown key is an error,
   like everywhere else in the policy.
2. **Defaults: everything on except `sync`.** This builds decision 0031 item 4. A repository that
   syncs today stops syncing after the upgrade until someone sets `sync = true`.
3. **Why `config.toml`.** It is committed, so a switch is the same for everyone working in the
   repository, and it is sealed, so an agent cannot flip one. No per-machine override was added:
   `acs off` remains the local, temporary switch.
4. **What each switch covers.** One example: with `claims = false`, two agents may edit
   `src/lib.rs` at the same time and neither is blocked, while an agent is still denied in a zone
   owned by someone else.

   | Switch | Off means |
   | --- | --- |
   | `claims` | Nothing is claimed and no claim blocks an edit or a commit. The rule against deleting files outside one's claims does not apply. `acs claim` and `acs_claim` answer that claims are off; `acs watch` does not start. |
   | `guards` | No sealed paths other than ACS's own, no banned commands, no protected branches, no delete limits, no questions about writes ACS cannot place. `guards.toml` is still read and checked, so a mistake in it is reported. |
   | `territory` | No zones: every file is `shared`. A `sealed` zone is a territory rule and goes with it. `acs override` has nothing to do and says so. |
   | `lore` | No entry is shown to an agent and `acs_lore_propose` stores nothing. `acs lore` still lists and shows what is in the repository. |
   | `history` | No copies of files are kept for `acs undo`, no changes are attributed, no session records are written, and commits get no `ACS-Session` trailer. `acs log`, `acs blame`, `acs stats`, `acs verify` and `acs undo` still read what was recorded before and say that recording is off. |
   | `commit_checks` | The git hooks reject nothing: not a commit of files someone else holds, not an agent's commit of sealed paths, not an agent's force push to a protected branch. |
   | `sync` | Nothing is fetched or pushed, `acs init` adds no fetch setting, and claims fetched earlier are ignored. `acs sync` still writes local session records. |

5. **Self-protection is not a feature.** Whatever is off, agents cannot write `.acs/`, `.git/`,
   `.husky/` or the harness's hook and plugin settings (`SELF_PROTECTION_SEALED` in `acs-core`,
   the default seals minus `.github/workflows/**` and `**/.env*`), cannot run the `acs` commands
   that change state, cannot skip the git hooks, and cannot touch `ACS_HOME`. Otherwise
   `guards = false` would let an agent switch the other features off, or back on.
6. **The trailer belongs to `history`, not to `commit_checks`.** The trailer points at a session
   record; without records it points at nothing. `commit_checks` is only about what git is
   stopped from doing. This answers decision 0031's open question: the trailer can be switched
   off, together with the records.
7. **`acs features`** lists the switches; `acs features <name> on|off` rewrites the one line of
   `config.toml` and leaves comments alone. Agents may run the list and are denied the switch.
   `acs features sync on` also adds the fetch setting `acs init` now skips.
8. **`acs init` writes the table out** with every switch at its default, one comment per line,
   and prints what is on and off.
9. **Where it is enforced.** `guards` and `territory` in the policy compiler (the matchers leave
   out what is off; the files are still validated), `claims` and the rest of `guards` in the
   engine, the others in the hook pipeline, the git hooks and the commands. The engine reads the
   switches through `PolicyView::config()`, so the frozen trait did not change; `Config` gained
   one field and its schema was regenerated.

## Consequences

- **This repository stops syncing after `acs` is reinstalled,** until the human sets
  `sync = true` (`acs features sync on`). Agents cannot: the file is sealed.
- With `history` off, `acs watch` and the VS Code extension lose one of the three signals they
  use to tell an agent's change from a person's (post-tool attributions). A file an agent's shell
  command changed without claiming it may then be claimed for the person.
- With `claims` off the VS Code extension's claim calls are refused; it was not changed to hide
  itself.
- Sessions that end while `history` is off stay unrecorded; if history is switched back on, the
  next sync records what is left of them (usually little, since no changes were attributed).
- `claims` is a boolean. Decision 0031's observe mode needs a third state; the key can later
  accept `"observe"`, `"enforce"` and `"off"` next to `true` and `false` without breaking files
  written now.
- The usage skill and the MCP tool descriptions still tell agents to claim before editing. Where
  claims are off, the session-start context and the `acs_claim` reply say so instead.

## Not done

- One real-agent run (`tests/e2e/real/features.sh`, recorded in
  `docs/releases/feature-switches.md`) covers `claims`, `lore` and `history` off with
  `territory` on. The other switches are covered by `crates/acs-cli/tests/features.rs`, the
  engine table and the policy tests only.
- **Hooks still start for every tool call.** A switch skips the work inside `acs hook`; it does
  not stop the harness from calling it. With `lore` off, a `Read` still starts `acs` before and
  after, for nothing. Skipping that needs either an early exit in the hook (saves the state
  work, not the process start) or hook registration that follows the switches (a change to how
  the plugin is installed). Neither is built.
- `acs doctor` reports the switches but still does not check sync (issue 29).
