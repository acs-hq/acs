//! Snapshot store: content-addressed pre- and post-images of files, indexed by session.
//!
//! Layout under [`RepoHome::snapshots_dir`] (decision record 0010):
//!
//! ```text
//! snapshots/
//!   index.db                SQLite index (WAL), its own file, separate from state.db
//!   blobs/<2 hex>/<62 hex>  zstd-compressed content, named by the SHA-256 of the *uncompressed* bytes
//! ```
//!
//! The write order makes crashes harmless: a blob is written (atomically) before the index rows
//! that name it, and [`SnapshotStore::gc`] deletes index rows before the blob files. Leftovers
//! (blob files without a row, temporary files) are swept by the next `gc`.
//!
//! Evicted blobs keep their entries: an entry whose blob is gone is reported as
//! [`SnapshotError::Evicted`], never as an obscure I/O failure, so undo can say "snapshot evicted".

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use acs_core::{ContentHash, RepoPath, SessionId, Timestamp};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::fsutil::{create_private_dir_all, private_file_options, write_private_atomic};
use crate::home::RepoHome;

/// zstd level for blobs: low, because capture runs in the pre-tool hook.
const ZSTD_LEVEL: i32 = 1;

/// Index schema migrations; `PRAGMA user_version` is the number applied.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE blobs (
        hash        TEXT PRIMARY KEY,
        size        INTEGER NOT NULL,  -- uncompressed bytes
        stored      INTEGER NOT NULL,  -- compressed bytes on disk
        last_access INTEGER NOT NULL   -- unix milliseconds
    ) WITHOUT ROWID;
    CREATE TABLE entries (
        id       INTEGER PRIMARY KEY AUTOINCREMENT,  -- the capture sequence
        session  TEXT NOT NULL,
        path     TEXT NOT NULL,
        kind     INTEGER NOT NULL,  -- 0 pre, 1 post
        taken_at INTEGER NOT NULL,  -- unix milliseconds
        hash     TEXT,              -- NULL: the file did not exist
        size     INTEGER NOT NULL,
        mode     INTEGER NOT NULL
    );
    CREATE INDEX entries_by_path ON entries (session, path, kind, id);
    CREATE INDEX entries_by_hash ON entries (hash);
"];

const SELECT_ENTRY: &str =
    "SELECT id, session, path, kind, taken_at, hash, size, mode FROM entries";

/// Errors from the snapshot store.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// A filesystem operation failed.
    #[error("cannot {action} {path}: {source}")]
    Io {
        /// What was being done.
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The SQLite index failed.
    #[error("snapshot index error: {0}")]
    Index(#[from] rusqlite::Error),
    /// The index was written by a newer ACS.
    #[error(
        "snapshot index has schema version {found}, this ACS supports up to {supported}; upgrade ACS"
    )]
    UnsupportedSchema {
        /// Version found in the file.
        found: u32,
        /// Newest version this build understands.
        supported: u32,
    },
    /// An index row holds a value that cannot be a snapshot entry.
    #[error("snapshot index holds an invalid value: {0}")]
    InvalidIndex(String),
    /// A path handed to capture is a directory; use [`SnapshotStore::capture_tree`].
    #[error("{0} is a directory; capture its files with capture_tree")]
    IsDirectory(PathBuf),
    /// A file found while walking a tree has no valid repository path.
    #[error("cannot snapshot {0}: its name is not a valid repository path")]
    InvalidPath(PathBuf),
    /// The blob was removed by eviction; the snapshot is no longer available.
    #[error("snapshot evicted: content {hash} was removed to stay within the snapshot size budget")]
    Evicted {
        /// Hash of the missing content.
        hash: ContentHash,
    },
    /// The index lists the blob but its file is gone (deleted outside ACS).
    #[error("snapshot blob {hash} is missing from disk")]
    MissingBlob {
        /// Hash of the missing content.
        hash: ContentHash,
    },
    /// The blob does not decompress or does not match its hash.
    #[error("snapshot blob {hash} is corrupt: {reason}")]
    Corrupt {
        /// Hash of the damaged content.
        hash: ContentHash,
        /// What failed.
        reason: String,
    },
}

impl SnapshotError {
    fn io(action: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.to_owned(),
            source,
        }
    }
}

/// Whether an image was taken before or after the tool ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapshotKind {
    /// Taken before the change.
    Pre,
    /// Taken after the change.
    Post,
}

impl SnapshotKind {
    fn code(self) -> i64 {
        match self {
            Self::Pre => 0,
            Self::Post => 1,
        }
    }
}

/// What a file looked like when it was captured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    /// The file did not exist (or was deleted).
    Absent,
    /// The file existed.
    File {
        /// SHA-256 of the content.
        hash: ContentHash,
        /// Content length in bytes.
        size: u64,
        /// Unix permission bits (`0o7777` mask). On Windows only the read-only flag is
        /// meaningful: `0o444` is read-only, `0o644` is writable.
        mode: u32,
    },
}

/// One captured image of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEntry {
    /// Capture sequence number; increases with every capture.
    pub seq: i64,
    /// The session the image belongs to.
    pub session: SessionId,
    /// The file.
    pub path: RepoPath,
    /// Pre- or post-image.
    pub kind: SnapshotKind,
    /// When it was captured.
    pub taken_at: Timestamp,
    /// The captured state.
    pub state: FileState,
}

/// The outcome of capturing one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureResult {
    /// The recorded entry.
    pub entry: SnapshotEntry,
    /// Whether a new blob was written (false: absent file or content already stored).
    pub stored_new_blob: bool,
}

/// What [`SnapshotStore::gc`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Blobs no entry refers to any more, removed regardless of the budget.
    pub unreferenced_blobs: usize,
    /// Blobs evicted (least recently used first) to get under the budget.
    pub evicted_blobs: usize,
    /// Compressed bytes freed by the two kinds of removal above.
    pub freed_bytes: u64,
    /// Blob files with no index row, and leftover temporary files, that were swept.
    pub orphan_files: usize,
    /// Compressed bytes still stored.
    pub remaining_bytes: u64,
    /// Compressed bytes pinned by live sessions.
    pub live_bytes: u64,
    /// Whether the store is still over budget because live sessions pin too much.
    pub over_budget: bool,
}

/// The snapshot store of one repository.
pub struct SnapshotStore {
    conn: Connection,
    dir: PathBuf,
}

impl std::fmt::Debug for SnapshotStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotStore")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl SnapshotStore {
    /// Opens (creating and migrating if needed) the store under `home.snapshots_dir()`.
    ///
    /// # Errors
    ///
    /// [`SnapshotError::Io`], [`SnapshotError::Index`], or [`SnapshotError::UnsupportedSchema`]
    /// when the index is newer than this build.
    pub fn open(home: &RepoHome) -> Result<Self, SnapshotError> {
        Self::open_dir(home.snapshots_dir())
    }

    fn open_dir(dir: PathBuf) -> Result<Self, SnapshotError> {
        let blobs = dir.join("blobs");
        create_private_dir_all(&blobs).map_err(|e| SnapshotError::io("create", &blobs, e))?;
        let db = dir.join("index.db");
        // Create the file owner-only first; SQLite gives its WAL side files the same mode.
        match private_file_options().open(&db) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(SnapshotError::io("create", &db, e)),
        }
        let conn = Connection::open(&db)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let store = Self { conn, dir };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), SnapshotError> {
        let supported = u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX);
        let tx = self.begin()?;
        let found: u32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if found > supported {
            return Err(SnapshotError::UnsupportedSchema { found, supported });
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(found as usize) {
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i as i64 + 1)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn blob_path(&self, hash: &ContentHash) -> PathBuf {
        let (shard, rest) = hash.as_str().split_at(2);
        self.dir.join("blobs").join(shard).join(rest)
    }

    fn begin(&self) -> Result<Transaction<'_>, SnapshotError> {
        Ok(Transaction::new_unchecked(
            &self.conn,
            TransactionBehavior::Immediate,
        )?)
    }

    /// Captures `paths` (repository path, absolute path on disk) as `kind` images of `session`,
    /// in one index transaction. A missing file is recorded as [`FileState::Absent`]. Content
    /// already stored is not rewritten. Symlinks are followed (the target's content is captured).
    ///
    /// # Errors
    ///
    /// [`SnapshotError::IsDirectory`] for a directory, otherwise I/O and index errors. Nothing is
    /// recorded if an error is returned (blobs written so far are swept by `gc`).
    pub fn capture(
        &self,
        session: &SessionId,
        kind: SnapshotKind,
        paths: &[(RepoPath, &Path)],
        now: Timestamp,
    ) -> Result<Vec<CaptureResult>, SnapshotError> {
        let now_ms = now.as_millisecond();
        let tx = self.begin()?;
        let mut compressor = zstd::bulk::Compressor::new(ZSTD_LEVEL)
            .map_err(|e| SnapshotError::io("start compressor for", &self.dir, e))?;
        let mut results = Vec::with_capacity(paths.len());
        {
            let mut insert_entry = tx.prepare_cached(
                "INSERT INTO entries (session, path, kind, taken_at, hash, size, mode)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut upsert_blob = tx.prepare_cached(
                "INSERT INTO blobs (hash, size, stored, last_access) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (hash) DO UPDATE SET last_access = excluded.last_access,
                                                  stored = excluded.stored",
            )?;
            for (repo_path, abs) in paths {
                let (state, stored_new_blob) = match read_file(abs)? {
                    None => (FileState::Absent, false),
                    Some((bytes, mode)) => {
                        let hash = ContentHash::of(&bytes);
                        let blob = self.blob_path(&hash);
                        let (stored, new) = match fs::metadata(&blob) {
                            Ok(meta) => (meta.len(), false),
                            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                                let packed = compressor
                                    .compress(&bytes)
                                    .map_err(|e| SnapshotError::io("compress", abs, e))?;
                                if let Some(shard) = blob.parent() {
                                    create_private_dir_all(shard)
                                        .map_err(|e| SnapshotError::io("create", shard, e))?;
                                }
                                write_private_atomic(&blob, &packed)
                                    .map_err(|e| SnapshotError::io("write", &blob, e))?;
                                (packed.len() as u64, true)
                            }
                            Err(e) => return Err(SnapshotError::io("inspect", &blob, e)),
                        };
                        let size = bytes.len() as u64;
                        upsert_blob.execute(params![
                            hash.as_str(),
                            db_int(size),
                            db_int(stored),
                            now_ms
                        ])?;
                        (FileState::File { hash, size, mode }, new)
                    }
                };
                let (hash, size, mode) = match &state {
                    FileState::Absent => (None, 0, 0),
                    FileState::File { hash, size, mode } => (Some(hash.as_str()), *size, *mode),
                };
                insert_entry.execute(params![
                    session.as_str(),
                    repo_path.as_str(),
                    kind.code(),
                    now_ms,
                    hash,
                    db_int(size),
                    mode
                ])?;
                results.push(CaptureResult {
                    entry: SnapshotEntry {
                        seq: tx.last_insert_rowid(),
                        session: session.clone(),
                        path: repo_path.clone(),
                        kind,
                        taken_at: now,
                        state,
                    },
                    stored_new_blob,
                });
            }
        }
        tx.commit()?;
        Ok(results)
    }

    /// Captures every file under `root` (recursively, in sorted order; symlinks are skipped) as
    /// `kind` images, naming them `prefix/<relative path>`. If `root` is a file it is captured as
    /// `prefix`; if it does not exist nothing is captured.
    ///
    /// # Errors
    ///
    /// As [`capture`](Self::capture), plus [`SnapshotError::InvalidPath`] for a file name that is
    /// not UTF-8.
    pub fn capture_tree(
        &self,
        session: &SessionId,
        kind: SnapshotKind,
        prefix: &RepoPath,
        root: &Path,
        now: Timestamp,
    ) -> Result<Vec<CaptureResult>, SnapshotError> {
        let mut files = Vec::new();
        match fs::metadata(root) {
            Ok(meta) if meta.is_file() => files.push((prefix.clone(), root.to_owned())),
            Ok(_) => walk(root, prefix.as_str(), &mut files)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(SnapshotError::io("inspect", root, e)),
        }
        let borrowed: Vec<(RepoPath, &Path)> = files
            .iter()
            .map(|(r, p)| (r.clone(), p.as_path()))
            .collect();
        self.capture(session, kind, &borrowed, now)
    }

    /// The first pre-image of `path` in `session`: the state before the session first touched it.
    ///
    /// # Errors
    ///
    /// Index errors.
    pub fn pre_image(
        &self,
        session: &SessionId,
        path: &RepoPath,
    ) -> Result<Option<SnapshotEntry>, SnapshotError> {
        self.image(session, path, SnapshotKind::Pre, "ASC")
    }

    /// The latest post-image of `path` in `session`: the state the session last left it in.
    ///
    /// # Errors
    ///
    /// Index errors.
    pub fn post_image(
        &self,
        session: &SessionId,
        path: &RepoPath,
    ) -> Result<Option<SnapshotEntry>, SnapshotError> {
        self.image(session, path, SnapshotKind::Post, "DESC")
    }

    fn image(
        &self,
        session: &SessionId,
        path: &RepoPath,
        kind: SnapshotKind,
        order: &str,
    ) -> Result<Option<SnapshotEntry>, SnapshotError> {
        let sql = format!(
            "{SELECT_ENTRY} WHERE session = ?1 AND path = ?2 AND kind = ?3 ORDER BY id {order} LIMIT 1"
        );
        let row = self
            .conn
            .query_row(
                &sql,
                params![session.as_str(), path.as_str(), kind.code()],
                raw_entry,
            )
            .optional()?;
        row.map(RawEntry::into_entry).transpose()
    }

    /// Every entry of `session`, in capture order.
    ///
    /// # Errors
    ///
    /// Index errors.
    pub fn list(&self, session: &SessionId) -> Result<Vec<SnapshotEntry>, SnapshotError> {
        let sql = format!("{SELECT_ENTRY} WHERE session = ?1 ORDER BY id");
        let mut stmt = self.conn.prepare(&sql)?;
        let raws = stmt
            .query_map(params![session.as_str()], raw_entry)?
            .collect::<Result<Vec<_>, _>>()?;
        raws.into_iter().map(RawEntry::into_entry).collect()
    }

    /// Forgets every entry of `session`, so its blobs become collectable by [`gc`](Self::gc).
    /// Returns the number of entries removed.
    ///
    /// # Errors
    ///
    /// Index errors.
    pub fn drop_session(&self, session: &SessionId) -> Result<usize, SnapshotError> {
        Ok(self.conn.execute(
            "DELETE FROM entries WHERE session = ?1",
            params![session.as_str()],
        )?)
    }

    /// Reads and verifies the content stored under `hash`.
    ///
    /// # Errors
    ///
    /// [`SnapshotError::Evicted`] if the blob was evicted, [`SnapshotError::MissingBlob`] if its
    /// file was deleted behind our back, [`SnapshotError::Corrupt`] if it does not decompress
    /// to content with this hash.
    pub fn read_blob(&self, hash: &ContentHash) -> Result<Vec<u8>, SnapshotError> {
        let known: Option<i64> = self
            .conn
            .query_row(
                "SELECT size FROM blobs WHERE hash = ?1",
                params![hash.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if known.is_none() {
            return Err(SnapshotError::Evicted { hash: hash.clone() });
        }
        let path = self.blob_path(hash);
        let packed = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(SnapshotError::MissingBlob { hash: hash.clone() });
            }
            Err(e) => return Err(SnapshotError::io("read", &path, e)),
        };
        let corrupt = |reason: String| SnapshotError::Corrupt {
            hash: hash.clone(),
            reason,
        };
        let bytes = zstd::stream::decode_all(packed.as_slice())
            .map_err(|e| corrupt(format!("decompression failed: {e}")))?;
        if ContentHash::of(&bytes) != *hash {
            return Err(corrupt("content does not match its hash".to_owned()));
        }
        Ok(bytes)
    }

    /// Restores `entry` to `dest`: writes the captured bytes and mode (replacing whatever is
    /// there, creating parent directories), or deletes `dest` if the entry is
    /// [`FileState::Absent`]. Marks the blob as recently used at `now`.
    ///
    /// # Errors
    ///
    /// The errors of [`read_blob`](Self::read_blob), plus I/O errors writing `dest`.
    pub fn restore(
        &self,
        entry: &SnapshotEntry,
        dest: &Path,
        now: Timestamp,
    ) -> Result<(), SnapshotError> {
        match &entry.state {
            FileState::Absent => match fs::remove_file(dest) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(SnapshotError::io("remove", dest, e)),
            },
            FileState::File { hash, mode, .. } => {
                let bytes = self.read_blob(hash)?;
                self.conn.execute(
                    "UPDATE blobs SET last_access = ?2 WHERE hash = ?1",
                    params![hash.as_str(), now.as_millisecond()],
                )?;
                write_restored(dest, &bytes, *mode)
            }
        }
    }

    /// Writes `bytes` with `mode` to `dest` the way [`restore`](Self::restore) does (temporary
    /// sibling file, then rename; parent directories are created). Used by `acs undo` to write
    /// merged content.
    ///
    /// # Errors
    ///
    /// I/O errors writing `dest`.
    pub fn write_file(dest: &Path, bytes: &[u8], mode: u32) -> Result<(), SnapshotError> {
        write_restored(dest, bytes, mode)
    }

    /// Enforces the size budget.
    ///
    /// Removes blobs no entry refers to, then evicts least-recently-used blobs (by compressed
    /// size on disk) until the store holds at most `budget_bytes`. Blobs referenced by an entry of
    /// a session in `live_sessions` are never evicted, so the store may stay over budget; the
    /// report says so. Entries of evicted blobs stay in the index and report
    /// [`SnapshotError::Evicted`]. Finally, blob files without an index row and temporary files
    /// are swept.
    ///
    /// # Errors
    ///
    /// Index and I/O errors; an interrupted gc is finished by the next one.
    pub fn gc(
        &self,
        budget_bytes: u64,
        live_sessions: &HashSet<SessionId>,
    ) -> Result<GcReport, SnapshotError> {
        let mut report = GcReport::default();
        let mut doomed: Vec<String> = Vec::new();

        // Step 1: drop index rows (one transaction). Files go afterwards.
        let tx = self.begin()?;
        let mut live_hashes: HashSet<String> = HashSet::new();
        {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT hash FROM entries WHERE session = ?1 AND hash IS NOT NULL",
            )?;
            for session in live_sessions {
                let rows = stmt.query_map(params![session.as_str()], |r| r.get::<_, String>(0))?;
                for hash in rows {
                    live_hashes.insert(hash?);
                }
            }
        }
        let referenced: Vec<(String, u64)> = {
            let mut stmt = tx.prepare(
                "SELECT hash, stored, EXISTS (SELECT 1 FROM entries e WHERE e.hash = blobs.hash)
                 FROM blobs ORDER BY last_access, hash",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    as_u64(r.get::<_, i64>(1)?),
                    r.get::<_, bool>(2)?,
                ))
            })?;
            let mut referenced = Vec::new();
            for row in rows {
                let (hash, stored, is_referenced) = row?;
                if is_referenced {
                    referenced.push((hash, stored));
                } else {
                    report.unreferenced_blobs += 1;
                    report.freed_bytes += stored;
                    doomed.push(hash);
                }
            }
            referenced
        };
        let mut total: u64 = referenced.iter().map(|(_, s)| s).sum();
        report.live_bytes = referenced
            .iter()
            .filter(|(h, _)| live_hashes.contains(h))
            .map(|(_, s)| s)
            .sum();
        for (hash, stored) in referenced {
            if total <= budget_bytes {
                break;
            }
            if live_hashes.contains(&hash) {
                continue;
            }
            total -= stored;
            report.evicted_blobs += 1;
            report.freed_bytes += stored;
            doomed.push(hash);
        }
        {
            let mut delete = tx.prepare("DELETE FROM blobs WHERE hash = ?1")?;
            for hash in &doomed {
                delete.execute(params![hash])?;
            }
        }
        tx.commit()?;
        report.remaining_bytes = total;
        report.over_budget = total > budget_bytes;

        // Step 2: delete files with no index row. Re-checked under the write lock, so a capture
        // that stored the same content in between keeps its blob.
        let tx = self.begin()?;
        let known: HashSet<String> = {
            let mut stmt = tx.prepare("SELECT hash FROM blobs")?;
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<_, _>>()?
        };
        report.orphan_files = self.sweep(&known)?.saturating_sub(doomed.len());
        tx.commit()?;
        Ok(report)
    }

    /// Deletes blob files not in `known` and leftover temporary files; returns how many.
    fn sweep(&self, known: &HashSet<String>) -> Result<usize, SnapshotError> {
        let blobs = self.dir.join("blobs");
        let mut removed = 0usize;
        let shards = fs::read_dir(&blobs).map_err(|e| SnapshotError::io("list", &blobs, e))?;
        for shard in shards {
            let shard = shard.map_err(|e| SnapshotError::io("list", &blobs, e))?;
            if !shard.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let shard_name = shard.file_name().to_string_lossy().into_owned();
            let shard_path = shard.path();
            let files =
                fs::read_dir(&shard_path).map_err(|e| SnapshotError::io("list", &shard_path, e))?;
            for file in files {
                let file = file.map_err(|e| SnapshotError::io("list", &shard_path, e))?;
                let name = file.file_name().to_string_lossy().into_owned();
                let orphan =
                    name.starts_with('.') || !known.contains(&format!("{shard_name}{name}"));
                if orphan {
                    match fs::remove_file(file.path()) {
                        Ok(()) => removed += 1,
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                        Err(e) => return Err(SnapshotError::io("remove", &file.path(), e)),
                    }
                }
            }
        }
        Ok(removed)
    }

    /// Total compressed bytes the index accounts for.
    ///
    /// # Errors
    ///
    /// Index errors.
    pub fn stored_bytes(&self) -> Result<u64, SnapshotError> {
        let total: i64 =
            self.conn
                .query_row("SELECT COALESCE(SUM(stored), 0) FROM blobs", [], |r| {
                    r.get(0)
                })?;
        Ok(as_u64(total))
    }
}

struct RawEntry {
    seq: i64,
    session: String,
    path: String,
    kind: i64,
    taken_at: i64,
    hash: Option<String>,
    size: u64,
    mode: u32,
}

/// SQLite integers are signed 64-bit; sizes never get near the limit.
fn db_int(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn as_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn raw_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawEntry> {
    Ok(RawEntry {
        seq: r.get(0)?,
        session: r.get(1)?,
        path: r.get(2)?,
        kind: r.get(3)?,
        taken_at: r.get(4)?,
        hash: r.get(5)?,
        size: as_u64(r.get::<_, i64>(6)?),
        mode: r.get(7)?,
    })
}

fn parse_hash(hash: &str) -> Result<ContentHash, SnapshotError> {
    ContentHash::try_from(hash.to_owned()).map_err(|e| SnapshotError::InvalidIndex(e.to_string()))
}

impl RawEntry {
    fn into_entry(self) -> Result<SnapshotEntry, SnapshotError> {
        let bad = |what: &str, e: &dyn std::fmt::Display| {
            SnapshotError::InvalidIndex(format!("{what}: {e}"))
        };
        Ok(SnapshotEntry {
            seq: self.seq,
            session: self.session.parse().map_err(|e| bad("session", &e))?,
            path: self.path.parse().map_err(|e| bad("path", &e))?,
            kind: match self.kind {
                0 => SnapshotKind::Pre,
                1 => SnapshotKind::Post,
                other => return Err(bad("kind", &other)),
            },
            taken_at: Timestamp::from_millisecond(self.taken_at).map_err(|e| bad("time", &e))?,
            state: match self.hash {
                None => FileState::Absent,
                Some(hash) => FileState::File {
                    hash: parse_hash(&hash)?,
                    size: self.size,
                    mode: self.mode,
                },
            },
        })
    }
}

/// Reads a file and its mode; `None` if it does not exist.
pub(crate) fn read_file(path: &Path) -> Result<Option<(Vec<u8>, u32)>, SnapshotError> {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SnapshotError::io("inspect", path, e)),
    };
    if meta.is_dir() {
        return Err(SnapshotError::IsDirectory(path.to_owned()));
    }
    match fs::read(path) {
        Ok(bytes) => Ok(Some((bytes, mode_of(&meta)))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SnapshotError::io("read", path, e)),
    }
}

#[cfg(unix)]
fn mode_of(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn mode_of(meta: &fs::Metadata) -> u32 {
    if meta.permissions().readonly() {
        0o444
    } else {
        0o644
    }
}

/// Collects the regular files under `dir` as `(repo path, disk path)`, sorted.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<(RepoPath, PathBuf)>) -> Result<(), SnapshotError> {
    let mut children = Vec::new();
    for child in fs::read_dir(dir).map_err(|e| SnapshotError::io("list", dir, e))? {
        children.push(child.map_err(|e| SnapshotError::io("list", dir, e))?);
    }
    children.sort_by_key(fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let name = child
            .file_name()
            .into_string()
            .map_err(|_| SnapshotError::InvalidPath(path.clone()))?;
        let repo = format!("{prefix}/{name}");
        let Ok(kind) = child.file_type() else {
            continue;
        };
        if kind.is_dir() {
            walk(&path, &repo, out)?;
        } else if kind.is_file() {
            let repo = repo
                .parse()
                .map_err(|_| SnapshotError::InvalidPath(path.clone()))?;
            out.push((repo, path));
        }
    }
    Ok(())
}

/// Writes `bytes` with `mode` to `dest` via a sibling temporary file and a rename.
fn write_restored(dest: &Path, bytes: &[u8], mode: u32) -> Result<(), SnapshotError> {
    use std::io::Write;
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| SnapshotError::io("create", parent, e))?;
    let name = dest
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let tmp = parent.join(format!(".{name}.acs-restore-{}", std::process::id()));
    let result = (|| {
        let _ = fs::remove_file(&tmp);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        drop(file);
        set_mode(&tmp, mode)?;
        clear_readonly(dest)?;
        fs::rename(&tmp, dest)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(SnapshotError::io("restore", dest, e));
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_readonly(mode & 0o200 == 0);
    fs::set_permissions(path, perms)
}

/// Windows refuses to replace a read-only file; Unix does not care.
#[cfg(not(unix))]
#[allow(clippy::permissions_set_readonly_false)]
fn clear_readonly(dest: &Path) -> io::Result<()> {
    match fs::metadata(dest) {
        Ok(meta) if meta.permissions().readonly() => {
            let mut perms = meta.permissions();
            perms.set_readonly(false);
            fs::set_permissions(dest, perms)
        }
        _ => Ok(()),
    }
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)]
fn clear_readonly(_dest: &Path) -> io::Result<()> {
    Ok(())
}
