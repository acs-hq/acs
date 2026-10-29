//! Repository identity: the `repo-id` that names `ACS_HOME/repos/<repo-id>/`.
//!
//! A repository's id is the lowercase hex SHA-256 of its root-commit hashes, sorted ascending
//! and joined with `\n`. Clones and all linked worktrees of a repository therefore share one
//! id, and so one state directory. A repository with no commits yet gets an id derived from its
//! canonical git directory, and its state moves to the commit-derived id once it has commits
//! (see [`RepoIdentity::migration`]).
//!
//! Resolving an id from scratch walks history with `git`, which is slow on big repositories, so
//! the result is cached under `ACS_HOME/index/`. A cached lookup reads a few small files and
//! spawns no process. See `docs/decisions/0003-acs-home-and-repo-id.md` for the root-selection
//! rule, the cache design and the alternatives that were rejected.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use acs_core::ContentHash;
use serde::{Deserialize, Serialize};

use crate::fsutil::write_private_atomic;
use crate::home::{AcsHome, HomeError, Migration, RepoHome};

/// The identity of a repository on this machine: 64 lowercase hex characters.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoId(String);

/// A string that is not a valid [`RepoId`].
#[derive(Debug, thiserror::Error)]
#[error("invalid repo id {0:?}: expected 64 lowercase hex characters")]
pub struct InvalidRepoId(String);

impl RepoId {
    /// The id for a set of root commits: SHA-256 of the lowercased hashes, sorted ascending
    /// (duplicates removed) and joined with `\n`. The order of `roots` does not matter.
    #[must_use]
    pub fn from_roots<I, S>(roots: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut roots: Vec<String> = roots
            .into_iter()
            .map(|root| root.as_ref().to_ascii_lowercase())
            .collect();
        roots.sort();
        roots.dedup();
        Self(
            ContentHash::of(roots.join("\n").as_bytes())
                .as_str()
                .to_owned(),
        )
    }

    /// The fallback id for a repository without commits: SHA-256 of `path:` followed by the
    /// canonical git directory (see [`find_common_dir`]). On Windows and macOS, whose default
    /// filesystems ignore case, the path is lowercased first.
    #[must_use]
    pub fn from_path(common_dir: &Path) -> Self {
        let mut key = b"path:".to_vec();
        key.extend(path_key(common_dir));
        Self(ContentHash::of(&key).as_str().to_owned())
    }

    /// The id as a lowercase hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RepoId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for RepoId {
    type Err = InvalidRepoId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if is_lower_hex(s, &[64]) {
            Ok(Self(s.to_owned()))
        } else {
            Err(InvalidRepoId(s.to_owned()))
        }
    }
}

fn is_lower_hex(s: &str, lengths: &[usize]) -> bool {
    lengths.contains(&s.len()) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Bytes that identify `path` for hashing and cache keys.
///
/// Unix uses the raw bytes. Elsewhere the path is converted lossily to UTF-8. On Windows and
/// macOS it is also lowercased, because the default filesystems there are case-insensitive and
/// two spellings of one directory must give one key. (A case-sensitive volume on those systems
/// could in theory map two directories to one key; the consequence is two empty repositories
/// sharing a temporary state directory, and nothing for repositories with commits.)
fn path_key(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = path.to_string_lossy().into_owned().into_bytes();
    if cfg!(any(windows, target_os = "macos")) {
        String::from_utf8_lossy(&bytes).to_lowercase().into_bytes()
    } else {
        bytes
    }
}

/// How a [`RepoIdentity`] was derived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdKind {
    /// From the root commits of a complete history. Stable across clones and worktrees.
    Commits,
    /// From the root commits visible in a **shallow** clone, whose "roots" are the shallow
    /// boundary and not real roots. The id is provisional: it differs from the id of the full
    /// history, and from other shallow clones cut at a different depth. State stored under it is
    /// not migrated when the clone is deepened.
    Shallow,
    /// From the git directory's path, because the repository has no commits yet. Replaced by a
    /// [`IdKind::Commits`] id (and the state migrated) once it has some.
    Path,
}

/// Where the id of a [`RepoIdentity`] came from on this call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdSource {
    /// Read from the cache under `ACS_HOME/index/`; no history was read and no process spawned.
    Cache,
    /// Computed now (and cached, when it is a commit-derived id).
    Computed,
}

/// The resolved identity of a repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoIdentity {
    id: RepoId,
    kind: IdKind,
    source: IdSource,
    common_dir: PathBuf,
    roots: Vec<String>,
    migration: Option<Migration>,
    migration_error: Option<String>,
}

impl RepoIdentity {
    /// Resolves the identity of the repository containing `start`, using the `git` on `PATH`
    /// for the uncached path. See [`RepoIdentity::resolve_with`].
    ///
    /// # Errors
    ///
    /// See [`RepoIdError`].
    pub fn resolve(home: &AcsHome, start: &Path) -> Result<Self, RepoIdError> {
        Self::resolve_with(home, start, &GitCli::default())
    }

    /// Resolves the identity of the repository containing `start` (any path inside a work tree,
    /// a linked worktree, a bare repository or the git directory itself).
    ///
    /// 1. Find the common git directory (see [`find_common_dir`]).
    /// 2. If the cache under `home` has an entry that is still valid, return it. No process is
    ///    spawned and no history is read.
    /// 3. If the repository has no branches or remote-tracking refs, return the path-derived id
    ///    (a cheap file check, no process).
    /// 4. Otherwise ask `finder` for the root commits, derive the id, migrate state from the
    ///    path-derived id if it exists (complete histories only), and cache the result.
    ///
    /// Nothing under `home` is created for repositories without commits; callers create the
    /// state directory with [`RepoHome::ensure`].
    ///
    /// # Errors
    ///
    /// See [`RepoIdError`]. A failed cache write is not an error: the cache is an optimization.
    /// A failed *migration* is not an error either: it is reported in
    /// [`RepoIdentity::migration`]'s absence and the result is not cached, so the next call
    /// retries it.
    pub fn resolve_with(
        home: &AcsHome,
        start: &Path,
        finder: &dyn RootFinder,
    ) -> Result<Self, RepoIdError> {
        let common_dir = find_common_dir(start)?;
        let stamp = Stamp::of(&common_dir);
        let cache = Cache::new(home, &common_dir);
        if let Some(entry) = cache.read(&stamp) {
            return Ok(Self {
                id: entry.id,
                kind: entry.kind.into(),
                source: IdSource::Cache,
                common_dir,
                roots: entry.roots,
                migration: None,
                migration_error: None,
            });
        }
        let path_identity = |common_dir: PathBuf| Self {
            id: RepoId::from_path(&common_dir),
            kind: IdKind::Path,
            source: IdSource::Computed,
            common_dir,
            roots: Vec::new(),
            migration: None,
            migration_error: None,
        };
        if !may_have_commits(&common_dir) {
            return Ok(path_identity(common_dir));
        }
        let roots = finder.root_commits(&common_dir)?;
        if roots.is_empty() {
            return Ok(path_identity(common_dir));
        }
        let id = RepoId::from_roots(&roots);
        let (kind, cache_kind) = if stamp.shallow {
            (IdKind::Shallow, CacheKind::Shallow)
        } else {
            (IdKind::Commits, CacheKind::Commits)
        };
        let mut migration = None;
        if kind == IdKind::Commits {
            let from = RepoId::from_path(&common_dir);
            match home.migrate_repo(&from, &id) {
                Ok(Migration::NotNeeded) => {}
                Ok(done) => migration = Some(done),
                // Do not cache: the next resolve retries the migration.
                Err(err) => {
                    return Ok(Self {
                        id,
                        kind,
                        source: IdSource::Computed,
                        common_dir,
                        roots,
                        migration: None,
                        migration_error: Some(err.to_string()),
                    });
                }
            }
        }
        cache.write(&CacheEntry {
            version: CACHE_VERSION,
            common_dir: common_dir.to_string_lossy().into_owned(),
            id: id.clone(),
            kind: cache_kind,
            roots: roots.clone(),
            stamp: stamp.text,
        });
        Ok(Self {
            id,
            kind,
            source: IdSource::Computed,
            common_dir,
            roots,
            migration,
            migration_error: None,
        })
    }

    /// The repository id.
    #[must_use]
    pub fn id(&self) -> &RepoId {
        &self.id
    }

    /// How the id was derived.
    #[must_use]
    pub fn kind(&self) -> IdKind {
        self.kind
    }

    /// Whether this call used the cache or computed the id.
    #[must_use]
    pub fn source(&self) -> IdSource {
        self.source
    }

    /// The canonical common git directory shared by all worktrees of the repository.
    #[must_use]
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// The root commits the id was derived from, sorted. Empty for [`IdKind::Path`].
    #[must_use]
    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    /// Set when this call moved state from the path-derived id to the commit-derived id. Only
    /// the call that performs the move reports it; later calls return `None`. When it is
    /// [`Migration::Merged`] with a non-empty `left_behind`, some old files conflicted with
    /// existing state and were kept in the old directory.
    #[must_use]
    pub fn migration(&self) -> Option<&Migration> {
        self.migration.as_ref()
    }

    /// Set when moving state from the path-derived id failed. The state is intact in the old
    /// directory and the move is retried on the next resolve.
    #[must_use]
    pub fn migration_error(&self) -> Option<&str> {
        self.migration_error.as_deref()
    }

    /// The state directory layout for this repository under `home`. Nothing is created.
    #[must_use]
    pub fn home(&self, home: &AcsHome) -> RepoHome {
        home.repo(&self.id)
    }
}

/// Why a repository identity could not be resolved.
#[derive(Debug, thiserror::Error)]
pub enum RepoIdError {
    /// No git repository contains the path.
    #[error("{} is not inside a git repository", .0.display())]
    NotARepository(PathBuf),
    /// A `.git` file or directory exists but does not describe a usable git directory.
    #[error("{} is not a valid git directory: {reason}", .path.display())]
    InvalidGitDir {
        /// The offending path.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// The `git` executable could not be started.
    #[error("cannot run git ({program}): {source}; is git installed and on PATH?")]
    GitUnavailable {
        /// The program that was tried.
        program: String,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// `git` ran and failed.
    #[error("git failed in {} ({status}): {stderr}", .common_dir.display())]
    GitFailed {
        /// The git directory it was run against.
        common_dir: PathBuf,
        /// The exit status, rendered.
        status: String,
        /// Trimmed standard error.
        stderr: String,
    },
    /// `git` printed something that is not a list of commit hashes.
    #[error("unexpected output from git: {0:?}")]
    BadGitOutput(String),
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
    /// Creating or moving state under `ACS_HOME` failed.
    #[error(transparent)]
    Home(#[from] HomeError),
}

impl RepoIdError {
    fn io(action: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Finding the common git directory
// ---------------------------------------------------------------------------------------------

/// Finds the common git directory of the repository containing `start`.
///
/// `start` may be any path inside a work tree (it need not exist yet: the nearest existing
/// ancestor is used), a linked worktree, a bare repository, or inside a git directory. The
/// result is canonical (symlinks resolved, no `\\?\` prefix on Windows) and is the same for
/// the main work tree and every linked worktree: for a linked worktree the `.git` file points
/// at `<common>/worktrees/<name>`, whose `commondir` file points back at `<common>`. The
/// `GIT_DIR` family of environment variables is deliberately ignored, so that a call made from
/// inside a git hook still describes the repository containing `start`.
///
/// # Errors
///
/// [`RepoIdError::NotARepository`] if no ancestor of `start` is a repository,
/// [`RepoIdError::InvalidGitDir`] if a `.git` file is malformed, and [`RepoIdError::Io`] for
/// filesystem failures.
pub fn find_common_dir(start: &Path) -> Result<PathBuf, RepoIdError> {
    let start = canonicalize_existing_prefix(start)?;
    for dir in start.ancestors() {
        let dotgit = dir.join(".git");
        match fs::metadata(&dotgit) {
            Ok(meta) if meta.is_dir() => {
                if looks_like_git_dir(&dotgit) {
                    return common_dir_of(&dotgit);
                }
            }
            Ok(meta) if meta.is_file() => {
                let gitdir = read_gitdir_file(&dotgit)?;
                return common_dir_of(&dir.join(gitdir));
            }
            _ => {}
        }
        // A bare repository, or a path inside a git directory.
        if looks_like_git_dir(dir) {
            return common_dir_of(dir);
        }
    }
    Err(RepoIdError::NotARepository(start))
}

fn canonicalize_existing_prefix(path: &Path) -> Result<PathBuf, RepoIdError> {
    let mut existing = path;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        match dunce::canonicalize(existing) {
            Ok(mut canonical) => {
                canonical.extend(tail.iter().rev());
                return Ok(canonical);
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                match (existing.parent(), existing.file_name()) {
                    (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
                        tail.push(name);
                        existing = parent;
                    }
                    _ => return Err(RepoIdError::io("resolving", path, err)),
                }
            }
            Err(err) => return Err(RepoIdError::io("resolving", path, err)),
        }
    }
}

/// A directory with the layout of a git directory: `HEAD` file plus `objects/` and `refs/`
/// (or a `commondir` file, for a linked worktree's private directory).
fn looks_like_git_dir(dir: &Path) -> bool {
    if !dir.join("HEAD").is_file() {
        return false;
    }
    dir.join("commondir").is_file() || (dir.join("objects").is_dir() && dir.join("refs").is_dir())
}

/// Reads `gitdir: <path>` from a `.git` file. The path may be relative to the file's directory.
fn read_gitdir_file(dotgit: &Path) -> Result<PathBuf, RepoIdError> {
    let text = fs::read_to_string(dotgit).map_err(|err| RepoIdError::io("reading", dotgit, err))?;
    text.lines()
        .next()
        .and_then(|line| line.strip_prefix("gitdir:"))
        .map(|rest| PathBuf::from(rest.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| RepoIdError::InvalidGitDir {
            path: dotgit.to_path_buf(),
            reason: "expected a line of the form `gitdir: <path>`".to_owned(),
        })
}

/// Canonicalizes a git directory and follows its `commondir` file, if any.
fn common_dir_of(gitdir: &Path) -> Result<PathBuf, RepoIdError> {
    let gitdir = dunce::canonicalize(gitdir).map_err(|err| {
        if err.kind() == io::ErrorKind::NotFound {
            RepoIdError::InvalidGitDir {
                path: gitdir.to_path_buf(),
                reason: "the directory does not exist".to_owned(),
            }
        } else {
            RepoIdError::io("resolving", gitdir, err)
        }
    })?;
    let common = match fs::read_to_string(gitdir.join("commondir")) {
        Ok(text) => {
            let target = gitdir.join(text.trim());
            dunce::canonicalize(&target)
                .map_err(|err| RepoIdError::io("resolving", &target, err))?
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => gitdir,
        Err(err) => return Err(RepoIdError::io("reading", &gitdir.join("commondir"), err)),
    };
    if common.join("HEAD").is_file() {
        Ok(common)
    } else {
        Err(RepoIdError::InvalidGitDir {
            path: common,
            reason: "no HEAD file".to_owned(),
        })
    }
}

/// Cheap, process-free check for "might this repository have commits": any loose or packed
/// branch or remote-tracking ref, or a reftable. Errs on the side of `true`; the caller then
/// asks git, which gives the definitive answer.
fn may_have_commits(common_dir: &Path) -> bool {
    if common_dir.join("reftable").is_dir() {
        return true;
    }
    if has_file_below(&common_dir.join("refs").join("heads"), 0)
        || has_file_below(&common_dir.join("refs").join("remotes"), 0)
    {
        return true;
    }
    fs::read_to_string(common_dir.join("packed-refs")).is_ok_and(|text| {
        text.lines().any(|line| {
            line.split_once(' ').is_some_and(|(_, name)| {
                name.starts_with("refs/heads/") || name.starts_with("refs/remotes/")
            })
        })
    })
}

fn has_file_below(dir: &Path, depth: usize) -> bool {
    const MAX_DEPTH: usize = 16;
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth < MAX_DEPTH && has_file_below(&entry.path(), depth + 1) {
                return true;
            }
        } else {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------
// Root-commit discovery
// ---------------------------------------------------------------------------------------------

/// Finds the root commits of a repository. Abstracted so tests can count how often history is
/// read.
pub trait RootFinder {
    /// The root commits (commits without parents) reachable from the repository's branches and
    /// remote-tracking refs, as lowercase hex hashes, sorted and without duplicates.
    ///
    /// `refs/remotes/*/acs/*` is excluded: those are ACS's own claim and ledger refs, whose
    /// histories are unrelated to the code. An empty result means the repository has no
    /// commits.
    ///
    /// # Errors
    ///
    /// [`RepoIdError::GitUnavailable`], [`RepoIdError::GitFailed`] or
    /// [`RepoIdError::BadGitOutput`].
    fn root_commits(&self, common_dir: &Path) -> Result<Vec<String>, RepoIdError>;
}

/// Root-commit discovery by running `git rev-list --max-parents=0`.
#[derive(Clone, Debug)]
pub struct GitCli {
    program: OsString,
}

impl Default for GitCli {
    fn default() -> Self {
        Self {
            program: OsString::from("git"),
        }
    }
}

impl GitCli {
    /// Uses the given git executable instead of the one found on `PATH`.
    #[must_use]
    pub fn with_program(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

/// Environment variables that would redirect git away from the directory we pass it. They are
/// set, for example, while git runs a hook.
const GIT_REDIRECT_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_GRAFT_FILE",
    "GIT_SHALLOW_FILE",
    "GIT_REPLACE_REF_BASE",
];

impl RootFinder for GitCli {
    fn root_commits(&self, common_dir: &Path) -> Result<Vec<String>, RepoIdError> {
        let mut command = Command::new(&self.program);
        for var in GIT_REDIRECT_ENV {
            command.env_remove(var);
        }
        command
            // Replacement refs and grafts would make the roots differ between clones.
            .arg("--no-replace-objects")
            .arg("--git-dir")
            .arg(common_dir)
            .args([
                "rev-list",
                "--max-parents=0",
                "--branches",
                "--exclude=*/acs/*",
            ])
            .arg("--remotes")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: do not flash a console when called from a GUI harness.
            command.creation_flags(0x0800_0000);
        }
        let output = command
            .output()
            .map_err(|source| RepoIdError::GitUnavailable {
                program: self.program.to_string_lossy().into_owned(),
                source,
            })?;
        if !output.status.success() {
            return Err(RepoIdError::GitFailed {
                common_dir: common_dir.to_path_buf(),
                status: output.status.to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut roots = Vec::new();
        for line in stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if !is_lower_hex(line, &[40, 64]) {
                return Err(RepoIdError::BadGitOutput(line.to_owned()));
            }
            roots.push(line.to_owned());
        }
        roots.sort();
        roots.dedup();
        Ok(roots)
    }
}

// ---------------------------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------------------------

const CACHE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum CacheKind {
    Commits,
    Shallow,
}

impl From<CacheKind> for IdKind {
    fn from(kind: CacheKind) -> Self {
        match kind {
            CacheKind::Commits => Self::Commits,
            CacheKind::Shallow => Self::Shallow,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    version: u32,
    common_dir: String,
    #[serde(serialize_with = "ser_id", deserialize_with = "de_id")]
    id: RepoId,
    kind: CacheKind,
    roots: Vec<String>,
    stamp: String,
}

fn ser_id<S: serde::Serializer>(id: &RepoId, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(id.as_str())
}

fn de_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<RepoId, D::Error> {
    let text = String::deserialize(deserializer)?;
    text.parse().map_err(serde::de::Error::custom)
}

/// What must still hold for a cache entry to be trusted: the git directory is the same
/// directory (not deleted and recreated at the same path) and the shallow boundary has not
/// moved.
struct Stamp {
    text: String,
    shallow: bool,
}

impl Stamp {
    fn of(common_dir: &Path) -> Self {
        let nanos = |time: io::Result<SystemTime>| {
            time.ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or_else(|| "-".to_owned(), |d| format!("{}", d.as_nanos()))
        };
        // Creation time of the git directory. Not available on every filesystem; then "-",
        // which only weakens detection of a repository recreated at the same path.
        let birth =
            fs::metadata(common_dir).map_or_else(|_| "-".to_owned(), |m| nanos(m.created()));
        let (shallow, shallow_text) = match fs::metadata(common_dir.join("shallow")) {
            Ok(meta) => (true, format!("{}:{}", meta.len(), nanos(meta.modified()))),
            Err(_) => (false, "-".to_owned()),
        };
        Self {
            text: format!("v1;birth={birth};shallow={shallow_text}"),
            shallow,
        }
    }
}

struct Cache {
    path: PathBuf,
    index_dir: PathBuf,
    common_dir: String,
}

impl Cache {
    fn new(home: &AcsHome, common_dir: &Path) -> Self {
        let key = ContentHash::of(&path_key(common_dir));
        let index_dir = home.index_dir();
        Self {
            path: index_dir.join(format!("{}.json", key.as_str())),
            index_dir,
            common_dir: common_dir.to_string_lossy().into_owned(),
        }
    }

    /// The entry, if present, well-formed, internally consistent and still matching `stamp`.
    /// Any problem means "no entry"; the cache can always be rebuilt.
    fn read(&self, stamp: &Stamp) -> Option<CacheEntry> {
        let bytes = fs::read(&self.path).ok()?;
        let entry: CacheEntry = serde_json::from_slice(&bytes).ok()?;
        let consistent = entry.version == CACHE_VERSION
            && entry.common_dir == self.common_dir
            && entry.stamp == stamp.text
            && !entry.roots.is_empty()
            && entry.roots.iter().all(|root| is_lower_hex(root, &[40, 64]))
            && RepoId::from_roots(&entry.roots) == entry.id
            && (entry.kind == CacheKind::Shallow) == stamp.shallow;
        consistent.then_some(entry)
    }

    /// Best effort: failing to cache only costs time on the next call.
    fn write(&self, entry: &CacheEntry) {
        let Ok(bytes) = serde_json::to_vec(entry) else {
            return;
        };
        if crate::fsutil::create_private_dir_all(&self.index_dir).is_ok() {
            let _ = write_private_atomic(&self.path, &bytes);
        }
    }
}
