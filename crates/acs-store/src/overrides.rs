//! Human overrides of `owned` territory (decision 0026): granting, revoking, the overrides in force
//! for a seat, and recording their use atomically.

use std::collections::BTreeMap;

use acs_core::{OverrideId, OverrideMode, OverrideUse, RepoPath, SeatId, Timestamp};
use rusqlite::{OptionalExtension, Row, params};

use crate::db::{
    StateDb, StoreError, enum_from_db, enum_to_db, from_db, id_from_db, new_id, to_db,
};

/// An override as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredOverride {
    /// Identifier.
    pub override_id: OverrideId,
    /// The glob it covers, repository-relative.
    pub pattern: String,
    /// How long it lasts.
    pub mode: OverrideMode,
    /// Who granted it (git email).
    pub granted_by: String,
    /// When it was granted.
    pub granted_at: Timestamp,
    /// When it lapses if no seat uses it.
    pub expires_at: Timestamp,
    /// The seat it is bound to, once a seat used it.
    pub seat_id: Option<SeatId>,
    /// When a `once` override was used up.
    pub consumed_at: Option<Timestamp>,
    /// When it was revoked.
    pub revoked_at: Option<Timestamp>,
}

/// What to grant.
#[derive(Debug, Clone, Copy)]
pub struct NewOverride<'a> {
    /// The glob, repository-relative.
    pub pattern: &'a str,
    /// How long it lasts.
    pub mode: OverrideMode,
    /// Who grants it (git email).
    pub granted_by: &'a str,
    /// When.
    pub now: Timestamp,
    /// When it lapses if no seat uses it.
    pub expires_at: Timestamp,
}

const COLUMNS: &str = "override_id, pattern, mode, granted_by, granted_at, expires_at, seat_id, \
                       consumed_at, revoked_at";

fn read_override(row: &Row<'_>) -> rusqlite::Result<RawOverride> {
    Ok(RawOverride {
        override_id: row.get(0)?,
        pattern: row.get(1)?,
        mode: row.get(2)?,
        granted_by: row.get(3)?,
        granted_at: row.get(4)?,
        expires_at: row.get(5)?,
        seat_id: row.get(6)?,
        consumed_at: row.get(7)?,
        revoked_at: row.get(8)?,
    })
}

struct RawOverride {
    override_id: String,
    pattern: String,
    mode: String,
    granted_by: String,
    granted_at: i64,
    expires_at: i64,
    seat_id: Option<String>,
    consumed_at: Option<i64>,
    revoked_at: Option<i64>,
}

impl RawOverride {
    fn parse(self) -> Result<StoredOverride, StoreError> {
        Ok(StoredOverride {
            override_id: id_from_db(self.override_id)?,
            pattern: self.pattern,
            mode: enum_from_db(&self.mode)?,
            granted_by: self.granted_by,
            granted_at: from_db(self.granted_at)?,
            expires_at: from_db(self.expires_at)?,
            seat_id: self.seat_id.map(id_from_db).transpose()?,
            consumed_at: self.consumed_at.map(from_db).transpose()?,
            revoked_at: self.revoked_at.map(from_db).transpose()?,
        })
    }
}

impl StateDb {
    /// Grants an override and returns its id.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn grant_override(&self, new: &NewOverride<'_>) -> Result<OverrideId, StoreError> {
        let id: OverrideId = new_id(OverrideId::PREFIX)?;
        self.conn
            .prepare_cached(
                "INSERT INTO overrides (override_id, pattern, mode, granted_by, granted_at, \
                 expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                id.as_str(),
                new.pattern,
                enum_to_db(&new.mode)?,
                new.granted_by,
                to_db(new.now),
                to_db(new.expires_at)
            ])?;
        Ok(id)
    }

    /// Revokes an override that is still in force. Returns whether it was.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn revoke_override(&self, id: &OverrideId, now: Timestamp) -> Result<bool, StoreError> {
        let changed = self
            .conn
            .prepare_cached(
                "UPDATE overrides SET revoked_at = ?2 \
                 WHERE override_id = ?1 AND revoked_at IS NULL AND consumed_at IS NULL",
            )?
            .execute(params![id.as_str(), to_db(now)])?;
        Ok(changed > 0)
    }

    /// One override by id.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn override_by_id(&self, id: &OverrideId) -> Result<Option<StoredOverride>, StoreError> {
        self.conn
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM overrides WHERE override_id = ?1"
            ))?
            .query_row(params![id.as_str()], read_override)
            .optional()?
            .map(RawOverride::parse)
            .transpose()
    }

    /// The overrides `seat` may use at `now`: bound to it, or unbound and not lapsed; never
    /// revoked or used up. Oldest first.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn overrides_for_seat(
        &self,
        seat: &SeatId,
        now: Timestamp,
    ) -> Result<Vec<StoredOverride>, StoreError> {
        self.query_overrides(
            "WHERE revoked_at IS NULL AND consumed_at IS NULL \
             AND (seat_id = ?1 OR (seat_id IS NULL AND expires_at > ?2)) \
             ORDER BY granted_at, override_id",
            params![seat.as_str(), to_db(now)],
        )
    }

    /// Every override still in force at `now`, for `acs status`: unbound and not lapsed, or
    /// bound to a seat that has not ended. Oldest first.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn active_overrides(&self, now: Timestamp) -> Result<Vec<StoredOverride>, StoreError> {
        self.query_overrides(
            "WHERE revoked_at IS NULL AND consumed_at IS NULL \
             AND ((seat_id IS NULL AND expires_at > ?1) \
                  OR seat_id IN (SELECT seat_id FROM seats WHERE ended_at IS NULL)) \
             ORDER BY granted_at, override_id",
            params![to_db(now)],
        )
    }

    fn query_overrides(
        &self,
        tail: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<StoredOverride>, StoreError> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT {COLUMNS} FROM overrides {tail}"))?;
        let rows = stmt.query_map(args, read_override)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.parse()?);
        }
        Ok(out)
    }

    /// Records that `seat` relied on overrides for these paths, in one transaction: a `once`
    /// override is used up, a `session` override is bound to the seat, and every path is
    /// recorded. Returns `false`, changing nothing, if any override is no longer the seat's to
    /// use (another seat took it, it was revoked or it lapsed); the caller decides again.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn apply_overrides(
        &self,
        seat: &SeatId,
        uses: &[(OverrideId, RepoPath)],
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        if uses.is_empty() {
            return Ok(true);
        }
        // Dropping the transaction without committing rolls it back.
        let tx = self.write_tx()?;
        let mut ids: Vec<&OverrideId> = uses.iter().map(|(id, _)| id).collect();
        ids.sort();
        ids.dedup();
        for id in ids {
            let changed = tx
                .prepare_cached(
                    "UPDATE overrides \
                     SET seat_id = ?2, \
                         consumed_at = CASE mode WHEN 'once' THEN ?3 ELSE NULL END \
                     WHERE override_id = ?1 AND revoked_at IS NULL AND consumed_at IS NULL \
                       AND (seat_id = ?2 OR (seat_id IS NULL AND expires_at > ?3))",
                )?
                .execute(params![id.as_str(), seat.as_str(), to_db(now)])?;
            if changed == 0 {
                return Ok(false);
            }
        }
        for (id, path) in uses {
            tx.prepare_cached(
                "INSERT INTO override_uses (override_id, seat_id, path, at) \
                 VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![
                id.as_str(),
                seat.as_str(),
                path.as_str(),
                to_db(now)
            ])?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// The overrides `seat` used, each with the paths it let through, in order of first use. The
    /// `overrides` part of the seat's session record.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn override_uses_for_seat(&self, seat: &SeatId) -> Result<Vec<OverrideUse>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT u.override_id, u.path, o.pattern, o.mode, o.granted_by, o.granted_at \
             FROM override_uses u JOIN overrides o ON o.override_id = u.override_id \
             WHERE u.seat_id = ?1 ORDER BY u.at, u.rowid",
        )?;
        let rows = stmt.query_map(params![seat.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;
        let mut order: Vec<OverrideId> = Vec::new();
        let mut by_id: BTreeMap<OverrideId, OverrideUse> = BTreeMap::new();
        for row in rows {
            let (id, path, pattern, mode, granted_by, granted_at) = row?;
            let id: OverrideId = id_from_db(id)?;
            let path: RepoPath = path
                .parse()
                .map_err(|err| StoreError::invalid("path", err))?;
            let entry = match by_id.get_mut(&id) {
                Some(entry) => entry,
                None => {
                    order.push(id.clone());
                    by_id.entry(id.clone()).or_insert(OverrideUse {
                        override_id: id,
                        pattern,
                        mode: enum_from_db(&mode)?,
                        granted_by,
                        granted_at: from_db(granted_at)?,
                        paths: Vec::new(),
                    })
                }
            };
            if !entry.paths.contains(&path) {
                entry.paths.push(path);
            }
        }
        Ok(order
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .collect())
    }
}
