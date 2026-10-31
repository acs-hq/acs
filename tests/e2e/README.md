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
| Junction deletion: recovery with `acs undo` | to do | M2 |
| Interleaved undo | to do | M2 |
| Interface notices | to do | M5 |
| Queued edits | to do | M5 |

`crates/acs-cli/tests/e2e.rs` covers the pipeline's individual behaviours.

## Real agents

`real/three_sessions.sh` is the same acceptance test with three real Claude Code sessions in a
throwaway repository, for the manual check before each release (plan section 9.1). It spends real
model calls, so CI does not run it. `real/check.mjs` judges the run from Claude Code's own logs, and
the result goes in `docs/releases/`.
