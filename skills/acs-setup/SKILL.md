---
name: acs-setup
description: Set up ACS (Agent Control System) in the current git repository. Installs the acs binary and the harness plugin, writes the repository policy and commits it on a branch. Use when your pilot asks to set up, install, adopt or try ACS.
---

# Setting up ACS in a repository

ACS is a gate between coding agents and a repository: claims stop agents overwriting each other,
and sealed paths stay out of their reach. Your pilot asked for it in one sentence, so work through
the steps below in order and ask them nothing the steps do not ask. They only approve prompts.

If a step fails, stop and show your pilot the output. Do not improvise around it.

## 1. Check where you are

**Repository.** Run `git rev-parse --show-toplevel` and run everything below from that directory.
If this is not a git repository, stop: tell your pilot ACS needs one.

**Harness.** Find yours in this table.

| You are running in | Harness id | How to tell |
| --- | --- | --- |
| Claude Code | `claude-code` | `CLAUDECODE=1` is in your environment |

Any other harness: stop here and install nothing. Tell your pilot that ACS does not support that
harness yet, only Claude Code.

**Operating system.** `uname -s` prints `Darwin`, `Linux`, or something starting with `MINGW` or
`MSYS` on Windows. Where it does not run at all, you are on Windows without a POSIX shell.

## 2. Install the `acs` binary

Skip this step if `acs --version` already works.

macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/acs-hq/acs/releases/latest/download/acs-cli-installer.sh | sh
```

Windows:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/acs-hq/acs/releases/latest/download/acs-cli-installer.ps1 | iex"
```

The installer puts `acs` in `$CARGO_HOME/bin` (normally `~/.cargo/bin`) and adds it to `PATH` for
new terminals, but not for this session. In a POSIX shell, start every later command in this
session with `export PATH="$HOME/.cargo/bin:$PATH" && `. In PowerShell, start it with
`$env:Path = "$HOME\.cargo\bin;$env:Path"; `. Then check that `acs --version` works. Remember that
you installed it here: step 6 needs to know.

## 3. Install the plugin

```sh
acs plugin install claude-code
```

This enables the plugin for this repository only, in `.claude/settings.json`. Do not pass
`--scope user` unless your pilot asks for it: that gates every repository they open.

## 4. Set up the repository

```sh
acs init
acs doctor
```

`acs init` writes `.acs/policy/`, installs the git hooks and adds the fetch refspec for shared
claims. Running it in a repository that already has `.acs/policy/` is safe and keeps that policy.
In `acs doctor`, stop on any `FAIL`. A `WARN` about a harness you are not running in is fine.

## 5. Commit on the branch `acs/adopt`

Commit before the gate starts. Once the plugin is active, agents may not commit `.acs/**` or
`.claude/settings.json`, because both are sealed.

Run `git status --short` and work out which of these exist as changes: `.acs`,
`.claude/settings.json`, and `.husky` if the repository uses husky (`acs init` adds to its hook
files). If there are none, this repository had already adopted ACS: skip to step 6. Otherwise commit
only those paths, so your pilot's other work stays out of it:

```sh
git switch -c acs/adopt
git add .acs .claude/settings.json
git commit -m "Adopt ACS" -- .acs .claude/settings.json
```

If `git switch -c` says the branch exists, use `git switch acs/adopt`.

If the commit is rejected because the files are sealed, the plugin was already active in this
session. That is the gate working. Do not use `--no-verify`, `acs off` or any other way around it.
Leave the files staged and give your pilot the `git commit` line to run in their own terminal.

## 6. Tell your pilot

Show this summary, filled in from what happened, and nothing longer than one screen:

```text
ACS is set up in <repository>.

Binary     acs <version>, <path from `acs doctor`>
Plugin     acs@acs-plugins for this repository
Policy     .acs/policy/ with the defaults (`acs status` lists the sealed paths)
Git hooks  pre-commit, prepare-commit-msg, pre-push
Claims     shared through <remote>, or: on this machine only, no remote
Doctor     <n> ok, <n> warnings, 0 failures
Commit     <short hash> on acs/adopt

The gate starts at the next Claude Code session.
```

Then say how to start it:

- If you installed the binary in step 2: open a new terminal and start Claude Code there. Claude
  Code only finds `acs` on the `PATH` it started with, and without it the hooks check nothing.
- Otherwise: restart Claude Code, or run `/reload-plugins`.

Finally, ask your pilot whether to open a pull request for `acs/adopt`. Do not push or open one
until they say yes. On a yes, push the branch and open the pull request with the tools you have.

From the next session on, the `acs-usage` skill tells you how to work under the gate.
