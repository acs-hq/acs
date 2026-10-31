# Orchestrator handoff (after Phase 3)

Written 2026-10-30 at the end of the session that built Phase 2, and updated 2026-10-31 after
Phase 3 (WP3.1). It replaces the handoff written after Phase 1; that version is in git history
(`git show 49c2cf0:docs/HANDOFF.md`) and its process lessons still apply (summarized in section 2).
Read this with the implementation plan and `docs/decisions/` before starting the next phase.

**Antigravity and GitLab are deferred (2026-10-31).** Section 6 says what changed and when the
human must be reminded. Check its triggers before starting any WP.

## 1. Where things stand

- `main` has Phase 0, Phase 1, **Phase 2 (WP2.1–WP2.6)** and **Phase 3 (WP3.1)**. The next step
  in the plan's work order (section 8.2) is WP5.2, Claude Code only.
- Fmt, clippy `-D warnings` and the workspace tests pass locally on Windows for Phase 3. Check
  that CI has run on Linux and macOS: the detached spawn has a Unix path of its own, and it has
  never been run.
- **Frozen contracts:** `spec/SPEC.md` (draft 0.1), `spec/schemas/` (drift test), `crates/acs-core`
  public types and `gate.rs`, and the Phase 2 cross-WP interfaces in decision **0011** (crate roles,
  CLI surface, markers, MCP seat binding). Only the orchestrator changes them.
- Phase 2 decision records: **0011** architecture (orchestrator), **0012** hook pipeline, **0013**
  Claude Code adapter, **0014** Antigravity adapter, **0015** git backstop, **0016** MCP server,
  **0017** harness paths in the default sealed list (orchestrator, with an amendment for
  `.husky/**`), **0018** CLI. Phase 3: **0020** team sync, including the GitHub push check and the
  `acs-os` exception to `unsafe_code = forbid`.
- Shared rules handed to every Phase 2 subagent: `docs/phase2-rules.md` (reuse it for Phase 3+,
  updating the reading list and decision numbers).

| Crate | WP | Entry points |
| --- | --- | --- |
| acs-adapters | 2.1–2.3 | `Adapter` trait, `by_name`, `canonical`, `claude_code::ClaudeCode`, `antigravity::Antigravity` |
| acs-git | 2.1, 2.4, 3.1 | `repo::discover`, `status::scan_changes`, `hooks::{install, uninstall, status}`, `trailers`, `refs::{SyncStorage, CustomRefs}`, `sync::run` |
| acs-store | 2.1 adds | migration 0003: `SeatInfo` (session id, intent), last decision, flags (`gate_off`), attributions, `stale_live_seats` |
| acs-mcp | 2.6 | `Backend` trait, `serve_stdio` (rmcp 3.8, the only async runtime in the binary) |
| acs-cli | 2.1, 2.4–2.6, 3.1 | `runtime` (`RepoContext`, seats, `status`/`claim_paths`/`release_paths`/`set_intent`/`last_decision`, `sync::{lock, spawn_if_due, spawn_now}`), `hook`, `cli`, `commands/{init,status,claim,release,off,doctor,git_hook,mcp,sync}` |
| acs-os | 3.1 | `keep_stdio_from_children`; the only crate allowed `unsafe`, one Windows call (decision 0020) |

Commands: `acs hook --canonical <phase>`, `acs hook <claude-code|antigravity|canonical>`,
`acs mcp [--harness]`, `acs git-hook <name>`, `acs init|status|claim|release|off|on|doctor|sync`
(each with `--json`).

Integrations: `integrations/claude-code/` (plugin `acs`, marketplace `acs-plugins` in the root
`.claude-plugin/marketplace.json`) and `integrations/antigravity/` (plugin `acs`, always-on rule).
Both carry an **interim** `acs-usage` skill; WP5.1 owns the canonical skills and must keep the
plugin copies in sync.

### Acceptance evidence worth knowing

- **Real Claude Code run** (`claude -p` 2.1.314 with the plugin via `--plugin-dir`): an edit of a
  file claimed by another seat was denied and the model quoted the guidance naming the holder
  (`docs/adapters/claude-code.md` §17). Separately, `claude -p` with `acs mcp` listed and called
  all five tools (decision 0016). Hooks and MCP were not yet exercised together in one real session.
- **Antigravity was never run** (not installable in the cloud environment, where
  `antigravity.google` is blocked by network policy). Its adapter rests on Google's GitHub repos
  and integrators' reports; `docs/adapters/antigravity.md` lists 12 source disagreements and an
  11-step manual checklist. Treat it as unverified until someone runs that checklist. `agy` 1.4.12
  is on the human's Windows machine, so the checklist can run there; it is deferred (section 6).
- **Budgets** (release, Linux, 4 shared vCPUs; `cargo test --release -p acs-cli --test perf --
  --ignored --nocapture`): pre-tool edit p50 8.7 ms / p99 18.4 ms; post-tool shell at 100k files
  p50 16 ms / p99 47 ms **only with the scoped scan**. A full `git status` scan of 100k files
  missed (p50 147 ms), so above 20,000 index entries the scan is scoped to predicted targets,
  claims and nearby directories (decision 0012) and opaque scripts writing elsewhere go unseen.

## 2. Process lessons

From Phase 1 (still true): subagent worktrees must start from a commit that has every contract
change; `Cargo.toml`/`Cargo.lock` conflicts are routine (union, then re-lock); Windows CI is the
real test for path/filesystem code; don't push while waiting on a CI run you need (concurrency
cancels it); give merge commits an explicit message with the attribution trailer.

New in Phase 2:

1. **Create worktrees yourself** (`git worktree add -b <wp> .claude/worktrees/<wp> <sha>`) and
   tell each subagent its absolute path. This keeps `main` untouched until the end and makes the
   base commit explicit. Don't `cd` into a worktree from the orchestrator shell (it moves the
   session's working directory); use `git -C` or absolute paths.
2. **Split adapter WPs into research and code stages.** Research (docs, real payload capture,
   sealed-path proposals) does not need the pipeline, so it ran in parallel with WP2.1 and its
   findings fed WP2.1 mid-flight (resume semantics, SessionEnd budget, detached-child stdio).
   Resume the same agent for stage 2 with `SendMessage` so it keeps its context.
3. **Start downstream WPs early with a stub.** WP2.5 started before WP2.4 landed, with the hook
   installer behind one stubbed call, and got the API by message later.
4. **Merges break each other's tests, not just code.** Run the *whole* workspace test suite after
   every merge: fixture-coverage tests (WP2.3) caught a fixture added by WP2.6, and a hardcoded
   sealed-glob count (WP2.5) broke when the defaults grew.
5. **Use unique scratchpad directory names per subagent** (`wp22-e2e`, not `e2e`); two agents
   clobbered each other's scratch repo.
6. Tell subagents that `assert_cmd` is not available offline; the tests use
   `std::process::Command` with `CARGO_BIN_EXE_acs` and the `World` fixture in
   `crates/acs-cli/tests/common/`.
7. `cargo-deny` is not installed locally; CI's deny job is the only check. Every crate must be
   `publish = false` (path dependencies are otherwise "wildcards").
8. Performance assertions in debug-build tests flake on Windows runners (antivirus scans new
   files); keep them as sanity bounds and leave real budgets to WP10.1's release benchmarks.
9. On Windows, cargo puts the target directory on `PATH` for test processes, so the freshly built
   `acs.exe` is "installed" in every test. Tests that need `acs` absent must filter `PATH`
   (see `path_without_acs` in `crates/acs-git/tests/hooks.rs`). Reproduce on Linux with
   `PATH=$PWD/target/debug:$PATH cargo test`.

## 3. Must-know items for the next phases

**Any code that starts a detached process** — `Stdio::null()` is not enough on Windows, where a
child inherits every inheritable handle, the hook's stdout pipe included. Call
`acs_os::keep_stdio_from_children` first, as `runtime::sync` does. Only a test run on Windows shows
the difference (decision 0020).

**WP4.1 (records)** — write records with `SyncStorage::append_ledger(pilot, session_id, bytes)` on
a `refs::CustomRefs`; the next sync reconciles and pushes them. It is safe from concurrent hooks and
never fails on signing (`LedgerAppend.signed` says what happened). `ledger_entries` reads a chain;
reading every pilot's ledger for `acs log` needs a method listing fetched pilots, which belongs on
the trait. `stale_live_seats(before)` finds crashed sessions (human seats excluded); aggregate
`files_changed` as the first `pre_hash` and last `post_hash` per path from `attributions`. A missing
pre-image entry means "unknown", not "did not exist". Redact event `detail` again before it goes
into a record. Intent text needs the redaction pass (plan §9.2).

**WP4.4 (signing)** — `ledger.rs` signs with `commit-tree -S` when `commit.gpgsign` is true and
falls back to unsigned. `signing.rs` still owns detecting and verifying; join commits (empty tree,
two parents) are part of a valid chain and carry no record.

**WP4.2 (blame)** — `acs_git::trailers::session_ids(message, comment_char)`; uncommitted lines come
from `path_attributions`.

**WP4.3 (undo)** — unpredicted shell changes record the *staged* content as pre-image (committed
content above 20k entries); wrong only if the file already had unstaged edits. Snapshots of ended
sessions are kept (no `drop_session` at session end) and evicted by budget only.

**WP5.1 (skills)** — sync the interim plugin skills; MCP tool names are
`mcp__plugin_acs_acs__<tool>` in Claude Code; in Antigravity the plugin's server is renamed
`acs_acs` and tools are reached through `call_mcp_tool` (or directly, via `alwaysLoad`).
Make the "deletes need a claim first" rule obvious (default `max_files_deleted_outside_claims = 0`).
For now, build and verify for Claude Code only and leave the Antigravity skill copy as it is
(section 6); it gets synced when Antigravity returns.

**WP5.2 (distribution)** — `acs plugin install` replaces the printed commands in
`commands::init::harness::HARNESSES` and doctor's `plugin_*` hints. Antigravity **fails closed**
(every tool call denied) when the hook binary is missing; installers must put `acs` on PATH before
the plugin is enabled. Claude Code fails open; an optional strict mode (`onFailure: block`) is
recorded in 0013. For now, `acs plugin install` targets Claude Code only (section 6), and the
PATH-before-plugin rule still applies when Antigravity returns.

**WP7.2 (hardening)** — known bypasses: an agent starting a nested harness with hooks disabled
(`claude --bare -p`, `claude --settings '{"disableAllHooks":true}'`); writes to harness config
outside the repo (`~/.claude/settings.json`, `~/.gemini/config/hooks.json`), which repository globs
cannot seal (file tools get `path_escapes_repo` ask; shell writes are uncovered); agents running
`acs off`. Classify agent CLIs as unknown-target commands.

**Known gaps and sharp edges**

- Antigravity: no session start/end or prompt events (`PreInvocation` stands in, its first-call
  rule is a guess); ACS `allow` renders as Antigravity `ask` ("no objection") — verify it doesn't
  force prompts; subagent conversations are separate seats and can conflict with their parent.
- Claude Code: `CLAUDE_CODE_SESSION_ID` is stale after `--continue` and `/clear`; MCP falls back
  to the most recent live seat in the worktree and warns on ambiguity. Persisting
  `tool_use_id → seat` from pre-tool events (the `_meta["claudecode/toolUseId"]` is already
  extracted) would remove the ambiguity.
- Git backstop: a worktree whose checkout has no `.acs/policy/` (for example before the adoption
  commit) gets no backstop, even when ACS state exists for the repo — consider keying on the state
  instead. A stale commit marker (agent commit rejected by a later user hook) can make a human
  commit within 10 minutes look like an agent's.
- Human claims (`acs claim`) are never heartbeated; they expire after `claim_ttl_secs` until WP7.3
  or WP7.4 refreshes them.
- Team sync: claims taken on two machines inside one sync window leave both seats denied until one
  lets go (section 4, decision 0020). `acs doctor` does not check sync yet.
- `acs-cli` could not be type-checked for Windows locally (C dependencies need mingw); Windows CI
  is the check.

## 4. Candidate contract changes (decide before the WP that needs them)

| Change | Needed by | Source |
| --- | --- | --- |
| `SimpleCommand.wrappers` so bans can match `sudo *`; agent-CLI classification | WP7.2 | WP1.4, WP2.2 |
| Optional `[lore]` config keys (inject cap 4,000 chars, stale threshold 30%) | WP6.1, WP6.2 | plan §7 |
| Tie-break for claims taken on two machines inside one sync window (today both are denied until one lets go): the earlier `acquired_at` keeps it | before a team relies on M1 | WP3.1, decision 0020 |
| `FileChange` cannot say "pre-hash unknown" vs "did not exist" | WP4.1 | WP2.1 |
| Expose the engine's claim-message helpers (`age`, holder) from `acs-core` | any | WP2.4 duplicated them |
| `ResolvedPath.linked_truncated` | optional | WP1.3 |

## 5. For the human

- **Deferred, see section 6 for when:** allow `antigravity.google` in the cloud environment's
  network settings (Allowed domains) so a session can verify the adapter against the official
  docs; run the Antigravity manual checklist (`docs/adapters/antigravity.md`) on a machine with
  Antigravity installed and record results in `docs/releases/`.
- Still open from Phase 1: branch protection on `main`; CODEOWNERS for `.acs/policy/` and `spec/`;
  the `acs` name check (crates.io, npm, Homebrew); the home for the setup skill; governance. The
  release workflow has never run. The GitHub default branch is already `main` (checked
  2026-10-31).
- The marketplace command printed by `acs init` assumes the repository is `acs-hq/acs`
  (`claude plugin marketplace add acs-hq/acs`).

## 6. Deferred: Antigravity and GitLab

The human deferred both on 2026-10-31 to reduce complexity. This is a deferral, not a removal. The
implementation plan is being updated to match; until a decision record exists (now 0019), this section
is the reference.

**Keep in place:** the Antigravity adapter, its fixtures and tests, `integrations/antigravity/`, its
default sealed paths (`.agents/**`, removing them would let an agent edit those configs) and the
`Adapter` trait. Do not write Claude-specific logic outside `claude_code.rs`; in particular, do not
assume every harness sends session start, session end or prompt events.

**Changed until Antigravity returns:**

- M1 gate: three Claude Code sessions with overlapping tasks produce zero silent overwrites; every
  collision is visible in `acs status`; sealed paths hold; one-sentence setup works in Claude Code.
- WP5.1 no longer depends on WP2.3, and its acceptance covers Claude Code only.
- WP5.2's `acs plugin install` covers Claude Code only.
- The WP2.3 and WP2.6 acceptance runs in Antigravity stay open.
- The pre-release manual run (plan section 9.1) uses Claude Code only.

**Changed until GitLab returns:**

- WP3.1 verifies `refs/acs/*` pushes on GitHub only.
- WP5.3 parses GitHub CODEOWNERS only.
- WP9.2 ships the GitHub Action only.

**Remind the human at these points, and do not continue past a hard stop without their answer:**

| When | What to tell the human | Type |
| --- | --- | --- |
| WP3.1 starts (raised 2026-10-31) | A one-off push of `refs/acs/*` to GitLab is optional now. If GitLab rejects it later, the fallback is a dedicated branch instead of custom refs. | Reminder |
| WP5.3 starts | GitLab CODEOWNERS parsing is skipped; it returns with WP9.2. | Reminder |
| Any public launch | ACS has only been proven across Claude Code sessions, not across harnesses. | Reminder |
| WP8.3 merges, before WP9.1 starts | Antigravity must come back now: allow `antigravity.google`, run the manual checklist, sync the skill copy, add `acs plugin install antigravity`, run the three-harness M1 gate, finish the WP2.3 and WP2.6 acceptance. | **Hard stop** |
| WP9.2 starts | GitLab must come back now: the CI template, the GitLab CODEOWNERS format and the push check. | **Hard stop** |

Antigravity must be verified before WP9.1 because its known gaps (`allow` rendered as `ask`, no
session events) may need contract changes, which are cheap in draft 0.1 and costly after Spec 1.0.
