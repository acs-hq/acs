//! `acs blame` through the built binary: a repository with human, agent and multi-session
//! commits, a session with no ledger record, an uncommitted change, and lines with no ACS data.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::needless_range_loop)]

mod common;

use std::fs;

use acs_core::Timestamp;
use acs_git::refs::{CustomRefs, Pilot, SyncStorage};
use acs_store::{Attribution, AttributionSource, NewSeat};
use common::{World, git};
use serde_json::{Value, json};

const SARA: &str = "sara@example.com";
const OMAR: &str = "omar@example.com";

fn record(pilot: &str, n: u32, harness: &str, intent: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "spec": "acs/0.1",
        "session_id": format!("ses_{n:08}"),
        "seat": {
            "seat_id": format!("seat_{n:08}"),
            "pilot": pilot,
            "harness": harness,
            "harness_session_id": format!("h{n}"),
            "worktree": "/w",
            "target_branch": "main",
            "started_at": "2026-10-29T14:02:11Z",
            "last_seen_at": "2026-10-29T14:31:02Z"
        },
        "intent": intent,
        "claims": [],
        "files_changed": [],
        "guard_decisions": {"counts": {}, "notable": []},
        "commits": [],
        "lore_loaded": [],
        "started_at": format!("2026-10-29T14:0{n}:11Z"),
        "ended_at": "2026-10-29T14:31:02Z",
        "ended_reason": "clean",
        "acs_version": "0.1.0"
    }))
    .unwrap()
}

fn commit_all(world: &World, message: &str) {
    git(&world.repo, &world.git_config, &["add", "-A"]);
    git(
        &world.repo,
        &world.git_config,
        &["commit", "-q", "-m", message],
    );
}

fn append_lines(world: &World, lines: &[&str]) {
    let path = world.repo.join("notes.txt");
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    fs::write(path, text).unwrap();
}

fn blame_json(world: &World) -> Vec<Value> {
    let output = world.run(&["blame", "notes.txt", "--json"], b"", &world.repo);
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    value["lines"].as_array().unwrap().clone()
}

#[test]
fn lines_are_attributed_to_human_agent_multi_session_and_uncommitted_work() {
    let world = World::new();
    let ledger = CustomRefs::new(&world.repo, "origin");
    let sara = Pilot::new(SARA);
    let omar = Pilot::new(OMAR);
    ledger
        .append_ledger(
            &sara,
            "ses_00000001",
            &record(SARA, 1, "claude-code", "Add the agent lines"),
        )
        .unwrap();
    ledger
        .append_ledger(
            &omar,
            "ses_00000002",
            &record(OMAR, 2, "codex", "Merge helper"),
        )
        .unwrap();

    // A session this machine knows, whose record was never written, and one for uncommitted work.
    let db = world.db();
    let now = Timestamp::now();
    let seat = |harness: &str, session: &str| {
        db.get_or_create_seat_info(
            &NewSeat {
                harness,
                harness_session_id: session,
                pilot: SARA,
                worktree: world.repo.to_str().unwrap(),
                branch: Some("main"),
                target_branch: "main",
            },
            now,
        )
        .unwrap()
        .0
    };
    let unrecorded = seat("cursor", "c1");
    db.set_seat_intent(&unrecorded.seat.seat_id, "Tidy the notes")
        .unwrap();
    let unrecorded = db.seat_info(&unrecorded.seat.seat_id).unwrap().unwrap();
    let editing = seat("claude-code", "live");

    // Commit 1: a human, no trailer. Commit 2: one agent session. Commit 3: a human merging
    // the work of two sessions, one with a record and one without.
    append_lines(&world, &["human one", "human two"]);
    commit_all(&world, "start the notes");
    append_lines(&world, &["agent one", "agent two"]);
    commit_all(&world, "agent work\n\nACS-Session: ses_00000001");
    append_lines(&world, &["combined"]);
    commit_all(
        &world,
        &format!(
            "merge the sessions\n\nACS-Session: ses_00000002\nACS-Session: {}",
            unrecorded.session_id
        ),
    );
    // Uncommitted: the live seat's attribution row.
    append_lines(&world, &["still editing"]);
    db.record_attributions(
        &editing.seat.seat_id,
        &[Attribution {
            path: "notes.txt".parse().unwrap(),
            pre_hash: None,
            post_hash: None,
        }],
        AttributionSource::Tool,
        now,
    )
    .unwrap();

    let lines = blame_json(&world);
    assert_eq!(lines.len(), 6);
    let sessions = |at: usize| -> Vec<String> {
        lines[at]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["session_id"].as_str().unwrap().to_owned())
            .collect()
    };
    // Human lines: no session.
    for at in 0..2 {
        assert_eq!(lines[at]["status"], "committed");
        assert!(sessions(at).is_empty(), "{:?}", lines[at]);
        assert!(lines[at]["commit"].is_string());
    }
    assert_eq!(lines[0]["text"], "human one");
    // The agent session resolves through the ledger.
    for at in 2..4 {
        assert_eq!(sessions(at), ["ses_00000001"]);
        let session = &lines[at]["sessions"][0];
        assert_eq!(session["pilot"], SARA);
        assert_eq!(session["harness"], "claude-code");
        assert_eq!(session["intent"], "Add the agent lines");
        assert_eq!(session["source"], "ledger");
    }
    // Two sessions in one commit: the other pilot's record, and a local seat.
    assert_eq!(
        sessions(4),
        ["ses_00000002".to_owned(), unrecorded.session_id.to_string()]
    );
    assert_eq!(lines[4]["sessions"][0]["pilot"], OMAR);
    assert_eq!(lines[4]["sessions"][0]["harness"], "codex");
    assert_eq!(lines[4]["sessions"][1]["source"], "local_seat");
    assert_eq!(lines[4]["sessions"][1]["harness"], "cursor");
    assert_eq!(lines[4]["sessions"][1]["intent"], "Tidy the notes");
    // The uncommitted line comes from the attribution table.
    assert_eq!(lines[5]["status"], "uncommitted");
    assert!(lines[5]["commit"].is_null());
    assert_eq!(sessions(5), [editing.session_id.to_string()]);
    assert_eq!(lines[5]["sessions"][0]["source"], "local_seat");
    assert_eq!(lines[5]["original_line"], 6);

    // Human output: runs grouped, within 80 columns, with a legend.
    let output = world.run(&["blame", "notes.txt"], b"", &world.repo);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.lines().all(|l| l.chars().count() <= 80), "{text}");
    assert!(text.contains("no session"), "{text}");
    assert!(text.contains("1-2"), "{text}");
    assert!(text.contains("3-4"), "{text}");
    assert!(text.contains("ses_00000002+1"), "{text}");
    assert!(text.contains("uncommitted"), "{text}");
    assert!(text.contains("Sessions"), "{text}");
    assert!(text.contains("intent: Add the agent lines"), "{text}");
    assert!(text.contains("no ledger record yet"), "{text}");
}

#[test]
fn an_untracked_file_is_uncommitted_and_a_missing_one_is_an_error() {
    let world = World::new();
    world.write("scratch.txt", "one\ntwo\n");
    let output = world.run(&["blame", "scratch.txt", "--json"], b"", &world.repo);
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let lines = value["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|l| l["status"] == "uncommitted"));
    assert!(
        lines
            .iter()
            .all(|l| l["sessions"].as_array().unwrap().is_empty())
    );

    let output = world.run(&["blame", "nope.txt", "--json"], b"", &world.repo);
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "blame");
}

#[test]
fn a_file_from_a_subdirectory_and_a_path_outside_the_repository() {
    let world = World::new();
    let sub = world.repo.join("src");
    let output = world.run(&["blame", "lib.rs", "--json"], b"", &sub);
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["path"], "src/lib.rs");
    assert_eq!(value["lines"][0]["status"], "committed");
    assert!(value["lines"][0]["sessions"].as_array().unwrap().is_empty());

    let outside = world.tmp.path().join("elsewhere.txt");
    fs::write(&outside, "x\n").unwrap();
    let output = world.run(
        &["blame", outside.to_str().unwrap(), "--json"],
        b"",
        &world.repo,
    );
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "outside");
}
