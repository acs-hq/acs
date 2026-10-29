//! Seats: one row per harness session.
//!
//! A seat is created on the first event of a session and keyed by `seat_id`; the pair
//! `(harness, harness_session_id)` is unique, so every `acs` process that sees the same session
//! lands on the same seat.

use acs_core::{ClaimId, ReleaseReason, Seat, SeatId, Timestamp};
use rusqlite::{OptionalExtension, Row, params};

use crate::claims::archive_claims;
use crate::db::{StateDb, StoreError, from_db, id_from_db, new_id, to_db};

/// What is known about a session when its first event arrives.
#[derive(Debug, Clone, Copy)]
pub struct NewSeat<'a> {
    /// Name of the harness, for example `claude-code`.
    pub harness: &'a str,
    /// The harness's own session identifier.
    pub harness_session_id: &'a str,
    /// The pilot's git email.
    pub pilot: &'a str,
    /// Absolute path of the worktree the session works in.
    pub worktree: &'a str,
    /// The checked-out branch; `None` when HEAD is detached.
    pub branch: Option<&'a str>,
    /// The branch the work will be integrated into.
    pub target_branch: &'a str,
}

const SEAT_COLUMNS: &str = "seat_id, pilot, harness, harness_session_id, worktree, branch, \
                            target_branch, started_at, last_seen_at, ended_at";

/// A seat row before its ids and timestamps are validated.
struct RawSeat {
    seat_id: String,
    pilot: String,
    harness: String,
    harness_session_id: String,
    worktree: String,
    branch: Option<String>,
    target_branch: String,
    started_at: i64,
    last_seen_at: i64,
    ended_at: Option<i64>,
}

impl RawSeat {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            seat_id: row.get(0)?,
            pilot: row.get(1)?,
            harness: row.get(2)?,
            harness_session_id: row.get(3)?,
            worktree: row.get(4)?,
            branch: row.get(5)?,
            target_branch: row.get(6)?,
            started_at: row.get(7)?,
            last_seen_at: row.get(8)?,
            ended_at: row.get(9)?,
        })
    }

    fn into_seat(self) -> Result<(Seat, Option<Timestamp>), StoreError> {
        let ended_at = self.ended_at.map(from_db).transpose()?;
        let seat = Seat {
            seat_id: id_from_db(self.seat_id)?,
            pilot: self.pilot,
            harness: self.harness,
            harness_session_id: self.harness_session_id,
            worktree: self.worktree,
            branch: self.branch,
            target_branch: self.target_branch,
            started_at: from_db(self.started_at)?,
            last_seen_at: from_db(self.last_seen_at)?,
        };
        Ok((seat, ended_at))
    }
}

impl StateDb {
    /// Returns the seat of a harness session, creating it on the session's first event.
    ///
    /// Safe to call from concurrent processes: exactly one seat exists per
    /// `(harness, harness_session_id)`. An existing seat is returned as stored (its pilot,
    /// worktree and branches are those of the first event). A seat that had ended (`end_seat`)
    /// is revived, because the harness resumed the session or a late event arrived.
    ///
    /// The common case, an existing live seat, is a read and does not take the write lock.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn get_or_create_seat(
        &self,
        new: &NewSeat<'_>,
        now: Timestamp,
    ) -> Result<Seat, StoreError> {
        if let Some((seat, None)) = self.find_seat(new.harness, new.harness_session_id)? {
            return Ok(seat);
        }
        self.write(|tx| {
            let found = tx
                .query_row(
                    &format!(
                        "SELECT {SEAT_COLUMNS} FROM seats WHERE harness = ?1 AND harness_session_id = ?2"
                    ),
                    params![new.harness, new.harness_session_id],
                    RawSeat::from_row,
                )
                .optional()?;
            if let Some(raw) = found {
                let (seat, ended_at) = raw.into_seat()?;
                if ended_at.is_some() {
                    tx.execute(
                        "UPDATE seats SET ended_at = NULL, last_seen_at = max(last_seen_at, ?2) \
                         WHERE seat_id = ?1",
                        params![seat.seat_id.as_str(), to_db(now)],
                    )?;
                }
                return Ok(seat);
            }
            let seat_id: SeatId = new_id(SeatId::PREFIX)?;
            tx.execute(
                "INSERT INTO seats (seat_id, pilot, harness, harness_session_id, worktree, branch, \
                 target_branch, started_at, last_seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    seat_id.as_str(),
                    new.pilot,
                    new.harness,
                    new.harness_session_id,
                    new.worktree,
                    new.branch,
                    new.target_branch,
                    to_db(now),
                ],
            )?;
            Ok(Seat {
                seat_id,
                pilot: new.pilot.to_owned(),
                harness: new.harness.to_owned(),
                harness_session_id: new.harness_session_id.to_owned(),
                worktree: new.worktree.to_owned(),
                branch: new.branch.map(str::to_owned),
                target_branch: new.target_branch.to_owned(),
                started_at: now,
                last_seen_at: now,
            })
        })
    }

    /// The seat with this id, if it exists.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn get_seat(&self, seat_id: &SeatId) -> Result<Option<Seat>, StoreError> {
        let raw = self
            .conn
            .prepare_cached(&format!(
                "SELECT {SEAT_COLUMNS} FROM seats WHERE seat_id = ?1"
            ))?
            .query_row(params![seat_id.as_str()], RawSeat::from_row)
            .optional()?;
        raw.map(|raw| raw.into_seat().map(|(seat, _)| seat))
            .transpose()
    }

    /// When the seat ended (`end_seat`), or `None` while it is live or if it does not exist. A
    /// seat that is not live but has no end time crashed (SPEC section 4.3).
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn seat_ended_at(&self, seat_id: &SeatId) -> Result<Option<Timestamp>, StoreError> {
        let micros: Option<Option<i64>> = self
            .conn
            .prepare_cached("SELECT ended_at FROM seats WHERE seat_id = ?1")?
            .query_row(params![seat_id.as_str()], |row| row.get(0))
            .optional()?;
        micros.flatten().map(from_db).transpose()
    }

    /// Records that the seat produced an event: sets `last_seen_at` to `now` (never moving it
    /// backwards).
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownSeat`] if there is no such seat; any other [`StoreError`].
    pub fn touch_seat(&self, seat_id: &SeatId, now: Timestamp) -> Result<(), StoreError> {
        let changed = self
            .conn
            .prepare_cached(
                "UPDATE seats SET last_seen_at = max(last_seen_at, ?2) WHERE seat_id = ?1",
            )?
            .execute(params![seat_id.as_str(), to_db(now)])?;
        if changed == 0 {
            return Err(StoreError::UnknownSeat(seat_id.to_string()));
        }
        Ok(())
    }

    /// Ends the seat at `now` and releases all its claims with reason `session_end` (claims that
    /// had already run out are recorded as `expired`). Returns the claims that left the live
    /// table. Calling it again is harmless.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownSeat`] if there is no such seat; any other [`StoreError`].
    pub fn end_seat(&self, seat_id: &SeatId, now: Timestamp) -> Result<Vec<ClaimId>, StoreError> {
        self.write(|tx| {
            let changed = tx.execute(
                "UPDATE seats SET ended_at = ?2, last_seen_at = max(last_seen_at, ?2) WHERE seat_id = ?1",
                params![seat_id.as_str(), to_db(now)],
            )?;
            if changed == 0 {
                return Err(StoreError::UnknownSeat(seat_id.to_string()));
            }
            archive_claims(
                tx,
                "seat_id = :seat",
                &[(":seat", &seat_id.as_str())],
                ReleaseReason::SessionEnd,
                now,
            )
        })
    }

    fn find_seat(
        &self,
        harness: &str,
        harness_session_id: &str,
    ) -> Result<Option<(Seat, Option<Timestamp>)>, StoreError> {
        let raw = self
            .conn
            .prepare_cached(&format!(
                "SELECT {SEAT_COLUMNS} FROM seats WHERE harness = ?1 AND harness_session_id = ?2"
            ))?
            .query_row(params![harness, harness_session_id], RawSeat::from_row)
            .optional()?;
        raw.map(RawSeat::into_seat).transpose()
    }
}
