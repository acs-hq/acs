# Decision records

Short records of decisions that are not obvious from the code, named `NNNN-title.md` (zero-padded, in order). Each has three sections:

- **Context**: the situation and constraints.
- **Decision**: what was chosen.
- **Consequences**: what follows from it, including trade-offs and follow-ups.

Keep them brief. Supersede an old record with a new one rather than rewriting history.

Every new record says who decided, in its status line: `Decided by: the human` or `Decided by: Claude`. Add a row to the index below in the same commit.

## Index

"Claude" means a Claude Code session decided while building, without asking. "You" means the human decided. "Open" lists what still waits for the human, with its issue.

| No. | Subject | Decided by | Open |
| --- | --- | --- | --- |
| [0001](0001-workspace-and-toolchain.md) | Workspace and toolchain | Claude | |
| [0002](0002-contracts.md) | Contracts | Claude | |
| [0003](0003-acs-home-and-repo-id.md) | `ACS_HOME` and repository identity | Claude | |
| [0004](0004-gate-interfaces.md) | Gate interfaces for Phase 1 | Claude | |
| [0005](0005-policy-loader.md) | Policy loader | Claude | |
| [0006](0006-state-store.md) | State store | Claude | |
| [0007](0007-path-safety.md) | Path safety | Claude | |
| [0008](0008-shell-analysis.md) | Shell analysis | Claude | |
| [0009](0009-decision-engine.md) | Decision engine | Claude | |
| [0010](0010-snapshot-store.md) | Snapshot store | Claude | |
| [0011](0011-phase2-architecture.md) | Phase 2 architecture | Claude | |
| [0012](0012-hook-pipeline.md) | Hook pipeline | Claude | |
| [0013](0013-claude-code-adapter.md) | Claude Code adapter and plugin | Claude | None (strict mode: settled by 0043) |
| [0014](0014-antigravity-adapter.md) | Antigravity adapter and plugin | Claude | |
| [0015](0015-git-backstop.md) | Git backstop | Claude | |
| [0016](0016-mcp-server.md) | MCP server | Claude | |
| [0017](0017-harness-sealed-defaults.md) | Harness files in the default sealed list | Claude | |
| [0018](0018-cli-m1.md) | CLI for M1 | Claude | |
| [0019](0019-defer-antigravity-and-gitlab.md) | Defer Antigravity and GitLab | You | |
| [0020](0020-team-sync.md) | Team sync | Claude | |
| [0021](0021-distribution.md) | Distribution and `acs plugin install` | Claude; Homebrew not published is yours | Release setup (issue 5) |
| [0022](0022-setup-and-usage-skills.md) | Setup and usage skills | Claude | |
| [0023](0023-m1-hardening.md) | M1 hardening | Claude | Windows latency budgets (issue 2) |
| [0024](0024-antigravity-gate.md) | The Antigravity gate | Claude | The desktop app checks (issue 6); the three questions are settled by 0043 |
| [0025](0025-init-proposals-without-codeowners.md) | `acs init` proposals without CODEOWNERS | You | |
| [0026](0026-territory-overrides-and-self-protection.md) | Owned territory, overrides, self-protection | You, on four questions; the rest Claude | |
| [0027](0027-outside-the-repository-is-opt-in.md) | Writes outside the repository ask only on request | You | |
| [0028](0028-human-claims-from-editors.md) | Human claims from editors | Claude; the release rule and the `notify` crate are yours | Publishing the extension |
| [0029](0029-lore.md) | Lore | Claude | |
| [0030](0030-session-records-undo-verify.md) | Session records, log, blame, undo, verify, stats | Claude | |
| [0031](0031-first-testers-and-first-run-defaults.md) | First testers before the spec; first-run defaults | You | Territory prompt at init, commit trailer toggle (the workflow and `.env` seals: settled by 0042) |
| [0032](0032-calls-are-judged-in-the-work-tree-of-their-files.md) | A call is judged in the work tree its files are in | Claude | Four cases not covered (issue 69) |
| [0033](0033-feature-switches.md) | Feature switches: `[features]` in `config.toml`, `acs features` | You for the idea and the seven switches; Claude for what each covers and what is never off | Set `sync = true` here if this repository should keep syncing |
| [0034](0034-what-agents-are-told-follows-the-switches.md) | What agents are told (MCP tools, instructions, rule, skill) follows the feature switches | You for the rule and for covering every switch; Claude for the details | A real-agent run; hook cost (issue 70) |
| [0035](0035-the-hook-ends-early-when-no-feature-needs-the-call.md) | `acs hook` ends early for a tool call that no switched-on feature needs | Claude, on a task that set which calls; the keep-alive, sync after a read and session end are Claude's | Whether hooks should stop starting at all (issue 70) |
| [0036](0036-no-prompt-submit-hook-until-notices-exist.md) | The Claude Code plugin registers no prompt-submit hook until notices exist | You; leaving Antigravity's hook in place is Claude's (it is that harness's session start) | Register it again when Phase 8 builds the first notice |
| [0037](0037-snapshot-cleanup-runs-in-sync.md) | The copies kept for undo are trimmed to 500 MiB per repository during `acs sync` | Claude (budget, when it runs, crashed seats not counted as live) | Whether the budget should become a setting |
| [0038](0038-help-flags-are-read-only.md) | `acs <subcommand> --help` is allowed to agents when help is all the command does | Claude | None |
| [0039](0039-acs-uninstall.md) | `acs uninstall`: hooks, refspecs, refs and plugin entries always; policy, local state and remote refs on request | Claude, on the scope issue 63 set | None |
| [0040](0040-the-main-branch-is-detected-at-init.md) | `acs init` writes the main branch when it is not `main`; `acs doctor` warns about a missing target branch | Claude, on the rule issue 67 set | None |
| [0041](0041-acs-undo-last.md) | `acs undo --last` picks the last agent session that changed files, and stops if it is still running | Claude, on the scope issue 64 set | None |
| [0042](0042-workflows-unsealed-and-example-env-files-writable.md) | `.github/workflows/**` is not sealed by default; `**/.env*` leaves out example files | You for both changes; Claude for building the `.env` change as a built-in exception (`**/.env*.{example,sample,template,dist}`) | Add `.github/workflows/**` to this repository's `guards.toml` if agents here should stay out of workflows |
| [0043](0043-antigravity-open-questions-settled.md) | Antigravity: `ask` stays an ask under `always-proceed`, subagents stay separate sessions, claims outlive a session, CLI support is stated, no strict mode for Claude Code in v0.1, agents run real checks through scripts; the adapter reads `agy` 1.5.4's new fields | You for writes outside the repository and for claims outliving a session; Claude for the rest, told to decide without asking | The desktop app checks (issue 6) |
| [0044](0044-write-paths-audit-fixes.md) | Write-paths audit: undo deletes only files with an absent pre-image, undo asks git whether a file equals `HEAD`, hook wrappers keep a disabled hook disabled, redaction finds more tokens, lore accept refuses a checked-out branch | Claude, on the audit brief | None; the open findings are in `docs/security/write-paths.md` |
| [0045](0045-write-paths-follow-up-fixes.md) | Write-paths follow-up: a failed undo commit leaves the index as it was, hook removal drops the newline install added, undo does not write through links, each pilot is told once what team sync publishes | You for the hook newline and the sync notice (the recommended options of issues 76 and 78); Claude for the other two | None; W2 (issue 74) and W9 (issue 38) are still open |
| [0046](0046-agents-are-asked-for-status-and-intent.md) | Every session is asked for `acs_status` and `acs_intent` once, at its start and by name; a session that edits without an intent is reminded once; Antigravity is told not to call `acs_status` with every message | You that both calls are expected for any task in any repository, and that Antigravity must not repeat `acs_status`; Claude for how | Reinstall `acs`; run `acs plugin install antigravity` again where Antigravity is used; check both desktop apps |
| [0047](0047-codex-adapter.md) | Codex is supported: adapter `codex`, installed as a block of hooks and an MCP server in `.codex/config.toml`; `ask` is rendered as deny; Codex's config files and `~/.codex` are sealed | You that Codex is supported; Claude for how | Reinstall `acs`; in a Codex repository run `acs plugin install codex`, then trust the project and the ACS hooks in Codex's `/hooks` |
| [0048](0048-agents-are-asked-for-lore-at-the-end-of-a-task.md) | New phase `turn_end` (the harnesses' `Stop`): an agent that changed files is asked once, at the end of a turn, to propose lore, where lore is on; Claude Code's start message loads `acs_lore_propose`; the propose reply and the VS Code status bar tell the pilot about waiting proposals; `docs/lore.md` says what goes in lore and what in hand-off notes | You for issue 80's recommendation (A, C, D's documentation) plus B; Claude for how, including blocking one `Stop` per session | Reinstall `acs`; run `acs plugin install codex` / `antigravity` again where those are used; check both desktop apps |
| [0049](0049-lore-benchmarks.md) | The hook is benchmarked with 50 lore entries: three warm calls under the pre-tool budget, first touches and `turn_end` measured without one; no cache of parsed lore yet (lore adds 6 to 8 ms per call at 50 entries; the first touch after a commit costs about 0.4 s on Windows) | Claude, on issue 57's brief | Issue 81: fewer git runs after a commit, a staleness cache worktrees do not reset, the parsed-lore cache |
| [0050](0050-undo-acts-in-the-work-tree-the-session-changed.md) | `acs undo` acts in the work tree the session changed: attributions and snapshot entries record their work tree (state schema 8), each file is planned and restored there, a file whose worktree is gone is left unknown; one undo commit per work tree | Claude, on issue 74's proposed fix | Reinstall `acs` |
| [0051](0051-install-from-main-until-the-first-release.md) | Until v0.1.0, the README and the setup skill install `acs` with `cargo install --git` from `main`, and the README says how to update the Claude Code plugin | You that testers get the newest `main`; Claude for how | Push `main` so testers get it; put the installers back at the first release (issue 5) |
| [0052](0052-plugin-install-without-the-harness-cli.md) | `acs plugin install` works without the harness's command line: for Claude Code it writes the two settings entries itself at project and local scope and names the step left in the desktop app (user scope still stops); Antigravity skips `agy plugin validate`; the Codex text mentions the app | You that all three must work for desktop-app users; Claude for how | Reinstall `acs`; check the plugin in each desktop app (issue 83) |
