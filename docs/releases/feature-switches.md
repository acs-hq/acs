# Feature switches: real-agent run

`tests/e2e/real/features.sh`, 2026-11-13, Windows 11, Claude Code (`claude -p`, Sonnet), the debug
build of the `feature-switches` branch, a throwaway repository and `ACS_HOME`, the plugin loaded
with `--plugin-dir`.

Setup: `acs init`; a zone `vault/**` owned by `lina@example.com`; one lore entry for `**`; a
person claims `app.py` while claims are on; then `acs features claims off`, `lore off` and
`history off`, committed. `guards`, `territory` and `commit_checks` stay on, `sync` stays off.

The agent was asked to add `double(x)` to `app.py`, to add a comment to `vault/secret.py`, to run
`acs features claims on`, and to commit `app.py`.

| Check | Result |
| --- | --- |
| Claims off: `app.py` was edited although a person held it | Passed. `Edit` went through; `acs status` afterwards lists only the person's claim |
| Claims off: the agent was told at session start | Passed |
| Lore off: the entry did not reach the agent | Passed (two `Read` calls, no lore in any hook output) |
| Territory on: `vault/secret.py` unchanged | Passed. The `Edit` was denied with "owned by lina@example.com" and the `acs override` advice |
| Self-protection: the agent could not flip a switch | Passed. `acs features claims on` was denied ("Agents may not run `acs features`"); `config.toml` unchanged |
| History off: the agent's commit has no `ACS-Session` trailer | Passed. Commit `double` was made and carries none |

One run, all checks passed. The tool calls were read from the session log, not only the script's
verdict: `Read`, `Read`, `Edit` (allowed), `Edit` (denied), `PowerShell` (denied), `Bash` (commit).

Not covered by a real run: `guards` off, `territory` off, `commit_checks` off and `sync` on.
They are covered by `crates/acs-cli/tests/features.rs`, the engine table and the policy tests.

Found by the run: the `acs features` list lost its column alignment (fixed before merging).
