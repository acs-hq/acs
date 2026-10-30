//! Repository flags: a small key/value table shared by every process of a repository.
//!
//! Used for `gate_off` (`acs off`); the value is free text, by convention `"1"` for a boolean
//! that is set. A flag that is not set has no row.

use acs_core::Timestamp;
use rusqlite::{OptionalExtension, params};

use crate::db::{StateDb, StoreError, to_db};

/// The flag `acs off` sets: the gate is disabled for this repository on this machine (sealed
/// paths still hold).
pub const FLAG_GATE_OFF: &str = "gate_off";

impl StateDb {
    /// Sets a flag, replacing its value.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn set_flag(&self, key: &str, value: &str, now: Timestamp) -> Result<(), StoreError> {
        self.conn
            .prepare_cached(
                "INSERT INTO repo_flags (key, value, updated_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value, \
                 updated_at = excluded.updated_at",
            )?
            .execute(params![key, value, to_db(now)])?;
        Ok(())
    }

    /// The value of a flag, or `None` if it is not set.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn flag(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .prepare_cached("SELECT value FROM repo_flags WHERE key = ?1")?
            .query_row(params![key], |row| row.get(0))
            .optional()?)
    }

    /// Clears a flag. Returns whether it was set.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn clear_flag(&self, key: &str) -> Result<bool, StoreError> {
        let changed = self
            .conn
            .prepare_cached("DELETE FROM repo_flags WHERE key = ?1")?
            .execute(params![key])?;
        Ok(changed > 0)
    }

    /// Whether a boolean flag is set (any value other than `"0"` counts).
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn flag_is_set(&self, key: &str) -> Result<bool, StoreError> {
        Ok(self.flag(key)?.is_some_and(|value| value != "0"))
    }
}
