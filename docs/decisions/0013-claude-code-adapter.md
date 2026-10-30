# 0013 — Claude Code adapter and plugin (WP2.2)

Date: 2026-10-30. Status: accepted (WP2.2), pending orchestrator review.

## Context

WP2.2 adds the `claude-code` adapter to `acs-adapters`, the Claude Code plugin under
`integrations/claude-code/`, a marketplace file at the repository root, and the adapter document
`docs/adapters/claude-code.md` (sources, access date, every payload relied on, captured
fixtures). Plan risk 9.2 ("a harness cannot deny or inject context from a hook") was checked first
and **does not apply**: a `PreToolUse` hook denies a call and the reason reaches the model, and
`additionalContext` reaches the model from `SessionStart`, `UserPromptSubmit`, `PreToolUse` and
`PostToolUse` (captured with Claude Code 2.1.314, `claude -p`). Claude Code is tier A.

## Decision

### Adapter (`acs-adapters/src/claude_code.rs`)

- **Phase** comes from `hook_event_name`: `SessionStart`, `UserPromptSubmit`, `PreToolUse`,
  `PostToolUse` and `PostToolUseFailure` (both `post_tool`), `SessionEnd`. `--phase` is only a
  fallback when the payload has no `hook_event_name`. Any other event (for example `Stop`) is an
  `AdapterError`, so the pipeline allows it with `unknown_payload`; the plugin registers only the
  six supported events, so this is a safety net.
- **Errors** carry the session id and `cwd` through `AdapterError::recovering`. A payload with no
  `session_id` or `cwd`, a tool event without `tool_name`, or a file-changing tool (`Write`,
  `Edit`, `MultiEdit`, `NotebookEdit`) or shell tool without the field that names its target or
  command is an error rather than an ungated event: a changed payload shape must show up as an
  `unknown_payload` warning, not as silent allows.
- **Tool kinds**: `Write` write; `Edit`, `MultiEdit`, `NotebookEdit` edit; `Bash`, `PowerShell` and
  `Monitor` with a `command` shell; `Read`, `NotebookRead`, `Glob`, `Grep` read; everything else,
  including `mcp__*`, `Agent`, `Workflow`, `Skill`, web tools, other. Paths are the raw absolute
  strings Claude Code gives (Windows keeps backslashes; the pipeline normalises). `Edit` carries
  `old_string`/`new_string` (an empty `new_string` is kept); the others carry no edit text.
- **Results**: `PostToolUse` is `success: true` and, for shell tools, `exit_code: 0` (a non-zero
  exit fires `PostToolUseFailure` instead); an interrupted tool response is `success: false`.
  `PostToolUseFailure` is `success: false` with the code from the `Exit code N` first line of
  `error`, `None` when absent. The pipeline does not read `result` (decision 0012).
- **`raw`** keeps only small identifiers and modes (`hook_event_name`, `tool_use_id`, `prompt_id`,
  `permission_mode`, `agent_id`, `agent_type`, `source`, `reason`, `model`, `mcp_server`,
  `is_interrupt`, `duration_ms`). It never holds tool input, tool output, prompts or the transcript
  path. Subagent calls keep the parent's `session_id` (one seat) with `agent_id` in `raw`.
- **Rendering**:
  - allow: empty stdout and exit 0; with message, context or warnings, an `additionalContext`
    document for the event. **Never `permissionDecision: "allow"`**: it would skip the user's own
    permission prompt.
  - deny (and `queue`, treated as deny): JSON `permissionDecision: "deny"` on stdout, the same
    reason on stderr, exit 2. Verified: the JSON reason is the one the model sees, and exit 2 still
    blocks if the JSON were malformed. An empty message gets a generic reason naming the code.
  - ask: JSON `permissionDecision: "ask"` with the reason, exit 0. In non-interactive runs Claude
    Code turns it into a denial with the reason visible (verified in `-p`, also under
    `bypassPermissions`).
  - post-tool, session start and prompt submit: `additionalContext`; session end: nothing (Claude
    Code discards it). Warnings and context are rendered as context lines after the message; on a
    deny they are appended to the reason (extra context is not reliable alongside a deny).
  - Text is capped at 10,000 characters (Claude Code's own cap, beyond which the model gets only a
    file path) with a short marker. Messages are factual statements, as the docs advise.
  - A decision without an event (unparseable payload) is an allow with empty output.
  - `updatedInput` and `systemMessage` are not used.
- **Hook failures are fail-open by default** in Claude Code (missing binary, timeout, exit code
  other than 0 or 2, malformed JSON): verified. `acs hook` therefore prints exactly one JSON
  document on stdout, logs only to stderr, and uses exit codes 0 and 2 only (0011 already makes
  argument errors exit 1).

### Plugin (`integrations/claude-code/`) and marketplace

- Plugin name `acs` (skills `acs:acs-usage`, MCP tools `mcp__plugin_acs_acs__<tool>`). Manifest,
  `hooks/hooks.json`, `.mcp.json`, `skills/acs-usage/SKILL.md`, `README.md`; license is the
  workspace license (`MIT OR Apache-2.0`). `claude plugin validate` passes for the plugin and for
  the root `.claude-plugin/marketplace.json` (marketplace name `acs-plugins`; install with
  `claude plugin install acs@acs-plugins`).
- Hooks use exec form (`command: "acs"`, `args: ["hook", "claude-code"]`, no shell: works the same
  on Windows, where `acs.exe` must be on `PATH`), no matcher on any event so MCP tools are seen,
  explicit timeouts (20 s pre-tool, session start and prompt; 30 s post-tool; 5 s session end, which
  Claude Code caps at 1.5 s for plugin hooks anyway).
- **No `onFailure: "block"` by default.** It makes a missing or broken `acs` binary block every tool
  call, including the Bash call that installs it, so the setup skill could not recover. It needs
  Claude Code 2.1.295 and its handling in older versions is untested. Hard guarantees come from
  sealed paths and the git backstop. `acs init` may later offer a strict mode that writes a project
  hook with `onFailure: "block"` for `PreToolUse`.
- `.mcp.json` registers `acs mcp --harness claude-code` as server `acs`; `acs mcp` arrives with
  WP2.6 (until then Claude Code reports the server as failed and carries on).
- **Skill copy.** `skills/acs-usage/SKILL.md` in the plugin is a minimal copy written by WP2.2.
  WP5.1 owns the full skills under `skills/` and **must keep the plugin copy in sync** (or make the
  plugin's skills directory a build output of `skills/`).
- The plugin is distributed from the repository root marketplace with a relative source
  (`./integrations/claude-code`); WP5.2 owns packaging.

### Sealed paths for Claude Code (orchestrator applies; `DEFAULT_SEALED` is not edited here)

Tier 1, proposed defaults (details and reasons in `docs/adapters/claude-code.md` section 14):

```
**/.claude/settings*.json      (replaces the root-only .claude/settings*.json)
**/.claude/hooks/**
**/.mcp.json
```

Tier 2 (opt-in per repository in `guards.toml`, not defaults): `**/.claude/skills/**`,
`**/.claude/agents/**`, `**/.claude/commands/**`, `**/.claude/workflows/**`. Not recommended:
`.claude/**` (contains `.claude/worktrees/` and Claude Code state) and `.claude-plugin/**` (would
block development of the plugin in this repository; defaults are not removable).

## Consequences

- `acs hook claude-code` needs nothing from the adapter beyond `by_name`; the pipeline already
  handles resume (seats revive), compaction (`session_start` returns status every time) and prompt
  events.
- The adapter relies on `acs` being on Claude Code's `PATH`. WP5.2 installers must put it there;
  the hooks fail open without it.
- **Open items for later WPs:** (1) WP2.6: `CLAUDE_CODE_SESSION_ID` in the MCP environment is
  stale after `/clear` and on `--continue`, so validate it against live seats and fall back to the
  worktree match; each MCP call also carries `_meta["claudecode/toolUseId"]`, equal to the
  `tool_use_id` of its `PreToolUse` event, usable for exact binding (adapter doc section 9). The
  adapter keeps `tool_use_id` in `raw`. (2) WP1.4/WP2.1: nested `claude --bare -p` or
  `claude --settings '{"disableAllHooks":true}'` run from a shell bypasses hooks; shell analysis
  should treat agent CLIs as `ask` or unknown targets. (3) Files outside the repository
  (`~/.claude/settings.json`, `~/.claude.json`, installed plugin files, managed settings) cannot be
  sealed by repository globs; only `path_escapes_repo` (ask) guards them. (4) Interactive `ask`,
  `/clear`, Windows and macOS were not run; Windows shapes come from the docs.
- Changing `tests/fixtures/claude-code/` or the adapter requires re-checking the adapter document;
  the adapter's unit tests parse every fixture file and compare output shapes with the `output/`
  fixtures.
