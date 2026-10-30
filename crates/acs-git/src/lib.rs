//! ACS git: local repository reads and working-tree change detection.
//!
//! - [`repo`]: finding the work tree root, the git directory and the common git directory from a
//!   directory; the current branch read straight from `HEAD` (no process); whether a remote is
//!   configured; the pilot's email (`git config user.email`, spawned only when a seat is
//!   created); blobs of the index (for baselines).
//! - [`status`]: which files a shell command changed, for post-tool attribution.
//!
//! Network access, signing, hooks and trailers arrive in later work packages and shell out to the
//! user's `git`. Nothing here needs a git library: the hot-path reads are a few small files, and
//! the one place that needs git's own logic (`.gitignore`, index comparison) runs `git`.

pub mod repo;
pub mod status;

mod error;

pub use error::GitError;
pub use repo::{RepoLayout, discover};
