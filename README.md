<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/acs-logo.svg">
    <img src="docs/assets/acs-logo-on-light.svg" alt="ACS" width="300">
  </picture>
</p>

<h3 align="center">Rules for any agent that works in your repository</h3>

<p align="center">
  ACS, the Agent Control System, checks every tool call a coding agent makes, before it runs.<br>
  One writer per file. Paths no agent may touch. A record of every session, and a way to take one back.
</p>

<p align="center">
  <a href="#get-started">Get started</a> ·
  <a href="#what-you-get">What you get</a> ·
  <a href="#commands">Commands</a> ·
  <a href="docs/wiki/Home.md">Guide</a> ·
  <a href="#what-acs-does-not-do">Limits</a>
</p>

---

## The problem

Open two agent sessions in one repository and neither knows the other exists. Both edit the same
file, and the last one to write wins. The other session's work is gone, and often nobody notices
until much later. Add teammates with their own agents and it happens more often.

Agents also do things nobody asked for: `git push --force`, a stray `rm -rf`, an edit to your CI
workflow or a `.env` file.

## What ACS does

ACS sits between the agent and your repository. Your agent harness calls one small `acs` program
on every tool call, and it answers **allow**, **deny** or **ask the person**, the same way every
time and in a few milliseconds. No model is involved and nothing is sent anywhere.

When it refuses, it tells the agent why and what to do instead:

```text
src/api.ts is claimed by sara@example.com (claude-code, active just now).
Work on other files, or ask your pilot to coordinate.
```

The agent moves on to other work. You hear from ACS only when something was refused.

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
acs plugin install claude-code   # or: antigravity
acs init
acs doctor
```

`acs plugin install` enables the plugin for this repository. With `--scope user` it gates every
repository you open, even those that never adopted ACS. `acs init` writes the policy to
`.acs/policy/`, installs the git hooks and is safe to run again. In `acs doctor`, look for
`0 failure(s)`.

Commit `.acs/` and the plugin setting from your own terminal and merge them to your main branch.
Teammates install the program, pull, and run `acs init` once in their clone.

</details>

## What you get

| | Feature | What it does |
| --- | --- | --- |
| 🔒 | **Claims** | One writer per file. The first session to edit a file holds it until it goes quiet or ends. Other sessions are told who holds it. |
| 🛑 | **Guards** | Paths no agent may write (`.git/**`, `.env*`, CI workflows, your own list), commands no agent may run, protected branches, and a ceiling on what one command may delete. |
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

$ acs undo ses_4k2                 # shows the plan, changes nothing
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
| `acs log` · `acs blame` · `acs undo` | See past sessions, who wrote a line, and take a session back |
| `acs verify` · `acs stats` | Check the records are unaltered. Count what ACS prevented. |
| `acs uninstall` | Take ACS out of a repository |

Every command takes `--json` and `--help`.

## Works with

| Harness | |
| --- | --- |
| Claude Code | Supported |
| Antigravity | Supported |
| Codex | Coming |
| VS Code | An extension claims the files you edit and marks the ones others hold |

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

Licensed under either of MIT or [Apache-2.0](LICENSE-APACHE) at your option.
