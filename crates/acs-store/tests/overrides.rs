//! Human overrides: granting, lapsing, binding to a seat, using up and recording (decision 0026).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod store_common;

use acs_core::{OverrideId, OverrideMode};
use acs_store::{NewOverride, StateDb};
use store_common::{TestRepo, at, path, seat};

fn grant(db: &StateDb, pattern: &str, mode: OverrideMode, secs: i64) -> OverrideId {
    db.grant_override(&NewOverride {
        pattern,
        mode,
        granted_by: "omar@example.com",
        now: at(secs),
        expires_at: at(secs + 900),
    })
    .unwrap()
}

#[test]
fn a_once_override_is_used_up_by_the_first_seat() {
    let repo = TestRepo::new();
    let db = repo.open();
    let a = seat(&db, "sara@example.com", "a");
    let b = seat(&db, "sara@example.com", "b");
    let id = grant(&db, "payments/**", OverrideMode::Once, 0);

    assert_eq!(db.overrides_for_seat(&a.seat_id, at(10)).unwrap().len(), 1);
    assert_eq!(db.active_overrides(at(10)).unwrap().len(), 1);
    assert!(
        db.apply_overrides(&a.seat_id, &[(id.clone(), path("payments/a.rs"))], at(10))
            .unwrap()
    );
    assert!(
        db.overrides_for_seat(&a.seat_id, at(11))
            .unwrap()
            .is_empty()
    );
    assert!(
        db.overrides_for_seat(&b.seat_id, at(11))
            .unwrap()
            .is_empty()
    );
    assert!(db.active_overrides(at(11)).unwrap().is_empty());
    assert!(
        !db.apply_overrides(&b.seat_id, &[(id.clone(), path("payments/b.rs"))], at(12))
            .unwrap(),
        "a used-up override cannot be used again"
    );
    let stored = db.override_by_id(&id).unwrap().unwrap();
    assert_eq!(stored.seat_id.as_ref(), Some(&a.seat_id));
    assert_eq!(stored.consumed_at, Some(at(10)));

    let uses = db.override_uses_for_seat(&a.seat_id).unwrap();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].override_id, id);
    assert_eq!(uses[0].paths, vec![path("payments/a.rs")]);
    assert_eq!(uses[0].granted_by, "omar@example.com");
    assert!(db.override_uses_for_seat(&b.seat_id).unwrap().is_empty());
}

#[test]
fn a_session_override_binds_to_its_first_seat_and_lasts_until_it_ends() {
    let repo = TestRepo::new();
    let db = repo.open();
    let a = seat(&db, "sara@example.com", "a");
    let b = seat(&db, "sara@example.com", "b");
    let id = grant(&db, "payments/**", OverrideMode::Session, 0);

    for (secs, file) in [
        (10, "payments/a.rs"),
        (5000, "payments/b.rs"),
        (6000, "payments/a.rs"),
    ] {
        assert!(
            db.apply_overrides(&a.seat_id, &[(id.clone(), path(file))], at(secs))
                .unwrap(),
            "{file} at {secs}"
        );
    }
    assert_eq!(
        db.overrides_for_seat(&a.seat_id, at(7000)).unwrap().len(),
        1,
        "bound overrides do not lapse"
    );
    assert!(
        db.overrides_for_seat(&b.seat_id, at(20))
            .unwrap()
            .is_empty()
    );
    assert!(
        !db.apply_overrides(&b.seat_id, &[(id.clone(), path("payments/c.rs"))], at(20))
            .unwrap()
    );
    let uses = db.override_uses_for_seat(&a.seat_id).unwrap();
    assert_eq!(
        uses[0].paths,
        vec![path("payments/a.rs"), path("payments/b.rs")],
        "each path once, in order of first use"
    );

    assert_eq!(db.active_overrides(at(7000)).unwrap().len(), 1);
    db.end_seat(&a.seat_id, at(7001)).unwrap();
    assert!(
        db.active_overrides(at(7002)).unwrap().is_empty(),
        "an ended session's override is no longer active"
    );
}

#[test]
fn unused_overrides_lapse_and_revoked_ones_stop_at_once() {
    let repo = TestRepo::new();
    let db = repo.open();
    let a = seat(&db, "sara@example.com", "a");
    let lapsing = grant(&db, "a/**", OverrideMode::Once, 0);
    assert_eq!(db.overrides_for_seat(&a.seat_id, at(899)).unwrap().len(), 1);
    assert!(
        db.overrides_for_seat(&a.seat_id, at(900))
            .unwrap()
            .is_empty()
    );
    assert!(
        !db.apply_overrides(&a.seat_id, &[(lapsing, path("a/x"))], at(901))
            .unwrap()
    );

    let revoked = grant(&db, "b/**", OverrideMode::Session, 1000);
    assert!(db.revoke_override(&revoked, at(1001)).unwrap());
    assert!(
        !db.revoke_override(&revoked, at(1002)).unwrap(),
        "only once"
    );
    assert!(
        db.overrides_for_seat(&a.seat_id, at(1003))
            .unwrap()
            .is_empty()
    );
    assert!(
        !db.apply_overrides(&a.seat_id, &[(revoked, path("b/x"))], at(1003))
            .unwrap()
    );
}

#[test]
fn a_failed_application_changes_nothing() {
    let repo = TestRepo::new();
    let db = repo.open();
    let a = seat(&db, "sara@example.com", "a");
    let good = grant(&db, "a/**", OverrideMode::Once, 0);
    let gone = grant(&db, "b/**", OverrideMode::Once, 0);
    db.revoke_override(&gone, at(1)).unwrap();
    assert!(
        !db.apply_overrides(
            &a.seat_id,
            &[(good.clone(), path("a/x")), (gone, path("b/x"))],
            at(2)
        )
        .unwrap()
    );
    assert_eq!(
        db.overrides_for_seat(&a.seat_id, at(3)).unwrap()[0].override_id,
        good,
        "the good override was not used up"
    );
    assert!(db.override_uses_for_seat(&a.seat_id).unwrap().is_empty());
}
