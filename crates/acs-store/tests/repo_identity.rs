//! Repository identity against real git repositories.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use acs_store::{
    AcsHome, GitCli, IdKind, IdSource, Migration, RepoId, RepoIdError, RepoIdentity, RootFinder,
    find_common_dir,
};
use common::Sandbox;

/// A finder that counts how often history is read.
struct Counting {
    inner: GitCli,
    calls: Cell<usize>,
}

impl Counting {
    fn new() -> Self {
        Self {
            inner: GitCli::default(),
            calls: Cell::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.get()
    }
}

impl RootFinder for Counting {
    fn root_commits(&self, common_dir: &Path) -> Result<Vec<String>, RepoIdError> {
        self.calls.set(self.calls.get() + 1);
        self.inner.root_commits(common_dir)
    }
}

fn new_home(sandbox: &Sandbox) -> AcsHome {
    AcsHome::at(sandbox.join("acs-home")).unwrap()
}

fn resolve(home: &AcsHome, path: &Path) -> RepoIdentity {
    RepoIdentity::resolve(home, path).unwrap()
}

/// An origin repository with a few commits, two clones and three linked worktrees.
#[test]
fn two_clones_and_three_worktrees_share_one_repo_id() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let origin = sandbox.init("origin");
    let root = sandbox.commit_file(&origin, "README.md", "hello\n");
    sandbox.commit_file(&origin, "src/lib.rs", "// lib\n");
    sandbox.commit_file(&origin, "src/main.rs", "fn main() {}\n");

    let clone_a = sandbox.join("clone-a");
    let clone_b = sandbox.join("clone-b");
    sandbox.git(
        sandbox.path(),
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            clone_a.to_str().unwrap(),
        ],
    );
    sandbox.git(
        sandbox.path(),
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            clone_b.to_str().unwrap(),
        ],
    );

    let wt1 = sandbox.join("wt1");
    let wt2 = sandbox.join("wt2");
    let wt3 = sandbox.join("wt3");
    sandbox.git(
        &origin,
        &[
            "worktree",
            "add",
            "--quiet",
            wt1.to_str().unwrap(),
            "-b",
            "feature-1",
        ],
    );
    sandbox.git(
        &origin,
        &[
            "worktree",
            "add",
            "--quiet",
            wt2.to_str().unwrap(),
            "-b",
            "feature-2",
        ],
    );
    sandbox.git(
        &clone_a,
        &[
            "worktree",
            "add",
            "--quiet",
            wt3.to_str().unwrap(),
            "-b",
            "feature-3",
        ],
    );

    let expected = RepoId::from_roots([&root]);
    for path in [&origin, &clone_a, &clone_b, &wt1, &wt2, &wt3] {
        let identity = resolve(&home, path);
        assert_eq!(identity.id(), &expected, "{}", path.display());
        assert_eq!(identity.kind(), IdKind::Commits);
        assert_eq!(identity.roots(), std::slice::from_ref(&root));
    }

    // Worktrees share the common directory of the repository they were added from.
    let main_common = find_common_dir(&origin).unwrap();
    assert_eq!(find_common_dir(&wt1).unwrap(), main_common);
    assert_eq!(find_common_dir(&wt2).unwrap(), main_common);
    assert_eq!(
        find_common_dir(&wt3).unwrap(),
        find_common_dir(&clone_a).unwrap()
    );
    assert_ne!(find_common_dir(&clone_a).unwrap(), main_common);
}

#[test]
fn id_is_the_documented_hash_of_the_root_commit() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let repo = sandbox.init("repo");
    let root = sandbox.commit_file(&repo, "a.txt", "a\n");
    sandbox.commit_file(&repo, "b.txt", "b\n");
    let identity = resolve(&home, &repo);
    // One root: SHA-256 of the 40-character hash with no trailing newline.
    let digest = acs_core::ContentHash::of(root.as_bytes());
    assert_eq!(identity.id().as_str(), digest.as_str());
}

#[test]
fn different_repositories_have_different_ids() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let a = sandbox.init("a");
    sandbox.commit_file(&a, "a.txt", "a\n");
    let b = sandbox.init("b");
    sandbox.commit_file(&b, "b.txt", "b\n");
    assert_ne!(resolve(&home, &a).id(), resolve(&home, &b).id());
}

#[test]
fn a_subdirectory_of_a_worktree_resolves_like_the_root() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let repo = sandbox.init("repo");
    sandbox.commit_file(&repo, "src/deep/er/file.rs", "x\n");
    let wt = sandbox.join("wt");
    sandbox.git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            wt.to_str().unwrap(),
            "-b",
            "wt",
        ],
    );

    let at_root = resolve(&home, &repo);
    for start in [
        repo.join("src"),
        repo.join("src/deep/er"),
        repo.join("src/deep/er/file.rs"),
        // A path that does not exist yet (a file about to be created).
        repo.join("src/new-dir/new-file.rs"),
        wt.join("src/deep"),
    ] {
        let identity = resolve(&home, &start);
        assert_eq!(identity.id(), at_root.id(), "{}", start.display());
        assert_eq!(identity.common_dir(), at_root.common_dir());
    }
    // From inside the git directory too.
    assert_eq!(
        resolve(&home, &repo.join(".git").join("hooks")).id(),
        at_root.id()
    );
}

#[test]
fn a_path_outside_any_repository_is_an_error() {
    let sandbox = Sandbox::new();
    let plain = sandbox.join("plain");
    fs::create_dir_all(&plain).unwrap();
    let err = RepoIdentity::resolve(&new_home(&sandbox), &plain).unwrap_err();
    assert!(matches!(err, RepoIdError::NotARepository(_)), "{err}");
}

#[test]
fn a_bare_repository_resolves_like_its_source() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let origin = sandbox.init("origin");
    sandbox.commit_file(&origin, "a.txt", "a\n");
    let bare = sandbox.join("bare.git");
    sandbox.git(
        sandbox.path(),
        &[
            "clone",
            "--quiet",
            "--bare",
            origin.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    assert_eq!(resolve(&home, &bare).id(), resolve(&home, &origin).id());
}

#[test]
fn history_with_two_roots_gives_an_order_independent_id() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);

    // Same two root commits (fixed dates, same messages), created in opposite orders.
    let first = sandbox.init("first");
    let r1 = sandbox.commit_empty(&first, "root one");
    sandbox.git(&first, &["checkout", "--quiet", "--orphan", "other"]);
    let r2 = sandbox.commit_empty(&first, "root two");

    let second = sandbox.init("second");
    sandbox.git(&second, &["checkout", "--quiet", "--orphan", "other"]);
    assert_eq!(sandbox.commit_empty(&second, "root two"), r2);
    sandbox.git(&second, &["checkout", "--quiet", "--orphan", "main"]);
    assert_eq!(sandbox.commit_empty(&second, "root one"), r1);

    let a = resolve(&home, &first);
    let b = resolve(&home, &second);
    assert_eq!(a.roots().len(), 2);
    assert_eq!(a.id(), b.id());
    assert_eq!(a.roots(), b.roots());
    // The documented rule, spelled out: sorted ascending, joined with a newline.
    let mut sorted = [r1.clone(), r2.clone()];
    sorted.sort();
    let digest = acs_core::ContentHash::of(sorted.join("\n").as_bytes());
    assert_eq!(a.id().as_str(), digest.as_str());
    assert_eq!(a.id(), &RepoId::from_roots([r2, r1]));
}

#[test]
fn acs_own_refs_do_not_change_the_repo_id() {
    let sandbox = Sandbox::new();
    let repo = sandbox.init("repo");
    let root = sandbox.commit_file(&repo, "a.txt", "a\n");

    // An unrelated history reachable only through ACS's ref namespaces.
    sandbox.git(&repo, &["checkout", "--quiet", "--orphan", "scratch"]);
    sandbox.git(&repo, &["rm", "--quiet", "-rf", "."]);
    let ledger_root = sandbox.commit_empty(&repo, "ledger");
    sandbox.git(
        &repo,
        &[
            "update-ref",
            "refs/acs/ledger/0123456789abcdef",
            &ledger_root,
        ],
    );
    sandbox.git(
        &repo,
        &[
            "update-ref",
            "refs/remotes/origin/acs/claims/0123456789abcdef",
            &ledger_root,
        ],
    );
    sandbox.git(&repo, &["checkout", "--quiet", "main"]);
    sandbox.git(&repo, &["branch", "--quiet", "-D", "scratch"]);

    let common = find_common_dir(&repo).unwrap();
    assert_eq!(GitCli::default().root_commits(&common).unwrap(), [root]);
}

#[test]
fn a_shallow_clone_gets_a_provisional_id() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let origin = sandbox.init("origin");
    sandbox.commit_file(&origin, "a.txt", "1\n");
    sandbox.commit_file(&origin, "a.txt", "2\n");
    sandbox.commit_file(&origin, "a.txt", "3\n");
    let shallow = sandbox.join("shallow");
    sandbox.git(
        sandbox.path(),
        &[
            "clone",
            "--quiet",
            "--no-local",
            "--depth",
            "1",
            origin.to_str().unwrap(),
            shallow.to_str().unwrap(),
        ],
    );
    let full = resolve(&home, &origin);
    let provisional = resolve(&home, &shallow);
    assert_eq!(provisional.kind(), IdKind::Shallow);
    assert_ne!(provisional.id(), full.id());
    // Cached, with the shallow marker preserved.
    let again = resolve(&home, &shallow);
    assert_eq!(again.source(), IdSource::Cache);
    assert_eq!(again.kind(), IdKind::Shallow);

    // Deepening changes the shallow file, which invalidates the entry.
    sandbox.git(&shallow, &["fetch", "--quiet", "--unshallow"]);
    let deep = resolve(&home, &shallow);
    assert_eq!(deep.kind(), IdKind::Commits);
    assert_eq!(deep.id(), full.id());
}

// ---- The cache ------------------------------------------------------------------------------

#[test]
fn a_cached_lookup_reads_no_history_and_spawns_no_process() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let repo = sandbox.init("repo");
    sandbox.commit_file(&repo, "a.txt", "a\n");
    let wt = sandbox.join("wt");
    sandbox.git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            wt.to_str().unwrap(),
            "-b",
            "wt",
        ],
    );

    let finder = Counting::new();
    let first = RepoIdentity::resolve_with(&home, &repo, &finder).unwrap();
    assert_eq!(first.source(), IdSource::Computed);
    assert_eq!(finder.calls(), 1);

    // Same worktree, a subdirectory, and a linked worktree: all served from the cache.
    for start in [&repo, &repo.join("sub"), &wt] {
        let again = RepoIdentity::resolve_with(&home, start, &finder).unwrap();
        assert_eq!(again.source(), IdSource::Cache, "{}", start.display());
        assert_eq!(again.id(), first.id());
        assert_eq!(again.roots(), first.roots());
    }
    assert_eq!(finder.calls(), 1);

    // A different home has its own cache.
    let other_home = AcsHome::at(sandbox.join("other-home")).unwrap();
    RepoIdentity::resolve_with(&other_home, &repo, &finder).unwrap();
    assert_eq!(finder.calls(), 2);
}

#[test]
fn the_cache_survives_agents_deleting_the_working_tree() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let repo = sandbox.init("repo");
    sandbox.commit_file(&repo, "src/a.rs", "a\n");
    let finder = Counting::new();
    let first = RepoIdentity::resolve_with(&home, &repo, &finder).unwrap();

    for entry in fs::read_dir(&repo).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap() != ".git" {
            if path.is_dir() {
                fs::remove_dir_all(path).unwrap();
            } else {
                fs::remove_file(path).unwrap();
            }
        }
    }
    let again = RepoIdentity::resolve_with(&home, &repo, &finder).unwrap();
    assert_eq!(again.id(), first.id());
    assert_eq!(again.source(), IdSource::Cache);
    assert_eq!(finder.calls(), 1);
}

#[test]
fn a_corrupt_or_foreign_cache_entry_is_recomputed() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let repo = sandbox.init("repo");
    sandbox.commit_file(&repo, "a.txt", "a\n");
    let finder = Counting::new();
    let first = RepoIdentity::resolve_with(&home, &repo, &finder).unwrap();

    let entries: Vec<PathBuf> = fs::read_dir(home.index_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "exactly one cache file and no leftover temp files"
    );

    for garbage in [
        "not json".to_owned(),
        String::new(),
        // Valid JSON but the id does not match the roots.
        fs::read_to_string(&entries[0])
            .unwrap()
            .replace(first.id().as_str(), &"0".repeat(64)),
    ] {
        fs::write(&entries[0], garbage).unwrap();
        let again = RepoIdentity::resolve_with(&home, &repo, &finder).unwrap();
        assert_eq!(again.source(), IdSource::Computed);
        assert_eq!(again.id(), first.id());
    }
    assert_eq!(finder.calls(), 4);
    // And the entry was repaired.
    let repaired = RepoIdentity::resolve_with(&home, &repo, &finder).unwrap();
    assert_eq!(repaired.source(), IdSource::Cache);
}

#[test]
fn a_repository_recreated_at_the_same_path_is_not_served_stale() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let repo = sandbox.init("repo");
    sandbox.commit_file(&repo, "a.txt", "first life\n");
    let first = resolve(&home, &repo);

    // Needs a filesystem that reports creation times; otherwise the stamp cannot tell.
    if fs::metadata(repo.join(".git"))
        .and_then(|m| m.created())
        .is_err()
    {
        return;
    }
    fs::remove_dir_all(repo.join(".git")).unwrap();
    sandbox.git(&repo, &["init", "--quiet"]);
    sandbox.commit_file(&repo, "b.txt", "second life\n");
    let second = resolve(&home, &repo);
    assert_ne!(second.id(), first.id());
    assert_eq!(second.source(), IdSource::Computed);
}

#[test]
fn a_missing_git_executable_is_a_clear_error() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let repo = sandbox.init("repo");
    sandbox.commit_file(&repo, "a.txt", "a\n");
    let finder = GitCli::with_program("acs-no-such-git-binary");
    let err = RepoIdentity::resolve_with(&home, &repo, &finder).unwrap_err();
    assert!(matches!(err, RepoIdError::GitUnavailable { .. }), "{err}");
    assert!(err.to_string().contains("PATH"));
}

// ---- Repositories without commits, and migration -----------------------------------------------

#[test]
fn an_empty_repository_uses_a_path_id_and_spawns_nothing() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let repo = sandbox.init("repo");
    let finder = Counting::new();
    let identity = RepoIdentity::resolve_with(&home, &repo, &finder).unwrap();
    assert_eq!(identity.kind(), IdKind::Path);
    assert!(identity.roots().is_empty());
    assert_eq!(identity.id(), &RepoId::from_path(identity.common_dir()));
    assert_eq!(finder.calls(), 0);
    assert!(
        !home.root().exists(),
        "resolving creates nothing for an empty repository"
    );

    // Stable, and the same from a subdirectory.
    fs::create_dir_all(repo.join("sub")).unwrap();
    assert_eq!(resolve(&home, &repo.join("sub")).id(), identity.id());
}

#[test]
fn the_first_commit_migrates_state_to_the_commit_derived_id() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let repo = sandbox.init("repo");

    let before = resolve(&home, &repo);
    assert_eq!(before.kind(), IdKind::Path);
    let old = before.home(&home);
    old.ensure().unwrap();
    fs::write(old.pending_dir().join("marker.json"), "{\"pending\":true}").unwrap();
    fs::write(old.snapshots_dir().join("blob"), b"snapshot bytes").unwrap();
    fs::write(old.state_db(), b"sqlite").unwrap();

    let root = sandbox.commit_file(&repo, "a.txt", "a\n");
    let after = resolve(&home, &repo);
    assert_eq!(after.kind(), IdKind::Commits);
    assert_eq!(after.id(), &RepoId::from_roots([&root]));
    assert_ne!(after.id(), before.id());
    assert_eq!(after.migration(), Some(&Migration::Moved));
    assert!(after.migration_error().is_none());

    let new = after.home(&home);
    assert!(!old.dir().exists(), "the old directory is gone");
    assert_eq!(
        fs::read(new.pending_dir().join("marker.json")).unwrap(),
        b"{\"pending\":true}"
    );
    assert_eq!(
        fs::read(new.snapshots_dir().join("blob")).unwrap(),
        b"snapshot bytes"
    );
    assert_eq!(fs::read(new.state_db()).unwrap(), b"sqlite");

    // Only the call that migrated reports it.
    let third = resolve(&home, &repo);
    assert_eq!(third.source(), IdSource::Cache);
    assert!(third.migration().is_none());
    assert_eq!(third.id(), after.id());
}

#[test]
fn migration_into_an_existing_directory_merges_without_losing_data() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let repo = sandbox.init("repo");

    let old = resolve(&home, &repo).home(&home);
    old.ensure().unwrap();
    fs::write(old.state_db(), b"old db").unwrap();
    fs::write(old.dir().join("state.db-wal"), b"old wal").unwrap();
    fs::write(old.pending_dir().join("a.txt"), b"old a").unwrap();
    fs::write(old.pending_dir().join("b.txt"), b"old b").unwrap();
    fs::write(old.snapshots_dir().join("same"), b"same bytes").unwrap();

    let root = sandbox.commit_file(&repo, "a.txt", "a\n");
    // Another clone of the repository already created state under the commit-derived id.
    let target = home.repo(&RepoId::from_roots([&root]));
    target.ensure().unwrap();
    fs::write(target.state_db(), b"target db").unwrap();
    fs::write(target.pending_dir().join("a.txt"), b"target a").unwrap();
    fs::write(target.snapshots_dir().join("same"), b"same bytes").unwrap();

    let identity = resolve(&home, &repo);
    assert_eq!(identity.id(), target.id());
    let Some(Migration::Merged { moved, left_behind }) = identity.migration() else {
        panic!("expected a merge, got {:?}", identity.migration());
    };
    assert_eq!(*moved, 1, "only pending/b.txt had no counterpart");
    let mut left: Vec<String> = left_behind
        .iter()
        .map(|p| {
            p.strip_prefix(old.dir())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    left.sort();
    assert_eq!(left, ["pending/a.txt", "state.db", "state.db-wal"]);

    // The target's own data is untouched; the database companions never mix.
    assert_eq!(fs::read(target.state_db()).unwrap(), b"target db");
    assert!(!target.dir().join("state.db-wal").exists());
    assert_eq!(
        fs::read(target.pending_dir().join("a.txt")).unwrap(),
        b"target a"
    );
    assert_eq!(
        fs::read(target.pending_dir().join("b.txt")).unwrap(),
        b"old b"
    );
    // Conflicting files are still in the old directory, byte for byte.
    assert_eq!(fs::read(old.state_db()).unwrap(), b"old db");
    assert_eq!(
        fs::read(old.dir().join("state.db-wal")).unwrap(),
        b"old wal"
    );
    assert_eq!(fs::read(old.pending_dir().join("a.txt")).unwrap(), b"old a");
    // The byte-identical duplicate was dropped from the old directory.
    assert!(!old.snapshots_dir().join("same").exists());
}

#[test]
fn a_failed_migration_is_reported_and_retried() {
    let sandbox = Sandbox::new();
    let home = new_home(&sandbox);
    let repo = sandbox.init("repo");
    let old = resolve(&home, &repo).home(&home);
    old.ensure().unwrap();
    fs::write(old.pending_dir().join("m"), b"m").unwrap();
    let root = sandbox.commit_file(&repo, "a.txt", "a\n");

    // Make the target path unusable: a regular file where the directory should be created.
    let target = home.repo(&RepoId::from_roots([&root]));
    fs::write(target.dir(), b"in the way").unwrap();

    let finder = Counting::new();
    let identity = RepoIdentity::resolve_with(&home, &repo, &finder).unwrap();
    assert!(identity.migration_error().is_some());
    assert_eq!(identity.id(), target.id());
    assert!(old.pending_dir().join("m").exists(), "nothing was lost");

    // Not cached, so the next call tries again and succeeds once the obstacle is gone.
    fs::remove_file(target.dir()).unwrap();
    let retried = RepoIdentity::resolve_with(&home, &repo, &finder).unwrap();
    assert_eq!(finder.calls(), 2);
    assert_eq!(retried.migration(), Some(&Migration::Moved));
    assert!(target.pending_dir().join("m").exists());
}
