//! Sixteen real OS processes race for overlapping claims on one database.
//!
//! The test binary re-invokes itself: the parent test spawns 16 children that run
//! `claim_race_child` (a no-op unless the `ACS_RACE_*` variables are set, which only the parent
//! sets, on the children). Each child opens the same, initially empty, database and loops:
//! ask for 1 to 3 overlapping scopes, and when granted log `enter`, hold them for a moment,
//! sometimes send a heartbeat, log `exit`, release. Afterwards the parent replays the log in
//! order and fails if two overlapping scopes were ever held by different seats at once.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod store_common;

use std::process::{Command, Stdio};
use std::time::Duration;

use acs_core::gate::ClaimRequest;
use acs_core::{ClaimScope, ReleaseReason, Timestamp};
use acs_store::{AcquireOutcome, AcsHome, DbConfig, NewSeat, RepoId, StateDb};
use rusqlite::{Connection, params};
use store_common::{file, request, symbol};

const WORKERS: usize = 16;
const ITERATIONS: usize = 60;
const ROOT_VAR: &str = "ACS_RACE_ROOT";
const WORKER_VAR: &str = "ACS_RACE_WORKER";

fn repo_id() -> RepoId {
    RepoId::from_roots(["b".repeat(40)])
}

/// Scopes that overlap each other in many ways: shared files, a class and its method, siblings.
fn pool() -> Vec<ClaimScope> {
    vec![
        file("f0.rs"),
        file("f1.rs"),
        file("f2.rs"),
        file("f3.rs"),
        file("s.rs"),
        symbol("s.rs", "C"),
        symbol("s.rs", "C.m"),
        symbol("s.rs", "D"),
        symbol("t.rs", "A"),
        symbol("t.rs", "B"),
    ]
}

/// Tiny xorshift generator; the test needs spread, not quality.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The child: runs only when the parent started it.
#[test]
fn claim_race_child() {
    let (Ok(root), Ok(worker)) = (std::env::var(ROOT_VAR), std::env::var(WORKER_VAR)) else {
        return;
    };
    let worker: usize = worker.parse().unwrap();
    let home = AcsHome::at(root).unwrap().repo(&repo_id());
    let config = DbConfig {
        busy_timeout: Duration::from_secs(60),
        ..DbConfig::default()
    };
    // All children open a database that may not exist yet: creation and migration race too.
    let db = StateDb::open_with(&home, &config).unwrap();
    let log = Connection::open(db.path()).unwrap();
    log.busy_timeout(Duration::from_secs(60)).unwrap();
    log.execute_batch(
        "CREATE TABLE IF NOT EXISTS race_log (seq INTEGER PRIMARY KEY AUTOINCREMENT, \
         kind TEXT NOT NULL, worker INTEGER NOT NULL, scopes TEXT NOT NULL)",
    )
    .unwrap();

    let seat = db
        .get_or_create_seat(
            &NewSeat {
                harness: "race",
                harness_session_id: &format!("worker-{worker}"),
                // Several workers share a pilot on purpose: same pilot, different seats conflict.
                pilot: &format!("pilot{}@example.com", worker % 4),
                worktree: "/w",
                branch: None,
                target_branch: "main",
            },
            Timestamp::now(),
        )
        .unwrap();
    let scopes = pool();
    let seed = (worker as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ seed);
    let (mut grants, mut conflicts) = (0u32, 0u32);

    for _ in 0..ITERATIONS {
        let wanted: Vec<ClaimScope> = (0..1 + rng.below(3))
            .map(|_| scopes[rng.below(scopes.len())].clone())
            .collect();
        let requests: Vec<ClaimRequest> = wanted
            .iter()
            .map(|s| request(s.clone(), "main", 600))
            .collect();
        let outcome = db
            .try_acquire(
                &seat.seat_id,
                &requests,
                Timestamp::now(),
                Duration::from_secs(90),
            )
            .unwrap();
        match outcome {
            AcquireOutcome::Conflict(_) => {
                conflicts += 1;
                std::thread::sleep(Duration::from_micros(100 + rng.below(400) as u64));
            }
            AcquireOutcome::Granted(acquired) => {
                grants += 1;
                let json = serde_json::to_string(&wanted).unwrap();
                let record = |kind: &str| {
                    log.execute(
                        "INSERT INTO race_log (kind, worker, scopes) VALUES (?1, ?2, ?3)",
                        params![kind, worker as i64, json],
                    )
                    .unwrap();
                };
                record("enter");
                std::thread::sleep(Duration::from_micros(200 + rng.below(800) as u64));
                if rng.below(3) == 0 {
                    // A repeat of the same request is a heartbeat and must return the same ids.
                    let again = db
                        .try_acquire(
                            &seat.seat_id,
                            &requests,
                            Timestamp::now(),
                            Duration::from_secs(90),
                        )
                        .unwrap();
                    match again {
                        AcquireOutcome::Granted(a) => assert_eq!(a.claim_ids, acquired.claim_ids),
                        AcquireOutcome::Conflict(c) => panic!("own heartbeat conflicted: {c:?}"),
                    }
                }
                record("exit");
                db.release_claims(
                    &acquired.claim_ids,
                    ReleaseReason::Released,
                    Timestamp::now(),
                )
                .unwrap();
            }
        }
    }
    println!("RACE worker={worker} grants={grants} conflicts={conflicts}");
}

fn field(output: &str, key: &str) -> u32 {
    // The harness prints "test claim_race_child ... " before the child's own output.
    let line = output
        .lines()
        .find_map(|l| l.split_once("RACE ").map(|(_, rest)| rest))
        .unwrap();
    let pair = line
        .split_whitespace()
        .find(|w| w.starts_with(key))
        .unwrap();
    pair[key.len()..].parse().unwrap()
}

#[test]
fn sixteen_processes_never_hold_conflicting_claims() {
    let dir = tempfile::Builder::new()
        .prefix("acs-race-")
        .tempdir()
        .unwrap();
    let root = dir.path().join("acs");
    let exe = std::env::current_exe().unwrap();

    let children: Vec<_> = (0..WORKERS)
        .map(|worker| {
            Command::new(&exe)
                .args([
                    "--exact",
                    "claim_race_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(ROOT_VAR, &root)
                .env(WORKER_VAR, worker.to_string())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();

    let (mut grants, mut conflicts) = (0, 0);
    for child in children {
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(
            output.status.success(),
            "child failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        grants += field(&stdout, "grants=");
        conflicts += field(&stdout, "conflicts=");
    }
    assert!(grants > 0, "no child ever got a claim");
    assert!(
        conflicts > 0,
        "no contention happened: the test proves nothing"
    );

    // Replay the log: no two overlapping scopes held by different workers at the same time.
    let home = AcsHome::at(&root).unwrap().repo(&repo_id());
    let raw = Connection::open(home.state_db()).unwrap();
    let mut stmt = raw
        .prepare("SELECT kind, worker, scopes FROM race_log ORDER BY seq")
        .unwrap();
    let rows: Vec<(String, i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len() as u32, 2 * grants);
    let mut active: Vec<(i64, Vec<ClaimScope>)> = Vec::new();
    for (kind, worker, scopes) in rows {
        let scopes: Vec<ClaimScope> = serde_json::from_str(&scopes).unwrap();
        if kind == "enter" {
            for (other, held) in &active {
                for mine in &scopes {
                    for theirs in held {
                        assert!(
                            !mine.overlaps(theirs),
                            "workers {worker} and {other} held overlapping {mine:?} and {theirs:?} at once"
                        );
                    }
                }
            }
            active.push((worker, scopes));
        } else {
            let at = active.iter().position(|(w, _)| *w == worker).unwrap();
            active.remove(at);
        }
    }
    assert!(active.is_empty());

    // Everything was released; every grant left exactly one history row.
    let db = StateDb::open(&home).unwrap();
    assert!(db.live_claims(Timestamp::now()).unwrap().is_empty());
    let history: i64 = raw
        .query_row("SELECT count(*) FROM claim_history", [], |r| r.get(0))
        .unwrap();
    let seats: i64 = raw
        .query_row("SELECT count(*) FROM seats", [], |r| r.get(0))
        .unwrap();
    assert_eq!(seats, WORKERS as i64);
    // A grant claims one row per distinct scope of the request, so history >= grants.
    assert!(
        history >= i64::from(grants),
        "{history} history rows for {grants} grants"
    );
    println!("grants={grants} conflicts={conflicts} history={history}");
}
