//! WP4.1 end to end: session records are written to the ledger when sessions end or crash, and
//! `acs log` shows them (decision 0030).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use acs_core::{OverrideMode, Timestamp};
use acs_store::NewOverride;
use common::{Agent, World};
use serde_json::Value;

/// `acs <args>` with the world's environment and `extra` variables, no stdin.
fn run_env(world: &World, args: &[&str], extra: &[(&str, &str)]) -> Output {
    Command::new(common::acs_bin())
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

fn log_json(world: &World, extra_args: &[&str], env: &[(&str, &str)]) -> Vec<Value> {
    let mut args = vec!["log", "--json"];
    args.extend_from_slice(extra_args);
    let out = run_env(world, &args, env);
    assert!(out.status.success(), "{out:?}");
    match serde_json::from_slice::<Value>(&out.stdout).unwrap() {
        Value::Array(items) => items,
        other => panic!("not an array: {other}"),
    }
}

fn text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn git_out(dir: &Path, config: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// How many commits all ledgers hold (records plus joins).
fn ledger_commits(world: &World) -> usize {
    let refs = git_out(
        &world.repo,
        &world.git_config,
        &["for-each-ref", "--format=%(refname)", "refs/acs/ledger"],
    );
    refs.lines()
        .map(|name| {
            git_out(
                &world.repo,
                &world.git_config,
                &["rev-list", "--count", name],
            )
            .parse::<usize>()
            .unwrap()
        })
        .sum()
}

/// An agent that edits `a.txt` under a claim, as a hook sequence does.
fn edit(agent: &Agent<'_>, world: &World, path: &str, content: &str) {
    agent.pre_edit(path);
    world.write(path, content);
    agent.post_edit(path);
}

#[test]
fn no_records_says_how_they_appear() {
    let world = World::new();
    assert!(log_json(&world, &[], &[]).is_empty());
    let out = run_env(&world, &["log"], &[]);
    assert!(out.status.success());
    let said = text(&out);
    assert!(said.contains("No session records yet"), "{said}");
    assert!(said.contains("when it ends"), "{said}");
}

#[test]
fn a_crashed_session_gets_a_complete_record_on_the_next_run() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "crash-1");
    agent.start();
    edit(&agent, &world, "a.txt", "changed\n");
    world
        .db()
        .set_seat_intent(&agent.seat_id(), "fix a.txt using --password hunter2")
        .unwrap();
    let info = agent.info();

    // Not silent long enough: still a live session, nothing recorded.
    assert!(log_json(&world, &[], &[]).is_empty());

    // No end event ever comes; the next run finds the seat silent.
    std::thread::sleep(Duration::from_millis(1300));
    let records = log_json(&world, &[], &[("ACS_CRASH_AFTER_SECS", "0")]);
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    assert_eq!(record["ended_reason"], "crashed");
    assert_eq!(record["session_id"], info.session_id.as_str());
    assert_eq!(record["seat"]["worktree"], "repo", "folder name only");
    assert_eq!(record["seat"]["pilot"], "sara@example.com");
    assert_eq!(record["files_changed"][0]["path"], "a.txt");
    assert!(record["files_changed"][0]["pre_hash"].is_string());
    assert!(record["files_changed"][0]["post_hash"].is_string());
    assert_eq!(record["claims"].as_array().unwrap().len(), 1);
    let intent = record["intent"].as_str().unwrap();
    assert!(
        intent.contains("***") && !intent.contains("hunter2"),
        "{intent}"
    );
    assert_eq!(
        record["ended_at"], record["seat"]["last_seen_at"],
        "a crashed session ends at the last sign of life"
    );
    assert!(record["pilot_id"].is_string() && record["commit"].is_string());
    assert!(
        !record
            .to_string()
            .contains(world.repo.to_str().unwrap().replace('\\', "\\\\").as_str())
    );

    // The same seat is not recorded twice.
    assert_eq!(
        log_json(&world, &[], &[("ACS_CRASH_AFTER_SECS", "0")]).len(),
        1
    );
    assert_eq!(ledger_commits(&world), 1);
}

#[test]
fn a_clean_session_lists_its_commits_and_a_resumed_one_gets_a_second_record() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "clean-1");
    agent.start();
    edit(&agent, &world, "a.txt", "one\n");
    let session = agent.info().session_id;
    common::git(&world.repo, &world.git_config, &["add", "-A"]);
    common::git(
        &world.repo,
        &world.git_config,
        &[
            "commit",
            "-q",
            "-m",
            "change a",
            "-m",
            &format!("ACS-Session: {session}"),
        ],
    );
    let head = git_out(&world.repo, &world.git_config, &["rev-parse", "HEAD"]);
    agent.end();

    let records = log_json(&world, &[], &[]);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["ended_reason"], "clean");
    assert_eq!(records[0]["commits"], serde_json::json!([head]));
    let human = text(&run_env(&world, &["log"], &[]));
    assert!(
        human.contains(session.as_str()) && human.contains("clean"),
        "{human}"
    );
    assert!(
        human.contains("1 file changed, 1 claim, 1 commit, 0 denied"),
        "{human}"
    );
    assert!(human.lines().all(|l| l.chars().count() <= 80), "{human}");
    assert_eq!(ledger_commits(&world), 1);

    // The session resumes, changes another file and ends again.
    agent.start();
    edit(&agent, &world, "src/util.rs", "pub fn util2() {}\n");
    agent.end();
    let records = log_json(&world, &[], &[]);
    assert_eq!(
        records.len(),
        1,
        "one session, latest record wins: {records:?}"
    );
    assert_eq!(records[0]["files_changed"].as_array().unwrap().len(), 2);
    assert_eq!(
        ledger_commits(&world),
        2,
        "the first record stays in the ledger"
    );
    assert_eq!(log_json(&world, &[], &[]).len(), 1);
    assert_eq!(ledger_commits(&world), 2, "nothing new to record");
}

#[test]
fn a_session_with_nothing_to_record_leaves_no_commit() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "idle-1");
    agent.start();
    agent.end();
    assert!(log_json(&world, &[], &[]).is_empty());
    assert_eq!(ledger_commits(&world), 0);
}

#[test]
fn sync_without_a_remote_still_writes_records() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "local-1");
    agent.start();
    edit(&agent, &world, "a.txt", "x\n");
    agent.end();
    let out = run_env(&world, &["sync", "--json"], &[]);
    assert!(out.status.success(), "{out:?}");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["status"], "local_only");
    assert_eq!(report["records"]["written"], 1);
    assert_eq!(ledger_commits(&world), 1);
}

#[test]
fn overrides_appear_in_the_log_and_in_the_record() {
    let world = World::new();
    world.write(
        ".acs/policy/territory.toml",
        "[[zone]]\npaths = [\"payments/**\"]\nkind = \"owned\"\nowners = [\"lina@example.com\"]\n",
    );
    world.write("payments/a.rs", "x\n");
    let now = Timestamp::now();
    world
        .db()
        .grant_override(&NewOverride {
            pattern: "payments/**",
            mode: OverrideMode::Once,
            granted_by: "sara@example.com",
            now,
            expires_at: Timestamp::from_second(now.as_second() + 900).unwrap(),
        })
        .unwrap();
    let agent = Agent::new(&world, "claude-code", "override-1");
    agent.start();
    edit(&agent, &world, "payments/a.rs", "y\n");
    // Ended in the store, not by the hook: a hook would start a detached sync in this repository.
    world
        .db()
        .end_seat(&agent.seat_id(), Timestamp::now())
        .unwrap();

    let records = log_json(&world, &[], &[]);
    let used = &records[0]["overrides"][0];
    assert_eq!(used["pattern"], "payments/**");
    assert_eq!(used["mode"], "once");
    assert_eq!(used["granted_by"], "sara@example.com");
    assert_eq!(used["paths"], serde_json::json!(["payments/a.rs"]));

    let human = text(&run_env(&world, &["log"], &[]));
    assert!(human.contains("Overrides used"), "{human}");
    assert!(human.contains("payments/**"), "{human}");
    assert!(human.contains("once"), "{human}");
    assert!(human.contains("granted by sara@example.com"), "{human}");
    assert!(human.contains("payments/a.rs"), "{human}");
    assert!(human.lines().all(|l| l.chars().count() <= 80), "{human}");
}

#[test]
fn filters_narrow_the_listing() {
    let world = World::new();
    let first = Agent::new(&world, "claude-code", "f1");
    first.start();
    edit(&first, &world, "a.txt", "1\n");
    first.end();
    std::thread::sleep(Duration::from_millis(30));
    let second = Agent::new(&world, "codex", "f2");
    second.start();
    edit(&second, &world, "src/lib.rs", "pub fn x() {}\n");
    second.end();

    assert_eq!(log_json(&world, &[], &[]).len(), 2);
    let newest = log_json(&world, &["--limit", "1"], &[]);
    assert_eq!(newest.len(), 1);
    assert_eq!(newest[0]["seat"]["harness"], "codex", "newest first");

    let by_harness = log_json(&world, &["--harness", "claude-code"], &[]);
    assert_eq!(by_harness.len(), 1);
    assert_eq!(by_harness[0]["files_changed"][0]["path"], "a.txt");

    assert_eq!(log_json(&world, &["--path", "src"], &[]).len(), 1);
    assert_eq!(log_json(&world, &["--path", "**/*.rs"], &[]).len(), 1);
    assert_eq!(log_json(&world, &["--path", "nothing/**"], &[]).len(), 0);
    assert_eq!(log_json(&world, &["--pilot", "SARA@"], &[]).len(), 2);
    assert_eq!(log_json(&world, &["--pilot", "lina"], &[]).len(), 0);
    assert_eq!(log_json(&world, &["--since", "1h"], &[]).len(), 2);
    assert_eq!(log_json(&world, &["--since", "2099-01-01"], &[]).len(), 0);

    let bad = run_env(&world, &["log", "--since", "soon", "--json"], &[]);
    assert!(!bad.status.success());
    let error: Value = serde_json::from_slice(&bad.stdout).unwrap();
    assert_eq!(error["error"]["code"], "invalid");
}

#[test]
fn ending_a_session_in_an_adopted_repository_writes_its_record_without_a_remote() {
    let world = World::new();
    world.write(".acs/policy/config.toml", "");
    let agent = Agent::new(&world, "claude-code", "adopted-1");
    agent.start();
    edit(&agent, &world, "a.txt", "x\n");
    agent.end();
    // The hook only starts a detached `acs sync`; the record follows on its own.
    let mut waited = 0;
    while ledger_commits(&world) == 0 && waited < 150 {
        std::thread::sleep(Duration::from_millis(100));
        waited += 1;
    }
    assert_eq!(
        ledger_commits(&world),
        1,
        "the detached sync wrote the record"
    );
    // Let the child finish before the repository is deleted.
    let _ = run_env(&world, &["sync", "--wait"], &[]);
}
