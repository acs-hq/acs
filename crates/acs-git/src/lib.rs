//! ACS git: local repository reads and working-tree change detection.
//!
//! - [`repo`]: finding the work tree root, the git directory and the common git directory from a
//!   directory; the current branch read straight from `HEAD` (no process); whether a remote is
//!   configured; the pilot's email (`git config user.email`, spawned only when a seat is
//!   created); blobs of the index (for baselines).
//! - [`status`]: which files a shell command changed, for post-tool attribution.
//! - [`hooks`]: installing the `pre-commit`, `prepare-commit-msg` and `pre-push` backstop hooks so
//!   they chain with existing ones, and the reads those hooks need.
//! - [`trailers`]: adding and reading commit-message trailers (`ACS-Session`).
//! - [`refs`]: where shared state lives: the [`refs::SyncStorage`] interface, implemented over
//!   `refs/acs/*` by [`refs::CustomRefs`]. All ref reads and writes go through it.
//! - [`sync`]: one sync with the remote: fetch, rebuild the remote-claims view, publish, push.
//!
//! Network access and signing shell out to the user's `git`. Nothing here needs a git library: the
//! hot-path reads are a few small files, and the one place that needs git's own logic
//! (`.gitignore`, index comparison) runs `git`.

pub mod hooks;
pub mod refs;
pub mod repo;
pub mod status;
pub mod sync;
pub mod trailers;

mod error;
mod ledger;

pub use error::GitError;
pub use repo::{RepoLayout, discover};
