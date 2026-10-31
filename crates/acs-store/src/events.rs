//! The per-tool-call event log and the stats counters.
//!
//! Events are structural only (SPEC section 11): seat, phase, tool, paths, decision and times;
//! never prompts or file contents. They feed `acs log` and the guard-decision summary of session
//! records.

use std::collections::BTreeMap;

use acs_core::{Outcome, Phase, ReasonCode, SeatId, Timestamp, ToolKind};
use rusqlite::{OptionalExtension, params};

use crate::db::{StateDb, StoreError, enum_from_db, enum_to_db, from_db, id_from_db, to_db};

/// Longest `detail` stored, in characters. Longer text is cut.
pub const MAX_DETAIL_CHARS: usize = 512;

/// One event to record.
#[derive(Debug, Clone, Copy)]
pub struct NewEvent<'a> {
    /// The seat the event belongs to.
    pub seat_id: &'a SeatId,
    /// When it happened.
    pub at: Timestamp,
    /// Where in the session it was raised.
    pub phase: Phase,
    /// The harness's name for the tool, for tool events.
    pub tool_name: Option<&'a str>,
    /// What kind of tool it was, for tool events.
    pub tool_kind: Option<ToolKind>,
    /// The paths the event touched, as resolved (repository-relative where possible).
    pub paths: &'a [String],
    /// Why the gate decided as it did, for `pre_tool` events.
    pub reason_code: Option<ReasonCode>,
    /// What the gate decided, for `pre_tool` events.
    pub outcome: Option<Outcome>,
    /// A short note: for a collision what the seat was told, for another notable denial the command
    /// line. The caller MUST redact secrets (records sync to the remote); the store cuts it to
    /// [`MAX_DETAIL_CHARS`].
    pub detail: Option<&'a str>,
}

/// An event as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    /// Row id; increases with insertion order.
    pub id: i64,
    /// The seat.
    pub seat_id: SeatId,
    /// When it happened.
    pub at: Timestamp,
    /// Where in the session it was raised.
    pub phase: Phase,
    /// The harness's name for the tool.
    pub tool_name: Option<String>,
    /// What kind of tool it was.
    pub tool_kind: Option<ToolKind>,
    /// The paths touched.
    pub paths: Vec<String>,
    /// Why the gate decided as it did.
    pub reason_code: Option<ReasonCode>,
    /// What the gate decided.
    pub outcome: Option<Outcome>,
    /// The note, if any.
    pub detail: Option<String>,
}

/// A call refused because another seat held a file, as read back with the refused seat's details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    /// When it happened.
    pub at: Timestamp,
    /// The seat that was refused.
    pub seat_id: SeatId,
    /// That seat's pilot.
    pub pilot: String,
    /// That seat's harness.
    pub harness: String,
    /// The tool that was refused.
    pub tool_name: Option<String>,
    /// The paths of the refused call.
    pub paths: Vec<String>,
    /// What the seat was told, which names the holder.
    pub detail: Option<String>,
}

/// A `Collision` row before its ids and timestamps are validated.
type RawCollision = (
    i64,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
);

/// An `events` row before its enums, ids and timestamps are validated.
type RawEvent = (
    i64,
    String,
    i64,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn event_from_raw(raw: RawEvent) -> Result<StoredEvent, StoreError> {
    let (id, seat, at, phase, tool_name, kind, paths, code, outcome, detail) = raw;
    Ok(StoredEvent {
        id,
        seat_id: id_from_db(seat)?,
        at: from_db(at)?,
        phase: enum_from_db(&phase)?,
        tool_name,
        tool_kind: kind.map(|k| enum_from_db(&k)).transpose()?,
        paths: serde_json::from_str(&paths)
            .map_err(|err| StoreError::invalid("event paths", err))?,
        reason_code: code.map(|c| enum_from_db(&c)).transpose()?,
        outcome: outcome.map(|o| enum_from_db(&o)).transpose()?,
        detail,
    })
}

impl StateDb {
    /// Appends an event to the log and returns its row id.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] if the seat does not exist (foreign key); any other
    /// [`StoreError`].
    pub fn record_event(&self, event: &NewEvent<'_>) -> Result<i64, StoreError> {
        let paths = serde_json::to_string(event.paths)
            .map_err(|err| StoreError::invalid("event paths", err))?;
        let detail: Option<String> = event
            .detail
            .map(|text| text.chars().take(MAX_DETAIL_CHARS).collect());
        self.conn
            .prepare_cached(
                "INSERT INTO events (seat_id, at, phase, tool_name, tool_kind, paths, \
                 reason_code, outcome, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?
            .execute(params![
                event.seat_id.as_str(),
                to_db(event.at),
                enum_to_db(&event.phase)?,
                event.tool_name,
                event.tool_kind.as_ref().map(enum_to_db).transpose()?,
                paths,
                event.reason_code.as_ref().map(enum_to_db).transpose()?,
                event.outcome.as_ref().map(enum_to_db).transpose()?,
                detail,
            ])?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The events of a seat, oldest first.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn seat_events(&self, seat_id: &SeatId) -> Result<Vec<StoredEvent>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, seat_id, at, phase, tool_name, tool_kind, paths, reason_code, outcome, \
             detail FROM events WHERE seat_id = ?1 ORDER BY id",
        )?;
        let rows: Vec<RawEvent> = stmt
            .query_map(params![seat_id.as_str()], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        rows.into_iter().map(event_from_raw).collect()
    }

    /// How many times each reason code was recorded for the seat (codes never recorded are
    /// omitted): the `counts` of a session record's guard decisions.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn reason_counts(&self, seat_id: &SeatId) -> Result<BTreeMap<ReasonCode, u64>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT reason_code, count(*) FROM events \
             WHERE seat_id = ?1 AND reason_code IS NOT NULL GROUP BY reason_code",
        )?;
        let rows: Vec<(String, i64)> = stmt
            .query_map(params![seat_id.as_str()], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        rows.into_iter()
            .map(|(code, count)| Ok((enum_from_db(&code)?, u64::try_from(count).unwrap_or(0))))
            .collect()
    }

    /// The collisions since `since`, newest first, at most `limit` of them, and how many there were
    /// in all.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn collisions_since(
        &self,
        since: Timestamp,
        limit: usize,
    ) -> Result<(Vec<Collision>, u64), StoreError> {
        let total: i64 = self
            .conn
            .prepare_cached(
                "SELECT count(*) FROM events WHERE reason_code = 'claim_conflict' AND at >= ?1",
            )?
            .query_row(params![to_db(since)], |row| row.get(0))?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT e.at, e.seat_id, s.pilot, s.harness, e.tool_name, e.paths, e.detail \
             FROM events e JOIN seats s ON s.seat_id = e.seat_id \
             WHERE e.reason_code = 'claim_conflict' AND e.at >= ?1 \
             ORDER BY e.at DESC, e.id DESC LIMIT ?2",
        )?;
        let rows: Vec<RawCollision> = stmt
            .query_map(
                params![to_db(since), i64::try_from(limit).unwrap_or(i64::MAX)],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )?
            .collect::<Result<_, _>>()?;
        let collisions = rows
            .into_iter()
            .map(|(at, seat, pilot, harness, tool_name, paths, detail)| {
                Ok(Collision {
                    at: from_db(at)?,
                    seat_id: id_from_db(seat)?,
                    pilot,
                    harness,
                    tool_name,
                    paths: serde_json::from_str(&paths)
                        .map_err(|err| StoreError::invalid("event paths", err))?,
                    detail,
                })
            })
            .collect::<Result<_, StoreError>>()?;
        Ok((collisions, u64::try_from(total).unwrap_or(0)))
    }

    /// Deletes events older than `before`. Returns how many were deleted. The event log is a
    /// diagnostic aid, so `acs gc` can trim it.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn prune_events(&self, before: Timestamp) -> Result<usize, StoreError> {
        self.write(|tx| Ok(tx.execute("DELETE FROM events WHERE at < ?1", params![to_db(before)])?))
    }

    /// Adds `n` to the named counter, creating it at zero first. Counters never go down.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn increment(&self, counter: &str, n: u64) -> Result<(), StoreError> {
        self.conn
            .prepare_cached(
                "INSERT INTO stats (counter, value) VALUES (?1, ?2) \
                 ON CONFLICT (counter) DO UPDATE SET value = CASE \
                 WHEN value > 9223372036854775807 - ?2 THEN 9223372036854775807 \
                 ELSE value + ?2 END",
            )?
            .execute(params![counter, i64::try_from(n).unwrap_or(i64::MAX)])?;
        Ok(())
    }

    /// The value of a counter; zero if it was never incremented.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn counter(&self, counter: &str) -> Result<u64, StoreError> {
        let value: Option<i64> = self
            .conn
            .prepare_cached("SELECT value FROM stats WHERE counter = ?1")?
            .query_row(params![counter], |row| row.get(0))
            .optional()?;
        Ok(value.map_or(0, |v| u64::try_from(v).unwrap_or(0)))
    }

    /// All counters by name.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn counters(&self) -> Result<BTreeMap<String, u64>, StoreError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT counter, value FROM stats")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(name, value)| (name, u64::try_from(value).unwrap_or(0)))
            .collect())
    }
}
