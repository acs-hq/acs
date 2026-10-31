//! The state database: opening, migrations, failure typing, seats, claims, events, notices.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod store_common;

use std::fs;
use std::time::Duration;

use acs_core::gate::ClaimSource;
use acs_core::{
    Claim, ClaimOrigin, NoticeKind, Outcome, Phase, ReasonCode, ReleaseReason, SeatId, ToolKind,
};
use acs_store::{
    AcquireOutcome, Acquired, Conflict, DbConfig, NewEvent, NewSeat, SCHEMA_VERSION, StateDb,
    StoreError,
};
use rusqlite::Connection;
use store_common::{STALE_AFTER, TestRepo, at, file, path, request, seat, symbol, want};

fn granted(outcome: AcquireOutcome) -> Acquired {
    match outcome {
        AcquireOutcome::Granted(acquired) => acquired,
        AcquireOutcome::Conflict(conflict) => panic!("expected a grant, got {conflict:?}"),
    }
}

fn conflict(outcome: AcquireOutcome) -> Conflict {
    match outcome {
        AcquireOutcome::Conflict(conflict) => conflict,
        AcquireOutcome::Granted(acquired) => panic!("expected a conflict, got {acquired:?}"),
    }
}

fn acquire(
    db: &StateDb,
    seat: &SeatId,
    requests: &[acs_core::gate::ClaimRequest],
    secs: i64,
) -> AcquireOutcome {
    db.try_acquire(seat, requests, at(secs), STALE_AFTER)
        .unwrap()
}

// ---------------------------------------------------------------------------------------------
// Opening, permissions, failure typing
// ---------------------------------------------------------------------------------------------

#[test]
fn open_creates_a_wal_database_at_the_current_schema_version() {
    let repo = TestRepo::new();
    assert!(!repo.home.state_db().exists());
    let db = repo.open();
    assert_eq!(db.path(), repo.home.state_db());
    let raw = Connection::open(repo.home.state_db()).unwrap();
    let mode: String = raw
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    let version: i64 = raw
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    assert_eq!(version, SCHEMA_VERSION);
    // Reopening is a no-op.
    drop(db);
    repo.open();
}

#[cfg(unix)]
#[test]
fn database_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let repo = TestRepo::new();
    let db = repo.open();
    seat(&db, "sara@example.com", "s1"); // makes -wal and -shm exist
    let dir = repo.home.dir();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("state.db") {
            let mode = entry.metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{name}");
        }
    }
    assert!(repo.home.state_db().exists());
}

#[test]
fn a_foreign_key_violation_is_an_error() {
    let repo = TestRepo::new();
    let db = repo.open();
    let ghost: SeatId = "seat_ghost".parse().unwrap();
    let result = db.insert_notice(NoticeKind::LoreStale, &ghost, "x", at(0));
    assert!(result.is_err(), "foreign_keys must be ON");
}

#[test]
fn a_corrupt_file_is_a_typed_error() {
    let repo = TestRepo::new();
    repo.home.ensure().unwrap();
    fs::write(
        repo.home.state_db(),
        b"this is definitely not a sqlite database, sorry\n".repeat(50),
    )
    .unwrap();
    let err = StateDb::open(&repo.home).unwrap_err();
    assert!(matches!(err, StoreError::Corrupt { .. }), "{err:?}");
    assert!(err.is_state_unavailable());
    assert!(err.to_string().contains("acs doctor"));
}

#[test]
fn a_damaged_database_fails_the_integrity_check() {
    let repo = TestRepo::new();
    let db = repo.open();
    for i in 0..200 {
        seat(&db, "sara@example.com", &format!("s{i}"));
    }
    db.integrity_check().unwrap();
    drop(db);
    // Checkpoint everything into the main file, then trash a page in the middle of it.
    let raw = Connection::open(repo.home.state_db()).unwrap();
    raw.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    drop(raw);
    let mut bytes = fs::read(repo.home.state_db()).unwrap();
    assert!(bytes.len() > 8192);
    for byte in &mut bytes[4096..8192] {
        *byte = 0xA5;
    }
    fs::write(repo.home.state_db(), bytes).unwrap();
    // Opening may succeed or fail depending on which page was hit; using it must not pretend all
    // is well.
    match StateDb::open(&repo.home) {
        Err(err) => assert!(err.is_state_unavailable(), "{err:?}"),
        Ok(db) => {
            let checked = db.integrity_check();
            let read = db.get_or_create_seat(
                &NewSeat {
                    harness: "claude-code",
                    harness_session_id: "s100",
                    pilot: "sara@example.com",
                    worktree: "/w",
                    branch: None,
                    target_branch: "main",
                },
                at(0),
            );
            assert!(checked.is_err() || read.is_err());
        }
    }
}

#[test]
fn a_missing_database_is_a_typed_error_when_creation_is_off() {
    let repo = TestRepo::new();
    repo.home.ensure().unwrap();
    let config = DbConfig {
        create: false,
        ..DbConfig::default()
    };
    let err = StateDb::open_with(&repo.home, &config).unwrap_err();
    assert!(matches!(err, StoreError::Missing { .. }), "{err:?}");
    assert!(err.is_state_unavailable());
    assert!(!repo.home.state_db().exists(), "must not create it");
    repo.open();
    StateDb::open_with(&repo.home, &config).unwrap();
}

#[test]
fn a_locked_database_times_out_with_a_typed_error() {
    let repo = TestRepo::new();
    let db = repo.open();
    let s = seat(&db, "sara@example.com", "s1");
    // Another process holds the write lock.
    let blocker = Connection::open(repo.home.state_db()).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let config = DbConfig {
        busy_timeout: Duration::from_millis(50),
        ..DbConfig::default()
    };
    let quick = StateDb::open_with(&repo.home, &config).unwrap();
    let started = std::time::Instant::now();
    let err = quick
        .try_acquire(&s.seat_id, &[want("a.rs")], at(1), STALE_AFTER)
        .unwrap_err();
    assert!(matches!(err, StoreError::Locked), "{err:?}");
    assert!(err.is_state_unavailable());
    assert!(started.elapsed() < Duration::from_secs(2));
    // Reads are not blocked by a writer in WAL mode.
    quick
        .claims_snapshot(&[path("a.rs")], "main", at(1), STALE_AFTER)
        .unwrap();
    blocker.execute_batch("ROLLBACK").unwrap();
    granted(
        quick
            .try_acquire(&s.seat_id, &[want("a.rs")], at(2), STALE_AFTER)
            .unwrap(),
    );
}

// ---------------------------------------------------------------------------------------------
// Migrations
// ---------------------------------------------------------------------------------------------

const MIGRATIONS: [&str; 4] = [
    include_str!("../src/migrations/0001_local_state.sql"),
    include_str!("../src/migrations/0002_remote_claims.sql"),
    include_str!("../src/migrations/0003_hook_pipeline.sql"),
    include_str!("../src/migrations/0004_collisions.sql"),
];

fn schema_of(path: &std::path::Path) -> Vec<String> {
    let raw = Connection::open(path).unwrap();
    let mut stmt = raw
        .prepare("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn migrations_run_from_empty_and_from_every_previous_version() {
    assert_eq!(
        MIGRATIONS.len() as i64,
        SCHEMA_VERSION,
        "list every migration here"
    );
    let fresh = TestRepo::new();
    drop(fresh.open());
    let expected = schema_of(&fresh.home.state_db());

    for from in 0..MIGRATIONS.len() {
        let repo = TestRepo::new();
        repo.home.ensure().unwrap();
        let raw = Connection::open(repo.home.state_db()).unwrap();
        for sql in &MIGRATIONS[..from] {
            raw.execute_batch(sql).unwrap();
        }
        raw.pragma_update(None, "user_version", from as i64)
            .unwrap();
        if from >= 1 {
            raw.execute(
                "INSERT INTO seats (seat_id, pilot, harness, harness_session_id, worktree, \
                 target_branch, started_at, last_seen_at) \
                 VALUES ('seat_old', 'sara@example.com', 'claude-code', 'old', '/w', 'main', 1, 1)",
                [],
            )
            .unwrap();
        }
        drop(raw);

        let db = repo.open();
        assert_eq!(
            schema_of(&repo.home.state_db()),
            expected,
            "upgrade from v{from}"
        );
        if from >= 1 {
            let old: SeatId = "seat_old".parse().unwrap();
            assert!(
                db.get_seat(&old).unwrap().is_some(),
                "data survives v{from} -> latest"
            );
        }
        // The upgraded database works.
        let s = seat(&db, "omar@example.com", "new");
        granted(acquire(&db, &s.seat_id, &[want("a.rs")], 1));
    }
}

#[test]
fn a_database_from_a_newer_acs_is_refused() {
    let repo = TestRepo::new();
    drop(repo.open());
    let raw = Connection::open(repo.home.state_db()).unwrap();
    raw.pragma_update(None, "user_version", SCHEMA_VERSION + 5)
        .unwrap();
    drop(raw);
    let err = StateDb::open(&repo.home).unwrap_err();
    assert!(
        matches!(err, StoreError::SchemaTooNew { found, supported } if found == SCHEMA_VERSION + 5 && supported == SCHEMA_VERSION),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Seats
// ---------------------------------------------------------------------------------------------

#[test]
fn one_seat_per_harness_session() {
    let repo = TestRepo::new();
    let db = repo.open();
    let a = seat(&db, "sara@example.com", "s1");
    let again = seat(&db, "someone-else@example.com", "s1");
    assert_eq!(a, again, "same session, same seat, as first stored");
    assert_eq!(a.pilot, "sara@example.com");
    assert!(a.seat_id.as_str().starts_with("seat_"));
    let b = seat(&db, "sara@example.com", "s2");
    assert_ne!(a.seat_id, b.seat_id);
    // Another harness with the same session id is another seat.
    let c = db
        .get_or_create_seat(
            &NewSeat {
                harness: "antigravity",
                harness_session_id: "s1",
                pilot: "sara@example.com",
                worktree: "/w",
                branch: None,
                target_branch: "main",
            },
            at(0),
        )
        .unwrap();
    assert_ne!(a.seat_id, c.seat_id);
    assert_eq!(c.branch, None);
    assert_eq!(db.get_seat(&a.seat_id).unwrap(), Some(a));
    assert_eq!(db.get_seat(&"seat_nope".parse().unwrap()).unwrap(), None);
}

#[test]
fn touch_moves_last_seen_forward_only() {
    let repo = TestRepo::new();
    let db = repo.open();
    let s = seat(&db, "sara@example.com", "s1");
    db.touch_seat(&s.seat_id, at(50)).unwrap();
    assert_eq!(
        db.get_seat(&s.seat_id).unwrap().unwrap().last_seen_at,
        at(50)
    );
    db.touch_seat(&s.seat_id, at(10)).unwrap();
    assert_eq!(
        db.get_seat(&s.seat_id).unwrap().unwrap().last_seen_at,
        at(50)
    );
    let err = db
        .touch_seat(&"seat_nope".parse().unwrap(), at(1))
        .unwrap_err();
    assert!(matches!(err, StoreError::UnknownSeat(_)));
    assert!(!err.is_state_unavailable());
}

#[test]
fn ending_a_seat_releases_its_claims_and_a_late_event_revives_it() {
    let repo = TestRepo::new();
    let db = repo.open();
    let s = seat(&db, "sara@example.com", "s1");
    let ids = granted(acquire(&db, &s.seat_id, &[want("a.rs"), want("b.rs")], 10)).claim_ids;
    assert_eq!(db.seat_ended_at(&s.seat_id).unwrap(), None);

    let mut released = db.end_seat(&s.seat_id, at(100)).unwrap();
    released.sort();
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(released, expected);
    assert_eq!(db.seat_ended_at(&s.seat_id).unwrap(), Some(at(100)));
    assert!(db.live_claims(at(101)).unwrap().is_empty());
    assert!(
        db.end_seat(&s.seat_id, at(101)).unwrap().is_empty(),
        "idempotent"
    );

    let history = db.seat_claim_history(&s.seat_id).unwrap();
    assert_eq!(history.len(), 2);
    assert!(
        history
            .iter()
            .all(|h| h.release_reason == Some(ReleaseReason::SessionEnd))
    );
    assert!(
        history
            .iter()
            .all(|h| h.released_at == Some(at(100)) || h.released_at == Some(at(101)))
    );

    // The harness resumes the session.
    let again = seat(&db, "sara@example.com", "s1");
    assert_eq!(again.seat_id, s.seat_id);
    assert_eq!(db.seat_ended_at(&s.seat_id).unwrap(), None);
    assert!(db.end_seat(&"seat_nope".parse().unwrap(), at(1)).is_err());
}

// ---------------------------------------------------------------------------------------------
// Claims
// ---------------------------------------------------------------------------------------------

#[test]
fn a_claim_blocks_until_exactly_its_ttl_runs_out() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let omar = seat(&db, "omar@example.com", "s2");
    let ids = granted(acquire(
        &db,
        &sara.seat_id,
        &[request(file("a.rs"), "main", 900)],
        100,
    ))
    .claim_ids;
    assert_eq!(ids.len(), 1);

    let held = conflict(acquire(&db, &omar.seat_id, &[want("a.rs")], 100 + 900 - 1));
    assert_eq!(held.conflicts.len(), 1);
    assert_eq!(held.conflicts[0].claim.claim_id, ids[0]);
    assert_eq!(held.conflicts[0].source, ClaimSource::Local);
    assert_eq!(
        held.conflicts[0].holder_harness.as_deref(),
        Some("claude-code")
    );
    assert!(held.conflicts[0].holder_last_seen_at.is_some());

    let new = granted(acquire(&db, &omar.seat_id, &[want("a.rs")], 100 + 900));
    assert_ne!(new.claim_ids[0], ids[0]);

    // The expired claim moved to the history with reason `expired`, released when it ran out.
    let history = db.seat_claim_history(&sara.seat_id).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].release_reason, Some(ReleaseReason::Expired));
    assert_eq!(history[0].released_at, Some(at(1000)));
    assert_eq!(history[0].acquired_at, at(100));
    let live = db.live_claims(at(1000)).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].claim.seat_id, omar.seat_id);
}

#[test]
fn queries_ignore_expired_claims_before_anything_sweeps_them() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    granted(acquire(
        &db,
        &sara.seat_id,
        &[request(file("a.rs"), "main", 60)],
        0,
    ));
    let snap = |secs| {
        db.claims_snapshot(&[path("a.rs")], "main", at(secs), STALE_AFTER)
            .unwrap()
            .claims
            .len()
    };
    assert_eq!(snap(59), 1);
    assert_eq!(snap(60), 0);
    assert_eq!(db.live_claims(at(60)).unwrap().len(), 0);
    assert!(
        db.seat_claim_history(&sara.seat_id).unwrap()[0]
            .released_at
            .is_none(),
        "not swept yet"
    );
    assert_eq!(db.sweep_expired(at(59)).unwrap(), 0);
    assert_eq!(db.sweep_expired(at(61)).unwrap(), 1);
    let history = db.seat_claim_history(&sara.seat_id).unwrap();
    assert_eq!(history[0].release_reason, Some(ReleaseReason::Expired));
}

#[test]
fn a_repeated_request_is_a_heartbeat() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let omar = seat(&db, "omar@example.com", "s2");
    let first = granted(acquire(
        &db,
        &sara.seat_id,
        &[request(file("a.rs"), "main", 100)],
        0,
    ))
    .claim_ids;
    // 90 s later the same seat asks again with a longer TTL: same claim, refreshed.
    let second = granted(acquire(
        &db,
        &sara.seat_id,
        &[request(file("a.rs"), "main", 200)],
        90,
    ))
    .claim_ids;
    assert_eq!(first, second);
    let live = db.live_claims(at(91)).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].claim.acquired_at, at(0));
    assert_eq!(live[0].claim.heartbeat_at, at(90));
    assert_eq!(live[0].claim.ttl_secs, 200);
    // Still blocking at the old expiry, free at the new one.
    conflict(acquire(&db, &omar.seat_id, &[want("a.rs")], 150));
    conflict(acquire(&db, &omar.seat_id, &[want("a.rs")], 289));
    granted(acquire(&db, &omar.seat_id, &[want("a.rs")], 290));
}

#[test]
fn heartbeat_extends_live_claims_but_never_revives_expired_ones() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let omar = seat(&db, "omar@example.com", "s2");
    granted(acquire(
        &db,
        &sara.seat_id,
        &[
            request(file("a.rs"), "main", 100),
            request(file("b.rs"), "main", 30),
        ],
        0,
    ));

    assert_eq!(db.heartbeat_seat_claims(&sara.seat_id, at(20)).unwrap(), 2);
    // b.rs would have expired at 30, but the heartbeat at 20 carried it to 50; a.rs to 120.
    conflict(acquire(&db, &omar.seat_id, &[want("b.rs")], 49));
    // At 60 b.rs (heartbeat 20, ttl 30) has expired; the heartbeat must not bring it back.
    assert_eq!(db.heartbeat_seat_claims(&sara.seat_id, at(60)).unwrap(), 1);
    granted(acquire(&db, &omar.seat_id, &[want("b.rs")], 61));
    conflict(acquire(&db, &omar.seat_id, &[want("a.rs")], 159));
    granted(acquire(&db, &omar.seat_id, &[want("a.rs")], 160));
    assert_eq!(
        db.heartbeat_seat_claims(&"seat_nope".parse().unwrap(), at(1))
            .unwrap(),
        0
    );
}

#[test]
fn nothing_is_granted_when_one_request_conflicts() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let omar = seat(&db, "omar@example.com", "s2");
    let held = granted(acquire(&db, &sara.seat_id, &[want("b.rs")], 0)).claim_ids;

    let result = conflict(acquire(
        &db,
        &omar.seat_id,
        &[want("a.rs"), want("b.rs"), want("c.rs")],
        1,
    ));
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].claim.claim_id, held[0]);
    assert_eq!(
        db.live_claims(at(2)).unwrap().len(),
        1,
        "a.rs and c.rs were not taken"
    );
    granted(acquire(
        &db,
        &omar.seat_id,
        &[want("a.rs"), want("c.rs")],
        2,
    ));

    // Several blockers are all reported, each once.
    let sara_more = granted(acquire(&db, &sara.seat_id, &[want("d.rs")], 3)).claim_ids;
    let both = conflict(acquire(
        &db,
        &omar.seat_id,
        &[want("b.rs"), want("d.rs"), want("b.rs")],
        4,
    ));
    let mut found: Vec<_> = both
        .conflicts
        .iter()
        .map(|c| c.claim.claim_id.clone())
        .collect();
    found.sort();
    let mut want_ids = vec![held[0].clone(), sara_more[0].clone()];
    want_ids.sort();
    assert_eq!(found, want_ids);
}

#[test]
fn scopes_follow_the_overlap_rules() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let omar = seat(&db, "omar@example.com", "s2");
    granted(acquire(
        &db,
        &sara.seat_id,
        &[request(symbol("a.rs", "Svc.apply"), "main", 900)],
        0,
    ));
    // A sibling symbol and another file are free; the containing class and the whole file are not.
    granted(acquire(
        &db,
        &omar.seat_id,
        &[
            request(symbol("a.rs", "Svc.refund"), "main", 900),
            request(file("b.rs"), "main", 900),
        ],
        1,
    ));
    conflict(acquire(
        &db,
        &omar.seat_id,
        &[request(symbol("a.rs", "Svc"), "main", 900)],
        2,
    ));
    conflict(acquire(
        &db,
        &omar.seat_id,
        &[request(file("a.rs"), "main", 900)],
        3,
    ));
    // The same seat may hold a file claim next to its own symbol claim.
    granted(acquire(
        &db,
        &sara.seat_id,
        &[request(symbol("c.rs", "f"), "main", 900)],
        4,
    ));
    granted(acquire(
        &db,
        &sara.seat_id,
        &[request(file("c.rs"), "main", 900)],
        5,
    ));
    conflict(acquire(
        &db,
        &omar.seat_id,
        &[request(symbol("c.rs", "g"), "main", 900)],
        6,
    ));
}

#[test]
fn claims_on_different_target_branches_do_not_conflict() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let omar = seat(&db, "omar@example.com", "s2");
    granted(acquire(
        &db,
        &sara.seat_id,
        &[request(file("a.rs"), "main", 900)],
        0,
    ));
    granted(acquire(
        &db,
        &omar.seat_id,
        &[request(file("a.rs"), "experiment", 900)],
        1,
    ));
    conflict(acquire(
        &db,
        &omar.seat_id,
        &[request(file("a.rs"), "main", 900)],
        2,
    ));
    // A seat can hold the same file on two branches; each is its own claim.
    let both = db.live_claims(at(3)).unwrap();
    assert_eq!(both.len(), 2);
    let only_main = db
        .claims_snapshot(&[path("a.rs")], "main", at(3), STALE_AFTER)
        .unwrap();
    assert_eq!(only_main.claims.len(), 1);
    assert_eq!(only_main.claims[0].claim.seat_id, sara.seat_id);
}

#[test]
fn two_seats_of_one_pilot_conflict() {
    let repo = TestRepo::new();
    let db = repo.open();
    let first = seat(&db, "sara@example.com", "s1");
    let second = seat(&db, "sara@example.com", "s2");
    assert_ne!(first.seat_id, second.seat_id);
    granted(acquire(&db, &first.seat_id, &[want("a.rs")], 0));
    let blocked = conflict(acquire(&db, &second.seat_id, &[want("a.rs")], 1));
    assert_eq!(blocked.conflicts[0].claim.pilot, "sara@example.com");
}

#[test]
fn releasing_frees_the_file_and_records_the_reason() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let omar = seat(&db, "omar@example.com", "s2");
    let ids = granted(acquire(
        &db,
        &sara.seat_id,
        &[want("a.rs"), want("b.rs"), want("c.rs")],
        0,
    ))
    .claim_ids;

    let one = db
        .release_claims(&ids[..1], ReleaseReason::Released, at(10))
        .unwrap();
    assert_eq!(one, vec![ids[0].clone()]);
    granted(acquire(&db, &omar.seat_id, &[want("a.rs")], 11));
    conflict(acquire(&db, &omar.seat_id, &[want("b.rs")], 11));
    assert!(
        db.release_claims(&ids[..1], ReleaseReason::Released, at(12))
            .unwrap()
            .is_empty()
    );

    let rest = db
        .release_seat_claims(&sara.seat_id, ReleaseReason::SessionEnd, at(20))
        .unwrap();
    assert_eq!(rest.len(), 2);
    let history = db.seat_claim_history(&sara.seat_id).unwrap();
    let reasons: Vec<_> = history.iter().map(|h| h.release_reason).collect();
    assert_eq!(history.len(), 3);
    assert_eq!(
        reasons
            .iter()
            .filter(|r| **r == Some(ReleaseReason::Released))
            .count(),
        1
    );
    assert_eq!(
        reasons
            .iter()
            .filter(|r| **r == Some(ReleaseReason::SessionEnd))
            .count(),
        2
    );
    granted(acquire(
        &db,
        &omar.seat_id,
        &[want("b.rs"), want("c.rs")],
        21,
    ));
}

#[test]
fn releasing_an_already_expired_claim_is_recorded_as_expired() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let ids = granted(acquire(
        &db,
        &sara.seat_id,
        &[request(file("a.rs"), "main", 10)],
        0,
    ))
    .claim_ids;
    db.release_claims(&ids, ReleaseReason::Released, at(500))
        .unwrap();
    let history = db.seat_claim_history(&sara.seat_id).unwrap();
    assert_eq!(history[0].release_reason, Some(ReleaseReason::Expired));
    assert_eq!(history[0].released_at, Some(at(10)));
}

#[test]
fn acquiring_for_an_unknown_seat_is_an_error() {
    let repo = TestRepo::new();
    let db = repo.open();
    let err = db
        .try_acquire(
            &"seat_nope".parse().unwrap(),
            &[want("a.rs")],
            at(0),
            STALE_AFTER,
        )
        .unwrap_err();
    assert!(matches!(err, StoreError::UnknownSeat(_)));
}

#[test]
fn an_empty_request_grants_nothing_and_succeeds() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    assert!(
        granted(acquire(&db, &sara.seat_id, &[], 0))
            .claim_ids
            .is_empty()
    );
}

#[test]
fn duplicate_requests_in_one_call_share_a_claim() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let ids = granted(acquire(
        &db,
        &sara.seat_id,
        &[want("a.rs"), want("a.rs")],
        0,
    ))
    .claim_ids;
    assert_eq!(ids[0], ids[1]);
    assert_eq!(db.live_claims(at(1)).unwrap().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// Snapshot and the remote view
// ---------------------------------------------------------------------------------------------

fn remote_claim(id: &str, seat: &str, pilot: &str, p: &str, heartbeat: i64, ttl: u64) -> Claim {
    Claim {
        claim_id: id.parse().unwrap(),
        seat_id: seat.parse().unwrap(),
        pilot: pilot.to_owned(),
        scope: file(p),
        target_branch: "main".into(),
        acquired_at: at(heartbeat),
        heartbeat_at: at(heartbeat),
        ttl_secs: ttl,
        origin: ClaimOrigin::Agent,
    }
}

#[test]
fn snapshot_holds_local_claims_on_the_requested_files_including_the_seats_own() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let omar = seat(&db, "omar@example.com", "s2");
    granted(acquire(
        &db,
        &sara.seat_id,
        &[want("a.rs"), want("z.rs")],
        0,
    ));
    granted(acquire(
        &db,
        &omar.seat_id,
        &[request(symbol("b.rs", "f"), "main", 900)],
        1,
    ));

    let snap = db
        .claims_snapshot(
            &[path("a.rs"), path("b.rs"), path("a.rs"), path("none.rs")],
            "main",
            at(2),
            STALE_AFTER,
        )
        .unwrap();
    let files: Vec<_> = snap
        .claims
        .iter()
        .map(|c| c.claim.scope.path().as_str().to_owned())
        .collect();
    assert_eq!(
        files,
        ["a.rs", "b.rs"],
        "only the files asked about, each claim once"
    );
    assert!(snap.claims.iter().all(|c| c.source == ClaimSource::Local));
    assert_eq!(snap.claims[0].claim.seat_id, sara.seat_id);
    assert!(snap.remote_stale, "never fetched");
    assert!(
        db.claims_snapshot(&[], "main", at(2), STALE_AFTER)
            .unwrap()
            .claims
            .is_empty()
    );
}

#[test]
fn snapshot_scales_past_the_parameter_chunk() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let paths: Vec<_> = (0..1300).map(|i| path(&format!("src/f{i}.rs"))).collect();
    let requests: Vec<_> = paths.iter().map(|p| want(p.as_str())).collect();
    granted(acquire(&db, &sara.seat_id, &requests, 0));
    let snap = db
        .claims_snapshot(&paths, "main", at(1), STALE_AFTER)
        .unwrap();
    assert_eq!(snap.claims.len(), 1300);
}

#[test]
fn fresh_remote_claims_block_and_stale_ones_do_not() {
    let repo = TestRepo::new();
    let db = repo.open();
    let omar = seat(&db, "omar@example.com", "s2");
    let theirs = remote_claim(
        "clm_remote1",
        "seat_far",
        "lina@example.com",
        "a.rs",
        0,
        900,
    );
    db.replace_remote_claims("pilot-lina", &[theirs], at(5))
        .unwrap();

    // No successful fetch recorded yet: the view is stale, so the remote claim is ignored.
    let snap = db
        .claims_snapshot(&[path("a.rs")], "main", at(10), STALE_AFTER)
        .unwrap();
    assert!(snap.remote_stale && snap.claims.is_empty());

    db.record_sync_success(at(10)).unwrap();
    let snap = db
        .claims_snapshot(&[path("a.rs")], "main", at(20), STALE_AFTER)
        .unwrap();
    assert!(!snap.remote_stale);
    assert_eq!(snap.claims.len(), 1);
    assert_eq!(snap.claims[0].source, ClaimSource::Remote);
    assert_eq!(snap.claims[0].holder_harness, None);

    let blocked = conflict(acquire(&db, &omar.seat_id, &[want("a.rs")], 20));
    assert_eq!(blocked.conflicts[0].source, ClaimSource::Remote);

    // Exactly stale_after later the view is still fresh; one second more and it is stale.
    let edge = db
        .claims_snapshot(&[path("a.rs")], "main", at(10 + 90), STALE_AFTER)
        .unwrap();
    assert!(!edge.remote_stale);
    granted(acquire(&db, &omar.seat_id, &[want("a.rs")], 10 + 91));

    let status = db.remote_sync_status().unwrap();
    assert_eq!(status.last_success_at, Some(at(10)));
}

#[test]
fn a_seat_keeps_its_claim_against_a_later_remote_claim_and_loses_to_an_earlier_one() {
    let repo = TestRepo::new();
    let db = repo.open();
    let omar = seat(&db, "omar@example.com", "s2");
    granted(acquire(&db, &omar.seat_id, &[want("a.rs")], 10));

    let remote = |acquired| {
        remote_claim(
            "clm_remote1",
            "seat_far",
            "lina@example.com",
            "a.rs",
            acquired,
            900,
        )
    };
    db.replace_remote_claims("pilot-lina", &[remote(20)], at(30))
        .unwrap();
    db.record_sync_success(at(30)).unwrap();
    granted(acquire(&db, &omar.seat_id, &[want("a.rs")], 31));

    db.replace_remote_claims("pilot-lina", &[remote(5)], at(32))
        .unwrap();
    db.record_sync_success(at(32)).unwrap();
    let blocked = conflict(acquire(&db, &omar.seat_id, &[want("a.rs")], 33));
    assert_eq!(blocked.conflicts[0].source, ClaimSource::Remote);
}

#[test]
fn remote_claims_expire_by_their_own_ttl_and_respect_the_branch() {
    let repo = TestRepo::new();
    let db = repo.open();
    let omar = seat(&db, "omar@example.com", "s2");
    let mut other_branch = remote_claim(
        "clm_remote2",
        "seat_far",
        "lina@example.com",
        "b.rs",
        0,
        900,
    );
    other_branch.target_branch = "experiment".into();
    db.replace_remote_claims(
        "pilot-lina",
        &[
            remote_claim(
                "clm_remote1",
                "seat_far",
                "lina@example.com",
                "a.rs",
                0,
                100,
            ),
            other_branch,
        ],
        at(0),
    )
    .unwrap();
    db.record_sync_success(at(90)).unwrap();
    conflict(acquire(&db, &omar.seat_id, &[want("a.rs")], 99));
    granted(acquire(
        &db,
        &omar.seat_id,
        &[want("a.rs"), want("b.rs")],
        100,
    ));
}

#[test]
fn replacing_remote_claims_replaces_one_pilot_only() {
    let repo = TestRepo::new();
    let db = repo.open();
    db.record_sync_success(at(0)).unwrap();
    db.replace_remote_claims(
        "p1",
        &[
            remote_claim("clm_1", "seat_x", "a@e.com", "a.rs", 0, 900),
            remote_claim("clm_2", "seat_x", "a@e.com", "b.rs", 0, 900),
        ],
        at(0),
    )
    .unwrap();
    db.replace_remote_claims(
        "p2",
        &[remote_claim("clm_3", "seat_y", "b@e.com", "c.rs", 0, 900)],
        at(0),
    )
    .unwrap();
    let all = [path("a.rs"), path("b.rs"), path("c.rs")];
    let count = |secs| {
        db.claims_snapshot(&all, "main", at(secs), STALE_AFTER)
            .unwrap()
            .claims
            .len()
    };
    assert_eq!(count(1), 3);
    db.replace_remote_claims(
        "p1",
        &[remote_claim("clm_2", "seat_x", "a@e.com", "b.rs", 0, 900)],
        at(2),
    )
    .unwrap();
    assert_eq!(count(3), 2);
    db.replace_remote_claims("p1", &[], at(3)).unwrap();
    assert_eq!(count(4), 1);
    assert_eq!(db.retain_remote_pilots(&["p1"]).unwrap(), 1);
    assert_eq!(count(5), 0);
}

#[test]
fn a_remote_copy_of_a_local_claim_is_not_listed_twice() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let ids = granted(acquire(&db, &sara.seat_id, &[want("a.rs")], 0)).claim_ids;
    db.record_sync_success(at(1)).unwrap();
    // Our own claims come back through our own ref.
    let mut copy = remote_claim(
        ids[0].as_str(),
        sara.seat_id.as_str(),
        "sara@example.com",
        "a.rs",
        0,
        900,
    );
    copy.claim_id = ids[0].clone();
    db.replace_remote_claims("pilot-sara", &[copy], at(1))
        .unwrap();
    let snap = db
        .claims_snapshot(&[path("a.rs")], "main", at(2), STALE_AFTER)
        .unwrap();
    assert_eq!(snap.claims.len(), 1);
    assert_eq!(snap.claims[0].source, ClaimSource::Local);
    // And it does not block the seat that owns it.
    granted(acquire(&db, &sara.seat_id, &[want("a.rs")], 3));
}

#[test]
fn sync_status_keeps_the_last_success_across_failures() {
    let repo = TestRepo::new();
    let db = repo.open();
    assert_eq!(
        db.remote_sync_status().unwrap(),
        acs_store::SyncStatus::default()
    );
    db.record_sync_failure(at(1), "no route to host").unwrap();
    let status = db.remote_sync_status().unwrap();
    assert_eq!(
        (status.last_success_at, status.last_attempt_at),
        (None, Some(at(1)))
    );
    assert_eq!(status.last_error.as_deref(), Some("no route to host"));
    db.record_sync_success(at(5)).unwrap();
    db.record_sync_failure(at(9), "timeout").unwrap();
    let status = db.remote_sync_status().unwrap();
    assert_eq!(status.last_success_at, Some(at(5)));
    assert_eq!(status.last_attempt_at, Some(at(9)));
    db.record_sync_success(at(12)).unwrap();
    assert_eq!(db.remote_sync_status().unwrap().last_error, None);
}

// ---------------------------------------------------------------------------------------------
// Events, notices, stats
// ---------------------------------------------------------------------------------------------

#[test]
fn events_are_recorded_structurally_and_summarized() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let paths = vec!["src/a.rs".to_owned()];
    let long = "x".repeat(2000);
    let base = |at_secs: i64| NewEvent {
        seat_id: &sara.seat_id,
        at: at(at_secs),
        phase: Phase::PreTool,
        tool_name: Some("Edit"),
        tool_kind: Some(ToolKind::Edit),
        paths: &paths,
        reason_code: Some(ReasonCode::Ok),
        outcome: Some(Outcome::Allow),
        detail: None,
    };
    let first = db.record_event(&base(1)).unwrap();
    let second = db
        .record_event(&NewEvent {
            reason_code: Some(ReasonCode::ClaimConflict),
            outcome: Some(Outcome::Deny),
            detail: Some(&long),
            ..base(2)
        })
        .unwrap();
    db.record_event(&NewEvent {
        phase: Phase::PostTool,
        reason_code: None,
        outcome: None,
        paths: &[],
        tool_name: None,
        tool_kind: None,
        ..base(3)
    })
    .unwrap();
    assert!(second > first);

    let events = db.seat_events(&sara.seat_id).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].paths, paths);
    assert_eq!(events[0].phase, Phase::PreTool);
    assert_eq!(events[0].tool_kind, Some(ToolKind::Edit));
    assert_eq!(events[1].outcome, Some(Outcome::Deny));
    assert_eq!(
        events[1].detail.as_ref().unwrap().chars().count(),
        acs_store::events::MAX_DETAIL_CHARS
    );
    assert_eq!(events[2].reason_code, None);
    assert!(events[2].paths.is_empty());

    let counts = db.reason_counts(&sara.seat_id).unwrap();
    assert_eq!(counts.len(), 2);
    assert_eq!(counts[&ReasonCode::Ok], 1);
    assert_eq!(counts[&ReasonCode::ClaimConflict], 1);

    assert_eq!(db.prune_events(at(3)).unwrap(), 2);
    assert_eq!(db.seat_events(&sara.seat_id).unwrap().len(), 1);
}

#[test]
fn notices_are_delivered_once_in_order() {
    let repo = TestRepo::new();
    let db = repo.open();
    let sara = seat(&db, "sara@example.com", "s1");
    let omar = seat(&db, "omar@example.com", "s2");
    let first = db
        .insert_notice(
            NoticeKind::ClaimReleased,
            &sara.seat_id,
            "a.rs is free",
            at(1),
        )
        .unwrap();
    let second = db
        .insert_notice(
            NoticeKind::InterfaceChanged,
            &sara.seat_id,
            "f() changed",
            at(2),
        )
        .unwrap();
    db.insert_notice(NoticeKind::LoreStale, &omar.seat_id, "other seat", at(3))
        .unwrap();
    assert!(first.as_str().starts_with("ntc_"));

    let pending = db.pending_notices(&sara.seat_id).unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].notice_id, first);
    assert_eq!(pending[0].kind, NoticeKind::ClaimReleased);
    assert_eq!(pending[0].body, "a.rs is free");
    assert_eq!(pending[0].created_at, at(1));
    assert_eq!(pending[0].target_seat, sara.seat_id);
    assert_eq!(pending[0].delivered_at, None);
    assert_eq!(pending[1].notice_id, second);

    assert_eq!(
        db.mark_delivered(std::slice::from_ref(&first), at(10))
            .unwrap(),
        1
    );
    assert_eq!(
        db.mark_delivered(std::slice::from_ref(&first), at(11))
            .unwrap(),
        0,
        "first delivery time stays"
    );
    let rest = db.pending_notices(&sara.seat_id).unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].notice_id, second);
    assert_eq!(db.pending_notices(&omar.seat_id).unwrap().len(), 1);
    assert_eq!(db.mark_delivered(&[], at(12)).unwrap(), 0);
}

#[test]
fn counters_only_go_up() {
    let repo = TestRepo::new();
    let db = repo.open();
    assert_eq!(db.counter("hooks").unwrap(), 0);
    db.increment("hooks", 1).unwrap();
    db.increment("hooks", 4).unwrap();
    db.increment("denied", 0).unwrap();
    assert_eq!(db.counter("hooks").unwrap(), 5);
    assert_eq!(db.counters().unwrap().len(), 2);
    db.increment("hooks", u64::MAX).unwrap();
    assert_eq!(
        db.counter("hooks").unwrap(),
        i64::MAX as u64,
        "saturates instead of wrapping"
    );
}
