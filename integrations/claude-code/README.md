# ACS plugin for Claude Code

Bundles the ACS hooks, the ACS MCP server and the `acs-setup` and `acs-usage` skills for Claude Code.

- `hooks/hooks.json` runs `acs hook claude-code` on SessionStart, UserPromptSubmit, PreToolUse,
  PostToolUse, PostToolUseFailure and SessionEnd. The hooks deny a tool call that would overwrite
  a file another agent has claimed and tell Claude who holds it.
- `.mcp.json` registers `acs mcp --harness claude-code` as the MCP server `acs`.
- `skills/` holds copies of the canonical skills in the repository's `skills/` directory. Edit
  those and copy them here; a test fails if they differ.

The `acs` binary must be on `PATH`. Without it the hooks fail open: Claude Code reports a hook
error and the tool call proceeds.

Install it with `acs plugin install claude-code`, which enables it for the current repository. Try
it from a checkout with `claude --plugin-dir ./integrations/claude-code`.

Adapter notes and payload shapes: `docs/adapters/claude-code.md`.

Licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE` at the repository root).
