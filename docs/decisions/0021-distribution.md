# 0021 — Distribution and `acs plugin install` (WP5.2)

Date: 2026-10-31. Status: accepted (orchestrator).

## Context

WP5.2 ships `acs` through cargo-dist and adds `acs plugin install <harness>`, Claude Code only until
Antigravity returns (decision 0019). The release workflow had never run. `dist plan` on the
configuration from WP0.1 failed outright: `publish = false` hid the only binary from dist.

## Decision

### Release

- `crates/acs-cli` sets `[package.metadata.dist] dist = true`, and the workspace version moves to
  `0.1.0`, so a `v0.1.0` tag matches it.
- `dist` 0.33.1 generates `release.yml`. The file is not edited by hand: `dist plan` fails on any
  difference. dist 0.33.1 is on GitHub only (crates.io stops at 0.32.0), so a local copy goes in
  `target/tools/`, with the checksum checked.
- Channels: shell and PowerShell installers, Homebrew (`acs-hq/homebrew-tap`, formula `acs`), npm
  (`@acs-hq/acs`), Scoop. The names are placeholders until the human's name check (plan section 9.3).
- **Scoop has no bucket repository.** A custom dist job renders `integrations/scoop/acs.json.in` from
  the release's checksums and ships `acs.json` as a release file;
  `scoop install <release url>/acs.json` works without a second repository or a secret. Scoop was
  picked over winget for that reason; winget needs a pull request to Microsoft's repository per
  release.
- Archives are named after the package, so the installers are `acs-cli-installer.sh` and `.ps1`.
  The package is not renamed: `-p acs-cli` is used throughout. `tests/packaging.rs` ties the names
  in the setup skill and the Scoop template to the package.
- `.github/workflows/installers.yml` tries each channel on a clean machine after a release, then
  `acs plugin install` with the real Claude Code. dist calls it after the announcement, and it can
  be run by hand with a tag. CI gains a `release plan` job so a broken release configuration shows
  on the push that broke it, not on a tag.

### `acs plugin install`

- **The default scope is `project`, not Claude Code's default of `user`.** The hook gates every git
  repository it runs in, with the built-in guards where there is no `.acs/policy` (SPEC section 5: a
  missing file means built-in defaults). Tested: in a repository that never adopted ACS, an agent's
  edit of `.env` is denied. A user-scope plugin would change how agents behave in all of a person's
  repositories. Project scope also suits adoption: `.claude/settings.json` gets `enabledPlugins` and
  `extraKnownMarketplaces`, so teammates receive the plugin with the commit. `--scope user` remains
  and warns. Claude Code runs in the work tree root, where it writes that file.
- `acs` must resolve on `PATH` first, or the command refuses (plan: installers put `acs` on `PATH`
  before any plugin is enabled).
- `already enabled` means `claude plugin list --json` has an entry with this id, `enabled: true` and
  the requested scope. The list also holds entries of other projects, but `enabled` is worked out
  for the directory `claude` runs in **[captured]**, so no path comparison is needed. A disabled
  plugin needs no special case: `plugin install` enables it again **[captured]**.
- The add and install steps are judged by exit code, not `--json` output, so an older `claude`
  works. The listing confirms the result afterwards.
- Windows: the `claude` found on `PATH` must be one Windows can start. npm puts an extensionless POSIX
  shim next to `claude.cmd`, and `find_on_path` returns the shim, so `find_spawnable_on_path` was
  added.
- `init` and `doctor` now print `acs plugin install claude-code`. A new harness is one more arm in
  `plugin::install`; `antigravity` answers `unsupported_harness` with the manual command.

### npm

`.cmd` shims work as hook commands: with only `acs.cmd` on `PATH`, Claude Code 2.1.316 on Windows
ran it and the denial reached the agent **[captured]**. This corrects
`docs/adapters/claude-code.md`, which said otherwise from the documentation. npm is still the
weakest channel for the gate, because every hook call starts Node, so the budgets in plan section 4.7
do not hold. The README steers people to the installers.

## Consequences

- A release needs setup only the human can do: `docs/releases/README.md`. If the tap or `NPM_TOKEN`
  is missing, the publish jobs fail after the GitHub Release exists, and `announce` and the installer
  checks are skipped.
- All channels download from public release URLs, so none works while the repository is private.
- Verified locally on Windows: `dist plan`, a `dist build` of `x86_64-pc-windows-msvc`, the real
  PowerShell installer from a localhost server, `.github/smoke.sh` on what it installed, and
  `acs plugin install` against the real `claude` in a sandboxed `CLAUDE_CONFIG_DIR`. Not verified
  until the first tag: every other target, Homebrew, npm, Scoop and `installers.yml` itself. In
  particular, `aarch64-pc-windows-msvc` cross-builds C dependencies in a `cargo-xwin` container.
- WP7.2 should decide what agents may do with `acs plugin install`: project scope writes the sealed
  `.claude/settings.json` through a shell command.
