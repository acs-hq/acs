//! The state database: opening it, migrating it and the errors it can produce.
//!
//! `repos/<repo-id>/state.db` is one SQLite file in WAL mode shared by every `acs` process of a
//! repository on this machine (hooks, the CLI, the MCP server, background sync). The methods that
//! use it live next to what they manage: [`crate::seats`], [`crate::claims`], [`crate::events`]
//! and [`crate::notices`]. Design choices are recorded in `docs/decisions/0006-state-store.md`.
//!
//! Rules every method follows:
//!
//! - **Time is injected.** Nothing here reads the system clock; every method that needs "now"
//!   takes a [`Timestamp`]. Timestamps are stored as microseconds since the Unix epoch.
//! - **Writes are serialized with `BEGIN IMMEDIATE`.** A deferred transaction that reads and then
//!   writes can fail with `SQLITE_BUSY` immediately, ignoring the busy timeout, in WAL mode; an
//!   immediate one waits its turn instead. Reads never block and are never blocked.
//! - **Any error means the state cannot be trusted for this event.** The pipeline maps every
//!   [`StoreError`] it gets from opening or using the database to `GateState::state_unavailable`
//!   (see [`StoreError::is_state_unavailable`]).

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use acs_core::{ParseError, Timestamp};
use rusqlite::{Connection, ErrorCode, OpenFlags, Transaction, TransactionBehavior};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::fsutil::private_file_options;
use crate::home::{HomeError, RepoHome};

/// How long a connection waits for another process's write lock before failing with
/// [`StoreError::Locked`].
///
/// Write transactions here take well under a millisecond, so real contention clears in a few
/// milliseconds and this value is never reached; it only bounds the damage when a process is
/// stuck holding the lock. It is long enough that a burst of agents starting together does not
/// turn into `state_unavailable` (which weakens enforcement), and short enough that a hook still
/// answers within any harness's hook timeout.
pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(3);

/// Errors from the state store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The repository directory under `ACS_HOME` could not be created.
    #[error(transparent)]
    Home(#[from] HomeError),
    /// A filesystem operation failed.
    #[error("{action} {}: {source}", .path.display())]
    Io {
        /// What was being attempted.
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The database file does not exist and the caller asked not to create it.
    #[error("the state database {} does not exist; run `acs doctor`", .path.display())]
    Missing {
        /// The expected location.
        path: PathBuf,
    },
    /// Another process holds the database locked for longer than the busy timeout.
    #[error("the state database is locked by another process; retry, or run `acs doctor`")]
    Locked,
    /// The file is not a SQLite database or is damaged.
    #[error("the state database is corrupt ({detail}); run `acs doctor` to repair it")]
    Corrupt {
        /// What SQLite reported.
        detail: String,
    },
    /// The database was written by a newer `acs`.
    #[error(
        "the state database has schema version {found} but this acs understands up to {supported}; \
         upgrade acs"
    )]
    SchemaTooNew {
        /// The version stored in the file.
        found: i64,
        /// The newest version this build can use.
        supported: i64,
    },
    /// The database could not be opened for another reason (for example permissions).
    #[error("cannot open the state database {}: {source}", .path.display())]
    OpenFailed {
        /// The database path.
        path: PathBuf,
        /// What SQLite reported.
        #[source]
        source: rusqlite::Error,
    },
    /// Any other SQLite failure.
    #[error("state database error: {0}")]
    Sqlite(#[source] rusqlite::Error),
    /// A seat id that is not in the database.
    #[error("unknown seat {0}")]
    UnknownSeat(String),
    /// A stored value could not be read back (a row written by something else, or damage).
    #[error("the state database holds an invalid value: {0}")]
    InvalidData(String),
    /// The operating system's random number generator failed while making an id.
    #[error("cannot generate an id: {0}")]
    Random(String),
}

impl StoreError {
    /// Whether the pipeline should treat this as "the state database could not be used"
    /// (`GateState::state_unavailable`, SPEC section 11). True for every error except a lookup of
    /// an unknown seat, which is a caller bug rather than a database problem.
    #[must_use]
    pub fn is_state_unavailable(&self) -> bool {
        !matches!(self, Self::UnknownSeat(_))
    }

    pub(crate) fn invalid(what: &str, err: impl std::fmt::Display) -> Self {
        Self::InvalidData(format!("{what}: {err}"))
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(failure, message) = &err {
            match failure.code {
                ErrorCode::DatabaseBusy
                | ErrorCode::DatabaseLocked
                | ErrorCode::FileLockingProtocolFailed => return Self::Locked,
                ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt => {
                    return Self::Corrupt {
                        detail: message.clone().unwrap_or_else(|| failure.to_string()),
                    };
                }
                _ => {}
            }
        }
        Self::Sqlite(err)
    }
}

/// Options for opening the state database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbConfig {
    /// How long to wait for another process's write lock (default [`DEFAULT_BUSY_TIMEOUT`]).
    pub busy_timeout: Duration,
    /// Whether to create the database when it does not exist (default `true`). Read-only
    /// commands such as `acs status` can set this to `false` to get [`StoreError::Missing`]
    /// instead of creating an empty database.
    pub create: bool,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
            create: true,
        }
    }
}

/// The migrations, in order. Entry `i` upgrades `user_version` `i` to `i + 1`. Forward-only:
/// never edit a released entry, add a new one.
const MIGRATIONS: &[&str] = &[
    include_str!("migrations/0001_local_state.sql"),
    include_str!("migrations/0002_remote_claims.sql"),
    include_str!("migrations/0003_hook_pipeline.sql"),
    include_str!("migrations/0004_collisions.sql"),
];

/// The schema version this build reads and writes.
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

/// A connection to a repository's state database.
///
/// Opening costs a few hundred microseconds (pragmas and a version check); the hook opens one per
/// process. Methods take `&self`: SQLite serializes writers across processes, and a connection
/// is used by one thread at a time (it is `Send` but not `Sync`).
#[derive(Debug)]
pub struct StateDb {
    pub(crate) conn: Connection,
    path: PathBuf,
}

impl StateDb {
    /// Opens `state.db` of `home`, creating the repository directory, the file (mode `0o600` on
    /// Unix) and the schema when missing, and migrating an older schema forward.
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`], [`StoreError::Locked`], [`StoreError::SchemaTooNew`] or another
    /// [`StoreError`]. All of them mean the pipeline should report `state_unavailable`.
    pub fn open(home: &RepoHome) -> Result<Self, StoreError> {
        Self::open_with(home, &DbConfig::default())
    }

    /// Like [`StateDb::open`] with explicit [`DbConfig`] options.
    ///
    /// # Errors
    ///
    /// As for [`StateDb::open`]; also [`StoreError::Missing`] when `config.create` is false and
    /// the file does not exist.
    pub fn open_with(home: &RepoHome, config: &DbConfig) -> Result<Self, StoreError> {
        if config.create {
            home.ensure()?;
        }
        Self::open_at(&home.state_db(), config)
    }

    /// Opens the database file at `path` (its directory must exist). The path-based form of
    /// [`StateDb::open_with`], for tools and tests that manage the location themselves.
    ///
    /// # Errors
    ///
    /// As for [`StateDb::open_with`].
    pub fn open_at(path: &Path, config: &DbConfig) -> Result<Self, StoreError> {
        if config.create {
            // Create the file ourselves so it is owner-only from the first byte; SQLite gives the
            // -wal and -shm files the same mode as the main file.
            match private_file_options().open(path) {
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => {
                    return Err(StoreError::Io {
                        action: "creating",
                        path: path.to_path_buf(),
                        source: err,
                    });
                }
            }
        } else if !path.exists() {
            return Err(StoreError::Missing {
                path: path.to_path_buf(),
            });
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|source| StoreError::OpenFailed {
            path: path.to_path_buf(),
            source,
        })?;
        let db = Self {
            conn,
            path: path.to_path_buf(),
        };
        db.configure(config)?;
        Ok(db)
    }

    /// The path of the database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Runs SQLite's integrity check (`PRAGMA integrity_check`). Slow on big databases; meant for
    /// `acs doctor`, not for the hook path.
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] with SQLite's findings, or another error if the check cannot run.
    pub fn integrity_check(&self) -> Result<(), StoreError> {
        let mut stmt = self.conn.prepare("PRAGMA integrity_check")?;
        let findings: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        if findings.len() == 1 && findings[0] == "ok" {
            Ok(())
        } else {
            Err(StoreError::Corrupt {
                detail: findings.join("; "),
            })
        }
    }

    /// Sets the connection up and migrates the schema, retrying while another process is doing
    /// the same.
    ///
    /// SQLite's busy handler does not cover everything the first opener does: switching a new
    /// file to WAL mode, or reading it while another connection does, can fail at once with
    /// "locked" instead of waiting. Several hooks starting together on a new repository is a
    /// real situation, so the whole (idempotent) sequence is retried with a short backoff until
    /// the busy timeout is used up. This is waiting, not domain time: it uses the monotonic
    /// clock, never the injected one.
    fn configure(&self, config: &DbConfig) -> Result<(), StoreError> {
        let started = Instant::now();
        let mut pause = Duration::from_millis(1);
        loop {
            match self.configure_once(config) {
                Err(StoreError::Locked) if started.elapsed() < config.busy_timeout => {
                    std::thread::sleep(pause);
                    pause = (pause * 2).min(Duration::from_millis(50));
                }
                other => return other,
            }
        }
    }

    fn configure_once(&self, config: &DbConfig) -> Result<(), StoreError> {
        let conn = &self.conn;
        conn.busy_timeout(config.busy_timeout)?;
        // The first read of the file: a non-database file fails here, as `Corrupt`.
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            // Persistent in the file header, so this happens once per database.
            conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
                row.get::<_, String>(0)
            })?;
        }
        // NORMAL in WAL mode never loses a committed transaction to a process crash; only a power
        // failure can lose the most recent ones. Claims expire anyway, so that is acceptable.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_size_limit", 4 * 1024 * 1024)?;
        migrate(conn, MIGRATIONS)
    }

    /// Starts an immediate (write-locking) transaction.
    pub(crate) fn write_tx(&self) -> Result<Transaction<'_>, StoreError> {
        Ok(Transaction::new_unchecked(
            &self.conn,
            TransactionBehavior::Immediate,
        )?)
    }

    /// Runs `body` in a write transaction and commits it if `body` succeeds.
    pub(crate) fn write<T>(
        &self,
        body: impl FnOnce(&Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let tx = self.write_tx()?;
        let value = body(&tx)?;
        tx.commit()?;
        Ok(value)
    }
}

/// Brings the database to `migrations.len()`. Each step runs in its own immediate transaction
/// that re-reads `user_version` first, so processes racing to upgrade apply each step once.
pub(crate) fn migrate(conn: &Connection, migrations: &[&str]) -> Result<(), StoreError> {
    let latest = migrations.len() as i64;
    let current = user_version(conn)?;
    if current > latest {
        return Err(StoreError::SchemaTooNew {
            found: current,
            supported: latest,
        });
    }
    for (index, sql) in migrations.iter().enumerate() {
        let target = index as i64 + 1;
        if current >= target {
            continue;
        }
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        if user_version(&tx)? >= target {
            continue; // Another process applied it while we waited for the lock.
        }
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", target)?;
        tx.commit()?;
    }
    Ok(())
}

fn user_version(conn: &Connection) -> Result<i64, StoreError> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

// ---------------------------------------------------------------------------------------------
// Conversions shared by the modules
// ---------------------------------------------------------------------------------------------

/// A timestamp as stored: microseconds since the Unix epoch.
pub(crate) fn to_db(at: Timestamp) -> i64 {
    at.as_microsecond()
}

/// Reads a stored timestamp back.
pub(crate) fn from_db(micros: i64) -> Result<Timestamp, StoreError> {
    Timestamp::from_microsecond(micros).map_err(|err| StoreError::invalid("timestamp", err))
}

/// `heartbeat + ttl` in stored form, saturating instead of overflowing.
pub(crate) fn expiry_micros(heartbeat_micros: i64, ttl_secs: i64) -> i64 {
    heartbeat_micros.saturating_add(ttl_secs.saturating_mul(1_000_000))
}

/// A TTL as stored (it is a `u64` in the records; SQLite integers are signed).
pub(crate) fn ttl_to_db(ttl_secs: u64) -> i64 {
    i64::try_from(ttl_secs).unwrap_or(i64::MAX)
}

/// An enum's wire name (`snake_case`), the form stored in text columns.
pub(crate) fn enum_to_db<T: Serialize>(value: &T) -> Result<String, StoreError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => Ok(name),
        Ok(other) => Err(StoreError::invalid("enum", other)),
        Err(err) => Err(StoreError::invalid("enum", err)),
    }
}

/// Reads an enum back from its wire name.
pub(crate) fn enum_from_db<T: DeserializeOwned>(name: &str) -> Result<T, StoreError> {
    serde_json::from_value(serde_json::Value::String(name.to_owned()))
        .map_err(|err| StoreError::invalid("enum", err))
}

/// Parses a stored id.
pub(crate) fn id_from_db<T>(value: String) -> Result<T, StoreError>
where
    T: TryFrom<String, Error = ParseError>,
{
    T::try_from(value).map_err(|err| StoreError::invalid("id", err))
}

/// Makes a new random id of type `T` with the given prefix.
///
/// The body is 128 random bits from the operating system (`getrandom`) in lowercase RFC 4648
/// base32 (`a-z`, `2-7`): 26 characters, inside the `[0-9a-z_-]{1,64}` charset of acs-core ids.
/// Ids are not time-ordered; 128 random bits make collisions between processes and machines
/// practically impossible without any coordination.
pub(crate) fn new_id<T>(prefix: &str) -> Result<T, StoreError>
where
    T: TryFrom<String, Error = ParseError>,
{
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|err| StoreError::Random(err.to_string()))?;
    let mut id = String::with_capacity(prefix.len() + 26);
    id.push_str(prefix);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            id.push(char::from(ALPHABET[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        id.push(char::from(ALPHABET[((buffer << (5 - bits)) & 31) as usize]));
    }
    T::try_from(id).map_err(|err| StoreError::invalid("generated id", err))
}

/// Comma-separated numbered placeholders `?{start}, ?{start+1}, ...` for an `IN (...)` list.
pub(crate) fn placeholders(start: usize, count: usize) -> String {
    let mut out = String::with_capacity(count * 5);
    for i in 0..count {
        if i > 0 {
            out.push(',');
        }
        // Writing to a String cannot fail.
        let _ = write!(out, "?{}", start + i);
    }
    out
}

#[cfg(test)]
mod tests {
    use acs_core::{ClaimId, SeatId};

    use super::*;

    #[test]
    fn ids_have_the_prefix_the_charset_and_are_distinct() {
        let a: SeatId = new_id(SeatId::PREFIX).unwrap();
        let b: SeatId = new_id(SeatId::PREFIX).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.as_str().len(), "seat_".len() + 26);
        let c: ClaimId = new_id(ClaimId::PREFIX).unwrap();
        assert!(c.as_str().starts_with("clm_"));
        assert!(
            c.as_str()["clm_".len()..]
                .bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        );
    }

    #[test]
    fn expiry_saturates() {
        assert_eq!(expiry_micros(1_000_000, 900), 901_000_000);
        assert_eq!(expiry_micros(i64::MAX - 5, i64::MAX), i64::MAX);
        assert_eq!(ttl_to_db(u64::MAX), i64::MAX);
    }

    #[test]
    fn placeholder_lists() {
        assert_eq!(placeholders(3, 3), "?3,?4,?5");
        assert_eq!(placeholders(1, 0), "");
    }

    #[test]
    fn migrations_apply_in_order_once() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn, &["CREATE TABLE a (x)", "CREATE TABLE b (x)"]).unwrap();
        assert_eq!(user_version(&conn).unwrap(), 2);
        // Re-running is a no-op (would fail with "table a already exists" otherwise).
        migrate(&conn, &["CREATE TABLE a (x)", "CREATE TABLE b (x)"]).unwrap();
        // A newer step is applied on top.
        migrate(
            &conn,
            &[
                "CREATE TABLE a (x)",
                "CREATE TABLE b (x)",
                "CREATE TABLE c (x)",
            ],
        )
        .unwrap();
        assert_eq!(user_version(&conn).unwrap(), 3);
        // An older binary refuses a newer database.
        let err = migrate(&conn, &["CREATE TABLE a (x)"]).unwrap_err();
        assert!(matches!(
            err,
            StoreError::SchemaTooNew {
                found: 3,
                supported: 1
            }
        ));
    }

    #[test]
    fn a_failing_migration_rolls_back_entirely() {
        let conn = Connection::open_in_memory().unwrap();
        let err = migrate(&conn, &["CREATE TABLE a (x); CREATE TABLE a (y)"]).unwrap_err();
        assert!(matches!(err, StoreError::Sqlite(_)));
        assert_eq!(user_version(&conn).unwrap(), 0);
        let tables: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tables, 0);
    }
}
