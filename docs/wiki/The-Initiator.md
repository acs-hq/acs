# The initiator

You are the initiator if you are the first person to adopt ACS in a repository. You do this once. Everyone else follows [Teammates](Teammates).

## Your steps

### 1. Set up ACS

Install ACS (see [Install](Install)). Then, in the repository:

```bash
acs plugin install claude-code
```

```bash
acs init
```

```bash
acs doctor
```

- `acs plugin install claude-code` turns on the ACS plugin for this repository by writing `.claude/settings.json`. Keep the default scope, `project`: it is the one that reaches your team when you commit it. `--scope user` would apply ACS to every repository you open, including ones that never adopted it.
- `acs init` writes the policy files to `.acs/policy/`, adds a git setting for sharing claims and installs git hooks. A claim is one session's temporary hold on one file. It is safe to run `acs init` again.
- If your main branch is not called `main` (for example `master` or `develop`), `acs init` finds it from the remote's `HEAD` and writes it into the policy, so claims are compared on the right branch and agents cannot rewrite its history. Its output has a `Main branch` line; check it.
- `acs init` also proposes zones for `territory.toml` from folder names and recent git history, and prints them. It writes them only if you run `acs init --yes` or answer yes when it asks. See [Zones acs init proposes](Configuring-Policy#zones-acs-init-proposes).
- `acs doctor` checks the result. Look for `0 failure(s)` and no warning on the `Claude Code plugin` line.

Then start a new Claude Code session so the plugin loads.

### 2. Edit the policy

The defaults work, so this step is optional. To change them, see [Configuring policy](Configuring-Policy), then run `acs doctor` again.

### 3. Commit from your own terminal

Commit these yourself, not through an agent. Once the plugin is active, agents cannot edit or commit `.acs/**` or `.claude/settings*.json`, because those paths are sealed (no agent may change them).

```bash
git add .acs .claude/settings.json
```

If your repository tracks its git hooks in `.husky/` or a `core.hooksPath` folder such as `.githooks/`, `acs init` changed files there, so add that folder too. For example:

```bash
git add .husky
```

```bash
git commit -m "Adopt ACS"
```

### 4. Get that commit onto the main branch

Merge it the way your team merges anything else. Do this before people start work: a worktree or branch without a committed `.acs/policy/` gets no git hook checks.

### 5. Tell the team

ACS only coordinates people who run it, so everyone needs to do their part. You can send them this:

> We now use ACS in this repository. Please install it, pull the main branch, run `acs init` once in your clone, accept the ACS plugin in Claude Code, start a new session and run `acs doctor`. Steps: [Teammates](Teammates).

## Who does what

| What | Does it travel through git? | Who has to act |
| --- | --- | --- |
| Policy files in `.acs/policy/` | Yes | Initiator commits them. Teammates get them with `git pull`. |
| Plugin setting in `.claude/settings.json` | Yes | Initiator commits it. Each teammate accepts the plugin in Claude Code. |
| Hooks tracked in `.husky/` or a folder such as `.githooks/` | Yes | Initiator commits them. |
| Hooks that are not tracked by the repository | No | Everyone runs `acs init`. |
| The git setting that fetches claims | No, it is local to each clone | Everyone runs `acs init`. |
| The `acs` program itself | No | Everyone installs it. |
| A new Claude Code session | No | Everyone starts one after setup. |
| Live claims | Yes, once you switch team sync on | Initiator switches it on and commits. Nobody else, after `acs init`. |

## How claims reach your team

Team sync is off until you switch it on, so a fresh setup sends nothing to the remote. To share claims with teammates, run this in your own terminal and commit the change to `.acs/policy/config.toml`:

```bash
acs features sync on
```

From then on ACS syncs claims on its own in the background while sessions run, about every 30 seconds by default. You do not need to push or pull anything for this. Teammates run `acs init` once after pulling the change. If you add the remote later, run `acs init` again.

With sync off, or with no remote, claims stay on your machine.

Claims travel through hidden refs (`refs/acs/*`), not through your commits.

- They hold file paths, the pilot's git email and timestamps, never file contents. The pilot is the person running a session.
- Anyone who can read the repository can read them. On a public repository they are public.
- They are not branches and do not appear in GitHub's branch list.
- Normal clones and pulls do not fetch them. After `acs init` they appear in `git branch -r` as `origin/acs/claims/<id>`. That is expected.

On your own machine, ACS keeps its state in `~/.acs` (or the folder in `ACS_HOME`), outside the repository. It holds claims, an event log and snapshots, and it is shared by all clones and worktrees of a repository on that machine.

## Removing ACS

One command takes ACS out again: `acs uninstall`. See [Removing ACS](Removing-ACS).
