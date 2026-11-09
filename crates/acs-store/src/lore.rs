//! Which lore entries each seat has been shown (WP6.1).
//!
//! The hook shows an entry the first time a seat works in its scope, then never again in that
//! session. [`StateDb::mark_lore_loaded`] is the only writer and decides that atomically, so two
//! hooks of one seat running at once never show an entry twice.

use std::collections::HashSet;

use acs_core::{LoreId, SeatId, Timestamp};
use rusqlite::params;

use crate::db::{StateDb, StoreError, from_db, to_db};

/// One entry shown to a seat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoreLoaded {
    /// The entry.
    pub lore_id: LoreId,
    /// When it was shown.
    pub loaded_at: Timestamp,
    /// Whether it was shown with a request to verify it (stale).
    pub stale: bool,
}

impl StateDb {
    /// The ids of the lore entries `seat` has been shown.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn lore_loaded_ids(&self, seat: &SeatId) -> Result<HashSet<String>, StoreError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT lore_id FROM lore_loaded WHERE seat_id = ?1")?;
        let rows = stmt.query_map(params![seat.as_str()], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records that `seat` is being shown `entries` (id and whether it is stale) and returns the
    /// ids it had not been shown before, in the order given. Only those may be shown: an id
    /// missing from the result was shown by another process of the same seat a moment ago.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn mark_lore_loaded(
        &self,
        seat: &SeatId,
        entries: &[(LoreId, bool)],
        now: Timestamp,
    ) -> Result<Vec<LoreId>, StoreError> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        self.write(|tx| {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO lore_loaded (seat_id, lore_id, loaded_at, stale) \
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut fresh = Vec::new();
            for (id, stale) in entries {
                let inserted =
                    stmt.execute(params![seat.as_str(), id.as_str(), to_db(now), *stale])?;
                if inserted > 0 {
                    fresh.push(id.clone());
                }
            }
            Ok(fresh)
        })
    }

    /// Every entry `seat` has been shown, oldest first (for session records and statistics).
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn lore_loaded_for_seat(&self, seat: &SeatId) -> Result<Vec<LoreLoaded>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT lore_id, loaded_at, stale FROM lore_loaded WHERE seat_id = ?1 \
             ORDER BY loaded_at, lore_id",
        )?;
        let rows = stmt.query_map(params![seat.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, at, stale) = row?;
            out.push(LoreLoaded {
                lore_id: id
                    .parse()
                    .map_err(|err| StoreError::invalid("lore id", err))?,
                loaded_at: from_db(at)?,
                stale,
            });
        }
        Ok(out)
    }
}
