# Codex adapter: formats relied on

Added 2026-11-19 (decision 0047). Codex CLI **0.170.0** (npm `@openai/codex`) on Windows 11, logged
in with ChatGPT, model `gpt-6-luna`. Every payload below was captured from real `codex exec` runs
with a recording hook and a recording MCP server; the fixtures in `tests/fixtures/codex/` are
those captures with the user name and paths replaced. Where the official docs and a capture
disagree, the capture wins.

## 1. Sources

- The hooks page of the official docs (`developers.openai.com/codex/hooks`, which redirects to
  `learn.chatgpt.com/docs/hooks`), read 2026-11-19. It says itself that the schemas linked from it
  may be ahead of the release, so field names were checked against captures.
- The config reference and the plugin pages on the same site, read the same day.
- Captures: `codex exec --dangerously-bypass-hook-trust -s workspace-write -c projects=...` in a
  throwaway repository whose `.codex/hooks.json` ran a Node script writing stdin to a file, and a
  Node MCP server registered with `-c mcp_servers.probe...` that logged every message.

## 2. Where hooks and MCP servers are configured

Codex reads hooks from `hooks.json` files and from `[hooks]` tables in `config.toml`, in every
layer, and runs all that match: the system layer, the user layer (`$CODEX_HOME`, default
`~/.codex`), and each **trusted** project layer (`.codex/` from the project root down to the
working directory). MCP servers are `[mcp_servers.<name>]` tables in the same `config.toml` files.

ACS uses `config.toml` only (`acs plugin install codex`, `crates/acs-cli/src/commands/codex_config.rs`):
one text block between `# >>> acs` and `# <<< acs`, appended to the project's
`.codex/config.toml` (default) or the user's `$CODEX_HOME/config.toml` (`--scope user`). The block is
`integrations/codex/config.toml`. It registers `SessionStart`, `PreToolUse`, `PostToolUse` and
`SessionEnd`, each running `acs hook codex`, and `[mcp_servers.acs]` running
`acs mcp --harness codex` with `default_tools_approval_mode = "approve"`: without that, an ACS
tool call in `codex exec` fails with "MCP tool call requires approval, but approval policy is
never". `UserPromptSubmit` is not registered (decision 0036), nor `PermissionRequest`, `Stop` and
the rest, which ACS has no use for.

Codex has plugins, but a plugin is installed from a marketplace inside the TUI (`/plugins`), and
the docs say plugin hooks are supported "for plugins installed manually in Codex desktop". A
`config.toml` block works in the CLI and needs no marketplace.

**Trust.** Codex loads a project's `.codex/` only when the project is trusted
(`projects.<path>.trust_level = "trusted"`, which Codex asks for the first time it starts in a
folder), and runs a non-managed hook only after a person has reviewed and trusted it in `/hooks`.
An untrusted hook is skipped, so the gate is off until then. `--dangerously-bypass-hook-trust`
skips the review for one run (the real-agent script uses it). Where Codex stores hook trust is not
documented; `acs doctor` cannot see it.

## 3. Input payloads

One JSON object on stdin. Common fields: `session_id`, `transcript_path` (the rollout file under
`~/.codex/sessions/`), `cwd` (native path), `hook_event_name`, `model`, `permission_mode`;
turn-scoped events add `turn_id`. Captured:

| Event | Extra fields |
| --- | --- |
| `SessionStart` | `source` (`startup`) |
| `UserPromptSubmit` | `turn_id`, `prompt` |
| `PreToolUse` | `turn_id`, `tool_name`, `tool_input`, `tool_use_id` |
| `PostToolUse` | the same and `tool_response` |
| `PermissionRequest` | `turn_id`, `tool_name`, `tool_input` (no `tool_use_id`) |
| `Stop` | `turn_id`, `stop_hook_active`, `last_assistant_message` |
| `SessionEnd` | `reason` (`other`); no `model` or `permission_mode` |

The hook command runs in the session's working directory, through `cmd.exe` on Windows, as the
signed-in user (not the sandbox user; see section 7).

## 4. Tools

| `tool_name` | `tool_input` | `tool_response` | Canonical |
| --- | --- | --- | --- |
| `Bash` | `{"command": "<text>"}` | the output as a string, no exit code | `shell`. On Windows Codex runs it as `powershell.exe -Command '<text>'`, so the adapter names the tool `PowerShell` when `cwd` is a Windows path and ACS parses the command as PowerShell; `raw.tool_name` keeps `Bash`. |
| `apply_patch` | `{"command": "*** Begin Patch\n...*** End Patch"}` | `Exit code: N\nWall time: ...\nOutput:\nSuccess. Updated the following files:\nM a.txt\n` | from the patch's file lines, below |
| `mcp__<server>__<tool>` | the tool's arguments | the MCP result object | `other` |
| anything else (`update_plan`, `spawn_agent`, ...) | arguments | | `other` |

`apply_patch` paths are relative to `cwd`. A patch's file lines are `*** Add File: p`,
`*** Update File: p`, `*** Delete File: p`, and `*** Move to: q` right after an update. The
adapter's kind: only adds → `write`; adds and updates → `edit`; only deletes → `delete`; one
update with a move → `move` (`paths` = source, destination). One event has one kind, so a patch
that deletes or moves **and** changes other files is a `delete` of every path it names: deletes
are checked more strictly than writes, so nothing gets through that separate calls would not.
In the captures the model made one change per patch, and did a rename as a delete and an add
unless told to use `Move to`.

A shell call's result carries no exit code, so a finished shell call counts as successful with
an unknown exit code; an `apply_patch` result's `Exit code: N` is read.

## 5. Output

| Outcome | What the adapter prints |
| --- | --- |
| allow, nothing to say | nothing, exit 0. Never `permissionDecision: "allow"`, which would skip Codex's own approval. |
| allow with context | `{"hookSpecificOutput": {"hookEventName": <event>, "additionalContext": <text>}}`, exit 0 (`SessionStart`, `PreToolUse`, `PostToolUse`, `UserPromptSubmit`) |
| deny | `{"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": <text>}}` on stdout, nothing on stderr, **exit 0** |
| ask, queue | as deny. The docs say `permissionDecision: "ask"` is not supported: the hook run is marked failed **and the tool call goes ahead**. The reason gets a line saying the pilot can let it through with `acs override`. |

**Exit code 2 does not block.** The docs say a `PreToolUse` hook blocks with exit 2 and its reason
on stderr. Codex 0.170 on Windows did not: a test hook answering with exit 2 and the reason on
stderr, with or without the JSON deny on stdout, showed `hook: PreToolUse Failed` and the patch
was applied. The JSON deny with exit 0 showed `hook: PreToolUse Blocked` and the model got
"Blocked by PreToolUse hook: `<reason>`". The first version of the adapter used exit 2 (Claude
Code's form, where it is the backstop for a malformed document), and a real run let an edit of a
claimed file through. So any failure of the hook, a crash included, lets the call run: Codex fails
open, like Claude Code.

Context is cut to 8,000 characters: Codex's default `additionalContextLimit` is 2,500 tokens, and
longer text is written to a file with only a preview shown.

## 6. MCP session binding

Codex 0.170 sends `initialize` with `clientInfo.name = "codex-mcp-client"` and puts the session in
every `tools/call`: `_meta["x-codex-turn-metadata"].session_id` (also `thread_id`, `turn_id`,
the sandbox mode and the workspaces), `_meta.sessionId`, `_meta.threadId` and `_meta.callId`
(the same as the hook's `tool_use_id`). The session id equals the hooks' `session_id`
(capture `mcp_tools_call.captured.json`). `acs-mcp`'s `bind.rs` takes it from the turn metadata,
else `sessionId`. Codex passes the MCP server few environment variables, so a variable such as
`ACS_HOME` must be listed in the server's `env`.

In the session's tool list the ACS tools are `mcp__acs__acs_status`, `mcp__acs__acs_intent`, and
so on (`runtime::tool_names`).

## 7. Environment and sandbox

Codex's shell commands carry `CODEX_THREAD_ID` (= the session id), `CODEX_SESSION_ID`,
`CODEX_TOOL_CALL_ID`, `CODEX_SANDBOX_NETWORK_DISABLED` and `CODEX_CI`; `acs git-hook` takes
`CODEX_THREAD_ID` as the sign of an agent's commit. With the Windows sandbox
(`[windows] sandbox = "elevated"`), commands run as a separate local user
(`CodexSandboxOffline`), and git refuses the repository as "dubious ownership". So in that mode an
agent's `git commit` fails before ACS's git hooks run; hooks and the MCP server are not affected,
since Codex starts them as the signed-in user.

## 8. Self-protection

- Sealed in every repository (`DEFAULT_SEALED` and `SELF_PROTECTION_SEALED`):
  `**/.codex/config.toml` and `**/.codex/hooks.json`. Codex's marketplace file
  `.agents/plugins/marketplace.json` was already sealed.
- Sealed outside the repository: the whole `$CODEX_HOME` (`~/.codex`). Besides `config.toml` and
  `hooks.json`, a profile there (`<name>.config.toml`, used with `codex -p <name>`) can turn hooks
  off.
- Denied commands (`engine::protect::codex_off`): `codex --disable hooks` (or `codex_hooks`), a
  `-c`/`--config` override of `features`, `hooks`, `mcp_servers` or `projects`,
  `codex features disable hooks` and `codex mcp remove acs`.

## 9. Real runs (2026-11-19, Codex CLI 0.170.0, Windows 11, `gpt-6-luna`)

`tests/e2e/real/codex.sh <scratch-dir>`: a throwaway repository with `acs init` and
`acs plugin install codex`, a person's claim on `src/held.py`, and one `codex exec` session asked
to edit the held file, a free file and `.git/config` with `apply_patch`, and to delete an
unclaimed file with a shell command.

| Run | Result | What it showed |
| --- | --- | --- |
| 1 | 8 of 12 | Everything ACS did was right, but the agent called `acs_claim` before each step and released at the end, so the hooks had nothing to block and the script's checks expected the wrong thing. The prompt now allows only `acs_status` and `acs_intent`. |
| 2 | 8 of 12 | The same run again: the prompt edit had not been applied. |
| 3 | 7 of 12 | The held file was changed and the unclaimed file deleted. Replaying the payloads into `acs hook codex` gave the right deny, so Codex was ignoring it: the exit-2 finding of section 5. |
| 4 | 12 of 13 | With the JSON deny and exit 0, every gate check passed. The free file's claim was no longer in `acs status` because session end releases a session's claims. The script now reads the session record from `acs log`. |
| 5 | **13 of 13** | All checks pass. The record shows one Codex seat with the agent's intent (from `acs_intent`, so the MCP binding found the hooks' session), the claim on `src/free.py` released at session end, the file change, and the three denials (`claim_conflict`, `sealed_path`, `delete_outside_claims`). |

Not run: the interactive TUI, the trust prompts and `/hooks` review (the script passes
`--dangerously-bypass-hook-trust` and a `-c projects=...` trust for the one run), the Codex
desktop app, macOS and Linux.
