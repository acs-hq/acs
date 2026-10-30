# ACS plugin for Antigravity

Installs ACS into Antigravity (the `agy` CLI and the app): a `PreToolUse` gate, post-tool
attribution, per-model-call context injection, the `acs` MCP server, an always-on rule and a
usage skill. Formats, sources and open questions: `docs/adapters/antigravity.md`. Decisions:
`docs/decisions/0014-antigravity-adapter.md`.

| File | Purpose |
| --- | --- |
| `plugin.json` | Manifest. The name is lowercase kebab-case, as `agy plugin validate` requires. |
| `hooks.json` | `PreToolUse`, `PostToolUse` and `PreInvocation` hooks running `acs hook antigravity --phase ...`. No `Stop` hook (it fires at every turn end, not at session end). |
| `mcp_config.json` | Registers `acs mcp --harness antigravity`. Appears to the agent as server `acs_acs` (plugin MCP servers are named `<plugin>_<server>`). |
| `rules/acs.md` | Always-on rule: call `acs_status` / `acs_intent` first, follow denials. This is the tier B fallback and stays useful even when hooks work. |
| `skills/acs-usage/SKILL.md` | The usage etiquette. **Copy of the text WP5.1 will own**; keep in sync with the canonical skills when they land. |

## Install

`acs` must be on `PATH` first: with the hooks installed and no `acs` binary, Antigravity
fails closed and denies every tool call. `acs doctor` (WP2.5) checks this.

```
agy plugin validate --strict integrations/antigravity
agy plugin install integrations/antigravity
```

(or `/plugin` in the TUI, "Install from local directory...").

## Fallback: workspace hooks

Whether a deny from a **plugin-bundled** hook gates the tool has not been verified (one third
party reports plugin hooks as observe-only; the changelog suggests they gate). If a manual
check shows they do not, install the hooks at workspace level instead: copy the `acs` entry of
`hooks.json` to `<repo>/.agents/hooks.json` (the folder must be trusted) or to
`~/.gemini/config/hooks.json`, and keep the plugin for the MCP server, rule and skill.
`.agents/hooks.json` is a sealed path, so agents cannot edit it.
