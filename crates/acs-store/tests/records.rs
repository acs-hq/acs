//! Session records: assembly from a seat's history, and the table that remembers what is already
//! recorded (WP4.1).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod store_common;

use acs_core::{ContentHash, EndedReason, Outcome, OverrideMode, Phase, ReasonCode, ToolKind};
use acs_store::records::{
    MAX_INTENT_CHARS, MAX_NOTABLE, MAX_NOTABLE_CHARS, RecordInputs, is_worth_recording,
    worktree_name,
};
use acs_store::{Attribution, AttributionSource, NewEvent, NewOverride, SeatInfo, StateDb};
use store_common::{TestRepo, at, path, seat, want};

fn hash(c: char) -> ContentHash {
    ContentHash::try_from(c.to_string().repeat(64)).unwrap()
}

fn info(db: &StateDb, session: &str) -> SeatInfo {
    let s = seat(db, "sara@example.com", session);
    db.seat_info(&s.seat_id).unwrap().unwrap()
}

fn redact(text: &str) -> String {
    text.replace("hunter2", "***")
}

fn inputs(reason: EndedReason, ended: i64) -> RecordInputs<'static> {
    RecordInputs {
        commits: vec!["abc".repeat(13) + "a"],
        ended_at: at(ended),
        ended_reason: reason,
        acs_version: "9.9.9",
        redact: &redact,
    }
}

fn event(
    db: &StateDb,
    info: &SeatInfo,
    secs: i64,
    code: ReasonCode,
    outcome: Outcome,
    paths: &[&str],
    detail: Option<&str>,
) {
    let paths: Vec<String> = paths.iter().map(|p| (*p).to_owned()).collect();
    db.record_event(&NewEvent {
        seat_id: &info.seat.seat_id,
        at: at(secs),
        phase: Phase::PreTool,
        tool_name: Some("Bash"),
        tool_kind: Some(ToolKind::Shell),
        paths: &paths,
        reason_code: Some(code),
        outcome: Some(outcome),
        detail,
    })
    .unwrap();
}

#[test]
fn a_record_gathers_the_seats_history() {
    let repo = TestRepo::new();
    let db = repo.open();
    let info0 = info(&db, "one");
    let id = info0.seat.seat_id.clone();
    db.set_seat_intent(&id, "fix the login, key hunter2")
        .unwrap();
    db.try_acquire(&id, &[want("src/a.rs")], at(5), store_common::STALE_AFTER)
        .unwrap();
    // a.rs changes twice, b.rs once; the record keeps first pre and last post, in first-change order.
    let change = |p: &str, pre, post| Attribution {
        path: path(p),
        pre_hash: pre,
        post_hash: post,
    };
    db.record_attributions(
        &id,
        &[change("src/a.rs", Some(hash('1')), Some(hash('2')))],
        AttributionSource::Tool,
        at(6),
    )
    .unwrap();
    db.record_attributions(
        &id,
        &[change("src/b.rs", None, Some(hash('3')))],
        AttributionSource::Shell,
        at(7),
    )
    .unwrap();
    db.record_attributions(
        &id,
        &[change("src/a.rs", Some(hash('2')), Some(hash('4')))],
        AttributionSource::Tool,
        at(8),
    )
    .unwrap();
    event(
        &db,
        &info0,
        9,
        ReasonCode::Ok,
        Outcome::Allow,
        &["src/a.rs"],
        None,
    );
    event(
        &db,
        &info0,
        10,
        ReasonCode::BannedCommand,
        Outcome::Deny,
        &[],
        Some("curl --token hunter2 x"),
    );
    event(
        &db,
        &info0,
        11,
        ReasonCode::ClaimConflict,
        Outcome::Deny,
        &["src/c.rs"],
        Some("held by omar@example.com for src/c.rs"),
    );
    event(
        &db,
        &info0,
        12,
        ReasonCode::DestructiveBudget,
        Outcome::Ask,
        &[],
        None,
    );
    db.mark_lore_loaded(&id, &[("lore_7f3a".parse().unwrap(), false)], at(4))
        .unwrap();
    let grant = db
        .grant_override(&NewOverride {
            pattern: "payments/**",
            mode: OverrideMode::Session,
            granted_by: "omar@example.com",
            now: at(1),
            expires_at: at(900),
        })
        .unwrap();
    db.apply_overrides(&id, &[(grant, path("payments/x.rs"))], at(3))
        .unwrap();

    let record = db
        .assemble_record(
            &db.seat_info(&id).unwrap().unwrap(),
            inputs(EndedReason::Clean, 20),
        )
        .unwrap();

    assert_eq!(
        record.seat.worktree, "repo",
        "only the folder name leaves the machine"
    );
    assert_eq!(record.intent.as_deref(), Some("fix the login, key ***"));
    assert_eq!(record.claims.len(), 1);
    let files: Vec<_> = record
        .files_changed
        .iter()
        .map(|f| (f.path.as_str(), f.pre_hash.clone(), f.post_hash.clone()))
        .collect();
    assert_eq!(
        files,
        vec![
            ("src/a.rs", Some(hash('1')), Some(hash('4'))),
            ("src/b.rs", None, Some(hash('3'))),
        ]
    );
    assert_eq!(record.guard_decisions.counts[&ReasonCode::BannedCommand], 1);
    assert_eq!(record.guard_decisions.counts[&ReasonCode::Ok], 1);
    let notable: Vec<_> = record
        .guard_decisions
        .notable
        .iter()
        .map(|n| (n.reason_code, n.outcome, n.path_or_command.as_str()))
        .collect();
    assert_eq!(
        notable,
        vec![
            (ReasonCode::BannedCommand, Outcome::Deny, "curl --token *** x"),
            // A conflict lists the path: its stored message names the holder.
            (ReasonCode::ClaimConflict, Outcome::Deny, "src/c.rs"),
            (ReasonCode::DestructiveBudget, Outcome::Ask, "Bash"),
        ]
    );
    assert_eq!(record.overrides.len(), 1);
    assert_eq!(record.overrides[0].paths, vec![path("payments/x.rs")]);
    assert_eq!(record.started_at, at(0));
    assert_eq!(record.ended_at, at(20));
    assert_eq!(record.acs_version, "9.9.9");
    assert!(is_worth_recording(&record));
    // And it round-trips as the ledger stores it.
    let bytes = serde_json::to_vec(&record).unwrap();
    assert_eq!(
        serde_json::from_slice::<acs_core::SessionRecord>(&bytes).unwrap(),
        record
    );
}

#[test]
fn intent_and_notable_text_are_capped() {
    let repo = TestRepo::new();
    let db = repo.open();
    let info0 = info(&db, "caps");
    let id = info0.seat.seat_id.clone();
    db.set_seat_intent(&id, &"x".repeat(2000)).unwrap();
    for i in 0..(MAX_NOTABLE + 10) {
        let command = format!("rm {}", "y".repeat(400));
        event(
            &db,
            &info0,
            i64::try_from(i).unwrap(),
            ReasonCode::BannedCommand,
            Outcome::Deny,
            &[],
            Some(&command),
        );
    }
    let record = db
        .assemble_record(
            &db.seat_info(&id).unwrap().unwrap(),
            inputs(EndedReason::Clean, 99),
        )
        .unwrap();
    assert_eq!(record.intent.unwrap().chars().count(), MAX_INTENT_CHARS);
    assert_eq!(record.guard_decisions.notable.len(), MAX_NOTABLE);
    assert!(
        record
            .guard_decisions
            .notable
            .iter()
            .all(|n| n.path_or_command.chars().count() <= MAX_NOTABLE_CHARS)
    );
    assert_eq!(
        record.guard_decisions.counts[&ReasonCode::BannedCommand],
        u64::try_from(MAX_NOTABLE + 10).unwrap(),
        "counts are not capped"
    );
}

#[test]
fn an_empty_session_is_not_worth_recording() {
    let repo = TestRepo::new();
    let db = repo.open();
    let info0 = info(&db, "quiet");
    event(&db, &info0, 1, ReasonCode::Ok, Outcome::Allow, &[], None);
    let record = db
        .assemble_record(
            &info0,
            RecordInputs {
                commits: Vec::new(),
                ..inputs(EndedReason::Clean, 5)
            },
        )
        .unwrap();
    assert!(!is_worth_recording(&record));
}

#[test]
fn worktree_names_drop_the_directories() {
    assert_eq!(worktree_name("/home/sara/work/repo"), "repo");
    assert_eq!(worktree_name("C:\\Users\\sara\\work\\repo\\"), "repo");
    assert_eq!(worktree_name("repo"), "repo");
}

#[test]
fn due_records_follow_ends_crashes_and_comebacks() {
    let repo = TestRepo::new();
    let db = repo.open();
    let crash_before = at(3600);

    let ended = info(&db, "ended");
    let crashed = info(&db, "crashed");
    let busy = info(&db, "busy");
    let human = db
        .get_or_create_seat(
            &acs_store::NewSeat {
                harness: "human",
                harness_session_id: "me",
                pilot: "sara@example.com",
                worktree: "/w",
                branch: None,
                target_branch: "main",
            },
            at(0),
        )
        .unwrap();
    db.touch_seat(&crashed.seat.seat_id, at(100)).unwrap();
    db.touch_seat(&busy.seat.seat_id, at(3700)).unwrap();
    db.end_seat(&ended.seat.seat_id, at(50)).unwrap();
    let _ = human;

    let due = db.records_due(crash_before).unwrap();
    let summary: Vec<_> = due
        .iter()
        .map(|d| (d.info.seat.harness_session_id.clone(), d.reason))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("ended".to_owned(), EndedReason::Clean),
            ("crashed".to_owned(), EndedReason::Crashed),
        ],
        "a live seat seen recently and the human seat are not due"
    );

    // Recording the ended seat (as skipped) settles it; the crashed one is still due.
    let ended_info = db.seat_info(&ended.seat.seat_id).unwrap().unwrap();
    db.mark_recorded(
        &ended_info,
        ended_info.seat.last_seen_at,
        EndedReason::Clean,
        None,
        false,
        at(4000),
    )
    .unwrap();
    let crashed_info = db.seat_info(&crashed.seat.seat_id).unwrap().unwrap();
    db.mark_recorded(
        &crashed_info,
        crashed_info.seat.last_seen_at,
        EndedReason::Crashed,
        Some("c0ffee"),
        true,
        at(4000),
    )
    .unwrap();
    assert!(db.records_due(crash_before).unwrap().is_empty());
    let remembered = db.recorded_seat(&crashed.seat.seat_id).unwrap().unwrap();
    assert_eq!(remembered.ledger_commit.as_deref(), Some("c0ffee"));
    assert!(remembered.signed);
    assert_eq!(remembered.ended_reason, EndedReason::Crashed);

    // The crashed seat comes back and ends: it is due again, now clean.
    db.touch_seat(&crashed.seat.seat_id, at(5000)).unwrap();
    db.end_seat(&crashed.seat.seat_id, at(5001)).unwrap();
    let due = db.records_due(crash_before).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].info.seat.harness_session_id, "crashed");
    assert_eq!(due[0].reason, EndedReason::Clean);
    assert_eq!(
        due[0].info.session_id, crashed_info.session_id,
        "same session id"
    );
    assert!(db.recorded_seat(&busy.seat.seat_id).unwrap().is_none());
}
