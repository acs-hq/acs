# 0043 — Antigravity: the open questions settled, and `agy` 1.5.9

Date: 2026-11-15. Status: accepted. The desktop app checks stay open (issue 6).

Decided by: the human for points 1 (writes outside the repository) and 3; Claude for the rest,
on the human's instruction to settle every open Antigravity question without asking ("don't
leave decisions for me, make your own").

## Context

Decision 0024 left four questions for the human, issue 10 asked what to call Antigravity support,
and issue 11 asked whether Claude Code should fail closed like Antigravity when `acs` is missing.
Most testing since then ran in Claude Code. On 2026-11-15 an agent re-ran the plugin on `agy`
1.5.9 (the gate ran on 1.4.12) and listed what had drifted. This record settles the questions and
records what 1.5.9 changed. The evidence is in `docs/adapters/antigravity.md` section 16 and
`docs/releases/antigravity-gate.md`.

## Decisions

1. **ACS `ask` stays `force_ask`, also under `always-proceed` (issue 7).** A person who runs
   Antigravity in `always-proceed` has chosen not to be asked, and Antigravity then approves ACS's
   ask without a prompt. ACS does not turn its asks into denials there. The human's part: a write
   outside the repository is fine, because ACS covers the repository only (it has not asked about
   those since decision 0027 anyway). Claude's part: the two asks that are left, a delete over the
   per-command limit and a risky command whose targets ACS cannot tell, are treated the same way.
   Turning them into denials would stop work the person has already said they do not want to be
   asked about, in one harness only. Everything ACS denies (claims, sealed paths, owned zones,
   self-protection) is a `deny` and holds in every mode.
2. **Subagents stay separate sessions (issue 8).** A subagent can still be refused a file its
   parent holds. The 1.5.9 payloads were checked for a way to join them: a subagent's hook
   payloads carry its own `conversationId` and nothing that names the parent (no
   `parentConversationId`, the same `cwd`, the same workspace). The parent's id is still only in
   the subagent's transcript, whose format is undocumented. Joining them would mean reading that
   file in every hook call, which the human ruled out as too big for the problem. What an agent
   can do today: release a file with `acs_release` before handing it to a subagent. Look again
   when Antigravity sends the parent's id in the hook payload. This does not block Spec 1.0: the
   spec says nothing about subagents, a session is whatever the harness reports as one, and
   accepting the gap changes no contract.
3. **Claims outlive an Antigravity session by up to `claim_ttl_secs` (issue 9). Accepted by the
   human.** Antigravity has no session-end event. No shorter time limit for Antigravity and no
   `Stop` hook. The usage skill tells agents to release what they are done with.
4. **Antigravity is supported in the CLI and says so (issue 10).** The README's "Works with
   Claude Code · Antigravity" stands. The user guide now says ACS works with Claude Code and with
   the Antigravity CLI (`agy`), and that the Antigravity desktop app has not been checked yet.
   `docs/DEFAULTS.md` no longer says to keep it from testers.
5. **Claude Code keeps failing open when `acs` is missing; no strict mode in v0.1 (issue 11).**
   ACS is aimed at any GitHub repository, so people will open an adopted repository without
   having `acs`: outside contributors, a teammate on a new machine. Blocking every tool call for
   them, as Antigravity does, is the worse default for something that guards against accidents
   and is not a security boundary. Antigravity's behaviour is the harness's own and ACS cannot
   change it. `acs doctor` checks for `acs` on `PATH` and the guide tells teams to install it
   first. A strict mode that people can turn on can be built later if testers ask for it;
   decision 0013 describes how (`onFailure: block`).
6. **Agents still may not run `acs` from their shell, in throwaway repositories too.** The rule of
   decision 0026 cannot tell which repository a command acts on, and an exception keyed on a
   scratch folder or on `ACS_HOME` would be a way around self-protection. Real-agent checks are
   scripts under `tests/e2e/real/` that set up their own repository and `ACS_HOME`; an agent may
   run those. `tests/e2e/real/antigravity.sh` is the one for Antigravity. The handoff's sentence
   that agents may run `acs init` in temp folders is corrected.
7. **The adapter reads what `agy` 1.5.4 added, and the plugin still works on older builds.**
   - `hookEventName` gives the phase when the command line does not. The plugin keeps passing
     `--phase`, which also works before 1.5.4, and `--phase` wins when both are present. `Stop`
     and `PostInvocation` name no phase: a turn end is not a session end.
   - `cwd`, the agent's working directory, is the directory a call is judged in. The first
     workspace root is the fallback for older builds.
   - `toolCall.id` is the same before and after one call. It stays in the event's `raw`; nothing
     in ACS needs it yet, and `ToolEvent` does not change.
   - `swarm` is a known tool that changes no file by path. `sed_file` and `notebook_edit` are
     named in the 1.5.9 changelog but were not offered to the model in the runs, so their
     arguments are not known: they fall under the existing rule that an unknown tool with a path
     argument is a write, and a test pins that.
8. **The plugin has a logo** (`assets/logo.svg`, the mark from `docs/assets/`), so
   `agy plugin validate --strict` passes with no notes.

## Also fixed here

- **Issue 72.** In PowerShell, `$x = "text"` (also `'text'` and `+=`) is a value whatever the
  text mentions. It used to be read as a command ACS cannot see into, and was denied when the
  text named a sealed path. `$x = <command>` and `$x = & "tool" ...` are still read as commands.

## What 1.5.9 showed that ACS does not act on

- On Windows, `agy` before 1.5.8 silently skipped hook commands containing double quotes. ACS's
  hook commands have none, so they were never affected; the plugin README now says to keep it so.
- `agy` before 1.5.8 ignored a `hooks.json` saved with a byte-order mark. `acs plugin install`
  writes none.
- `run_workflow` (for everyone since 1.5.9) runs a Python script that starts many subagents.
  Each subagent's tool calls go through the hooks as before. What the script itself writes is as
  invisible to ACS as any script run from the shell (`docs/security/overrides.md`, risk 1).
- An agent can read files outside the repository. ACS allows reads and covers the repository
  only.

## Consequences

- Issues 7, 8, 9, 10, 11 and 72 are closed by this record. Issue 6 (the desktop app) stays open
  for the human.
- **Reinstall `acs` after merging**, and run `acs plugin install antigravity` again where
  Antigravity is used (the plugin gained a file). No schema change.
