# Orchestrator handoff (after WP5.1 and WP5.2)

Written 2026-10-30 at the end of the session that built Phase 2, and updated 2026-10-31 after
Phase 3 (WP3.1) and again after WP5.2 and WP5.1. It replaces the handoff written after Phase 1;
that version is in git history (`git show 49c2cf0:docs/HANDOFF.md`) and its process lessons still
apply (summarized in section 2). Read this with the implementation plan and `docs/decisions/`
before starting the next phase.

**Antigravity and GitLab are deferred (2026-10-31).** Section 6 says what changed and when the
human must be reminded. Check its triggers before starting any WP.

## 1. Where things stand

- `main` has Phase 0, Phase 1, **Phase 2 (WP2.1–WP2.6)**, **Phase 3 (WP3.1)**, including the rule
  that the first of two machines' claims keeps the file, and **WP5.2 and WP5.1 for Claude Code**
  (decisions 0021, 0022). Fmt, clippy `-D warnings` and the workspace tests pass locally on Windows
  (544 passed, 3 ignored). The human reported CI green at `a0baf6f` (2026-10-31); WP5.1 and WP5.2
  are not CI-checked yet, because the human pushes. `.github/workflows/installers.yml` and
  `scoop-manifest.yml` have had no YAML parser run over them, so GitHub is their first real check.
- **Every M1 work package is built, but M1 is not released.** Between here and the v0.1 tag
  (plan sections 8.2 and 8.3):
  1. The M1 parts of WP10.1 and WP10.2 were left out of the WP5 session on purpose. There is no CI
     perf job, and the budgets were last measured on Linux in Phase 2.
  2. The M1 gate with real sessions: three Claude Code sessions with overlapping tasks. Only one
     real session at a time has been run, and only scripted seats overlap
     (`docs/releases/v0.1.0.md`).
  3. The release setup only the human can do (`docs/releases/README.md`): name check, public
     repository, Homebrew tap, npm scope and tokens. Every installer needs a public repository.
  4. The first tag, which is the first real run of `release.yml` and `installers.yml`.
- **Then dogfooding.** The human runs `acs init` here (see section 2). No release exists yet, so
  `acs` comes from `cargo install --path crates/acs-cli --locked`.
- The implementation plan is not in the repository. Its current copy is
  `C:\Users\omara\Downloads\ACS — Implementation Plan Updated.md`; `docs/phase2-rules.md` still
  names an old path.
- **Frozen contracts:** `spec/SPEC.md` (draft 0.1), `spec/schemas/` (drift test), `crates/acs-core`
  public types and `gate.rs`, and the Phase 2 cross-WP interfaces in decision **0011** (crate roles,
  CLI surface, markers, MCP seat binding). Only the orchestrator changes them.
- Phase 2 decision records: **0011** architecture (orchestrator), **0012** hook pipeline, **0013**
  Claude Code adapter, **0014** Antigravity adapter, **0015** git backstop, **0016** MCP server,
  **0017** harness paths in the default sealed list (orchestrator, with an amendment for
  `.husky/**`), **0018** CLI. Phase 3: **0020** team sync, including the GitHub push check and the
  `acs-os` exception to `unsafe_code = forbid`. Phase 5: **0021** distribution and `acs plugin
  install`, **0022** skills.
- Shared rules handed to every Phase 2 subagent: `docs/phase2-rules.md` (reuse it for Phase 3+,
  updating the reading list and decision numbers).

| Crate | WP | Entry points |
| --- | --- | --- |
| acs-adapters | 2.1–2.3 | `Adapter` trait, `by_name`, `canonical`, `claude_code::ClaudeCode`, `antigravity::Antigravity` |
| acs-git | 2.1, 2.4, 3.1 | `repo::discover`, `status::scan_changes`, `hooks::{install, uninstall, status}`, `trailers`, `refs::{SyncStorage, CustomRefs}`, `sync::run` |
| acs-store | 2.1 adds | migration 0003: `SeatInfo` (session id, intent), last decision, flags (`gate_off`), attributions, `stale_live_seats` |
| acs-mcp | 2.6 | `Backend` trait, `serve_stdio` (rmcp 3.8, the only async runtime in the binary) |
| acs-cli | 2.1, 2.4–2.6, 3.1, 5.2 | `runtime` (`RepoContext`, seats, `status`/`claim_paths`/`release_paths`/`set_intent`/`last_decision`, `sync::{lock, spawn_if_due, spawn_now}`), `hook`, `cli`, `commands/{init,status,claim,release,off,doctor,git_hook,mcp,sync,plugin}` |
| acs-os | 3.1 | `keep_stdio_from_children`; the only crate allowed `unsafe`, one Windows call (decision 0020) |

Commands: `acs hook --canonical <phase>`, `acs hook <claude-code|antigravity|canonical>`,
`acs mcp [--harness]`, `acs git-hook <name>`, `acs init|status|claim|release|off|on|doctor|sync`
and `acs plugin install <harness>` (each with `--json`).

Integrations: `integrations/claude-code/` (plugin `acs`, marketplace `acs-plugins` in the root
`.claude-plugin/marketplace.json`), `integrations/antigravity/` (plugin `acs`, always-on rule) and
`integrations/scoop/` (the manifest template). The canonical skills are in `skills/`. The Claude
Code plugin holds exact copies (`crates/acs-cli/tests/packaging.rs` fails otherwise); Antigravity
keeps its **interim** `acs-usage` copy until the gate.

### Acceptance evidence worth knowing

- **Real Claude Code run** (`claude -p` 2.1.314 with the plugin via `--plugin-dir`): an edit of a
  file claimed by another seat was denied and the model quoted the guidance naming the holder
  (`docs/adapters/claude-code.md` §17). Separately, `claude -p` with `acs mcp` listed and called
  all five tools (decision 0016). Hooks and MCP then ran together on Windows (`docs/adapters/claude-code.md` §18).
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

New in Phase 3:

1. **The human's standing rules.** Subagents, if used, run on Sonnet. Finish work on a branch and
   merge it into `main` as several commits, not one; the human does the pushing. `acs init` may run
   in temp and test folders, never in the root of this repository: when that is needed, tell the
   human. Do not start the next WP until asked.
2. **Development now runs on the human's Windows machine, under Avast.** If the antivirus blocks
   anything, stop and tell the human; do not look for a way around it. Stay in the repository's own
   `target/`: a fresh `CARGO_TARGET_DIR` under `AppData\Local\Temp` got `build-script-build.exe`
   blocked and `query-cache.bin` quarantined (`BV:Downloader-BTG`), while builds in `target/` were
   left alone. That also rules out building in a worktree with its own target directory, so the
   subagent workflow above needs thought before it is reused here. No Avast exception is set.
3. **CI is out of reach from a session here.** `gh` is not installed and the repository is private,
   so the public API answers 404. Ask the human for the result.
4. **Stopping a background script does not stop what it started on Windows.** Stopping the task left
   its bash loop running, and the loop moved on to the next build. Check with `Get-CimInstance
   Win32_Process`, then stop the script before its `cargo`.
5. **Sync tests.** The remote view goes stale after three intervals, so an interval of 1 second gave
   a 3 second window that a busy machine overran; `crates/acs-cli/tests/sync.rs` uses 5. A test that
   polls with an edit claims the file the moment the edit is allowed, so poll `acs status` and try
   the edit once. Hooks now start a real sync where a repository has a remote and `.acs/policy/`, so
   a test with an unreachable remote should not send session start or post-tool events.

New in WP5:

1. **Run the real thing, and test what the docs claim.** `dist plan` found that the release could
   not have worked at all, and a run with `acs.cmd` showed the adapter doc's "`.cmd` shims do not
   work" was wrong, which saved a guard against npm on Windows that was not needed. A green test
   suite proves neither.
2. **Real Claude Code from a session here, safely.** `claude plugin ...` needs no login, so run it
   with `CLAUDE_CONFIG_DIR` in the scratchpad. A real `claude -p` needs the human's login and so
   their real config: load the plugin with `--plugin-dir`, which changes no settings, and do not
   leave a `.claude/settings.json` naming the plugin in the repository it runs in. To let an agent
   run `acs plugin install` in such a session, put a `claude.cmd` shim first on `PATH` that sets
   `CLAUDE_CONFIG_DIR`. Start the real `claude` by its absolute path.
3. **`--allowedTools` swallows what follows it,** so put the prompt before it. `claude -p` warns
   about stdin unless it gets `< /dev/null`. Pass `--session-id` in nested runs (adapter doc section
   15).
4. **A hook that cannot start fails open with no message to the agent:** an edit of a claimed file
   went through. Claude Code then caches the MCP failure for 15 minutes, so a bad run can look like
   an MCP bug in the next one.
5. **`dist` 0.33.1 is at `target/tools/dist-0.33.1/` in this checkout,** with its checksum verified.
   `target/` is not committed, so a fresh clone needs it again (the human approved the download).

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

**The Antigravity gate (skills and plugin install)** — sync the skill copy from `skills/`; in
Antigravity the plugin's server is renamed `acs_acs` and tools are reached through `call_mcp_tool`
(or directly, via `alwaysLoad`). Add a row to the setup skill's harness table and an arm to
`plugin::install`. Antigravity **fails closed** (every tool call denied) when the hook binary is
missing, so the `acs`-on-`PATH` check there matters more than it does for Claude Code, which fails
open; an optional strict mode (`onFailure: block`) is recorded in 0013.

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
- Team sync: `acs doctor` does not check sync yet. Claims taken on two machines inside one sync
  window are settled by the first claim keeping the file (SPEC section 6, decision 0020), but edits
  made before either machine has fetched are only caught at commit time.

## 4. Candidate contract changes (decide before the WP that needs them)

| Change | Needed by | Source |
| --- | --- | --- |
| `SimpleCommand.wrappers` so bans can match `sudo *`; agent-CLI classification | WP7.2 | WP1.4, WP2.2 |
| Optional `[lore]` config keys (inject cap 4,000 chars, stale threshold 30%) | WP6.1, WP6.2 | plan §7 |
| `FileChange` cannot say "pre-hash unknown" vs "did not exist" | WP4.1 | WP2.1 |
| Expose the engine's claim-message helpers (`age`, holder) from `acs-core` | any | WP2.4 duplicated them |
| `ResolvedPath.linked_truncated` | optional | WP1.3 |

## 5. For the human

- **Deferred, see section 6 for when:** allow `antigravity.google` in the cloud environment's
  network settings (Allowed domains) so a session can verify the adapter against the official
  docs; run the Antigravity manual checklist (`docs/adapters/antigravity.md`) on a machine with
  Antigravity installed and record results in `docs/releases/`.
- After M1: run `acs init` in this repository to start dogfooding. Agents then cannot edit
  `.acs/**`, `.github/workflows/**` or the Claude settings files, so those changes fall to the human.
- Release setup: `docs/releases/README.md` (name check, public repository, Homebrew tap, npm scope
  and tokens). The first tag is the first real run of `release.yml` and `installers.yml`.
- Still open from Phase 1: branch protection on `main`; CODEOWNERS for `.acs/policy/` and `spec/`;
  the home for the setup skill (the README uses the raw GitHub URL, which needs a public
  repository); governance. The GitHub default branch is already `main` (checked 2026-10-31).
- `acs plugin install` assumes the repository is `acs-hq/acs`; `--marketplace` overrides it.

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
