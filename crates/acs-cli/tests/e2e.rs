//! End-to-end scenarios: several seats on one repository, driven through the built `acs` binary
//! with canonical events (plan section 9.1: simulated agents, no model calls).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use acs_core::{NoticeKind, Outcome, ReasonCode};
use acs_store::{AttributionSource, FLAG_GATE_OFF};
use common::{Agent, World};
use serde_json::json;

#[test]
fn three_seats_share_one_repository() {
    let world = World::new();
    let claude = Agent::new(&world, "claude-code", "c1");
    let antigravity = Agent::new(&world, "antigravity", "g1");
    let third = Agent::new(&world, "canonical", "x1");
    for agent in [&claude, &antigravity, &third] {
        let start = agent.start();
        assert_eq!(start.outcome, Outcome::Allow);
        assert!(start.context[0].contains("You hold no claims yet"));
    }

    // The first to edit a file gets it.
    let d = claude.pre_edit("src/lib.rs");
    assert_eq!((d.outcome, d.reason_code), (Outcome::Allow, ReasonCode::Ok));
    assert_eq!(d.claims_acquired.len(), 1);
    assert_eq!(claude.claimed(), vec!["src/lib.rs"]);

    // The second is told who holds it, since when, and what to do.
    let d = antigravity.pre_edit("src/lib.rs");
    assert_eq!(d.outcome, Outcome::Deny);
    assert_eq!(d.reason_code, ReasonCode::ClaimConflict);
    assert!(d.message.contains("src/lib.rs"), "{}", d.message);
    assert!(d.message.contains("sara@example.com"), "{}", d.message);
    assert!(d.message.contains("claude-code"), "{}", d.message);
    assert!(d.claims_acquired.is_empty());

    // Others work on other files; the same seat re-editing its file is a heartbeat.
    assert_eq!(antigravity.pre_edit("src/util.rs").outcome, Outcome::Allow);
    assert_eq!(third.pre_edit("a.txt").outcome, Outcome::Allow);
    assert_eq!(claude.pre_edit("src/lib.rs").outcome, Outcome::Allow);
    assert_eq!(claude.claimed(), vec!["src/lib.rs"]);

    // The third seat sees everyone else's claims in its denial context at its next start.
    let again = third.start();
    let status = &again.context[0];
    assert!(status.contains("You hold: `a.txt`"), "{status}");
    assert!(status.contains("`src/lib.rs`"), "{status}");

    // The last decision of the denied seat is on record for `acs_why`.
    let last = world.db().last_decision(&antigravity.seat_id()).unwrap();
    assert_eq!(last.unwrap().outcome, Outcome::Allow, "its latest call was allowed");

    // Ending a session releases its claims, so another seat can proceed.
    assert_eq!(claude.end().outcome, Outcome::Allow);
    assert!(claude.claimed().is_empty());
    assert_eq!(antigravity.pre_edit("src/lib.rs").outcome, Outcome::Allow);
    assert_eq!(antigravity.claimed(), vec!["src/lib.rs", "src/util.rs"]);
}

#[test]
fn sealed_paths_are_denied_for_every_tool() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    for path in [".acs/policy/guards.toml", ".git/hooks/pre-commit", ".env", "app/.env.local"] {
        let d = agent.pre_edit(path);
        assert_eq!(d.outcome, Outcome::Deny, "{path}");
        assert_eq!(d.reason_code, ReasonCode::SealedPath, "{path}");
    }
    let d = agent.pre_shell("echo 'x' > .acs/policy/config.toml");
    assert_eq!(d.reason_code, ReasonCode::SealedPath);
    assert!(agent.claimed().is_empty(), "a denial acquires nothing");
}

#[test]
fn shell_deletes_need_a_claim() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    let other = Agent::new(&world, "antigravity", "s2");
    agent.start();
    other.start();

    let d = agent.pre_shell("rm src/util.rs");
    assert_eq!(d.outcome, Outcome::Deny);
    assert_eq!(d.reason_code, ReasonCode::DeleteOutsideClaims);

    // Claiming it first (by editing) makes the delete the seat's own business.
    assert_eq!(agent.pre_edit("src/util.rs").outcome, Outcome::Allow);
    assert_eq!(agent.pre_shell("rm src/util.rs").outcome, Outcome::Allow);

    // Someone else's claimed file is not deletable.
    assert_eq!(other.pre_edit("src/lib.rs").outcome, Outcome::Allow);
    let d = agent.pre_shell("rm src/lib.rs");
    assert_eq!(d.outcome, Outcome::Deny);

    // A recursive delete is expanded: it covers files nobody claimed.
    let d = agent.pre_shell("rm -rf src");
    assert_eq!(d.outcome, Outcome::Deny, "{d:?}");
    // And so is the repository root.
    let d = agent.pre_shell("rm -rf .");
    assert_ne!(d.outcome, Outcome::Allow, "{d:?}");
}

#[test]
fn banned_commands_and_unknown_targets() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    let d = agent.pre_shell("git reset --hard HEAD~1");
    assert_eq!((d.outcome, d.reason_code), (Outcome::Deny, ReasonCode::BannedCommand));
    // A delete whose target cannot be determined asks the human.
    let d = agent.pre_shell("rm -rf $BUILD_DIR");
    assert_eq!(
        (d.outcome, d.reason_code),
        (Outcome::Ask, ReasonCode::DestructiveUnknownTargets)
    );
}

#[test]
fn a_shell_command_that_edits_files_is_attributed_in_post_tool() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    let other = Agent::new(&world, "antigravity", "s2");
    agent.start();
    other.start();
    assert_eq!(other.pre_edit("src/util.rs").outcome, Outcome::Allow);
    world.tick();

    // The command is opaque to the analysis, so nothing can be predicted: only the diff knows.
    let command = "./generate.sh";
    assert_eq!(agent.pre_shell(command).outcome, Outcome::Allow);
    world.tick();
    world.write("a.txt", "changed by the script\n");
    world.write("generated/out.txt", "new file\n");
    world.write("target/debug/artifact", "ignored build output\n");
    world.write("noise.log", "ignored log\n");
    // Another seat's claimed file changing in the same window is not this command's doing.
    world.write("src/util.rs", "pub fn util() { /* other seat */ }\n");
    let d = agent.post_shell(command);
    assert_eq!(d.outcome, Outcome::Allow);

    let db = world.db();
    let rows = db.seat_attributions(&agent.seat_id()).unwrap();
    let by_path = |p: &str| rows.iter().find(|r| r.path.as_str() == p);
    let changed = by_path("a.txt").expect("a.txt attributed");
    assert_eq!(changed.via, AttributionSource::Shell);
    assert_eq!(
        changed.pre_hash,
        Some(acs_core::ContentHash::of(b"a\n")),
        "the pre-state is the staged content"
    );
    assert_eq!(
        changed.post_hash,
        Some(acs_core::ContentHash::of(b"changed by the script\n"))
    );
    let created = by_path("generated/out.txt").expect("new file attributed");
    assert_eq!(created.pre_hash, None);
    assert!(created.post_hash.is_some());
    assert!(by_path("target/debug/artifact").is_none(), "ignored");
    assert!(by_path("noise.log").is_none(), "ignored");
    assert!(by_path("src/util.rs").is_none(), "claimed by another seat");
    assert_eq!(rows.len(), 2, "{rows:?}");

    // The before-state of a changed tracked file was stored, so it can be undone.
    let snapshots = world.snapshots();
    let pre = snapshots
        .pre_image(&agent.info().session_id, &"a.txt".parse().unwrap())
        .unwrap()
        .expect("pre-image");
    match pre.state {
        acs_store::FileState::File { hash, .. } => {
            assert_eq!(hash, acs_core::ContentHash::of(b"a\n"));
        }
        acs_store::FileState::Absent => panic!("a.txt existed"),
    }
}

#[test]
fn a_predicted_shell_delete_is_attributed_without_a_scan() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    assert_eq!(agent.pre_edit("src/util.rs").outcome, Outcome::Allow);
    agent.post_edit("src/util.rs");
    assert_eq!(agent.pre_shell("rm src/util.rs").outcome, Outcome::Allow);
    fs::remove_file(world.repo.join("src/util.rs")).unwrap();
    agent.post_shell("rm src/util.rs");
    let rows = world.db().seat_attributions(&agent.seat_id()).unwrap();
    let last = rows.last().unwrap();
    assert_eq!(last.path.as_str(), "src/util.rs");
    assert_eq!(last.post_hash, None, "deleted");
    assert!(last.pre_hash.is_some());
}

#[test]
fn file_tool_edits_are_attributed_with_pre_and_post_hashes() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    assert_eq!(agent.pre_edit("a.txt").outcome, Outcome::Allow);
    world.write("a.txt", "edited\n");
    agent.post_edit("a.txt");
    // A second, unchanged call adds nothing.
    assert_eq!(agent.pre_edit("a.txt").outcome, Outcome::Allow);
    agent.post_edit("a.txt");
    let rows = world.db().seat_attributions(&agent.seat_id()).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].via, AttributionSource::Tool);
    assert_eq!(rows[0].pre_hash, Some(acs_core::ContentHash::of(b"a\n")));
    assert_eq!(rows[0].post_hash, Some(acs_core::ContentHash::of(b"edited\n")));
}

#[test]
fn ending_a_session_keeps_its_snapshots_and_a_resumed_session_gets_status_again() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    agent.pre_edit("a.txt");
    world.write("a.txt", "edited\n");
    agent.post_edit("a.txt");
    let session = agent.info().session_id;
    agent.end();
    assert!(agent.claimed().is_empty());
    assert!(!world.snapshots().list(&session).unwrap().is_empty(), "undo needs them");

    // `claude --resume` reuses the session id: the seat is revived with the same session id and
    // the status context is delivered again, whatever the reason for the start.
    let resumed = agent.start();
    assert!(resumed.context[0].contains("You hold no claims yet"));
    assert_eq!(agent.info().session_id, session);
    assert!(agent.info().ended_at.is_none());
}

#[test]
fn prompt_submit_returns_context_only_for_new_notices() {
    let world = World::new();
    let agent = Agent::new(&world, "antigravity", "s1");
    agent.start();
    let d = agent.send("prompt_submit", json!({"prompt": "do the thing"}));
    assert!(d.context.is_empty(), "{d:?}");

    world
        .db()
        .insert_notice(
            NoticeKind::ClaimReleased,
            &agent.seat_id(),
            "`src/lib.rs` was released.",
            acs_core::Timestamp::now(),
        )
        .unwrap();
    let d = agent.send("prompt_submit", json!({"prompt": "go on"}));
    assert_eq!(d.context, vec!["`src/lib.rs` was released."]);
    let d = agent.send("prompt_submit", json!({"prompt": "go on"}));
    assert!(d.context.is_empty(), "delivered once");
}

#[test]
fn pre_tool_context_delivers_notices_on_allow_but_not_on_deny() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    world
        .db()
        .insert_notice(
            NoticeKind::LoreStale,
            &agent.seat_id(),
            "lore is stale",
            acs_core::Timestamp::now(),
        )
        .unwrap();
    let d = agent.pre_edit(".acs/policy/guards.toml");
    assert_eq!(d.outcome, Outcome::Deny);
    assert!(d.context.is_empty());
    let d = agent.pre_edit("a.txt");
    assert_eq!(d.context, vec!["lore is stale"]);
    assert!(world.db().pending_notices(&agent.seat_id()).unwrap().is_empty());
}

#[test]
fn gate_off_allows_everything_but_sealed_paths() {
    let world = World::new();
    let one = Agent::new(&world, "claude-code", "s1");
    let two = Agent::new(&world, "antigravity", "s2");
    one.start();
    two.start();
    assert_eq!(one.pre_edit("a.txt").outcome, Outcome::Allow);
    assert_eq!(two.pre_edit("a.txt").outcome, Outcome::Deny);
    world.db().set_flag(FLAG_GATE_OFF, "1", acs_core::Timestamp::now()).unwrap();
    let d = two.pre_edit("a.txt");
    assert_eq!((d.outcome, d.reason_code), (Outcome::Allow, ReasonCode::GateOff));
    let d = two.pre_edit(".acs/policy/guards.toml");
    assert_eq!((d.outcome, d.reason_code), (Outcome::Deny, ReasonCode::SealedPath));
    let status = two.start();
    assert!(status.context[0].contains("`acs off` is active"));
}

#[test]
fn unknown_payloads_are_allowed_logged_and_warned_once_per_session() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    // A well-formed JSON object that is not a ToolEvent, from a session we can recognize.
    let payload = json!({"harness_session_id": "s9", "cwd": world.repo, "phase": "teleport"});
    let send = || {
        let out = world.run(
            &["hook", "--canonical", "pre_tool"],
            payload.to_string().as_bytes(),
            &world.repo,
        );
        assert_eq!(out.status.code(), Some(0));
        serde_json::from_slice::<acs_core::Decision>(&out.stdout).unwrap()
    };
    let first = send();
    assert_eq!((first.outcome, first.reason_code), (Outcome::Allow, ReasonCode::UnknownPayload));
    assert_eq!(first.warnings.len(), 1, "{first:?}");
    assert!(first.warnings[0].contains("could not read"));
    let second = send();
    assert_eq!(second.reason_code, ReasonCode::UnknownPayload);
    assert!(second.warnings.is_empty(), "once per session: {second:?}");

    let dir = world.repo_home().pending_dir().join("unknown");
    let saved: Vec<_> = fs::read_dir(dir).unwrap().collect();
    assert_eq!(saved.len(), 2, "the raw payloads are kept");

    // Garbage that recovers nothing is warned about every time.
    let garbage = |expect_warning: bool| {
        let out = world.run(&["hook", "--canonical", "pre_tool"], b"not json", &world.repo);
        let d: acs_core::Decision = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(d.reason_code, ReasonCode::UnknownPayload);
        assert_eq!(!d.warnings.is_empty(), expect_warning);
    };
    garbage(true);
    garbage(true);
}

#[test]
fn a_git_commit_leaves_a_pending_commit_marker() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    let marker_dir = world.repo_home().pending_dir().join("commit");
    assert!(!marker_dir.exists());
    assert_eq!(agent.pre_shell("git status").outcome, Outcome::Allow);
    assert!(!marker_dir.exists(), "only commits leave a marker");
    assert_eq!(agent.pre_shell("git commit -am 'fix refunds'").outcome, Outcome::Allow);

    let key = acs_cli::hook::markers::worktree_key(&world.repo);
    let text = fs::read_to_string(marker_dir.join(format!("{key}.json"))).unwrap();
    let marker: serde_json::Value = serde_json::from_str(&text).unwrap();
    let info = agent.info();
    assert_eq!(marker["session_id"], info.session_id.as_str());
    assert_eq!(marker["seat_id"], info.seat.seat_id.as_str());
    assert_eq!(marker["harness"], "claude-code");
    assert!(marker["created_at"].as_str().unwrap().ends_with('Z'));
    let read = acs_cli::hook::markers::read_commit_marker(
        &world.repo_home().pending_dir(),
        &world.repo,
        acs_core::Timestamp::now(),
    );
    assert!(read.is_some());

    // Several sessions in one work tree: the last writer wins.
    let other = Agent::new(&world, "antigravity", "s2");
    other.start();
    other.pre_shell("git commit -m other");
    let text = fs::read_to_string(marker_dir.join(format!("{key}.json"))).unwrap();
    assert!(text.contains(other.info().session_id.as_str()));
}

#[test]
fn a_broken_state_database_degrades_as_the_spec_says() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    agent.pre_edit("a.txt");
    // Damage the database: not a SQLite file any more.
    let db_file = world.repo_home().state_db();
    for suffix in ["-wal", "-shm"] {
        let mut side = db_file.clone().into_os_string();
        side.push(suffix);
        let _ = fs::remove_file(side);
    }
    fs::write(&db_file, b"this is not a database, just text, long enough to look like a page.....").unwrap();

    // Ordinary edits go through with a warning; destructive actions are denied; sealed paths
    // and banned commands still hold.
    let d = agent.pre_edit("src/lib.rs");
    assert_eq!((d.outcome, d.reason_code), (Outcome::Allow, ReasonCode::StateUnavailable));
    assert!(!d.warnings.is_empty());
    let d = agent.pre_shell("rm src/lib.rs");
    assert_eq!((d.outcome, d.reason_code), (Outcome::Deny, ReasonCode::StateUnavailable));
    let d = agent.pre_edit(".git/config");
    assert_eq!(d.reason_code, ReasonCode::SealedPath);
    let d = agent.pre_shell("git reset --hard");
    assert_eq!(d.reason_code, ReasonCode::BannedCommand);
    // Post-tool and session end never fail.
    assert_eq!(agent.post_edit("src/lib.rs").outcome, Outcome::Allow);
    assert_eq!(agent.end().outcome, Outcome::Allow);
}

#[test]
fn an_invalid_policy_falls_back_with_a_warning() {
    let world = World::new();
    world.write(".acs/policy/guards.toml", "sealed = [");
    let agent = Agent::new(&world, "claude-code", "s1");
    let d = agent.start();
    assert!(d.warnings.iter().any(|w| w.contains("guards.toml")), "{d:?}");
    let d = agent.pre_edit("a.txt");
    assert_eq!(d.outcome, Outcome::Allow);
    assert!(!d.warnings.is_empty(), "the fallback is reported on every call");
    assert_eq!(agent.pre_edit(".git/config").outcome, Outcome::Deny, "defaults still seal");
}

#[test]
fn outside_a_repository_and_for_unknown_harnesses() {
    let world = World::new();
    let elsewhere = world.tmp.path().join("not-a-repo");
    fs::create_dir_all(&elsewhere).unwrap();
    let event = json!({"spec":"acs/0.1","harness":"canonical","harness_session_id":"s","phase":"pre_tool",
        "cwd": elsewhere, "tool":{"name":"Edit","kind":"edit"},"paths":["x"]});
    let out = world.run(
        &["hook", "--canonical", "pre_tool"],
        event.to_string().as_bytes(),
        &elsewhere,
    );
    assert_eq!(out.status.code(), Some(0));
    let d: acs_core::Decision = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((d.outcome, d.reason_code), (Outcome::Allow, ReasonCode::Ok));

    let out = world.run(&["hook", "no-such-harness"], b"{}", &world.repo);
    assert_eq!(out.status.code(), Some(1), "never 2: that blocks some harnesses");
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown harness"));
}

#[test]
fn a_move_checks_both_ends() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    // Destination in a sealed place.
    let d = agent.pre_tool("Move", "move", &["a.txt", ".git/hooks/pre-commit"]);
    assert_eq!(d.reason_code, ReasonCode::SealedPath);
    // Source not claimed.
    let d = agent.pre_tool("Move", "move", &["a.txt", "b.txt"]);
    assert_eq!(d.reason_code, ReasonCode::DeleteOutsideClaims);
    assert_eq!(agent.pre_edit("a.txt").outcome, Outcome::Allow);
    let d = agent.pre_tool("Move", "move", &["a.txt", "b.txt"]);
    assert_eq!(d.outcome, Outcome::Allow, "{d:?}");
    assert!(agent.claimed().contains(&"b.txt".to_owned()));
}

#[test]
fn an_edit_that_escapes_the_repository_asks() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    let d = agent.pre_edit("../outside.txt");
    assert_eq!((d.outcome, d.reason_code), (Outcome::Ask, ReasonCode::PathEscapesRepo));
}

#[test]
fn concurrent_hooks_never_double_grant_a_claim() {
    let world = World::new();
    let agents: Vec<Agent<'_>> = (0..6)
        .map(|i| Agent::new(&world, "claude-code", &format!("race{i}")))
        .collect();
    for agent in &agents {
        agent.start();
    }
    let outcomes: Vec<Outcome> = std::thread::scope(|scope| {
        let handles: Vec<_> = agents
            .iter()
            .map(|agent| scope.spawn(|| agent.pre_edit("src/lib.rs").outcome))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(outcomes.iter().filter(|o| **o == Outcome::Allow).count(), 1, "{outcomes:?}");
    assert_eq!(outcomes.iter().filter(|o| **o == Outcome::Deny).count(), 5);
}
