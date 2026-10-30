//! `acs doctor`: a healthy repository, a corrupt state database, a corrupt policy cache, an
//! invalid policy, loose permissions, a missing `acs` on `PATH`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod cli_common;
mod common;

use std::fs;

use cli_common::{acs, acs_in, assert_failure, assert_fits_80, assert_success, json, stdout};
use common::World;
use serde_json::Value;

fn check<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no check {id} in {doc}"))
}

fn status_of(doc: &Value, id: &str) -> String {
    check(doc, id)["status"].as_str().unwrap().to_owned()
}

/// A repository where `acs` has run once, so the state exists.
fn started() -> World {
    let world = World::new();
    assert_success(&acs(&world, &["init"]));
    assert_success(&acs(&world, &["status"]));
    world
}

#[test]
fn a_healthy_repository_passes() {
    let world = started();
    let out = acs(&world, &["doctor", "--json"]);
    assert_success(&out);
    let doc = json(&out);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["summary"]["fail"], 0);
    for id in [
        "acs_home",
        "repo_id",
        "state_db",
        "policy",
        "policy_cache",
        "acs_on_path",
        "gate",
    ] {
        assert_eq!(status_of(&doc, id), "pass", "{id}: {}", check(&doc, id));
    }
    assert_eq!(status_of(&doc, "git_hooks"), "pass");

    let out = acs(&world, &["doctor"]);
    assert_success(&out);
    let text = stdout(&out);
    assert_fits_80(&text);
    assert!(text.contains("State database"), "{text}");
    assert!(text.contains("0 failure(s)"), "{text}");
}

#[test]
fn a_corrupt_state_database_fails_and_fix_repairs_it() {
    let world = started();
    let db = world.repo_home().state_db();
    assert!(db.is_file());
    fs::write(&db, "this is definitely not a sqlite database\n".repeat(50)).unwrap();
    for suffix in ["-wal", "-shm"] {
        let mut companion = db.clone().into_os_string();
        companion.push(suffix);
        let _ = fs::remove_file(companion);
    }

    let out = acs(&world, &["doctor", "--json"]);
    assert_failure(&out);
    let doc = json(&out);
    assert_eq!(doc["ok"], false);
    assert_eq!(status_of(&doc, "state_db"), "fail");
    assert!(
        check(&doc, "state_db")["hint"]
            .as_str()
            .unwrap()
            .contains("--fix")
    );
    let text = stdout(&acs(&world, &["doctor"]));
    assert!(text.contains("FAIL  State database"), "{text}");
    assert!(text.contains("acs doctor --fix"), "{text}");
    assert_fits_80(&text);
    // Other commands name doctor, too.
    assert_failure(&acs(&world, &["claim", "a.txt"]));

    let out = acs(&world, &["doctor", "--fix", "--json"]);
    assert_success(&out);
    let doc = json(&out);
    let state = check(&doc, "state_db");
    assert_eq!(state["status"], "pass");
    assert_eq!(state["fixed"], true);
    assert!(state["message"].as_str().unwrap().contains(".corrupt-"));
    let aside: Vec<_> = fs::read_dir(db.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("state.db.corrupt-")
        })
        .collect();
    assert_eq!(aside.len(), 1, "the damaged file is kept, not deleted");

    // Everything works again, and a second doctor run has nothing to repair.
    assert_success(&acs(&world, &["claim", "a.txt"]));
    let again = json(&acs(&world, &["doctor", "--json"]));
    assert_eq!(again["ok"], true);
    assert_eq!(check(&again, "state_db")["fixed"], false);
    let text = stdout(&acs(&world, &["doctor", "--fix"]));
    assert_fits_80(&text);
}

#[test]
fn a_corrupt_policy_cache_fails_and_fix_clears_it() {
    let world = started();
    let cache = world.repo_home().cache_dir().join(acs_policy::CACHE_FILE);
    assert!(cache.is_file(), "a hook or status call wrote the cache");
    fs::write(&cache, "{not json").unwrap();

    let doc = json(&acs(&world, &["doctor", "--json"]));
    assert_eq!(status_of(&doc, "policy_cache"), "fail");
    let out = acs(&world, &["doctor", "--fix", "--json"]);
    assert_success(&out);
    let doc = json(&out);
    assert_eq!(check(&doc, "policy_cache")["fixed"], true);
    assert!(!cache.exists());
    // The next call rebuilds it.
    assert_success(&acs(&world, &["status"]));
    assert!(cache.is_file());
    assert_eq!(
        status_of(&json(&acs(&world, &["doctor", "--json"])), "policy_cache"),
        "pass"
    );
}

#[test]
fn an_invalid_policy_file_is_a_warning_that_shows_the_error() {
    let world = started();
    fs::write(
        world.repo.join(".acs/policy/guards.toml"),
        "sealed = 3\nunknown_key = true\n",
    )
    .unwrap();

    let out = acs(&world, &["doctor", "--json"]);
    assert_success(&out);
    let doc = json(&out);
    assert_eq!(doc["ok"], true);
    assert_eq!(status_of(&doc, "policy"), "warn");
    let message = check(&doc, "policy")["message"].as_str().unwrap();
    assert!(message.contains("guards.toml"), "{message}");

    let text = stdout(&acs(&world, &["doctor"]));
    assert!(text.contains("WARN  Policy"), "{text}");
    assert!(text.contains("guards.toml"), "{text}");
    assert_fits_80(&text);
    // ACS keeps working on the last valid policy, and status shows the warning too.
    let view = json(&acs(&world, &["status", "--json"]));
    assert!(!view["policy_warnings"].as_array().unwrap().is_empty());
}

#[test]
fn doctor_reports_the_gate_being_off() {
    let world = started();
    assert_success(&acs(&world, &["off"]));
    let doc = json(&acs(&world, &["doctor", "--json"]));
    assert_eq!(status_of(&doc, "gate"), "warn");
    assert!(
        check(&doc, "gate")["message"]
            .as_str()
            .unwrap()
            .contains("OFF")
    );
    assert_eq!(doc["ok"], true);
}

#[cfg(unix)]
#[test]
fn loose_permissions_on_acs_home_fail_and_fix_tightens_them() {
    use std::os::unix::fs::PermissionsExt;
    let world = started();
    let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&world.home), 0o700);
    fs::set_permissions(&world.home, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(
        world.repo_home().state_db(),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();

    let doc = json(&acs(&world, &["doctor", "--json"]));
    assert_eq!(status_of(&doc, "acs_home"), "fail");
    let doc = json(&acs(&world, &["doctor", "--fix", "--json"]));
    assert_eq!(check(&doc, "acs_home")["fixed"], true);
    assert_eq!(mode(&world.home), 0o700);
    assert_eq!(mode(&world.repo_home().state_db()), 0o600);
    assert_eq!(
        status_of(&json(&acs(&world, &["doctor", "--json"])), "acs_home"),
        "pass"
    );
}

#[cfg(unix)]
#[test]
fn a_missing_acs_binary_fails_only_when_a_plugin_is_installed() {
    let world = started();
    let path = cli_common::git_only_path(&world);
    let run = |args: &[&str]| cli_common::run_with_path(&world, &world.repo, &path, args);

    let doc = json(&run(&["doctor", "--json"]));
    assert_eq!(status_of(&doc, "acs_on_path"), "warn");

    // A project settings file that enables the plugin: hooks would run without the binary.
    fs::create_dir_all(world.repo.join(".claude")).unwrap();
    fs::write(
        world.repo.join(".claude/settings.json"),
        r#"{"enabledPlugins":{"acs@acs-plugins":true}}"#,
    )
    .unwrap();
    let out = run(&["doctor", "--json"]);
    assert_failure(&out);
    let doc = json(&out);
    assert_eq!(status_of(&doc, "acs_on_path"), "fail");
    assert_eq!(status_of(&doc, "plugin_claude-code"), "pass");
    let text = stdout(&run(&["doctor"]));
    assert_fits_80(&text);
    assert!(text.contains("FAIL  acs on PATH"), "{text}");
}

#[test]
fn an_undetected_plugin_gets_the_install_command() {
    let world = started();
    fs::create_dir_all(world.repo.join(".agents")).unwrap();
    let doc = json(&acs(&world, &["doctor", "--json"]));
    let plugin = check(&doc, "plugin_antigravity");
    assert_eq!(plugin["status"], "warn");
    assert!(
        plugin["hint"]
            .as_str()
            .unwrap()
            .contains("agy plugin install")
    );
}

#[test]
fn outside_a_repository_only_the_global_checks_run() {
    let world = World::new();
    let plain = world.tmp.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    let out = acs_in(&world, &plain, &["doctor", "--json"]);
    assert_success(&out);
    let doc = json(&out);
    assert_eq!(status_of(&doc, "acs_home"), "pass");
    assert_eq!(status_of(&doc, "repository"), "warn");
    assert!(
        doc["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["id"] != "state_db")
    );
    assert_success(&acs_in(&world, &plain, &["doctor"]));
}

#[test]
fn doctor_detects_a_removed_git_hook_and_fix_reinstalls_it() {
    let world = started();
    let hook = world.repo.join(".git/hooks/pre-push");
    assert!(hook.is_file());
    fs::remove_file(&hook).unwrap();

    let out = acs(&world, &["doctor", "--json"]);
    assert_failure(&out);
    let doc = json(&out);
    assert_eq!(status_of(&doc, "git_hooks"), "fail");
    let message = check(&doc, "git_hooks")["message"].as_str().unwrap();
    assert!(
        message.contains("pre-push") && !message.contains("pre-commit"),
        "{message}"
    );
    let text = stdout(&acs(&world, &["doctor"]));
    assert!(text.contains("FAIL  Git hooks"), "{text}");
    assert!(text.contains("acs doctor --fix"), "{text}");
    assert_fits_80(&text);

    let out = acs(&world, &["doctor", "--fix", "--json"]);
    assert_success(&out);
    let doc = json(&out);
    assert_eq!(check(&doc, "git_hooks")["fixed"], true);
    assert!(hook.is_file());
    assert_eq!(
        status_of(&json(&acs(&world, &["doctor", "--json"])), "git_hooks"),
        "pass"
    );
}

#[test]
fn doctor_detects_an_outdated_or_stripped_hook_block() {
    let world = started();
    // What `husky` or `pre-commit install` do: rewrite the file without the ACS block.
    fs::write(
        world.repo.join(".git/hooks/pre-commit"),
        "#!/bin/sh\necho regenerated\n",
    )
    .unwrap();
    let doc = json(&acs(&world, &["doctor", "--json"]));
    assert_eq!(status_of(&doc, "git_hooks"), "fail");
    assert!(
        check(&doc, "git_hooks")["message"]
            .as_str()
            .unwrap()
            .contains("ACS block is missing")
    );
}
