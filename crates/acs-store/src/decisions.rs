//! The last decision of each seat, for the MCP `acs_why` tool and `acs status`.

use acs_core::{Outcome, ReasonCode, SeatId, Timestamp};
use rusqlite::{OptionalExtension, params};

use crate::db::{StateDb, StoreError, enum_from_db, enum_to_db, from_db, to_db};

/// Longest message kept, in characters. Messages are short guidance; this only bounds a runaway.
pub const MAX_MESSAGE_CHARS: usize = 4096;

/// A seat's most recent pre-tool decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastDecision {
    /// What the gate decided.
    pub outcome: Outcome,
    /// Why.
    pub reason_code: ReasonCode,
    /// The guidance that was shown to the agent (empty when nothing was blocked).
    pub message: String,
    /// When it was decided.
    pub at: Timestamp,
}

impl StateDb {
    /// Records `decision` as the seat's last decision, replacing the previous one.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] if the seat does not exist (foreign key); any other
    /// [`StoreError`].
    pub fn set_last_decision(
        &self,
        seat_id: &SeatId,
        decision: &LastDecision,
    ) -> Result<(), StoreError> {
        let message: String = decision.message.chars().take(MAX_MESSAGE_CHARS).collect();
        self.conn
            .prepare_cached(
                "INSERT INTO last_decisions (seat_id, outcome, reason_code, message, at) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT (seat_id) DO UPDATE SET outcome = excluded.outcome, \
                 reason_code = excluded.reason_code, message = excluded.message, at = excluded.at",
            )?
            .execute(params![
                seat_id.as_str(),
                enum_to_db(&decision.outcome)?,
                enum_to_db(&decision.reason_code)?,
                message,
                to_db(decision.at),
            ])?;
        Ok(())
    }

    /// The seat's last decision, or `None` if it has not had a pre-tool event yet.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn last_decision(&self, seat_id: &SeatId) -> Result<Option<LastDecision>, StoreError> {
        let row: Option<(String, String, String, i64)> = self
            .conn
            .prepare_cached(
                "SELECT outcome, reason_code, message, at FROM last_decisions WHERE seat_id = ?1",
            )?
            .query_row(params![seat_id.as_str()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()?;
        row.map(|(outcome, code, message, at)| {
            Ok(LastDecision {
                outcome: enum_from_db(&outcome)?,
                reason_code: enum_from_db(&code)?,
                message,
                at: from_db(at)?,
            })
        })
        .transpose()
    }
}
