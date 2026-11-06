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

    for hook in ["pre-commit", "prepare-commit-msg", "pre-push"] {
        let text = fs::read_to_string(world.repo.join(".git/hooks").join(hook)).unwrap();
        assert!(text.contains("acs git-hook"), "{hook}: {text}");
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
    let hooks = report["git_hooks"]["hooks"].as_array().unwrap();
    assert_eq!(hooks.len(), 3);
    assert!(
        hooks.iter().all(|h| h["action"] == "unchanged"),
        "{hooks:?}"
    );

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
    assert!(text.contains("acs plugin install claude-code"), "{text}");
    assert!(text.contains("acs plugin install antigravity"), "{text}");
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

#[test]
fn init_installs_the_git_hooks_and_chains_an_existing_one() {
    let world = World::new();
    let hook = world.repo.join(".git/hooks/pre-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    let original = "#!/bin/sh\necho my own pre-commit\n";
    fs::write(&hook, original).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let report = json(&acs(&world, &["init", "--json"]));
    assert_eq!(report["ok"], true);
    assert_eq!(report["changed"], true);
    let actions: Vec<(&str, &str)> = report["git_hooks"]["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| (h["name"].as_str().unwrap(), h["action"].as_str().unwrap()))
        .collect();
    assert_eq!(
        actions,
        vec![
            ("pre-commit", "injected"),
            ("prepare-commit-msg", "created"),
            ("pre-push", "created")
        ]
    );
    let text = fs::read_to_string(&hook).unwrap();
    assert!(text.contains("echo my own pre-commit") && text.contains("acs git-hook pre-commit"));

    let human = stdout(&acs(&world, &["init"]));
    assert!(human.contains("unchanged pre-commit"), "{human}");
    assert_fits_80(&human);
}

#[test]
fn init_notes_that_husky_9_hook_files_are_committed() {
    let world = World::new();
    let stubs = world.repo.join(".husky/_");
    fs::create_dir_all(&stubs).unwrap();
    fs::write(stubs.join("h"), "#!/usr/bin/env sh\n").unwrap();
    git(
        &world.repo,
        &world.git_config,
        &["config", "core.hooksPath", ".husky/_"],
    );
    let out = acs(&world, &["init"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("husky 9 detected"), "{text}");
    assert!(world.repo.join(".husky/pre-commit").is_file());
    assert_fits_80(&text);
}

/// Commits `count` changes to `file` as `email`, before any ACS hook is installed.
fn commits_by(world: &World, email: &str, file: &str, count: usize) {
    let path = world.repo.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for i in 0..count {
        fs::write(&path, format!("{email} {i}\n")).unwrap();
        git(&world.repo, &world.git_config, &["add", "-A"]);
        let author = format!("user.email={email}");
        git(
            &world.repo,
            &world.git_config,
            &["-c", &author, "commit", "-q", "-m", "change"],
        );
    }
}

/// A team repository: `payments/` is Sara's, `lib/` is Omar's, `docs/` is documentation.
fn team_world() -> World {
    let world = World::new();
    commits_by(&world, "sara@example.com", "payments/refund.rs", 10);
    commits_by(&world, "omar@example.com", "lib/money.rs", 11);
    commits_by(&world, "sara@example.com", "lib/money.rs", 1);
    commits_by(&world, "omar@example.com", "docs/guide.md", 2);
    world
}

fn zones(report: &serde_json::Value) -> Vec<String> {
    report["territory"]["proposal"]["zones"]
        .as_array()
        .unwrap()
        .iter()
        .map(|z| {
            let owners: Vec<&str> = z["owners"]
                .as_array()
                .map(|o| o.iter().map(|v| v.as_str().unwrap()).collect())
                .unwrap_or_default();
            format!(
                "{} {} {}",
                z["kind"].as_str().unwrap(),
                z["folder"].as_str().unwrap(),
                owners.join(",")
            )
        })
        .collect()
}

fn territory_action(report: &serde_json::Value) -> String {
    report["policy_files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["file"] == "territory.toml")
        .unwrap()["action"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn loaded_zones(world: &World) -> Vec<acs_core::Zone> {
    let loaded = acs_policy::load(
        &world.repo.join(".acs/policy"),
        acs_policy::LoadOptions::default(),
    )
    .unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    loaded.policy.files().territory.zone.clone()
}

#[test]
fn init_proposes_territory_and_writes_it_only_with_yes() {
    let world = team_world();
    let expected = [
        "open docs ",
        "owned lib omar@example.com",
        "owned payments sara@example.com",
    ];

    // Without a terminal and without --yes: reported, the template is written.
    let first = acs(&world, &["init", "--json"]);
    assert_success(&first);
    let report = json(&first);
    assert_eq!(report["territory"]["status"], "proposed");
    assert_eq!(zones(&report), expected);
    assert_eq!(report["territory"]["proposal"]["authors"], 2);
    assert!(loaded_zones(&world).is_empty());
    let human = stdout(&acs(&world, &["init"]));
    assert!(human.contains("acs init --yes"), "{human}");
    assert!(
        human.contains("owned  payments/  sara@example.com"),
        "{human}"
    );
    assert_fits_80(&human);

    // --yes replaces the untouched template.
    let second = acs(&world, &["init", "--yes", "--json"]);
    assert_success(&second);
    let report = json(&second);
    assert_eq!(report["territory"]["status"], "applied");
    assert_eq!(territory_action(&report), "replaced");
    assert_eq!(report["changed"], true);
    let written = loaded_zones(&world);
    assert_eq!(written.len(), 3);
    assert_eq!(written[0].paths, ["docs/**"]);
    assert_eq!(written[0].kind, acs_core::ZoneKind::Open);
    let text = fs::read_to_string(world.repo.join(".acs/policy/territory.toml")).unwrap();
    assert!(text.contains("# payments/: 10 of 10 commits"), "{text}");
    assert!(text.contains("owners = [\"omar@example.com\"]"), "{text}");

    // The written file counts as edited: nothing is proposed or changed again.
    let before = snapshot(&world);
    let third = acs(&world, &["init", "--yes", "--json"]);
    let report = json(&third);
    assert_eq!(report["territory"]["status"], "kept");
    assert!(report["territory"].get("proposal").is_none());
    assert_eq!(report["changed"], false);
    assert_eq!(snapshot(&world), before);
}

#[test]
fn init_yes_on_a_fresh_repository_writes_the_proposal_directly() {
    let world = team_world();
    let out = acs(&world, &["init", "--yes"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains("written from folder names and git history"),
        "{text}"
    );
    assert!(text.contains("does not enforce owned zones"), "{text}");
    assert_fits_80(&text);
    assert_eq!(loaded_zones(&world).len(), 3);
}

#[test]
fn an_edited_territory_is_never_replaced() {
    let world = team_world();
    let dir = world.repo.join(".acs/policy");
    fs::create_dir_all(&dir).unwrap();
    let mine = "[[zone]]\npaths = [\"lib/**\"]\nkind = \"open\"\n";
    fs::write(dir.join("territory.toml"), mine).unwrap();
    let report = json(&acs(&world, &["init", "--yes", "--json"]));
    assert_eq!(report["territory"]["status"], "kept");
    assert_eq!(territory_action(&report), "exists");
    assert_eq!(
        fs::read_to_string(dir.join("territory.toml")).unwrap(),
        mine
    );
}

#[test]
fn the_thresholds_can_be_raised() {
    let world = team_world();
    let report = json(&acs(
        &world,
        &["init", "--json", "--min-commits", "11", "--min-share", "95"],
    ));
    // payments/ has 10 commits (under 11); lib/ has 11 of 12 by Omar (92%, under 95%).
    assert_eq!(zones(&report), ["open docs "]);
    let refused = acs(&world, &["init", "--min-share", "50"]);
    assert_failure(&refused);
}

#[test]
fn a_one_author_repository_gets_no_owned_zones() {
    let world = World::new();
    commits_by(&world, "sara@example.com", "payments/refund.rs", 15);
    let out = acs(&world, &["init", "--yes", "--json"]);
    assert_success(&out);
    let report = json(&out);
    assert_eq!(report["territory"]["status"], "nothing_proposed");
    assert_eq!(territory_action(&report), "created");
    let notes = report["territory"]["proposal"]["notes"].to_string();
    assert!(notes.contains("Only one author"), "{notes}");
    assert!(loaded_zones(&world).is_empty());
    let human = stdout(&acs(&world, &["init"]));
    assert!(human.contains("nothing to propose"), "{human}");
}

#[test]
fn old_history_is_left_out() {
    let world = World::new();
    let path = world.repo.join("payments/refund.rs");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for email in ["sara@example.com", "omar@example.com"] {
        for n in 0..10 {
            fs::write(&path, format!("{email} {n}\n")).unwrap();
            git(&world.repo, &world.git_config, &["add", "-A"]);
            let status = Command::new("git")
                .arg("-C")
                .arg(&world.repo)
                .args(["-c", &format!("user.email={email}")])
                .args(["commit", "-q", "-m", "old"])
                .env("GIT_CONFIG_GLOBAL", &world.git_config)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_DATE", "2020-01-01T12:00:00Z")
                .env("GIT_COMMITTER_DATE", "2020-01-01T12:00:00Z")
                .status()
                .unwrap();
            assert!(status.success());
        }
    }
    let report = json(&acs(&world, &["init", "--json"]));
    // The 2020 commits are outside the window, so neither author can own payments/.
    assert!(report["territory"]["proposal"]["commits"].as_u64().unwrap() <= 1);
    assert_eq!(report["territory"]["status"], "nothing_proposed");
}
