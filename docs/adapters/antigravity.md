# Antigravity adapter: formats relied on

Work package WP2.3. Stages 1 (research) and 2 (adapter and plugin) are done; the "As implemented"
section below says what the code does and lists the manual verification a human must still run.
Accessed **2026-10-30**. Antigravity CLI `agy` was at
version **1.4.11** that day.

Read this first:

- **Verified on 2026-11-03 against a real `agy` 1.4.12 on Windows and the official docs.** Section
  15 records what the runs and the official pages showed, and wins wherever it disagrees with
  sections 1 to 14, which are kept as the research record. Decision 0024 has the resulting changes.
- **No official documentation page could be read** (on 2026-10-30; see section 15 for 2026-11-03). `antigravity.google` (including
  `/docs/hooks`) is refused by this environment's egress proxy (HTTP 403 on CONNECT, policy
  denial), so the pages Google calls "the official docs" were never opened. What is official
  here is the `google-antigravity` GitHub organisation (2 public repositories:
  `antigravity-cli`, which holds the README and a detailed `CHANGELOG.md`, and
  `antigravity-sdk-python`). Everything about hook payload shapes beyond the changelog comes from
  third-party integrators' issues and pull requests, which disagree with each other in places.
  Section 12 lists each disagreement.
- **Nothing was run.** Antigravity is not installed. No payload below is a capture; the fixtures
  say which fields are quoted and which are assembled.
- Re-verify (ideally with `antigravity.google` allowed in the session's network settings, and
  one real `agy` run with a recording hook) before stage 2 leans on anything marked *unverified*.

## 1. Surfaces and what they share

| Surface | What it is | Hooks |
| --- | --- | --- |
| Antigravity CLI (`agy`) | Go terminal agent, "shared core agent engine" with the app | Yes, `hooks.json`. This is the best-documented target. |
| Antigravity 2.0 / desktop app, IDE | GUI on the same engine; shares settings and permissions with the CLI [S2] | The IDE payload schema (`exa.hooks_pb`, IDE 2.5.5) has the same fields [S12]. One issue says the app honours hook `allow` and `deny` [S5]; one forum report (via [S19]) says the IDE does not run workspace hooks. *Unverified.* |
| Antigravity SDK (Python) | Library to build agents on the same harness; hooks are Python callbacks, not `hooks.json` [S3] | Not relevant to the plugin; useful for tool names. |

The adapter targets the payloads of `hooks.json` command hooks, as run by `agy` and (by
inference) the app.

**Sharing with Gemini CLI: partial, not for hooks.**

| Item | Shared with Gemini CLI? |
| --- | --- |
| Home directory `~/.gemini/` | Yes (Antigravity uses `~/.gemini/config/`, `~/.gemini/antigravity-cli/`) |
| `GEMINI.md` and `AGENTS.md` as rules | Yes, both are read as user rules [S1 1.4.4] |
| Plugin manifest folder | Antigravity also accepts `.gemini-plugin/` besides `.antigravity-plugin/` [S1 1.3.2]; `${extensionPath}` placeholders resolve [S1 1.0.x] |
| Hook configuration | **No.** Gemini CLI keeps hooks under `hooks` in `~/.gemini/settings.json` / `.gemini/settings.json`, with events `BeforeTool`, `AfterTool`, `BeforeAgent`, `SessionStart`, ..., exit code 2 = block, and `hookSpecificOutput.additionalContext` [S23]. Antigravity reads `hooks.json`, events `PreToolUse`, `PostToolUse`, `PreInvocation`, `PostInvocation`, `Stop`, a top-level `decision`, and no `additionalContext` [S1, S8, S11]. `agy` explicitly reads a different file than Gemini CLI [S14, S1 1.0.10]. |
| MCP config, skills, rules, sealed globs | Different files (section 8, 9, 11) |

So the Antigravity adapter cannot reuse a Gemini CLI adapter.

## 2. Hooks

### 2.1 Where they are configured

| Scope | File | Source |
| --- | --- | --- |
| User (global) | `~/.gemini/config/hooks.json` | [S1 1.0.10, 1.1.1], [S5], [S11], [S14] |
| Workspace | `<workspace>/.agents/hooks.json`; the folder must be **trusted** for it to load | [S1 1.1.1] |
| Plugin | `hooks.json` at the **plugin root**. `hooks/hooks.json` is **not read** | [S1 1.4.3] |

`~/.gemini/antigravity-cli/hooks.json` was an old, wrong location and is never loaded [S1 1.0.10]
(some third-party guides still cite it, [S8]). The legacy directory name `.agent/` (singular) is
reported as still accepted for skills/rules/MCP [S20 summaries]; whether it also holds
`hooks.json` is *unverified*. Hooks from user and workspace scopes both run; any deny wins [S8].
`/btw` side questions no longer run workspace hooks [S1 1.3.3]. Hooks fire for subagent tool calls
too [S3 for the SDK, S9 for the CLI].

### 2.2 File format

Top-level keys are **hook names**; each maps event names to handlers. Tool events
(`PreToolUse`, `PostToolUse`) take a list of matcher groups; the other events take a flat list.
Shown by [S8], [S11], [S16]:

```json
{
  "acs": {
    "PreToolUse":    [ { "matcher": ".*", "hooks": [ { "type": "command", "command": "acs hook antigravity --phase pre_tool",  "timeout": 10 } ] } ],
    "PostToolUse":   [ { "matcher": ".*", "hooks": [ { "type": "command", "command": "acs hook antigravity --phase post_tool", "timeout": 10 } ] } ],
    "PreInvocation": [ { "type": "command", "command": "acs hook antigravity --phase prompt_submit", "timeout": 10 } ],
    "Stop":          [ { "type": "command", "command": "acs hook antigravity --phase session_end",   "timeout": 10 } ]
  }
}
```

- `timeout` is in seconds. `0` kills the process immediately and breaks the turn [S8]. No default
  is documented for Antigravity (Gemini CLI uses 60).
- An optional `enabled` field exists (`/hooks` writes it) [S14].
- A Claude Code style top-level `"hooks"` key loads as a single hook *named* `hooks` [S1 1.4.3];
  Claude Code files cannot be dropped in.
- `matcher` is a regular expression on the tool name. `"*"`, `""` and `".*"` are all used as
  catch-alls in the sources [S8, S16, S19]; `".*"` is the only form that is also a valid regex.
  `agy plugin validate` warns on "unrecognized hook tool matchers" and on invalid regexes [S1
  1.4.3], so a matcher naming a tool that does not exist is flagged. Whether matching is anchored
  (full match) is *unverified* ([S16] assumes full match).
- Files with a UTF-8 BOM load [S1 1.3.2]. `//` comments and trailing commas are tolerated in
  `mcp_config.json` [S1]; for `hooks.json` this is *unverified*.
- `command` is run by a shell-like launcher. A relative script path resolves against the
  directory `agy` was launched from, not the workspace, and then fails with exit status 127 [S8].
  Plugin hooks run **in the plugin directory** and receive the workspace only through stdin
  `workspacePaths` [S17]. A bare program name (`acs`) resolved from `PATH` is the form the
  integrators use for installed tools [S19], but *no source states PATH lookup rules*.
- `agy` logs `loaded N named hooks from N hooks.json file(s)` at startup, and `/hooks` lists them
  (including plugin-bundled ones since 1.2.3) [S14, S1].

### 2.3 Events

Five events exist [S8, S15, S16, S19]. Compare with the ACS phases:

| Antigravity event | When | Can gate? | Can inject context? | ACS phase (proposed) |
| --- | --- | --- | --- | --- |
| `PreToolUse` | before a tool runs; matcher on tool name | **Yes**: deny with a reason the model sees | **No** | `pre_tool` |
| `PostToolUse` | after a tool step finished | no | no (`overwriteResult` ignored [S6]) | `post_tool` |
| `PreInvocation` | before each model call (`invocationNum`) | `terminationBehavior: "terminate"` since 1.4.11 [S1]; earlier builds ignored it [S7] | **Yes**: `injectSteps` | `prompt_submit` |
| `PostInvocation` | after a model call | `terminationBehavior` only | no | (not used) |
| `Stop` | the agent is about to end its turn | can force a continue with `{"decision":"continue"}` | no | see 2.7 |

There is **no `SessionStart`, no `SessionEnd` and no `UserPromptSubmit`**. `SessionStart` is
disputed: one integrator's live test saw it never fire [S15, and its experiment repo]; another's
CLI tracing PR lists it as firing in the CLI but absent from the IDE schema [S12]; the official
changelog never mentions it. Do not rely on it. `agy plugin validate` "warns about unknown hook
events" [S1], so registering it is at best a warning.

### 2.4 Input (stdin)

One JSON object on stdin, **camelCase**, **no event name** (every integrator passes the event on
argv for that reason [S12, S19]). Common fields (`HookArgsCommon`) from [S9], [S11], [S12]:

| Field | Meaning | Notes |
| --- | --- | --- |
| `conversationId` | Antigravity's session id (UUID) | The only session field in practice. For a subagent's tool calls it is the **subagent's** conversation id [S9]. |
| `workspacePaths` | array of workspace roots | Empty under `agy --print` without `--new-project` [S11]. Hook processes of plugins do **not** run in the workspace. |
| `transcriptPath` | path of `transcript_full.jsonl` | Format undocumented. |
| `artifactDirectoryPath` | per-conversation artifact dir | |
| `modelName` | e.g. `gemini-3.8-flash-medium` | |
| `lastUserInput` | latest user message | Declared in the schema; the official changelog says 1.4.7 added it to the stdin JSON [S1], but reports on 1.2.14 say it was never sent [S6, S7]. Treat as possibly absent. |
| `executionId`, `isBattleMode` | in the IDE schema [S12] | ignore |

Per event:

- `PreToolUse`: `toolCall: {name, args}`, `stepIdx`. (A `PreToolHookArgs` carries the full args
  [S9].)
- `PostToolUse`: `toolCall: {name, args}`, `stepIdx` (0-based step index, **no tool call id**),
  `error` (string, `""` on success), `result` (declared, **never populated** [S6]). So ACS gets
  no exit code for shell commands: success is `error == ""`, `exit_code` is unknown.
- `PreInvocation` / `PostInvocation`: `invocationNum`, `initialNumSteps` (`modelOutput`,
  `modelThinking` are declared but absent in the CLI [S12]).
- `Stop`: `executionNum`, `terminationReason` (`NO_TOOL_CALL`, `ERROR`, `USER_CANCELED`, ...),
  `error`, `fullyIdle`, `finalModelOutput` (absent in the CLI) [S12].
- Not sent: a tool call id, `parentConversationId` (declared, not sent for subagents [S6]), the
  prompt (unless `lastUserInput` is present), the event name.

Use a real JSON parser, not regexes (nested payloads) [S17].

### 2.5 Output (stdout) and exit codes

**Always print one JSON object and exit 0.** Antigravity fails **closed** on a missing, empty,
malformed or unknown decision in `PreToolUse` [S5, S6, S14, S1 1.0.x "empty decision strings"]:
on 1.4.1/1.4.2 a `PreToolUse` hook that prints `{}` denies the call and blocks even `view_file`
[S5]. (On 1.3.3 `{}` allowed it, and one integrator reports empty stdout as allow [S9]; builds
differ, so always print an explicit decision.) A non-zero exit, a timeout or invalid JSON in a
`PreToolUse` hook **blocks** the call [S6, S8]. A failing `PostToolUse` hook is logged and the
tool keeps its result [S1 1.4.3]. Exit code 2 plus stderr does **not** block a model call from a
`PreInvocation` hook [S7]; exit code 1 is not a block either. The Gemini-style "exit 2 = block"
does not apply. This matches the failure-mode rule in decision 0011: `acs hook` must never crash
and must print a rendered decision even for internal errors.

`PreToolUse` output, top level (not wrapped in `hookSpecificOutput`, which fails validation [S8]):

| ACS outcome | Output | Notes |
| --- | --- | --- |
| `deny` | `{"decision":"deny","reason":"<message>"}` | Honoured in every mode, including `--dangerously-skip-permissions` [S5]. The TUI shows `Tool call denied by jsonhook__<name>_PreToolUse_0_0: <reason>` and the model sees the reason and adapts [S8]; with a deny reason the agent "pivots" (a Python workaround to `curl` was observed, so the message must be clear). `allow_tool:false` + `deny_reason` is the field spelling used in one guide [S8]; every other source uses `decision`/`reason`. Prefer `decision`/`reason`; stage 2 should emit both only if a live run shows the guide's spelling is needed. |
| `ask` | `{"decision":"force_ask","reason":"..."}` | `force_ask` forces a human prompt and ignores the always-allow cache; plain `ask` respects the cache, so an always-allowed command would slip through an ACS `ask` [S13]. *Unverified*; headless runs cannot prompt and auto-deny. An unknown decision string is an error, which fails closed (deny), the right fallback. |
| `allow` (no objection) | see below | |
| `warnings` on allow | no channel | `PreToolUse` cannot carry context. |

**The `allow` problem.** `{"decision":"allow"}` is ignored by the CLI ("the gate only consumes
`deny`" [S5]; headless runs auto-deny an ungranted command even after `allow`; still present on
1.4.7), but the desktop app honours it and treats it as approval without a prompt [S5, S14].
Emitting `allow` could therefore silently disable the user's own permission prompts in the app,
which ACS must not do. Two independent integrators emit `{"decision":"ask"}` instead as "no
objection, continue with Antigravity's own permission flow" [S13, S14]. **Decision (orchestrator, stage 2):** ACS `allow`
renders as `{"decision":"ask"}`, with no override; confirm
in a live run that `ask` does not force a prompt for tools Antigravity would run unprompted
(reads, auto mode). If it does, narrow the matcher to side-effecting tools (section 4).

`PreToolUse` can also `overwrite` the arguments (permission is re-checked against the rewritten
call) [S5, S6]. ACS does not use this.

`PreInvocation` output: `{"injectSteps":[{"ephemeralMessage":"<text>"}]}` injects context; `{}`
does nothing [S7, S11, S16]. See 2.6.

`PostToolUse`, `PostInvocation`, `Stop`: print `{}` (live-tested for `Stop` and `PostToolUse` on
1.2.9 [S11]); one tracer answers `Stop` with `{"decision": ""}` [S12]. Antigravity treats
`{"decision":"continue","reason":"..."}` from `Stop` as "keep running" [S16]; a stop hook that
always blocks is capped after a configurable number of consecutive continuations [S1 1.1.9].

### 2.6 Context injection

- `PreToolUse` cannot inject. The only model-visible channel from a tool hook is the deny
  `reason`.
- `PreInvocation` can: `injectSteps` with `ephemeralMessage` (a model-visible message that is
  not persisted; an integrator verified live on 1.2.9 that the model answered from it [S11]) or
  `userMessage` (a persisted user message, *unverified*, [S15]). `additionalContext`
  (Gemini/Claude style) is **not** supported [S15] (the issue [S7] says the event "only accepts
  `injectSteps`").
- `PreInvocation` fires before **every** model call, not once per turn or session. An injector
  must dedupe itself (integrators inject "once per conversation" using their own state [S11]).
  Because the ACS pipeline owns notice delivery state, the hook output must be empty unless the
  decision carries new context or notices.
- Whether an `ephemeralMessage` stays visible on later turns, and whether it works in interactive
  TUI sessions and on resume, was not verified [S11].

### 2.7 Mapping events to ACS phases

**Settled in stage 2 (section 13): no `Stop` hook; `PreInvocation` is `session_start` for the first invocation and `prompt_submit` after.** The table below is the original proposal.

Hooks carry the event only by command line, so the command line must carry the ACS phase.

| hooks.json event | Command | `ToolEvent.phase` |
| --- | --- | --- |
| `PreToolUse` | `acs hook antigravity --phase pre_tool` | `pre_tool` |
| `PostToolUse` | `acs hook antigravity --phase post_tool` | `post_tool` |
| `PreInvocation` | `acs hook antigravity --phase prompt_submit` | `prompt_submit` (`prompt` = `lastUserInput` when present). The first one in a conversation doubles as session start: seats are created lazily from any event. |
| `Stop` | `acs hook antigravity --phase session_end` or no hook | `Stop` is the end of an **agent turn**, not of the session (it fires with `executionNum`/`fullyIdle`). Mapping it to `session_end` releases the seat's claims every time the agent goes idle waiting for the user, and the next event must re-open the seat. This is only right if WP2.1 seat resolution revives an ended seat for the same `harness_session_id`; otherwise register no `Stop` hook and rely on claim TTL and crashed-session detection. Resolved: do not register `Stop`. |

There is no true session-end signal in Antigravity.

## 3. Tools and `ToolEvent` mapping

Tool names depend on the Antigravity build. The official changelog does not announce the
rename, but two independent integrators observed it: `write_to_file`, `replace_file_content`,
`view_file` (<= 1.4.2) became `write_file`, `edit_file`, `read_file` in **1.4.3** [S9, S10],
with the path argument `TargetFile`/`AbsolutePath` becoming `Path`. The Python SDK's built-in
tool ids are `create_file`, `edit_file`, `view_file`, `run_command` [S4]. The adapter must accept
every spelling below. Argument keys are PascalCase, but their casing is not guaranteed
[S19]: look keys up case-insensitively.

| Tool name(s) | `kind` | Path field(s) | Text / command fields |
| --- | --- | --- | --- |
| `run_command` | `shell` | none (the command text is the target) | `CommandLine`; `Cwd` was in args <= 1.4.2 and was removed in 1.4.3 [S10]. Also `WaitMsBeforeAsync`, `toolSummary`, `toolAction`. |
| `write_to_file` (<= 1.4.2), `write_file` (>= 1.4.3), `create_file` (SDK) | `write` | `TargetFile`, `Path` | content `CodeContent` / `Content`; no old text, so `edit = None`. `Overwrite` flag; `Append` on `write_file` [S10]. |
| `replace_file_content` (<= 1.4.2), `edit_file` (>= 1.4.3) | `edit` | `TargetFile`, `Path` | old `TargetContent` / `Target`, new `ReplacementContent` / `Replacement` -> `EditInfo`. Also `StartLine`, `EndLine`, `AllowMultiple`, `Instruction`, `Description`. |
| `multi_replace_file_content` | `edit` | `TargetFile` | `ReplacementChunks[]` of `{TargetContent, ReplacementContent, ...}` (*unverified shape*). `EditInfo` only when there is exactly one chunk; otherwise `None`. |
| `view_file`, `read_file`, `view_code_item`, `view_file_outline` | `read` | `AbsolutePath`, `Path`, `File`; `view_code_item` has symbol `NodePaths` which are **not** paths [S19] | none |
| `list_dir`, `list_directory`, `find_by_name`, `find_file`, `grep_search`, `search_directory` | `read` | `DirectoryPath`, `SearchPath`, `SearchDirectory` | `Pattern`, `Query`, `Includes` |
| `call_mcp_tool` | `other` | none | `ServerName`, `ToolName`, `Arguments` (older builds: `toolName`) [S8, S10] |
| `invoke_subagent`, `define_subagent`, `manage_subagents`, `send_message`, `manage_task`, `schedule`, `ask_question`, `ask_permission`, `list_permissions`, `search_web`, `read_url_content`, `generate_image`, `run_workflow`, `list_resources`, `read_resource`, `finish` | `other` | none | none |

- **No delete or move tool** appears in any source or in the SDK's list. Deletes and moves
  happen through `run_command`, which `acs-shell` already analyses. `delete` and `move` kinds are
  never produced.
- `tool.name` should be the harness name as given, but the shell dialect rule of decision 0011 §3
  keys on it (see below).
- **Unknown tool names with path-like arguments** (any key ending in `path`, `file`, `dir`,
  `directory`, `uri`; `file:` URIs decoded [S19]) are a gating hazard because tool names change
  between builds. Stage 2 must choose: classify them as `write` (safe, may deny an unknown read
  tool on a claimed file) or `other` with `paths` filled in. Recommendation: `write`, and log the
  tool name to the unknown-payload store.
- **Shell dialect on Windows.** `agy` defaults its shell to **PowerShell** (changelog 1.0.10),
  using PowerShell 7 when present and falling back to Windows PowerShell [S1 1.3.3]. The tool is
  named `run_command` on every OS, so decision 0011 §3 would classify the command as bash.
  Workaround inside the frozen contract: when `cwd` or the first `workspacePaths` entry is a
  Windows path (drive letter or UNC), the adapter sets `tool.name` to `run_command (powershell)`,
  since a name containing `powershell` selects the PowerShell dialect. (The rest of the pipeline
  only looks at the name for that purpose.) No contract change needed.
- **`ToolEvent.cwd`:** hook processes of plugins do not start in the workspace. Use `Cwd` from
  the args when present (<= 1.4.2), else `workspacePaths[0]`, else the process directory.
- **`ToolEvent.harness_session_id`** is `conversationId`. For a subagent's calls it is the
  subagent's id, not the root's (the root id is available only to MCP servers, section 7). The
  `transcriptPath` of a subagent probably lives under the parent conversation's
  `.system_generated/subagents/` directory (the changelog names that directory [S1 1.2.2]), which
  would allow deriving the root id. *Unverified.* Until then each subagent looks like its own
  seat, so a parent and its subagent can conflict on claims.
- **Post-tool facts:** `result.success = (error == "")`; no exit code.

## 4. Hook invocation

```
acs hook antigravity --phase <pre_tool|post_tool|prompt_submit|session_end>
```

Matcher for `PreToolUse`: `.*`. A name list (`run_command|write_to_file|write_file|edit_file|...`)
would fail open when Antigravity renames or adds a tool, and `agy plugin validate` flags
matchers naming unknown tools. If the live check in 2.5 shows that an `ask` "no objection"
output causes prompts for reads, narrow the matcher to everything except the read tools in
section 3 rather than to a write list. The ACS adapter should answer read tools as fast as
possible; the cost is one process spawn per tool call.

Timeout: 10 seconds (never 0).

## 5. Plugins

Install forms:

- Marketplace: `agy plugin install <name>@antigravity-plugins-official`, or `/plugin` in the TUI.
- Local directory: `/plugin` -> "Install from local directory...", or `agy plugin install <dir>`
  (an install replaces the managed directory exactly, and says `(replaced existing installation)`
  [S1 1.3.2, 1.4.8]). `agy plugin validate <dir>` checks a plugin the way the loader does;
  `agy plugin import claude` converts Claude Code plugins. `agy plugin list [--json]`,
  `enable`, `disable`, `uninstall` exist.
- Direct copy: workspace `.agents/plugins/<name>/` or global `~/.gemini/config/plugins/<name>/`
  [S18, S1 1.0.x]; a plugin copied into the global folder whose MCP server needs configuration
  variables starts **disabled** until enabled [S1 1.2.11]. Plugins are discovered at session start.
- Enablement state lives in the shared `config.json` (`~/.gemini/config/config.json`) [S1].

Layout of an installed plugin (official pieces from the changelog [S1], file names confirmed by
`agy plugin install` naming "skill, agent, rule, command, MCP server, and hook"; example tree
from [S18], *community*):

```
my-plugin/
  plugin.json                  # manifest (or .antigravity-plugin/plugin.json, or .gemini-plugin/)
  hooks.json                   # hooks (NOT hooks/hooks.json)
  mcp_config.json              # MCP servers
  rules/*.md                   # rules (front matter with `trigger`)
  skills/<skill>/SKILL.md      # skills (front matter name, description; body limit 24 KB)
  agents/<agent>.md            # subagents
  skills.json rules.json agents.json   # optional component lists
  bin/                         # command line tools the agent can run by name
```

- `plugin.json`: `name` (lowercase kebab-case; the marketplace cannot publish others), and
  optional `displayName`, `description`, `version`, `logo` (relative path, forward slashes),
  `suggestedPrompts`, `skills` (array, or a single path string), `author` (string or
  `{name,url}`), `homepage`, `repository` (https). An unknown field is a **warning** in
  `agy plugin validate` ("did you mean"); [S18] says some top-level fields make the loader drop
  the plugin, which is *unverified* and conflicts with the validator's wording. Keep the
  manifest minimal and run `agy plugin validate --strict` in stage 2 if `agy` is available.
- Not loaded from plugins: `commands/`, `workflows/` Markdown [S1 1.4.3]. **Workflows are
  workspace/global only**, so the plugin cannot ship one; a rule does the same job.
- Variables: `PLUGIN_DATA` (per-plugin data folder) [S1 1.3.2]; Gemini-style `${extensionPath}`
  resolves to the install directory [S1]. `${CLAUDE_PLUGIN_ROOT}` is expanded only by `agy plugin
  import claude`. The native plugin root variable, if any, is **not documented**: do not use a
  placeholder; rely on `acs` being on `PATH`.
- Plugin MCP servers are namespaced `<plugin>_<server>` [S1 1.2.2], so a server `acs` in plugin
  `acs` appears as `acs_acs` in `call_mcp_tool.ServerName`. Plugin MCP servers resolve relative
  working directories against the plugin directory [S1].
- **Plugin hooks and vetoes.** One integrator states plugin hooks are "observe/side-effect only"
  on Antigravity [S17]. Read closely, that wiki's hooks use Claude style exit-code vetoes, which
  Antigravity ignores, so "observe-only" is explained by the missing JSON decision. The
  changelog supports that plugin hooks gate: a disabled plugin's hooks "could break file-editing
  tools" [S1 1.1.7] and `/hooks` lists plugin hooks. Still, verify that a plugin-bundled
  `PreToolUse` deny reaches the model; the workspace `.agents/hooks.json` is the fallback
  location for a veto.

## 6. Skills, rules, workflows, agent instructions

| What | Workspace | User | Notes |
| --- | --- | --- | --- |
| Skills | `.agents/skills/<name>/SKILL.md` (legacy `.agent/skills/`) | `~/.gemini/config/skills/` | front matter `name`, `description`; `skills.json` manifests in every `.agents/` directory between the working directory and the project root load [S1 1.2.16] |
| Rules | `.agents/rules/*.md`; also `AGENTS.md`, `GEMINI.md` | `~/.gemini/config/rules/`, `~/.gemini/GEMINI.md`, `~/.gemini/AGENTS.md`, and the same under `~/.gemini/config/` | rule front matter needs `trigger` (`always_on`, `model_decision`, `glob`, `manual`); `model_decision` needs `description`, `glob` needs `globs` [S1 1.4.1]; dedicated 20,000-token budget for user + workspace rules [S1]; rules "explicitly override earlier built-in instructions" |
| Workflows | `.agents/workflows/*.md`, invoked as slash commands | global equivalents | not loadable from plugins |
| Subagents | `.agents/agents/*.md` | `~/.gemini/config/agents/` | |
| Manifests | `skills.json`, `rules.json`, `agents.json`, `plugins.json` in `.agents/` or `~/.gemini/config/` | | entries load only items directly inside a directory; `include_only` for nested |

Workspace rule paths are consistent in third-party material [S18, S20]; the official page URLs
(`antigravity.google/docs/rules-workflows`, `/docs/skills`, `/docs/mcp`) were named in search
results but are unreadable here.

**Tier B fallback rule.** A workspace rule `.agents/rules/acs.md` with front matter
`trigger: always_on` telling the agent to call `acs_status` at the start of every task, and to
read denial messages as instructions, is cheap and should ship with the plugin even though
hooks work, because `PreInvocation` injection is soft and unverified in interactive sessions.

## 7. MCP

- **User config:** `~/.gemini/config/mcp_config.json` (moved there from a legacy
  `antigravity-cli` path [S1 1.0.x]); `agy mcp add|remove|list|enable|disable` edit only this
  file (`--type`, `--env`, `--header`) [S1 1.1.16].
- **Workspace config:** `<workspace>/.agents/mcp_config.json` [S8, S19, S20]. Official
  confirmation is missing; `agy mcp add` only writes the user file.
- **Format** (stdio servers) [S8]:

  ```json
  { "mcpServers": { "acs": { "command": "acs", "args": ["mcp", "--harness", "antigravity"] } } }
  ```

  HTTP servers use `"type": "http"`/`url` and `headers`. Other recognised keys: `env`,
  `disabled`, `enabledTools`, `timeoutSeconds`, `tools.eager`. `//` and `/* */` comments and
  trailing commas parse; a malformed entry is skipped without taking down the others [S1].
- **Tool discovery:** MCP tools are loaded **on demand** and called through the `call_mcp_tool`
  dispatcher (`ServerName`, `ToolName`, `Arguments`). A server can mark tools as always loaded
  with `_meta: {"antigravity/alwaysLoad": true}` on the tool in its `tools/list` response [S1
  1.3.3]. **Note for WP2.6:** `acs_status` and `acs_claim` should set it, otherwise the agent may
  not see them without a discovery step.
- **Session id for MCP seat binding (decision 0011 §5): available, per call.** Since 1.4.4 every
  MCP `tools/call` request carries `params._meta["antigravity.google/root_conversation_id"]`, the
  **top-level** conversation id, also when a subagent makes the call [S1 1.4.4]. It is **not**
  given at `initialize`, and the processes of MCP servers are **not** documented to receive the
  id in their environment. One integrator reports that *hook* processes inherit `agy`'s full
  environment plus `ANTIGRAVITY_CONVERSATION_ID` (used for resume) [S9]; whether MCP servers get it
  is untested. So binding is: read `_meta` on each `tools/call`; with an older `agy` or no
  `_meta`, fall back to decision 0011's "most recently seen live seat of this harness in this work
  tree". The hook `conversationId` equals the root id for the main agent, so the two bind
  together; subagent hook events carry their own id (section 3).
- MCP tool calls appear to `PreToolUse` as `call_mcp_tool` and are permission-gated like
  `run_command` [S13].

## 8. Environment variables

| Variable | Where | Source |
| --- | --- | --- |
| `ANTIGRAVITY_CONVERSATION_ID` | hook processes (reported); used to resume (`--conversation`) | [S9], one integrator, *unverified*, no version given |
| `PLUGIN_DATA` | plugin processes | [S1 1.3.2] |
| `GIT_EDITOR`, `HGEDITOR`, `JJ_EDITOR` = `false` | agent shell commands | [S1 1.3.6] |
| `AGY_ADC_AUTH`, `AGY_CLI_*`, `GEMINI_API_KEY`, `GOOGLE_GEMINI_BASE_URL` | agy configuration only | [S1] |

No `ANTIGRAVITY_SESSION_ID`, no project-dir variable, and no event-name variable exist in any
source. Use stdin for everything.

## 9. Settings and permissions files

| File | Purpose |
| --- | --- |
| `~/.gemini/config/config.json` | shared settings, plugin enablement, "always allow" rules [S1] |
| `~/.gemini/antigravity-cli/settings.json` | CLI settings (`permission.allow`, `permissions`, `toolPermission`, `modelProvider`, `queuedMessages`, ...) [S1, S5] |
| `~/.gemini/config/projects/` | per-project permission configs (take precedence over global) [S1 1.0.x] |
| `~/.gemini/antigravity-cli/cache/projects.json` | workspace to project map; no workspace-local `.antigravitycli` any more [S1] |

`"toolPermission": "always-proceed"` auto-approves tools; `--dangerously-skip-permissions` skips
prompts but **not** hook denials [S5]. All of these sit outside the repository.

## 10. Failure behaviour summary

| Situation | Antigravity does |
| --- | --- |
| `PreToolUse` hook crash, timeout, invalid JSON, `{}`, unknown decision | blocks the tool (1.4.x) |
| `PostToolUse` hook failure | logged; tool keeps its result |
| Hook file not found / relative command from another cwd | exit 127, which counts as a failed hook (blocks `PreToolUse`) [S8] |
| Workspace not trusted | workspace `hooks.json` is not loaded |
| `agy --print` with no `workspacePaths` | events without a workspace |

A missing `acs` binary on `PATH` therefore **fails closed**: every tool call is denied. The setup
skill and `acs doctor` must verify the binary resolves before the hooks are installed.

## 11. Sealed paths

Repository-relative globs in the spec's dialect (SPEC section 5). They target the files an
agent in the workspace could edit to switch ACS off or bypass it. The current default
`.agents/**/hooks*.json` is **still the right location** (`**` matches zero segments, so it
covers `.agents/hooks.json`); it only misses the legacy `.agent/` directory.

**Required (proposed in stage 1; the orchestrator applies `**/`-prefixed `.agents`/`.agent` hooks and plugins globs to `DEFAULT_SEALED` and not `mcp_config*` or the root `.antigravity-plugin/`, `.gemini-plugin/` folders):**

```
.agents/**/hooks*.json
.agent/**/hooks*.json
.agents/**/mcp_config*.json
.agent/**/mcp_config*.json
.agents/plugins/**
.agent/plugins/**
.agents/plugins.json
.agent/plugins.json
.antigravity-plugin/**
.gemini-plugin/**
```

Why each: hooks files switch the gate off; `mcp_config*.json` removes or repoints the ACS MCP
server; `plugins/**` and `plugins.json` enable/disable or replace the ACS plugin (the plugin's
own `hooks.json` and `mcp_config.json` live there; the first two globs already match
`.agents/plugins/acs/hooks.json` through `**`, but `plugin.json` and the ACS rule/skill files
need the broader glob); the manifest folders cover a repository that carries a plugin at its
root.

**Recommended only if the ACS rule and skills are installed into the workspace** (the plugin
form installs globally and leaves no repository files):

```
.agents/rules/acs*.md
.agent/rules/acs*.md
.agents/skills/acs*/**
.agent/skills/acs*/**
```

**Considered and not recommended in the defaults:** `AGENTS.md`, `GEMINI.md`, `.agents/rules/**`,
`.agents/skills/**`, `.agents/workflows/**`. An agent that rewrites these can weaken the
instruction layer (tier B) but cannot disable the hook gate, and sealing them blocks ordinary
legitimate edits. Projects that want them sealed can add them in `guards.toml`.

**Cannot be sealed by repository globs** (all outside the work tree, so ACS's own
`path_escapes_repo` ask applies to write tools, and `agy` itself asks before editing
`~/.gemini/config`): `~/.gemini/config/hooks.json`, `mcp_config.json`, `config.json`,
`plugins/**`, `~/.gemini/antigravity-cli/settings.json`. This is a real gap for shell commands
that write there; list it in the threat model.

## 12. Gaps and disagreements between sources

1. **Official docs unread** (network policy). The summaries returned by a search tool say the
   official hooks page lists tool arguments (`write_to_file` with `TargetFile`, `Overwrite`,
   `CodeContent`, ...) and PostToolUse fields; not read directly [S20].
2. **Payload key style:** one guide shows Claude-style `session_id`, `transcript_path`,
   `hook_event_name` [S8]; every integrator who ran `agy` sees camelCase `conversationId`,
   `transcriptPath`, with no event name [S9, S11, S12, S14, S15]. The adapter should accept both
   (`conversationId` or `session_id`) and say which it saw in a log line.
3. **Decision spelling:** `allow_tool` / `deny_reason` [S8] versus `decision` / `reason` [S5, S6,
   S9, S13, S14, S16, S19]. The guide may describe an older or different build.
4. **`allow`:** ignored by the CLI, honoured by the app (section 2.5).
5. **Empty output:** allow (1.3.3, [S9]) versus deny (1.4.1+, [S5]).
6. **`SessionStart`:** fires (CLI, [S12]) versus never fires ([S15] and its experiment).
7. **`lastUserInput`:** added in 1.4.7 per the changelog versus "never sent" on 1.2.14.
8. **Tool names and argument names changed in 1.4.3** (section 3); the changelog is silent.
9. **PreInvocation `terminate`:** ignored on 1.2.14 [S7], supported from 1.4.11 [S1].
10. **Plugin manifest strictness:** [S18] versus the validator's warnings.
11. **Subagent session ids** and the `PostToolUse` lack of results (no exit codes).
12. **Version drift:** `agy` auto-updates across patch versions in a running session [S21]; the
    payload contract may change silently. The adapter must ignore unknown fields and the
    unknown-payload path (decision 0011 §3) must stay allow-with-warning. Note the interplay with
    section 2.5: an adapter that fails to parse must still print a valid explicit decision.

## 13. As implemented (stage 2)

Code: `crates/acs-adapters/src/antigravity.rs` (registered as `antigravity`); tests:
`crates/acs-adapters/tests/antigravity_fixtures.rs` (every file in `tests/fixtures/antigravity/`
is parsed or rendered; a test fails when a new fixture is not covered). Plugin:
`integrations/antigravity/`. Decisions: `docs/decisions/0014-antigravity-adapter.md`.

**A real Antigravity run was not possible** (not installed, `antigravity.google` blocked), so the
code is verified against documented and third-party payloads only. The binary was driven by hand
with the fixtures (`acs hook antigravity --phase ...` in a temporary repository) and printed the
shapes below, but that tests ACS, not Antigravity.

Resolved from the proposals above (orchestrator answers; the `allow` rendering changed in decision
0024, see section 15):

- `Stop` is **not** mapped and no `Stop` hook is registered. Claims expire by TTL; crashed-session
  detection covers the rest.
- ACS `allow` renders `{"decision":"allow"}` (was `{"decision":"ask"}` until decision 0024); `ask`
  renders `{"decision":"force_ask","reason":...}`;
  `deny` and `queue` render `{"decision":"deny","reason":...}`. Warnings are appended to the
  reason of a deny or ask and dropped on an allow. Exactly one JSON object, exit 0, always. An
  unparseable payload renders `{"decision":"allow"}` (the adapter does not know the event then, and
  `PreToolUse` fails closed on anything else).
- `PreInvocation` is `session_start` when `invocationNum <= 1` and `initialNumSteps <= 1` (a
  missing field does not count against it; with neither field, `prompt_submit`), else
  `prompt_submit`. Output is `{"injectSteps":[{"ephemeralMessage":"<context, then warnings>"}]}`
  when the decision has context or warnings, else `{}`. The rule is a guess: both numbers' exact
  meaning is undocumented.
- `PostToolUse` result: `success = (error == "")`, no exit code; absent `error` leaves the result
  unset. The pipeline never reads it.
- **Subagents** are separate seats (their hook `conversationId` is their own). The root id could
  only be derived from `transcriptPath` if the subagent's transcript lives under the parent's
  conversation directory; the only hint is the changelog naming a `.system_generated/subagents`
  directory (1.2.2), which does not document the transcript location. Known gap.
- Without `--phase` the phase is inferred (`toolCall` plus an `error` key is post-tool, `toolCall`
  alone is pre-tool, `invocationNum` is `prompt_submit`); this is a fallback, the plugin always
  passes `--phase`.

Rendered output, by phase:

| Phase | Allow | Ask | Deny / queue |
| --- | --- | --- | --- |
| `pre_tool` (and unparsed) | `{"decision":"allow"}` | `{"decision":"force_ask","reason":"..."}` | `{"decision":"deny","reason":"..."}` |
| `session_start`, `prompt_submit` | `{}` or `{"injectSteps":[...]}` | same as allow | same as allow (never produced) |
| `post_tool`, `session_end` | `{}` | `{}` | `{}` |

### Manual verification checklist (for a human with `agy` installed)

1. `agy --version`; note it. Allow `antigravity.google` and re-read the hooks, plugin and MCP
   pages against section 2 to 7; fix this document first if they differ.
2. `agy plugin validate --strict integrations/antigravity` passes (manifest fields, hook matcher
   `.*`, `hooks.json` at the plugin root). Then `agy plugin install integrations/antigravity`;
   `/hooks` lists the `acs` hooks; `/mcp` shows `acs_acs` ready.
3. Record one real payload per event (a hook that tees stdin to a file): `PreToolUse` for
   `run_command` and a file write, `PostToolUse`, `PreInvocation`. Compare field names with the
   fixtures (camelCase, `conversationId`, `workspacePaths`, `toolCall.args` keys, tool names for
   the installed version). Add them as fixtures before changing code.
4. Denial: have two sessions (or `acs claim` from a shell) hold `src/a.rs`, then ask the agent to
   edit it. The call is blocked and the agent quotes the reason. Repeat with a plugin-installed
   hook and with a workspace `.agents/hooks.json` hook: if only the workspace one gates, switch
   to the fallback in `integrations/antigravity/README.md`.
5. **`ask` as "no objection".** With the default permission mode, a file edit and a shell command
   that Antigravity would normally approve (or that your settings always allow) must behave as
   without ACS: no extra prompt for reads or for always-allowed commands, in the CLI, in
   `--mode auto`, and in the desktop app. If `ask` forces prompts, escalate: the alternatives are
   narrowing the matcher to side-effecting tools or emitting `allow` where the surface ignores it.
6. `force_ask`: trigger a `path_escapes_repo` ask (write outside the repository); a human prompt
   appears with the reason, and a headless run (`agy -p`) denies.
7. Fail-closed check: temporarily rename the `acs` binary; every tool call must be denied (this is
   expected, and why `acs doctor` must verify `acs` is on `PATH`). Restore it.
8. Context: start a conversation; confirm the first model call receives the ACS status
   (`ephemeralMessage`), whether it stays visible on later turns, and whether `invocationNum`
   restarts each turn (if it does, status repeats every turn and the first-invocation rule needs
   `initialNumSteps` alone).
9. MCP binding: call `acs_status` and confirm the server sees
   `params._meta["antigravity.google/root_conversation_id"]` and that it equals the hooks'
   `conversationId` for the main agent. Confirm whether `ANTIGRAVITY_CONVERSATION_ID` is in the
   MCP server's environment. Check subagent calls carry a different `conversationId` in hooks.
10. Windows: `run_command` runs PowerShell; confirm a `Remove-Item` is denied outside claims
    (the adapter names the tool `run_command (powershell)`).
11. The rule `.agents/rules/acs.md` (installed from the plugin's `rules/`) loads: ask the agent
    "what does the ACS rule say?" without ACS hooks, and check it calls `acs_status` first.

## 14. Support tier (plan section 9.2)

**Verdict: tier A with caveats, plus the tier B rule as belt and braces.**

- A `PreToolUse` hook can deny, and the reason reaches the model: yes (documented usage in [S8],
  confirmed by [S5], [S6]; `deny` is honoured even with permissions skipped).
- A hook can inject context: yes, but only through `PreInvocation` `injectSteps`
  (`ephemeralMessage`), not from `PreToolUse`, and its persistence in interactive sessions is
  unverified.
- MCP: yes; the root conversation id arrives in `_meta` per call.
- Caveats: no session start or end event, no user-prompt event (use `PreInvocation`), `allow`
  is unusable as-is, no tool results in `PostToolUse`, fail-closed behaviour when `acs` cannot
  run, subagents appear as separate sessions, build-dependent tool names, plugin-bundled hook
  vetoes unverified.

## 15. Verified (2026-11-03, `agy` 1.4.12, Windows 11)

Runs with `agy -p`, model `gemini-3.8-flash-medium`, in throwaway repositories, hooks and plugin in
the workspace only. A recording hook (stdin to a file) captured every event; the `real_*.json`
fixtures are those captures with the workspace and home paths replaced. The official pages
`antigravity.google/docs/{hooks,plugins,build-plugins,mcp,rules,skills}` were read the same day
[S24]. `agy changelog` prints the changelog offline; 1.4.12 changed nothing about hooks.

### Answers to the open points

| Point (section) | Answer |
| --- | --- |
| Hook locations (2.1) | Confirmed: workspace `.agents/hooks.json`, user `~/.gemini/config/hooks.json`, plugin root `hooks.json`. Workspace plugins in `.agents/plugins/<name>/` load too. `agy -p` loads workspace hooks and plugins with no trust prompt |
| Events (2.3) | Six, per the official reference: `SessionStart` exists and fires in the CLI once per new conversation and once per subagent, but not when a conversation is resumed with `--conversation` |
| Payload keys (2.4, 12.2) | camelCase, no event name. Every event carries `conversationId`, `workspacePaths` (forward slashes on Windows), `transcriptPath` (`transcript_full.jsonl`), `artifactDirectoryPath`, `modelName` and `lastUserInput` (empty in a subagent). `Stop`'s `terminationReason` was `NO_TOOL_CALL` |
| Counters (2.6, checklist 8) | `invocationNum` restarts at 0 every turn. First call of a new conversation: `initialNumSteps` 1; of a resumed turn: the earlier steps (4); of a subagent: 0. Decision 0024 keeps the first-invocation rule |
| Tool names (3, 12.8) | 1.4.12: `read_file {Path}`, `edit_file {Path, Target, Replacement}`, `write_file {Path, Content}`, `run_command {CommandLine, WaitMsBeforeAsync, toolSummary}` (no `Cwd`). No list or search tool was offered: the agent listed and searched through `run_command`. The official reference still lists the pre-1.4.3 names. Paths arrive with either slash |
| MCP in hooks (7) | `mcp_<server>_<tool>` (`mcp_acs_acs_acs_status`), not `call_mcp_tool`. Stream output lists `call_mcp_tool` among the tools, but the model called the MCP tools directly |
| `allow` (2.5, checklist 5) | The official reference: `allow` delegates "to the next hook or default policy"; `auto_approve` approves; `ask` prompts but respects always-allow; `force_ask` always prompts; also `deny_unless_prior_grant`, `overwrite`, `permissionOverrides`. Measured under `request-review`: no hook, `allow` and `ask` refused the same calls (`run_command`); `force_ask` refused `read_file`. Decision 0024 renders ACS allow as `allow` |
| `force_ask` (checklist 6) | Prompts (refused headless) in `request-review`. **Approved without a prompt in `always-proceed`**: a write outside the repository went through although ACS answered `force_ask` |
| Fail closed (2.5, 10, checklist 7) | Confirmed for `PreToolUse`: with `acs` missing, every call was denied with cmd's "'acs' is not recognized ..." (hook commands run through `cmd.exe`). A failing `PreInvocation` hook is logged and ignored. The build-plugins page says failures fail open; not for `PreToolUse` in the CLI |
| Plugin hooks gate (5, checklist 4) | Yes: the plugin's `PreToolUse` deny blocked `edit_file` and the model quoted the reason exactly, then did not try another way |
| Context (2.6, checklist 8) | `ephemeralMessage` from `PreInvocation` reaches the model (quoted verbatim) and is stored in the transcript as an `EPHEMERAL_MESSAGE` step |
| MCP binding (7, checklist 9) | `acs_status` listed a file the agent had just edited as its own claim: MCP and hooks share the seat |
| Hook environment (8) | `ANTIGRAVITY_CONVERSATION_ID` is set, equal to the payload's `conversationId` (the subagent's own id in a subagent). The hook's working directory is the folder holding its `hooks.json` |
| Subagents (3, checklist 9) | Separate seats, confirmed: a subagent was refused an edit of a file its parent had just created. The parent id appears only in the subagent's transcript (`[Message] ... sender=<id>`, step 1) and in MCP `_meta`. Decision 0024 leaves the fix to the orchestrator |
| Windows shell (checklist 10) | A PowerShell `Remove-Item` of an unclaimed file was refused (`delete_outside_claims`) |
| Rule (checklist 11) | The plugin's always-on rule loaded: the agent called `acs_status` and `acs_intent` unprompted and could summarize the rule |
| Validator (checklist 2) | `agy plugin validate --strict` passes; the only note is the missing logo |

### Not verified

- **The desktop app**, by the human's decision: whether `allow` skips the user's prompts there,
  whether `force_ask` prompts, whether workspace plugins and hooks load, and the counters. See
  `docs/HANDOFF.md`.
- Interactive TUI sessions (`agy` without `-p`): prompts and trust dialogs.
- macOS and Linux, a user-scope (global) plugin install, `ANTIGRAVITY_CONVERSATION_ID` in an MCP
  server's environment.

## Sources

Reliability: **official** = Google's `google-antigravity` GitHub organisation; **GDE** = a
Google Developer Expert's community article; **third party** = issues and PRs by other
integrators (several are AI-assisted and mark their own claims "unverified").

| Id | Source | Reliability |
| --- | --- | --- |
| S1 | https://raw.githubusercontent.com/google-antigravity/antigravity-cli/main/CHANGELOG.md (versions 1.0.0 to 1.4.11; entries cited as `S1 <version>`) | official |
| S2 | https://raw.githubusercontent.com/google-antigravity/antigravity-cli/main/README.md | official |
| S3 | https://raw.githubusercontent.com/google-antigravity/antigravity-sdk-python/main/google/antigravity/hooks/README.md | official (SDK, not `hooks.json`) |
| S4 | https://raw.githubusercontent.com/google-antigravity/antigravity-sdk-python/main/google/antigravity/types.py (`BuiltinTools`) and the SDK README | official |
| S5 | https://github.com/google-antigravity/antigravity-cli/issues/1053 (`allow` ignored; versions 1.2.7 to 1.4.7) | third-party report on the official tracker |
| S6 | https://github.com/google-antigravity/antigravity-cli/issues/1138 (PostToolUse payload) | same |
| S7 | https://github.com/google-antigravity/antigravity-cli/issues/1183 (PreInvocation) | same |
| S8 | https://gist.github.com/tanaikech/004f4bcdb530f1a07706c85c13dc8205 (copy of "A Developer's Guide to Agent Hooks in Antigravity CLI") | GDE |
| S9 | https://github.com/lucasschirm/claude-marketplace/issues/49 | third party (spike on a live `agy`) |
| S10 | https://github.com/novadeck-org/novadeck/pull/372 (tool signatures probed 2026-10-20) | third party |
| S11 | https://github.com/ojungo69/oboete/pull/34 | third party (live, 1.2.9) |
| S12 | https://github.com/future-agi/docs/pull/929 (IDE 2.5.5 `exa.hooks_pb` fields, CLI behaviour) | third party |
| S13 | https://github.com/T0mSIlver/starbridge/issues/962 | third party |
| S14 | https://github.com/mlorentedev/dotfiles/pull/1688 | third party |
| S15 | https://github.com/colinsurprenant/director/pull/33 | third party (live check by one author) |
| S16 | https://github.com/Morrison-Lab/ai-config/pull/2233 | third party, unverified claims |
| S17 | https://github.com/alexherrero/crickets/wiki/Hooks | third party |
| S18 | https://github.com/smartworldarafath/Google_Antigravity_Plugins | community |
| S19 | https://github.com/OualidGuennif/aeion-factory/pull/14 | third party, unverified |
| S20 | web-search result summaries mentioning `https://antigravity.google/docs/hooks/`, `.../docs/rules-workflows`, `.../docs/skills`, `.../docs/mcp` | the pages themselves were **not read** |
| S21 | https://github.com/jmmshn/bees/pull/178 | third party |
| S23 | https://raw.githubusercontent.com/google-gemini/gemini-cli/main/docs/hooks/reference.md (Gemini CLI, for the comparison) | official (Gemini CLI) |
| S24 | https://antigravity.google/docs/hooks, `/docs/plugins`, `/docs/build-plugins`, `/docs/mcp`, `/docs/rules`, `/docs/skills` (read 2026-11-03) | official; disagrees with the CLI on tool names and on failing open |

Fetched or searched on 2026-10-30. Fixtures: `tests/fixtures/antigravity/`.
