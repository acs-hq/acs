# 0054 — `acs` updates itself (Windows installer installs first)

Date: 2026-11-22. Status: accepted.

Decided by: the human, that `acs` updates itself after a release tag, on by default, that Scoop
installs get a notice only (issue 79, 2026-11-16), that the updater ships in the first release,
`1.0.0` (decision 0053), and that only Windows installs replace themselves for now (2026-11-22).
Claude for how: the signature method, the interval, how the install method is detected, how agents
are kept from the switch, and the network and archive tools.

## Context

ACS is the gate on people's machines, so whoever can publish a release can change that gate
everywhere an updated `acs` runs. The hook path has a budget of about 15 ms and must never touch
the network. A running `.exe` cannot be overwritten on Windows. The checks issue 79 asked for
before any code:

1. **`dist`'s updater library (`axoupdater`).** It reads installer receipts and GitHub releases,
   but it brings a full HTTP client stack into the binary, verifies no signature, and with
   `install-updater = true` only ships a separate `acs-update` program a person must run. Not used.
2. **How the background process reaches the network today.** Team sync runs `git`; `acs` has no
   HTTP client. Windows 10 (1803 and later) and 11 ship `curl.exe` and `tar.exe` (bsdtar, which
   reads zip) in `System32`; checked on the human's machine (curl 8.22, bsdtar 3.8.9).
3. **Antivirus when a program replaces its own file.** Not knowable from a session: Avast has
   blocked fresh executables in this project before. It is the first thing the human's real run
   checks (below).

The PowerShell installer that `dist` 0.33.1 generates writes a receipt to
`%LOCALAPPDATA%\acs-cli\acs-cli-receipt.json` (or `%XDG_CONFIG_HOME%\acs-cli\` when that is set)
holding `install_prefix`, `install_layout` (`cargo-home`: the binary is in `<prefix>\bin`; `flat`:
in `<prefix>`) and `version`. It writes none when installed with `ACS_CLI_DISABLE_UPDATE` or
`ACS_CLI_UNMANAGED_INSTALL` set.

## Decision

### Which installs update themselves

- **Only release builds.** The code is compiled into every build, but it acts only when the cargo
  feature `self-update` is on, and only `dist` turns it on (`features = ["self-update"]` under
  `[package.metadata.dist]` in `crates/acs-cli/Cargo.toml`). `cargo install --path` or `--git`
  never updates itself, so a developer's own build is never replaced by a release.
- **Installer installs on Windows** replace themselves: a receipt exists and its binary directory
  (from `install_prefix` and `install_layout`) is the directory of the running `acs.exe`, compared
  case-insensitively after canonicalising.
- **Everything else gets a notice only:** Scoop (the running file is under `scoop\apps\`), a
  Windows install without a matching receipt, and every macOS and Linux install. The notice names
  the command that updates it (`scoop update acs`, or the installer line from the README).
- **Within the major version.** Automatic updates stay within the running major version (1.x).
  A newer major is a notice; `acs update` installs it when a person asks.
- **Pre-releases.** A build whose own version is a pre-release (`1.0.0-rc.1`) also considers
  pre-releases; a stable build considers only stable releases. That is how the updater is tried
  before `1.0.0` (rc.1 updates to rc.2, and rc.2 to `1.0.0`).

### When

- At most once a day (24 hours since the last check started), from the session-start hook. The
  hook only reads one small state file under `ACS_HOME` and, when a check is due, starts a
  detached `acs update --background` (detached like `runtime::sync`, with
  `acs_os::keep_stdio_from_children`). Builds without the feature return before reading anything.
  The hook never touches the network and never prints anything about updates into a session.
- Only one check runs at a time (a lock file under `ACS_HOME`), and the state records the start of
  a check before any network call, so two sessions starting together start one check.

### What a check does

1. Lists the releases of `acs-hq/acs` with
   `https://api.github.com/repos/acs-hq/acs/releases?per_page=30` (unauthenticated: 60 calls an
   hour per address, once a day is far below it). Drafts are skipped; pre-releases as above. The
   newest version by semver order is the candidate. The newest known version is stored for
   `acs doctor` whatever happens next.
2. Stops unless the candidate is newer than the running version, or a pin names it. **It never
   moves to an older version unless a pin asks for it.**
3. Downloads `acs-cli-<target>.zip` and `acs-cli-<target>.zip.sig` from
   `https://github.com/acs-hq/acs/releases/download/<tag>/` into a fresh folder under
   `ACS_HOME\update\`. `<target>` is the running build's own target (`x86_64-pc-windows-msvc` or
   `aarch64-pc-windows-msvc`).
4. Verifies the signature (below) before anything is unpacked.
5. Unpacks `acs.exe` with `tar.exe`, copies it next to the running file as `acs-new.exe`, and runs
   `acs-new.exe --version`: it must start and print the candidate version. This is also where an
   antivirus block shows.
6. Swaps: renames the running `acs.exe` to `acs.exe.old-<unix seconds>`, then `acs-new.exe` to
   `acs.exe`. If the second rename fails, the first is undone. The two renames are microseconds
   apart; a hook that starts in between finds no `acs` (Claude Code fails open, Antigravity
   denies that one call).
7. Deletes older `acs.exe.old-*` files at the start of every later check; a file still in use
   by a running process stays until a later check.

`curl.exe` and `tar.exe` are started by their absolute paths under `%SystemRoot%\System32`, never
from `PATH`, with `--proto =https --tlsv1.2 --fail --location` and a time limit.

### Signature

- **An Ed25519 key whose public half is compiled into `acs`.** GitHub artifact attestations were
  the other candidate; verifying them needs Sigstore's certificate chain and transparency log,
  which means a large dependency or the `gh` command on every machine.
- The release workflow signs, in a custom `dist` job (`.github/workflows/sign-release.yml`, listed
  in `global-artifacts-jobs` next to the Scoop manifest), each archive with `openssl pkeyutl
  -sign -rawin`, from the repository secret `ACS_RELEASE_SIGNING_KEY` (a PEM private key). The
  signed message binds the version and the file, so an old signed archive cannot be passed off as a
  newer release:

  ```text
  acs-release-v1
  <tag, for example v1.0.0>
  <archive file name>
  <lowercase hex SHA-256 of the archive>
  ```

  The `.sig` file holds the 64-byte signature in base64 on one line.
- `acs` rebuilds that message from the tag it is installing, the file name and the SHA-256 it
  computes itself, and verifies it with the compiled-in public key (crate `ed25519-compact`, no
  default features). Until the human creates the key, the compiled-in key is all zeros and every
  update is refused with "no release key in this build".
- The human creates the key and keeps the private half; a session never sees it.

### The switch, and keeping agents away from it

- Settings live in `ACS_HOME\update\settings.toml`: `auto = true|false` (default true) and
  `pin = "<version>"` (optional). `auto = false` means no contact with GitHub at all.
- `acs update` checks and updates now, in the foreground. `acs update on|off`, `acs update pin
  <version>` and `acs update unpin` change the settings. `acs doctor` shows the installed version,
  how it was installed, whether updating is on, off, pinned or notice-only, the newest known
  version and when it was checked, and the last failure.
- Agents can do none of this. `acs update` is not in self-protection's read-only list, so an
  agent's `acs update ...` is denied, and it joins the stateful list so `xargs acs update` is
  denied too (issue 12's rule for human-only commands). `ACS_HOME` is already sealed for agents;
  the receipt folder (`%LOCALAPPDATA%\acs-cli`) joins the outside seals, so an agent cannot delete
  the receipt or point it elsewhere. The environment variables a test uses to redirect the updater
  exist only in debug builds.

### Failure

No network, a failed listing, a bad signature, a version that will not start or a failed rename
leaves the running program in place, is written to the update state with the time, and is shown by
`acs doctor`. It never blocks a hook and never prints into an agent's session.

### State and policy formats

A new `acs` must read what the old one wrote, while sessions run, and teammates may be a version
apart for a day. Within major version 1: state migrations stay forward-only and automatic (as
today), nothing removes or renames a field in the claims ref, session records or policy files,
and a new field must be optional for readers. A change that cannot meet that needs a new major
version, which never arrives by automatic update.

## Consequences

- The README says that `acs` contacts github.com once a day to check for updates, how to turn it
  off (`acs update off`, or install with `ACS_CLI_DISABLE_UPDATE=1`), and that this is the only
  contact with any server besides your own git remote. `docs/wiki/` gets an "Updates" page.
  `docs/releases/README.md` says what a release must carry for the updater (the `.sig` files, the
  secret).
- **Only the human can:** create the key and store the secret (the commands are in issue 79); push
  the release tags; do the real run: install `v1.0.0-rc.1` with the PowerShell installer, publish
  `v1.0.0-rc.2`, and see the first update itself, recorded in `docs/releases/`; and add an
  antivirus exception only if that run shows one is needed.
- macOS and Linux replace themselves only after a real run on each; until then they get the notice.
- winget is a later, separate channel (issue 79, "Later").
