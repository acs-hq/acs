# 0031 — First testers before the spec, and first-run defaults

Date: 2026-11-11. Status: accepted. Decided by: the human, on Claude's recommendations. Nothing in
this record is built yet; the work is in the GitHub milestone "First testers".

## Context

Phases 0 to 7 and the M1 parts of Phase 10 are built. The plan's next steps were Phase 8 (symbol
claims, interface notices, queued edits), the Antigravity hard stop, then Phase 9 (Spec 1.0,
forges). The human wants people to try ACS before any of that, starting with friends.

Today a fresh `acs init` turns on everything: blocking claims, the delete rule, team sync and a
territory prompt. Several open bugs make blocking claims costly for a first-time user: a person
cannot release an agent's claim (issue 34), one long command loses a session's claims (32), a
blocked session leaves half-done work (37), and a person's commit can be rejected as an agent's
(38). Sync pushes file paths, intent text and the pilot's git email to `refs/acs/*` on the remote
without asking.

## Decision

1. **Order.** First testers come before Phase 8, Phase 9 and Spec 1.0. Those are postponed, not
   removed. Phase 8 only matters once claims block. The Antigravity hard stop still applies
   before WP9.1.
2. **Release work is cut to the minimum testers need:** a binary and a plugin they can install
   without a Rust toolchain. Other channels and any public announcement wait.
3. **Claims default to an observe mode** (to be built): an edit of a file another session holds is
   allowed, the agent is told who else is editing, and the event is recorded as a would-be
   collision. Blocking (`enforce`) and `off` are settings. The design gets its own decision record
   when it is built.
4. **Team sync and the ledger push are off by default.** Nothing leaves the machine until the
   repository opts in. Session records, `acs log`, `acs blame`, `acs undo` and `acs stats` stay on,
   local only.
5. **Territories stay available and enforced** when a repository configures them by hand. They do
   not depend on the claims mode.
6. **Unchanged defaults:** self-protection seals, banned commands, protected branches, the bulk
   delete limit, lore, git hooks, `protect_outside_repo = false`. Strict mode, `acs watch`, the VS
   Code extension and Antigravity stay opt-in.

## Open for the human

- Whether `acs init` still offers its territory proposal, or never asks.
- Whether `.github/workflows/**` stays a flat deny, becomes an ask, or is unsealed by default, and
  what to do about `**/.env*` also matching `.env.example`.
- Whether the `ACS-Session` commit trailer can be switched off.

## Consequences

- The wiki home page leads with "one writer per file", which will no longer describe a fresh
  install. It changes when observe mode lands.
- The usage skill and the MCP tool descriptions tell agents to claim before editing. In observe
  mode they must ask for `acs_intent` only.
- With blocking off by default, testers give no feedback on blocking unless they switch it on.
  Would-be collision counts in `acs status` and `acs stats` are the substitute.
- Work that came out of the same review, each an issue in the milestone: the worktree path bug
  from the dogfooding table, an audit of everything ACS writes to a user's repository or remote,
  a live view of sessions, `acs uninstall`, `acs undo --last`, and a report command for testers.
