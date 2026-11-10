//! `acs verify` through the built binary: clean, tampered, unsigned and signed ledgers.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::Write;
use std::process::{Command, Output, Stdio};

use acs_git::refs::{CustomRefs, Pilot, SyncStorage, ledger_ref};
use common::{World, git};
use serde_json::{Value, json};

const SARA: &str = "sara@example.com";

fn record(pilot: &str, n: u32) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "spec": "acs/0.1",
        "session_id": format!("ses_{n:08}"),
        "seat": {
            "seat_id": format!("seat_{n:08}"),
            "pilot": pilot,
            "harness": "claude-code",
            "harness_session_id": format!("h{n}"),
            "worktree": "/w",
            "target_branch": "main",
            "started_at": "2026-10-29T14:02:11Z",
            "last_seen_at": "2026-10-29T14:31:02Z"
        },
        "claims": [],
        "files_changed": [],
        "guard_decisions": {"counts": {}, "notable": []},
        "commits": [],
        "lore_loaded": [],
        "started_at": "2026-10-29T14:02:11Z",
        "ended_at": "2026-10-29T14:31:02Z",
        "ended_reason": "clean",
        "acs_version": "0.1.0"
    }))
    .unwrap()
}

fn append(world: &World, n: u32) {
    CustomRefs::new(&world.repo, "origin")
        .append_ledger(&Pilot::new(SARA), &format!("ses_{n:08}"), &record(SARA, n))
        .unwrap();
}

fn out(world: &World, args: &[&str]) -> (i32, String) {
    let output: Output = world.run(args, b"", &world.repo);
    (
        output.status.code().unwrap(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

fn json_of(world: &World, args: &[&str]) -> (i32, Value) {
    let (code, text) = out(world, args);
    (
        code,
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}")),
    )
}

fn raw_git(world: &World, args: &[&str], stdin: Option<&[u8]>, env: &[(&str, &str)]) -> String {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&world.repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(bytes) = stdin {
        child.stdin.take().unwrap().write_all(bytes).unwrap();
    } else {
        drop(child.stdin.take());
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn no_ledger_is_nothing_to_verify() {
    let world = World::new();
    let (code, text) = out(&world, &["verify"]);
    assert_eq!(code, 0);
    assert!(text.contains("No ledgers"), "{text}");
    let (code, value) = json_of(&world, &["verify", "--json"]);
    assert_eq!((code, &value["ok"]), (0, &json!(true)));
    let (code, value) = json_of(
        &world,
        &["verify", "--pilot", "nobody@example.com", "--json"],
    );
    assert_eq!((code, &value["error"]["code"]), (1, &json!("no_ledger")));
}

#[test]
fn unsigned_commits_are_reported_and_only_fail_when_required() {
    let world = World::new();
    for n in 1..=3 {
        append(&world, n);
    }
    let (code, text) = out(&world, &["verify"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("3 record(s)") && text.contains("3 unsigned"),
        "{text}"
    );
    assert!(text.lines().all(|l| l.chars().count() <= 80), "{text}");
    let (code, value) = json_of(&world, &["verify", "--json", "--pilot", SARA]);
    assert_eq!(code, 0);
    assert_eq!(value["pilots"][0]["unsigned"], 3);
    assert_eq!(value["pilots"][0]["pilot"], SARA);
    let (code, text) = out(&world, &["verify", "--require-signed"]);
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("--require-signed"), "{text}");
}

#[test]
fn a_rewritten_unsigned_ledger_is_detected_after_the_first_verification() {
    let world = World::new();
    for n in 1..=3 {
        append(&world, n);
    }
    assert_eq!(out(&world, &["verify"]).0, 0);

    // Re-create the chain with the middle record edited, same author, nothing signed.
    let name = ledger_ref(&Pilot::new(SARA).id);
    let mut previous: Option<String> = None;
    for n in 1..=3u32 {
        let bytes = record(SARA, if n == 2 { 99 } else { n });
        let blob = raw_git(&world, &["hash-object", "-w", "--stdin"], Some(&bytes), &[]);
        let tree = raw_git(
            &world,
            &["mktree"],
            Some(format!("100644 blob {blob}\trecord.json\n").as_bytes()),
            &[],
        );
        let mut args = vec!["commit-tree".to_owned(), tree];
        if let Some(parent) = &previous {
            args.extend(["-p".to_owned(), parent.clone()]);
        }
        args.extend(["-m".to_owned(), format!("session ses_{n:08}")]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        previous = Some(raw_git(
            &world,
            &args,
            None,
            &[
                ("GIT_AUTHOR_NAME", "ACS"),
                ("GIT_AUTHOR_EMAIL", SARA),
                ("GIT_COMMITTER_NAME", "ACS"),
                ("GIT_COMMITTER_EMAIL", SARA),
            ],
        ));
    }
    raw_git(
        &world,
        &["update-ref", &name, &previous.unwrap()],
        None,
        &[],
    );

    let (code, value) = json_of(&world, &["verify", "--json"]);
    assert_eq!(code, 1, "{value}");
    assert_eq!(value["ok"], false);
    assert_eq!(
        value["pilots"][0]["problems"][0]["kind"],
        "rewritten_history"
    );
    let (code, text) = out(&world, &["verify"]);
    assert_eq!(code, 1);
    assert!(
        text.contains("PROBLEMS") && text.contains("rewritten"),
        "{text}"
    );
    assert!(text.lines().all(|l| l.chars().count() <= 80), "{text}");
    // Still reported next time, until the person accepts it.
    assert_eq!(out(&world, &["verify"]).0, 1);
    assert_eq!(out(&world, &["verify", "--accept-history"]).0, 0);
    assert_eq!(out(&world, &["verify"]).0, 0);
}

#[test]
fn a_commit_by_someone_else_in_a_ledger_fails() {
    let world = World::new();
    append(&world, 1);
    let name = ledger_ref(&Pilot::new(SARA).id);
    let parent = raw_git(&world, &["rev-parse", &name], None, &[]);
    let tree = raw_git(
        &world,
        &["rev-parse", &format!("{name}^{{tree}}")],
        None,
        &[],
    );
    let forged = raw_git(
        &world,
        &[
            "commit-tree",
            &tree,
            "-p",
            &parent,
            "-m",
            "session ses_00000001",
        ],
        None,
        &[
            ("GIT_AUTHOR_NAME", "x"),
            ("GIT_AUTHOR_EMAIL", "mallory@example.com"),
            ("GIT_COMMITTER_NAME", "x"),
            ("GIT_COMMITTER_EMAIL", "mallory@example.com"),
        ],
    );
    raw_git(&world, &["update-ref", &name, &forged], None, &[]);
    let (code, value) = json_of(&world, &["verify", "--json"]);
    assert_eq!(code, 1);
    assert_eq!(value["pilots"][0]["problems"][0]["kind"], "wrong_author");
}

#[test]
fn signed_ledgers_verify_and_a_changed_commit_does_not() {
    let world = World::new();
    let key = world.tmp.path().join("key");
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !made.is_ok_and(|s| s.success()) {
        eprintln!("skipped: ssh-keygen is not available");
        return;
    }
    let allowed = world.tmp.path().join("allowed");
    let public = std::fs::read_to_string(format!("{}.pub", key.display())).unwrap();
    std::fs::write(&allowed, format!("{SARA} {public}")).unwrap();
    for (k, v) in [
        ("commit.gpgsign", "true"),
        ("gpg.format", "ssh"),
        ("user.signingkey", key.to_str().unwrap()),
        ("gpg.ssh.allowedSignersFile", allowed.to_str().unwrap()),
    ] {
        git(&world.repo, &world.git_config, &["config", k, v]);
    }
    for n in 1..=2 {
        append(&world, n);
    }
    let (code, value) = json_of(&world, &["verify", "--json", "--require-signed"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["pilots"][0]["signed"], 2);

    // Change the tip commit after signing (its message), keeping the signature.
    let name = ledger_ref(&Pilot::new(SARA).id);
    let raw = raw_git(&world, &["cat-file", "commit", &name], None, &[]);
    let edited = format!("{}\n", raw.replacen("session ses_", "session xes_", 1));
    let id = raw_git(
        &world,
        &["hash-object", "-t", "commit", "-w", "--stdin"],
        Some(edited.as_bytes()),
        &[],
    );
    raw_git(&world, &["update-ref", &name, &id], None, &[]);
    let (code, value) = json_of(&world, &["verify", "--json"]);
    assert_eq!(code, 1, "{value}");
    let kinds: Vec<&Value> = value["pilots"][0]["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["kind"])
        .collect();
    assert!(kinds.contains(&&json!("bad_signature")), "{kinds:?}");
}
