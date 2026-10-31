# Claude Code adapter notes

Status: WP2.2 complete (research, adapter, plugin, acceptance run), Windows runs added 2026-10-31
(section 18). Written 2026-10-30 against Claude Code **2.1.314** (`claude --version` on the research
machine). The adapter
(`crates/acs-adapters/src/claude_code.rs`) and the plugin (`integrations/claude-code/`) match this
document; if Claude Code changes, update this file first. Choices made beyond the plan and spec are
in `docs/decisions/0013-claude-code-adapter.md`.

Everything below is marked as one of:

- **[captured]** observed by running the real `claude` CLI 2.1.314 with hooks configured
  (`claude -p`, non-interactive) on 2026-10-30.
- **[docs]** taken from the official documentation, not observed.
- **[untested]** inferred; the text says why it could not be tested.

## 1. Sources

All pages accessed 2026-10-30 (the `.md` variant of each page was downloaded).

| Topic | URL |
| --- | --- |
| Hooks reference (events, input, output, exit codes) | <https://code.claude.com/docs/en/hooks> |
| Hooks guide | <https://code.claude.com/docs/en/hooks-guide> |
| Plugins overview | <https://code.claude.com/docs/en/plugins> |
| Create a plugin | <https://code.claude.com/docs/en/plugins/create> |
| Plugin components (hooks, MCP, skills in a plugin) | <https://code.claude.com/docs/en/plugins/components> |
| Plugin manifest reference (`plugins-reference` redirects here) | <https://code.claude.com/docs/en/plugins/manifest-reference> |
| Install and manage plugins | <https://code.claude.com/docs/en/plugins/install> |
| Marketplace reference / create a marketplace | <https://code.claude.com/docs/en/plugins/marketplace-reference>, <https://code.claude.com/docs/en/plugins/create-marketplace> |
| MCP | <https://code.claude.com/docs/en/mcp> |
| Settings files and precedence | <https://code.claude.com/docs/en/settings> |
| Settings reference | <https://code.claude.com/docs/en/settings-reference> |
| Environment variables | <https://code.claude.com/docs/en/env-vars> |
| Tools reference | <https://code.claude.com/docs/en/tools-reference> |
| Tool input types (Agent SDK) | <https://code.claude.com/docs/en/agent-sdk/typescript> |
| Permission modes, protected paths | <https://code.claude.com/docs/en/permission-modes> |
| Skills | <https://code.claude.com/docs/en/skills> |

## 2. Verdicts

- **Plan risk 9.2 ("a harness can't deny a tool call or inject context from a hook"): does not
  apply to Claude Code.** A `PreToolUse` hook can deny a call and the reason reaches the model
  **[captured]**; `additionalContext` is accepted from `SessionStart`, `UserPromptSubmit`,
  `PreToolUse` and `PostToolUse` and reaches the model **[captured]**. Claude Code is tier A.
- Denial holds in every permission mode including `bypassPermissions` **[captured]**, and in
  subagents (their tool calls fire the same hooks, with `agent_id`) **[captured]**.
- **Fail-open by default.** A hook that cannot start, times out, exits with a code other than 0
  or 2, or prints malformed JSON does not block the tool call **[captured for "cannot start"]**.
  `"onFailure": "block"` (Claude Code 2.1.295+) turns this into fail-closed **[captured]**.
  See section 8.
- The session id is available to MCP servers through `CLAUDE_CODE_SESSION_ID` **[captured]**,
  with one documented and observed exception on `--continue` (section 9).

## 3. How hooks are configured

A hook configuration has three levels: event, matcher group, handler. In `settings.json` it is
the `"hooks"` object; in a plugin it is `hooks/hooks.json` with the same object under a top-level
`"hooks"` key **[docs]**:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "hooks": [
          { "type": "command", "command": "acs", "args": ["hook", "claude-code"], "timeout": 20 }
        ]
      }
    ]
  }
}
```

- **Exec form** (`command` + `args`): no shell, `args` elements are passed verbatim, `command` is
  resolved on `PATH` **[docs]**, **[captured]** (`command: "python3"`, `args: [...]` and the
  "Executable not found in $PATH" error for a missing binary). It avoids quoting problems on
  Windows. The docs say `command` must resolve to a real executable on Windows, but a `.cmd` shim
  worked in 2.1.316 (section 18). The plugin uses exec form.
- **Matchers**: omitted, `""` or `"*"` match everything. A value of only letters, digits, `_`,
  `-`, space, `,` and `|` is an exact name list; anything else is an unanchored JavaScript regex.
  Tool events match `tool_name`; `SessionStart` matches `source`; `SessionEnd` matches `reason`.
  `UserPromptSubmit` has no matcher support **[docs]**. ACS registers no matcher on any event, so
  it sees every tool, including MCP tools.
- **Timeouts**: default 600 s for command hooks; 30 s for `UserPromptSubmit`. `SessionEnd` hooks
  share a **1.5 s budget**, and a timeout set on a *plugin-provided* hook does not raise it
  (`CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS` does) **[docs]**. `acs hook claude-code` on
  `session_end` must therefore finish in well under 1.5 s.
- **Parallelism**: all matching hooks for an event run in parallel; the same handler defined at
  several settings levels runs once **[docs]**. When several `PreToolUse` hooks disagree the
  precedence is `deny` > `defer` > `ask` > `allow` **[docs]**.
- **Where hooks come from**: `~/.claude/settings.json`, `.claude/settings.json`,
  `.claude/settings.local.json`, managed settings, plugin `hooks/hooks.json`, skill and subagent
  frontmatter. Entries merge across levels instead of replacing each other **[docs]**.
- **Disabling**: `"disableAllHooks": true` in any settings file turns off user, project, local and
  plugin hooks (managed hooks keep running); `--settings '{"disableAllHooks":true}'` and
  `--bare` skip hooks for one run **[docs]**.
- `if` (permission-rule filter), `async`, `once`, `statusMessage`, `shell`, `onFailure` are
  further optional handler fields **[docs]**. ACS uses none except possibly `onFailure`.

Events ACS registers: `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`,
`PostToolUseFailure`, `SessionEnd`. Events deliberately not registered: `Stop`, `StopFailure`,
`SubagentStart`, `SubagentStop`, `PostToolBatch`, `PermissionRequest`, `PermissionDenied`,
`Notification`, `PreCompact`, `PostCompact`, `CwdChanged`, `FileChanged`, `InstructionsLoaded`,
`ConfigChange` and the rest. (`SessionStart` with `source: "compact"` is the compaction hook we
need; see section 9.)

## 4. Input payloads

Hooks receive one JSON object on stdin **[docs]**, **[captured]**. Fixtures live in
`tests/fixtures/claude-code/` (section 15 lists provenance of each file).

### 4.1 Common fields

| Field | Seen on | Notes |
| --- | --- | --- |
| `session_id` | all | The harness session id; the ToolEvent `harness_session_id`. A UUID |
| `transcript_path` | all | Path to the conversation JSONL. Do not read it; it lags |
| `cwd` | all | Working directory when the hook fired. Follows `cd` and worktree entry **[docs]** |
| `scratchpad_dir` | all (2.1.257+) | Absent when there is no scratchpad |
| `hook_event_name` | all | `SessionStart`, `PreToolUse`, ... selects the phase |
| `prompt_id` | after the first prompt | UUID of the current prompt |
| `permission_mode` | tool events, `UserPromptSubmit` | `default`, `plan`, `acceptEdits`, `auto`, `dontAsk`, `bypassPermissions` |
| `effort` | tool events | `{"level": "low"..."max"}` |
| `agent_id`, `agent_type` | inside a subagent | **[captured]**: subagent tool calls carry the **same `session_id`** as the parent plus these two fields |

Unknown extra fields appear regularly across versions; the adapter must ignore them.

### 4.2 Per event

| Event | Extra fields | ToolEvent phase |
| --- | --- | --- |
| `SessionStart` | `source` (`startup`, `resume`, `clear`, `compact`, `fork`); optional `model`, `agent_type`, `session_title`; on resume/fork also `seconds_since_last_response`, `context_tokens`, `prompt_cache_likely_expired`, `estimated_cache_write_usd` | `session_start` |
| `UserPromptSubmit` | `prompt`; optional `session_title` | `prompt_submit` (`prompt` must not be persisted unless `ledger.store_prompts`) |
| `PreToolUse` | `tool_name`, `tool_input`, `tool_use_id`; MCP tools also `mcp_server: {name, source}` (2.1.274+) | `pre_tool` |
| `PostToolUse` | `tool_name`, `tool_input`, `tool_response`, `tool_use_id`, `duration_ms`; MCP tools also `mcp_server` | `post_tool`, `result.success = true` |
| `PostToolUseFailure` | `tool_name`, `tool_input`, `tool_use_id`, `error`, `is_interrupt`, `duration_ms` | `post_tool`, `result.success = false` |
| `SessionEnd` | `reason` (`clear`, `resume`, `logout`, `prompt_input_exit`, `other`) | `session_end` |

Notes that matter for the adapter:

- `tool_input.file_path` / `notebook_path` is **always absolute** for file tools (Claude Code
  expands `~` and relative paths before hooks run) **[docs]**. On Windows it uses backslashes
  (`C:\project\src\index.ts`, fixture `pre_tool_use.write.windows.json`) **[docs]**; the pipeline
  normalises, not the adapter.
- `PostToolUse` carries no exit code for Bash/PowerShell: `tool_response` is
  `{stdout, stderr, interrupted, isImage, noOutputExpected}` **[captured]**. A command that exits
  non-zero fires `PostToolUseFailure` instead **[captured]** with
  `error = "Exit code N\n<output>"` (first line `Exit code N` is documented as the stable part;
  the rest is display text). So: `PostToolUse` Bash gives `exit_code = Some(0)`;
  `PostToolUseFailure` parses `^Exit code (\d+)`, else `None`.
- `tool_response` is tool-specific and can be large (Read returns file content; Write returns
  `content`, `structuredPatch`, `gitDiff`). The adapter must not copy it into `raw` unbounded.
  Write/Edit responses include `gitDiff` and `structuredPatch`, which are *not* relied on.
- `PostToolUse` for an MCP tool has `tool_response` as an **array** of content blocks
  **[captured]**, not an object.
- `PostToolBatch.tool_calls[].tool_response` is a plain string **[captured]**; not used.
- `PostToolUse` does not fire for denied calls, and `PostToolUseFailure` does not fire for
  permission denials or validation rejections **[docs]**.
- `@file` references in a prompt are inlined without any tool call, so no `PreToolUse` for them
  **[docs]**. Reads are not gated by ACS anyway.
- Subagents: calls made by a subagent fire the same hooks in the **same session**; `agent_id`
  distinguishes them **[captured]**. ACS uses one seat per `session_id`, so subagents share the
  parent's seat and claims.

## 5. Tool names and mapping to `ToolEvent.tool`

Built-in tool names (exact strings used in hooks) are listed in the tools reference **[docs]**.
`MultiEdit` and `NotebookRead` no longer appear in the current tool list; the adapter still maps
them defensively for older versions.

`tool.name` is the raw `tool_name` string. `tool.kind` is chosen by this table. Unknown names are
`other`: every built-in tool of 2.1.314 that can change files is listed below, and SPEC section 2
only asks that an *unclassifiable file-changing* tool is never `read`.

| Tool | kind | paths (`ToolEvent.paths`) | edit (`old_text`, `new_text`) | command | Source |
| --- | --- | --- | --- | --- | --- |
| `Write` | `write` | `[tool_input.file_path]` | none | | [captured] |
| `Edit` | `edit` | `[tool_input.file_path]` | `old_string`, `new_string` (`replace_all` is not representable) | | [captured] |
| `MultiEdit` (legacy) | `edit` | `[tool_input.file_path]` | none (`edits[]` has several pairs) | | [docs: legacy] |
| `NotebookEdit` | `edit` | `[tool_input.notebook_path]` | none (cell-level: `cell_id`, `new_source`, `cell_type`, `edit_mode` replace/insert/delete) | | [captured] |
| `Bash` | `shell` | none (pipeline derives targets from the command) | | `tool_input.command` | [captured] |
| `PowerShell` | `shell` | none | | `tool_input.command` | [docs] |
| `Monitor` | `shell` when `tool_input.command` is present, else `other` (`ws` source) | none | | `tool_input.command` | [docs] |
| `Read` | `read` | `[tool_input.file_path]` | | | [captured] |
| `NotebookRead` (legacy) | `read` | `[tool_input.notebook_path]` | | | [docs: legacy] |
| `Glob` | `read` | `[tool_input.path]` when present | | | [docs] |
| `Grep` | `read` | `[tool_input.path]` when present | | | [docs] |
| `WebFetch`, `WebSearch` | `other` | none | | | [docs] |
| `Agent` (older name `Task`) | `other` (the subagent's own calls arrive as separate events) | none | | | [captured] |
| `Workflow` | `other` (orchestrates subagents; their calls arrive separately) | none | | | [docs] |
| `Skill`, `ToolSearch`, `AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`, `TaskCreate`/`TaskGet`/`TaskList`/`TaskUpdate`/`TaskStop`, `TodoWrite`, `Cron*`, `ScheduleWakeup`, `SendMessage`, `ListAgents`, `LSP`, `ListMcpResourcesTool`, `ReadMcpResourceTool`, `WaitForMcpServers`, `EnterWorktree`, `ExitWorktree`, `PushNotification`, `SendUserFile`, `SendFeedback`, `RemoteTrigger`, `Artifact`, ... | `other` | none | | | [captured: Skill, ToolSearch], [docs] rest |
| `mcp__<server>__<tool>`, `mcp__plugin_<plugin>_<server>__<tool>` | `other` | none | | | [captured] |

Remarks:

- On Linux and macOS, `Glob` and `Grep` are **absent by default**; searches arrive as `Bash`
  (`find`, `grep` run through embedded `bfs`/`ugrep`) **[docs]**. They return when the user passes
  `--tools`/`--allowedTools` naming them or removes Bash.
- Shell dialect selection (decision 0011 section 3) works from `tool.name`: `Bash` is bash,
  `PowerShell` is PowerShell. On Windows without Git Bash Claude Code registers only `PowerShell`
  **[docs]**, so a `Bash`-only matcher would miss it; ACS registers no matcher.
- `EnterWorktree` changes the session's directory; later events carry the new `cwd` **[docs]**.
  `${CLAUDE_PROJECT_DIR}` stays at the original project root. The pipeline must resolve the
  repository from `cwd`, not from `CLAUDE_PROJECT_DIR`.
- `Edit` on a notebook, `Write` over an existing file, and `NotebookEdit` with `edit_mode: delete`
  all change files; `kind` is `write`/`edit` for all of them. `Write` over an existing file is
  `write` (the pipeline decides create vs overwrite from the file system).
- Plugin-provided MCP tools (including ACS's own) have the form
  `mcp__plugin_<plugin>_<server>__<tool>` and a PreToolUse `mcp_server` of
  `{"name": "plugin:<plugin>:<server>", "source": "plugin"}` **[captured]**. They are `other`
  and must always be allowed (no paths, no command).

## 6. Output formats

A hook answers with an exit code, stdout and stderr **[docs]**:

- **Exit 0**: stdout is parsed as JSON only if it is exactly one JSON object (whitespace around it
  allowed). Any other stdout is plain text: ignored on most events, added as context on
  `UserPromptSubmit`, `UserPromptExpansion`, `SessionStart`, `PostModelSwitch`. Malformed JSON
  that starts with `{` and ends with `}` is a non-blocking error.
- **Exit 2**: blocking error on events that can block (`PreToolUse`: blocks the call;
  `UserPromptSubmit`: rejects the prompt). stderr is the reason. JSON on stdout is still read but
  cannot override the block.
- **Any other exit code** (including 1): non-blocking error, the action proceeds. Never use 1.
- Stderr of an exit-0 hook is never shown to the model.

### 6.1 `PreToolUse` outcomes

| ACS outcome | Hook output | Verified |
| --- | --- | --- |
| allow, nothing to say | exit 0, empty stdout | [captured] (tool ran, normal permission flow applied) |
| allow with `context`/`warnings` | exit 0, `{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"..."}}` (**no** `permissionDecision`) | [captured]: model saw the context next to the tool result |
| deny | stdout `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"<message>"}}`, **stderr = same message, exit 2** | [captured]: tool blocked, JSON reason reached the model (`PreToolUse:Write hook error: <reason>`); with exit 2 and no JSON the model sees `PreToolUse:Write hook error: [<command>]: <stderr>`; with both, the JSON reason is used |
| ask | stdout `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"<message>"}}`, exit 0 | [captured] in `claude -p` (no human to ask): treated as a denial and the reason reached the model, also under `bypassPermissions`. Interactive prompt [docs] (reason shown to the user, labelled `[plugin:<name>]`); [untested] interactively |
| queue (reserved) | treat as deny | spec rule |
| `unknown_payload` (adapter could not parse) | allow: exit 0, empty stdout | spec section 11 |

Design rules that follow:

1. **Never emit `permissionDecision: "allow"`.** `allow` skips the user's permission prompt
   **[docs]**; ACS must not widen what the user approved. Omit the field and let Claude Code's
   normal flow run. (Deny and ask *rules* in the user's settings are evaluated regardless of a
   hook's allow **[docs]**, but prompts are not.)
2. **Deny with JSON and exit 2 together.** If the JSON is somehow malformed, exit 2 still blocks
   and stderr carries the reason. Verified combination above. Deny output must also work when the
   engine itself failed in a fail-closed branch.
3. **stdout carries one JSON object and nothing else.** Anything else printed on stdout (logs,
   banners, a stray `println!`) turns a deny into a non-blocking error and the call proceeds.
   `acs hook` must log to stderr only.
4. Messages in `permissionDecisionReason` are shown to the model verbatim, prefixed by Claude
   Code. Fit them to the guidance in SPEC 3.3. Context strings should be **factual statements,
   not imperative system-style instructions**: the docs warn that out-of-band command phrasing can
   trigger the model's prompt-injection defences, which makes it surface the text to the user
   instead of using it.
5. `additionalContext` is capped at 10,000 characters per string (longer text is written to a file
   and replaced by a path and a 2,000-character preview). Keep context to a few lines.
6. `updatedInput` (rewrite the call) is **never** used by ACS: it replaces the whole input and
   would let a hook change what ACS validated. Note the reverse risk in section 14.

### 6.2 Other events

| Phase | Output | Verified |
| --- | --- | --- |
| `session_start` | `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"..."}}`, or plain stdout text. Also accepts `initialUserMessage` (only `-p`), `sessionTitle`, `watchPaths`, `reloadSkills`; no blocking. | [captured] context reached the model |
| `prompt_submit` | `{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"..."}}`, or plain stdout. A top-level `{"decision":"block","reason":"..."}` would reject the prompt (the reason goes to the user, not the model); ACS never blocks prompts | [captured] context reached the model |
| `post_tool` | `{"hookSpecificOutput":{"hookEventName":"PostToolUse" or "PostToolUseFailure","additionalContext":"..."}}`. The tool already ran: nothing can be denied. Exit 2 shows stderr to the model but ACS does not use it | [captured] context reached the model (`PostToolUse`); `PostToolUseFailure` [docs] |
| `session_end` | print nothing; output is discarded (including `systemMessage`) | [docs] |

`warnings` from a `Decision` are rendered as `additionalContext` lines (SPEC section 3 says
adapters render them as context). `systemMessage` shows a warning to the **user** only (not the
model); ACS may use it for state-unavailable and similar operator-facing warnings.

### 6.3 Context injection per event

| Event | `additionalContext` supported | Reaches model | Notes |
| --- | --- | --- | --- |
| `SessionStart` | yes | yes [captured] | at conversation start, before the first prompt; runs again on resume/clear/compact (`source`), so it can refresh |
| `UserPromptSubmit` | yes | yes [captured] | alongside the submitted prompt |
| `PreToolUse` | yes (allow path) | yes [captured] | next to the tool result. Ignored when `permissionDecision` is `defer`. On `deny` the reason is the message; do not rely on `additionalContext` there |
| `PostToolUse` | yes | yes [captured] | next to the tool result |
| `PostToolUseFailure` | yes | [docs] | |
| `Stop`, `SubagentStop`, `SubagentStart`, `PostToolBatch`, ... | yes | [docs] | not used |
| `SessionEnd` | no | n/a | |

## 7. Environment variables

| Variable | Hooks | MCP stdio servers | Notes |
| --- | --- | --- | --- |
| `CLAUDE_CODE_SESSION_ID` | yes [docs] | **yes [captured]** | hook value equals `session_id`, updated on `/clear`. The MCP server keeps the id it was spawned with (section 9) |
| `CLAUDE_PROJECT_DIR` | yes [captured] | yes [captured] | project root where the session started; **does not follow** worktree entry or `cd` |
| `CLAUDE_PLUGIN_ROOT` | yes [captured] (plugin hooks) | yes [captured] (plugin servers) | install dir of the plugin version; changes on update, do not store state |
| `CLAUDE_PLUGIN_DATA` | yes [captured] | yes [docs] | `~/.claude/plugins/data/<id>/`, persists across updates |
| `CLAUDE_PLUGIN_OPTION_<KEY>` | yes [docs] | | one per `userConfig` value |
| `CLAUDE_ENV_FILE` | `SessionStart` hooks only [captured] | no | file to append `export` lines to for later Bash commands |
| `CLAUDECODE=1` | yes | yes [captured] | |
| `CLAUDE_CODE_CHILD_SESSION=1` | yes [docs] | **not set** [docs] | |
| `CLAUDE_CODE_REMOTE=true` | cloud sessions [docs] | | |
| `CLAUDE_CODE_BRIDGE_SESSION_ID` | when Remote Control is connected [docs] | | not a hook session id |
| `CLAUDE_EFFORT` | yes [docs] | | |

There is no `CLAUDE_MODEL`. Hooks inherit Claude Code's environment (minus `OTEL_*`), so
`ACS_HOME`, `PATH` and `HOME` are those of the user who launched `claude`. The `PATH` that exec-form
hooks use to find `acs` is Claude Code's own; a plugin's `bin/` directory is added to the **Bash
tool's** `PATH` only, not to hook processes **[docs]**.

The research machine runs inside another Claude Code session; its environment carried many
`CLAUDE_CODE_*` variables (including tokens). The captured fixtures contain none of them: only
hook stdin was kept.

## 8. Exit codes and failure semantics (what ACS can rely on)

| Situation | Claude Code behaviour | Verified |
| --- | --- | --- |
| `acs` not on `PATH` (exec form) | non-blocking error, **tool runs** | [captured] (`PreToolUse:Write hook error` is only a notice) |
| same, with `"onFailure": "block"` | tool blocked; message `failed; blocking because onFailure is "block"` plus `Executable not found in $PATH` | [captured] |
| hook times out | hook cancelled, output discarded, tool runs (unless `onFailure: block`) | [docs] |
| exit code not 0 or 2 | non-blocking (unless `onFailure: block`) | [docs] |
| invalid JSON on stdout with exit 0 | non-blocking error | [docs] |
| exit 2 | blocks `PreToolUse` and `UserPromptSubmit`; stderr is the reason | [captured] |
| hook starts a background child and leaves its stdout/stderr attached | Claude Code keeps reading the hook output; a deny printed after it stops reading is ignored, and an allow followed by an attached child makes Claude Code **block the call** with `refusing to honor the suspect capture` | [docs] |

`onFailure` requires Claude Code 2.1.295 or later. How an older version treats an unknown
`onFailure` key in a plugin's `hooks.json` was not tested.

Decision (recorded in decision 0013): the plugin does **not** set
`onFailure: "block"` by default, because with the binary missing every tool call (including the
`Bash` call that installs `acs`) would be blocked and the setup skill could not recover. ACS's own
hard guarantees come from sealed paths and the git backstop (decision 0011). `acs init` may offer a
strict mode that writes a project-level hook entry with `onFailure: "block"` for `PreToolUse`.

**Background children.** SPEC section 9 has post-tool spawn a detached `acs sync`. That child must
be started with stdin, stdout and stderr redirected to null (and, on Windows, detached from the
console), or Claude Code can block or wait on it. This applies to WP2.1 (pipeline) and WP3.1
(sync). Contract note for WP2.1: `acs hook` must never leave an inherited pipe open in a child.

## 9. Session identity, lifecycle and MCP seat binding

Facts **[captured]** unless noted:

- `session_id` in hook stdin is stable for a session and identical for the main agent and its
  subagents.
- `--resume <id>`: `SessionStart` has `source: "resume"` and the same `session_id`; the MCP
  server's `CLAUDE_CODE_SESSION_ID` equals it.
- `--continue`: `SessionStart` has `source: "resume"` and the resumed `session_id`, but the MCP
  server's `CLAUDE_CODE_SESSION_ID` was the **startup id, not the resumed one** (it differed from
  the hook `session_id` in the capture). The docs state the same: "On `--continue` or `--resume`
  without an explicit ID it may receive the initial startup ID instead".
- `/clear` creates a new session id; `SessionEnd` (`reason: "clear"`) fires for the old id and
  `SessionStart` (`source: "clear"`) for the new one **[docs]**. The MCP server keeps the old id
  **[docs]**.
- `SessionStart` with `source: "compact"` fires after compaction in the **same** session **[docs]**;
  compaction drops earlier context, so this is the moment to re-inject ACS status.
- `SessionEnd` fires at the end of every `claude -p` run too, with `reason: "other"`; resuming that
  session later reuses the same `session_id`.
  **Consequence for WP2.1:** "one seat per harness session, ended at session end" collides with
  resume. Resume must reopen the seat (or create a new seat for the same `harness_session_id`);
  a `session_start` event for a session whose seat exists and is live is a heartbeat plus context
  refresh.
- Events from a Claude Code session can arrive without a preceding `SessionStart` (hooks added
  mid-session, plugin enabled with `/reload-plugins`, crashed earlier `SessionStart`). The
  pipeline already creates the seat on the first event of any phase (SPEC section 4.1).

**MCP seat binding (decision 0011 section 5).** In order:

1. `CLAUDE_CODE_SESSION_ID` from the MCP server's environment: use it as `harness_session_id`.
   Correct for fresh sessions and `--resume <id>`; **stale after `/clear` and on `--continue`**.
2. Because (1) can be stale, WP2.6 must validate it: if no live seat of harness `claude-code`
   has that `harness_session_id`, fall through.
3. Fallback: most recently seen live `claude-code` seat whose worktree is the server's work tree
   (`CLAUDE_PROJECT_DIR`, else the server's cwd).
4. A second correlation channel exists: every MCP `tools/call` carries
   `params._meta["claudecode/toolUseId"]`, equal to the `tool_use_id` of the `PreToolUse` event for
   that call **[captured]**. The hook pipeline records `tool_use_id` for MCP tool pre-events on
   `mcp__plugin_acs_*`, so `acs mcp` can bind the exact seat per call even when (1) is stale. This is
   optional hardening; it needs the pipeline to persist `(tool_use_id -> seat)` briefly.
   The MCP `initialize` params carry no session info (clientInfo is
   `{"name":"claude-code","version":"2.1.314"}` and `roots` capability).

`CLAUDE_PROJECT_DIR` is set in the MCP server's environment too, so the server can find the project
without relying on cwd **[captured]**.

## 10. Plugin layout

A plugin is a directory with an optional manifest **[docs]**, loaded and verified by running a
throwaway plugin with `claude --plugin-dir` **[captured]** (hooks from `hooks/hooks.json` fired,
the plugin MCP server started and was callable, the plugin skill ran as `acs-demo:acs-usage`).

```
integrations/claude-code/            # the plugin root (WP2.2 stage 2)
  .claude-plugin/plugin.json         # manifest; only name is required
  hooks/hooks.json                   # {"hooks": {...}} as in section 3
  .mcp.json                          # {"mcpServers": {"acs": {"command": "acs", "args": ["mcp", "--harness", "claude-code"]}}}
  skills/acs-setup/SKILL.md          # skill namespaced as /acs:acs-setup (plugin name acs)
  skills/acs-usage/SKILL.md
  bin/                               # optional; on the Bash tool's PATH only
  README.md, LICENSE                 # `claude plugin validate` warns without them
```

- Manifest `.claude-plugin/plugin.json`: `name` (kebab-case, required), `version`, `description`,
  `author`, `license`, `homepage`, `repository`, component path overrides (`hooks`, `mcpServers`,
  `skills`, ...; each path starts with `./` and stays inside the plugin root). Unknown top-level
  keys are stripped with a validation warning. Components in default locations need no manifest
  entry. `hooks/hooks.json` and a manifest `hooks` key both load and merge **[docs]**.
- `claude plugin validate ./integrations/claude-code` checks the manifest, hooks and MCP entries
  (**[captured]**: passed with README/LICENSE/icon warnings only; `--strict` turns warnings into
  failures).
- **Names.** Skills are namespaced by the plugin name (`/<plugin>:<skill>`). MCP server key `acs`
  in plugin `acs` appears as `plugin:acs:acs` in `/mcp`; its tools are
  `mcp__plugin_acs_acs__acs_status`, `...acs_claim`, `...acs_release`, `...acs_intent`,
  `...acs_why` **[docs]** (pattern **[captured]** with `acs-demo`/`envdump`). A matcher written
  against the bare server name never fires. The plugin name must be fixed before the usage skill
  text is written, because the skill refers to tool names.
- **Substitution.** `${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}` and `${CLAUDE_PROJECT_DIR}`
  are substituted in hook `command`/`args` and MCP `command`/`args`/`env`; `${user_config.*}`
  only in exec form **[docs]**.
- **Install.**
  - `acs plugin install claude-code` runs the marketplace steps below at project scope (decision
    0021).
  - Development, one session: `claude --plugin-dir ./integrations/claude-code` **[captured]**.
  - Through a marketplace: a repository (or directory) with `.claude-plugin/marketplace.json`
    `{"name","owner":{"name"},"plugins":[{"name":"acs","source":"./integrations/claude-code","description":"..."}]}`;
    then `claude plugin marketplace add <path|owner/repo>` and
    `claude plugin install acs@<marketplace> [--scope user|project|local]` **[docs]**. Install and
    enable state live in `enabledPlugins` (key `acs@<marketplace>`) in the settings file of the
    chosen scope; installed files land under `~/.claude/plugins/`. Project scope writes
    `.claude/settings.json`, which is a sealed path: `acs init` writes it as the human, outside
    the gate. Marketplace names such as `claude-code-plugins` and anything impersonating
    Anthropic are reserved; use `acs` or a similar name. `claude plugin install --yes` accepts
    `command`-source prompts non-interactively.
  - `claude plugin validate ./marketplace-dir` validates a marketplace.
  - Plugins load at session start or after `/reload-plugins`; `claude plugin disable acs` and
    `/plugin` turn the whole bundle (hooks, MCP, skills) off.
- Hooks of a plugin that is not enabled do not run. A plugin that is installed but whose `acs`
  binary is missing leaves a non-blocking hook error on every event (section 8).

## 11. MCP server registration

Three ways for ACS **[docs]**:

1. Plugin `.mcp.json` (preferred; travels with the plugin). Stdio entry:
   `{"mcpServers":{"acs":{"command":"acs","args":["mcp","--harness","claude-code"]}}}`. Without a
   `${CLAUDE_PLUGIN_ROOT}` reference the command is resolved on `PATH`. The plugin server needs no
   user approval **[captured]** (it ran in `claude -p`).
2. Project `.mcp.json` at the project root (committed). In interactive sessions Claude Code asks
   the user to approve project-scoped servers; `claude -p` loads them without asking **[captured]**
   (PreToolUse `mcp_server` was `{"name":"envdump","source":"project"}`; a server from
   `--mcp-config` reports `"source":"dynamic"`, a plugin server `"source":"plugin"`). Approval
   state lives in `enabledMcpjsonServers` / `enableAllProjectMcpServers` /
   `disabledMcpjsonServers` in settings.
3. `claude mcp add --scope user|project|local ...`, which writes `~/.claude.json` (user, local) or
   `.mcp.json` (project).

Facts: server start timeout `MCP_TIMEOUT`; per-call timeout `MCP_TOOL_TIMEOUT` or a `timeout` field
per server; on session end Claude Code sends SIGINT, then SIGTERM after 100 ms, SIGKILL after a
further 400 ms (Windows: closes stdin, kills the tree after 300 ms), so `acs mcp` must exit
promptly on stdin EOF and on those signals **[docs]**. Tool output over 10,000 tokens warns and is
capped at 25,000 by default.

Tool search: with tool search enabled (default in this environment) MCP tools are *deferred*: the
model must find them through `ToolSearch` before calling them. The `acs_*` tool descriptions
should be written so a keyword search finds them (**[captured]**: the model called ToolSearch
with `select:<exact name>`). `ENABLE_TOOL_SEARCH` controls this **[docs]**.

## 12. Skills

Skills are `SKILL.md` files in `<dir>/<skill-name>/` with YAML frontmatter (`name`, `description`)
**[docs]**, **[captured]** (plugin skill invoked via the `Skill` tool, content delivered). Locations:
enterprise (managed settings dir), personal `~/.claude/skills/`, project `.claude/skills/`, nested
`<subdir>/.claude/skills/`, `--add-dir` directories, and plugin `skills/` (namespaced
`<plugin>:<skill>`). Plugin skills are the delivery route for `acs-setup` and `acs-usage`
(WP5.1). A `CLAUDE.md` at a plugin root is not loaded as context **[docs]**.

## 13. Settings files and precedence

Files **[docs]**: user `~/.claude/settings.json` (and `~/.claude.json` for MCP/state), shared
project `.claude/settings.json`, project-local `.claude/settings.local.json`, managed settings
(`managed-settings.json` in a system directory, MDM, or server-managed). `CLAUDE_CONFIG_DIR`
relocates the user files. `--settings <file|json>` overrides for one session. Precedence, highest
first: managed, command line, local, shared project, user. Lists such as `permissions.allow`
merge; hooks merge. Project settings are read from the session's **primary working directory**
(so a session started in a monorepo subdirectory reads that subdirectory's `.claude/`); the local
file is read from the repository root in a git repository. In project settings, hooks and some keys
wait for the workspace-trust dialog. Hook entries from a lower level cannot remove a managed hook.

Settings keys that switch ACS's Claude Code integration off or around it **[docs]**:
`disableAllHooks`, `hooks` (editing or removing entries), `enabledPlugins` (`"acs@x": false`),
`extraKnownMarketplaces`, `disabledMcpjsonServers`, `enableAllProjectMcpServers`, `env` (for
example overriding `PATH` so `acs` resolves to another binary), `permissions` (allow rules do not
bypass hook denials), `allowManagedHooksOnly` (managed only), `disableSkillShellExecution`.

Claude Code itself treats writes under `.claude/` as protected: prompted in `default` and
`acceptEdits`, classified in `auto`, denied in `dontAsk`, **allowed in `bypassPermissions`**.
`.mcp.json` and `.claude.json` are protected files too. ACS's sealed paths give the
mode-independent guarantee.

## 14. Sealed paths for Claude Code (proposed)

Repo-relative globs in the SPEC section 5 dialect (root-anchored; `**` as a whole segment matches
zero or more segments). The orchestrator adds the accepted ones to `DEFAULT_SEALED` (decision 0011
section 6). Current defaults already contain `.claude/settings*.json` and `.agents/**/hooks*.json`.

### 14.1 Recommended additions (tier 1)

| Glob | Why |
| --- | --- |
| `**/.claude/settings*.json` | Replaces the root-only `.claude/settings*.json`. Claude Code reads project settings from the *primary working directory*, so a session started in `packages/web/` reads `packages/web/.claude/settings.json`; that file can set `disableAllHooks`, drop hook entries, disable the plugin (`enabledPlugins`), override `env.PATH`, or register another MCP/marketplace. Matches `settings.json` and `settings.local.json` at any depth (`**` also matches zero segments, so the root file stays covered) |
| `**/.claude/hooks/**` | Conventional home of hook scripts that settings point at; replacing a script is equivalent to editing the hook |
| `**/.mcp.json` | Project MCP registration: removing the ACS server, or adding a server that shadows or abuses tool names |
| `.claude-plugin/**` and `**/.claude-plugin/**` | **Not recommended as a default**; see 14.3 |

`**/.env*` (already default) covers `.envrc` (direnv, which can alter `PATH` for the shell).

### 14.2 Hardening (tier 2, per repository through `guards.toml`, not a default)

These are in-repo ways to inject *additional* hooks (not remove ACS's). Added hooks cannot lift an
ACS deny (deny wins), but a `PreToolUse` hook that returns `updatedInput` can change the input
after ACS validated the original (section 6.1 rule 6 and the parallel-execution note in section 3).
Sealing them blocks agents from writing legitimate skills and agents, so they are opt-in:

| Glob | Why |
| --- | --- |
| `**/.claude/skills/**/.claude-plugin/**` | skill folders loaded as plugins carry their own hooks/MCP servers (needs workspace trust) |
| `**/.claude/skills/**`, `**/.claude/agents/**`, `**/.claude/commands/**` | skill/agent/command frontmatter can declare hooks |
| `**/.claude/workflows/**` | workflow scripts orchestrate subagents |

Do **not** seal `.claude/**` wholesale: it contains `.claude/worktrees/` (git worktrees of this very
repository, where agents work), `.claude/agent-memory/` and other state Claude Code manages.

### 14.3 Not sealable by repo-relative globs (gaps)

- `~/.claude/settings.json`, `~/.claude.json`, `~/.claude/plugins/**` (installed plugin files),
  the managed settings file, `CLAUDE_CONFIG_DIR`: outside the repository. An agent writing there
  trips `path_escapes_repo` (ask) in the pipeline; in `claude -p` an `ask` is a denial, and
  interactively it is a prompt **[untested interactively]**. Recommendation to the orchestrator:
  consider a contract addition for harness-global config paths (for example a home-relative
  sealed list), or accept `ask` as the guard.
- **Nested `claude` processes.** An agent can run `claude --bare -p ...` (skips all hooks) or
  `claude --settings '{"disableAllHooks":true}' -p ...` through Bash and the child edits files with
  no ACS hooks at all. The shell analysis (WP1.4) should treat an invocation of `claude`, and of
  other agent CLIs, as an `ask` or unknown-target command. The git backstop (WP2.4) still catches
  commits.
- `.claude-plugin/marketplace.json` / `plugin.json` inside the repo alter only what is *offered*;
  enabling needs `enabledPlugins` (sealed) or a user action. Sealing them would also stop agents
  developing ACS's own plugin in this repository (`integrations/claude-code/**`), and defaults are
  not removable (decision 0004 rule 7). Not recommended.
- A human can always turn ACS off by editing the sealed files outside a session or with
  `acs off` (which keeps sealed paths).

## 15. Limitations and open points

- **Interactive `ask`**, `/clear` and `--continue` in an interactive session, Windows without Git
  Bash (the PowerShell tool), and `bypassPermissions` interactive behaviour were not run. The
  research used `claude -p` on Linux; section 18 covers `claude -p` on Windows with Git Bash.
- Capture ran inside another Claude Code session. The nested CLI inherited the outer session id
  until `--session-id` was passed, so session-id checks in section 9 use `--session-id`.
- `tool_input` for `Write` has no `old` content and `Edit` has no line numbers: symbol-level
  claims come from `old_string`/`new_string` only. `Write` of an existing file has no diff; the
  pipeline derives pre-images from its own snapshots.
- A `Bash` tool call may `cd`; the payload `cwd` is the cwd *before* the command. `Bash` commands
  are analysed by acs-shell; `Glob`/`Grep` on Linux/macOS do not exist as tools.
- `PostToolUse` fires per tool call; there is no hook for files changed by a background Bash
  command after the call returned (`run_in_background`) or by another process. `FileChanged` exists
  but only for explicitly watched filenames.
- `PreToolUse` does not fire for `@file` mentions, `EndConversation`, or MCP tools that are not
  called (tool search lookups themselves are tool calls of kind `other`).
- Hook parallelism and `updatedInput` (section 14.2).
- A hook that crashes or whose binary is missing fails open (section 8).
- `SessionEnd` has a 1.5 s budget; `UserPromptSubmit` and the model-switch events 30 s by default.
- Fields the adapter ignores today but that may be useful later: `permission_mode`
  (`bypassPermissions` could tighten `ask` into deny), `effort`, `prompt_id` (turn correlation),
  `agent_id`/`agent_type` (subagent attribution), `transcript_path`.

## 16. How this research was run

- Docs: the `.md` form of each page under <https://code.claude.com/docs/en/> was downloaded and
  read locally on 2026-10-30.
- Capture: scratch git repository with a `.claude/settings.json` whose hooks (17 events) ran a
  script appending stdin and a filtered environment to a file; sessions run with
  `claude -p "<scripted task>" --permission-mode acceptEdits --output-format json --debug-file ...`;
  further runs used a policy hook that returned scripted outputs (deny via JSON, via exit 2, via
  both, ask, `additionalContext` per event), a stub stdio MCP server dumping its environment, a
  throwaway plugin loaded with `--plugin-dir`, `--settings` files with a missing hook binary (with
  and without `onFailure: block`), and `--resume` / `--continue`. Nothing outside the scratch
  directory was modified; no settings file of the repository or the user was touched.
- Fixture provenance: see `tests/fixtures/claude-code/README.md`.

## 17. Acceptance run (real session, 2026-10-30)

Setup, all in a scratch directory (`ACS_HOME` inside it, debug build of `acs` copied to a `bin/`
directory placed first on `PATH` for the run only, no global install, no change to any settings
file):

1. A throwaway git repository (`user.email dev@example.com`) with `src/payments/refund.rs`.
2. A second seat, `harness_session_id` `sara-session-1`, pilot `sara@example.com` (through
   `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_0`/`GIT_CONFIG_VALUE_0` in the environment of that one
   command), claims the file through the canonical adapter:
   `acs hook --canonical session_start < ...` then `acs hook --canonical pre_tool < ...` with an
   `Edit` event for `src/payments/refund.rs`. The result was `allow` with one claim acquired.
3. Claude Code, loading the plugin from the checkout:
   `claude -p "Edit src/payments/refund.rs so the function refund returns the number 1 (use the Edit
   tool). If the edit is blocked, quote the exact blocking message you received verbatim, and also
   quote verbatim any text that mentions ACS and was added to your context at the start of the
   session. Then stop; do not try any other way to change the file."
   --plugin-dir integrations/claude-code --permission-mode acceptEdits --session-id <uuid>`.

Outcome: the edit was blocked, the file is unchanged (`fn refund() {}`), and the model quoted the
guidance, naming the holder. Transcript excerpt (model output, trimmed to the quotes):

```
The edit was blocked ...

The blocking message, verbatim:
> PreToolUse:Edit hook error: `src/payments/refund.rs` is claimed by sara@example.com (canonical,
> active just now). Work on other files, or ask your pilot to coordinate.

Text mentioning ACS that was added to my context at the start of the session, verbatim:
> ACS coordinates the agents working in this repository. Editing a file claims it for you; a file
> another seat has claimed is blocked until they finish.
> You hold no claims yet.
> Claimed by others: `src/payments/refund.rs` (sara@example.com, canonical).
> Never writable: `.git/**`, `.acs/**`, `.github/workflows/**`, `**/.env*` and 2 more.
```

Claude Code's debug log for the same run (`--debug-file`) shows the hook side:

```
Hooks: SessionStart:startup [acs hook claude-code] (plugin acs@inline) finished with status 0 (117ms)
Hook SessionStart:startup success: {"hookSpecificOutput":{"additionalContext":"ACS coordinates ...","hookEventName":"SessionStart"}}
Hooks: PreToolUse:Edit [acs hook claude-code] (plugin acs@inline) finished with status 2 (17ms)
Hook PreToolUse:Edit (PreToolUse) error: status code 2
stdout: {"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"`src/payments/refund.rs` is claimed by sara@example.com (canonical, active just now). Work on other files, or ask your pilot to coordinate."}}
stderr: `src/payments/refund.rs` is claimed by sara@example.com (canonical, active just now). Work on other files, or ask your pilot to coordinate.
```

Observations from the run:

- Plugin loaded with `--plugin-dir` (hooks, skill `acs:acs-usage` listed). The MCP server
  `plugin:acs:acs` failed to connect because `acs mcp` does not exist yet (WP2.6); Claude Code
  reported "Skipping connection (recent failure cached ...)" and carried on. A failed MCP server
  does not affect the hooks.
- With `acs` missing from `PATH` (an earlier, accidental run in a clobbered scratch directory), every
  hook failed to start and the tool call went ahead: the fail-open behaviour of section 8, seen
  with the real plugin.
- Hook latency as logged by Claude Code (debug build, cold): SessionStart 117 ms, PreToolUse 13
  to 34 ms, PostToolUse 11 ms.
- The same run with `acs mcp` serving, once WP2.6 landed, is in section 18.

## 18. Windows runs (real sessions, 2026-10-31)

`claude -p` 2.1.316 with Sonnet on Windows 11, Git Bash as the Bash tool, plugin loaded with
`--plugin-dir`, `acs` 0.1.0 from a local build. All **[captured]**.

- An edit of a claimed file and an edit of a sealed file were denied, and the agent quoted each
  guidance message and stopped. A new file was written.
- **Hooks and MCP in one session, a first on any platform.** `acs_intent`, `acs_status`, `acs_claim`,
  `acs_release` and `acs_why` all worked through `mcp__plugin_acs_acs__*`. `acs_why` explained a
  denial the hook had just made, so the two share a seat.
- **A `.cmd` shim works as the hook command.** With only `acs.cmd` on `PATH`, hooks ran and denied as
  usual, which contradicts the docs (section 2).
- **A hook whose command cannot start fails open silently:** the edit of a claimed file went through
  with no message to the agent, and only a `SessionEnd` hook error was printed at the end.
- Claude Code caches an MCP connection failure for 15 minutes ("Skipping connection (recent failure
  cached ...)"), so one bad run makes the next look broken too.
- `claude plugin marketplace add`, `install` and `list` need no login, and work with `CLAUDE_CONFIG_DIR`
  in a scratch directory. `marketplace add --scope project` plus `install --scope project` write
  `extraKnownMarketplaces` and `enabledPlugins` to `.claude/settings.json`. Both are safe to repeat.
  `install` enables a disabled plugin again. `list --json` also shows other projects' entries, with
  `enabled` worked out for the current directory.
