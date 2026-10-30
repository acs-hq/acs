# 0014 — Antigravity adapter and plugin (WP2.3)

Date: 2026-10-30. Status: accepted (WP2.3), pending orchestrator review.

## Context

WP2.3 adds the `antigravity` adapter, the Antigravity plugin and the format record
`docs/adapters/antigravity.md`. Plan section 9.2 asks whether Antigravity can deny a tool call and
inject context from a hook. **No real Antigravity run was possible:** it is not installed in the
build environment, and the official documentation host `antigravity.google` is refused by the
environment's network policy. The formats come from the official changelog in the
`google-antigravity/antigravity-cli` repository (Antigravity 1.4.11 on 2026-10-30), from the
official SDK documentation, and from third-party issues and pull requests that disagree in places
(doc section 12). Everything below that rests on a third-party report is marked in the adapter
doc as unverified, and a manual checklist for a human is in its section 13.

## Decision

**Risk 9.2 verdict: tier A with caveats**, plus a tier B always-on rule shipped with the plugin.
A `PreToolUse` hook can deny and the model sees the reason; context can be injected only from
`PreInvocation` (`injectSteps` with `ephemeralMessage`), never from a tool hook; MCP seat binding
works through a per-call `_meta` key. The risk does not trigger the fallback, but the rule stays,
because injection is soft and its persistence is unverified.

Choices beyond the plan and the spec:

1. **Phase comes from the command line.** Antigravity payloads carry no event name, so the plugin
   runs `acs hook antigravity --phase pre_tool | post_tool | prompt_submit`. Without `--phase` the
   adapter infers the phase from the payload fields (fallback only).
2. **First invocation is `session_start`.** Antigravity has no session start, session end or
   user-prompt event. `PreInvocation` (before every model call) is `session_start` when
   `invocationNum <= 1` and `initialNumSteps <= 1` (a missing field does not count against it;
   with neither, `prompt_submit`), otherwise `prompt_submit`. The meaning of both counters is
   undocumented; if `invocationNum` restarts each turn the status repeats every turn (noisy,
   harmless). The prompt is `lastUserInput` when present.
3. **No `Stop` hook, no `session_end`.** `Stop` fires at the end of every agent turn; mapping it
   would release claims whenever the agent idles. Claims expire by TTL and crashed-session
   detection handles the rest (orchestrator decision).
4. **Rendering.** Exactly one JSON object on stdout and exit 0, always (Antigravity fails closed on
   an empty or unknown `PreToolUse` answer, and exit code 2 means nothing there):
   `allow` becomes `{"decision":"ask"}` (no objection; defer to Antigravity's own permission flow),
   never `allow`, which the desktop app treats as approval and which would skip the user's own
   prompts; `ask` becomes `force_ask` with the reason (plain `ask` honours the always-allow cache,
   so an always-allowed command would slip past an ACS ask); `deny` and `queue` become `deny` with
   the reason. An unparsed payload renders `{"decision":"ask"}`, because the event, and so
   whether the call can be blocked, is unknown. There is no environment override: adapters are
   pure.
5. **Warnings and context.** `PreToolUse` has no context channel: warnings are appended to the
   reason of a deny or ask and dropped on an allow. `PreInvocation` output is `{}` unless the
   decision has context or warnings, then one `ephemeralMessage` (context, then warnings, blank
   line separated). `PostToolUse` and the other phases print `{}`.
6. **Tool classification, version tolerant.** Tool names changed in Antigravity 1.4.3 (reported by
   two integrators, not in the changelog), so the adapter accepts both spellings and the SDK's:
   write `write_to_file | write_file | create_file`; edit `replace_file_content |
   multi_replace_file_content | edit_file`; read `view_file | read_file | view_code_item |
   view_file_outline | list_dir | list_directory | find_by_name | find_file | grep_search |
   search_directory`; shell `run_command`; a list of known others. Argument keys are matched
   case-insensitively; paths come from `TargetFile`, `Path`, `AbsolutePath`, `FilePath`, `File`
   (and the directory keys for reads), `file:` URIs are decoded. An **unknown tool whose
   arguments hold path-like keys (suffix `path`, `file`, `dir`, `directory`, `uri`, plural too,
   nested to depth 4) is classified `write`** (conservative; it can wrongly deny an unknown read
   tool on a claimed file); without path-like keys it is `other`. `EditInfo` comes from
   `Target/TargetContent` and `Replacement/ReplacementContent`; for `multi_replace_file_content`
   only when there is exactly one chunk (the `ReplacementChunks` shape is unverified). No `delete`
   or `move` kinds are produced: Antigravity has no such tools, so deletes and moves reach the
   pipeline as `run_command` and go through `acs-shell`.
7. **Windows.** Antigravity runs commands in PowerShell on Windows but names the tool
   `run_command` everywhere, which decision 0011 section 3 would read as bash. When the event's
   working directory is a Windows path (drive letter or UNC) the tool name is
   `run_command (powershell)`, which selects the PowerShell dialect without a contract change.
8. **Working directory.** `Cwd` argument when present (removed in 1.4.3), else `workspacePaths[0]`
   (plugin hooks do not run in the workspace), else a payload `cwd`, else empty (the pipeline then
   uses the process directory).
9. **Session id.** `conversationId`, or Claude style `session_id` (one guide documents it).
   **Subagents are separate seats**: their hook `conversationId` is their own, `parentConversationId`
   is declared but not sent, and no documented source shows where a subagent transcript lives, so
   the root id is not derived from `transcriptPath`. Known gap: a parent and its subagent can
   conflict on claims.
10. **Parse errors** are `AdapterError`s carrying the recovered `conversationId` (or `session_id`)
    and the first workspace path (or `cwd`), so the pipeline logs against the right session.
11. **Plugin** (`integrations/antigravity/`): `plugin.json` (name `acs`, minimal fields),
    `hooks.json` at the plugin root (PreToolUse and PostToolUse matcher `.*`, PreInvocation, timeout
    10, no Stop), `mcp_config.json` (`acs mcp --harness antigravity`; the agent sees it as
    `acs_acs`), `rules/acs.md` (`trigger: always_on`) and `skills/acs-usage/SKILL.md`. The matcher is
    `.*` rather than a tool list because a list fails open when tool names change. Whether a
    deny from a plugin-bundled hook gates is unverified; the fallback (copy the hooks to
    `.agents/hooks.json` or `~/.gemini/config/hooks.json`) is in the plugin README.
12. **Sealed paths** are only proposed (the adapter doc, section 11). The orchestrator applies
    `**/`-prefixed `.agents` and `.agent` hooks and plugins globs to `DEFAULT_SEALED`.
    `DEFAULT_SEALED` was not touched.

## Consequences

- **Hook failure fails closed.** A missing or crashing `acs` blocks every tool call in
  Antigravity. `acs doctor` (WP2.5) and the setup skill must verify `acs` resolves on `PATH`
  before installing the hooks.
- **`ask` as "no objection" is unverified.** If it forces a prompt for tools Antigravity would run
  unprompted (reads, auto mode, always-allowed commands), the matcher must be narrowed to
  side-effecting tools or the `allow` rendering revisited (checklist item 5).
- **WP2.6** should set `_meta: {"antigravity/alwaysLoad": true}` on the ACS tools (otherwise
  Antigravity discovers MCP tools on demand) and read
  `params._meta["antigravity.google/root_conversation_id"]` on each `tools/call` for seat binding.
  The id is not given at `initialize`, and an MCP server's environment is not documented to carry
  it; fall back to decision 0011 section 5's second rule.
- **WP5.1:** `skills/acs-usage/SKILL.md` and `rules/acs.md` in this plugin are interim copies of the
  usage etiquette. When WP5.1 lands the canonical skills, sync them (or generate these from the
  canonical text) so the two do not drift.
- **WP10.1:** `PreToolUse` matches every tool, so there is one process start per tool call,
  including reads.
- **Global Antigravity files** (`~/.gemini/config/hooks.json`, `mcp_config.json`, `config.json`,
  plugins, `~/.gemini/antigravity-cli/settings.json`) cannot be sealed by repository globs. Write
  tools that target them trigger `path_escapes_repo` (ask); shell writes there are a threat-model
  gap.
- Version drift: `agy` auto-updates in a running session; the payload may change silently. Parsing
  ignores unknown fields, and any unparseable payload is allowed (`unknown_payload`) and rendered
  as the safe pre-tool answer.
- Untested: any real Antigravity run, on any platform; Windows paths are tested as strings only.
