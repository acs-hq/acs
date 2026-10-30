# Antigravity fixtures

Payloads for the Antigravity adapter (`crates/acs-adapters/src/antigravity.rs`), accessed
2026-10-30. Details, sources and gaps are in `docs/adapters/antigravity.md`.

**No real Antigravity run was possible** (Antigravity is not installed here, and the official
documentation host `antigravity.google` is blocked by this environment's network policy), so
none of these files is a byte-exact capture. Each is built from field names and values quoted in
the sources below. When a real capture becomes available, add it here as a new file before
changing adapter code (plan section 9.1).

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
| `out_pre_tool_ask_defer.json` | PreToolUse "no objection", deferring to Antigravity's own permission flow | unverified [S14][S13] |
| `out_pre_tool_allow.json` | PreToolUse explicit allow | quoted shape [S5][S9]; effect is build-dependent (see the doc) |
| `out_pre_invocation_inject.json` | PreInvocation context injection | quoted shape [S7][S11] |
| `out_empty_object.json` | PostToolUse / PostInvocation / Stop no-op. **Not safe for PreToolUse** on 1.4.1+, where `{}` denies | quoted [S7][S5] |
| `out_stop_continue.json` | Stop that keeps the agent running | quoted [S16] |

## Sources

`[S#]` identifiers are defined, with URLs and reliability notes, in the "Sources" section of
`docs/adapters/antigravity.md`.
