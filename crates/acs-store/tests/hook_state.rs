//! The store additions of the hook pipeline (migration 0003): session ids and intent, live and
//! stale seats, last decisions, repository flags and attributions.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod store_common;

use acs_core::{ContentHash, Outcome, ReasonCode, SeatId};
use acs_store::{Attribution, AttributionSource, LastDecision, NewSeat, StateDb};
use rusqlite::Connection;
use store_common::{TestRepo, at, path};

fn new_seat<'a>(harness: &'a str, session: &'a str) -> NewSeat<'a> {
    NewSeat {
        harness,
        harness_session_id: session,
        pilot: "sara@example.com",
        worktree: "/work",
        branch: Some("main"),
        target_branch: "main",
    }
}

#[test]
fn a_seat_gets_one_session_id_for_life() {
    let repo = TestRepo::new();
    let db = repo.open();
    let (first, created) = db
        .get_or_create_seat_info(&new_seat("claude-code", "s1"), at(0))
        .unwrap();
    assert!(created);
    assert!(first.session_id.as_str().starts_with("ses_"));
    assert_eq!(first.intent, None);

    let (again, created) = db
        .get_or_create_seat_info(&new_seat("claude-code", "s1"), at(5))
        .unwrap();
    assert!(!created);
    assert_eq!(again.session_id, first.session_id);
    assert_eq!(again.seat.seat_id, first.seat.seat_id);

    let (other, _) = db
        .get_or_create_seat_info(&new_seat("claude-code", "s2"), at(5))
        .unwrap();
    assert_ne!(other.session_id, first.session_id);

    // A seat that ended and comes back (a resumed session) keeps its session id.
    db.end_seat(&first.seat.seat_id, at(10)).unwrap();
    let info = db.seat_info(&first.seat.seat_id).unwrap().unwrap();
    assert_eq!(info.ended_at, Some(at(10)));
    let (revived, created) = db
        .get_or_create_seat_info(&new_seat("claude-code", "s1"), at(20))
        .unwrap();
    assert!(!created);
    assert_eq!(revived.ended_at, None);
    assert_eq!(revived.session_id, first.session_id);
}

#[test]
fn the_plain_seat_api_still_works_and_agrees() {
    let repo = TestRepo::new();
    let db = repo.open();
    let seat = db
        .get_or_create_seat(&new_seat("claude-code", "s1"), at(0))
        .unwrap();
    let info = db
        .seat_by_session("claude-code", "s1")
        .unwrap()
        .expect("seat exists");
    assert_eq!(info.seat, seat);
    assert!(db.seat_by_session("claude-code", "nope").unwrap().is_none());
    assert!(
        db.seat_info(&"seat_missing".parse().unwrap())
            .unwrap()
            .is_none()
    );
}

#[test]
fn intent_is_stored_and_replaced() {
    let repo = TestRepo::new();
    let db = repo.open();
    let seat = db
        .get_or_create_seat(&new_seat("claude-code", "s1"), at(0))
        .unwrap();
    db.set_seat_intent(&seat.seat_id, "fix refunds").unwrap();
    db.set_seat_intent(&seat.seat_id, "fix refunds, then tests")
        .unwrap();
    let info = db.seat_info(&seat.seat_id).unwrap().unwrap();
    assert_eq!(info.intent.as_deref(), Some("fix refunds, then tests"));
    let missing: SeatId = "seat_missing".parse().unwrap();
    assert!(db.set_seat_intent(&missing, "x").is_err());
}

#[test]
fn live_and_stale_seats() {
    let repo = TestRepo::new();
    let db = repo.open();
    let old = db
        .get_or_create_seat(&new_seat("claude-code", "old"), at(0))
        .unwrap();
    let fresh = db
        .get_or_create_seat(&new_seat("claude-code", "fresh"), at(0))
        .unwrap();
    let ended = db
        .get_or_create_seat(&new_seat("claude-code", "ended"), at(0))
        .unwrap();
    let human = db
        .get_or_create_seat(&new_seat("human", "cli"), at(0))
        .unwrap();
    db.touch_seat(&fresh.seat_id, at(1000)).unwrap();
    db.touch_seat(&human.seat_id, at(0)).unwrap();
    db.end_seat(&ended.seat_id, at(10)).unwrap();

    let live: Vec<_> = db
        .live_seats()
        .unwrap()
        .into_iter()
        .map(|i| i.seat.seat_id)
        .collect();
    assert_eq!(live.len(), 3);
    assert_eq!(live[0], fresh.seat_id, "most recently seen first");
    assert!(!live.contains(&ended.seat_id));

    // Only the old claude-code seat is stale: the ended seat ended cleanly and the human seat
    // never ends.
    let stale = db.stale_live_seats(at(500)).unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].seat.seat_id, old.seat_id);
    // Strictly older than the threshold.
    assert!(db.stale_live_seats(at(0)).unwrap().is_empty());
}

#[test]
fn last_decision_is_replaced_per_seat() {
    let repo = TestRepo::new();
    let db = repo.open();
    let a = db
        .get_or_create_seat(&new_seat("claude-code", "a"), at(0))
        .unwrap();
    let b = db
        .get_or_create_seat(&new_seat("claude-code", "b"), at(0))
        .unwrap();
    assert!(db.last_decision(&a.seat_id).unwrap().is_none());
    let deny = LastDecision {
        outcome: Outcome::Deny,
        reason_code: ReasonCode::ClaimConflict,
        message: "claimed by sara".into(),
        at: at(1),
    };
    db.set_last_decision(&a.seat_id, &deny).unwrap();
    assert_eq!(db.last_decision(&a.seat_id).unwrap(), Some(deny));
    let allow = LastDecision {
        outcome: Outcome::Allow,
        reason_code: ReasonCode::Ok,
        message: String::new(),
        at: at(2),
    };
    db.set_last_decision(&a.seat_id, &allow).unwrap();
    assert_eq!(db.last_decision(&a.seat_id).unwrap(), Some(allow));
    assert!(db.last_decision(&b.seat_id).unwrap().is_none());
}

#[test]
fn flags_are_set_read_and_cleared() {
    let repo = TestRepo::new();
    let db = repo.open();
    assert_eq!(db.flag("gate_off").unwrap(), None);
    assert!(!db.flag_is_set("gate_off").unwrap());
    db.set_flag("gate_off", "1", at(0)).unwrap();
    assert!(db.flag_is_set("gate_off").unwrap());
    db.set_flag("gate_off", "0", at(1)).unwrap();
    assert!(!db.flag_is_set("gate_off").unwrap());
    assert!(db.clear_flag("gate_off").unwrap());
    assert!(!db.clear_flag("gate_off").unwrap());
}

#[test]
fn attributions_are_recorded_per_seat_and_per_path() {
    let repo = TestRepo::new();
    let db = repo.open();
    let a = db
        .get_or_create_seat(&new_seat("claude-code", "a"), at(0))
        .unwrap();
    let b = db
        .get_or_create_seat(&new_seat("claude-code", "b"), at(0))
        .unwrap();
    let h1 = ContentHash::of(b"one");
    let h2 = ContentHash::of(b"two");
    db.record_attributions(
        &a.seat_id,
        &[
            Attribution {
                path: path("src/a.rs"),
                pre_hash: None,
                post_hash: Some(h1.clone()),
            },
            Attribution {
                path: path("src/b.rs"),
                pre_hash: Some(h1.clone()),
                post_hash: None,
            },
        ],
        AttributionSource::Shell,
        at(1),
    )
    .unwrap();
    db.record_attributions(
        &b.seat_id,
        &[Attribution {
            path: path("src/a.rs"),
            pre_hash: Some(h1.clone()),
            post_hash: Some(h2.clone()),
        }],
        AttributionSource::Tool,
        at(2),
    )
    .unwrap();
    db.record_attributions(&b.seat_id, &[], AttributionSource::Tool, at(3))
        .unwrap();

    let mine = db.seat_attributions(&a.seat_id).unwrap();
    assert_eq!(mine.len(), 2);
    assert_eq!(mine[0].path, path("src/a.rs"));
    assert_eq!(mine[0].pre_hash, None);
    assert_eq!(mine[0].via, AttributionSource::Shell);
    assert_eq!(mine[1].post_hash, None);

    let history = db.path_attributions(&path("src/a.rs")).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].seat_id, b.seat_id, "newest first");
    assert_eq!(history[0].post_hash, Some(h2));
    assert_eq!(history[0].via, AttributionSource::Tool);
}

#[test]
fn upgrading_a_version_two_database_backfills_session_ids() {
    let repo = TestRepo::new();
    repo.home.ensure().unwrap();
    {
        let raw = Connection::open(repo.home.state_db()).unwrap();
        raw.execute_batch(include_str!("../src/migrations/0001_local_state.sql"))
            .unwrap();
        raw.execute_batch(include_str!("../src/migrations/0002_remote_claims.sql"))
            .unwrap();
        raw.pragma_update(None, "user_version", 2).unwrap();
        for (id, session) in [("seat_one", "one"), ("seat_two", "two")] {
            raw.execute(
                "INSERT INTO seats (seat_id, pilot, harness, harness_session_id, worktree, \
                 target_branch, started_at, last_seen_at) \
                 VALUES (?1, 'sara@example.com', 'claude-code', ?2, '/w', 'main', 1, 1)",
                [id, session],
            )
            .unwrap();
        }
    }
    let db = StateDb::open(&repo.home).unwrap();
    let one = db.seat_info(&"seat_one".parse().unwrap()).unwrap().unwrap();
    let two = db.seat_info(&"seat_two".parse().unwrap()).unwrap().unwrap();
    assert!(one.session_id.as_str().starts_with("ses_"));
    assert_ne!(one.session_id, two.session_id);
}
