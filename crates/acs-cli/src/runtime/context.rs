//! Opening a repository: [`RepoContext`].

use std::cell::OnceCell;
use std::path::Path;

use acs_core::gate::PolicyView;
use acs_git::{GitError, RepoLayout};
use acs_paths::{PathResolver, PathsError};
use acs_policy::{CompiledPolicy, LoadOptions, LoadOutcome, Origin};
use acs_store::{AcsHome, HomeError, RepoHome, RepoIdentity, SnapshotStore, StateDb};

/// Why a repository could not be opened at all. Problems with the state database, `ACS_HOME` or
/// the snapshot store are not errors here: they are reported by [`RepoContext::db`] and
/// [`RepoContext::snapshots`], so the hook can still enforce what needs no state.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The directory is not inside a git work tree.
    #[error(transparent)]
    NotARepo(#[from] GitError),
    /// The work tree root cannot be used for path resolution.
    #[error(transparent)]
    Paths(#[from] PathsError),
}

/// A repository opened for one `acs` invocation.
pub struct RepoContext {
    /// Where the work tree and its git directories are.
    pub layout: RepoLayout,
    /// Normalizes raw paths into repository paths.
    pub resolver: PathResolver,
    /// The compiled policy, how it was obtained, and its warnings.
    pub policy: LoadOutcome,
    /// Whether the repository has a git remote configured.
    pub has_remote: bool,
    /// `ACS_HOME`, when it could be resolved.
    pub acs_home: Option<AcsHome>,
    /// This repository's state directory, when the repository id could be resolved.
    pub repo_home: Option<RepoHome>,
    db: Result<StateDb, String>,
    snapshots: OnceCell<Result<SnapshotStore, String>>,
}

impl std::fmt::Debug for RepoContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepoContext")
            .field("work_tree", &self.layout.work_tree)
            .field("db_ok", &self.db.is_ok())
            .finish_non_exhaustive()
    }
}

impl RepoContext {
    /// Opens the repository that contains `cwd`.
    ///
    /// Resolution order: work tree, path resolver, `ACS_HOME` and repository id, state database,
    /// policy (through the cache in the repository's state directory), remote. A failure to
    /// resolve the id or open the database leaves [`RepoContext::db`] in its error state and the
    /// policy is then loaded without the cache.
    ///
    /// # Errors
    ///
    /// [`OpenError`] when `cwd` is not in a git work tree.
    pub fn open(cwd: &Path) -> Result<Self, OpenError> {
        Self::open_with(cwd, AcsHome::from_env())
    }

    /// Like [`RepoContext::open`] with an explicit `ACS_HOME` resolution (tests, and callers
    /// that already resolved it). An `Err` leaves the state unavailable.
    ///
    /// # Errors
    ///
    /// [`OpenError`] when `cwd` is not in a git work tree.
    pub fn open_with(cwd: &Path, home: Result<AcsHome, HomeError>) -> Result<Self, OpenError> {
        let layout = acs_git::discover(cwd)?;
        let resolver = PathResolver::new(&layout.work_tree)?;
        let has_remote = acs_git::repo::has_remote(&layout.common_dir);

        let mut acs_home = None;
        let mut repo_home = None;
        let db = match home {
            Err(err) => Err(err.to_string()),
            Ok(home) => {
                let opened = RepoIdentity::resolve(&home, &layout.work_tree)
                    .map_err(|err| err.to_string())
                    .and_then(|identity| {
                        let repo = identity.home(&home);
                        repo_home = Some(repo.clone());
                        StateDb::open(&repo).map_err(|err| err.to_string())
                    });
                acs_home = Some(home);
                opened
            }
        };

        let policy_dir = layout.work_tree.join(".acs").join("policy");
        let opts = LoadOptions {
            case_insensitive: resolver.case_insensitive(),
        };
        let policy = match &repo_home {
            Some(repo) => {
                // `StateDb::open` creates the directory; if it failed, try on our own so the
                // policy cache still works.
                if db.is_err() {
                    let _ = repo.ensure();
                }
                acs_policy::load_cached(&policy_dir, &repo.cache_dir(), opts)
            }
            None => load_uncached(&policy_dir, opts),
        };

        Ok(Self {
            layout,
            resolver,
            policy,
            has_remote,
            acs_home,
            repo_home,
            db,
            snapshots: OnceCell::new(),
        })
    }

    /// The compiled policy as the engine's view of it.
    #[must_use]
    pub fn policy_view(&self) -> &dyn PolicyView {
        &self.policy.policy
    }

    /// The state database, or why it is unavailable (the hook maps the latter to
    /// `GateState::state_unavailable`).
    ///
    /// # Errors
    ///
    /// The message of the failure that made the database unusable.
    pub fn db(&self) -> Result<&StateDb, &str> {
        self.db.as_ref().map_err(String::as_str)
    }

    /// The snapshot store, opened on first use.
    ///
    /// # Errors
    ///
    /// The message of the failure that made it unusable.
    pub fn snapshots(&self) -> Result<&SnapshotStore, &str> {
        self.snapshots
            .get_or_init(|| {
                let Some(repo) = &self.repo_home else {
                    return Err("the repository's state directory is unavailable".to_owned());
                };
                SnapshotStore::open(repo).map_err(|err| err.to_string())
            })
            .as_ref()
            .map_err(String::as_str)
    }

    /// The work tree root as a string (the seat's `worktree`).
    #[must_use]
    pub fn worktree_str(&self) -> String {
        self.layout.work_tree.to_string_lossy().into_owned()
    }
}

/// Loads the policy when there is no state directory to cache in: strictly from disk, falling
/// back to the built-in defaults (with a warning) when the files are invalid.
fn load_uncached(policy_dir: &Path, opts: LoadOptions) -> LoadOutcome {
    match acs_policy::load(policy_dir, opts) {
        Ok(loaded) => LoadOutcome {
            policy: loaded.policy,
            origin: Origin::Fresh,
            warnings: loaded.warnings,
        },
        Err(err) => LoadOutcome {
            policy: CompiledPolicy::builtin(opts.case_insensitive),
            origin: Origin::Defaults,
            warnings: vec![format!(
                "the policy files are invalid ({err}); only the built-in default guards apply"
            )],
        },
    }
}
