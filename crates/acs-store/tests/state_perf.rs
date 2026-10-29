//! Latency of the claim path on a warm database with about 1,000 live claims.
//!
//! `claims_snapshot` and `try_acquire` sit on the hook's 15 ms p50 budget (plan section 4.7).
//! Run `cargo test -p acs-store --release --test state_perf -- --nocapture` to see the numbers;
//! the budget is only asserted in release builds.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod store_common;

use std::time::{Duration, Instant};

use acs_store::{AcquireOutcome, StateDb};
use store_common::{STALE_AFTER, TestRepo, at, path, seat, want};

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn report(name: &str, mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    let p50 = percentile(&samples, 0.5);
    let p99 = percentile(&samples, 0.99);
    println!("{name:<44} p50 {p50:>10?}   p99 {p99:>10?}");
    p50
}

#[test]
fn claim_path_latency_with_a_thousand_claims() {
    let repo = TestRepo::new();
    let db = repo.open();

    // 1,000 live claims over 20 seats, as 20 events of 50 files each.
    let seats: Vec<_> = (0..20)
        .map(|i| seat(&db, &format!("pilot{i}@example.com"), &format!("s{i}")))
        .collect();
    for (i, s) in seats.iter().enumerate() {
        let requests: Vec<_> = (0..50)
            .map(|j| want(&format!("src/mod{i}/file{j}.rs")))
            .collect();
        let outcome = db
            .try_acquire(&s.seat_id, &requests, at(0), STALE_AFTER)
            .unwrap();
        assert!(matches!(outcome, AcquireOutcome::Granted(_)));
    }
    assert_eq!(db.live_claims(at(1)).unwrap().len(), 1000);
    drop(db);

    // The hook's view: a new process opens the database once, then asks.
    let mut opens = Vec::new();
    for _ in 0..50 {
        let started = Instant::now();
        let db = StateDb::open(&repo.home).unwrap();
        opens.push(started.elapsed());
        drop(db);
    }
    let db = repo.open();
    let me = &seats[0];

    const N: i64 = 300;
    let (mut snapshots, mut heartbeats, mut fresh, mut both) = (vec![], vec![], vec![], vec![]);
    for i in 0..N {
        let now = at(1 + i);
        // An edit of a file this seat already holds: snapshot, then a heartbeat request.
        let held = path("src/mod0/file7.rs");
        let started = Instant::now();
        let snap = db
            .claims_snapshot(std::slice::from_ref(&held), "main", now, STALE_AFTER)
            .unwrap();
        snapshots.push(started.elapsed());
        assert_eq!(snap.claims.len(), 1);

        let started = Instant::now();
        let out = db
            .try_acquire(&me.seat_id, &[want(held.as_str())], now, STALE_AFTER)
            .unwrap();
        heartbeats.push(started.elapsed());
        assert!(matches!(out, AcquireOutcome::Granted(_)));

        // A new file: snapshot, then a real insert.
        let new = format!("src/new/f{i}.rs");
        let started = Instant::now();
        let snap = db
            .claims_snapshot(&[path(&new)], "main", now, STALE_AFTER)
            .unwrap();
        let out = db
            .try_acquire(&me.seat_id, &[want(&new)], now, STALE_AFTER)
            .unwrap();
        let elapsed = started.elapsed();
        both.push(elapsed);
        assert!(snap.claims.is_empty() && matches!(out, AcquireOutcome::Granted(_)));
        let started = Instant::now();
        db.try_acquire(&me.seat_id, &[want(&new)], now, STALE_AFTER)
            .unwrap();
        fresh.push(started.elapsed());
    }

    println!();
    let open_p50 = report("open (existing db, pragmas, version check)", opens);
    report("claims_snapshot, 1 path", snapshots);
    let heartbeat_p50 = report("try_acquire, 1 request (heartbeat)", heartbeats);
    report("try_acquire, 1 request (repeat of new)", fresh);
    let both_p50 = report("snapshot + try_acquire (new claim)", both);

    if !cfg!(debug_assertions) {
        assert!(both_p50 < Duration::from_millis(3), "{both_p50:?}");
        assert!(
            heartbeat_p50 < Duration::from_millis(3),
            "{heartbeat_p50:?}"
        );
        assert!(open_p50 < Duration::from_millis(5), "{open_p50:?}");
    }
}
