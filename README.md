# ACS

Agent Control System: coordinates multiple coding agents and humans working in one repo: guards, claims, snapshots, ledger. A single static `acs` binary is invoked by each agent harness on every tool call and decides, deterministically and fast, whether the action may proceed.

## Status

Pre-release. The gate, team sync and setup are built for Claude Code; other harnesses come later.

## Get started

Tell your agent, in the repository you want to protect:

> Set up ACS in this repository by following https://raw.githubusercontent.com/acs-hq/acs/main/skills/acs-setup/SKILL.md

It installs the binary and the plugin, writes the policy and commits the result on a branch. You only approve prompts.

## Install by hand

macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/acs-hq/acs/releases/latest/download/acs-cli-installer.sh | sh
```

Windows:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/acs-hq/acs/releases/latest/download/acs-cli-installer.ps1 | iex"
```

Or with a package manager:

```sh
brew install acs-hq/tap/acs
scoop install https://github.com/acs-hq/acs/releases/latest/download/acs.json
npm install --global @acs-hq/acs
```

Prefer the installers, Homebrew or Scoop for a repository you gate: with npm, every hook call starts Node, which is much slower. `npx @acs-hq/acs` is fine for trying the commands, but hooks need `acs` installed on `PATH`.

Then, in the repository:

```sh
acs plugin install claude-code
acs init
```

`acs plugin install` enables the plugin for this repository through `.claude/settings.json`. With `--scope user` it gates every repository you open, even those that never adopted ACS.

## Build and test

Requires the toolchain pinned in `rust-toolchain.toml` (installed automatically by rustup).

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release -p acs-cli   # produces target/release/acs
```

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
