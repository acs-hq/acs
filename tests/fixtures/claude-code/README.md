# Claude Code fixtures

Hook payloads (stdin of a hook command) and hook outputs (stdout) for Claude Code 2.1.314.
Spec and sources: `docs/adapters/claude-code.md`. One JSON file per case, named
`<event>.<tool or variant>.json`.

**Sanitising.** Real captures were taken on 2026-10-30 with `claude -p` in a throwaway repository.
The session id, prompt id, project path (`/home/dev/project`), transcript path and scratchpad path
were replaced by fixed values; the model name in `Agent` results is a placeholder. `tool_use_id`s are
the real ones. Nothing from the process environment was kept.

## Provenance

| File | Origin |
| --- | --- |
| `session_start.json` | real capture (`source: startup`) |
| `session_start.resume.json` | real capture (`--resume`) |
| `session_start.compact.json`, `session_start.clear.json` | **docs** (fields per hooks reference) |
| `user_prompt_submit.json` | real capture |
| `pre_tool_use.write.json`, `post_tool_use.write.json` | real capture |
| `pre_tool_use.edit.json`, `post_tool_use.edit.json` | real capture |
| `pre_tool_use.read.json`, `post_tool_use.read.json` | real capture |
| `pre_tool_use.bash.json`, `post_tool_use.bash.json` | real capture |
| `pre_tool_use.bash.mv.json`, `pre_tool_use.bash.rm.json` | real capture |
| `pre_tool_use.bash.subagent.json` | real capture (inside a subagent: has `agent_id`, `agent_type`) |
| `post_tool_use_failure.bash.json` | real capture (`error` is `Exit code 1\n...`) |
| `pre_tool_use.notebook_edit.json`, `post_tool_use.notebook_edit.json` | real capture |
| `pre_tool_use.mcp.json`, `post_tool_use.mcp.json` | real capture (`mcp_server.source` is `dynamic`; `tool_response` is an array) |
| `pre_tool_use.mcp_plugin.json` | real capture (plugin MCP server: `mcp__plugin_<plugin>_<server>__<tool>`, `source: plugin`) |
| `pre_tool_use.agent.json` | real capture |
| `pre_tool_use.skill.json` | real capture |
| `session_end.json` | real capture (`reason: other`) |
| `pre_tool_use.powershell.json` | **docs** |
| `pre_tool_use.write.windows.json` | **docs** (backslash path) |
| `pre_tool_use.glob.json`, `pre_tool_use.grep.json` | **docs** (tools absent by default on Linux/macOS) |
| `pre_tool_use.webfetch.json`, `pre_tool_use.monitor.json` | **docs** (Agent SDK tool input types) |
| `pre_tool_use.notebook_edit.insert.json` | **docs** (`cell_type`, `edit_mode`) |
| `output/pre_tool_use.deny.json` | shape verified by capture (tool blocked, reason reached the model); text is an example |
| `output/pre_tool_use.ask.json` | shape verified in `claude -p` (blocked, reason shown); interactive prompt per docs |
| `output/pre_tool_use.allow_with_context.json` | shape verified by capture (no `permissionDecision`) |
| `output/session_start.context.json`, `output/user_prompt_submit.context.json`, `output/post_tool_use.context.json` | shape verified by capture (`additionalContext` reached the model) |
| `output/empty.json` | no-op output (`{}`); an empty stdout is equivalent |

A deny is rendered as the `output/pre_tool_use.deny.json` object on stdout, the same message on
stderr, and exit code 2 (see the adapter document, section 6.1).
