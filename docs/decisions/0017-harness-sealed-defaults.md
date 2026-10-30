# 0017 — Harness configuration in the default sealed list

Date: 2026-10-30. Status: accepted (orchestrator).

## Context

Plan section 4.3 and decision 0011 §6: the default sealed list must cover every supported harness's
hook and settings files, so an agent cannot switch ACS off by editing its own configuration. The
WP2.2 (Claude Code) and WP2.3 (Antigravity) research, recorded in `docs/adapters/`, listed the
candidates.

## Decision

`DEFAULT_SEALED` grows from 6 to 12 globs:

- Claude Code: `.claude/settings*.json` becomes `**/.claude/settings*.json` (Claude Code reads
  project settings from the directory it was started in, which may be a subdirectory of a
  monorepo), plus `**/.claude/hooks/**`. Plugin enablement lives in the settings files.
- Antigravity: `.agents/**/hooks*.json` becomes `**/.agents/**/hooks*.json`, plus the legacy
  `.agent/` directory and plugin enablement: `**/.agent/**/hooks*.json`, `**/.agents/plugins/**`,
  `**/.agent/plugins/**`, `**/.agents/plugins.json`, `**/.agent/plugins.json`.

Deliberately **not** sealed by default:

- `.mcp.json` and `mcp_config.json`: MCP is not the gate; removing the ACS server does not
  disable hooks, and agents legitimately configure MCP servers.
- `.claude/**` as a whole (it holds worktrees and harness state), skills, agents, commands and
  rules directories (they weaken only the instruction layer, and sealing them blocks normal
  work); repositories can add them in `guards.toml`.
- The repository-root `.antigravity-plugin/` and `.gemini-plugin/` manifest directories: they
  matter only when the repository itself is installed as a plugin.

`CACHE_FORMAT` in `acs-policy` goes from 1 to 2 so cached policies pick up the new defaults.

## Amendment (same day, after WP2.4)

`.husky/**` is added (13 globs, `CACHE_FORMAT` 3). The git backstop (decision 0015) writes its
block into husky's committed `.husky/<hook>` scripts; default git hooks under `.git/` were already
sealed, and leaving `.husky/` writable would let an agent remove the backstop silently. Agents in
husky repositories ask their pilot to change those hooks.

## Consequences

- Files outside the repository (`~/.claude/settings.json`, `~/.gemini/config/hooks.json`, ...)
  cannot be sealed by repository globs; writes there get `path_escapes_repo` (ask) from file
  tools, and shell writes there are a known gap for the threat model (WP10.3) and WP7.2.
- An agent can still start a nested harness with hooks disabled (`claude --bare`, ...); WP7.2
  should classify agent CLIs as unknown-target commands.
