//! Helpers for the state-store tests. Not every test file uses every helper.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use acs_core::gate::ClaimRequest;
use acs_core::{ClaimOrigin, ClaimScope, RepoPath, Seat, SymbolPath, Timestamp};
use acs_store::{AcsHome, NewSeat, RepoHome, RepoId, StateDb};
use tempfile::TempDir;

/// A temporary `ACS_HOME` with one repository.
pub struct TestRepo {
    pub dir: TempDir,
    pub home: RepoHome,
}

impl TestRepo {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("acs-state-")
            .tempdir()
            .unwrap();
        let home = AcsHome::at(dir.path().join("acs"))
            .unwrap()
            .repo(&RepoId::from_roots(["a".repeat(40)]));
        Self { dir, home }
    }

    pub fn open(&self) -> StateDb {
        StateDb::open(&self.home).unwrap()
    }
}

/// The base of the injected clock: 2027-01-15T08:00:00Z.
pub const T0: i64 = 1_800_000_000;

/// The injected clock at `T0 + secs`.
pub fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(T0 + secs).unwrap()
}

/// How long the remote view stays fresh in tests.
pub const STALE_AFTER: Duration = Duration::from_secs(90);

pub fn path(p: &str) -> RepoPath {
    p.parse().unwrap()
}

pub fn file(p: &str) -> ClaimScope {
    ClaimScope::File { file: path(p) }
}

pub fn symbol(p: &str, symbol_path: &str) -> ClaimScope {
    ClaimScope::Symbol {
        symbol: path(p),
        symbol_path: symbol_path.parse::<SymbolPath>().unwrap(),
    }
}

pub fn request(scope: ClaimScope, branch: &str, ttl_secs: u64) -> ClaimRequest {
    ClaimRequest {
        scope,
        target_branch: branch.to_owned(),
        ttl_secs,
        origin: ClaimOrigin::Agent,
    }
}

/// A file claim request on `main` with a 900 s TTL.
pub fn want(p: &str) -> ClaimRequest {
    request(file(p), "main", 900)
}

/// Creates (or finds) the seat of a session of `pilot` on `main`.
pub fn seat(db: &StateDb, pilot: &str, session: &str) -> Seat {
    db.get_or_create_seat(
        &NewSeat {
            harness: "claude-code",
            harness_session_id: session,
            pilot,
            worktree: "/work/repo",
            branch: Some("feature"),
            target_branch: "main",
        },
        at(0),
    )
    .unwrap()
}
