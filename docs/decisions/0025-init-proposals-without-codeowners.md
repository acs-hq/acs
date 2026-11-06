# 0025 — `acs init` proposals from history and folder names; CODEOWNERS is future work

Date: 2026-11-07. Status: accepted (human decision; WP5.3 built on the branch
`wp5.3-init-proposals`).

## Context

WP5.3 asked `acs init` to propose `territory.toml` from three sources: GitHub CODEOWNERS files
(behind a parser interface so GitLab's format could join at WP9.2), per-folder commit share by
author over the last 12 months, and folder conventions, with thresholds in config and `--yes` for
non-interactive use.

The human judged CODEOWNERS a niche input: small open-source repositories and solo developers
running several agents, ACS's main users, rarely have one. It is mostly found in company
repositories paired with GitHub's "require review from code owners" rule.

## Decision

1. **CODEOWNERS is future work, not part of the plan.** No CODEOWNERS parsing, for GitHub or
   GitLab. It can come back as an optional extra input if users ask: where a repository has one,
   it is the team's own record of ownership and may disagree with history.
2. **Proposals come from two deterministic sources** (`crates/acs-cli/src/commands/init/propose.rs`),
   with no LLM call in the binary:
   - **Folder names.** Top-level and second-level folders named `doc`, `docs`, `documentation`,
     `example`, `examples`, `samples`, `sandbox`, `playground`, `scratch` or `tmp` (any case) are
     `open`, and nothing under them is proposed as owned.
   - **Git history.** `git log` on `HEAD` for the last 12 months, no merges, at most 20,000
     commits, author emails after `.mailmap`, lowercased. `[bot]` accounts and `noreply@` or
     `no-reply@` addresses are left out (an agent committing under its vendor's address is not a
     person; GitHub's `<id>+<name>@users.noreply.github.com` addresses are kept). A commit counts
     once per folder it touched. A top-level folder with at least 10 commits where one author made
     at least 75% is `owned` by that author; otherwise each of its second-level folders is tested
     the same way. Owned zones need at least two authors in the window, because in a one-person
     repository every folder would be owned by the same person. Only folders that exist in
     `HEAD`'s tree are proposed, and names that would need glob escaping are skipped.
   - Everything else stays `shared`, the default, so it gets no zone. Sealed paths are the
     built-in defaults and are not proposed again. Teams are not inferred.
3. **The person reviews it, through the setup skill.** `skills/acs-setup` shows the proposal to
   the pilot and asks once: write it as it is, write it with changes (the agent edits
   `territory.toml` after `acs init --yes`), or skip it. Each written zone carries a comment with
   its reason ("folder name", "9 of 10 commits in the last 12 months").
4. **When it is written.** The proposal is computed while `territory.toml` is missing or still the
   untouched template (compared without line endings). It is written with `--yes`, or when the
   person answers yes at the one prompt, which appears only in a terminal, without `--json`, and
   only when the file is missing. `--yes` may replace the untouched template, and re-checks the
   file just before writing; it never replaces an edited file. Without either, `acs init` reports
   the proposal (`territory.status` = `proposed` in `--json`) and writes the template, so a plain
   re-run still changes nothing.
5. **Thresholds are flags, not config.** `--min-share <PERCENT>` (51 to 100, default 75) and
   `--min-commits <N>` (default 10). The plan put them in config, but a new `config.toml` key is a
   change to the frozen policy types and schemas, for settings used once at setup.

## Consequences

- WP5.3's reminder about skipping GitLab CODEOWNERS is gone, and the GitLab hard stop at WP9.2
  covers two items: the CI template and the `refs/acs/*` push check.
- Proposed `owned` zones are not enforced until territory enforcement (WP7.1) lands; the output, the
  written file and the setup skill all say so. Proposed `open` zones remove claims there, so two
  agents can overwrite each other's edits in those folders; the setup skill tells the pilot.
- History is read from the current branch only, and `git log --since` stops walking at commits
  older than the window, so history with commit dates out of order can be cut short. Both are
  acceptable for a proposal the person reviews.
- On this repository the proposal is two `open` zones (`docs/`, `spec/examples/`): with the agent
  address left out, one person made the recent commits, so nothing is owned.
- The other uses of CODEOWNERS in the plan and handoff (protecting `.acs/policy/` and `spec/`
  in pull requests, WP10.3's threat model) are separate from proposals and are not decided here.
