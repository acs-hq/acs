# Orchestrator handoff (after Phase 1)

Written 2026-10-29 at the end of the session that built Phase 0 and Phase 1. Read this together
with the implementation plan and `docs/decisions/` before starting Phase 2. It records what exists,
what was decided beyond the plan, and what the next phases must not miss.

## 1. Where things stand

- `main` has Phase 0 (WP0.1–0.3) and Phase 1 (WP1.1–1.6). CI (fmt, clippy `-D warnings`, tests on
  Linux/macOS/Windows, cargo-deny) is green on all three OSes.
- **Frozen contracts:** `spec/SPEC.md` (draft 0.1), `spec/schemas/` (generated; a drift test fails
  on any change), `crates/acs-core` public types, and `crates/acs-core/src/gate.rs` (in-process
  interfaces between components, decision 0004). Only the orchestrator changes them; regenerate
  schemas with `ACS_UPDATE_SCHEMAS=1 cargo test -p acs-core --test schemas`.
- Decision records `0001`–`0010` cover every choice made beyond the plan. **0002** (contracts),
  **0004** (gate interfaces and cross-component rules) and the per-crate records are required
  reading for any Phase 2 subagent.

| Crate | WP | Entry points |
| --- | --- | --- |
| acs-core | 0.2, 1.5 | types, `gate::*`, `engine::decide(&GateInput, &dyn PolicyView) -> GateOutput` |
| acs-policy | 1.1 | `load_cached(policy_dir, cache_dir, LoadOptions) -> LoadOutcome`, `CompiledPolicy: PolicyView`, `glob::Glob` |
| acs-paths | 1.3 | `PathResolver::{new, resolve, resolve_entry, resolve_recursive_report, expand_recursive, expand_glob}` |
| acs-shell | 1.4 | `analyze(command, Dialect) -> ShellAnalysis` |
| acs-store | 0.3, 1.2, 1.6 | `AcsHome`, `RepoIdentity::resolve`, `StateDb`, `SnapshotStore` |

## 2. Process lessons (for the orchestrator)

1. **Worktree subagents branch from `main`, not from your working branch.** In Phase 1 the
   interface commit was only on the session branch, so subagents started without `gate.rs` and had
   to fast-forward. Before fanning out, merge or push every contract commit to `main` (or tell each
   subagent to `git merge --ff-only <sha>` first and verify `git log`).
2. **Shared-file conflicts are routine:** every WP adds to root `[workspace.dependencies]` and
   `Cargo.lock`. Resolve `Cargo.toml` by taking the union of both sides (watch for the same crate
   added twice with different specs), then `git checkout --ours Cargo.lock && cargo metadata
   --format-version 1 >/dev/null` to re-lock, then run the full checks. Two WPs sharing a crate
   (`acs-store` in Phase 1) also conflict in its `lib.rs` and `Cargo.toml`; resolve by hand.
3. **Windows CI is the real test for path, filesystem and locking code.** Phase 1's only CI
   failure was a junction bug (fixed in `799f4ef`) that Linux could not show. Before pushing
   Windows-specific code, type-check it locally:
   `rustup target add x86_64-pc-windows-gnu` then
   `cargo clippy -p <crate> --tests --target x86_64-pc-windows-gnu -- -D warnings`
   (works for crates without C dependencies: acs-core, acs-policy, acs-paths, acs-shell's Rust).
4. CI cancels a running run when the same branch is pushed again (`concurrency`); don't push a
   follow-up commit while waiting for a run you need.
5. Six parallel subagents on a 4-CPU box worked (tell them `CARGO_BUILD_JOBS=2`). Most WPs took
   10–18 min; WP1.4 (shell analysis) took ~40 min. A long run is not necessarily a loop: check the
   worktree's commit times and file mtimes before stopping one.
6. Subagents' commit trailers name their own model (their harness attribution reminder overrides
   the brief). That is expected; don't rewrite history for it. Give merge commits an explicit
   message with the attribution trailer (`git merge --no-edit` drops it).
7. Hand each subagent: a shared rules file (reading list, ownership, definition of done, commit
   rules — the Phase 1 one is reproduced in §5), its WP, the decision-record number to use, and the
   exact interfaces it produces or consumes. Ask for a report with API, acceptance evidence,
   decisions, contract change requests and untested platforms.

## 3. Must-know items for Phase 2 (WP2.1 hook pipeline and the adapters)

**Pipeline glue (WP2.1)** — the engine is pure; the pipeline owns all of this:
- Pass an **absolute** `cwd` to `PathResolver` (relative → `Unresolvable`).
- Use `resolve_entry` for `rm`/`mv` sources (a link in the last component is the thing deleted),
  `resolve` for writes, and `resolve_recursive_report` for recursive targets (its `truncated` flag
  is not in `ResolvedPath`; treat truncated as "may reach more").
- Expand every recursive and glob target into `Expansion` (`expand_recursive`, `expand_glob`). A
  recursive delete **without** an expansion makes the engine ask (`destructive_unknown_targets`).
  The repository root is `RepoPath(".")`, which no glob matches — always expand it.
- Shell `Pattern` targets must be expanded; `Partial` targets stay unknown. A git ref containing
  `$` is unknown.
- **Local-only repos:** `StateDb::claims_snapshot` reports `remote_stale = true` when no fetch ever
  happened, and the engine then warns on every claim-checking event. Set
  `ClaimsSnapshot.remote_stale = false` when the repository has no remote / sync is not set up.
- `try_acquire(seat, requests, now, stale_after)` with `stale_after = 3 × sync_interval_secs`; call
  it only for `allow`/`ask`; on `deny` just `record_event`. Map `StoreError::is_state_unavailable()`
  to `GateState::state_unavailable`.
- The engine requests a snapshot for every write/delete target; skip paths that don't exist. Call
  `SnapshotStore::drop_session` when a session ends, or its blobs are only reclaimed by budget.
- Surface `acs_policy::LoadOutcome::warnings` in `Decision.warnings` — otherwise an invalid policy
  silently degrades to last-valid/defaults.
- `decide` returns allow/`ok` for non-`pre_tool` phases; session start/prompt/end handling is the
  pipeline's. The lore injection seam is marked in `engine` `collect_context` (Phase 6).
- Reusable test helpers: `crates/acs-core/tests/common/mod.rs` (`FakePolicy`, fixture builders).
- WP1.2 has no API yet for crashed-session detection (seats without `ended_at` and an old
  `last_seen_at`) or history pruning; WP2.1/WP4.1 must add them to `acs-store`.

**Adapters (WP2.2, WP2.3) and anything that changes built-in defaults:**
- Adapter WPs add their harness's hook/settings files to `DEFAULT_SEALED` in acs-core (an
  orchestrator change: regenerate schemas, update the SPEC table — `tests/spec_doc.rs` checks it)
  **and bump `CACHE_FORMAT` in `crates/acs-policy/src/cache.rs`**, or stale cached policies keep the
  old defaults.
- Ban matching is case-sensitive and applies to `SimpleCommand.words` joined by spaces. acs-shell
  strips wrappers (`sudo`, `env`, ...) and system bin dirs, and lowercases/strips `.exe` only for the
  PowerShell/cmd dialects — pick the dialect from the harness's shell on Windows.
- Default bans were changed to full-match globs: `git push --force*`, `git reset --hard*`,
  `git clean -f*` (decision 0005). `git push origin main --force` does not match a ban; it is caught
  by `GitInvocation.force_push` + protected branches.

**Behavior to watch when dogfooding (§0 rule 6):**
- With the default `max_files_deleted_outside_claims = 0`, an agent can delete only files its seat
  claimed earlier (by editing them or via `acs_claim`). The usage skill (WP5.1) and the MCP
  `acs_claim` tool description (WP2.6) must make this obvious. Deleting a file another seat claims
  reports `delete_outside_claims` (guards run before claims); consider naming the holder in that
  message.
- Owned zones behave as shared until WP7.1; `queue`/`claim_queued` stay reserved until Phase 8.

## 4. Candidate contract changes (decide before the WP that needs them)

| Change | Needed by | Source |
| --- | --- | --- |
| `ResolvedPath.linked_truncated: bool` | WP2.1 (optional) | WP1.3 report |
| `SimpleCommand.wrappers: Vec<String>` so bans can match `sudo *` | WP7.2 hardening | WP1.4 report |
| Optional `[lore]` config keys (inject cap 4,000 chars, stale threshold 30%) | WP6.1, WP6.2 | plan §7; decision 0002 review |
| SPEC §8: state that "stale" is strictly older than 3 × interval and never-fetched is stale | WP3.1 | WP1.2 report |
| Adapter sealed paths in `DEFAULT_SEALED` | WP2.2, WP2.3 | plan §4.3 |

Later-phase reminders: WP4.3 undo must compare current content with the session's post-image
before calling `SnapshotStore::restore` (which overwrites unconditionally); an evicted snapshot is
`SnapshotError::Evicted` ("snapshot evicted"). acs-lore (Phase 6) should reuse `acs_policy::glob`
for scopes rather than adding a second matcher. Windows file permissions under `ACS_HOME` rely on
inherited ACLs (decision 0003); `acs doctor` (WP2.5) is the place to warn.

## 5. Shared rules given to every Phase 1 subagent

Each subagent got these rules, adapted per phase:

- **Reading list:** plan §0, §3, §4, §5, §9 and its WP; `spec/SPEC.md`; all of
  `docs/decisions/`; the `acs-core` types it touches.
- **Ownership:** edit only the paths the WP owns, plus its tests and its decision record. Shared
  files may only gain entries: root `[workspace.dependencies]`, `Cargo.lock`, and the WP's own
  crate `Cargo.toml`. Contract changes go into the report, not the code.
- **Dependency order:** a crate may only depend on crates listed before it in plan §5.
- **Definition of done:** fmt, clippy `-D warnings`, workspace tests and `cargo deny check` all
  pass; public items are documented; no `unwrap`/`expect` outside tests; tests are hermetic and
  cross-platform, with `cfg` gates for OS-specific checks; the WP's own acceptance checks are met;
  a decision record exists.
- **Commits:** commit as you go, with the attribution trailer. Never push and never open PRs.
- **Resources:** run cargo with `CARGO_BUILD_JOBS=2`, and avoid `--release` builds except for
  performance checks.
- **Report:** commits, public API, acceptance evidence with numbers, decisions, contract change
  requests, untested platforms, and notes for later phases.

## 6. For the human

- **Set the repository's default branch to `main`** (GitHub → Settings → General → Default branch).
  It is currently the first branch pushed, `claude/trusting-cori-vcw7mq`.
- Consider branch protection on `main` requiring the CI checks, and CODEOWNERS for `.acs/policy/`
  and `spec/` once there are other contributors.
- Open non-code tasks from plan §9.3 still apply: the name check for `acs` (crates.io, npm,
  Homebrew), the home for the setup skill, and governance. The license is already set to
  MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`).
- The release workflow (cargo-dist 0.33.1, tag-triggered) has never run. musl and aarch64-linux
  builds with the C dependencies (SQLite, zstd, tree-sitter) are untested until WP5.2.
