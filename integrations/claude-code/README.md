# ACS plugin for Claude Code

Bundles the ACS hooks, the ACS MCP server and the `acs-usage` skill for Claude Code.

- `hooks/hooks.json` runs `acs hook claude-code` on SessionStart, UserPromptSubmit, PreToolUse,
  PostToolUse, PostToolUseFailure and SessionEnd. The hooks deny a tool call that would overwrite
  a file another agent has claimed and tell Claude who holds it.
- `.mcp.json` registers `acs mcp --harness claude-code` as the MCP server `acs`.
- `skills/acs-usage/SKILL.md` tells the agent how to work in an ACS repository.

The `acs` binary must be on `PATH`. Without it the hooks fail open: Claude Code reports a hook
error and the tool call proceeds.

Try it from a checkout: `claude --plugin-dir ./integrations/claude-code`. To install it, add this
repository's marketplace (`claude plugin marketplace add <path or owner/repo>`) and run
`claude plugin install acs@acs-plugins`.

Adapter notes and payload shapes: `docs/adapters/claude-code.md`.

Licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE` at the repository root).
