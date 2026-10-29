//! ACS store: `ACS_HOME`, repository identity, SQLite state and snapshot blobs.
//!
//! - [`home`]: resolving `ACS_HOME` and the per-repository directory layout, created lazily
//!   with owner-only permissions.
//! - [`repo_id`]: the repository id (stable across clones and worktrees), with a cache so the
//!   hook hot path never walks history or spawns a process.
//! - [`fsutil`]: owner-only directory and file helpers.
//! - [`db`], [`seats`], [`claims`], [`events`], [`notices`]: the SQLite state database
//!   (`repos/<repo-id>/state.db`): seats, atomic claims with lazy expiry, the remote-claims view,
//!   the event log, notices and counters. Start at [`StateDb`].
//! - [`snapshots`]: content-addressed, zstd-compressed pre- and post-images with their own index
//!   (`repos/<repo-id>/snapshots/index.db`) and LRU eviction. Start at [`SnapshotStore`].

pub mod claims;
pub mod db;
pub mod events;
pub mod fsutil;
pub mod home;
pub mod notices;
pub mod repo_id;
pub mod seats;
pub mod snapshots;

pub use claims::{AcquireOutcome, Acquired, Conflict, SyncStatus};
pub use db::{DEFAULT_BUSY_TIMEOUT, DbConfig, SCHEMA_VERSION, StateDb, StoreError};
pub use events::{NewEvent, StoredEvent};
pub use home::{AcsHome, HomeError, Migration, RepoHome};
pub use repo_id::{
    GitCli, IdKind, IdSource, InvalidRepoId, RepoId, RepoIdError, RepoIdentity, RootFinder,
    find_common_dir,
};
pub use seats::NewSeat;
pub use snapshots::{
    CaptureResult, FileState, GcReport, SnapshotEntry, SnapshotError, SnapshotKind, SnapshotStore,
};
