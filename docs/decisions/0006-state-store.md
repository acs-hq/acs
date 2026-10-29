# 0006: State store

Date: 2026-10-29 (WP1.2)

## Context

Every `acs hook` process reads and writes the repository's `state.db` (SPEC section 10) on the 15 ms p50 path, while up to dozens of other processes (other agents' hooks, the CLI, background sync) do the same. Decision 0004 rules 1, 2, 8 and 10 fix the contract: claim requests are idempotent heartbeats, all requests of an event are acquired in one `BEGIN IMMEDIATE` transaction re-checked with `acs_core::gate::claim_conflicts`, the store generates ids, and the workspace has one `rusqlite`.

## Decision

**Crates** (crates.io, 2026-10-29): `rusqlite 0.41.1` with `bundled` (SQLite built in, so behavior does not depend on the system library; the same entry WP1.6 uses) and `getrandom 0.4.3` (OS randomness for ids; MIT/Apache-2.0, tiny). `cargo deny` passes unchanged.

**Connection settings.** WAL (persistent, set once per database), `synchronous = NORMAL` (a process crash never loses a committed transaction; a power failure may lose the last few, which is fine for state that expires), `foreign_keys = ON`, `journal_size_limit = 4 MiB`, `busy_timeout = 3 s` (`DEFAULT_BUSY_TIMEOUT`, configurable through `DbConfig`). Write transactions last well under a millisecond, so the timeout is reached only if a process hangs holding the lock; 3 s is long enough that a burst of agents starting together does not degrade into `state_unavailable` (which means no claims) and short enough for any harness hook timeout. The file is created by the store with `fsutil::private_file_options` (`0o600`); SQLite gives `-wal` and `-shm` the same mode.

**Writers always use `BEGIN IMMEDIATE`.** A deferred transaction that reads and then writes fails at once with `SQLITE_BUSY` in WAL mode when another writer got in between, ignoring the busy timeout. Reads (`claims_snapshot`, `get_seat`, `pending_notices`, ...) take no write lock and are never blocked. `claims_snapshot` runs its queries in one read transaction so the local claims, the remote claims and the staleness flag come from one point in time.

**Opening is retried.** SQLite's busy handler does not cover switching a new file to WAL or reading it while another connection does that: those fail immediately with "locked". The 16-process test, in which every child opens a database that does not exist yet, hit this. `StateDb::open*` therefore retries the whole idempotent sequence (pragmas, version check, migrations) with a short backoff until the busy timeout is used up. This waiting uses the monotonic clock; no domain time is read inside the store.

**Migrations.** `PRAGMA user_version`, forward-only, `src/migrations/NNNN_name.sql` included with `include_str!`, one `BEGIN IMMEDIATE` transaction per step that re-reads the version first (processes racing to upgrade apply each step once). A database newer than the binary is refused (`SchemaTooNew`); never edit a released migration, add one. Version 1 holds the local state; version 2 adds the remote-claims view, which also gives the "from each previous version" test two real steps (it builds a database at every version, upgrades it, and compares the schema with a fresh one).

**Schema.** All tables are `STRICT`. Timestamps are `INTEGER` microseconds since the Unix epoch (`jiff` timestamps truncated to microseconds); enums are their `snake_case` wire names.

- `seats(seat_id PK, ..., ended_at)`, `UNIQUE(harness, harness_session_id)`.
- `claims`: live claims only. Columns for the `Claim` record plus `expires_at = heartbeat_at + ttl` so "not expired" is an indexable comparison, and `symbol_path` (NULL for a file claim). Index `(path, target_branch, expires_at)` serves the hook's lookup; a unique index `(seat_id, target_branch, path, COALESCE(symbol_path, ''))` makes "a seat holds a scope once" a database invariant. TTLs are clamped to 10^10 seconds so the arithmetic cannot overflow.
- `claim_history`: every ended claim, with `released_at` and `release_reason` (`released`, `expired`, `session_end`). A claim that had already run out is recorded as `expired` and released at its expiry time, whatever reason the caller gave. `seat_claim_history` unions this with the still-live claims for session records.
- `events`: seat, phase, tool name and kind, paths (JSON), `reason_code`, `outcome`, an optional `detail` cut to 512 characters whose redaction is the caller's job. No prompts, no file contents. `prune_events` trims it.
- `notices`, with a partial index on undelivered rows per seat; `stats(counter, value)` with saturating increments.
- `remote_claims(pilot_id, claim_id, ...)` with `fetched_at` per row, and `remote_sync` (one row: last success, last attempt, last error). `replace_remote_claims` replaces one pilot's claims; `retain_remote_pilots` drops pilots whose refs are gone; `record_sync_success/failure` maintain the status.

**Claims semantics.**

- `try_acquire(seat, requests, now, stale_after)` takes `stale_after` (3 x `sync_interval_secs`) because `claim_conflicts` needs the staleness of the remote view and the store cannot read the policy. It returns `Result<AcquireOutcome, StoreError>` where the outcome is `Granted(Acquired { claim_ids })` (one id per request, in order) or `Conflict(Conflict { conflicts })` listing every blocking claim once. Phase one checks every request against all local rows on its file (expired ones included, `claim_conflicts` decides) and the remote rows; if anything conflicts the transaction ends without writing. Phase two moves expired claims on those files to the history, then refreshes the seat's existing claim for each scope (`heartbeat_at = max(old, now)`, new TTL, same id) or inserts a new one. Requests for the same scope within one call share a claim.
- Expiry is lazy: queries filter on `expires_at > now` (the same instant as `Claim::is_expired`, both truncated to microseconds), and the move to the history happens in whatever write next touches the file, in `end_seat`, `release_*`, or `sweep_expired`. `claims_snapshot` is read-only and never moves anything.
- `heartbeat_seat_claims` only extends claims that have not expired: a revived claim could coexist with one someone else took in the meantime.
- `claims_snapshot(paths, target_branch, now, stale_after)` returns claims of every seat, including the caller's, on those files **and on that target branch** (a claim on another branch can never conflict or count as "held before this event" for this seat). Remote claims are omitted when the view is stale (`remote_stale = true`), and a remote claim with the id of a local one (our own claims fetched back through our own ref) is dropped. With no remote the view is permanently stale, so the pipeline should warn about staleness only when a remote exists.
- A seat that ended and sends another event is revived by `get_or_create_seat`: harnesses resume sessions, and late post-tool events happen. The record for the first run is the caller's concern.

**Ids.** `seat_`, `clm_`, `ntc_` plus 26 characters: 128 random bits from the OS in lowercase RFC 4648 base32 (`a-z2-7`), inside the `[0-9a-z_-]{1,64}` charset. They are not time-ordered; 128 random bits need no coordination between processes or machines. `ses_` ids are minted by whoever assembles session records (not part of this store yet).

**Errors.** `StoreError` has typed variants (`Locked`, `Corrupt`, `Missing`, `SchemaTooNew`, `OpenFailed`, `Io`, `Sqlite`, ...) with actionable messages (`acs doctor`). `is_state_unavailable()` is true for all of them except `UnknownSeat`, so the pipeline can map any failure to `GateState::state_unavailable` without matching. `DbConfig { create: false }` gives `Missing` instead of creating an empty database (for read-only commands); `integrity_check` is for `acs doctor`, not the hook path.

**The 16-process test** (`tests/claim_races.rs`) re-invokes the test binary 16 times. Each child opens the not-yet-existing database (creation and migration race too), loops 60 times asking for 1 to 3 scopes from a pool chosen to overlap (shared files, a class and its method, sibling symbols; four pilots share the 16 seats), holds granted claims for 0.2 to 1 ms, sometimes sends a heartbeat that must return the same ids, and logs `enter`/`exit` rows with an autoincrement sequence. The parent replays the log and fails if two overlapping scopes were ever held by different seats at once. Disabling the conflict check or blinding it to symbol claims makes the test fail.

## Consequences

- The pipeline (WP2.1) calls, per pre-tool event: `open`, `get_or_create_seat`, `touch_seat`, `claims_snapshot`, the engine, `try_acquire` (on `ask` and `allow`), `record_event`; on `deny` it records the event only. Each write is its own short transaction; with `synchronous = NORMAL` a commit is a WAL append, not an fsync.
- Measured on a warm database with 1,000 live claims (release build, Linux, local disk): open 0.40 ms p50; `claims_snapshot` 0.07 ms; `try_acquire` of one request 0.08 ms; both together 0.17 ms p50 and 0.38 ms p99. Process start and the rest of the pipeline dominate the 15 ms budget.
- `claim_history` and `events` grow without bound until `acs gc` (WP-later) prunes them; `prune_events` exists, history pruning should follow the ledger (a record is final once written).
- Windows and macOS were not run locally. Windows has no owner-only enforcement beyond the inherited ACL (decision 0003). SQLite's locking differs per platform (Windows uses mandatory locks), which the retry-on-open loop and the busy timeout are meant to absorb; the 16-process test runs there in CI.
- Spec follow-up: SPEC section 8 says the view is stale when the last successful fetch is "older than" 3 x sync interval; the store uses strictly greater than, and treats "never fetched" as stale.
