# 0004 — In-process gate interfaces for Phase 1

Date: 2026-10-29. Status: accepted (orchestrator), frozen for Phase 1.

## Context

Phase 1 builds six components in parallel: policy (WP1.1), state store (WP1.2), paths (WP1.3),
shell analysis (WP1.4), decision engine (WP1.5) and snapshots (WP1.6). The engine lives in
`acs-core`, which is pure and sits at the top of the dependency order, so it cannot depend on the
crates that produce its inputs. The plan says WP1.5 depends on "the types (not implementations)"
of WP1.1 and WP1.2; those types have to exist before fan-out.

## Decision

`crates/acs-core/src/gate.rs` (module `acs_core::gate`) defines the Rust contracts between the
components. They are in-process interfaces, not wire formats: no JSON Schema, not in `SPEC.md`.

| Type | Produced by | Consumed by |
| --- | --- | --- |
| `ResolvedPath`, `PathLocation` | acs-paths | pipeline, engine |
| `ShellAnalysis`, `SimpleCommand`, `ShellTarget`, `GitInvocation`, `CommandClass`, `Confidence`, `TargetOp` | acs-shell | pipeline, engine |
| `PolicyView` (trait), `ZoneMatch` | acs-policy implements | engine |
| `ClaimsSnapshot`, `HeldClaim`, `ClaimSource` | acs-store | engine |
| `ClaimRequest` | engine | acs-store |
| `claim_conflicts()` | acs-core | engine **and** acs-store (single implementation of spec §6) |
| `Target`, `Expansion`, `GateState`, `GateInput` | pipeline (WP2.1) | engine |
| `GateOutput`, `SnapshotRequest` | engine | pipeline, acs-store |

The engine's entry point (WP1.5) is
`acs_core::engine::decide(input: &GateInput<'_>, policy: &dyn PolicyView) -> GateOutput`.

### Cross-component rules

1. **Claim requests are idempotent heartbeats.** The engine requests a file claim for every write
   or delete target that needs one, even if the seat already holds it. The store treats a request
   for a scope the seat already holds (same seat, same scope, same target branch, not expired) as a
   heartbeat: it refreshes `heartbeat_at` and returns the existing claim id.
2. **Atomicity.** The store acquires all requests of one event in one SQLite transaction
   (`BEGIN IMMEDIATE`), re-checking each with `claim_conflicts` against local claims and the
   remote view. Either all are granted or none, and the store reports the conflicting claims.
3. **Which writes need claims (Phase 1).** Targets in `shared` zones or in no zone need a file
   claim; `open` zones need none; `sealed` zones deny with `sealed_path`. `owned` zones are treated
   as `shared` until WP7.1 (so `owned_territory` is never returned in Phase 1). Symbol claims start
   in Phase 8; Phase 1 requests file claims only. Reads never need claims.
4. **Deletes outside claims.** A deleted repository file counts as "outside claims" when it is not
   covered by a non-expired claim the requesting seat held *before* this event (claims requested by
   the same event do not count) and it is not in an `open` zone. The count is compared with
   `destructive.max_files_deleted_outside_claims` (deny); the total number of files deleted by one
   command is compared with `max_files_deleted_per_command` (ask).
5. **Recursive and link-crossing targets.** The pipeline expands recursive and glob targets into
   `Expansion` (capped just above the largest budget). A delete whose `ResolvedPath` is outside the
   repository but has `linked_into_repo` entries reaches those repository paths: the engine checks
   them like in-repository delete targets *and* returns `path_escapes_repo` (ask) at least.
6. **Case.** `RepoPath` values from acs-paths are in on-disk case on case-insensitive
   filesystems. `PolicyView` implementations are constructed with the filesystem's case
   sensitivity and match case-insensitively when it is insensitive.
7. **Sealed defaults are not removable.** `DEFAULT_SEALED` (plus adapter additions later) always
   applies; `guards.toml`'s `sealed` adds to it. Gate-off and every other mode still enforce sealed
   paths.
8. **IDs and time.** The store generates ids (random, lowercase, within the id charset of
   0002); the engine never does. The engine takes `now` from `GateInput`.
9. **Snapshot index.** WP1.6 keeps its blob index in its own SQLite file
   `ACS_HOME/repos/<id>/snapshots/index.db`, not in `state.db`, so the snapshot store and the state
   store evolve independently. Eviction takes the set of live sessions as an argument.
10. **One `rusqlite`.** WP1.2 and WP1.6 use the same workspace `rusqlite` dependency (`bundled`).

## Consequences

- Changes to `gate.rs` are orchestrator-only, like the rest of the contracts; a work package that
  needs one stops and reports.
- WP2.1 (hook pipeline) is mostly glue: resolve paths and shell targets, expand, load the snapshot
  of claims, call `decide`, apply claim and snapshot requests, render.
