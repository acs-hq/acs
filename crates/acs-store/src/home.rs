//! `ACS_HOME`: where ACS keeps per-machine state, and the typed layout inside it.
//!
//! ```text
//! ACS_HOME/
//!   index/                  repository-identity cache (see [`crate::repo_id`])
//!   repos/<repo-id>/
//!     state.db              SQLite state (created by the state store, not here)
//!     snapshots/            content-addressed blobs
//!     cache/                compiled policy
//!     pending/              pending-commit markers and proposed lore
//! ```
//!
//! Nothing is created until [`AcsHome::ensure`] or [`RepoHome::ensure`] is called. On Unix every
//! directory is created `0o700` and existing ones are tightened to that; see
//! `docs/decisions/0003-acs-home-and-repo-id.md` for the permission model and what it does not
//! cover on Windows.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::fsutil::create_private_dir_all;
use crate::repo_id::RepoId;

/// Name of the environment variable that overrides the location of `ACS_HOME`.
pub const ENV_VAR: &str = "ACS_HOME";

/// Name of the directory under the user's home directory used when `ACS_HOME` is not set.
pub const DEFAULT_DIR_NAME: &str = ".acs";

/// Failure to locate, create or migrate state under `ACS_HOME`.
#[derive(Debug, thiserror::Error)]
pub enum HomeError {
    /// `ACS_HOME` (or a path given to [`AcsHome::at`]) is not an absolute path.
    #[error(
        "ACS_HOME must be an absolute path, got {}; set it to a full path or unset it to use ~/.acs",
        .0.display()
    )]
    NotAbsolute(PathBuf),
    /// `ACS_HOME` is unset and the user's home directory could not be determined.
    #[error("cannot determine the user's home directory; set ACS_HOME to an absolute path")]
    NoHomeDir,
    /// A filesystem operation failed.
    #[error("{action} {}: {source}", .path.display())]
    Io {
        /// What was being attempted, for example `creating directory`.
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}

impl HomeError {
    pub(crate) fn io(action: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }
}

/// The root of ACS's per-machine state.
///
/// Cheap to construct and clone; it only holds a path. Use [`AcsHome::from_env`] in the binary
/// and [`AcsHome::at`] in tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcsHome {
    root: PathBuf,
}

impl AcsHome {
    /// Resolves `ACS_HOME` from the process environment and the user's home directory.
    ///
    /// # Errors
    ///
    /// See [`AcsHome::resolve`].
    pub fn from_env() -> Result<Self, HomeError> {
        Self::resolve(
            std::env::var_os(ENV_VAR).as_deref(),
            std::env::home_dir().as_deref(),
        )
    }

    /// The pure form of [`AcsHome::from_env`]: `env_value` is the value of the `ACS_HOME`
    /// variable and `user_home` the user's home directory (the profile directory on Windows).
    ///
    /// A set, non-empty `env_value` wins and must be absolute. An unset or empty one falls back
    /// to `<user_home>/.acs`.
    ///
    /// # Errors
    ///
    /// [`HomeError::NotAbsolute`] if `env_value` is relative, [`HomeError::NoHomeDir`] if the
    /// default is needed and `user_home` is `None` or empty.
    pub fn resolve(env_value: Option<&OsStr>, user_home: Option<&Path>) -> Result<Self, HomeError> {
        match env_value {
            Some(value) if !value.is_empty() => Self::at(PathBuf::from(value)),
            _ => match user_home {
                Some(home) if !home.as_os_str().is_empty() => Self::at(home.join(DEFAULT_DIR_NAME)),
                _ => Err(HomeError::NoHomeDir),
            },
        }
    }

    /// An `ACS_HOME` rooted at `root`, which must be absolute. Nothing is created.
    ///
    /// # Errors
    ///
    /// [`HomeError::NotAbsolute`] if `root` is relative.
    pub fn at(root: impl Into<PathBuf>) -> Result<Self, HomeError> {
        let root = root.into();
        if root.is_absolute() {
            Ok(Self { root })
        } else {
            Err(HomeError::NotAbsolute(root))
        }
    }

    /// The root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `ACS_HOME/repos`, the parent of every per-repository directory.
    #[must_use]
    pub fn repos_dir(&self) -> PathBuf {
        self.root.join("repos")
    }

    /// `ACS_HOME/index`, where the repository-identity cache lives.
    #[must_use]
    pub fn index_dir(&self) -> PathBuf {
        self.root.join("index")
    }

    /// The per-repository directory layout for `id`. Nothing is created.
    #[must_use]
    pub fn repo(&self, id: &RepoId) -> RepoHome {
        RepoHome {
            id: id.clone(),
            dir: self.repos_dir().join(id.as_str()),
        }
    }

    /// Creates `ACS_HOME`, `repos/` and `index/` as owner-only directories.
    ///
    /// Idempotent. On Unix, directories that already exist with group or other permissions are
    /// tightened to `0o700`; this call fails if that is not possible (for example, the
    /// directory belongs to another user).
    ///
    /// # Errors
    ///
    /// [`HomeError::Io`] if a directory cannot be created or tightened.
    pub fn ensure(&self) -> Result<(), HomeError> {
        for dir in [&self.root, &self.repos_dir(), &self.index_dir()] {
            ensure_dir(dir)?;
        }
        Ok(())
    }

    /// Moves the state of repository `from` to repository `to`.
    ///
    /// Used when a repository's identity changes, in practice when a repository that had no
    /// commits (and so an identity derived from its path) gets its first commit. The rules:
    ///
    /// - `repos/<from>` absent: nothing to do ([`Migration::NotNeeded`]).
    /// - `repos/<to>` absent: the directory is renamed, atomically ([`Migration::Moved`]).
    /// - both present: contents are merged into `repos/<to>`. Entries missing from the target are
    ///   moved; a source file identical to the target's is dropped; **anything that conflicts
    ///   stays in `repos/<from>`** and is listed in [`Migration::Merged::left_behind`]. The
    ///   target's `state.db` always wins, and its `-wal` and `-shm` companions are never mixed
    ///   with the other database's. Nothing is overwritten or deleted unless it is a byte-exact
    ///   duplicate.
    ///
    /// # Errors
    ///
    /// [`HomeError::Io`] if a rename fails (for example a file is held open on Windows). The
    /// operation may then be partly done, but never loses data; calling it again continues.
    pub fn migrate_repo(&self, from: &RepoId, to: &RepoId) -> Result<Migration, HomeError> {
        if from == to {
            return Ok(Migration::NotNeeded);
        }
        let src = self.repos_dir().join(from.as_str());
        let dst = self.repos_dir().join(to.as_str());
        match fs::symlink_metadata(&src) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Ok(Migration::NotNeeded),
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Migration::NotNeeded),
            Err(err) => return Err(HomeError::io("inspecting", &src, err)),
        }
        ensure_dir(&self.repos_dir())?;
        if !dst.exists() {
            match fs::rename(&src, &dst) {
                Ok(()) => return Ok(Migration::Moved),
                // Another process may have created the target meanwhile; merge instead.
                Err(_) if dst.exists() => {}
                Err(err) => return Err(HomeError::io("renaming", &src, err)),
            }
        }
        ensure_dir(&dst)?;
        let mut report = MergeReport::default();
        merge_dir(&src, &dst, true, &mut report)?;
        Ok(Migration::Merged {
            moved: report.moved,
            left_behind: report.left_behind,
        })
    }
}

/// The result of [`AcsHome::migrate_repo`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Migration {
    /// The source directory did not exist.
    NotNeeded,
    /// The source directory was renamed to the target.
    Moved,
    /// The target already existed and the source was merged into it.
    Merged {
        /// Number of files and directories moved into the target.
        moved: usize,
        /// Paths under the old directory that conflicted with the target and were kept. Empty
        /// when the merge was complete (the old directory is then removed).
        left_behind: Vec<PathBuf>,
    },
}

#[derive(Default)]
struct MergeReport {
    moved: usize,
    left_behind: Vec<PathBuf>,
}

fn ensure_dir(dir: &Path) -> Result<(), HomeError> {
    create_private_dir_all(dir).map_err(|err| HomeError::io("creating directory", dir, err))
}

fn is_state_db_name(name: &OsStr) -> bool {
    name.to_str().is_some_and(|n| n.starts_with("state.db"))
}

/// Merges directory `src` into the existing directory `dst`. Removes `src` if it ends up empty.
fn merge_dir(
    src: &Path,
    dst: &Path,
    top_level: bool,
    report: &mut MergeReport,
) -> Result<(), HomeError> {
    let entries = fs::read_dir(src).map_err(|err| HomeError::io("reading directory", src, err))?;
    let mut names: Vec<OsString> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| HomeError::io("reading directory", src, err))?;
        names.push(entry.file_name());
    }
    names.sort();
    // The SQLite database and its sidecar files travel together or not at all.
    let keep_state_db = top_level && dst.join("state.db").exists();
    for name in names {
        let from = src.join(&name);
        let to = dst.join(&name);
        if keep_state_db && is_state_db_name(&name) {
            report.left_behind.push(from);
            continue;
        }
        let from_meta =
            fs::symlink_metadata(&from).map_err(|err| HomeError::io("inspecting", &from, err))?;
        match fs::symlink_metadata(&to) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                fs::rename(&from, &to).map_err(|err| HomeError::io("moving", &from, err))?;
                report.moved += 1;
            }
            Err(err) => return Err(HomeError::io("inspecting", &to, err)),
            Ok(to_meta) if from_meta.is_dir() && to_meta.is_dir() => {
                merge_dir(&from, &to, false, report)?;
            }
            Ok(to_meta) if from_meta.is_file() && to_meta.is_file() => {
                if same_content(&from, &to)? {
                    fs::remove_file(&from)
                        .map_err(|err| HomeError::io("removing duplicate", &from, err))?;
                } else {
                    report.left_behind.push(from);
                }
            }
            Ok(_) => report.left_behind.push(from),
        }
    }
    // Succeeds only if nothing was left behind; otherwise the directory stays, by design.
    let _ = fs::remove_dir(src);
    Ok(())
}

fn same_content(a: &Path, b: &Path) -> Result<bool, HomeError> {
    let meta_a = fs::metadata(a).map_err(|err| HomeError::io("inspecting", a, err))?;
    let meta_b = fs::metadata(b).map_err(|err| HomeError::io("inspecting", b, err))?;
    if meta_a.len() != meta_b.len() {
        return Ok(false);
    }
    let bytes_a = fs::read(a).map_err(|err| HomeError::io("reading", a, err))?;
    let bytes_b = fs::read(b).map_err(|err| HomeError::io("reading", b, err))?;
    Ok(bytes_a == bytes_b)
}

/// The directory layout of one repository's state: `ACS_HOME/repos/<repo-id>/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoHome {
    id: RepoId,
    dir: PathBuf,
}

impl RepoHome {
    /// The repository this directory belongs to.
    #[must_use]
    pub fn id(&self) -> &RepoId {
        &self.id
    }

    /// `repos/<repo-id>`.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `repos/<repo-id>/state.db`. A file; [`RepoHome::ensure`] does not create it.
    #[must_use]
    pub fn state_db(&self) -> PathBuf {
        self.dir.join("state.db")
    }

    /// `repos/<repo-id>/snapshots`: content-addressed blobs.
    #[must_use]
    pub fn snapshots_dir(&self) -> PathBuf {
        self.dir.join("snapshots")
    }

    /// `repos/<repo-id>/cache`: compiled policy.
    #[must_use]
    pub fn cache_dir(&self) -> PathBuf {
        self.dir.join("cache")
    }

    /// `repos/<repo-id>/pending`: pending-commit markers and proposed lore.
    #[must_use]
    pub fn pending_dir(&self) -> PathBuf {
        self.dir.join("pending")
    }

    /// Creates the repository directory and its `snapshots/`, `cache/` and `pending/`
    /// subdirectories as owner-only directories (parents included). Idempotent; never creates
    /// `state.db`.
    ///
    /// # Errors
    ///
    /// [`HomeError::Io`] if a directory cannot be created or tightened.
    pub fn ensure(&self) -> Result<(), HomeError> {
        // Parents first, so `ACS_HOME` and `repos/` get the same treatment as the leaves.
        if let Some(repos) = self.dir.parent() {
            if let Some(root) = repos.parent() {
                ensure_dir(root)?;
            }
            ensure_dir(repos)?;
        }
        for dir in [
            &self.dir,
            &self.snapshots_dir(),
            &self.cache_dir(),
            &self.pending_dir(),
        ] {
            ensure_dir(dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(path: &str) -> PathBuf {
        // Absolute on both Unix and Windows.
        if cfg!(windows) {
            PathBuf::from(format!("C:\\{path}"))
        } else {
            PathBuf::from(format!("/{path}"))
        }
    }

    #[test]
    fn env_value_wins_when_set_and_absolute() {
        let env = abs("data/acs").into_os_string();
        let home = AcsHome::resolve(Some(&env), Some(&abs("users/sara"))).unwrap();
        assert_eq!(home.root(), abs("data/acs"));
    }

    #[test]
    fn unset_or_empty_env_uses_dot_acs_in_the_user_home() {
        let user = abs("users/sara");
        for env in [None, Some(OsString::new())] {
            let home = AcsHome::resolve(env.as_deref(), Some(&user)).unwrap();
            assert_eq!(home.root(), user.join(".acs"));
        }
    }

    #[test]
    fn relative_env_value_is_rejected_with_a_clear_message() {
        let err = AcsHome::resolve(Some(OsStr::new("relative/dir")), Some(&abs("u"))).unwrap_err();
        assert!(matches!(err, HomeError::NotAbsolute(_)));
        let message = err.to_string();
        assert!(message.contains("absolute"), "{message}");
        assert!(message.contains("relative"), "{message}");
    }

    #[test]
    fn missing_home_dir_is_an_error_only_when_needed() {
        assert!(matches!(
            AcsHome::resolve(None, None),
            Err(HomeError::NoHomeDir)
        ));
        assert!(matches!(
            AcsHome::resolve(None, Some(Path::new(""))),
            Err(HomeError::NoHomeDir)
        ));
        let env = abs("x").into_os_string();
        assert!(AcsHome::resolve(Some(&env), None).is_ok());
    }

    #[test]
    fn layout_accessors() {
        let home = AcsHome::at(abs("h")).unwrap();
        let id = RepoId::from_roots(["a".repeat(40)]);
        let repo = home.repo(&id);
        let base = abs("h").join("repos").join(id.as_str());
        assert_eq!(repo.dir(), base);
        assert_eq!(repo.state_db(), base.join("state.db"));
        assert_eq!(repo.snapshots_dir(), base.join("snapshots"));
        assert_eq!(repo.cache_dir(), base.join("cache"));
        assert_eq!(repo.pending_dir(), base.join("pending"));
        assert_eq!(home.index_dir(), abs("h").join("index"));
    }
}
