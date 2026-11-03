# ACS skills

The canonical agent skills. Judgment work belongs in the agent, so the `acs` binary stays
deterministic; these skills tell the agent what to do.

- `acs-setup/SKILL.md` takes one sentence from a pilot to a working gate: binary, plugin, policy,
  a commit on `acs/adopt`.
- `acs-usage/SKILL.md` is the etiquette for an agent working in an ACS repository.

Both plugins ship copies: `integrations/claude-code/skills/` and
`integrations/antigravity/skills/`. Edit the files here and copy them over;
`crates/acs-cli/tests/packaging.rs` fails if a copy differs.
