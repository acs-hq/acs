# Working with ACS

Most of the time you do nothing. Your agents edit files, ACS tracks who holds what, and you only hear from it when something is blocked.

## Claims

A claim is one session's temporary hold on one file. While a session holds it, no other session can change that file.

- **Editing claims the file automatically.** Agents do not have to ask first, though ACS tells them to claim up front for large changes.
- **Every session counts separately.** Two sessions of yours block each other, just as a teammate's session would.
- **Branches do not separate you.** Sessions on different branches still block each other when their work goes into the same branch, which is `main` by default.
- **Claims last while the session is active.** The time limit is `claim_ttl_secs`, 900 seconds by default. Any tool call by a session refreshes all that session's claims, so the limit measures silence, not how long a file has been held.
- **A quiet session loses its claims.** Once a claim has expired it cannot be revived. A later edit is a new claim, and it is blocked if someone else has taken the file in the meantime.
- **Ending a session releases its claims.**

### Claiming a file yourself

ACS does not see your editor. To keep agents off files you are working on, claim them:

```bash
acs claim src/lib.rs src/util.rs
```

```bash
acs claim src/payments/
```

A folder claims the files under it. ACS refuses a claim on a sealed path or a file another session holds.

Release them when you finish:

```bash
acs release src/lib.rs
```

```bash
acs release --all
```

Your claims are not refreshed automatically. They expire at the time limit, 15 minutes by default, so claim again if you need longer.

## When ACS blocks an agent

The agent receives a message that says what is blocked and what to do next. It should read that message and move on, not retry or work around it. You will usually see it in the session.

| The message says | What it means | What to do |
| --- | --- | --- |
| `` `src/api.ts` is claimed by sara@example.com (claude-code, active just now) `` | Another session holds the file. | Let the agent do other work, or arrange a handover: end the other session, ask its agent to release the file, or wait for the claim to expire. |
| `` `.env` (sealed by `**/.env*`) is protected by ACS `` | The path is sealed: no agent may change it. | Make the change yourself. |
| `This would delete 1 file that you have not claimed` | The agent tried to delete a file it does not hold. | If you want it deleted, tell the agent to claim it first. |
| `` The command `git reset --hard HEAD~1` is banned by this repository's policy `` | The command is banned. | Ask the agent for a safer way, or run it yourself if you mean it. |
| `` `git rebase`, which rewrites history, would change protected branch `main` `` | Agents cannot rewrite history or force push on a protected branch. | Have the agent work on a feature branch, or do it yourself. |

The delete rule protects against accidents. It is not an approval step: an agent told to delete a file claims it first and then deletes it, and nobody is asked.

In three cases, ACS asks you instead of refusing. Approve or decline the prompt as you would any other Claude Code permission prompt.

| The message says | What it means |
| --- | --- |
| `This command would delete 51 files, more than the limit of 50 per command.` | The command is allowed only if you approve it. |
| `This command deletes, moves or rewrites files, but ACS cannot tell which ones` | ACS cannot work out the targets, for example because a variable hides them. The agent can avoid the question by naming the exact paths. |
| `` `notes.txt` is outside the repository `` | The agent wants to change a file outside the repository, where ACS cannot protect anything. |

## When a commit is rejected

The git hooks are the backstop. They check each commit for claimed and sealed files.

| Who commits | File claimed by another session in the same work tree | File claimed in another worktree or on another machine | Sealed path |
| --- | --- | --- | --- |
| An agent | Rejected | Rejected | Rejected |
| A person | Allowed | Rejected | Allowed |

A person can commit a file claimed by an agent in the same work tree because "agent edits, human commits" is the normal flow.

The rejection names the staged files that are off limits. To carry on:

- Unstage those files and commit the rest: `git restore --staged <path>`.
- Or wait until the holder releases them. `acs status` shows who that is.
- If you are a person and mean to commit anyway, `git commit --no-verify` is your override. It skips the hooks.

## acs status

`acs status` shows what ACS knows right now.

```bash
acs status
```

```text
ACS status
  branch    feature/refunds (work is integrated into main)
  you       sara@example.com (human)

Remote: in sync (last fetch 4s ago)

Your claims (1):
  src/payments/refund.ts                                 held 2m, expires in 13m

Claimed by others (2):
  src/api.ts                                  claude-code sara@example.com 3m
  src/auth.ts                                    lina@example.com [remote] 40s

Recent collisions (1 in the last 24 h):
  src/api.ts                               claude-code sara@example.com 1m ago
    `src/api.ts` is claimed by sara@example.com (claude-code, active just now).
    Work on other files, or ask your pilot to coordinate.

Sealed paths no agent may write (13):
  .git/**
  .acs/**
  .github/workflows/**
  **/.env*
  and 9 more (`acs status --sealed` lists all)
```

- **Your claims** are the ones you made with `acs claim`.
- **Claimed by others** lists live claims, including those of your own agents. Claims from other machines are marked `[remote]`.
- **Recent collisions** are the times a session was blocked by a claim in the last 24 hours, on this machine only.
- **Remote** says whether claims are being shared. If the last sync failed, a `Last sync error` line follows it.

`acs status --sealed` lists every sealed path, and `--json` gives the same information for scripts.

## Turning the gate off

The gate is ACS's check on each tool call. To turn it off for this repository on this machine:

```bash
acs off
```

Claims, owned zones and the delete rule then stop applying, on this machine only. Sealed paths and banned commands still hold, and agents still cannot run `acs` commands that change its state or skip the git hooks. `acs status` shows a `GATE OFF` warning until you turn it back on:

```bash
acs on
```

## Letting an agent into owned territory

An `owned` zone lets only its owners' agents change it. To let another agent in, grant an override from your own terminal:

```bash
acs override --path 'payments/**' --once
```

```text
This lets agents change files matching `payments/**` in owned territory, for the next tool call
that needs it. Claims and sealed paths still apply.
Type KXRM to confirm: KXRM
Override ovr_... granted for `payments/**`.
```

- `--once` covers one tool call: the first agent call that needs it uses it up.
- `--session` covers the rest of the session of the first agent that uses it.
- An override nobody uses lapses after `claim_ttl_secs` (15 minutes by default).
- It only opens owned zones. Claims, sealed paths and every other rule still apply.

`acs override` asks you to type a code, so it only works in a terminal, and agents are not allowed to run it. `acs status` lists the overrides still in force; `acs override --revoke <id>` withdraws one. Every use is recorded with the session that made it.

## Syncing by hand

ACS syncs claims on its own, so you rarely need this. To sync now:

```bash
acs sync
```
