# 0019 — Defer Antigravity and GitLab during Claude Code-only development

Date: 2026-10-31. Status: accepted (human decision; recorded by the orchestrator).

## Context

Phases 0–2 are on `main`. The Antigravity adapter and plugin exist (decision 0014) but have never
run in real Antigravity: it cannot be installed here and `antigravity.google` is blocked by the
cloud environment's network policy (`docs/HANDOFF.md` §1, §5). Nothing GitLab-specific is built
yet. Carrying both through every gate adds verification work that cannot be done in this
environment, while development runs on Claude Code only.

## Decision

Antigravity and GitLab are **deferred, not removed**.

### Stays in place

- The Antigravity adapter (`crates/acs-adapters/src/antigravity.rs`), its fixtures and tests, and
  `integrations/antigravity/`. They keep compiling and passing CI; nothing is deleted or stubbed.
- The Antigravity entries in `DEFAULT_SEALED` (decision 0017). Removing them would let an agent
  edit those configurations and switch ACS off.
- The `Adapter` trait as the only harness boundary: no Claude-specific logic outside
  `claude_code.rs`, and no code that assumes every harness sends session start, session end or
  prompt events.

### Changes until Antigravity returns

- M1 gate: three Claude Code sessions with overlapping tasks produce zero silent overwrites; every
  collision is visible in `acs status`; sealed paths hold; one-sentence setup works in Claude Code.
- WP5.1 no longer depends on WP2.3; its acceptance covers Claude Code only.
- WP5.2: `acs plugin install` covers Claude Code only.
- The WP2.3 and WP2.6 acceptance runs in Antigravity stay open.
- The pre-release manual run (plan §9.1) uses Claude Code only.

### Changes until GitLab returns

- WP3.1 verifies `refs/acs/*` pushes on GitHub only.
- WP5.3 parses GitHub CODEOWNERS only.
- WP9.2 ships the GitHub Action only.

### New work order

1. WP3.1
2. WP5.2 (Claude Code only)
3. WP5.1 (Claude Code only), with the M1 parts of WP10.1 and WP10.2; then M1 and dogfooding
4. Phases 4 and 6–8 following the dependency graph, plus WP5.3 and WP5.4 where their
   dependencies allow
5. Antigravity returns and is verified
6. Phase 9, with GitLab returning at WP9.2

Phase 10 still runs throughout and gates every release; only WP10.3 stays in M6. The M6 gate and
its two-extra-adapters requirement do not change.

### Reminders and hard stops (raised by the orchestrator with the human)

- **WP3.1 starts (reminder):** a one-off push of `refs/acs/*` to GitLab is optional now. If GitLab
  rejects it later, the fallback is a dedicated branch instead of custom refs; keep ref storage
  inside `acs-git` so the fallback stays cheap.
- **WP5.3 starts (reminder):** GitLab CODEOWNERS parsing is skipped and returns with WP9.2.
- **Before any public launch (reminder):** ACS has only been proven across Claude Code sessions,
  not across harnesses.
- **After WP8.3 merges, before WP9.1 starts (HARD STOP):** Antigravity must return. The human
  allows `antigravity.google` in the cloud environment and runs the manual checklist in
  `docs/adapters/antigravity.md`, recording results in `docs/releases/`. Agents then sync the skill
  copy, add `acs plugin install antigravity`, finish the WP2.3 and WP2.6 acceptance, and run the
  original M1 gate with two Claude Code sessions and one Antigravity session.
- **WP9.2 starts (HARD STOP):** GitLab must return: the CI template, the GitLab CODEOWNERS format
  and the `refs/acs/*` push check.

At a hard stop, work does not continue until the human answers.

## Consequences

- Antigravity must return before WP9.1 because its known gaps (ACS `allow` rendered as
  Antigravity `ask`, no session events) may require contract changes, which are cheap in draft 0.1
  and costly after Spec 1.0.
- M1 through M5 are proven on Claude Code only; any public launch before the Antigravity gate must
  say so.
- WP5.1 and WP5.2 write harness detection and `acs plugin install` keyed by harness name so
  Antigravity is one more case at the gate, not a redesign. WP5.3 keeps CODEOWNERS parsing behind
  an interface for the GitLab format.
- WP3.1 keeps all ref storage behind one interface in `acs-git`.
- The implementation plan (claude.ai artifact "ACS — Implementation Plan") was updated the same day
  to match this record.
