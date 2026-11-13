//! A team on several machines: a bare remote, and clones that do not share `ACS_HOME`.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use acs_core::Decision;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::{acs_bin, git};

/// Longest wait for something a detached sync does. Generous: Windows runners are slow to start
/// processes, and this is only reached when the test is going to fail anyway.
const PATIENCE: Duration = Duration::from_secs(60);

/// A remote with a committed policy syncing every five seconds. Not less: the view goes stale after
/// three intervals, and a test that syncs and then edits on a busy machine must stay inside that.
pub struct Team {
    _tmp: TempDir,
    pub base: PathBuf,
    pub remote: PathBuf,
}

/// One clone, as on a machine of its own: its own `ACS_HOME` and its own git identity.
pub struct Clone {
    pub repo: PathBuf,
    pub home: PathBuf,
    pub git_config: PathBuf,
}

impl Team {
    pub fn new() -> Self {
        let tmp = tempfile::Builder::new()
            .prefix("acs-sync-")
            .tempdir()
            .unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();
        let config = base.join("seed-gitconfig");
        fs::write(
            &config,
            "[user]\n\temail = seed@example.com\n\tname = Seed\n",
        )
        .unwrap();
        let remote = base.join("remote.git");
        fs::create_dir(&remote).unwrap();
        git(
            &remote,
            &config,
            &["init", "-q", "--bare", "-b", "main", "."],
        );
        let seed = base.join("seed");
        fs::create_dir_all(seed.join(".acs/policy")).unwrap();
        fs::create_dir_all(seed.join("src")).unwrap();
        git(&seed, &config, &["init", "-q", "-b", "main", "."]);
        fs::write(
            seed.join(".acs/policy/config.toml"),
            "spec = \"acs/0.1\"\nsync_interval_secs = 5\n[features]\nsync = true\n",
        )
        .unwrap();
        fs::write(seed.join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
        fs::write(seed.join("src/util.rs"), "pub fn util() {}\n").unwrap();
        git(&seed, &config, &["add", "-A"]);
        git(&seed, &config, &["commit", "-q", "-m", "init"]);
        git(
            &seed,
            &config,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&seed, &config, &["push", "-q", "origin", "main"]);
        Self {
            _tmp: tmp,
            base,
            remote,
        }
    }

    pub fn clone(&self, name: &str, email: &str) -> Clone {
        let git_config = self.base.join(format!("{name}-gitconfig"));
        fs::write(
            &git_config,
            format!("[user]\n\temail = {email}\n\tname = {name}\n"),
        )
        .unwrap();
        let repo = self.base.join(name);
        git(
            &self.base,
            &git_config,
            &[
                "clone",
                "-q",
                self.remote.to_str().unwrap(),
                repo.to_str().unwrap(),
            ],
        );
        Clone {
            repo,
            home: self.base.join(format!("{name}-home")),
            git_config,
        }
    }
}

impl Clone {
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(acs_bin());
        command
            .args(args)
            .current_dir(&self.repo)
            .env("ACS_HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", &self.git_config)
            .env("GIT_CONFIG_NOSYSTEM", "1");
        command
    }

    pub fn acs(&self, args: &[&str]) -> Output {
        self.command(args).stdin(Stdio::null()).output().unwrap()
    }

    /// `acs <args> --json`, which must succeed.
    pub fn json(&self, args: &[&str]) -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        let out = self.acs(&all);
        assert!(out.status.success(), "acs {args:?}: {out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// Sends one canonical event; `extra` is merged in.
    pub fn hook(&self, phase: &str, session: &str, extra: Value) -> Decision {
        let mut event = json!({
            "spec": "acs/0.1",
            "harness": "claude-code",
            "harness_session_id": session,
            "phase": phase,
            "cwd": self.repo,
        });
        if let (Some(base), Value::Object(more)) = (event.as_object_mut(), extra) {
            base.extend(more);
        }
        let mut child = self
            .command(&["hook", "--canonical", phase])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(event.to_string().as_bytes())
            .unwrap();
        // `wait_with_output` reads to end of file: a sync child holding the hook's pipes would
        // keep this waiting, as it would keep a harness.
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    pub fn edit(&self, session: &str, path: &str) -> Decision {
        let tool = json!({"tool": {"name": "Edit", "kind": "edit"}, "paths": [path]});
        self.hook("pre_tool", session, tool)
    }

    pub fn after_edit(&self, session: &str, path: &str) -> Decision {
        let tool = json!({"tool": {"name": "Edit", "kind": "edit"}, "paths": [path]});
        self.hook("post_tool", session, tool)
    }

    pub fn status(&self) -> Value {
        self.json(&["status"])
    }

    /// Whether `acs status` lists a claim on `path` from another machine.
    pub fn sees_remote_claim(&self, path: &str) -> bool {
        let view = self.status();
        let others = view["other_claims"].as_array().unwrap();
        others
            .iter()
            .any(|c| c["path"] == path && c["source"] == "remote")
    }

    pub fn sync_attempts(&self) -> Value {
        self.status()["remote"]["last_attempt_at"].clone()
    }
}

/// Polls `done` until it holds.
pub fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < PATIENCE, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}
