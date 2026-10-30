//! `acs init`: policy defaults, fetch refspec, idempotence, harness hints, `PATH` warning.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod cli_common;
mod common;

use std::fs;
use std::process::Command;

use cli_common::{
    acs, acs_in, assert_failure, assert_fits_80, assert_success, json, stderr, stdout,
};
use common::{World, git};

/// Everything `init` may change, as text: the policy files, the git config and the hooks.
fn snapshot(world: &World) -> String {
    let mut out = String::new();
    for rel in [
        ".acs/policy/config.toml",
        ".acs/policy/guards.toml",
        ".acs/policy/territory.toml",
        ".git/config",
        ".git/hooks/pre-commit",
        ".git/hooks/prepare-commit-msg",
        ".git/hooks/pre-push",
    ] {
        let text = fs::read_to_string(world.repo.join(rel)).unwrap_or_else(|_| "<absent>".into());
        out.push_str(&format!("== {rel}\n{text}\n"));
    }
    out
}

fn fetch_specs(world: &World, remote: &str) -> Vec<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(&world.repo)
        .args(["config", "--get-all", &format!("remote.{remote}.fetch")])
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

fn add_remote(world: &World, name: &str) {
    git(
        &world.repo,
        &world.git_config,
        &["remote", "add", name, "https://example.com/acme/repo.git"],
    );
}

#[test]
fn init_creates_policy_and_refspec_and_is_idempotent() {
    let world = World::new();
    add_remote(&world, "origin");
    add_remote(&world, "fork");

    let first = acs(&world, &["init"]);
    assert_success(&first);
    assert_fits_80(&stdout(&first));
    for file in ["config.toml", "guards.toml", "territory.toml"] {
        assert!(
            world.repo.join(".acs/policy").join(file).is_file(),
            "{file}"
        );
    }
    let config = fs::read_to_string(world.repo.join(".acs/policy/config.toml")).unwrap();
    assert!(config.contains("spec = \"acs/0.1\""));
    for remote in ["origin", "fork"] {
        let specs = fetch_specs(&world, remote);
        let wanted = format!("+refs/acs/*:refs/remotes/{remote}/acs/*");
        assert_eq!(
            specs.iter().filter(|s| **s == wanted).count(),
            1,
            "{specs:?}"
        );
        assert!(specs.len() >= 2, "the default refspec is kept: {specs:?}");
    }

    let before = snapshot(&world);
    let second = acs(&world, &["init", "--json"]);
    assert_success(&second);
    assert_eq!(snapshot(&world), before, "a second init changes nothing");
    let report = json(&second);
    assert_eq!(report["ok"], true);
    assert_eq!(report["changed"], false);
    assert!(
        report["policy_files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["action"] == "exists")
    );
    assert!(
        report["remotes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["action"] == "present")
    );
    assert_eq!(fetch_specs(&world, "origin").len(), 2);

    // The policy `init` wrote is accepted by everything else.
    let status = acs(&world, &["status", "--json"]);
    assert_success(&status);
    assert_eq!(
        json(&status)["policy_warnings"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn init_never_overwrites_an_edited_config() {
    let world = World::new();
    fs::create_dir_all(world.repo.join(".acs/policy")).unwrap();
    let mine = "spec = \"acs/0.1\"\ndefault_target_branch = \"trunk\"\n# keep me\n";
    fs::write(world.repo.join(".acs/policy/config.toml"), mine).unwrap();

    assert_success(&acs(&world, &["init"]));
    assert_eq!(
        fs::read_to_string(world.repo.join(".acs/policy/config.toml")).unwrap(),
        mine
    );
    // The files that were missing are still added.
    assert!(world.repo.join(".acs/policy/guards.toml").is_file());
    let status = json(&acs(&world, &["status", "--json"]));
    assert_eq!(status["target_branch"], "trunk");
}

#[test]
fn init_without_a_remote_says_so() {
    let world = World::new();
    let out = acs(&world, &["init"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("no git remote"), "{text}");
    let report = json(&acs(&world, &["init", "--json"]));
    assert_eq!(report["remotes"].as_array().unwrap().len(), 0);
}

#[test]
fn init_prints_install_commands_for_detected_harnesses() {
    let world = World::new();
    fs::create_dir_all(world.repo.join(".claude")).unwrap();
    fs::create_dir_all(world.repo.join(".agents")).unwrap();
    let out = acs(&world, &["init"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains("claude plugin install acs@acs-plugins"),
        "{text}"
    );
    assert!(text.contains("claude plugin marketplace add"), "{text}");
    assert!(text.contains("agy plugin install"), "{text}");
    assert_fits_80(&text);
    // Printed, not run: no settings file appeared.
    assert!(!world.repo.join(".claude/settings.json").exists());
    let report = json(&acs(&world, &["init", "--json"]));
    let ids: Vec<&str> = report["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"claude-code") && ids.contains(&"antigravity"),
        "{ids:?}"
    );
}

#[cfg(unix)]
#[test]
fn init_warns_loudly_when_acs_is_not_on_path() {
    let world = World::new();
    fs::create_dir_all(world.repo.join(".agents")).unwrap();
    let path = cli_common::git_only_path(&world);
    let out = cli_common::run_with_path(&world, &world.repo, &path, &["init"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("WARNING: `acs` is not on PATH"), "{text}");
    assert!(text.contains("DENIED"), "Antigravity fails closed: {text}");
    assert_fits_80(&text);
    let out = cli_common::run_with_path(&world, &world.repo, &path, &["init", "--json"]);
    assert_eq!(json(&out)["acs_on_path"]["found"], false);

    let with_acs = json(&acs(&world, &["init", "--json"]));
    assert_eq!(with_acs["acs_on_path"]["found"], true);
}

#[test]
fn init_works_from_a_subdirectory_and_outside_a_repository_it_fails() {
    let world = World::new();
    let out = acs_in(&world, &world.repo.join("src"), &["init"]);
    assert_success(&out);
    assert!(world.repo.join(".acs/policy/config.toml").is_file());
    assert!(!world.repo.join("src/.acs").exists());

    let plain = world.tmp.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    let out = acs_in(&world, &plain, &["init"]);
    assert_failure(&out);
    assert!(stderr(&out).contains("acs:"), "{}", stderr(&out));
    let out = acs_in(&world, &plain, &["init", "--json"]);
    assert_failure(&out);
    let error = json(&out);
    assert_eq!(error["ok"], false);
    assert_eq!(error["error"]["code"], "not_a_repo");
    assert!(stderr(&out).is_empty());
}
