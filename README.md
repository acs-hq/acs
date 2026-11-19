<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/acs-logo.svg">
    <img src="docs/assets/acs-logo-on-light.svg" alt="ACS" width="300">
  </picture>
</p>

<h1 align="center">Rules for any agent that works in your repository</h1>

<p align="center">
  <b>Works with</b>&nbsp; Claude Code · Antigravity · Codex
</p>

<p align="center">
  <a href="#get-started">Get started</a> ·
  <a href="#what-you-get">What you get</a> ·
  <a href="#commands">Commands</a> ·
  <a href="docs/wiki/Home.md">Guide</a> ·
  <a href="#what-acs-does-not-do">Limits</a>
</p>

---

ACS, the Agent Control System, is a command-line tool that works alongside git. A repository
adopts it once. From then on every coding agent that works there, whoever started it, acts inside
limits the repository's developers have set.

Each time an agent is about to write a file or run a command, ACS checks the call against rules
you commit next to your code. It lets the call through, blocks it with a reason, or asks you. One
agent or twenty, yours or a teammate's, the rules are the same.

## What you get to decide

| | |
| --- | --- |
| **Where agents may never write** | Sealed paths, banned commands, protected branches |
| **Who is in a file right now** | Claims: one writer per file, across sessions and machines |
| **Which folders belong to whom** | Zones with owners, and overrides only a person can grant |
| **What an agent is told first** | Lore: your conventions, handed over when an agent enters a folder |
| **What a session did, and taking it back** | A record per session, line-level blame, undo |

| One binary, no server | It never calls a model | Fast enough to run on every call |
| --- | --- | --- |
| A single static `acs` program. No daemon and no hosted service. Teams share state through the git remote they already have. | Every decision is plain code over your policy files. The same call gets the same answer every time. | A check is a fresh process with a 15 ms budget, so the agent doesn't notice it. |

When ACS blocks a call, the agent is told why and what to do instead:

```text
src/api.ts is claimed by sara@example.com (claude-code, active just now).
Work on other files, or ask your pilot to coordinate.
```

## Get started

Open an agent session in the repository you want to protect, and paste this:

```text
Set up ACS in this repository by following https://raw.githubusercontent.com/acs-hq/acs/main/skills/acs-setup/SKILL.md
```

The agent installs the program and the plugin, writes the policy and commits the result on a
branch. You only approve prompts.

<details>
<summary><b>Or install it by hand</b></summary>

<br>

macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/acs-hq/acs/releases/latest/download/acs-cli-installer.sh | sh
```

Windows:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/acs-hq/acs/releases/latest/download/acs-cli-installer.ps1 | iex"
```

Or with a package manager:

```sh
scoop install https://github.com/acs-hq/acs/releases/latest/download/acs.json
npm install --global @acs-hq/acs
```

Prefer the installers or Scoop for a repository you gate: with npm, every hook call starts Node,
which is much slower. `npx @acs-hq/acs` is fine for trying the commands, but hooks need `acs`
installed on `PATH`.

Then, in the repository:

```sh
acs plugin install claude-code   # or: antigravity, codex
acs init
acs doctor
```

`acs plugin install` enables the plugin for this repository. With `--scope user` it gates every
repository you open, even those that never adopted ACS. For Codex it adds ACS's hooks and MCP
server to `.codex/config.toml`; Codex runs them once you trust the project and, in `/hooks`, the
hooks. `acs init` writes the policy to
`.acs/policy/`, installs the git hooks and is safe to run again. In `acs doctor`, look for
`0 failure(s)`.

Commit `.acs/` and the plugin setting from your own terminal and merge them to your main branch.
Teammates install the program, pull, and run `acs init` once in their clone.

</details>

## What you get

| | Feature | What it does |
| --- | --- | --- |
| 🔒 | **Claims** | One writer per file. The first session to edit a file holds it until it goes quiet or ends. Other sessions are told who holds it. |
| 🛑 | **Guards** | Paths no agent may write (`.git/**`, real `.env` files, your own list such as CI workflows), commands no agent may run, protected branches, and a ceiling on what one command may delete. |
| 🗺️ | **Territory** | Give folders owners. Only the owners' agents may change an owned zone, and only a person can let another agent in. |
| 📖 | **Lore** | What you would tell a new colleague before they touch a folder. Written once, handed to every agent that works there. |
| 🧾 | **History, blame and undo** | A record of every agent session. See which session wrote a line, and take one session's changes back out without touching anyone else's. |
| 🤝 | **Team sync** | Shares claims and session records with your teammates' machines through the git remote you already have. No server. |
| ✅ | **Commit checks** | Git hooks apply the same rules at `git commit` and `git push`, and catch what slipped past. |

Every feature is a switch in `.acs/policy/config.toml`. The policy is plain TOML and Markdown in
your repository, edited by people, reviewed like code:

```toml
# .acs/policy/guards.toml
sealed = ["infra/terraform/**"]
protected_branches = ["main", "release/*"]

[destructive]
max_files_deleted_per_command = 20
ban = ["git push --force*", "git reset --hard*", "terraform apply*"]
```

```toml
# .acs/policy/territory.toml
[[zone]]
paths = ["payments/**"]
kind = "owned"
owners = ["sara@example.com", "team:platform"]
```

## A session, start to finish

```console
$ acs status
Your claims (1):
  src/payments/refund.ts                    held 2m, expires in 13m
Claimed by others (2):
  src/api.ts                     claude-code sara@example.com 3m
  src/auth.ts                       lina@example.com [remote] 40s

$ acs log --since 2d
$ acs blame payments/refund.py     # which session wrote each line, and why

$ acs undo --last                  # plan for the last session that changed files
$ acs undo ses_4k2 --apply         # puts the files back, keeps later work
```

Undo compares each file before the session, after it, and now. A file nobody has touched since
goes back as it was. A file someone changed afterwards is merged: the session's change comes out
and the later work stays. Anything already committed is undone with one new commit. History is
never rewritten.

## Commands

| Command | What it does |
| --- | --- |
| `acs init` | Set ACS up in a repository, or in your clone of one that already uses it |
| `acs status` | Who holds what, what was refused lately, whether sharing works |
| `acs doctor` | Check the install, the plugin, the policy and the hooks. `--fix` repairs what it can. |
| `acs claim` · `acs release` · `acs watch` | Hold files yourself, so agents stay out of what you are editing |
| `acs override` | Let an agent into an owned zone, once or for a session |
| `acs off` · `acs on` | Pause the gate on this machine. Sealed paths and banned commands still hold. |
| `acs lore` | Read the repository's lore and review what agents suggest |
| `acs log` · `acs blame` · `acs undo` | See past sessions, who wrote a line, and take a session back by id or with `--last` |
| `acs verify` · `acs stats` | Check the records are unaltered. Count what ACS prevented. |
| `acs uninstall` | Take ACS out of a repository |

Every command takes `--json` and `--help`. An extension for VS Code claims the files you edit and
marks the ones others hold.

## What ACS does not do

- **It does not merge, review or resolve conflicts.** Git and your code review still do that.
- **It does not let agents talk to each other.** A refused agent is told who holds the file and
  gets on with something else.
- **It does not see people who do not run it.** A teammate without ACS is invisible to it and is
  not blocked.
- **It is not a security boundary.** It guards against accidents and careless agents, not against
  one set on getting around it. See [Limits](docs/wiki/Troubleshooting-and-Limits.md#limits).

## Leaving is one command

```sh
acs uninstall --dry-run   # list what would go, change nothing
acs uninstall
```

Your hooks, git config and settings end up byte for byte as they were before `acs init`. It asks
before deleting the three things that cannot simply be put back: the team's policy, the history
on your machine and the refs on the remote. See [Removing ACS](docs/wiki/Removing-ACS.md).

## Documentation

| Page | Read it if |
| --- | --- |
| [The initiator](docs/wiki/The-Initiator.md) | You are the first person to adopt ACS in a repository |
| [Teammates](docs/wiki/Teammates.md) | Someone else has already adopted ACS in the repository |
| [Configuring policy](docs/wiki/Configuring-Policy.md) | You want to change the rules for your repository |
| [Working with ACS](docs/wiki/Working-with-ACS.md) | ACS blocked something, or you want to know how claims behave |
| [Lore](docs/wiki/Lore.md) | You want agents to know your conventions for part of the repository |
| [History and undo](docs/wiki/History-and-Undo.md) | You want to see what a session did, who wrote a line, or undo a session |
| [Troubleshooting and limits](docs/wiki/Troubleshooting-and-Limits.md) | Something looks wrong, or you want to know what ACS cannot do |
| [Removing ACS](docs/wiki/Removing-ACS.md) | You want to take ACS out of a repository |

Design decisions are in [`docs/decisions/`](docs/decisions/README.md), and the plan is in
[`docs/PLAN.md`](docs/PLAN.md).

## Status

Pre-release. Expect rough edges, and please [open an issue](https://github.com/acs-hq/acs/issues)
when you find one.

## Build from source

Requires the toolchain pinned in `rust-toolchain.toml`, which rustup installs on its own.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release -p acs-cli   # produces target/release/acs
```

## License

Licensed under [Apache-2.0](LICENSE-APACHE).
