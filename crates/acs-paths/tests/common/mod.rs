//! Shared fixtures for the path tests.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use acs_core::gate::{PathLocation, ResolvedPath};
use acs_paths::PathResolver;
use tempfile::TempDir;

/// A scratch world: `base/repo` (a repository) next to `base/outside`.
pub struct World {
    _tmp: TempDir,
    pub base: PathBuf,
    pub repo: PathBuf,
    pub outside: PathBuf,
}

pub fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, b"x").unwrap();
}

impl World {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();
        let repo = base.join("repo");
        let outside = base.join("outside");
        fs::create_dir_all(repo.join(".git")).unwrap();
        for file in [
            ".git/HEAD",
            ".git/config",
            "README.md",
            "src/a.rs",
            "src/b.rs",
            "src/deep/c.rs",
        ] {
            touch(&repo.join(file));
        }
        touch(&outside.join("file.txt"));
        Self {
            _tmp: tmp,
            base,
            repo,
            outside,
        }
    }

    pub fn resolver(&self) -> PathResolver {
        PathResolver::new(&self.repo).unwrap()
    }
}

/// Create a directory link (a symlink on Unix, a junction on Windows).
pub fn dir_link(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "mklink /J failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// The repository path of `resolved`; fails the test if it is not in the repository.
#[track_caller]
pub fn in_repo(resolved: &ResolvedPath) -> &str {
    match &resolved.location {
        PathLocation::InRepo(path) => path.as_str(),
        other => panic!("expected InRepo, got {other:?} for {:?}", resolved.raw),
    }
}

/// The absolute path of `resolved`; fails the test if it is not outside the repository.
#[track_caller]
pub fn outside(resolved: &ResolvedPath) -> PathBuf {
    match &resolved.location {
        PathLocation::Outside(path) => PathBuf::from(path),
        other => panic!("expected Outside, got {other:?} for {:?}", resolved.raw),
    }
}

pub fn files(expansion: &acs_core::gate::Expansion) -> Vec<&str> {
    expansion.files.iter().map(|f| f.as_str()).collect()
}
