# 0022 — Setup and usage skills (WP5.1)

Date: 2026-10-31. Status: accepted (orchestrator).

## Context

WP5.1 owns the canonical skills and keeps the Claude Code plugin's copies in sync. The setup skill
must take a pilot from one sentence to a working gate with nothing but prompt approvals, on Claude
Code only for now (decision 0019). The binary never calls an LLM, so all judgment sits here.

## Decision

- **`skills/` is canonical.** `integrations/claude-code/skills/` holds exact copies, enforced by
  `crates/acs-cli/tests/packaging.rs`. The interim copy in `integrations/antigravity/` stays as it
  is until the Antigravity gate.
- **Harness detection is one table.** A harness is one more row. Anything else stops before
  installing anything and says it is not supported yet.
- **The commit comes before the gate starts.** `.acs/**` and `.claude/settings.json` are sealed, and
  `pre-commit` rejects an agent commit of sealed paths once the plugin is active. Tested with the
  three cases: no plugin active, the commit passes; plugin active, it is rejected with a clear
  message; the pilot's own terminal, it passes. A plugin installed mid-session stays inactive until
  a restart or `/reload-plugins`, so the normal one-sentence path commits without trouble. Rejection
  (the plugin was already active) is treated as the gate working: the agent leaves the files staged
  and hands the pilot the command. No sealed-path exception was added for adoption.
- **Only ACS's files go in the commit**, named as paths, so a pilot's unfinished work stays out.
- **The summary follows the commit**, because it reports the commit. The plan lists the summary
  first; nothing depends on that order. The skill asks before pushing or opening a pull request.
- **A freshly installed binary needs a new terminal.** Claude Code only sees the `PATH` it started
  with, and without `acs` its hooks fail open, so `/reload-plugins` is not enough in that case. The
  skill says which of the two applies.
- **`acs_lore_propose` does not exist yet** (WP6.2). The usage skill names it and falls back to the
  final message, so the text does not need changing when the tool arrives.
- **The name is "Agent Control System"**, as in the design doc and the CLI. "Agent Coordination
  Standard" in the plugin manifest, the marketplace and the interim skill was wrong; the Antigravity
  copy keeps it until that copy is synced.

## Consequences

- Acceptance run on Windows with real Claude Code (Sonnet): one sentence in a fresh repository, 53
  seconds to a commit on `acs/adopt`. Unrelated uncommitted work stayed out, nothing was pushed, and
  the agent asked before a pull request. The binary came from a local build, because no release
  exists yet, so step 2 of the skill was skipped and is not yet proven with a real agent.
- The sentence needs a public URL. The README uses the raw GitHub URL, which works once the
  repository is public; the home for the setup skill is still the human's call (plan section 9.3).
- If the plugin is already active, a pilot must make the commit themselves, which breaks "only
  approving prompts". That only happens with a plugin enabled before setup.
