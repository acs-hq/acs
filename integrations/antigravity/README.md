# ACS plugin for Antigravity

Installs ACS into Antigravity (the `agy` CLI and the app): a `PreToolUse` gate, post-tool
attribution, context injection at the first model call, the `acs` MCP server, an always-on rule and
the two skills. Formats, sources and the real runs: `docs/adapters/antigravity.md` (section 15).
Decisions: `docs/decisions/0014-antigravity-adapter.md` and `0024-antigravity-gate.md`.

| File | Purpose |
| --- | --- |
| `plugin.json` | Manifest. The name is lowercase kebab-case, as `agy plugin validate` requires. |
| `hooks.json` | `PreToolUse`, `PostToolUse` and `PreInvocation` hooks running `acs hook antigravity --phase ...`. No `Stop` hook (it fires at every turn end, not at session end) and no `SessionStart` hook (the first `PreInvocation` does its job, also where `SessionStart` may not fire). |
| `mcp_config.json` | Registers `acs mcp --harness antigravity`. Appears to the agent as server `acs_acs`, its tools as `mcp_acs_acs_acs_status` and so on. |
| `rules/acs.md` | Always-on rule: call `acs_status` / `acs_intent` first, follow denials. |
| `skills/acs-setup/`, `skills/acs-usage/` | Exact copies of the canonical skills in `skills/` (`crates/acs-cli/tests/packaging.rs` checks it). |

These files are built into the `acs` binary, so a change here reaches users with the next build.

## Install

`acs` must be on `PATH` first: with the hooks installed and no `acs` binary, Antigravity fails
closed and denies every tool call. From the repository:

```
acs plugin install antigravity
```

This writes the plugin to `.agents/plugins/acs/` in the repository and runs `agy plugin validate`
on it. Antigravity loads it at the next session start. Commit the folder; it is a sealed path, so
agents cannot change it afterwards.

For every repository instead, install this folder globally yourself:
`agy plugin install integrations/antigravity` (it goes to `~/.gemini/config/plugins/acs/`). Then
ACS gates every workspace Antigravity opens, with the built-in guards where a repository has no
`.acs/policy/`.
