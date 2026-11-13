# Defaults for first testers

Written 2026-11-11. What a person gets when they run `acs init` in a repository for the first
time: what we agreed, what the code does today, and every setting that can be changed. Decision
0031 records the agreement; this file is the full list.

"Today" is what the code on `main` does. "For first testers" is what we agreed. Where the two
differ, the change is **not built yet** and the issue is named.

## 1. Features

| Feature | Today | For first testers | Built? |
| --- | --- | --- | --- |
| Claims | Blocking: an edit of a file another session holds is denied | **Observe** (see below) | **No. Observe mode is not implemented.** No issue yet; the prompt is in the maintainer's notes |
| Delete-outside-claims rule | On (limit 0) | Off in observe mode, recorded only | No, part of observe mode |
| Team sync and ledger push | On as soon as the repository has a remote | **Off** until the repository opts in | No, issue 61 |
| Territory prompt at `acs init` | Asks once in a terminal | No question at init | No, issue 61. Whether the proposal is still printed is open |
| Territories (`owned`, `open`, `sealed` zones) | Enforced when written in `territory.toml` | Same: available and enforced when configured by hand | Yes |
| Self-protection seals (`.git`, `.acs`, harness settings and hooks, `.husky`) | On, cannot be removed | On | Yes |
| `**/.env*` seal | On, also matches `.env.example` | On; the `.env.example` case is open | Issue 62, your decision |
| `.github/workflows/**` seal | Deny | Open: deny, ask or off | Issue 62, your decision |
| Banned commands, protected branches | On | On | Yes |
| Bulk delete limit | Ask above 50 files per command | Same | Yes |
| "Cannot tell which files" prompt | On | On; first to switch off if testers complain (issue 22) | Yes |
| Writes outside the repository | Not asked about (`protect_outside_repo = false`) | Same | Yes |
| Session records, `acs log`, `acs blame`, `acs undo`, `acs stats` | On | On, local only while sync is off | Yes |
| Lore | On; does nothing until an entry exists | Same | Yes |
| Git hooks and the `ACS-Session` commit trailer | On | On; a switch for the trailer is open | Yes; the switch is not built |
| Strict mode when `acs` is missing | Off (Claude Code fails open) | Off | Not built (issue 11) |
| `acs watch`, VS Code extension | Opt-in | Opt-in | Yes; the extension is not published |
| Antigravity | Opt-in | Opt-in, not mentioned to testers | Yes |

### Observe mode (not implemented)

Agreed as the default claims mode, with `enforce` (today's blocking) and `off` as the other two
values. **None of this exists in the code yet.** Today the only choices are blocking, or `acs off`,
which records nothing.

What it is meant to do:

- An edit of a file another session holds is allowed. The agent gets one short note naming the
  other session, its pilot and its intent.
- The event is recorded as a would-be collision and shown in `acs status` and `acs stats` as
  "would have prevented".
- The git pre-commit hook never rejects a commit because of a claim.
- Human claims (`acs claim`, `acs watch`, the extension) are advisory too.
- Agents are told to call only `acs_intent`, not to claim and release.
- Sealed paths, banned commands, protected branches, self-protection and territories work as now.

Still to decide when it is designed: where the setting lives, whether several sessions hold one
file at once, how `acs off` relates to it, and what remote claims do.

## 2. `config.toml`

| Key | Default | Verdict | Why |
| --- | --- | --- | --- |
| `spec` | `"acs/0.1"` (required) | Keep | Not a choice |
| `default_target_branch` | `"main"` | **Change how it is set** | A repository whose main branch is `master` or `develop` gets a wrong value silently. `acs init` should read the remote's default branch and write it. The value itself can stay `main` when nothing can be detected |
| `claim_ttl_secs` | `900` (15 minutes) | Keep | It measures silence, not how long a file is held. Shorter loses claims during long test runs; longer keeps a crashed session's files held. In observe mode nothing is blocked, so a wrong value costs little. For enforce mode the fix is issue 32 (one long command loses all claims), not a different number |
| `sync_interval_secs` | `30` | Keep | Only matters once sync is switched on. Claims are pushed about every 3 minutes whatever this says (a heartbeat is published after a quarter of the TTL); the interval mostly sets how fresh the fetched view is |
| `[ledger] store_prompts` | `false` | Keep, and **remove from the template** | Nothing in the code reads it: prompts are never stored either way (issue 46). A commented key that does nothing invites someone to set it |
| `[lore] max_inject_chars` | `4000` | Keep | About 1,000 tokens at most per tool call, only on the first touch of a scope |
| `[lore] stale_percent` | `30` | Keep | A stale entry is still shown, with a request to check it, so a wrong threshold is cheap |

Keys we agreed on that do not exist yet: the claims mode (default `observe`), a switch for team
sync (default off), and possibly a switch for the commit trailer. Each is a change to the frozen
policy types and schemas.

## 3. `guards.toml`

| Key | Default | Verdict |
| --- | --- | --- |
| `sealed` | Adds to the built-in list of 13 patterns; the built-ins cannot be removed | Keep, except the two cases in issue 62 |
| `protected_branches` | `["main", "release/*"]` | **Add the detected default branch.** A repository on `master` has no protected branch today |
| `protect_outside_repo` | `false` | Keep (decision 0027) |
| `[destructive] max_files_deleted_outside_claims` | `0` | Keep for enforce mode; not enforced in observe mode |
| `[destructive] max_files_deleted_per_command` | `50` | Keep |
| `[destructive] ban` | `["git push --force*", "git reset --hard*", "git clean -f*"]` | Keep |

Setting `protected_branches` or `ban` replaces the default list; setting `sealed` adds to it.

The built-in sealed list: `.git/**`, `.acs/**`, `.github/workflows/**`, `**/.env*`,
`**/.claude/settings*.json`, `**/.claude/hooks/**`, `.husky/**`, and six patterns for Antigravity's
hooks and plugins under `.agents/` and `.agent/`. `acs status --sealed` prints it.

## 4. `territory.toml` and lore files

- `territory.toml`: no zones by default, so the whole repository is `shared`. Zones have `paths`,
  `kind` (`owned`, `shared`, `open`, `sealed`) and `owners`; `[teams]` names groups of emails.
- Lore entries (`.acs/policy/lore/**.md`): `id`, `scope`, `owner`, `written_against`. None exist by
  default.

## 5. Command flags with defaults

| Command | Flag | Default |
| --- | --- | --- |
| `acs init` | `--min-share` | `75` (percent of commits for a proposed owned zone) |
| `acs init` | `--min-commits` | `10` |
| `acs plugin install` | `--scope` | `project` |
| `acs plugin install` | `--marketplace` | `acs-hq/acs` |
| `acs watch` | `--idle` | `120` seconds, capped at `claim_ttl_secs` |
| `acs log` | `--limit` | `20` |

## 6. Environment variables

| Variable | Default | Meaning |
| --- | --- | --- |
| `ACS_HOME` | `~/.acs` | Where ACS keeps its state, outside the repository |
| `ACS_SCAN` | `auto` | How the post-tool step finds changed files: `auto`, `full` or `scoped` |
| `ACS_SCAN_MAX_ENTRIES` | `20000` | Above this many tracked files, `auto` scans only likely targets |
| `ACS_CRASH_AFTER_SECS` | `3600` | When a silent session is recorded as crashed. Meant for tests |

## 7. VS Code extension settings

| Setting | Default |
| --- | --- |
| `acs.path` | `acs` (looked up on `PATH`) |
| `acs.claimOnEdit` | `true` |
| `acs.releaseAfterIdleSeconds` | `120` |
| `acs.refreshIntervalSeconds` | `5` |

## 8. Fixed values (not settings, but they shape behaviour)

| Value | Fixed at |
| --- | --- |
| Largest file ACS keeps a copy of for undo | 32 MB |
| Most files copied for one tool call | 2,000 |
| Collisions shown in `acs status` | Last 24 hours |
| An unused override lapses after | `claim_ttl_secs` |
| A fetched view of others' claims goes stale after | 3 sync intervals |
| One network call by sync gives up after | 120 seconds |
| A pending agent commit marker is trusted for | 10 minutes (issue 38) |
| Intent text kept | 500 characters |
| Lore proposals waiting per machine | 200 |

## 9. Found while checking

- **Copies kept for undo look as if they are never cleaned up.** The snapshot store has a cleanup
  routine with a size budget, but I found no code outside tests that calls it, so `ACS_HOME` would
  grow without limit. Not confirmed by a run. Worth an issue before testers use it for weeks.
- **A repository on `master` gets two wrong defaults at once** (`default_target_branch` and
  `protected_branches`), with no warning.
- **`store_prompts` is a setting with no effect.**
