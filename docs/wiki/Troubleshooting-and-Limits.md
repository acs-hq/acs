# Troubleshooting and limits

Start with `acs doctor`. It checks the installation, and `acs doctor --fix` repairs what it can: file permissions, a damaged state database or policy cache, and missing git hooks.

```bash
acs doctor
```

```bash
acs doctor --fix
```

## Troubleshooting

**ACS seems to do nothing, and there is no error.**
The `acs` program is probably not on your `PATH`. In that case the plugin's hooks do nothing and say nothing. Run `acs doctor` and check the `acs on PATH` and `Claude Code plugin` lines. Then start a new Claude Code session, because a session that was already open does not pick up the plugin.

**`acs doctor` warns about the Claude Code plugin.**
Accept the plugin in Claude Code if it offers it. If it does not, run this, which is safe to repeat:

```bash
acs plugin install claude-code
```

The check only reads settings files, so it can warn about a plugin that is in fact installed somewhere it cannot see.

**You fixed a problem, but ACS's tools still fail in Claude Code.**
Claude Code remembers a failed connection to the ACS tools for 15 minutes, so a fixed problem can look broken for a while. Wait, then start a new session.

**Updating `acs` fails on Windows.**
While a Claude Code session is running, the `acs` file is locked. Close all sessions first, then update.

**`acs doctor` warns about the policy.**
A policy file is invalid. ACS is using the last valid policy, or the built-in defaults if there is none. The warning names the problem. Unknown keys count as errors, so check for typos.

**A teammate's claims do not show up.**
Check the `Remote:` line in `acs status` and any `Last sync error` line after it, then try a sync by hand:

```bash
acs sync
```

- If you added the remote after setup, run `acs init` again.
- `acs sync` never prompts for credentials. If `git fetch` asks you for them, sync fails until they are cached.
- `acs doctor` does not check sync yet, so a clean result does not prove that claims are being shared.
- If the remote cannot be reached, you get a one-line warning. Local claims are still enforced, but other machines' claims are not.

**Your own commit is rejected as "made by agent session".**
If an agent's commit was stopped by another git hook in the last 10 minutes, ACS can mistake your commit for the agent's. Wait and try again.

**A session was blocked halfway through a task.**
It can leave uncommitted changes in the work tree. Check `git status` before you start something else there.

**You need ACS out of the way for a moment.**
Run `acs off`, and `acs on` when you finish. See [Turning the gate off](Working-with-ACS#turning-the-gate-off).

## Limits

**ACS works with Claude Code today.** It has no effect on other tools.

**ACS only coordinates people who run it.** A teammate without it is invisible to it, and it does not block them. It also does not see your own editor, unless you use `acs claim`.

**ACS is not a security boundary.** It denies the ways of switching it off that it can see: an agent running `acs off`, `acs override` or `git commit --no-verify`, writing to ACS's state or a harness's settings, or starting another agent with its hooks off. It cannot see inside scripts and programs it does not know, so an agent that hides what it does there can still get past it. It guards against accidents and careless agents, not against an agent set on getting around it. The project's security notes list what is not covered.

**The git hooks can be skipped by people.** They let everything through if `acs` is not on `PATH`, and `git commit --no-verify` skips them. ACS denies `--no-verify` to agents, not to you.

**Owned zones bind agents, not people.** Only the owners' agents may change an `owned` zone; you can still edit it yourself. See [territory.toml](Configuring-Policy#territorytoml).

**Sealed paths only stop edits ACS can see.** A tool that writes files itself, such as `dotnet ef migrations add`, is not blocked when it writes. The commit is where an agent gets caught.

**A worktree or branch without a committed `.acs/policy/` gets no git hook checks.** Commit the policy first.

**A long silence loses claims, and that includes one long command.** A session's claims are refreshed when a tool call finishes, so a command that runs longer than `claim_ttl_secs` can lose them. Raise the limit if your sessions run commands that long.

**Sharing is not instant.** Claims travel between machines about every 30 seconds by default, while sessions run. If two machines claim the same file before either has seen the other, the first claim keeps the file. Edits made in that gap are only caught at commit time.

**Claims are not private on a public repository.** They hold file paths, git emails and timestamps. Anyone who can read the repository can read them.

**ACS state is per machine.** `acs off` and collision history apply to one machine.
