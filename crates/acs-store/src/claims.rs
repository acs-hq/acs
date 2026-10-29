//! Claims: the live local claims, their history, and the remote-claims view.
//!
//! The rules (SPEC section 6, decision 0004 rules 1 and 2):
//!
//! - [`StateDb::try_acquire`] takes all the claims one event needs in a single `BEGIN IMMEDIATE`
//!   transaction. Inside it every request is re-checked with [`acs_core::gate::claim_conflicts`]
//!   against the local claims and the remote view, so two processes can never both be granted
//!   conflicting claims, whatever the (possibly stale) snapshot they decided on said. It is all or
//!   nothing.
//! - A request for a scope the seat already holds is a heartbeat: it refreshes the claim and
//!   returns the existing id.
//! - Expiry is lazy. Queries ignore claims whose `expires_at` has passed; a claim is moved to the
//!   history, with reason `expired`, by whatever write next touches its file (or by
//!   [`StateDb::sweep_expired`]). A heartbeat never revives an expired claim: someone else may
//!   hold the file by then.

use std::time::Duration;

use acs_core::gate::{ClaimRequest, ClaimSource, ClaimsSnapshot, HeldClaim, claim_conflicts};
use acs_core::{
    Claim, ClaimHistoryEntry, ClaimId, ClaimOrigin, ClaimScope, ReleaseReason, RepoPath, SeatId,
    SymbolPath, Timestamp,
};
use rusqlite::types::{ToSql, Value};
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use crate::db::{
    StateDb, StoreError, enum_from_db, enum_to_db, expiry_micros, from_db, id_from_db, new_id,
    placeholders, to_db, ttl_to_db,
};

/// How many file paths one snapshot query binds at most (SQLite limits bound parameters).
const PATHS_PER_QUERY: usize = 500;

/// All claims granted (or refreshed) by one [`StateDb::try_acquire`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acquired {
    /// One claim id per request, in request order. An id is that of the existing claim when the
    /// request was a heartbeat.
    pub claim_ids: Vec<ClaimId>,
}

/// Why a [`StateDb::try_acquire`] call granted nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// Every live claim, local or remote, that blocks at least one request, each listed once.
    pub conflicts: Vec<HeldClaim>,
}

/// The result of [`StateDb::try_acquire`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireOutcome {
    /// Every request was granted or refreshed.
    Granted(Acquired),
    /// Nothing was granted or changed.
    Conflict(Conflict),
}

/// The state of the last synchronization with the remote, from [`StateDb::remote_sync_status`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncStatus {
    /// When a fetch last succeeded. `None` if never: the remote view is then stale.
    pub last_success_at: Option<Timestamp>,
    /// When a sync was last attempted.
    pub last_attempt_at: Option<Timestamp>,
    /// What went wrong in the last attempt, if it failed.
    pub last_error: Option<String>,
}

/// Longest TTL stored. About 317 years, which cannot overflow the microsecond arithmetic and is
/// "never" for a claim.
const MAX_TTL_SECS: u64 = 10_000_000_000;

fn ttl_for_db(ttl_secs: u64) -> i64 {
    ttl_to_db(ttl_secs.min(MAX_TTL_SECS))
}

/// A claim row (local, or remote with the last two columns NULL) before validation.
struct RawClaim {
    claim_id: String,
    seat_id: String,
    pilot: String,
    path: String,
    symbol_path: Option<String>,
    target_branch: String,
    acquired_at: i64,
    heartbeat_at: i64,
    ttl_secs: i64,
    origin: String,
    holder_harness: Option<String>,
    holder_last_seen_at: Option<i64>,
}

impl RawClaim {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            claim_id: row.get(0)?,
            seat_id: row.get(1)?,
            pilot: row.get(2)?,
            path: row.get(3)?,
            symbol_path: row.get(4)?,
            target_branch: row.get(5)?,
            acquired_at: row.get(6)?,
            heartbeat_at: row.get(7)?,
            ttl_secs: row.get(8)?,
            origin: row.get(9)?,
            holder_harness: row.get(10)?,
            holder_last_seen_at: row.get(11)?,
        })
    }

    fn into_held(self, source: ClaimSource) -> Result<HeldClaim, StoreError> {
        let claim = Claim {
            claim_id: id_from_db(self.claim_id)?,
            seat_id: id_from_db(self.seat_id)?,
            pilot: self.pilot,
            scope: scope_from_db(self.path, self.symbol_path)?,
            target_branch: self.target_branch,
            acquired_at: from_db(self.acquired_at)?,
            heartbeat_at: from_db(self.heartbeat_at)?,
            ttl_secs: u64::try_from(self.ttl_secs).unwrap_or(0),
            origin: enum_from_db(&self.origin)?,
        };
        Ok(HeldClaim {
            claim,
            source,
            holder_harness: self.holder_harness,
            holder_last_seen_at: self.holder_last_seen_at.map(from_db).transpose()?,
        })
    }
}

const LOCAL_SELECT: &str = "SELECT c.claim_id, c.seat_id, c.pilot, c.path, c.symbol_path, \
    c.target_branch, c.acquired_at, c.heartbeat_at, c.ttl_secs, c.origin, s.harness, s.last_seen_at \
    FROM claims c LEFT JOIN seats s ON s.seat_id = c.seat_id";

const REMOTE_SELECT: &str = "SELECT claim_id, seat_id, pilot, path, symbol_path, target_branch, \
    acquired_at, heartbeat_at, ttl_secs, origin, NULL, NULL FROM remote_claims";

fn scope_from_db(path: String, symbol_path: Option<String>) -> Result<ClaimScope, StoreError> {
    let path = RepoPath::try_from(path).map_err(|err| StoreError::invalid("claim path", err))?;
    Ok(match symbol_path {
        None => ClaimScope::File { file: path },
        Some(symbol_path) => ClaimScope::Symbol {
            symbol: path,
            symbol_path: SymbolPath::try_from(symbol_path)
                .map_err(|err| StoreError::invalid("symbol path", err))?,
        },
    })
}

/// The scope as the `(path, symbol_path)` columns.
fn scope_to_db(scope: &ClaimScope) -> (&str, Option<&str>) {
    match scope {
        ClaimScope::File { file } => (file.as_str(), None),
        ClaimScope::Symbol {
            symbol,
            symbol_path,
        } => (symbol.as_str(), Some(symbol_path.as_str())),
    }
}

fn collect_held(
    stmt: &mut rusqlite::CachedStatement<'_>,
    params: impl rusqlite::Params,
    source: ClaimSource,
) -> Result<Vec<HeldClaim>, StoreError> {
    let raws: Vec<RawClaim> = stmt
        .query_map(params, RawClaim::from_row)?
        .collect::<Result<_, _>>()?;
    raws.into_iter().map(|raw| raw.into_held(source)).collect()
}

/// Whether the remote view is stale at `now`: never fetched, or the last successful fetch is
/// older than `stale_after` (SPEC section 8).
fn remote_is_stale(
    conn: &Connection,
    now: Timestamp,
    stale_after: Duration,
) -> Result<bool, StoreError> {
    let last: Option<Option<i64>> = conn
        .prepare_cached("SELECT last_success_at FROM remote_sync WHERE id = 1")?
        .query_row([], |row| row.get(0))
        .optional()?;
    Ok(match last.flatten() {
        None => true,
        Some(last) => {
            let limit = i64::try_from(stale_after.as_micros()).unwrap_or(i64::MAX);
            to_db(now).saturating_sub(last) > limit
        }
    })
}

/// Moves the claims matching `filter` from `claims` to `claim_history`.
///
/// A claim whose time has run out is recorded as `expired`, released at its expiry time, whatever
/// `reason` says; otherwise it is released at `now` with `reason`. `extra` binds the named
/// parameters used by `filter`. Returns the ids moved.
pub(crate) fn archive_claims(
    conn: &Connection,
    filter: &str,
    extra: &[(&str, &dyn ToSql)],
    reason: ReleaseReason,
    now: Timestamp,
) -> Result<Vec<ClaimId>, StoreError> {
    let reason = enum_to_db(&reason)?;
    let now = to_db(now);
    // SQLite rejects a bound name the statement does not use, so each statement gets its own set.
    let mut delete_args: Vec<(&str, &dyn ToSql)> = extra.to_vec();
    if filter.contains(":now") {
        delete_args.push((":now", &now));
    }
    let mut insert_args = extra.to_vec();
    insert_args.push((":now", &now));
    insert_args.push((":reason", &reason));
    conn.prepare_cached(&format!(
        "INSERT INTO claim_history (claim_id, seat_id, pilot, path, symbol_path, target_branch, \
         origin, acquired_at, released_at, release_reason) \
         SELECT claim_id, seat_id, pilot, path, symbol_path, target_branch, origin, acquired_at, \
                CASE WHEN expires_at <= :now THEN expires_at ELSE :now END, \
                CASE WHEN expires_at <= :now THEN 'expired' ELSE :reason END \
         FROM claims WHERE {filter}"
    ))?
    .execute(&insert_args[..])?;
    let ids: Vec<String> = conn
        .prepare_cached(&format!(
            "DELETE FROM claims WHERE {filter} RETURNING claim_id"
        ))?
        .query_map(&delete_args[..], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    ids.into_iter().map(id_from_db).collect()
}

impl StateDb {
    /// The claims relevant to an event touching `paths`: every unexpired claim, of any seat
    /// (including the requesting seat's own), whose scope is in one of those files and whose
    /// target branch is `target_branch`; local claims and, when the remote view is fresh, remote
    /// ones. A remote claim that duplicates a local one (the machine's own claims fetched back)
    /// is dropped.
    ///
    /// `remote_stale` is set when the last successful fetch is older than `stale_after`
    /// (`3 x sync_interval_secs`) or there never was one; remote claims are then left out
    /// (SPEC section 11: fail open on the remote). With no remote configured the view is
    /// permanently stale, so the pipeline should only warn about it when a remote exists.
    ///
    /// A read: it takes no write lock and moves nothing to the history. All queries see one
    /// consistent snapshot.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn claims_snapshot(
        &self,
        paths: &[RepoPath],
        target_branch: &str,
        now: Timestamp,
        stale_after: Duration,
    ) -> Result<ClaimsSnapshot, StoreError> {
        let mut files: Vec<&str> = paths.iter().map(RepoPath::as_str).collect();
        files.sort_unstable();
        files.dedup();
        let tx = self.conn.unchecked_transaction()?;
        let remote_stale = remote_is_stale(&tx, now, stale_after)?;
        let mut claims: Vec<HeldClaim> = Vec::new();
        for chunk in files.chunks(PATHS_PER_QUERY) {
            let list = placeholders(3, chunk.len());
            let mut args = vec![
                Value::Text(target_branch.to_owned()),
                Value::Integer(to_db(now)),
            ];
            args.extend(chunk.iter().map(|path| Value::Text((*path).to_owned())));
            let local = collect_held(
                &mut tx.prepare_cached(&format!(
                    "{LOCAL_SELECT} WHERE c.target_branch = ?1 AND c.expires_at > ?2 \
                     AND c.path IN ({list}) ORDER BY c.path, c.acquired_at, c.claim_id"
                ))?,
                params_from_iter(args.iter()),
                ClaimSource::Local,
            )?;
            claims.extend(local);
            if !remote_stale {
                let remote = collect_held(
                    &mut tx.prepare_cached(&format!(
                        "{REMOTE_SELECT} WHERE target_branch = ?1 AND expires_at > ?2 \
                         AND path IN ({list}) ORDER BY path, acquired_at, claim_id"
                    ))?,
                    params_from_iter(args.iter()),
                    ClaimSource::Remote,
                )?;
                for held in remote {
                    if !claims
                        .iter()
                        .any(|c| c.claim.claim_id == held.claim.claim_id)
                    {
                        claims.push(held);
                    }
                }
            }
        }
        tx.commit()?;
        Ok(ClaimsSnapshot {
            claims,
            remote_stale,
        })
    }

    /// Grants every request to `seat_id`, or none.
    ///
    /// Runs in one `BEGIN IMMEDIATE` transaction. Each request is checked with
    /// [`claim_conflicts`] against all local claims and the remote view on its file (`now` and
    /// `stale_after` decide expiry and staleness). If any request conflicts, nothing is changed
    /// and [`AcquireOutcome::Conflict`] lists all blocking claims. Otherwise each request either
    /// refreshes the seat's existing unexpired claim on the same scope and target branch (a
    /// heartbeat: `heartbeat_at = now`, new TTL, same id) or creates a new claim with
    /// `acquired_at = heartbeat_at = now`; expired claims on those files go to the history.
    /// Requests for the same scope within one call share one claim.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownSeat`] if the seat does not exist; any other [`StoreError`] (for
    /// example [`StoreError::Locked`] after the busy timeout).
    pub fn try_acquire(
        &self,
        seat_id: &SeatId,
        requests: &[ClaimRequest],
        now: Timestamp,
        stale_after: Duration,
    ) -> Result<AcquireOutcome, StoreError> {
        self.write(|tx| {
            let pilot: String = tx
                .prepare_cached("SELECT pilot FROM seats WHERE seat_id = ?1")?
                .query_row(params![seat_id.as_str()], |row| row.get(0))
                .optional()?
                .ok_or_else(|| StoreError::UnknownSeat(seat_id.to_string()))?;
            let remote_stale = remote_is_stale(tx, now, stale_after)?;

            // Phase 1: look for conflicts before changing anything.
            let mut conflicts: Vec<HeldClaim> = Vec::new();
            let mut checked_paths: Vec<&str> = Vec::new();
            for request in requests {
                let path = request.scope.path().as_str();
                let mut candidates = collect_held(
                    &mut tx.prepare_cached(&format!("{LOCAL_SELECT} WHERE c.path = ?1"))?,
                    params![path],
                    ClaimSource::Local,
                )?;
                if !remote_stale {
                    candidates.extend(collect_held(
                        &mut tx.prepare_cached(&format!("{REMOTE_SELECT} WHERE path = ?1"))?,
                        params![path],
                        ClaimSource::Remote,
                    )?);
                }
                for held in candidates {
                    if claim_conflicts(
                        &held,
                        seat_id,
                        &request.scope,
                        &request.target_branch,
                        now,
                        remote_stale,
                    ) && !conflicts
                        .iter()
                        .any(|c| c.claim.claim_id == held.claim.claim_id)
                    {
                        conflicts.push(held);
                    }
                }
                checked_paths.push(path);
            }
            if !conflicts.is_empty() {
                return Ok(AcquireOutcome::Conflict(Conflict { conflicts }));
            }

            // Phase 2: nothing blocks. Retire expired claims on these files, then refresh or insert.
            checked_paths.sort_unstable();
            checked_paths.dedup();
            let paths_json = serde_json::to_string(&checked_paths)
                .map_err(|err| StoreError::invalid("paths", err))?;
            archive_claims(
                tx,
                "expires_at <= :now AND path IN (SELECT value FROM json_each(:paths))",
                &[(":paths", &paths_json)],
                ReleaseReason::Expired,
                now,
            )?;
            let now_micros = to_db(now);
            let mut claim_ids = Vec::with_capacity(requests.len());
            for request in requests {
                let (path, symbol_path) = scope_to_db(&request.scope);
                let ttl = ttl_for_db(request.ttl_secs);
                let existing: Option<String> = tx
                    .prepare_cached(
                        "UPDATE claims SET heartbeat_at = max(heartbeat_at, ?5), ttl_secs = ?6, \
                         expires_at = max(heartbeat_at, ?5) + ?6 * 1000000 \
                         WHERE seat_id = ?1 AND target_branch = ?2 AND path = ?3 \
                         AND COALESCE(symbol_path, '') = COALESCE(?4, '') RETURNING claim_id",
                    )?
                    .query_row(
                        params![
                            seat_id.as_str(),
                            request.target_branch,
                            path,
                            symbol_path,
                            now_micros,
                            ttl
                        ],
                        |row| row.get(0),
                    )
                    .optional()?;
                let claim_id: ClaimId = match existing {
                    Some(id) => id_from_db(id)?,
                    None => {
                        let id: ClaimId = new_id(ClaimId::PREFIX)?;
                        tx.prepare_cached(
                            "INSERT INTO claims (claim_id, seat_id, pilot, path, symbol_path, \
                             target_branch, acquired_at, heartbeat_at, ttl_secs, expires_at, origin) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9, ?10)",
                        )?
                        .execute(params![
                            id.as_str(),
                            seat_id.as_str(),
                            pilot,
                            path,
                            symbol_path,
                            request.target_branch,
                            now_micros,
                            ttl,
                            expiry_micros(now_micros, ttl),
                            enum_to_db(&request.origin)?,
                        ])?;
                        id
                    }
                };
                claim_ids.push(claim_id);
            }
            Ok(AcquireOutcome::Granted(Acquired { claim_ids }))
        })
    }

    /// Refreshes `heartbeat_at` to `now` on all of the seat's unexpired claims. Claims that have
    /// already expired are not revived. Returns how many were refreshed.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn heartbeat_seat_claims(
        &self,
        seat_id: &SeatId,
        now: Timestamp,
    ) -> Result<usize, StoreError> {
        Ok(self
            .conn
            .prepare_cached(
                "UPDATE claims SET heartbeat_at = max(heartbeat_at, ?2), \
                 expires_at = max(heartbeat_at, ?2) + ttl_secs * 1000000 \
                 WHERE seat_id = ?1 AND expires_at > ?2",
            )?
            .execute(params![seat_id.as_str(), to_db(now)])?)
    }

    /// Releases the given claims with `reason` (a claim that had already expired is recorded as
    /// `expired` instead). Unknown ids are ignored. Returns the claims that were moved to the
    /// history.
    ///
    /// This does not check who owns the claims: callers that act for an agent pass only that
    /// seat's ids.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn release_claims(
        &self,
        claim_ids: &[ClaimId],
        reason: ReleaseReason,
        now: Timestamp,
    ) -> Result<Vec<ClaimId>, StoreError> {
        let ids = serde_json::to_string(&claim_ids.iter().map(ClaimId::as_str).collect::<Vec<_>>())
            .map_err(|err| StoreError::invalid("claim ids", err))?;
        self.write(|tx| {
            archive_claims(
                tx,
                "claim_id IN (SELECT value FROM json_each(:ids))",
                &[(":ids", &ids)],
                reason,
                now,
            )
        })
    }

    /// Releases all of the seat's claims with `reason`. Returns the claims moved to the history.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn release_seat_claims(
        &self,
        seat_id: &SeatId,
        reason: ReleaseReason,
        now: Timestamp,
    ) -> Result<Vec<ClaimId>, StoreError> {
        self.write(|tx| {
            archive_claims(
                tx,
                "seat_id = :seat",
                &[(":seat", &seat_id.as_str())],
                reason,
                now,
            )
        })
    }

    /// Moves every expired claim to the history with reason `expired`. Nothing needs this for
    /// correctness (queries ignore expired claims); it keeps the live table small. Returns how
    /// many were moved.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn sweep_expired(&self, now: Timestamp) -> Result<usize, StoreError> {
        self.write(|tx| archive_claims(tx, "expires_at <= :now", &[], ReleaseReason::Expired, now))
            .map(|ids| ids.len())
    }

    /// Every unexpired local claim, oldest first: for `acs status` and for publishing the
    /// machine's claims during sync.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn live_claims(&self, now: Timestamp) -> Result<Vec<HeldClaim>, StoreError> {
        collect_held(
            &mut self.conn.prepare_cached(&format!(
                "{LOCAL_SELECT} WHERE c.expires_at > ?1 ORDER BY c.acquired_at, c.claim_id"
            ))?,
            params![to_db(now)],
            ClaimSource::Local,
        )
    }

    /// Every claim the seat has held, oldest first, for its session record: ended claims from
    /// the history, then claims still live (without `released_at`).
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn seat_claim_history(
        &self,
        seat_id: &SeatId,
    ) -> Result<Vec<ClaimHistoryEntry>, StoreError> {
        type Row = (
            String,
            String,
            Option<String>,
            i64,
            Option<i64>,
            Option<String>,
        );
        let mut stmt = self.conn.prepare_cached(
            "SELECT claim_id, path, symbol_path, acquired_at, released_at, release_reason \
               FROM claim_history WHERE seat_id = ?1 \
             UNION ALL \
             SELECT claim_id, path, symbol_path, acquired_at, NULL, NULL \
               FROM claims WHERE seat_id = ?1 \
             ORDER BY 4, 1",
        )?;
        let rows: Vec<Row> = stmt
            .query_map(params![seat_id.as_str()], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        rows.into_iter()
            .map(
                |(claim_id, path, symbol_path, acquired, released, reason)| {
                    Ok(ClaimHistoryEntry {
                        claim_id: id_from_db(claim_id)?,
                        scope: scope_from_db(path, symbol_path)?,
                        acquired_at: from_db(acquired)?,
                        released_at: released.map(from_db).transpose()?,
                        release_reason: reason.map(|r| enum_from_db(&r)).transpose()?,
                    })
                },
            )
            .collect()
    }

    /// Replaces the stored claims of one remote pilot (`pilot_id` as in
    /// [`acs_core::pilot_id`]) with `claims`, as fetched at `fetched_at`.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn replace_remote_claims(
        &self,
        pilot_id: &str,
        claims: &[Claim],
        fetched_at: Timestamp,
    ) -> Result<(), StoreError> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM remote_claims WHERE pilot_id = ?1",
                params![pilot_id],
            )?;
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO remote_claims (pilot_id, claim_id, seat_id, pilot, path, \
                 symbol_path, target_branch, acquired_at, heartbeat_at, ttl_secs, expires_at, \
                 origin, fetched_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            for claim in claims {
                let (path, symbol_path) = scope_to_db(&claim.scope);
                let heartbeat = to_db(claim.heartbeat_at);
                let ttl = ttl_for_db(claim.ttl_secs);
                insert.execute(params![
                    pilot_id,
                    claim.claim_id.as_str(),
                    claim.seat_id.as_str(),
                    claim.pilot,
                    path,
                    symbol_path,
                    claim.target_branch,
                    to_db(claim.acquired_at),
                    heartbeat,
                    ttl,
                    expiry_micros(heartbeat, ttl),
                    enum_to_db::<ClaimOrigin>(&claim.origin)?,
                    to_db(fetched_at),
                ])?;
            }
            Ok(())
        })
    }

    /// Forgets the claims of every remote pilot not in `pilot_ids` (their refs are gone). Returns
    /// how many claims were dropped.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn retain_remote_pilots(&self, pilot_ids: &[&str]) -> Result<usize, StoreError> {
        let keep = serde_json::to_string(pilot_ids)
            .map_err(|err| StoreError::invalid("pilot ids", err))?;
        self.write(|tx| {
            Ok(tx.execute(
                "DELETE FROM remote_claims WHERE pilot_id NOT IN (SELECT value FROM json_each(?1))",
                params![keep],
            )?)
        })
    }

    /// Records a successful fetch at `at`: the remote view is fresh until `stale_after` passes.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn record_sync_success(&self, at: Timestamp) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO remote_sync (id, last_success_at, last_attempt_at, last_error) \
             VALUES (1, ?1, ?1, NULL) ON CONFLICT (id) DO UPDATE SET \
             last_success_at = ?1, last_attempt_at = ?1, last_error = NULL",
            params![to_db(at)],
        )?;
        Ok(())
    }

    /// Records a failed sync attempt at `at`. The last success time is kept, so the view ages.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn record_sync_failure(&self, at: Timestamp, error: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO remote_sync (id, last_success_at, last_attempt_at, last_error) \
             VALUES (1, NULL, ?1, ?2) ON CONFLICT (id) DO UPDATE SET \
             last_attempt_at = ?1, last_error = ?2",
            params![to_db(at), error],
        )?;
        Ok(())
    }

    /// The state of the last synchronization.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn remote_sync_status(&self) -> Result<SyncStatus, StoreError> {
        let row: Option<(Option<i64>, Option<i64>, Option<String>)> = self
            .conn
            .prepare_cached(
                "SELECT last_success_at, last_attempt_at, last_error FROM remote_sync WHERE id = 1",
            )?
            .query_row([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()?;
        let Some((success, attempt, error)) = row else {
            return Ok(SyncStatus::default());
        };
        Ok(SyncStatus {
            last_success_at: success.map(from_db).transpose()?,
            last_attempt_at: attempt.map(from_db).transpose()?,
            last_error: error,
        })
    }
}
