# Antigravity fixtures

Payloads for the Antigravity adapter (`crates/acs-adapters/src/antigravity.rs`), accessed
2026-10-30. Details, sources and gaps are in `docs/adapters/antigravity.md`.

The `real_*.json` files are **captures from a real `agy` 1.4.12 run** on Windows (2026-11-03,
decision 0024), recorded by a hook that saved its stdin. They are re-indented, and the workspace
(`C:/Users/dev/proj`) and home directory (`C:/Users/dev`) are replaced; nothing else is changed.
The other files were written before Antigravity could be run (2026-10-30) and are built from field
names and values quoted in the sources below. When the payload changes, add a new capture here
before changing adapter code (plan section 9.1).

## Real captures (`agy` 1.4.12)

| File | Event |
| --- | --- |
| `real_session_start.json` | SessionStart (ACS registers no hook for it) |
| `real_pre_invocation_first.json` | PreInvocation, first call of a new conversation (`invocationNum` 0, `initialNumSteps` 1) |
| `real_pre_invocation_second.json` | PreInvocation, second call of the same turn |
| `real_pre_invocation_resumed_turn.json` | PreInvocation, first call of a turn resumed with `--conversation` (`initialNumSteps` 4) |
| `real_pre_invocation_subagent_first.json` | PreInvocation, a subagent's first call (`initialNumSteps` 0, its own `conversationId`) |
| `real_post_invocation.json` | PostInvocation |
| `real_pre_tool_read_file.json` | PreToolUse `read_file {Path}` |
| `real_pre_tool_edit_file.json` | PreToolUse `edit_file {Path, Target, Replacement}` |
| `real_pre_tool_edit_file_backslash.json` | The same with a backslash path, as the model sometimes sends |
| `real_pre_tool_write_file.json` | PreToolUse `write_file {Path, Content}` |
| `real_pre_tool_run_command.json` | PreToolUse `run_command {CommandLine, WaitMsBeforeAsync, toolSummary}` |
| `real_pre_tool_mcp_acs_status.json` | PreToolUse for an MCP tool: `mcp_acs_acs_acs_status` |
| `real_pre_tool_invoke_subagent.json` | PreToolUse `invoke_subagent` |
| `real_post_tool_edit_file.json` | PostToolUse, success (`error` empty) |
| `real_post_tool_read_file_error.json` | PostToolUse with an `error` |
| `real_stop.json` | Stop (`terminationReason` `NO_TOOL_CALL`) |

Provenance legend: **quoted** = the shape and field names appear verbatim in a source;
**assembled** = field names quoted, but this exact object was put together here;
**unverified** = at least one field name or value rests on one third-party report.

## Hook input (stdin of the hook command)

All hook input files use the same common fields. Antigravity sends camelCase keys and does
**not** send the event name (the hook command line must carry it).

| File | Event | Provenance |
| --- | --- | --- |
| `pre_tool_run_command.json` | PreToolUse, Antigravity >= 1.4.3 `run_command` | assembled; `toolCall.{name,args.CommandLine}` quoted [S5][S8][S9] |
| `pre_tool_run_command_legacy_cwd.json` | PreToolUse, `run_command` as in <= 1.4.2 (has `Cwd`, `toolAction`) | assembled; `Cwd`/`toolAction` quoted [S10] |
| `pre_tool_run_command_windows.json` | PreToolUse, Windows (PowerShell command text, drive-letter paths) | assembled; shell dialect from [S1] |
| `pre_tool_write_file.json` | PreToolUse, `write_file {Path, Content}` (>= 1.4.3) | unverified [S9][S10] |
| `pre_tool_edit_file.json` | PreToolUse, `edit_file {Path, Target, Replacement}` (>= 1.4.3) | unverified [S10] |
| `pre_tool_write_to_file_legacy.json` | PreToolUse, `write_to_file {TargetFile, CodeContent, ...}` (<= 1.4.2) | assembled; arg names quoted [S10][S20] |
| `pre_tool_replace_file_content_legacy.json` | PreToolUse, `replace_file_content` (<= 1.4.2) | assembled; arg names quoted [S10][S20] |
| `pre_tool_multi_replace_file_content_legacy.json` | PreToolUse, `multi_replace_file_content` | unverified: `ReplacementChunks` shape from memory, not seen in any source |
| `pre_tool_view_file_legacy.json` | PreToolUse, `view_file {AbsolutePath}` (<= 1.4.2) | quoted [S10][S20] |
| `pre_tool_read_file.json` | PreToolUse, `read_file {Path, StartLine, EndLine}` (>= 1.4.3) | unverified [S9][S10] |
| `pre_tool_call_mcp_tool.json` | PreToolUse, MCP dispatch `call_mcp_tool {ServerName, ToolName, Arguments}` | quoted [S8][S10] (server name `acs_acs` follows the `<plugin>_<server>` rule [S1]) |
| `post_tool_write_file.json` | PostToolUse (adds `stepIdx`, `error`; **no tool result**) | quoted [S6][S11] |
| `post_tool_run_command_error.json` | PostToolUse with a non-empty `error` | assembled [S6][S11] |
| `pre_invocation.json` | PreInvocation (`invocationNum`, `initialNumSteps`) | quoted field names [S7][S12] |
| `post_invocation.json` | PostInvocation | assembled [S7][S12] |
| `stop.json` | Stop (`executionNum`, `terminationReason`, `fullyIdle`) | quoted field names [S12] |
| `mcp_tools_call_meta.json` | MCP `tools/call` request seen by an MCP server | the `_meta` key is quoted from the official changelog [S1] (1.4.4) |

## Hook output (stdout of the hook command)

| File | Used for | Provenance |
| --- | --- | --- |
| `out_pre_tool_deny.json` | PreToolUse deny; `reason` reaches the model | quoted [S5][S6][S8][S9] |
| `out_pre_tool_force_ask.json` | PreToolUse ask (forced human prompt, ignores the always-allow cache) | unverified: decision set reported in [S13][S14] |
| `out_pre_tool_ask_defer.json` | The "no objection" answer ACS used until decision 0024; no longer emitted | [S14][S13] |
| `out_pre_tool_allow.json` | PreToolUse "no objection": defers to Antigravity's own policy (what ACS emits since decision 0024) | official reference [S24]; measured in the CLI |
| `out_pre_invocation_inject.json` | PreInvocation context injection | quoted shape [S7][S11] |
| `out_empty_object.json` | PostToolUse / PostInvocation / Stop no-op. **Not safe for PreToolUse** on 1.4.1+, where `{}` denies | quoted [S7][S5] |
| `out_stop_continue.json` | Stop that keeps the agent running | quoted [S16] |

## Sources

`[S#]` identifiers are defined, with URLs and reliability notes, in the "Sources" section of
`docs/adapters/antigravity.md`.
