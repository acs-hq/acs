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
| [0013](0013-claude-code-adapter.md) | Claude Code adapter and plugin | Claude | Strict mode when `acs` is missing (issue 11) |
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
| [0024](0024-antigravity-gate.md) | The Antigravity gate | Claude | Three questions and the desktop app checks (issues 6, 7, 8, 9) |
| [0025](0025-init-proposals-without-codeowners.md) | `acs init` proposals without CODEOWNERS | You | |
| [0026](0026-territory-overrides-and-self-protection.md) | Owned territory, overrides, self-protection | You, on four questions; the rest Claude | |
| [0027](0027-outside-the-repository-is-opt-in.md) | Writes outside the repository ask only on request | You | |
| [0028](0028-human-claims-from-editors.md) | Human claims from editors | Claude; the release rule and the `notify` crate are yours | Publishing the extension |
| [0029](0029-lore.md) | Lore | Claude | |
| [0030](0030-session-records-undo-verify.md) | Session records, log, blame, undo, verify, stats | Claude | |
| [0031](0031-first-testers-and-first-run-defaults.md) | First testers before the spec; first-run defaults | You | Territory prompt at init, workflow and `.env` seals, commit trailer toggle |
| [0032](0032-calls-are-judged-in-the-work-tree-of-their-files.md) | A call is judged in the work tree its files are in | Claude | Four cases not covered (issue 69) |
| [0033](0033-feature-switches.md) | Feature switches: `[features]` in `config.toml`, `acs features` | You for the idea and the seven switches; Claude for what each covers and what is never off | Set `sync = true` here if this repository should keep syncing |
| [0034](0034-what-agents-are-told-follows-the-switches.md) | What agents are told (MCP tools, instructions, rule, skill) follows the feature switches | You for the rule and for covering every switch; Claude for the details | A real-agent run; hook cost (issue 70) |
| [0035](0035-the-hook-ends-early-when-no-feature-needs-the-call.md) | `acs hook` ends early for a tool call that no switched-on feature needs | Claude, on a task that set which calls; the keep-alive, sync after a read and session end are Claude's | Whether hooks should stop starting at all (issue 70) |
| [0036](0036-no-prompt-submit-hook-until-notices-exist.md) | The Claude Code plugin registers no prompt-submit hook until notices exist | You; leaving Antigravity's hook in place is Claude's (it is that harness's session start) | Register it again when Phase 8 builds the first notice |
| [0037](0037-snapshot-cleanup-runs-in-sync.md) | The copies kept for undo are trimmed to 500 MiB per repository during `acs sync` | Claude (budget, when it runs, crashed seats not counted as live) | Whether the budget should become a setting |
| [0038](0038-help-flags-are-read-only.md) | `acs <subcommand> --help` is allowed to agents when help is all the command does | Claude | None |
