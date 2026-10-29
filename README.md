# ACS

Agent Control System: coordinates multiple coding agents and humans working in one repo: guards, claims, snapshots, ledger. A single static `acs` binary is invoked by each agent harness on every tool call and decides, deterministically and fast, whether the action may proceed.

## Status

Pre-alpha, Phase 0 (foundations). The workspace builds, but no functionality exists yet.

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
