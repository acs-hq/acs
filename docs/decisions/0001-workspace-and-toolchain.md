# 0001: Workspace and toolchain

Date: 2026-10-29 (WP0.1)

## Context

ACS is a Rust tool whose binary is spawned on every harness tool call, so it must build reproducibly on Linux, macOS and Windows and ship as a single binary. WP0.1 sets up the workspace, CI and release plumbing before any real code exists.

## Decision

- **Workspace**: virtual workspace, `resolver = "3"`, edition 2024, members `crates/*` (eleven crates from plan section 5). Version `0.0.0`, `rust-version = "1.97"`. Library crates set `publish = false`. Package metadata is inherited via `[workspace.package]`.
- **Toolchain**: `rust-toolchain.toml` pins `1.97.0` (profile minimal, plus rustfmt and clippy). Bumps are deliberate commits.
- **Lints** (`[workspace.lints]`, inherited by every crate): `missing_docs = warn`, `unsafe_code = forbid`, `clippy::unwrap_used = warn`, `clippy::expect_used = warn`. CI runs clippy with `-D warnings`, so these are errors there. `clippy.toml` allows unwrap and expect in tests.
- **License**: `MIT OR Apache-2.0`, texts in `LICENSE-MIT` and `LICENSE-APACHE`.
- **Line endings**: `.gitattributes` forces LF so generated JSON schemas compare byte-for-byte on Windows.
- **Release profile**: `lto = "thin"`, `codegen-units = 1`, `strip = true`. `panic` is left at the default (unwind) so later code may rely on `catch_unwind` for fail-open hooks. `[profile.dist]` inherits release.
- **CI** (`.github/workflows/ci.yml`): fmt (ubuntu); clippy and test on ubuntu, macos and windows with `--locked`; cargo-deny. Action versions, looked up with `git ls-remote --tags` on 2026-10-29: `actions/checkout@v7` (latest v7.0.1), `Swatinem/rust-cache@v2` (latest v2.9.3), `EmbarkStudios/cargo-deny-action@v2` (latest v2.1.1). The pinned toolchain is installed by rustup from `rust-toolchain.toml`, so no toolchain action is used.
- **cargo-deny** (`deny.toml`): permissive license allow-list, registry restricted to crates.io, multiple versions warn, yanked crates denied. Validated locally with cargo-deny 0.20.2.
- **Release**: cargo-dist (`dist`) 0.33.1 generated `.github/workflows/release.yml` and `dist-workspace.toml`. Installers: shell and powershell. Seven targets (aarch64 and x86_64 macOS; x86_64 and aarch64 Linux gnu; x86_64 Linux musl; x86_64 and aarch64 Windows msvc). Only `acs-cli` is packaged. `pr-run-mode = "skip"` so the workflow triggers only on version tags. The workflow has not yet run for real.

## Consequences

- Later WPs add dependencies to `[workspace.dependencies]` and must keep `Cargo.lock` committed.
- Adding dependencies may require extending the `deny.toml` license allow-list.
- Cross-compiling musl and aarch64 Linux targets with C dependencies (SQLite, tree-sitter) is unverified; check on the first tagged dry run.
- The `dist` configuration may need installer tweaks (updater, Homebrew) in Phase 5.
