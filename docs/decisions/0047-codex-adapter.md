# 0047 — Codex is a supported harness: adapter, `config.toml` block, binding and seals

Date: 2026-11-19. Status: accepted.

Decided by: the human that ACS supports Codex ("Add support for codex", tested with the `codex`
CLI on this machine in a throwaway repository); Claude for everything below.

## Context

Codex CLI 0.170.0 is installed here (npm, logged in with ChatGPT). Its hooks are stable and on by
default, and follow Claude Code's protocol closely (`PreToolUse`, `permissionDecision`,
`additionalContext`). The differences that matter were found by capturing real payloads
(`docs/adapters/codex.md`): edits arrive as `apply_patch` with the patch text, the shell tool is
called `Bash` but runs PowerShell on Windows, `ask` is not supported (and fails open), project
configuration loads only in trusted projects, and every hook must be trusted once in `/hooks`.

## Decision

1. **Adapter `codex`** (`crates/acs-adapters/src/codex.rs`). `SessionStart`, `UserPromptSubmit`,
   `PreToolUse`, `PostToolUse` and `SessionEnd` map to the canonical phases; other events are an
   unknown payload. `apply_patch` paths and kind come from the patch's file lines; a patch that
   deletes or moves alongside other changes is a delete of every path, the stricter check. `Bash`
   is named `PowerShell` when `cwd` is a Windows path, so the PowerShell parser reads it.
2. **A deny is the JSON document with exit code 0, never exit 2.** The docs say exit 2 blocks;
   Codex 0.170 on Windows ran the tool call instead, and a real run let an edit of a claimed
   file through until this changed (docs/adapters/codex.md section 5). **`ask` and `queue` are
   rendered as deny**, with a line that the pilot can let the call through with `acs override`.
   Rendering `ask` as Codex's unsupported `"ask"` would let the call run.
3. **Installed as a block in `config.toml`, not as a Codex plugin.** `acs plugin install codex`
   appends the block in `integrations/codex/config.toml` (hooks plus `[mcp_servers.acs]`) to
   `.codex/config.toml`, or to `$CODEX_HOME/config.toml` with `--scope user`; there is no local
   scope. The block sits between marker lines, so the rest of the file is never rewritten and
   `acs uninstall` restores it byte for byte (the file is deleted when the block was all of it).
   A Codex plugin installs only from a marketplace in the TUI, and the docs tie plugin hooks to the
   desktop app.
4. **The ACS MCP tools are auto-approved** (`default_tools_approval_mode = "approve"`): in
   `codex exec` an MCP call that needs approval fails outright, and the ACS tools only claim,
   release and report.
5. **MCP binding** reads `_meta["x-codex-turn-metadata"].session_id`, else `_meta.sessionId`; the
   client name `codex-mcp-client` also identifies Codex.
6. **Self-protection.** `**/.codex/config.toml` and `**/.codex/hooks.json` join
   `DEFAULT_SEALED` and `SELF_PROTECTION_SEALED` (`CACHE_FORMAT` 5); the whole `$CODEX_HOME` is
   an outside seal; `codex --disable hooks`, `-c` overrides of `features`, `hooks`, `mcp_servers`
   or `projects`, `codex features disable hooks` and `codex mcp remove acs` are denied.
   `acs git-hook` treats `CODEX_THREAD_ID` as an agent's environment.
7. **No skills for Codex yet.** The session-start message and the MCP instructions carry what an
   agent must know; Codex's skill folders were not looked into.

## Consequences

- `tests/e2e/real/codex.sh` passed against real Codex 0.170.0 (docs/adapters/codex.md section 9).
  The TUI's trust prompts and `/hooks` review were not run.

- The gate is off until the person trusts the project and the ACS hooks in Codex (`/hooks`).
  `acs plugin install codex` says so; `acs doctor` cannot check it.
- With Codex's Windows sandbox, an agent's `git commit` fails on git's ownership check before
  ACS's git hooks run (docs/adapters/codex.md section 7).
- Agents can no longer edit `.codex/config.toml` in any repository with ACS, nor anything under
  `~/.codex`.
- Reinstall `acs` after merging; no state schema change.
