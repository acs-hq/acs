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
| Antigravity | `antigravity` | `ANTIGRAVITY_CONVERSATION_ID` is in your environment |

Any other harness: stop here and install nothing. Tell your pilot that ACS does not support that
harness yet, only Claude Code and Antigravity.

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

Use your harness id from step 1:

```sh
acs plugin install <harness id>
```

This installs the plugin for this repository only. Claude Code: it is enabled in
`.claude/settings.json`; do not pass `--scope user` unless your pilot asks for it, because that
gates every repository they open. Antigravity: the plugin is written to `.agents/plugins/acs/`.

## 4. Set up the repository

```sh
acs init
acs doctor
```

`acs init` writes `.acs/policy/`, installs the git hooks and adds the fetch refspec for shared
claims. Running it in a repository that already has `.acs/policy/` is safe and keeps that policy.
In `acs doctor`, stop on any `FAIL`. A `WARN` about a harness you are not running in is fine.

**Territory.** `acs init` also proposes zones: `open` for folders named like documentation,
examples or scratch space, and `owned` for folders where one person made most of the last 12
months' commits. It does not write them yet. Read the `Territory` part of its output:

- `nothing to propose` or `kept`: go on to step 5.
- `proposed`: show your pilot the zones as `acs init` printed them and ask one question: write
  them as they are, write them with changes, or skip them. Tell them what the kinds mean in one
  line each: `owned` lets only the owner's agents change that folder (your pilot can let another
  agent in with `acs override`); `open` lets agents edit without claims, so two agents can
  overwrite each other's changes there. Then:
  - As they are: run `acs init --yes`.
  - With changes: run `acs init --yes`, edit `.acs/policy/territory.toml` the way your pilot said
    (remove a zone, change a `kind`, add or change an owner), and run `acs doctor` again. Stop on
    a `FAIL`. If the edit is denied as sealed, the plugin is already active in this session: give
    your pilot the change to make in their editor.
  - Skip: do nothing. Every folder stays `shared`: any agent may change a file after claiming it.

## 5. Commit on the branch `acs/adopt`

Commit before the gate starts. Once the plugin is active, agents may not commit `.acs/**` or the
plugin files, because they are sealed.

Run `git status --short` and work out which of these exist as changes: `.acs`, the plugin path
(`.claude/settings.json` for Claude Code, `.agents/plugins/acs` for Antigravity), and `.husky` if
the repository uses husky (`acs init` adds to its hook files). If there are none, this repository
had already adopted ACS: skip to step 6. Otherwise commit only those paths, so your pilot's other
work stays out of it. For Claude Code:

```sh
git switch -c acs/adopt
git add .acs .claude/settings.json
git commit -m "Adopt ACS" -- .acs .claude/settings.json
```

For Antigravity, use `.agents/plugins/acs` in place of `.claude/settings.json` in both lines.

If `git switch -c` says the branch exists, use `git switch acs/adopt`.

If the commit is rejected because the files are sealed, the plugin was already active in this
session. That is the gate working. Do not use `--no-verify`, `acs off` or any other way around it.
Leave the files staged and give your pilot the `git commit` line to run in their own terminal.

## 6. Tell your pilot

Show this summary, filled in from what happened, and nothing longer than one screen:

```text
ACS is set up in <repository>.

Binary     acs <version>, <path from `acs doctor`>
Plugin     <acs@acs-plugins, or .agents/plugins/acs> for this repository
Policy     .acs/policy/ with the defaults (`acs status` lists the sealed paths)
Territory  <the zones written, for example "docs/ open, payments/ owned by sara@...">, or: none,
           every folder is shared
Git hooks  pre-commit, prepare-commit-msg, pre-push
Claims     shared through <remote>, or: on this machine only, no remote
Doctor     <n> ok, <n> warnings, 0 failures
Commit     <short hash> on acs/adopt

The gate starts at the next <Claude Code or Antigravity> session.
```

Then say how to start it:

- If you installed the binary in step 2: open a new terminal and start your harness there. It
  only finds `acs` on the `PATH` it started with. Without it, Claude Code's hooks check nothing,
  and Antigravity denies every tool call.
- Otherwise, Claude Code: restart it, or run `/reload-plugins`. Antigravity: start a new session;
  it loads plugins at session start.

Add one sentence on their own edits: ACS does not see them unless they run `acs watch` in a
terminal, use the ACS extension for VS Code, or claim files with `acs claim`. Do not run
`acs watch` yourself; ACS denies it to agents.

Finally, ask your pilot whether to open a pull request for `acs/adopt`. Do not push or open one
until they say yes. On a yes, push the branch and open the pull request with the tools you have.

From the next session on, the `acs-usage` skill tells you how to work under the gate.
