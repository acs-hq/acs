# 0053 — No npm channel, the installers come first, and the first release is 1.0.0

Date: 2026-11-22. Status: accepted. Changes decision 0021 (channels) and the version labels of
plan section 8.3.

Decided by: the human, on 2026-11-16 (npm dropped, installers first, Scoop kept, `acs` updates
itself after a release tag; issue 79) and on 2026-11-22 (the first release is 1.0.0 and carries
the updater). Claude for how it is written down.

## Context

Decision 0021 listed five channels: the shell and PowerShell installers, Homebrew (built, not
published), npm (`@acs-hq/acs`) and Scoop. Every agent tool call starts `acs` twice, so what sits in
front of the binary counts. Measured on one Windows 11 laptop with antivirus running, `acs
--version`, 30 runs each (issue 79):

| How `acs` is started | Median | p90 |
| --- | --- | --- |
| `acs.exe` on `PATH`, as the installers place it | 24 ms | 29 ms |
| npm with a plain `cmd` shim in front of the binary | 107 ms | 142 ms |
| npm with a minimal Node launcher | 224 ms | 290 ms |
| Node alone, running nothing | 77 ms | 108 ms |

The launcher `dist` generates loads more than the stand-in, so 224 ms is a lower bound. Shipping
the binary through npm without Node (per-platform packages) still leaves the `cmd` shim on Windows,
about 80 ms over the baseline.

The workspace version was `0.1.0`, and plan section 8.3 tied versions to milestones (M1 is v0.1,
M6 is v1.0). No release has been tagged.

## Decision

1. **npm is not a channel.** `dist-workspace.toml` drops the `npm` installer, `npm-scope`,
   `npm-package` and the `npm` publish job (`publish-jobs = []`); `release.yml` is regenerated with
   `dist generate`. `installers.yml` loses its `npm` job. The `npm install` of Claude Code in its
   `plugin` job stays: that installs Claude Code, not ACS. The release setup no longer needs an npm
   scope or `NPM_TOKEN`.
2. **The PowerShell and shell installers are the main way to install.** Scoop stays as it is.
   Homebrew stays built and unpublished (decision 0021).
3. **The first release is `1.0.0`.** The workspace version and the Antigravity plugin's version move
   to `1.0.0` now, so the first tag is `v1.0.0`. Release versions no longer follow the milestone
   labels of plan section 8.3: the milestones keep their gates, not their version numbers. The
   protocol version is separate and stays `acs/0.1` (`spec/SPEC.md`); "Spec 1.0" in the plan still
   means the frozen protocol, not this release.
4. **`acs` updates itself after a release tag, and the updater ships in `1.0.0`.** How is decision
   0054. Ordinary pushes to `main` release nothing.

## Consequences

- People who used npm for other tools see no `npx acs`. The README already installs from `main`
  until the first release (decision 0051); at the release it names the installers and Scoop only.
- Pre-release tags (`v1.0.0-rc.1`) are how the updater is tried before `v1.0.0`; `dist` publishes
  them as GitHub pre-releases.
- Issue 5's checklist loses its two npm items; its title and version items now say `v1.0.0`.
- Decision 0051's "until v0.1.0" now reads "until v1.0.0".
