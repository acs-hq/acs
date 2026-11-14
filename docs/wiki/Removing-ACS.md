# Removing ACS

One command takes ACS out of a repository: `acs uninstall`. Run it yourself, in your own terminal. Agents are not allowed to run it.

## See what it would do first

```bash
acs uninstall --dry-run
```

`--dry-run` lists everything `acs uninstall` would remove and changes nothing. Each line is marked `to remove`, and the output ends with `Dry run: nothing was changed.`

## Remove ACS from your clone

```bash
acs uninstall
```

It always removes what `acs init` and `acs plugin install` added to your clone:

| What | What happens to it |
| --- | --- |
| Git hooks | The ACS block is cut out of each hook. A hook that held only ACS's block is deleted. A hook of yours that ACS moved aside (`<hook>.acs-orig`) is put back. |
| The git setting that fetches claims | Removed from every remote. |
| Local claim refs (`refs/acs/*`, `origin/acs/*`) | Deleted. |
| The Claude Code plugin in `.claude/settings.json` and `.claude/settings.local.json` | Only ACS's two entries are removed. The rest of the file stays exactly as it was. A file that held nothing else is deleted. |
| The Antigravity plugin folder `.agents/plugins/acs/` | Deleted. |

Your hooks, git config and settings end up byte for byte as they were before `acs init`.

## Three things it asks about

These cannot simply be put back, so `acs uninstall` asks about each one on its own. Answer `y` to delete it; anything else keeps it.

| What | Flag | Why it asks |
| --- | --- | --- |
| The policy in `.acs/policy/` | `--policy` | It is your team's. Deleting it and committing that removes ACS for everyone. |
| This repository's history and claims in ACS's folder on your machine (`~/.acs`, or `ACS_HOME`) | `--state` | Every clone of the repository on your machine shares it, and `acs undo`, `acs log` and `acs blame` need it. |
| The claim and session refs on the remote | `--remote-refs` | Your teammates see them. |

`--all` says yes to all three. With `--json`, or when no terminal is attached, it asks nothing and keeps all three unless you pass their flags.

For example, to remove everything, local and on the remote, without questions:

```bash
acs uninstall --all
```

## Afterwards

- **Commit what changed in the repository**, so teammates stop using ACS too: `.acs/policy/` if you deleted it, `.claude/settings.json`, and `.husky/` or your hooks folder if ACS had changed hooks there. The output lists them.
- **Teammates** run `acs uninstall` in their own clones to remove the hooks and the git setting there, which do not travel with `git pull`.
- **A Claude Code plugin enabled for your user** (not just this repository) is not touched, because it applies to every repository you open. The output gives the `claude plugin uninstall` command when it finds one.
- **The `acs` program stays installed.** Remove it the way you installed it.
- **To set ACS up again**, run `acs init`. If you kept `.acs/policy/`, it is used as it is.

## If something fails

- **Local state cannot be deleted on Windows:** a running Claude Code session, `acs watch` or the VS Code extension may hold it open. Close them and run `acs uninstall --state` again.
- **Refs on the remote cannot be deleted:** `acs uninstall` never waits for a password. Check that a plain `git push` to that remote works, then run `acs uninstall --remote-refs` again.
