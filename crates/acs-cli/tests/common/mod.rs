//! Shared fixtures: a temporary git repository, a temporary `ACS_HOME`, and simulated agents that
//! talk to the built `acs` binary through canonical events.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use acs_core::{Decision, SeatId};
use acs_store::{AcsHome, RepoHome, RepoIdentity, SeatInfo, SnapshotStore, StateDb};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The `acs` binary built for this test run.
pub fn acs_bin() -> &'static str {
    env!("CARGO_BIN_EXE_acs")
}

/// A repository, an `ACS_HOME`, and an isolated git configuration.
pub struct World {
    pub tmp: TempDir,
    pub repo: PathBuf,
    pub home: PathBuf,
    pub git_config: PathBuf,
}

/// Runs `git` in `dir` with the isolated configuration.
pub fn git(dir: &Path, git_config: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

impl World {
    /// A repository with `a.txt`, `src/lib.rs`, `src/util.rs` and a `.gitignore` for `target/`,
    /// committed.
    pub fn new() -> Self {
        let tmp = tempfile::Builder::new().prefix("acs-e2e-").tempdir().unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();
        let repo = base.join("repo");
        let home = base.join("acs-home");
        let git_config = base.join("gitconfig");
        fs::create_dir_all(&repo).unwrap();
        fs::write(&git_config, "[user]\n\temail = sara@example.com\n\tname = Sara\n").unwrap();
        git(&repo, &git_config, &["init", "-q", "-b", "main", "."]);
        fs::write(repo.join(".gitignore"), "target/\n*.log\n").unwrap();
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
        fs::write(repo.join("src/util.rs"), "pub fn util() {}\n").unwrap();
        git(&repo, &git_config, &["add", "-A"]);
        git(&repo, &git_config, &["commit", "-q", "-m", "init"]);
        Self {
            tmp,
            repo,
            home,
            git_config,
        }
    }

    /// Runs `acs <args>` with the world's environment.
    pub fn run(&self, args: &[&str], stdin: &[u8], cwd: &Path) -> Output {
        let mut child = Command::new(acs_bin())
            .args(args)
            .current_dir(cwd)
            .env("ACS_HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", &self.git_config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin).unwrap();
        child.wait_with_output().unwrap()
    }

    /// Sends a canonical event to `acs hook --canonical <phase>` and parses the decision.
    pub fn hook(&self, phase: &str, event: &Value) -> Decision {
        let out = self.run(
            &["hook", "--canonical", phase],
            event.to_string().as_bytes(),
            &self.repo,
        );
        assert!(out.status.success(), "acs failed: {out:?}");
        serde_json::from_slice(&out.stdout).unwrap_or_else(|err| {
            panic!(
                "bad decision ({err}): {}",
                String::from_utf8_lossy(&out.stdout)
            )
        })
    }

    pub fn acs_home(&self) -> AcsHome {
        AcsHome::at(&self.home).unwrap()
    }

    pub fn repo_home(&self) -> RepoHome {
        let home = self.acs_home();
        RepoIdentity::resolve(&home, &self.repo).unwrap().home(&home)
    }

    pub fn db(&self) -> StateDb {
        StateDb::open(&self.repo_home()).unwrap()
    }

    pub fn snapshots(&self) -> SnapshotStore {
        SnapshotStore::open(&self.repo_home()).unwrap()
    }

    pub fn write(&self, rel: &str, text: &str) {
        let path = self.repo.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// Lets the file system clock move on so later writes are newer than earlier ones.
    pub fn tick(&self) {
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
}

/// One simulated agent session.
pub struct Agent<'w> {
    pub world: &'w World,
    pub harness: &'static str,
    pub session: String,
}

impl<'w> Agent<'w> {
    pub fn new(world: &'w World, harness: &'static str, session: &str) -> Self {
        Self {
            world,
            harness,
            session: session.to_owned(),
        }
    }

    pub fn event(&self, phase: &str, extra: Value) -> Value {
        let mut event = json!({
            "spec": "acs/0.1",
            "harness": self.harness,
            "harness_session_id": self.session,
            "phase": phase,
            "cwd": self.world.repo,
        });
        if let (Some(base), Value::Object(more)) = (event.as_object_mut(), extra) {
            base.extend(more);
        }
        event
    }

    pub fn send(&self, phase: &str, extra: Value) -> Decision {
        self.world.hook(phase, &self.event(phase, extra))
    }

    pub fn start(&self) -> Decision {
        self.send("session_start", json!({}))
    }

    pub fn end(&self) -> Decision {
        self.send("session_end", json!({}))
    }

    pub fn pre_edit(&self, path: &str) -> Decision {
        self.send(
            "pre_tool",
            json!({"tool": {"name": "Edit", "kind": "edit"}, "paths": [path]}),
        )
    }

    pub fn post_edit(&self, path: &str) -> Decision {
        self.send(
            "post_tool",
            json!({"tool": {"name": "Edit", "kind": "edit"}, "paths": [path]}),
        )
    }

    pub fn pre_tool(&self, name: &str, kind: &str, paths: &[&str]) -> Decision {
        self.send(
            "pre_tool",
            json!({"tool": {"name": name, "kind": kind}, "paths": paths}),
        )
    }

    pub fn pre_shell(&self, command: &str) -> Decision {
        self.send(
            "pre_tool",
            json!({"tool": {"name": "Bash", "kind": "shell"}, "command": command}),
        )
    }

    /// A post-tool shell event. It carries no `result`, like Claude Code's Bash `PostToolUse`.
    pub fn post_shell(&self, command: &str) -> Decision {
        self.send(
            "post_tool",
            json!({"tool": {"name": "Bash", "kind": "shell"}, "command": command}),
        )
    }

    pub fn info(&self) -> SeatInfo {
        self.world
            .db()
            .seat_by_session(self.harness, &self.session)
            .unwrap()
            .expect("seat exists")
    }

    pub fn seat_id(&self) -> SeatId {
        self.info().seat.seat_id
    }

    /// The paths of the seat's live claims, sorted.
    pub fn claimed(&self) -> Vec<String> {
        let id = self.seat_id();
        let mut paths: Vec<String> = self
            .world
            .db()
            .live_claims(acs_core::Timestamp::now())
            .unwrap()
            .into_iter()
            .filter(|h| h.claim.seat_id == id)
            .map(|h| h.claim.scope.path().to_string())
            .collect();
        paths.sort();
        paths
    }
}
