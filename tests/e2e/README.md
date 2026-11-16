# End-to-end scenarios

Scripted multi-seat runs (WP10.2). Simulated agents drive the built `acs` binary with canonical
events, so no model is called. They run as the `e2e_scenarios` test of `acs-cli`, because a virtual
workspace root can hold no targets:

```sh
cargo test -p acs-cli --test e2e_scenarios
```

| Scenario | File | Milestone |
| --- | --- | --- |
| v0.1 acceptance: three seats, overlapping tasks, zero silent overwrites, collisions in `acs status` | `acceptance.rs` | M1 |
| Sealed paths under the adversarial basics | `sealed.rs` | M1 |
| An offline team | `offline_team.rs` | M1 |
| Husky coexistence | `husky.rs` | M1 |
| Junction deletion: the gate stops it | `junction.rs` | M1 |
| Junction deletion: recovery with `acs undo` (gate off, delete goes through, every file back byte-exact) | `junction_undo.rs` | M2 |
| Interleaved undo: session B's later edits survive undoing session A | `undo_interleaved.rs` | M2 |
| Interface notices | to do | M5 |
| Queued edits | to do | M5 |

`crates/acs-cli/tests/e2e.rs` covers the pipeline's individual behaviours.

## Real agents

`real/three_sessions.sh` is the same acceptance test with three real Claude Code sessions in a
throwaway repository, for the manual check before each release (plan section 9.1). It spends real
model calls, so CI does not run it. `real/check.mjs` judges the run from Claude Code's own logs, and
the result goes in `docs/releases/`.

Three more real-agent scripts cover what a first tester does. Each takes an empty scratch
directory, prints one line per check and exits 1 when a check fails; `real/common.sh` holds what
they share.

| Script | What it checks |
| --- | --- |
| `real/first_run.sh` | `acs init --yes` and nothing else: an edit and a commit go through with the trailer, `.env` and a banned command hold, nothing reaches the remote, and `acs uninstall --all` leaves git's config, hooks and remote byte-identical |
| `real/first_calls.sh` | Agents given tasks that never mention ACS call `acs_status` and `acs_intent` before their first change: three Claude Code sessions, and one Antigravity conversation of three messages in which `acs_status` is called once (decision 0046) |
| `real/history.sh` | `acs log`, `acs blame`, `acs stats`, `acs verify`; `acs undo --last` keeps the other session's work byte-identical; a committed session is undone with a new commit; a person's commit against a live claim is rejected |
| `real/guards.sh` | An agent asked to delete a folder, force-push, `git clean`, change `.env`, run `acs off` and commit with `--no-verify` is refused each time; `.env.example` and `.github/workflows/` stay writable. On Windows it runs again through the PowerShell tool |
