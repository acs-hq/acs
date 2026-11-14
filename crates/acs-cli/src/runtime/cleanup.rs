//! Keeping the copies kept for undo within a size budget (decision 0037).
//!
//! The snapshot store only grows while sessions edit files. `acs sync` calls [`clean_snapshots`]
//! after it has written the session records; nothing on a hook's path does, because removing
//! blobs is not work for a hook's latency budget.

use std::collections::HashSet;

use acs_core::{SessionId, Timestamp};
use acs_store::{GcReport, StateDb};

use super::RepoContext;
use super::records::crash_after_secs;

/// The most the snapshot store of one repository may hold, in compressed bytes: 500 MiB. A
/// constant rather than a policy setting (the policy types are frozen).
pub const SNAPSHOT_BUDGET_BYTES: u64 = 500 * 1024 * 1024;

/// Overrides [`SNAPSHOT_BUDGET_BYTES`], in bytes (tests).
pub const SNAPSHOT_BUDGET_ENV: &str = "ACS_SNAPSHOT_BUDGET_BYTES";

/// The budget in force: [`SNAPSHOT_BUDGET_BYTES`] unless [`SNAPSHOT_BUDGET_ENV`] says otherwise.
#[must_use]
pub fn snapshot_budget() -> u64 {
    std::env::var(SNAPSHOT_BUDGET_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(SNAPSHOT_BUDGET_BYTES)
}

/// The sessions whose copies cleanup must keep: seats that have not ended and were seen within
/// the crash window. A seat silent for longer counts as crashed (its record is written as such)
/// and its copies are as removable as an ended session's; otherwise one crashed session would
/// pin its copies forever.
///
/// # Errors
///
/// The state database's error text.
pub fn live_sessions(db: &StateDb, now: Timestamp) -> Result<HashSet<SessionId>, String> {
    let quiet_since = now.as_second().saturating_sub(crash_after_secs());
    let seats = db.live_seats().map_err(|err| err.to_string())?;
    Ok(seats
        .into_iter()
        .filter(|s| s.seat.last_seen_at.as_second() >= quiet_since)
        .map(|s| s.session_id)
        .collect())
}

/// Evicts copies until the store is within budget (see `SnapshotStore::gc`), never those of live
/// sessions.
///
/// # Errors
///
/// Why the state or the snapshot store could not be read or trimmed.
pub fn clean_snapshots(ctx: &RepoContext, now: Timestamp) -> Result<GcReport, String> {
    let db = ctx.db().map_err(str::to_owned)?;
    let store = ctx.snapshots().map_err(str::to_owned)?;
    let live = live_sessions(db, now)?;
    store
        .gc(snapshot_budget(), &live)
        .map_err(|err| err.to_string())
}
