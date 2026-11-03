# 0024 — The Antigravity gate: real runs, `allow`, plugin install, skills

Date: 2026-11-03. Status: accepted for the CLI; **the desktop app and three questions are open for
the human** (end of this record).

## Context

Decision 0019 deferred Antigravity until after WP8.3. The human brought the gate forward: `agy`
1.4.12 is installed on their Windows machine, the official documentation at `antigravity.google`
is reachable from there, and the WP2.3 adapter (decision 0014) had never met a real Antigravity.
The human ruled out the desktop app and computer use for this work: anything that needs the app is
left for them (`docs/HANDOFF.md`).

Everything below was run with `agy -p` (headless print mode), model `gemini-3.8-flash-medium`, in
throwaway repositories with a throwaway `ACS_HOME`, and with hooks installed only in the workspace
(`.agents/hooks.json` for a recording hook, `.agents/plugins/acs/` for the plugin). No global agy
setting was changed. The results are in `docs/adapters/antigravity.md` section 15 and the release
record `docs/releases/antigravity-gate.md`; the captured payloads are the `real_*.json` fixtures.

## Decision

1. **ACS `allow` renders `{"decision":"allow"}`, no longer `{"decision":"ask"}`.** The official
   hooks reference defines `allow` as "delegating to the next hook or default policy" and has a
   separate `auto_approve` for approving without a prompt, so `allow` is exactly "no objection".
   The worry in 0014, that the app treats `allow` as approval, came from a third-party report on
   older builds. Measured in the CLI under `toolPermission: request-review`, the same task refused
   the same calls with no hook, with an `allow` hook and with an `ask` hook (`run_command` only),
   while `force_ask` refused even `read_file`. An unparseable payload also renders `allow`.
   **Not checked in the desktop app** (human checklist).
2. **ACS `ask` stays `force_ask`,** which forces a prompt in `request-review` (and so a refusal
   headless). **Finding:** in `toolPermission: always-proceed`, the mode this machine runs in,
   `force_ask` is approved without a prompt. A write outside the repository went through although
   ACS answered `force_ask` (`path_escapes_repo`). Left as a question for the human (below).
3. **Session start stays the first `PreInvocation`, and no `SessionStart` hook is registered.**
   Real counters: `invocationNum` starts at 0 in every turn; the first call of a new conversation
   has `initialNumSteps` 1, a resumed conversation's first call has the earlier steps (4 in the
   run), a subagent's first call has 0. The existing rule (`invocationNum <= 1` and
   `initialNumSteps <= 1`) gives one status per conversation and one per subagent, and none for a
   resumed turn. `SessionStart` does fire in the CLI (also for subagents, not for `--conversation`
   resumes), but nothing says it fires in the app, and registering both would inject the status
   twice. A payload without `invocationNum` now keeps the phase it was given (`SessionStart` given
   `--phase session_start` used to become `prompt_submit`).
4. **ACS's own MCP tools are `other`.** In hooks, MCP tools are named `mcp_<server>_<tool>` (the
   plugin's server is `acs_acs`, so `mcp_acs_acs_acs_claim`), not `call_mcp_tool` as 0014 assumed.
   `acs_claim` and `acs_release` take `paths`, so they would have fallen into the "unknown tool with
   paths is a write" rule. Other MCP tools keep that rule.
5. **`acs plugin install antigravity`.** ACS is not in Antigravity's marketplace, so the plugin is
   built into the binary (from `integrations/antigravity/`) and written to the workspace plugin
   folder `.agents/plugins/acs/`, which Antigravity loads at session start. That is the project
   scope, and the only one offered: a user-scope install would edit `~/.gemini/config`, which was not
   tested. It then runs `agy plugin validate` on the folder. Running it again writes nothing; a
   changed file is put back. `acs init` and `acs doctor` print this command now.
6. **Skills are synced.** Both plugins ship exact copies of `skills/acs-setup` and
   `skills/acs-usage` (`packaging.rs` checks both). The usage skill names the tools for both
   harnesses; the setup skill has an Antigravity row (detected by `ANTIGRAVITY_CONVERSATION_ID`) and
   per-harness install, commit and restart steps.

## What the real runs showed

- The plugin, installed only in the workspace, works end to end: `agy plugin validate --strict`
  passes, the `PreToolUse` deny reaches the model with the reason quoted exactly, the model did not
  try another way, `PreInvocation` injection reaches the model (it quoted the status, and the
  transcript stores it as an `EPHEMERAL_MESSAGE` step), and `acs_status` over MCP is bound to the
  same seat as the hooks (a file the agent just edited was listed as its own claim).
- A PowerShell `Remove-Item` of an unclaimed file was refused (`delete_outside_claims`).
- **Fail closed, confirmed:** with `acs` missing from `PATH`, every tool call was denied
  (`'acs' is not recognized ...`, so hook commands run through `cmd.exe` on Windows). A failing
  `PreInvocation` hook is only logged. The official plugin-authoring page says hook failures fail
  open; for `PreToolUse` in the 1.4.12 CLI they do not.
- The 1.4.12 tool set in these runs was `read_file {Path}`, `edit_file {Path, Target,
  Replacement}`, `write_file {Path, Content}`, `run_command {CommandLine, WaitMsBeforeAsync,
  toolSummary}` (no `Cwd`), `invoke_subagent`, `send_message` and the MCP tools. The official
  reference still lists the pre-1.4.3 names (`view_file`, `write_to_file`, ...). The model sends
  file paths with forward slashes or backslashes, so the adapter must accept both (it does).
- Hook processes run in the folder that holds the `hooks.json` (`<workspace>/.agents` for workspace
  hooks), and get `ANTIGRAVITY_CONVERSATION_ID`, equal to the payload's `conversationId`.
- `agy -p` loads workspace hooks and plugins without a trust prompt.
- **The original M1 gate** (two Claude Code sessions and one Antigravity session,
  `docs/releases/antigravity-gate.md`): zero silent overwrites and every collision in `acs status`
  in both runs; with the ACS tools allowed all three tasks landed (PASS). With them withheld, a
  finished Antigravity session's claims stayed live for their TTL (no session end) and blocked the
  third task, and the agent drove the human `acs` CLI (`acs claim`, `acs release`), which acts on
  the human seat.

## Open for the human

1. **The desktop app** (the human runs these; listed in `docs/HANDOFF.md`): that `allow` does not
   skip the user's own prompts, that `force_ask` prompts, that workspace plugins and hooks load,
   and that `SessionStart` and `PreInvocation` behave as in the CLI.
2. **`force_ask` under `always-proceed`.** Options: keep it (the user chose not to be asked; the
   reason still reaches the model only if it is denied), or render ACS `ask` as `deny` with the
   reason ("ask your pilot to run this"), which is safe everywhere but stops work a human would have
   approved. ACS asks are rare: writes outside the repository, deletes over the per-command budget,
   destructive commands with unknown targets.
3. **Subagents are separate seats, confirmed.** A subagent was refused an edit of a file its parent
   had just created. Its hooks carry its own `conversationId`, the environment carries the same, and
   its transcript lives in its own folder; the parent's id appears only in the subagent's transcript
   (step 1, `[Message] ... sender=<parent id>`) and in MCP `_meta`. Joining them would mean the hook
   reading an undocumented transcript format, which conflicts with adapters having no I/O (0014).
   Options: accept the gap and document it, or read the sender in the pipeline (not the adapter),
   behind a check that falls back to separate seats. Needs the orchestrator's call before Spec 1.0.
4. **Claims outlive an Antigravity session** by up to `claim_ttl_secs`, because no session end
   arrives; only an explicit `acs_release` frees them sooner. Options: accept it (the usage skill
   already says to release); a shorter TTL for seats of harnesses without session events; or a
   `Stop` hook that releases claims when `fullyIdle` is true, which 0014 rejected because `Stop`
   fires at every turn end and would drop a seat's claims while it waits for the user.
5. **Agents can run the human `acs` CLI** (`claim`, `release`, `off`) from their shell. This is
   WP7.2's scope; the gate run makes it concrete.

## Consequences

- `docs/adapters/antigravity.md` section 15 replaces the unverified claims it answers; section 13's
  checklist is done for the CLI.
- The interim Antigravity usage skill is gone; `integrations/antigravity/` is now built into `acs`,
  so a change there needs a rebuild to reach `acs plugin install antigravity`.
- `tests/e2e/real/three_sessions.sh` runs the original M1 gate with `ACS_REAL_ANTIGRAVITY=<task>`.
