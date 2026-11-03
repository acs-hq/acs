# Antigravity gate checks

The Antigravity gate of plan section 7, brought forward by the human (decision 0024). Machine:
Windows 11, Git Bash and PowerShell 5.1, `agy` 1.4.12 with `gemini-3.8-flash-medium`, Claude Code
2.1.316 with Sonnet, `acs` 0.1.0 from a local release build. Date: 2026-11-03. Every run used a
throwaway repository and `ACS_HOME`; no global agy or Claude Code setting was changed. **The desktop
app was not run** (the human's decision): its checks are in `docs/HANDOFF.md` section 7.

## Manual checklist (`docs/adapters/antigravity.md` section 13), CLI

| # | Check | Result |
| --- | --- | --- |
| 1 | Version, official docs | `agy` 1.4.12; the official pages were read and section 15 of the adapter doc records where they differ from the CLI |
| 2 | `agy plugin validate --strict` | Passes (one note: no logo). Installed in the workspace (`.agents/plugins/acs/`): hooks, MCP server `acs_acs` and rule load |
| 3 | Real payloads | Captured for every event; now the `real_*.json` fixtures |
| 4 | Denial | An edit of a human-claimed file was denied by the plugin's hook; the model quoted the reason exactly and did not try another way |
| 5 | "No objection" adds no prompts | CLI: under `request-review`, no hook, `allow` and `ask` refused the same calls. ACS now sends `allow` (decision 0024). App: not run |
| 6 | `force_ask` | Prompts (refused headless) under `request-review`; **approved silently under `always-proceed`** (an outside write went through) |
| 7 | Fail closed | With `acs` missing, every tool call was denied |
| 8 | Context | The session-start status reached the model; a resumed turn did not repeat it; counters recorded in the adapter doc |
| 9 | MCP binding | `acs_status` listed the agent's own fresh claim as its own. Subagents have their own ids in hooks and are separate seats |
| 10 | Windows shell | A PowerShell `Remove-Item` of an unclaimed file was refused |
| 11 | Rule | Loaded: the agent called `acs_status` and `acs_intent` unprompted |

## Original M1 gate: two Claude Code sessions and one Antigravity session

`ACS_REAL_ANTIGRAVITY=rounding tests/e2e/real/three_sessions.sh`: `discount` and `tax` in
`claude -p`, `rounding` in `agy -p`, all started together in one work tree. Each must add a
function to `src/pricing.py`, a line to `CHANGELOG.md`, change a file of its own and commit.
`check.mjs` judges from Claude Code's stream and Antigravity's conversation transcript.

| | Run 1: ACS tools withheld | Run 2: ACS tools allowed |
| --- | --- | --- |
| Sessions, over rounds | 5, over 3 | 4, over 2 |
| Successful file writes | 9 | 10 |
| Added lines lost | **0** | **0** |
| Calls blocked in the logs | 2 (`Edit`, by the hook) | 2 (`Edit`, by the hook) |
| Collisions in `acs status` | 2 | 2 |
| Tasks in `HEAD` at the end | 2 of 3 | **3 of 3** |
| Commits with an `ACS-Session` trailer | 3 of 3 | 3 of 3 |
| Verdict of `check.mjs` | FAIL (lost work only) | **PASS** |

The gate's two conditions, zero silent overwrites and every collision visible in `acs status`, held
in both runs. Run 2 is the normal configuration and passes outright.

Run 1 is what happens without the MCP tools, and it shows two real problems:

- **Antigravity's claims outlive its session.** Antigravity sends no session end, so a finished
  `agy` session's claims stay live until their TTL (15 minutes). `discount` was refused in rounds 2
  and 3, which started right after the Antigravity session ended, and ran out of rounds. In run 2 the
  agent released its claims with `acs_release`, as the usage skill says, and the problem did not
  arise.
- **Without MCP the agent used the human `acs` CLI.** It ran `acs claim`, `acs release --all` and
  `acs release <its files>` in the shell. Those act on the human seat, so its own agent claims were
  never released. While it waited for `tax`, it also read well outside the workspace: the run's logs,
  the gate script in the ACS checkout, the supervising session's task output and `ACS_HOME`. Reads
  only, and allowed by ACS, but this is the agent behaviour WP7.2 has to cover.

Run 2's Antigravity session stayed in the repository, used `acs_status`, `acs_intent`, `acs_claim`
and `acs_release`, and waited for `tax` before editing the shared files. No blocked agent in either
run edited around a denial. Reported Claude Code cost: $0.54 (run 1), $0.34 (run 2); Antigravity
does not report cost.
