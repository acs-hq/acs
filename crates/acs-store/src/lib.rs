//! ACS store: `ACS_HOME`, repository identity, SQLite state and snapshot blobs.
//!
//! This crate currently provides the foundation the rest of the store builds on:
//!
//! - [`home`]: resolving `ACS_HOME` and the per-repository directory layout, created lazily
//!   with owner-only permissions.
//! - [`repo_id`]: the repository id (stable across clones and worktrees), with a cache so the
//!   hook hot path never walks history or spawns a process.
//! - [`fsutil`]: owner-only directory and file helpers for the modules to come.

pub mod fsutil;
pub mod home;
pub mod repo_id;

pub use home::{AcsHome, HomeError, Migration, RepoHome};
pub use repo_id::{
    GitCli, IdKind, IdSource, InvalidRepoId, RepoId, RepoIdError, RepoIdentity, RootFinder,
    find_common_dir,
};
