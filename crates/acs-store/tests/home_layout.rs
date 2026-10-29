//! `ACS_HOME` layout and permissions on disk.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use acs_store::fsutil::{private_file_options, write_private_atomic};
use acs_store::{AcsHome, Migration, RepoId};
use common::Sandbox;

fn id(seed: &str) -> RepoId {
    RepoId::from_roots([seed.repeat(40)])
}

#[test]
fn nothing_is_created_until_ensure() {
    let sandbox = Sandbox::new();
    let home = AcsHome::at(sandbox.join("acs")).unwrap();
    let repo = home.repo(&id("a"));
    assert!(!home.root().exists());
    repo.ensure().unwrap();
    assert!(repo.snapshots_dir().is_dir());
    assert!(repo.cache_dir().is_dir());
    assert!(repo.pending_dir().is_dir());
    assert!(
        !repo.state_db().exists(),
        "state.db belongs to the state store"
    );
    assert!(home.repos_dir().is_dir());

    // Idempotent, and keeps existing content.
    fs::write(repo.pending_dir().join("keep"), b"x").unwrap();
    repo.ensure().unwrap();
    home.ensure().unwrap();
    assert!(repo.pending_dir().join("keep").exists());
    assert!(home.index_dir().is_dir());
}

#[test]
fn migrate_moves_the_directory_when_the_target_is_absent() {
    let sandbox = Sandbox::new();
    let home = AcsHome::at(sandbox.join("acs")).unwrap();
    let (from, to) = (id("a"), id("b"));
    assert_eq!(home.migrate_repo(&from, &to).unwrap(), Migration::NotNeeded);
    assert_eq!(
        home.migrate_repo(&from, &from).unwrap(),
        Migration::NotNeeded
    );
    let old = home.repo(&from);
    old.ensure().unwrap();
    fs::write(old.pending_dir().join("x"), b"x").unwrap();
    assert_eq!(home.migrate_repo(&from, &to).unwrap(), Migration::Moved);
    assert!(home.repo(&to).pending_dir().join("x").exists());
    assert!(!old.dir().exists());
}

#[test]
fn private_atomic_write_replaces_content_and_leaves_no_temp_files() {
    let sandbox = Sandbox::new();
    let target = sandbox.join("file.json");
    write_private_atomic(&target, b"one").unwrap();
    write_private_atomic(&target, b"two").unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"two");
    let names: Vec<_> = fs::read_dir(sandbox.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(
        names.iter().all(|n| !n.to_string_lossy().contains(".tmp-")),
        "{names:?}"
    );
    // create_new: an existing file is not clobbered by the options helper.
    assert!(private_file_options().open(&target).is_err());
}

#[cfg(unix)]
mod unix_permissions {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;
    use acs_store::{GitCli, RepoIdentity};

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn new_directories_are_owner_only() {
        let sandbox = Sandbox::new();
        let home = AcsHome::at(sandbox.join("nested").join("acs")).unwrap();
        let repo = home.repo(&id("a"));
        repo.ensure().unwrap();
        home.ensure().unwrap();
        for dir in [
            home.root().to_path_buf(),
            home.repos_dir(),
            home.index_dir(),
            repo.dir().to_path_buf(),
            repo.snapshots_dir(),
            repo.cache_dir(),
            repo.pending_dir(),
        ] {
            assert_eq!(mode(&dir), 0o700, "{}", dir.display());
        }
    }

    #[test]
    fn loose_existing_directories_are_tightened() {
        let sandbox = Sandbox::new();
        let root = sandbox.join("acs");
        let repo_dir = root.join("repos").join(id("a").as_str());
        fs::create_dir_all(repo_dir.join("pending")).unwrap();
        for dir in [
            &root,
            &root.join("repos"),
            &repo_dir,
            &repo_dir.join("pending"),
        ] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let home = AcsHome::at(&root).unwrap();
        home.repo(&id("a")).ensure().unwrap();
        for dir in [
            &root,
            &root.join("repos"),
            &repo_dir,
            &repo_dir.join("pending"),
        ] {
            assert_eq!(mode(dir), 0o700, "{}", dir.display());
        }
    }

    #[test]
    fn private_files_are_owner_only() {
        let sandbox = Sandbox::new();
        let path = sandbox.join("f");
        write_private_atomic(&path, b"x").unwrap();
        assert_eq!(mode(&path), 0o600);
        let created = sandbox.join("g");
        private_file_options().open(&created).unwrap();
        assert_eq!(mode(&created), 0o600);
    }

    #[test]
    fn the_identity_cache_is_owner_only() {
        let sandbox = Sandbox::new();
        let home = AcsHome::at(sandbox.join("acs")).unwrap();
        let repo = sandbox.init("repo");
        sandbox.commit_file(&repo, "a", "a");
        RepoIdentity::resolve_with(&home, &repo, &GitCli::default()).unwrap();
        assert_eq!(mode(&home.index_dir()), 0o700);
        let entry = fs::read_dir(home.index_dir())
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(mode(&entry.path()), 0o600);
    }
}
