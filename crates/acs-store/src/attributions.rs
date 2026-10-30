//! Post-tool attributions: which seat changed which path, with content hashes before and after.
//!
//! One row per path per post-tool event. Session records (`files_changed`, WP4.1) take the first
//! `pre_hash` and the last `post_hash` of each path of a seat; `acs blame` of uncommitted lines
//! (WP4.2) finds the seat that last changed a path. Structural data only: paths and hashes.

use acs_core::{ContentHash, RepoPath, SeatId, Timestamp};
use rusqlite::params;

use crate::db::{StateDb, StoreError, from_db, id_from_db, to_db};

/// How a changed path was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributionSource {
    /// The path was named by the tool event itself (a file tool).
    Tool,
    /// The path was found by the working-tree diff after a shell command.
    Shell,
}

impl AttributionSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Shell => "shell",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "tool" => Ok(Self::Tool),
            "shell" => Ok(Self::Shell),
            other => Err(StoreError::invalid("attribution source", other)),
        }
    }
}

/// One path changed by one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribution {
    /// The repository path.
    pub path: RepoPath,
    /// Hash before the change; `None` if the file did not exist.
    pub pre_hash: Option<ContentHash>,
    /// Hash after the change; `None` if the file no longer exists.
    pub post_hash: Option<ContentHash>,
}

/// An attribution as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAttribution {
    /// Row id; increases with insertion order.
    pub id: i64,
    /// The seat the change is attributed to.
    pub seat_id: SeatId,
    /// The repository path.
    pub path: RepoPath,
    /// Hash before the change; `None` if the file did not exist.
    pub pre_hash: Option<ContentHash>,
    /// Hash after the change; `None` if the file no longer exists.
    pub post_hash: Option<ContentHash>,
    /// When the post-tool event was handled.
    pub at: Timestamp,
    /// How the path was found.
    pub via: AttributionSource,
}

impl StateDb {
    /// Records the changes of one post-tool event in one transaction.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] if the seat does not exist (foreign key); any other
    /// [`StoreError`].
    pub fn record_attributions(
        &self,
        seat_id: &SeatId,
        changes: &[Attribution],
        via: AttributionSource,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        if changes.is_empty() {
            return Ok(());
        }
        self.write(|tx| {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO attributions (seat_id, path, pre_hash, post_hash, at, via) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for change in changes {
                stmt.execute(params![
                    seat_id.as_str(),
                    change.path.as_str(),
                    change.pre_hash.as_ref().map(ContentHash::as_str),
                    change.post_hash.as_ref().map(ContentHash::as_str),
                    to_db(now),
                    via.as_str(),
                ])?;
            }
            Ok(())
        })
    }

    /// Every attribution of the seat, oldest first.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn seat_attributions(
        &self,
        seat_id: &SeatId,
    ) -> Result<Vec<StoredAttribution>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, seat_id, path, pre_hash, post_hash, at, via FROM attributions \
             WHERE seat_id = ?1 ORDER BY id",
        )?;
        collect(&mut stmt, params![seat_id.as_str()])
    }

    /// Every attribution of a path, across seats, newest first: `acs blame` of uncommitted
    /// lines asks who changed it last.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn path_attributions(&self, path: &RepoPath) -> Result<Vec<StoredAttribution>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, seat_id, path, pre_hash, post_hash, at, via FROM attributions \
             WHERE path = ?1 ORDER BY id DESC",
        )?;
        collect(&mut stmt, params![path.as_str()])
    }
}

type RawRow = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    String,
);

fn collect(
    stmt: &mut rusqlite::CachedStatement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<StoredAttribution>, StoreError> {
    let rows: Vec<RawRow> = stmt
        .query_map(params, |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    rows.into_iter()
        .map(|(id, seat, path, pre, post, at, via)| {
            Ok(StoredAttribution {
                id,
                seat_id: id_from_db(seat)?,
                path: RepoPath::try_from(path).map_err(|e| StoreError::invalid("path", e))?,
                pre_hash: pre
                    .map(ContentHash::try_from)
                    .transpose()
                    .map_err(|e| StoreError::invalid("hash", e))?,
                post_hash: post
                    .map(ContentHash::try_from)
                    .transpose()
                    .map_err(|e| StoreError::invalid("hash", e))?,
                at: from_db(at)?,
                via: AttributionSource::parse(&via)?,
            })
        })
        .collect()
}
