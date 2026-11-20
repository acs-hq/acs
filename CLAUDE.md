# Working in this repository

The human does not read the code. They steer through three places, and every session keeps them
current.

| Question | Where | Rule |
| --- | --- | --- |
| What is open? | GitHub issues (`acs-hq/acs`) | Anything left open at the end of a session becomes an issue, or a comment on the one that covers it. |
| What waits for the human? | Issues labelled `decision-needed` or `human-task` | `decision-needed`: a choice only they can make; put the options in the issue. `human-task`: something only they can do (push, publish, accounts, sealed files, in-app checks). |
| What was decided, and by whom? | `docs/decisions/` and the index in its README | Every decision that is not obvious from the code gets a record that says `Decided by: the human` or `Decided by: Claude`, and a row in the index. |
| What is the plan? | `docs/PLAN.md` | A change of plan is a decision record plus a row in the plan's change table. |

- Write no notes, reports or prompts outside the repository. A prompt for a later session goes in
  the body of the issue for that work.
- Creating or editing issues and labels is public: show the human the list first, unless they
  asked for that issue.
- `docs/HANDOFF.md` is for the next session, not for the human. Do not put their to-do items only
  there.
- At the end of a session, tell the human which issues you opened or closed and which decisions
  you made without them.

Standing rules for implementation work (subagents on Sonnet, branches merged to `main` as several
commits, the human pushes, where `acs init` may run, Avast) are in `docs/HANDOFF.md`, section 2.

## Commands

The toolchain is pinned to Rust 1.97.0 (`rust-toolchain.toml`). CI runs exactly these; run them
before merging:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked --no-fail-fast   # without it, stops at the first failing binary
```

- One test: `cargo test -p acs-cli --test undo <name_filter>` (integration tests are one file
  each under `crates/<crate>/tests/`).
- Scripted multi-seat scenarios, no model calls: `cargo test -p acs-cli --test e2e_scenarios`
  (source in `tests/e2e/`).
- Latency benchmarks: `cargo bench -p acs-cli --bench hook_paths` (about 20 minutes on Windows;
  `ACS_PERF_FILES=2000` to try a change, not for reported numbers).
- Regenerated outputs: `ACS_UPDATE_SCHEMAS=1 cargo test -p acs-core --test schemas` after changing
  doc comments on `acs-core` types (they feed `spec/schemas/`); `ACS_SHELL_BLESS=1` fills
  shell-corpus cases that have no `>` lines yet.
- `cargo-deny` runs only in CI. Every crate must be `publish = false`.
- `tests/e2e/real/*.sh` drive real Claude Code, Antigravity or Codex sessions and spend model
  calls: manual release checks, not CI.
- Agents here are gated by the *installed* `acs`. New engine rules reach a session only after the
  human runs `cargo install --path crates/acs-cli --locked`.

## Architecture

One binary, `acs` (`crates/acs-cli`). Harnesses call `acs hook <harness>` before and after each
tool call, and `acs mcp` serves the six `acs_*` tools. No daemon: state is a per-repository SQLite
file under `ACS_HOME`, shared between machines through `refs/acs/*` on the git remote.

Hook flow: harness stdin → `acs-adapters` (pure; harness payload ↔ canonical `ToolEvent` /
`Decision`) → `acs-cli/src/hook` (pre-tool, post-tool and session pipelines) →
`acs-cli/src/runtime` (opens the repository context, resolves seats; the operations the hook, the
CLI and MCP share) → the `acs-core` engine (pure decisions, no I/O, time passed in) → the adapter
renders the decision back.

The engine's inputs come from `acs-paths` (canonical paths, links, junctions, repository escape),
`acs-shell` (tree-sitter-bash plus a PowerShell/cmd tokenizer; whatever it cannot see is unknown,
never harmless), `acs-policy` (`.acs/policy/*.toml`, built-in guards, compiled globs) and
`acs-lore`. `acs-store` holds the SQLite state and snapshots, `acs-git` reads the repository and
installs the git backstop hooks, and `acs-os` is the only crate allowed `unsafe`.

- `spec/SPEC.md` is the contract (`acs/0.1`); the JSON Schemas are generated from `acs-core` types.
- `integrations/` holds the harness plugins, embedded in the binary. After changing them or
  `skills/acs-usage/SKILL.md` (copy it to both plugins), rebuild before the `cli_plugin` and
  `packaging` tests pass.
- Latency budget: one fresh process per hook call, about 15 ms. Count the git processes a hook
  path spawns (about 40 ms each on Windows).
