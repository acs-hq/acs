//! The shared operations of `acs_cli::runtime`, called in-process the way the CLI and the MCP
//! server will call them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use acs_cli::runtime::seats::{self, HUMAN_HARNESS};
use acs_cli::runtime::{
    ClaimOutcome, RepoContext, claim_paths, last_decision, release_paths, set_intent, status,
};
use acs_core::{ClaimOrigin, Outcome, ReasonCode, Timestamp};
use common::{Agent, World};

fn open(world: &World) -> RepoContext {
    RepoContext::open_with(&world.repo, Ok(world.acs_home())).unwrap()
}

fn refused(outcome: ClaimOutcome) -> (ReasonCode, String) {
    match outcome {
        ClaimOutcome::Refused {
            reason_code,
            message,
        } => (reason_code, message),
        ClaimOutcome::Granted { .. } => panic!("expected a refusal"),
    }
}

#[test]
fn opening_a_directory_outside_a_repository_fails_cleanly() {
    let world = World::new();
    let elsewhere = world.tmp.path().join("plain");
    fs::create_dir_all(&elsewhere).unwrap();
    assert!(RepoContext::open_with(&elsewhere, Ok(world.acs_home())).is_err());
}

#[test]
fn a_context_reports_state_problems_without_failing_to_open() {
    let world = World::new();
    let bad_home = acs_store::AcsHome::resolve(None, None);
    let ctx = RepoContext::open_with(&world.repo, bad_home).unwrap();
    assert!(ctx.db().is_err());
    let view = status(&ctx, None, Timestamp::now());
    assert!(view.state_unavailable.is_some());
    assert!(
        view.sealed.iter().any(|g| g == ".git/**"),
        "policy still works"
    );
}

#[test]
fn human_seats_are_one_per_pilot_and_worktree() {
    let world = World::new();
    let ctx = open(&world);
    let db = ctx.db().unwrap();
    let now = Timestamp::now();
    let sara = seats::human_seat(&ctx, db, "sara@example.com", now).unwrap();
    assert!(sara.created);
    assert_eq!(sara.info.seat.harness, HUMAN_HARNESS);
    let again = seats::human_seat(&ctx, db, "sara@example.com", now).unwrap();
    assert!(!again.created);
    assert_eq!(again.info.seat.seat_id, sara.info.seat.seat_id);
    let omar = seats::human_seat(&ctx, db, "omar@example.com", now).unwrap();
    assert_ne!(omar.info.seat.seat_id, sara.info.seat.seat_id);
    // Human seats never look crashed.
    let later = Timestamp::from_second(now.as_second() + 100_000).unwrap();
    assert!(db.stale_live_seats(later).unwrap().is_empty());
}

#[test]
fn claiming_and_releasing_go_through_the_same_gate_as_the_hook() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    agent.pre_edit("src/lib.rs");

    let ctx = open(&world);
    let db = ctx.db().unwrap();
    let now = Timestamp::now();
    let human = seats::human_seat(&ctx, db, "sara@example.com", now)
        .unwrap()
        .info;

    // The agent's claim blocks the human's, with the engine's explanation.
    let (code, message) =
        refused(claim_paths(&ctx, &human, &["src/lib.rs".to_owned()], &world.repo, now).unwrap());
    assert_eq!(code, ReasonCode::ClaimConflict);
    assert!(
        message.contains("src/lib.rs") && message.contains("claude-code"),
        "{message}"
    );
    // The refusal is on record for `acs_why`.
    let last = last_decision(&ctx, &human.seat.seat_id).unwrap().unwrap();
    assert_eq!(
        (last.outcome, last.reason_code),
        (Outcome::Deny, ReasonCode::ClaimConflict)
    );

    // Sealed paths are refused too.
    let (code, _) =
        refused(claim_paths(&ctx, &human, &[".git/config".to_owned()], &world.repo, now).unwrap());
    assert_eq!(code, ReasonCode::SealedPath);
    let (code, _) = refused(claim_paths(&ctx, &human, &[], &world.repo, now).unwrap());
    assert_eq!(code, ReasonCode::Ok);

    // A free file is granted with origin `human`; claiming again is a heartbeat.
    let granted = |outcome| match outcome {
        ClaimOutcome::Granted {
            paths, claim_ids, ..
        } => (paths, claim_ids),
        ClaimOutcome::Refused { message, .. } => panic!("refused: {message}"),
    };
    let (paths, ids) =
        granted(claim_paths(&ctx, &human, &["a.txt".to_owned()], &world.repo, now).unwrap());
    assert_eq!(paths.len(), 1);
    let (_, ids_again) =
        granted(claim_paths(&ctx, &human, &["a.txt".to_owned()], &world.repo, now).unwrap());
    assert_eq!(ids, ids_again);
    let held = db.live_claims(now).unwrap();
    let mine = held
        .iter()
        .find(|h| h.claim.scope.path().as_str() == "a.txt")
        .unwrap();
    assert_eq!(mine.claim.origin, ClaimOrigin::Human);
}

#[test]
fn claiming_a_directory_takes_every_file_in_it_or_nothing() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    agent.pre_edit("src/util.rs");
    let ctx = open(&world);
    let db = ctx.db().unwrap();
    let now = Timestamp::now();
    let human = seats::human_seat(&ctx, db, "sara@example.com", now)
        .unwrap()
        .info;
    // One file of the directory is held by the agent: all or nothing.
    let (code, message) =
        refused(claim_paths(&ctx, &human, &["src".to_owned()], &world.repo, now).unwrap());
    assert_eq!(code, ReasonCode::ClaimConflict);
    assert!(message.contains("src/util.rs"), "{message}");
    assert!(
        db.live_claims(now)
            .unwrap()
            .iter()
            .all(|h| h.claim.seat_id != human.seat.seat_id),
        "nothing was taken"
    );
    // After the agent leaves, the whole directory is claimable.
    agent.end();
    match claim_paths(&ctx, &human, &["src".to_owned()], &world.repo, now).unwrap() {
        ClaimOutcome::Granted { paths, .. } => {
            let names: Vec<_> = paths.iter().map(|p| p.as_str()).collect();
            assert_eq!(names, vec!["src/lib.rs", "src/util.rs"]);
        }
        ClaimOutcome::Refused { message, .. } => panic!("{message}"),
    }
}

#[test]
fn releasing_only_touches_the_seats_own_claims() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    agent.pre_edit("src/lib.rs");
    let ctx = open(&world);
    let db = ctx.db().unwrap();
    let now = Timestamp::now();
    let human = seats::human_seat(&ctx, db, "sara@example.com", now)
        .unwrap()
        .info;
    claim_paths(&ctx, &human, &["a.txt".to_owned()], &world.repo, now).unwrap();

    let report = release_paths(
        &ctx,
        &human,
        &[
            "a.txt".to_owned(),
            "src/lib.rs".to_owned(),
            "../x".to_owned(),
        ],
        &world.repo,
        now,
    )
    .unwrap();
    let names = |v: &[acs_core::RepoPath]| v.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    assert_eq!(names(&report.released), vec!["a.txt"]);
    assert_eq!(
        names(&report.not_held),
        vec!["src/lib.rs"],
        "the agent's claim is not ours"
    );
    assert_eq!(report.outside, vec!["../x"]);
    assert_eq!(agent.claimed(), vec!["src/lib.rs"]);
}

#[test]
fn intent_is_trimmed_bounded_and_required() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    let ctx = open(&world);
    let id = agent.seat_id();
    set_intent(&ctx, &id, "  refactor refunds  ").unwrap();
    assert_eq!(agent.info().intent.as_deref(), Some("refactor refunds"));
    assert!(set_intent(&ctx, &id, "   ").is_err());
    set_intent(&ctx, &id, &"x".repeat(10_000)).unwrap();
    assert_eq!(
        agent.info().intent.unwrap().chars().count(),
        acs_cli::runtime::MAX_INTENT_CHARS
    );
    assert!(
        last_decision(&ctx, &id).unwrap().is_none(),
        "no pre-tool event yet"
    );
}

#[test]
fn status_lists_mine_others_sealed_gate_and_remote() {
    let world = World::new();
    let one = Agent::new(&world, "claude-code", "s1");
    let two = Agent::new(&world, "antigravity", "s2");
    one.start();
    two.start();
    one.pre_edit("a.txt");
    two.pre_edit("src/lib.rs");
    two.pre_edit("src/util.rs");

    let ctx = open(&world);
    let now = Timestamp::now();
    let view = status(&ctx, Some(&one.info()), now);
    assert_eq!(view.worktree, world.repo.to_string_lossy());
    assert_eq!(view.branch.as_deref(), Some("main"));
    assert_eq!(view.target_branch, "main");
    assert!(!view.gate_off);
    assert_eq!(
        view.my_claims
            .iter()
            .map(|c| c.path.as_str())
            .collect::<Vec<_>>(),
        vec!["a.txt"]
    );
    assert_eq!(view.other_claims.len(), 2);
    assert!(
        view.other_claims
            .iter()
            .all(|c| c.harness.as_deref() == Some("antigravity"))
    );
    assert!(view.sealed.contains(&".acs/**".to_owned()));
    assert!(!view.remote.configured && !view.remote.stale);
    assert_eq!(view.seat.as_ref().unwrap().harness, "claude-code");
    let text = view.summary_lines().join("\n");
    assert!(text.contains("You hold: `a.txt`"), "{text}");
    assert!(text.contains("Claimed by others"), "{text}");

    // It serializes for `--json`.
    let json = serde_json::to_value(&view).unwrap();
    assert_eq!(json["my_claims"][0]["path"], "a.txt");
    assert_eq!(json["remote"]["configured"], false);

    // Without a seat everything is "others".
    let anon = status(&ctx, None, now);
    assert!(anon.my_claims.is_empty());
    assert_eq!(anon.other_claims.len(), 3);
}

#[test]
fn the_remote_is_only_stale_once_sync_ran() {
    let world = World::new();
    common::git(
        &world.repo,
        &world.git_config,
        &["remote", "add", "origin", "https://example.com/r.git"],
    );
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    // A remote but no sync yet: local-only behavior, no stale warnings on claims.
    let d = agent.pre_edit("a.txt");
    assert_eq!(d.outcome, Outcome::Allow);
    assert!(d.warnings.is_empty(), "{d:?}");
    let ctx = open(&world);
    let now = Timestamp::now();
    let view = status(&ctx, None, now);
    assert!(view.remote.configured && !view.remote.sync_ran && !view.remote.stale);

    // Once a sync was attempted and never succeeded, the view is stale and claims warn.
    world.db().record_sync_failure(now, "network down").unwrap();
    let view = status(&ctx, None, now);
    assert!(view.remote.stale);
    assert_eq!(view.remote.last_error.as_deref(), Some("network down"));
    let d = agent.pre_edit("src/lib.rs");
    assert_eq!(d.outcome, Outcome::Allow, "stale remote fails open");
    assert!(
        d.warnings
            .iter()
            .any(|w| w.contains("remote") || w.contains("other machines")),
        "{d:?}"
    );
}
