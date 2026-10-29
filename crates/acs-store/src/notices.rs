//! Notices: one-way system messages to a seat, delivered once in its next hook context.

use acs_core::{Notice, NoticeId, NoticeKind, SeatId, Timestamp};
use rusqlite::params;

use crate::db::{
    StateDb, StoreError, enum_from_db, enum_to_db, from_db, id_from_db, new_id, to_db,
};

impl StateDb {
    /// Creates an undelivered notice for `target_seat` and returns its id.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] if the seat does not exist (foreign key); any other
    /// [`StoreError`].
    pub fn insert_notice(
        &self,
        kind: NoticeKind,
        target_seat: &SeatId,
        body: &str,
        now: Timestamp,
    ) -> Result<NoticeId, StoreError> {
        let id: NoticeId = new_id(NoticeId::PREFIX)?;
        self.conn
            .prepare_cached(
                "INSERT INTO notices (notice_id, kind, target_seat, body, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![
                id.as_str(),
                enum_to_db(&kind)?,
                target_seat.as_str(),
                body,
                to_db(now)
            ])?;
        Ok(id)
    }

    /// The seat's undelivered notices, oldest first.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn pending_notices(&self, seat_id: &SeatId) -> Result<Vec<Notice>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT notice_id, kind, body, created_at FROM notices \
             WHERE target_seat = ?1 AND delivered_at IS NULL ORDER BY created_at, notice_id",
        )?;
        let rows: Vec<(String, String, String, i64)> = stmt
            .query_map(params![seat_id.as_str()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<_, _>>()?;
        rows.into_iter()
            .map(|(id, kind, body, created)| {
                Ok(Notice {
                    notice_id: id_from_db(id)?,
                    kind: enum_from_db::<NoticeKind>(&kind)?,
                    target_seat: seat_id.clone(),
                    body,
                    created_at: from_db(created)?,
                    delivered_at: None,
                })
            })
            .collect()
    }

    /// Marks notices delivered at `now`. Notices already delivered keep their first delivery
    /// time. Returns how many were newly marked, which a caller racing another hook of the same
    /// seat can use to tell who delivered.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn mark_delivered(&self, ids: &[NoticeId], now: Timestamp) -> Result<usize, StoreError> {
        let ids = serde_json::to_string(&ids.iter().map(NoticeId::as_str).collect::<Vec<_>>())
            .map_err(|err| StoreError::invalid("notice ids", err))?;
        self.write(|tx| {
            Ok(tx.execute(
                "UPDATE notices SET delivered_at = ?2 WHERE delivered_at IS NULL \
                 AND notice_id IN (SELECT value FROM json_each(?1))",
                params![ids, to_db(now)],
            )?)
        })
    }
}
