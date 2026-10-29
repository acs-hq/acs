//! Shared helpers: hermetic git sandboxes. Not every test file uses every helper.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// A temporary directory with its own git configuration, so tests never read the developer's
/// or the CI runner's git settings.
pub struct Sandbox {
    dir: TempDir,
}

impl Sandbox {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("acs-store-")
            .tempdir()
            .unwrap();
        let sandbox = Self { dir };
        fs::create_dir_all(sandbox.home_dir()).unwrap();
        fs::write(
            sandbox.git_config_path(),
            "[user]\n\tname = Test\n\temail = test@example.com\n\
             [init]\n\tdefaultBranch = main\n\
             [commit]\n\tgpgsign = false\n\
             [core]\n\tautocrlf = false\n\
             [protocol \"file\"]\n\tallow = always\n",
        )
        .unwrap();
        sandbox
    }

    /// The sandbox root. May be a symlinked path (for example `/var` on macOS) on purpose.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    pub fn home_dir(&self) -> PathBuf {
        self.dir.path().join("userhome")
    }

    fn git_config_path(&self) -> PathBuf {
        self.dir.path().join("gitconfig")
    }

    /// Runs git with a hermetic environment and returns trimmed stdout. Panics on failure.
    pub fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.git_config_path())
            .env("HOME", self.home_dir())
            .env("USERPROFILE", self.home_dir())
            .env("XDG_CONFIG_HOME", self.home_dir().join(".config"))
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null());
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
        ] {
            command.env_remove(var);
        }
        let output = command
            .output()
            .expect("git must be installed to run these tests");
        assert!(
            output.status.success(),
            "git {args:?} failed in {}: {}",
            cwd.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// `git init` a new directory `name` inside the sandbox and returns its path.
    pub fn init(&self, name: &str) -> PathBuf {
        let dir = self.join(name);
        fs::create_dir_all(&dir).unwrap();
        self.git(&dir, &["init", "--quiet"]);
        dir
    }

    /// Writes `file` with `contents` and commits it.
    pub fn commit_file(&self, repo: &Path, file: &str, contents: &str) -> String {
        let path = repo.join(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        self.git(repo, &["add", "--", file]);
        self.git(repo, &["commit", "--quiet", "-m", &format!("add {file}")]);
        self.git(repo, &["rev-parse", "HEAD"])
    }

    pub fn commit_empty(&self, repo: &Path, message: &str) -> String {
        self.git(repo, &["commit", "--quiet", "--allow-empty", "-m", message]);
        self.git(repo, &["rev-parse", "HEAD"])
    }
}
