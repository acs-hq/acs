# Orchestrator handoff (after WP7.4)

Written 2026-10-30 at the end of the session that built Phase 2, and updated 2026-10-31 after
Phase 3 (WP3.1), after WP5.2 and WP5.1, and after the M1 parts of WP10. It replaces the handoff
written after Phase 1; that version is in git history (`git show 49c2cf0:docs/HANDOFF.md`) and
its process lessons still apply (summarized in section 2). Read this with the implementation plan
and `docs/decisions/` before starting the next phase.

**Antigravity and GitLab are deferred (2026-10-31).** Section 6 says what changed and when the
human must be reminded. Check its triggers before starting any WP.

**The human brought the Antigravity gate forward (2026-11-03).** Its CLI part is done on the branch
`antigravity-gate`, **not merged into `main`** (decision 0024, section 7 below). What is left of the
gate is the human's: the desktop app checks and three decisions.

**WP5.3 (`acs init` proposals) is built (2026-11-07, decision 0025).** The human dropped CODEOWNERS
from the plan as future work: proposals come from git history and folder names, and the setup
skill has the pilot review them. Section 6's GitLab items change to match.

**WP7.1 and WP7.2 are built (2026-11-09, decision 0026), and dogfooding has started (the human ran
`acs init` here on 2026-11-08).** Owned zones are enforced, `acs override` lets a person in, and
agents can no longer switch ACS off by the routes it can see. `docs/security/overrides.md` lists what
is not covered and the bugs dogfooding has turned up. Overrides are stored and shown in `acs status`;
putting them in session records and `acs log` is WP4.1's job (section 3). Since decision 0027,
writes outside the repository no longer ask by default (`protect_outside_repo` opts in).

**WP7.3 and WP7.4 are built (2026-11-09, decision 0028).** `integrations/vscode/` is the VS Code
extension: it claims the files a person edits and shows the files others hold. `acs watch` does the
claiming for any other editor. Both release a claim once the file is saved and idle for two minutes.
The extension is **not published**: the human tests it locally first (`docs/releases/wp7-humans.md`)
and owns the Marketplace and Open VSX accounts. CI does not build it yet (section 5).

## 1. Where things stand

- `main` has Phase 0, Phase 1, **Phase 2 (WP2.1–WP2.6)**, **Phase 3 (WP3.1)**, including the rule
  that the first of two machines' claims keeps the file, and **WP5.2 and WP5.1 for Claude Code**
  (decisions 0021, 0022). Fmt, clippy `-D warnings` and the workspace tests pass locally on Windows
  (560 passed, 1 ignored). CI's first run with WP10 (`92dd3a3`, 2026-11-01) failed on the new
  tests and on the perf comparison, not on ACS; the fixes passed CI (`2176a14`). **CI is green on
  `main` at `d832a71` (2026-11-06, run 42307284800): every job, macOS included,** with the
  Antigravity gate merged. Its own merge run had failed only because the macOS jobs got no runner.
  WP5.3 (`8aeeec5`, 2026-11-07) has its own run; its `acs-cli` tests passed locally on Windows.
  `.github/workflows/installers.yml` and `scoop-manifest.yml` have had no YAML parser run over them,
  so GitHub is their first real check.
- **Every M1 work package is built, the M1 parts of WP10.1 and WP10.2 are on `main` (decision 0023),
  and the real M1 gate passed twice (`docs/releases/v0.1.0.md`). M1 is not released.** Linux and macOS
  meet every budget in CI. Between here and the v0.1 tag (plan sections 8.2 and 8.3):
  1. **The Windows budgets, the human's call.** Windows misses them on the human's machine (pre-tool
     p50 57 ms against 15) and on GitHub's runner, which has no Avast (72 ms), and process start
     alone is 18 ms. CI gates on Linux and only reports macOS and Windows until this is settled.
     Decision 0023 has the numbers, the stage breakdown and a reverted experiment that saved 14 ms.
     Unmeasured leads: `current_human_seat` starts `git config` on every `acs status`, and post-tool
     has not been split into stages. The plan's fallback is a warm server (section 9.2), which only
     the human can approve.
  2. The release setup only the human can do (`docs/releases/README.md`): name check, public
     repository, npm scope and token. Every installer needs a public repository. Homebrew is built
     but not published until the human wants it (decision 0021).
  3. The first tag, which is the first real run of `release.yml` and `installers.yml`.
- **Then dogfooding.** The human runs `acs init` here (see section 2). No release exists yet, so
  `acs` comes from `cargo install --path crates/acs-cli --locked`.
- **Frozen contracts:** `spec/SPEC.md` (draft 0.1), `spec/schemas/` (drift test), `crates/acs-core`
  public types and `gate.rs`, and the Phase 2 cross-WP interfaces in decision **0011** (crate roles,
  CLI surface, markers, MCP seat binding). Only the orchestrator changes them.
- Phase 2 decision records: **0011** architecture (orchestrator), **0012** hook pipeline, **0013**
  Claude Code adapter, **0014** Antigravity adapter, **0015** git backstop, **0016** MCP server,
  **0017** harness paths in the default sealed list (orchestrator, with an amendment for
  `.husky/**`), **0018** CLI. Phase 3: **0020** team sync, including the GitHub push check and the
  `acs-os` exception to `unsafe_code = forbid`. Phase 5: **0021** distribution and `acs plugin
  install`, **0022** skills, **0025** `acs init` proposals without CODEOWNERS (WP5.3). Phase 7:
  **0026** owned territory, overrides and self-protection (WP7.1, WP7.2); **0027** writes outside
  the repository ask only on request; **0028** human claims from editors (WP7.3, WP7.4).
- Shared rules handed to every Phase 2 subagent: `docs/phase2-rules.md` (reuse it for Phase 3+,
  updating the reading list and decision numbers).

| Crate | WP | Entry points |
| --- | --- | --- |
| acs-adapters | 2.1–2.3 | `Adapter` trait, `by_name`, `canonical`, `claude_code::ClaudeCode`, `antigravity::Antigravity` |
| acs-git | 2.1, 2.4, 3.1 | `repo::discover`, `status::scan_changes`, `hooks::{install, uninstall, status}`, `trailers`, `refs::{SyncStorage, CustomRefs}`, `sync::run` |
| acs-store | 2.1 adds | migration 0003: `SeatInfo` (session id, intent), last decision, flags (`gate_off`), attributions, `stale_live_seats` |
| acs-mcp | 2.6 | `Backend` trait, `serve_stdio` (rmcp 3.8, the only async runtime in the binary) |
| acs-cli | 2.1, 2.4–2.6, 3.1, 5.2, 5.3, 7.1, 7.2, 7.4 | `runtime` (`RepoContext`, seats, `status`/`claim_paths`/`release_paths`/`set_intent`/`last_decision`, `sync::{lock, spawn_if_due, spawn_now}`, `protect::{StoreOverrides, outside_seals}`), `hook` (`markers::tool_markers` lists running calls for `acs watch`), `cli`, `commands/{init,status,claim,release,off,override,doctor,git_hook,mcp,sync,plugin,watch}`, `init::propose::{propose, propose_for, territory_toml}` |
| acs-os | 3.1 | `keep_stdio_from_children`; the only crate allowed `unsafe`, one Windows call (decision 0020) |

Commands: `acs hook --canonical <phase>`, `acs hook <claude-code|antigravity|canonical>`,
`acs mcp [--harness]`, `acs git-hook <name>`, `acs init|status|claim|release|off|on|doctor|sync`
`acs override --path <glob> --once|--session` / `--revoke <id>`, `acs watch [--idle <secs>]` (JSON
Lines with `--json`) and `acs plugin install <harness>` (each with `--json`). `acs init` also takes `--yes`, `--min-share` and `--min-commits` for its
territory proposal.

Integrations: `integrations/claude-code/` (plugin `acs`, marketplace `acs-plugins` in the root
`.claude-plugin/marketplace.json`), `integrations/antigravity/` (plugin `acs`, always-on rule),
`integrations/scoop/` (the manifest template) and `integrations/vscode/` (the editor extension,
TypeScript: `npm test`, `npm run package`). The canonical skills are in `skills/`. The Claude
Code plugin holds exact copies (`crates/acs-cli/tests/packaging.rs` fails otherwise); Antigravity
keeps its **interim** `acs-usage` copy until the gate.

### Acceptance evidence worth knowing

- **Real Claude Code run** (`claude -p` 2.1.314 with the plugin via `--plugin-dir`): an edit of a
  file claimed by another seat was denied and the model quoted the guidance naming the holder
  (`docs/adapters/claude-code.md` §17). Separately, `claude -p` with `acs mcp` listed and called
  all five tools (decision 0016). Hooks and MCP then ran together on Windows (`docs/adapters/claude-code.md` §18).
- **Antigravity, real CLI runs (branch `antigravity-gate`):** `agy -p` 1.4.12 on Windows ran the
  CLI part of the 11-step checklist; `docs/adapters/antigravity.md` section 15 has every answer and
  decision 0024 the changes. The desktop app was not run (the human's decision); see section 7.
- **Three real sessions at once:** `tests/e2e/real/three_sessions.sh`, run twice. Zero silent
  overwrites and every collision in `acs status`, judged from Claude Code's own logs. Agents with the
  ACS tools plan around each other, so `ACS_REAL_NO_MCP=1` is what makes the hooks do the blocking.
- **Budgets:** `cargo bench -p acs-cli --bench hook_paths`. In Phase 2 (release, Linux, 4 shared
  vCPUs, the old test): pre-tool edit p50 8.7 ms / p99 18.4 ms; post-tool shell at 100k files
  p50 16 ms / p99 47 ms **only with the scoped scan**. A full `git status` scan of 100k files
  missed (p50 147 ms), so above 20,000 index entries the scan is scoped to predicted targets,
  claims and nearby directories (decision 0012) and opaque scripts writing elsewhere go unseen.
  Windows misses every budget (section 1).

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
3. ~~CI is out of reach from a session here.~~ Since 2026-11-09 `gh` is installed and logged in
   (account `omardardir`): `gh issue`, `gh run list` and `gh run view` work against `acs-hq/acs`. The
   human still pushes, and judges CI unless they ask otherwise.
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

New in WP10:

1. **A benchmark's first full run takes about 20 minutes here,** nearly all of it `git add` of 100k
   files in a fresh repository. Keep other work off the machine while it measures, and use
   `ACS_PERF_FILES=2000` to try a change.
2. **The human does not want large side problems chased inside a work package.** Measure enough to
   describe it, then hand over a brief.
3. **`Path::join("a/b")` breaks `mklink` on Windows:** `cmd` reads `/b` as a switch. Join one part at
   a time in test fixtures.
4. **A real run can pass without testing anything.** Run 1 blocked no `Edit` at all. Read the calls
   in the logs, not just the verdict.

New in WP7 (the first work done under dogfooding):

1. **The session that builds a rule is not gated by it.** The hooks run the installed `acs`, so new
   engine rules reach agents here only after the human reinstalls
   (`cargo install --path crates/acs-cli --locked`). A reinstall that migrates the state schema makes
   older builds report the state unavailable.
2. **ACS friction is a bug report.** Report it to the human at once and add it to the dogfooding table
   in `docs/security/overrides.md`. Two came up in WP7: the `cd dir && a || b` directory bug in
   `acs-shell`, and prompts for every scratch write outside the repository (decision 0027).
3. **Edits made through `python` in the shell are invisible to ACS** (no claim, no conflict check).
   That is a known limit, not a way to work: prefer the Edit and Write tools for repository files, so
   other seats are protected from your edits.
4. **Multi-spot edits: write the patch script with the Write tool into the scratchpad, then run it.**
   Inline `python - <<'EOF'` in the Bash tool mangled `\` and line continuations in Rust strings
   twice, and a long heredoc failed with "unexpected EOF". Check `git diff` after a scripted edit.
5. **`cargo test --workspace` stops at the first failing test binary;** use `--no-fail-fast` for a full
   count. A skill or `integrations/` change made while tests build leaves a stale embedded copy, and
   `cli_plugin` then fails until the next build.
6. **Small traps that cost a run each:** the shell corpus's `ACS_SHELL_BLESS=1` only fills cases with
   no `>` lines, so delete a changed case's expectations before blessing it; doc comments on
   `acs-core` types feed the JSON Schemas, so a doc change needs `ACS_UPDATE_SCHEMAS=1 cargo test -p
   acs-core --test schemas`; `crates/acs-store/tests/state_store.rs` keeps its own `MIGRATIONS` list,
   so a new migration goes there too; `git merge` cannot read its message from stdin (`-F -`), so use
   `-m`; never put an interactive command (`git checkout -p`) in a tool call.
7. **`target/release/acs.exe` can be stale.** `tests/e2e/real/owned_territory.sh` picks the newer of
   the release and debug builds; `three_sessions.sh` still takes release only.

New in WP7.3 and WP7.4:

1. **A timing rule needs a test that fails without it.** The watcher's hold-back for running agent
   commands passed its test; switching the rule off made the test fail, which is what showed the
   test meant something. Do that check for every "wait for X" rule.
2. **`node --test out/test/` fails on Node 24** (it treats the directory as a module); pass a quoted
   glob, `node --test "out/test/*.test.js"`.
3. **`vsce package` asks a question when there is no `LICENSE`,** which hangs a tool call; the
   extension ships `LICENSE` and `LICENSE-APACHE`.
4. **A `cd` in the Bash tool moves the session's working directory** when it succeeds inside the
   project; use absolute paths, or `cd` back in the same command.

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
into a record. Intent text needs the redaction pass (plan §9.2). Fill `SessionRecord.overrides`
from `StateDb::override_uses_for_seat`, and make `acs log` show them: that is the part of WP7.1's
acceptance ("every override appears in `acs log` and in the session record") left to WP4.1
(decision 0026). Add read-only subcommands (`log`, `blame`, `stats`, `verify`) to `READ_ONLY` in
`crates/acs-core/src/engine/protect.rs`, or agents will be denied them.

**WP4.4 (signing)** — `ledger.rs` signs with `commit-tree -S` when `commit.gpgsign` is true and
falls back to unsigned. `signing.rs` still owns detecting and verifying; join commits (empty tree,
two parents) are part of a valid chain and carry no record.

**WP4.2 (blame)** — `acs_git::trailers::session_ids(message, comment_char)`; uncommitted lines come
from `path_attributions`.

**WP4.3 (undo)** — unpredicted shell changes record the *staged* content as pre-image (committed
content above 20k entries); wrong only if the file already had unstaged edits. Snapshots of ended
sessions are kept (no `drop_session` at session end) and evicted by budget only.

**Antigravity** — done on `antigravity-gate`: skills synced (both plugins ship exact copies),
`acs plugin install antigravity` (writes the built-in plugin to `.agents/plugins/acs/`), a setup
skill row. In hooks, MCP tools are `mcp_acs_acs_acs_<tool>`. Antigravity **fails closed** when the
hook binary is missing (confirmed), so the `acs`-on-`PATH` check matters more there than for Claude
Code, which fails open; an optional strict mode (`onFailure: block`) is recorded in 0013. A change
to `integrations/antigravity/` needs a rebuild to reach `acs plugin install antigravity`.

**WP7.1 and WP7.2 (done, decision 0026)** — every new bypass goes into
`crates/acs-cli/tests/fixtures/adversarial.txt` and, if it cannot be closed, into the residual risks
of `docs/security/overrides.md`. A new harness adds its user-level config to
`runtime::protect::outside_seals` and its hook-disabling flags to `protect::harness` in
`acs-core`. `tests/e2e/real/owned_territory.sh` is the real-agent check (`docs/releases/wp7-territory.md`).

**WP7.3 and WP7.4 (done, decision 0028)** — `acs watch` decides "agent or person" from agents'
live claims, post-tool attributions and running tool markers (`pending/tool/`): a change to any of
those (WP4.1 pruning attributions, WP8.3 queueing edits) must keep a running agent's changes
attributable, or the watcher will claim them for the person; `crates/acs-cli/tests/watch.rs`
catches it. The extension only uses `acs status|claim|release --json`, so a change to those JSON
shapes breaks it: its `test/cli.test.ts` runs against `target/debug/acs` (or `ACS_BIN`) and is
skipped when no binary exists. SPEC section 6.2 states the rule for editor integrations.

**Known gaps and sharp edges**

- Antigravity: no session end or prompt event; `SessionStart` fires in the CLI but ACS relies on
  the first `PreInvocation`, whose rule now rests on real counters. ACS `allow` renders as
  `allow` (decision 0024). In `always-proceed` mode `force_ask` is approved without a prompt.
  Subagents are separate seats and **do** conflict with their parent (seen in a real run).
- Claude Code: `CLAUDE_CODE_SESSION_ID` is stale after `--continue` and `/clear`; MCP falls back
  to the most recent live seat in the worktree and warns on ambiguity. Persisting
  `tool_use_id → seat` from pre-tool events (the `_meta["claudecode/toolUseId"]` is already
  extracted) would remove the ambiguity.
- Git backstop: a worktree whose checkout has no `.acs/policy/` (for example before the adoption
  commit) gets no backstop, even when ACS state exists for the repo — consider keying on the state
  instead. A stale commit marker (agent commit rejected by a later user hook) can make a human
  commit within 10 minutes look like an agent's.
- `acs_release` without `paths` is an error; two real sessions tried it first. A blocked session can
  leave uncommitted work in a shared work tree (`docs/releases/v0.1.0.md`).
- Claims made with `acs claim` are never heartbeated; they expire after `claim_ttl_secs`. The VS
  Code extension refreshes its own; `acs watch` caps `--idle` at the TTL instead.
- Team sync: `acs doctor` does not check sync yet. Claims taken on two machines inside one sync
  window are settled by the first claim keeping the file (SPEC section 6, decision 0020), but edits
  made before either machine has fetched are only caught at commit time.

## 4. Candidate contract changes (decide before the WP that needs them)

| Change | Needed by | Source |
| --- | --- | --- |
| `SimpleCommand.wrappers` so user bans can match `sudo *` (self-protection did not need it: `words` already drop wrappers, decision 0026) | before 1.0 | WP1.4, WP2.2 |
| Optional `[lore]` config keys (inject cap 4,000 chars, stale threshold 30%) | WP6.1, WP6.2 | plan §7 |
| `FileChange` cannot say "pre-hash unknown" vs "did not exist" | WP4.1 | WP2.1 |
| Expose the engine's claim-message helpers (`age`, holder) from `acs-core` | any | WP2.4 duplicated them |
| `ResolvedPath.linked_truncated` | optional | WP1.3 |

## 5. For the human

- **Antigravity (section 7):** the desktop app checks, the decisions of 0024, and whether to
  merge `antigravity-gate`.
- **Reminder: a missing `acs` breaks the two harnesses in opposite ways.** Both plugins' hooks run
  `acs`, found on the `PATH` the harness started with. Once a repository has adopted ACS and pushed
  its plugin files (`.agents/plugins/acs/`, `.claude/settings.json`), anyone who opens it without
  `acs` on `PATH` gets:
  - **Antigravity:** every tool call in that repository denied ("'acs' is not recognized ...") until
    they install `acs` and restart `agy` from a terminal that has it. Agy works normally elsewhere.
  - **Claude Code:** nothing blocked. Hooks fail open, so every tool call runs and ACS checks
    nothing (no claims, no sealed paths), with no message to the agent.

  Tell teammates to install `acs` before opening an adopted repository; `acs doctor` checks it. A
  strict mode that makes Claude Code fail closed too is noted in decision 0013, not enabled. The cloud environment's network setting for `antigravity.google` only
  matters if a cloud session ever needs the docs; this machine reads them.
- Dogfooding (started 2026-11-08): agents cannot edit `.acs/**`, `.github/workflows/**` or the
  Claude settings files, so those changes fall to the human. After WP7 merges: reinstall `acs`
  (`cargo install --path crates/acs-cli --locked`). The new build migrates the state to schema 5, and
  older builds then report it unavailable. This repository's `.acs/policy/territory.toml` still has
  the comment "ACS does not enforce `owned` zones yet"; it is sealed, so the human must edit it.
  Agents here can no longer run `acs` against scratch repositories from their shell (decision 0026).
- **The VS Code extension (WP7.3):** test it locally with `docs/releases/wp7-humans.md` (install
  the `.vsix` from `npm run package`, or F5 in `integrations/vscode`). Before publishing: a
  Marketplace publisher and an Open VSX namespace with tokens (`publisher` in `package.json` is the
  placeholder `acs-hq`), an icon, and a CI job running `npm ci && npm test` in `integrations/vscode`
  after building `acs` (`.github/workflows/` is sealed for agents). `acs watch` needs the reinstall
  (`cargo install --path crates/acs-cli --locked`).
- Release setup: `docs/releases/README.md` (name check, public repository, npm scope
  and token). The first tag is the first real run of `release.yml` and `installers.yml`.
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
- ~~WP5.3 parses GitHub CODEOWNERS only.~~ Superseded by decision 0025: WP5.3 parses no
  CODEOWNERS at all. CODEOWNERS is future work, for GitHub and GitLab alike.
- WP9.2 ships the GitHub Action only.

**Remind the human at these points, and do not continue past a hard stop without their answer:**

| When | What to tell the human | Type |
| --- | --- | --- |
| WP3.1 starts (raised 2026-10-31) | A one-off push of `refs/acs/*` to GitLab is optional now. If GitLab rejects it later, the fallback is a dedicated branch instead of custom refs. | Reminder |
| ~~WP5.3 starts~~ | Done; no longer needed: decision 0025 dropped CODEOWNERS parsing. | — |
| Any public launch | ACS has only been proven across Claude Code sessions, not across harnesses. | Reminder |
| WP8.3 merges, before WP9.1 starts | Antigravity must come back now. **Brought forward on 2026-11-03; the CLI part is done on `antigravity-gate` (section 7).** At this point, check that section 7's human items are settled and the branch is merged before WP9.1. | **Hard stop** |
| WP9.2 starts | GitLab must come back now: the CI template and the push check (GitLab CODEOWNERS is future work, decision 0025). | **Hard stop** |

Antigravity must be verified before WP9.1 because its known gaps (`allow` rendered as `ask`, no
session events) may need contract changes, which are cheap in draft 0.1 and costly after Spec 1.0.

## 7. The Antigravity gate, brought forward (2026-11-03)

Done in one session on the human's Windows machine, on the branch `antigravity-gate` (not merged).
Decision 0024 has the decisions, `docs/adapters/antigravity.md` section 15 the evidence,
`docs/releases/antigravity-gate.md` the record of the runs. Real runs used `agy -p` with
`gemini-3.8-flash-medium` and `claude -p` with Sonnet, in throwaway repositories with a throwaway
`ACS_HOME`; no global agy or Claude Code setting was changed. On the branch, fmt, clippy
`-D warnings` and the workspace tests pass locally on Windows (564 passed, 1 ignored); CI has not
seen it (the human pushes).

| Gate item (plan section 7) | State |
| --- | --- |
| Allow `antigravity.google` | Not needed here: this machine reads the official docs |
| Manual checklist, recorded in `docs/releases/` | Done for the CLI; the desktop app is left for the human (below) |
| Sync the skill copy; `acs plugin install antigravity`; WP2.3 and WP2.6 acceptance; known gaps | Done in the CLI; three gaps need the human's call (below) |
| Original M1 gate: two Claude Code sessions and one Antigravity session | **Passed** with the ACS tools allowed (zero silent overwrites, every collision in `acs status`, 3 of 3 tasks). With them withheld the two conditions held, but a finished Antigravity session's claims outlived it and blocked the third task (`docs/releases/antigravity-gate.md`) |

**For the human, in the desktop app** (not run, by the human's decision). Use a throwaway repository
with `acs init` and `acs plugin install antigravity`, opened in the app:

1. Do the workspace plugin's hooks, MCP server and rule load at all? (`.agents/plugins/acs/`; the
   CLI loads it without a trust prompt.)
2. With the app's normal permission mode: do reads and edits ACS has no objection to run exactly as
   without ACS? This is the `allow` question: a third-party report said the app treats `allow` as
   approval and skips prompts; the official reference says it defers to the default policy. A shell
   command that would normally prompt must still prompt.
3. An edit of a file claimed with `acs claim` is denied, and the agent sees the reason.
4. A write outside the repository (ACS answers `force_ask`) prompts you, with the reason.
5. The first message of a conversation carries the ACS status, and later turns do not repeat it.

**Decisions for the human** (decision 0024, "Open for the human"): `force_ask` under
`always-proceed` (keep, or render ACS `ask` as `deny`); subagents as separate seats (accept, or join
them through the subagent's transcript, which needs the orchestrator's call before Spec 1.0); claims
outliving an Antigravity session (accept, shorter TTL, or a `Stop` hook); and whether to merge
`antigravity-gate`. Agents running the human `acs` CLI is noted for WP7.2.

Process lessons from this session:

1. **Drive `agy` from PowerShell, or set `MSYS_NO_PATHCONV=1`.** Git Bash rewrote the prompt `/hooks`
   into `C:/Program Files/Git/hooks`, and the agent spent a run exploring that folder.
2. **PowerShell 5.1 mangles JSON passed to native programs**: inner quotes in arguments are dropped
   (use `--settings <file>`), and text piped into `acs hook` arrives unreadable. Pipe from bash.
3. **A recorder that writes with `Set-Content -Encoding utf8` adds a byte-order mark**, which made
   replayed payloads unparseable, and ACS answered them with an allow. Strip it before replaying.
4. **This machine's agy runs in `toolPermission: always-proceed`.** Test permission behaviour with a
   session-only `--settings` file (`{"toolPermission":"request-review"}`); headless runs list refused
   calls as `denied_actions`.
5. **`agy -p` output with `--output-format stream-json` leaves out edit arguments**; the
   conversation's `transcript_full.jsonl` under `~/.gemini/antigravity-cli/brain/<id>/` has them.
