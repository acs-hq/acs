# ACS for Codex

`config.toml` is the block `acs plugin install codex` adds to Codex's configuration: a
`SessionStart`, `PreToolUse`, `PostToolUse` and `SessionEnd` hook running `acs hook codex`, and
the `acs` MCP server (`acs mcp --harness codex`), whose tools Codex lists as
`mcp__acs__acs_status`, `mcp__acs__acs_intent` and so on. Formats and real runs:
`docs/adapters/codex.md`. Decision: `docs/decisions/0047-codex-adapter.md`. Checked with Codex CLI
0.170.0 on Windows.

The block is built into the `acs` binary, so a change here reaches users with the next build.

## Install

`acs` must be on `PATH` first. From the repository:

```
acs plugin install codex
```

This appends the block to `.codex/config.toml` (created if missing), between the lines
`# >>> acs` and `# <<< acs`. `--scope user` puts it in `$CODEX_HOME/config.toml` instead
(default `~/.codex/config.toml`), which gates every repository you open in Codex. Commit
`.codex/config.toml` so teammates get it too.

Then start `codex` in the repository once:

1. If Codex asks whether to trust the folder, say yes: Codex reads a project's `.codex/` only in
   a trusted project.
2. Open `/hooks`, review the four ACS hooks and trust them. Codex skips a hook nobody has trusted,
   and asks again when the hook changes.

Until both are done, nothing is checked. Without `acs` on `PATH`, the hooks fail open: Codex
reports a failed hook and runs the tool call unchecked.

`acs uninstall` removes the block again, and deletes `.codex/config.toml` if the block was all it
held.
