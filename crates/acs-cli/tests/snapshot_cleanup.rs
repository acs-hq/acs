//! Cleanup of the copies kept for undo (issue 68): `acs sync` keeps the snapshot store within its
//! budget, never touching the copies of running sessions, and `undo` and `doctor` say so.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::HashSet;
use std::process::{Command, Output, Stdio};

use common::{Agent, World, acs_bin};
use serde_json::Value;

/// Runs `acs <args>` in the world's repository with `extra` environment variables.
fn acs_env(world: &World, args: &[&str], extra: &[(&str, &str)]) -> Output {
    Command::new(acs_bin())
        .args(args)
        .current_dir(&world.repo)
        .env("ACS_HOME", &world.home)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .envs(extra.iter().copied())
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// One session that edits `path` to content of its own, then ends (or stays running).
fn session(world: &World, name: &str, path: &str, end: bool) -> String {
    let agent = Agent::new(world, "claude-code", name);
    agent.start();
    agent.pre_edit(path);
    // Content that does not compress to nothing, and differs per session.
    let body: String = (0..400)
        .map(|i| format!("{name}-{i}-{}\n", i * 7919 % 104_729))
        .collect();
    world.write(path, &body);
    agent.post_edit(path);
    if end {
        agent.end();
    }
    agent.info().session_id.to_string()
}

#[test]
fn sync_removes_copies_of_ended_sessions_over_budget_and_keeps_running_ones() {
    let world = World::new();
    // Each edits a file that exists, so undoing it needs the copy from before.
    let old = session(&world, "old", "a.txt", true);
    let newer = session(&world, "newer", "src/lib.rs", true);
    let running = session(&world, "running", "src/util.rs", false);

    let store = world.snapshots();
    let before = store.stored_bytes().unwrap();
    assert!(before > 0);
    // The confirmation of issue 68: nothing in the product shrinks the store. Without the fix
    // this stays at `before` however small the budget is.
    let budget = "1";
    let out = acs_env(
        &world,
        &["sync", "--wait"],
        &[("ACS_SNAPSHOT_BUDGET_BYTES", budget)],
    );
    assert!(out.status.success(), "{out:?}");

    let store = world.snapshots();
    let after = store.stored_bytes().unwrap();
    assert!(after < before, "store did not shrink: {before} -> {after}");
    let live: HashSet<_> = [running.parse().unwrap()].into();
    let report = store.gc(u64::MAX, &live).unwrap();
    assert_eq!(report.remaining_bytes, after);
    assert_eq!(
        report.live_bytes, after,
        "only the running session's copies are left"
    );

    // Undo of an evicted session says why it left the files alone.
    let out = acs_env(&world, &["undo", &old, "--json"], &[]);
    assert!(out.status.success(), "{out:?}");
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    let detail = view["files"][0]["detail"].as_str().unwrap();
    assert!(detail.contains("removed to save space"), "{detail}");
    assert_eq!(view["files"][0]["action"], "unknown");
    let out = acs_env(&world, &["undo", &newer], &[]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("removed to save space"));
    // The running session's copies are intact.
    let out = acs_env(&world, &["undo", &running, "--json"], &[]);
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(view["files"][0]["action"], "restore", "{view}");
}

#[test]
fn sync_within_the_budget_removes_nothing() {
    let world = World::new();
    let old = session(&world, "old", "a.txt", true);
    let before = world.snapshots().stored_bytes().unwrap();
    let out = acs_env(&world, &["sync", "--wait"], &[]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(world.snapshots().stored_bytes().unwrap(), before);
    let out = acs_env(&world, &["undo", &old, "--json"], &[]);
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(view["files"][0]["action"], "restore", "{view}");
}

#[test]
fn sync_leaves_the_copies_alone_with_history_off() {
    let world = World::new();
    session(&world, "old", "old.txt", true);
    let before = world.snapshots().stored_bytes().unwrap();
    // History off: nothing new is captured, so the store does not grow and old copies stay usable.
    // (Team sync is off by default, so the other tests also cover cleanup with `sync` off.)
    world.write(
        ".acs/policy/config.toml",
        "spec = \"acs/0.1\"\n\n[features]\nhistory = false\n",
    );
    let out = acs_env(
        &world,
        &["sync", "--wait"],
        &[("ACS_SNAPSHOT_BUDGET_BYTES", "1")],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(world.snapshots().stored_bytes().unwrap(), before);
}

#[test]
fn doctor_shows_the_store_and_warns_when_over_budget() {
    let world = World::new();
    session(&world, "old", "old.txt", true);
    let out = acs_env(&world, &["doctor", "--json"], &[]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let check = doc["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "snapshots")
        .unwrap_or_else(|| panic!("no snapshots check in {doc}"));
    assert_eq!(check["status"], "pass", "{check}");
    assert!(check["message"].as_str().unwrap().contains("500 MiB"));

    let out = acs_env(
        &world,
        &["doctor", "--json"],
        &[("ACS_SNAPSHOT_BUDGET_BYTES", "1")],
    );
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let check = doc["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "snapshots")
        .unwrap();
    assert_eq!(check["status"], "warn", "{check}");
    assert!(check["hint"].as_str().unwrap().contains("acs sync"));
}
