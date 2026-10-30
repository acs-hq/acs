//! Errors of this crate.

use std::io;
use std::path::PathBuf;

/// Errors from reading a git repository.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// No work tree contains the directory (it is outside any repository, or inside a bare one).
    #[error("{} is not inside a git work tree", .0.display())]
    NotAWorkTree(PathBuf),
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
    /// A `git` process could not be started or failed.
    #[error("git {args}: {detail}")]
    Command {
        /// The arguments, for the message.
        args: String,
        /// What went wrong.
        detail: String,
    },
}

impl GitError {
    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}
