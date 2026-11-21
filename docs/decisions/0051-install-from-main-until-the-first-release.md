# 0051 — Install from `main` until the first release

Date: 2026-11-21. Status: accepted.

Decided by: the human, that someone following the README must get the newest `main`, not an old
build; Claude, for how: `cargo install --git` from `main` in the README and the setup skill.

## Context

The README's hand install and step 2 of the setup skill pointed at
`releases/latest/download/acs-cli-installer.{sh,ps1}`, Scoop and npm. No release exists yet
(issue 5), so every one of those links fails, and a tester had no working way to install `acs`
except cloning and building it.

## Decision

1. Until v0.1.0 is released, both the README and `skills/acs-setup/SKILL.md` install with
   `cargo install --git https://github.com/acs-hq/acs --branch main --locked --force acs-cli`.
   `--force` makes the same line an update, whatever the earlier install came from (a checkout
   with `--path`, or an older `main`). Rust 1.97 or later is needed (`rust-version` in
   `Cargo.toml`); the pinned `rust-toolchain.toml` does not apply to `cargo install --git`.
2. The README says how to update the Claude Code plugin too, since it comes from this
   repository's marketplace and Claude Code keeps a cached copy:
   `claude plugin marketplace update acs-plugins`, then `claude plugin update acs@acs-plugins`.
3. The setup skill still names the installers, as what this step becomes at the first release
   (the packaging test checks the names).

## Consequences

- A tester gets exactly what is pushed to `main`. Commits that are not pushed reach nobody.
- When v0.1.0 is tagged, put the installer commands back in both places (issue 5).
