//! `acs plugin install`, against a stand-in `claude` that logs its arguments. The real command
//! line was exercised by hand (decision 0021) and is in the installer workflow.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod cli_common;
mod common;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use cli_common::{assert_failure, assert_fits_80, assert_success, json, path_with_acs, stdout};
use common::{World, acs_bin};

const ENABLED: &str = r#"[{"id":"acs@acs-plugins","version":"0.1.0","scope":"project",
    "enabled":true}]"#;

/// A `claude` that appends its arguments to `log` and its working directory to `cwd`, answers
/// `plugin list` with the file `listing`, turns that listing into `after-install` when installing,
/// and fails on the word in `fail-on`.
struct FakeClaude {
    dir: PathBuf,
}

impl FakeClaude {
    fn new(world: &World) -> Self {
        let dir = world.tmp.path().join("fake-claude");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("listing"), "[]").unwrap();
        fs::write(dir.join("after-install"), ENABLED).unwrap();
        let fake = Self { dir };
        fake.write_script();
        fake
    }

    #[cfg(unix)]
    fn write_script(&self) {
        use std::os::unix::fs::PermissionsExt;
        let dir = self.dir.display();
        let script = format!(
            r#"#!/bin/sh
echo "$*" >> "{dir}/log"
pwd >> "{dir}/cwd"
if [ -f "{dir}/fail-on" ] && [ "$2" = "$(cat "{dir}/fail-on")" ]; then
  echo "it went wrong" >&2
  exit 1
fi
if [ "$2" = list ]; then cat "{dir}/listing"; fi
if [ "$2" = install ]; then cp "{dir}/after-install" "{dir}/listing"; fi
exit 0
"#
        );
        let path = self.dir.join("claude");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(windows)]
    fn write_script(&self) {
        let dir = self.dir.display();
        let script = format!(
            "@echo off\r\n\
             echo %*>>\"{dir}\\log\"\r\n\
             cd>>\"{dir}\\cwd\"\r\n\
             set FAIL=\r\n\
             if exist \"{dir}\\fail-on\" set /p FAIL=<\"{dir}\\fail-on\"\r\n\
             if \"%FAIL%\"==\"%2\" (\r\n\
               echo it went wrong 1>&2\r\n\
               exit /b 1\r\n\
             )\r\n\
             if \"%2\"==\"list\" type \"{dir}\\listing\"\r\n\
             if \"%2\"==\"install\" copy /y \"{dir}\\after-install\" \"{dir}\\listing\" >nul\r\n\
             exit /b 0\r\n"
        );
        fs::write(self.dir.join("claude.cmd"), script).unwrap();
    }

    fn path(&self) -> OsString {
        let mut dirs = vec![self.dir.clone()];
        dirs.extend(std::env::split_paths(&path_with_acs()));
        std::env::join_paths(dirs).unwrap()
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join("log"))
            .unwrap_or_default()
            .lines()
            .map(|line| line.trim().to_owned())
            .collect()
    }

    /// The directory of the last call, canonical.
    fn last_cwd(&self) -> PathBuf {
        let log = fs::read_to_string(self.dir.join("cwd")).unwrap();
        dunce::canonicalize(log.lines().last().unwrap().trim()).unwrap()
    }

    fn fail_on(&self, word: &str) {
        fs::write(self.dir.join("fail-on"), word).unwrap();
    }

    fn list_as(&self, listing: &str) {
        fs::write(self.dir.join("listing"), listing).unwrap();
    }

    fn list_after_install_as(&self, listing: &str) {
        fs::write(self.dir.join("after-install"), listing).unwrap();
    }

    fn acs(&self, world: &World, cwd: &Path, args: &[&str]) -> Output {
        Command::new(acs_bin())
            .args(args)
            .current_dir(cwd)
            .env("ACS_HOME", &world.home)
            .env("GIT_CONFIG_GLOBAL", &world.git_config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("PATH", self.path())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

fn setup() -> (World, FakeClaude) {
    let world = World::new();
    let fake = FakeClaude::new(&world);
    (world, fake)
}

#[test]
fn installs_at_project_scope_from_the_acs_marketplace_by_default() {
    let (world, fake) = setup();
    let out = fake.acs(&world, &world.repo, &["plugin", "install", "claude-code"]);
    assert_success(&out);
    assert_eq!(
        fake.calls(),
        [
            "plugin list --json",
            "plugin marketplace add acs-hq/acs --scope project",
            "plugin install acs@acs-plugins --scope project",
            "plugin list --json",
        ]
    );
    let text = stdout(&out);
    assert!(text.contains("installed and enabled"), "{text}");
    assert!(text.contains("version 0.1.0"), "{text}");
    assert!(text.contains("Commit .claude/settings.json"), "{text}");
    assert_fits_80(&text);
}

#[test]
fn running_it_again_changes_nothing() {
    let (world, fake) = setup();
    fake.list_as(ENABLED);
    let out = fake.acs(
        &world,
        &world.repo,
        &["plugin", "install", "claude-code", "--json"],
    );
    assert_success(&out);
    let report = json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["changed"], false);
    assert_eq!(report["version"], "0.1.0");
    assert_eq!(report["steps"].as_array().unwrap().len(), 0);
    assert_eq!(fake.calls(), ["plugin list --json"]);

    let text = stdout(&fake.acs(&world, &world.repo, &["plugin", "install", "claude-code"]));
    assert!(text.contains("already enabled"), "{text}");
    assert!(!text.contains("Commit"), "nothing new to commit: {text}");
}

#[test]
fn the_scope_and_marketplace_are_passed_on_and_user_scope_warns() {
    let (world, fake) = setup();
    fake.list_after_install_as(&ENABLED.replace("project", "user"));
    let out = fake.acs(
        &world,
        &world.repo,
        &[
            "plugin",
            "install",
            "claude-code",
            "--scope",
            "user",
            "--marketplace",
            "./checkout",
        ],
    );
    assert_success(&out);
    assert_eq!(
        fake.calls()[1..3],
        [
            "plugin marketplace add ./checkout --scope user",
            "plugin install acs@acs-plugins --scope user",
        ]
    );
    let text = stdout(&out);
    assert!(text.contains("every git repository"), "{text}");
    assert!(!text.contains("Commit .claude/settings.json"), "{text}");
    assert_fits_80(&text);
}

#[test]
fn a_subdirectory_installs_for_the_repository_root() {
    let (world, fake) = setup();
    let nested = world.repo.join("src/deep");
    fs::create_dir_all(&nested).unwrap();
    let out = fake.acs(&world, &nested, &["plugin", "install", "claude-code"]);
    assert_success(&out);
    // Claude Code writes project settings where it runs: not into src/deep/.claude.
    assert_eq!(fake.last_cwd(), dunce::canonicalize(&world.repo).unwrap());
}

#[test]
fn repository_scopes_need_a_repository_and_user_scope_does_not() {
    let (world, fake) = setup();
    let plain = world.tmp.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    let out = fake.acs(
        &world,
        &plain,
        &["plugin", "install", "claude-code", "--json"],
    );
    assert_failure(&out);
    let report = json(&out);
    assert_eq!(report["error"]["code"], "not_a_repo");
    assert!(
        report["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--scope user"),
        "{report}"
    );
    assert!(fake.calls().is_empty(), "nothing ran: {:?}", fake.calls());

    fake.list_after_install_as(&ENABLED.replace("project", "user"));
    let out = fake.acs(
        &world,
        &plain,
        &["plugin", "install", "claude-code", "--scope", "user"],
    );
    assert_success(&out);
}

#[test]
fn a_failing_step_stops_there_and_says_why() {
    let (world, fake) = setup();
    fake.fail_on("marketplace");
    let out = fake.acs(&world, &world.repo, &["plugin", "install", "claude-code"]);
    assert_failure(&out);
    assert!(
        !fake.calls().iter().any(|c| c.starts_with("plugin install")),
        "{:?}",
        fake.calls()
    );
    let text = stdout(&out);
    assert!(text.contains("FAILED"), "{text}");
    assert!(text.contains("it went wrong"), "{text}");
    assert_fits_80(&text);

    let out = fake.acs(
        &world,
        &world.repo,
        &["plugin", "install", "claude-code", "--json"],
    );
    assert_failure(&out);
    let report = json(&out);
    assert_eq!(report["ok"], false);
    assert_eq!(report["steps"][0]["ok"], false);
}

#[test]
fn a_plugin_that_does_not_end_up_enabled_is_a_failure() {
    let (world, fake) = setup();
    fake.list_after_install_as("[]");
    let out = fake.acs(&world, &world.repo, &["plugin", "install", "claude-code"]);
    assert_failure(&out);
    assert!(stdout(&out).contains("does not list"), "{}", stdout(&out));
}

#[test]
fn antigravity_is_not_covered_yet_and_says_how_to_do_it_by_hand() {
    let (world, fake) = setup();
    let out = fake.acs(
        &world,
        &world.repo,
        &["plugin", "install", "antigravity", "--json"],
    );
    assert_failure(&out);
    let report = json(&out);
    assert_eq!(report["error"]["code"], "unsupported_harness");
    assert!(
        report["error"]["message"]
            .as_str()
            .unwrap()
            .contains("agy plugin install"),
        "{report}"
    );
}

#[test]
fn an_unknown_harness_lists_the_known_ones() {
    let (world, fake) = setup();
    let out = fake.acs(
        &world,
        &world.repo,
        &["plugin", "install", "nano", "--json"],
    );
    assert_failure(&out);
    let report = json(&out);
    assert_eq!(report["error"]["code"], "unknown_harness");
    let message = report["error"]["message"].as_str().unwrap();
    assert!(message.contains("claude-code"), "{message}");
    assert!(message.contains("antigravity"), "{message}");
}

#[cfg(unix)]
#[test]
fn acs_must_be_on_path_before_any_plugin_is_enabled() {
    let (world, fake) = setup();
    let path = {
        let mut dirs = vec![fake.dir.clone()];
        dirs.extend(std::env::split_paths(&cli_common::git_only_path(&world)));
        std::env::join_paths(dirs).unwrap()
    };
    let out = cli_common::run_with_path(
        &world,
        &world.repo,
        &path,
        &["plugin", "install", "claude-code", "--json"],
    );
    assert_failure(&out);
    let report = json(&out);
    assert_eq!(report["error"]["code"], "acs_not_on_path");
    assert!(fake.calls().is_empty(), "nothing ran: {:?}", fake.calls());
}

#[cfg(unix)]
#[test]
fn a_missing_harness_is_named() {
    let world = World::new();
    let dir = world.tmp.path().join("acs-only");
    fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(acs_bin(), dir.join("acs")).unwrap();
    let path = {
        let mut dirs = vec![dir];
        dirs.extend(std::env::split_paths(&cli_common::git_only_path(&world)));
        std::env::join_paths(dirs).unwrap()
    };
    let out = cli_common::run_with_path(
        &world,
        &world.repo,
        &path,
        &["plugin", "install", "claude-code", "--json"],
    );
    assert_failure(&out);
    assert_eq!(json(&out)["error"]["code"], "harness_not_found");
}
