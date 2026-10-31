# ACS skills

The canonical agent skills. Judgment work belongs in the agent, so the `acs` binary stays
deterministic; these skills tell the agent what to do.

- `acs-setup/SKILL.md` takes one sentence from a pilot to a working gate: binary, plugin, policy,
  a commit on `acs/adopt`.
- `acs-usage/SKILL.md` is the etiquette for an agent working in an ACS repository.

The Claude Code plugin ships copies in `integrations/claude-code/skills/`. Edit the files here and
copy them over; `crates/acs-cli/tests/packaging.rs` fails if the copies differ. The interim
`acs-usage` copy in `integrations/antigravity/` is synced when Antigravity returns (decision 0019).
