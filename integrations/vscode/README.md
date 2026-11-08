# ACS for VS Code

ACS (Agent Control System) coordinates the coding agents and people working in one repository.
This extension brings you into it from your editor:

- **Your edits are claimed for you.** Shortly after you start editing a file, the extension claims
  it as yours, so agents leave it alone until you are done. Once the file is saved and you have not
  touched it for two minutes (or you close it), the claim is released and agents may work on it
  again. An agent that tries meanwhile is told the file is yours, by name.
- **You see what others hold.** Files claimed by an agent or another person get a lock badge in
  the explorer and tabs, a marker in the gutter, and a hover saying who holds them, with which
  agent, and since when. The status bar counts the claims of others and yours; click it for the
  list.

ACS never blocks your editor. If you edit a file an agent holds, the extension warns you that your
edits may collide with the agent's.

## Requirements

- The `acs` command on `PATH` (or set `acs.path`). See the
  [ACS repository](https://github.com/acs-hq/acs) for installers.
- A repository set up with `acs init` (the extension starts when the workspace has
  `.acs/policy/config.toml`).

## Settings

| Setting | Default | What it does |
| --- | --- | --- |
| `acs.path` | `acs` | The `acs` program |
| `acs.claimOnEdit` | `true` | Claim files you edit |
| `acs.releaseAfterIdleSeconds` | `120` | Release a saved file's claim after this long without edits |
| `acs.refreshIntervalSeconds` | `5` | How often to ask `acs status` what others hold |

## Commands

- **ACS: Show Claims** lists every claimed file; pick one to open it.
- **ACS: Release My Claims** gives up all your claims in the open repositories.
- **ACS: Refresh** asks `acs status` now.

## How it works

The extension only runs the `acs` command line with `--json`: `acs status` every few seconds,
`acs claim` after your first edit of a file (and every five minutes while you keep it),
`acs release` when you are done. Claims are human claims of your seat, the same ones
`acs claim` makes in a terminal. Gitignored files and files under `.git/` are never claimed; files
sealed by the policy (which bind agents, not people) need no claim.

Editors without this extension can use `acs watch` in a terminal instead.

## License

MIT OR Apache-2.0.
