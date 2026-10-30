# 0011 — Phase 2 architecture: crate roles, CLI surface, cross-WP interfaces

Date: 2026-10-30. Status: accepted (orchestrator), frozen for Phase 2.

## Context

Phase 2 (WP2.1–WP2.6) turns the Phase 1 components into a working gate: the hook pipeline, two
harness adapters, the git backstop, the CLI and the MCP server. WP2.1 runs alone; WP2.2, WP2.3,
WP2.4 and WP2.6 then run in parallel and WP2.5 last. The plan leaves the seams between them open
(who owns `main.rs`, how MCP reuses the pipeline when `acs-mcp` sits below `acs-cli`, where
pending-commit markers live). This record fixes them before fan-out.

## Decision

### 1. Crate roles

- **`acs-adapters`** defines the adapter interface (WP2.1) and one module per harness. WP2.1
  writes `lib.rs` with the `Adapter` trait, `HookOutput`, `AdapterError`, the `canonical`
  adapter and a registry `by_name(&str) -> Option<&'static dyn Adapter>`. Harness WPs add a
  module (`claude_code.rs`, `antigravity.rs`) plus one `mod` line and one registry arm.
  Adapters are pure: bytes in, `ToolEvent` out; `Decision` in, bytes and an exit code out. No
  I/O, no store access.
- **`acs-git`** gets local repository reads from WP2.1 (`repo.rs`: work tree root, common git
  dir, current branch from `HEAD` read directly, pilot email; `status.rs`: working-tree change
  detection for post-tool attribution). WP2.4 adds `hooks.rs` and `trailers.rs`; WP3.1 later adds
  `refs.rs`, `sync.rs`, `ledger.rs`. Network and signing shell out to `git`.
- **`acs-cli`** is a library plus the `acs` binary. `src/lib.rs` exposes:
  - `runtime` (WP2.1): opening a repository context (work tree, repo id, `RepoHome`, `StateDb`,
    compiled policy with its warnings, `SnapshotStore`), resolving or creating seats (agent seats
    from events, a human seat per pilot and worktree for CLI claims), and the shared operations
    used by the hook, the CLI and MCP: `status`, `claim_paths`, `release_paths`, `set_intent`,
    `last_decision`.
  - `hook` (WP2.1): the pre-/post-tool and session pipelines.
  - `cli` (WP2.1): the clap command tree. Later WPs add one variant and one match arm each.
  - `commands/` (WP2.4 `git_hook.rs`, WP2.5 `init/status/claim/release/off/doctor`, WP2.6
    `mcp.rs`).
  `src/main.rs` only parses arguments and calls into the library.
- **`acs-mcp`** is backend-agnostic: it implements the MCP stdio server and the five tool
  definitions against a `Backend` trait it defines. `acs-cli` (`commands/mcp.rs`) implements the
  trait with `runtime` operations. This keeps the dependency order of plan section 5 (`acs-mcp`
  never depends on `acs-cli`) and lets `acs-mcp` be tested with a fake backend.

### 2. CLI surface

```
acs hook --canonical <phase>         stdin: ToolEvent JSON; stdout: Decision JSON; exit 0
acs hook <harness> [--phase <phase>] stdin: harness payload; the adapter parses and renders
acs mcp [--harness <name>]           MCP server over stdio (WP2.6)
acs git-hook <name> [args...]        called by installed git hooks (WP2.4)
acs init|status|claim|release|off|on|doctor  (WP2.5); every command accepts --json
```

`acs hook` never makes the harness fail by crashing: internal errors map to the failure-mode
table (SPEC section 11) and still produce a rendered decision. The process reads stdin fully and
writes its output once.

### 3. Hook behavior that spans WPs

- **Shell dialect** is chosen from `tool.name` (case-insensitive): a name containing
  `powershell` or `pwsh` is PowerShell, `cmd` is cmd, everything else bash. No `ToolEvent` change.
- **Unknown payloads**: when an adapter cannot parse, the pipeline allows with `unknown_payload`,
  writes the raw payload to `RepoHome::pending_dir()/unknown/` (bounded count), and warns once
  per harness session when the adapter could recover a session id (otherwise every time).
- **Session end** releases the seat's claims and ends the seat. It does **not** call
  `SnapshotStore::drop_session`: `acs undo` (WP4.3) needs a finished session's snapshots. Ended
  sessions simply stop being "live" for `SnapshotStore::gc`, which evicts them by budget.
- **Session ids**: each seat gets one `ses_` id at creation (the value of the `ACS-Session`
  trailer and the snapshot session key).
- **Pending-commit markers** (SPEC section 7): when a pre-tool shell event contains a git
  invocation with `creates_commit`, the pipeline writes
  `RepoHome::pending_dir()/commit/<key>.json` where `<key>` is the first 16 hex chars of the
  SHA-256 of the canonical worktree path. Content: `{"session_id","seat_id","harness",
  "created_at"}`; several sessions in one worktree overwrite (last writer wins). Markers older
  than 10 minutes are ignored. WP2.4's `prepare-commit-msg` consumes it; `pre-commit` and
  `pre-push` use it to tell agent commits from human ones.
- **Local-only repositories**: with no remote configured (or sync never set up),
  `ClaimsSnapshot.remote_stale` is forced to `false` so no stale-view warning appears.
- **`acs off`** is a per-repository flag in `state.db`; sealed paths still hold.

### 4. Store additions (WP2.1, migration `0003`)

`acs-store` gains, in one migration owned by WP2.1: per-seat `session_id` and `intent`; the last
decision per seat (outcome, reason code, message, time) for MCP `acs_why`; a key/value table for
repository flags (`gate_off`); and queries for seats without `ended_at` whose `last_seen_at` is
older than a threshold (crashed-session detection, used by WP4.1). Later Phase 2 WPs use these
APIs and do not edit `acs-store`.

### 5. MCP seat binding

An MCP server process belongs to one harness session but is not told the session id by every
harness. WP2.6 binds a server to a seat in this order: a session id the harness exposes to MCP
servers (environment variable or initialize parameter, as found by WP2.2/WP2.3 research); else
the most recently seen live seat of that harness whose worktree is the server's work tree; else
it reports that no session is active yet. Binding is re-evaluated per tool call until it succeeds.

### 6. Built-in sealed paths for harness configuration

Adapter WPs research which of their harness's files can switch ACS off (hook configs, settings,
plugin enablement, MCP registration) and list them in their report and adapter doc. The
orchestrator adds them to `DEFAULT_SEALED` after merging, regenerates the schemas, updates the
SPEC table and bumps `CACHE_FORMAT` in `acs-policy` so cached policies pick them up.

## Consequences

- Shared files touched by several Phase 2 WPs are limited to: root `Cargo.toml` and
  `Cargo.lock` (union), `crates/acs-adapters/src/lib.rs` (one module line and one registry arm
  per adapter), `crates/acs-cli/src/cli.rs` and `crates/acs-cli/src/commands/mod.rs` (one variant,
  one arm, one module line per WP), `crates/acs-git/src/lib.rs` (module lines).
- `acs-mcp` depends on `rmcp` (and therefore tokio); the hook path never starts a runtime, so
  hook startup is unaffected.
