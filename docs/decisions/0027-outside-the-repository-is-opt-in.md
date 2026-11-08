# 0027 — Writes outside the repository ask only on request

Date: 2026-11-09. Status: accepted (human decision, after dogfooding WP7).

## Context

Since Phase 1 every write or delete outside the repository returned `path_escapes_repo`, an `ask`.
Dogfooding showed what that costs: each scratch file an agent wrote to its temp folder stopped the
session with an "accept once / deny" prompt. ACS has nothing to coordinate out there (no claims,
territory or other seats), and the harness's own permission rules already cover it. A prompt that
fires for harmless writes trains the pilot to click accept without reading.

## Decision

1. **`guards.toml` gains `protect_outside_repo`, default `false`.** Off, ACS is not responsible for
   paths outside the repository. On, every write or delete outside asks, as before.
2. **Two kinds of outside target still ask when it is off:** targets that cannot be resolved (ACS
   cannot tell whether they are inside), and targets outside that link back into the repository
   (the junction case of WP1.3: deleting a scratch copy that reaches live files). Those are the
   repository's business.
3. **Outside seals are unaffected** (decision 0026): writes to `ACS_HOME`, harness user settings
   and plugins, and git directories outside the work tree are denied whatever this key says. They
   are self-protection, not responsibility for the rest of the machine.

The engine gets the setting through a new `PolicyView::protect_outside_repo` (orchestrator change
to the frozen `gate.rs` trait); `Guards` gains the field (schema regenerated). SPEC sections 3.2,
5.2 and 9 are updated.

## Consequences

- Fewer prompts, so the remaining ones mean something. Under Antigravity's `always-proceed` (issue
  #7), fewer asks are approved silently, because there are fewer asks.
- A repository that wants the old behaviour sets `protect_outside_repo = true`. Existing
  repositories change behaviour on upgrade; the default `guards.toml` template documents the key.
- An agent writing outside the repository is now governed only by its harness's permissions (and
  the outside seals). Listed as residual risk 12 in `docs/security/overrides.md`.
