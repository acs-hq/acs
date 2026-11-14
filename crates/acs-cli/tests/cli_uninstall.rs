//! `acs uninstall` (issue 63, decision 0039): `acs init` followed by `acs uninstall` leaves the
//! repository, its git config and its hooks byte-identical to before, in a plain repository,
//! with husky and with a `core.hooksPath` folder; the optional steps run only when asked for;
//! a dry run changes nothing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod cli_common;
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use cli_common::{acs, assert_fits_80, assert_success, json, stdout};
use common::{World, git};
use serde_json::Value;

/// Every file under `dir`, relative path to bytes, skipping git's object store and other files
/// git itself rewrites (index, logs) and ACS's state, which lives outside the repository.
fn files_under(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if [".git/objects", ".git/logs", ".git/index", ".git/FETCH_HEAD"]
            .iter()
            .any(|skip| rel == *skip)
        {
            continue;
        }
        if path.is_dir() {
            out.insert(format!("{rel}/"), Vec::new());
            files_under(&path, root, out);
        } else {
            out.insert(rel, fs::read(&path).unwrap());
        }
    }
}

/// The repository as uninstall must restore it: the work tree, `.git` (config, hooks, refs) and
/// every ref git knows.
fn snapshot(world: &World) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    files_under(&world.repo, &world.repo, &mut out);
    let refs = Command::new("git")
        .arg("-C")
        .arg(&world.repo)
        .args(["for-each-ref", "--format=%(refname) %(objectname)"])
        .output()
        .unwrap();
    out.insert("<refs>".to_owned(), refs.stdout);
    out
}

fn assert_same(before: &BTreeMap<String, Vec<u8>>, after: &BTreeMap<String, Vec<u8>>) {
    let keys = |m: &BTreeMap<String, Vec<u8>>| m.keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(before), keys(after), "the set of files differs");
    for (key, bytes) in before {
        assert_eq!(
            String::from_utf8_lossy(bytes),
            String::from_utf8_lossy(&after[key]),
            "{key} differs"
        );
    }
}

/// A bare repository next to the world's, added as `origin`.
fn add_bare_origin(world: &World) {
    let upstream = world.tmp.path().join("origin.git");
    git(
        world.tmp.path(),
        &world.git_config,
        &["init", "-q", "--bare", upstream.to_str().unwrap()],
    );
    git(
        &world.repo,
        &world.git_config,
        &["remote", "add", "origin", upstream.to_str().unwrap()],
    );
    git(
        &world.repo,
        &world.git_config,
        &["push", "-q", "origin", "main"],
    );
}

fn remote_acs_refs(world: &World) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(&world.repo)
        .args(["ls-remote", "origin", "refs/acs/*"])
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn step<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("no step {id} in {report}"))
}

/// `acs init` with team sync on, so the fetch refspec is added, plus ACS refs locally and on
/// the remote, as a synced repository has them.
fn set_up_everything(world: &World) {
    assert_success(&acs(world, &["init"]));
    assert_success(&acs(world, &["features", "sync", "on"]));
    assert_success(&acs(world, &["init"]));
    let head = Command::new("git")
        .arg("-C")
        .arg(&world.repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_owned();
    git(
        &world.repo,
        &world.git_config,
        &["update-ref", "refs/acs/claims/someone", &head],
    );
    git(
        &world.repo,
        &world.git_config,
        &["update-ref", "refs/remotes/origin/acs/claims/other", &head],
    );
    git(
        &world.repo,
        &world.git_config,
        &[
            "push",
            "-q",
            "--no-verify",
            "origin",
            "refs/acs/claims/someone",
        ],
    );
    assert_success(&acs(world, &["status"]));
}

#[test]
fn init_then_uninstall_all_restores_a_plain_repository_byte_for_byte() {
    let world = World::new();
    add_bare_origin(&world);
    let before = snapshot(&world);

    set_up_everything(&world);
    assert!(remote_acs_refs(&world).contains("refs/acs/claims/someone"));
    let state_dir = world.repo_home().dir().to_path_buf();
    assert!(state_dir.is_dir(), "the state exists");

    let out = acs(&world, &["uninstall", "--all", "--json"]);
    assert_success(&out);
    let report = json(&out);
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(report["changed"], true);
    for id in [
        "git_hooks",
        "fetch_refspecs",
        "local_refs",
        "policy",
        "state",
        "remote_refs",
    ] {
        assert_eq!(step(&report, id)["state"], "done", "{id}: {report}");
    }
    assert_same(&before, &snapshot(&world));
    assert_eq!(remote_acs_refs(&world), "");
    assert!(!state_dir.exists(), "the state is gone");
    let index = fs::read_dir(world.home.join("index")).map_or(0, Iterator::count);
    assert_eq!(index, 0, "no identity cache entry is left");

    // Nothing is left to remove.
    let again = json(&acs(&world, &["uninstall", "--all", "--json"]));
    assert_eq!(again["changed"], false, "{again}");
    let text = stdout(&acs(&world, &["uninstall"]));
    assert!(text.contains("Nothing to remove"), "{text}");
}

#[test]
fn without_flags_the_policy_state_and_remote_refs_are_kept() {
    let world = World::new();
    add_bare_origin(&world);
    set_up_everything(&world);

    let out = acs(&world, &["uninstall"]);
    assert_success(&out);
    let text = stdout(&out);
    assert_fits_80(&text);
    assert!(
        text.contains("Policy: kept (--policy removes it)"),
        "{text}"
    );
    assert!(
        text.contains("Local state: kept (--state removes it)"),
        "{text}"
    );
    assert!(world.repo.join(".acs/policy/config.toml").is_file());
    assert!(world.repo_home().dir().is_dir());
    assert!(remote_acs_refs(&world).contains("refs/acs/claims/someone"));
    let hook = fs::read_to_string(world.repo.join(".git/hooks/pre-commit")).unwrap_or_default();
    assert!(!hook.contains("acs git-hook"), "{hook}");
    // `acs init` sets it up again from the kept policy.
    assert_success(&acs(&world, &["init"]));
    let hook = fs::read_to_string(world.repo.join(".git/hooks/pre-commit")).unwrap();
    assert!(hook.contains("acs git-hook"));
}

#[test]
fn a_dry_run_changes_nothing() {
    let world = World::new();
    add_bare_origin(&world);
    set_up_everything(&world);
    let before = snapshot(&world);

    let out = acs(&world, &["uninstall", "--all", "--dry-run", "--json"]);
    assert_success(&out);
    let report = json(&out);
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["changed"], false);
    assert_eq!(step(&report, "git_hooks")["state"], "planned");
    assert_eq!(step(&report, "policy")["state"], "planned");
    let remote = step(&report, "remote_refs");
    assert_eq!(remote["state"], "planned", "{report}");
    assert_eq!(
        remote["items"],
        serde_json::json!(["origin: refs/acs/claims/someone"])
    );
    assert_same(&before, &snapshot(&world));
    assert!(remote_acs_refs(&world).contains("refs/acs/claims/someone"));

    let text = stdout(&acs(&world, &["uninstall", "--dry-run"]));
    assert_fits_80(&text);
    assert!(text.contains("Dry run: nothing was changed"), "{text}");
}

#[test]
fn hooks_of_your_own_are_restored_exactly() {
    let world = World::new();
    let hooks = world.repo.join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    // A shell script gets the block inserted; a Python one is moved aside and wrapped.
    fs::write(hooks.join("pre-commit"), "#!/bin/sh\necho mine\n").unwrap();
    fs::write(
        hooks.join("pre-push"),
        "#!/usr/bin/env python3\nprint('mine')\n",
    )
    .unwrap();
    let before = snapshot(&world);

    let report = json(&acs(&world, &["init", "--json"]));
    let actions: Vec<&str> = report["git_hooks"]["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["injected", "created", "wrapped"]);

    let out = acs(&world, &["uninstall", "--policy", "--json"]);
    assert_success(&out);
    let report = json(&out);
    let items = step(&report, "git_hooks")["items"].to_string();
    assert!(items.contains("pre-push.acs-orig moves back"), "{items}");
    assert_same(&before, &snapshot(&world));
}

#[test]
fn husky_9_scripts_are_restored_exactly() {
    let world = World::new();
    let stubs = world.repo.join(".husky/_");
    fs::create_dir_all(&stubs).unwrap();
    fs::write(stubs.join("h"), "#!/usr/bin/env sh\n").unwrap();
    fs::write(world.repo.join(".husky/pre-commit"), "npm test\n").unwrap();
    git(
        &world.repo,
        &world.git_config,
        &["config", "core.hooksPath", ".husky/_"],
    );
    let before = snapshot(&world);

    assert_success(&acs(&world, &["init"]));
    assert!(
        fs::read_to_string(world.repo.join(".husky/pre-commit"))
            .unwrap()
            .contains("acs git-hook")
    );
    let out = acs(&world, &["uninstall", "--policy"]);
    assert_success(&out);
    assert!(stdout(&out).contains(".husky/"), "{}", stdout(&out));
    assert_same(&before, &snapshot(&world));
}

#[test]
fn a_core_hooks_path_folder_is_restored_exactly() {
    let world = World::new();
    let dir = world.repo.join("tools/hooks");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("pre-commit"), "#!/bin/bash\nmake lint\n").unwrap();
    git(
        &world.repo,
        &world.git_config,
        &["config", "core.hooksPath", "tools/hooks"],
    );
    let before = snapshot(&world);

    assert_success(&acs(&world, &["init"]));
    assert!(dir.join("pre-push").is_file());
    assert_success(&acs(&world, &["uninstall", "--policy", "--state"]));
    assert_same(&before, &snapshot(&world));
}

#[test]
fn the_plugin_entries_are_taken_out_of_the_project_settings() {
    let world = World::new();
    let claude = world.repo.join(".claude");
    fs::create_dir_all(&claude).unwrap();
    let mine = "{\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(ls)\"\n    ]\n  }\n}\n";
    let with_acs = "{\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(ls)\"\n    ]\n  },\n  \"extraKnownMarketplaces\": {\n    \"acs-plugins\": {\n      \"source\": {\n        \"source\": \"github\",\n        \"repo\": \"acs-hq/acs\"\n      }\n    }\n  },\n  \"enabledPlugins\": {\n    \"acs@acs-plugins\": true\n  }\n}\n";
    fs::write(claude.join("settings.json"), with_acs).unwrap();
    fs::write(
        claude.join("settings.local.json"),
        "{\n  \"enabledPlugins\": {\n    \"acs@acs-plugins\": true\n  }\n}\n",
    )
    .unwrap();
    let agents = world.repo.join(".agents/plugins/acs");
    fs::create_dir_all(&agents).unwrap();
    fs::write(agents.join("plugin.json"), "{}").unwrap();

    let out = acs(&world, &["uninstall", "--json"]);
    assert_success(&out);
    let report = json(&out);
    assert_eq!(step(&report, "claude_code_settings")["state"], "done");
    assert_eq!(step(&report, "antigravity_plugin")["state"], "done");
    assert_eq!(
        fs::read_to_string(claude.join("settings.json")).unwrap(),
        mine
    );
    assert!(!claude.join("settings.local.json").exists());
    assert!(!world.repo.join(".agents").exists());
    let notes = report["notes"].to_string();
    assert!(notes.contains(".claude/settings.json"), "{notes}");
}

#[test]
fn outside_a_repository_it_fails() {
    let world = World::new();
    let outside = world.tmp.path().join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    let out = cli_common::acs_in(&world, &outside, &["uninstall", "--json"]);
    assert!(!out.status.success());
    assert_eq!(json(&out)["error"]["code"], "not_a_repo");
}
