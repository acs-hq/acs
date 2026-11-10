//! The storage interface and one sync, against a real bare remote.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use acs_core::gate::ClaimRequest;
use acs_core::{Claim, ClaimOrigin, ClaimScope, ClaimsDocument, SeatId, SpecVersion, Timestamp};
use acs_git::GitError;
use acs_git::refs::{
    CLAIMS_FILE, CustomRefs, LedgerAppend, LedgerEntry, Pilot, PilotDocument, RECORD_FILE,
    SyncStorage, claims_ref, ledger_ref,
};
use acs_git::sync::{SyncError, SyncOptions, needs_publishing, run};
use acs_store::{AcsHome, NewSeat, RepoIdentity, StateDb};
use tempfile::TempDir;

const SARA: &str = "sara@example.com";
const OMAR: &str = "omar@example.com";
const STALE_AFTER: Duration = Duration::from_secs(90);

fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 + secs).unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A bare remote with one commit, and clones of it that do not share `ACS_HOME`: machines.
struct Team {
    _tmp: TempDir,
    base: PathBuf,
    remote: PathBuf,
}

/// One clone and its state database.
struct Machine {
    repo: PathBuf,
    db: StateDb,
    pilot: String,
}

impl Team {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();
        let remote = base.join("remote.git");
        fs::create_dir(&remote).unwrap();
        git(&remote, &["init", "-q", "--bare", "-b", "main", "."]);
        let seed = base.join("seed");
        fs::create_dir(&seed).unwrap();
        git(&seed, &["init", "-q", "-b", "main", "."]);
        fs::write(seed.join("a.txt"), "a\n").unwrap();
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-q", "-m", "init"]);
        git(
            &seed,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&seed, &["push", "-q", "origin", "main"]);
        Self {
            _tmp: tmp,
            base,
            remote,
        }
    }

    fn machine(&self, name: &str, pilot: &str) -> Machine {
        let repo = self.base.join(name);
        git(
            &self.base,
            &[
                "clone",
                "-q",
                self.remote.to_str().unwrap(),
                repo.to_str().unwrap(),
            ],
        );
        git(&repo, &["config", "user.email", pilot]);
        let home = AcsHome::at(self.base.join(format!("{name}-home"))).unwrap();
        let identity = RepoIdentity::resolve(&home, &repo).unwrap();
        let db = StateDb::open(&identity.home(&home)).unwrap();
        Machine {
            repo,
            db,
            pilot: pilot.to_owned(),
        }
    }

    fn remote_ref(&self, name: &str) -> Option<String> {
        let line = git(&self.remote, &["for-each-ref", name]);
        line.split_whitespace().next().map(str::to_owned)
    }

    fn published(&self, pilot: &str) -> ClaimsDocument {
        let name = claims_ref(&acs_core::pilot_id(pilot));
        let text = git(&self.remote, &["show", &format!("{name}:{CLAIMS_FILE}")]);
        serde_json::from_str(&text).unwrap()
    }
}

impl Machine {
    fn storage(&self) -> CustomRefs {
        CustomRefs::new(&self.repo, "origin")
    }

    fn sync(&self, now: i64) -> Result<acs_git::sync::SyncReport, SyncError> {
        run(
            &self.storage(),
            &self.db,
            &SyncOptions {
                pilot: &self.pilot,
                now: at(now),
            },
        )
    }

    fn seat(&self, session: &str) -> SeatId {
        self.db
            .get_or_create_seat(
                &NewSeat {
                    harness: "claude-code",
                    harness_session_id: session,
                    pilot: &self.pilot,
                    worktree: self.repo.to_str().unwrap(),
                    branch: Some("main"),
                    target_branch: "main",
                },
                at(0),
            )
            .unwrap()
            .seat_id
    }

    fn claim(&self, seat: &SeatId, path: &str, now: i64) -> acs_core::ClaimId {
        let request = ClaimRequest {
            scope: ClaimScope::File {
                file: path.parse().unwrap(),
            },
            target_branch: "main".to_owned(),
            ttl_secs: 900,
            origin: ClaimOrigin::Agent,
        };
        match self
            .db
            .try_acquire(seat, &[request], at(now), STALE_AFTER)
            .unwrap()
        {
            acs_store::AcquireOutcome::Granted(acquired) => acquired.claim_ids[0].clone(),
            acs_store::AcquireOutcome::Conflict(conflict) => panic!("{conflict:?}"),
        }
    }

    fn remote_paths(&self, now: i64) -> Vec<String> {
        self.db
            .live_remote_claims(at(now))
            .unwrap()
            .into_iter()
            .map(|held| held.claim.scope.path().to_string())
            .collect()
    }

    fn pilot(&self) -> Pilot {
        Pilot::new(&self.pilot)
    }
}

#[test]
fn a_claim_reaches_the_other_machine_and_its_release_follows() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let omar = team.machine("omar", OMAR);
    let seat = sara.seat("s1");
    let claim = sara.claim(&seat, "a.txt", 1);

    let report = sara.sync(2).unwrap();
    assert_eq!(report.documents_published, 1);
    assert_eq!(report.refs_pushed, 1);
    assert!(
        team.remote_ref(&claims_ref(&acs_core::pilot_id(SARA)))
            .is_some()
    );
    assert_eq!(team.published(SARA).claims[0].claim_id, claim);

    let report = omar.sync(3).unwrap();
    assert_eq!((report.remote_pilots, report.remote_claims), (1, 1));
    assert_eq!(omar.remote_paths(3), ["a.txt"]);

    sara.db
        .release_claims(&[claim], acs_core::ReleaseReason::Released, at(4))
        .unwrap();
    assert_eq!(sara.sync(5).unwrap().documents_published, 1);
    assert_eq!(team.published(SARA).claims, Vec::<Claim>::new());
    assert_eq!(omar.sync(6).unwrap().remote_claims, 0);
    assert!(omar.remote_paths(6).is_empty());
}

#[test]
fn nothing_to_say_touches_neither_the_refs_nor_the_remote() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let report = sara.sync(1).unwrap();
    assert_eq!(report.documents_published, 0);
    assert_eq!(report.refs_pushed, 0);
    assert_eq!(git(&team.remote, &["for-each-ref", "refs/acs"]), "");
    let status = sara.db.remote_sync_status().unwrap();
    assert_eq!(status.last_success_at, Some(at(1)));
}

#[test]
fn a_heartbeat_is_only_published_once_it_matters() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let seat = sara.seat("s1");
    sara.claim(&seat, "a.txt", 0);
    assert_eq!(sara.sync(1).unwrap().documents_published, 1);

    sara.db.heartbeat_seat_claims(&seat, at(60)).unwrap();
    assert_eq!(sara.sync(61).unwrap().refs_pushed, 0, "60s of a 900s TTL");

    sara.db.heartbeat_seat_claims(&seat, at(300)).unwrap();
    assert_eq!(sara.sync(301).unwrap().refs_pushed, 1, "300s of a 900s TTL");
    assert_eq!(team.published(SARA).claims[0].heartbeat_at, at(300));
}

#[test]
fn own_claims_fetched_back_never_block_this_machine() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let (one, two) = (sara.seat("s1"), sara.seat("s2"));
    let claim = sara.claim(&one, "a.txt", 1);
    sara.sync(2).unwrap();
    sara.sync(3).unwrap();
    assert!(sara.remote_paths(3).is_empty(), "the local table is enough");
    sara.db
        .release_claims(&[claim], acs_core::ReleaseReason::Released, at(4))
        .unwrap();
    // The remote still says seat one holds the file; seat two must get it anyway.
    sara.claim(&two, "a.txt", 5);
}

#[test]
fn one_pilot_on_two_machines_keeps_the_claims_of_both() {
    let team = Team::new();
    let desktop = team.machine("desktop", SARA);
    let laptop = team.machine("laptop", SARA);
    let on_desktop = desktop.seat("d1");
    let on_laptop = laptop.seat("l1");
    desktop.claim(&on_desktop, "a.txt", 1);
    desktop.sync(2).unwrap();
    laptop.claim(&on_laptop, "b.txt", 3);
    laptop.sync(4).unwrap();

    let mut paths: Vec<String> = team
        .published(SARA)
        .claims
        .iter()
        .map(|c| c.scope.path().to_string())
        .collect();
    paths.sort();
    assert_eq!(paths, ["a.txt", "b.txt"], "the laptop did not erase a.txt");

    desktop.sync(5).unwrap();
    assert_eq!(desktop.remote_paths(5), ["b.txt"]);
    assert_eq!(laptop.remote_paths(5), ["a.txt"]);

    // The ones this machine let go of leave the document, the others' stay.
    laptop
        .db
        .release_seat_claims(&on_laptop, acs_core::ReleaseReason::SessionEnd, at(6))
        .unwrap();
    laptop.sync(7).unwrap();
    let left: Vec<String> = team
        .published(SARA)
        .claims
        .iter()
        .map(|c| c.scope.path().to_string())
        .collect();
    assert_eq!(left, ["a.txt"]);
}

#[test]
fn a_fetch_failure_is_recorded_and_the_view_ages() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    sara.sync(1).unwrap();
    fs::rename(&team.remote, team.base.join("gone.git")).unwrap();
    let err = sara.sync(2).unwrap_err();
    assert!(matches!(err, SyncError::Git(_)), "{err}");
    let status = sara.db.remote_sync_status().unwrap();
    assert_eq!(status.last_success_at, Some(at(1)), "the last good fetch");
    assert_eq!(status.last_attempt_at, Some(at(2)));
    assert!(status.last_error.is_some());
}

#[test]
fn a_rejected_push_is_an_error_but_the_fetched_view_stays_fresh() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let omar = team.machine("omar", OMAR);
    let seat = sara.seat("s1");
    sara.claim(&seat, "a.txt", 1);
    sara.sync(2).unwrap();

    // A clone with no push access, as a visitor to an open-source project has.
    let hook = team.remote.join("hooks").join("pre-receive");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    make_executable(&hook);
    let visitor = omar.seat("o1");
    omar.claim(&visitor, "b.txt", 3);
    assert!(omar.sync(4).is_err(), "the push is rejected");
    let status = omar.db.remote_sync_status().unwrap();
    assert_eq!(status.last_success_at, Some(at(4)), "the fetch worked");
    assert!(status.last_error.is_some());
    assert_eq!(omar.remote_paths(4), ["a.txt"]);
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

#[test]
fn sync_runs_no_push_hook_and_leaves_fetch_head_alone() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let hook = sara.repo.join(".git").join("hooks").join("pre-push");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    make_executable(&hook);
    git(&sara.repo, &["fetch", "-q", "origin", "main"]);
    let fetch_head = sara.repo.join(".git").join("FETCH_HEAD");
    let before = fs::read(&fetch_head).unwrap();

    let seat = sara.seat("s1");
    sara.claim(&seat, "a.txt", 1);
    assert_eq!(sara.sync(2).unwrap().refs_pushed, 1, "the hook did not run");
    assert_eq!(fs::read(&fetch_head).unwrap(), before);
}

#[test]
fn documents_that_lie_about_their_owner_are_ignored() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let omar = team.machine("omar", OMAR);
    let seat = sara.seat("s1");
    sara.claim(&seat, "a.txt", 1);
    // Sara publishes under her own ref a document that says it is Omar's.
    let mut document = {
        sara.sync(2).unwrap();
        team.published(SARA)
    };
    document.pilot = OMAR.to_owned();
    sara.storage()
        .write_claims(&sara.pilot(), &serde_json::to_vec(&document).unwrap())
        .unwrap();
    sara.storage().push(&[sara.pilot()], &[]).unwrap();
    let report = omar.sync(3).unwrap();
    assert_eq!(report.remote_claims, 0);
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(report.warnings[0].contains("other than the owner"));

    sara.storage()
        .write_claims(&sara.pilot(), b"not json")
        .unwrap();
    sara.storage().push(&[sara.pilot()], &[]).unwrap();
    assert_eq!(omar.sync(4).unwrap().warnings.len(), 1);
}

#[test]
fn a_pilot_whose_ref_disappeared_leaves_the_view() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let omar = team.machine("omar", OMAR);
    let seat = sara.seat("s1");
    sara.claim(&seat, "a.txt", 1);
    sara.sync(2).unwrap();
    omar.sync(3).unwrap();
    assert_eq!(omar.remote_paths(3), ["a.txt"]);
    git(
        &team.remote,
        &["update-ref", "-d", &claims_ref(&acs_core::pilot_id(SARA))],
    );
    omar.sync(4).unwrap();
    assert!(omar.remote_paths(4).is_empty());
}

// ---- the ledger ---------------------------------------------------------------------------

#[test]
fn the_ledger_is_a_chain_of_one_commit_per_record() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let storage = sara.storage();
    let pilot = sara.pilot();
    let one = storage
        .append_ledger(&pilot, "ses_1", b"{\"n\":1}")
        .unwrap();
    let two = storage
        .append_ledger(&pilot, "ses_2", b"{\"n\":2}")
        .unwrap();
    assert!(!one.signed && !two.signed);
    let name = ledger_ref(&pilot.id);
    assert_eq!(git(&sara.repo, &["rev-parse", &name]), two.commit);
    assert_eq!(
        git(&sara.repo, &["rev-parse", &format!("{name}^")]),
        one.commit
    );
    assert_eq!(
        git(&sara.repo, &["log", "-1", "--format=%s %ae", &name]),
        format!("session ses_2 {SARA}")
    );
    assert_eq!(
        git(&sara.repo, &["ls-tree", "--name-only", &name]),
        RECORD_FILE
    );
    let records: Vec<Vec<u8>> = storage
        .ledger_entries(&pilot.id)
        .unwrap()
        .into_iter()
        .map(|entry| entry.record)
        .collect();
    assert_eq!(records, [b"{\"n\":1}".to_vec(), b"{\"n\":2}".to_vec()]);

    sara.sync(1).unwrap();
    assert_eq!(team.remote_ref(&name), Some(two.commit.clone()));
    // A second sync has nothing to push, and the next record is a fast-forward.
    assert_eq!(sara.sync(2).unwrap().refs_pushed, 0);
    let three = storage
        .append_ledger(&pilot, "ses_3", b"{\"n\":3}")
        .unwrap();
    assert_eq!(sara.sync(3).unwrap().refs_pushed, 1);
    assert_eq!(team.remote_ref(&name), Some(three.commit));
}

#[test]
fn ledger_pilots_lists_own_and_fetched_ledgers_once() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let bob = team.machine("bob", "bob@example.com");
    assert!(sara.storage().ledger_pilots().unwrap().is_empty());
    sara.storage()
        .append_ledger(&sara.pilot(), "ses_1", b"1")
        .unwrap();
    bob.storage()
        .append_ledger(&bob.pilot(), "ses_2", b"2")
        .unwrap();
    bob.sync(1).unwrap();
    sara.sync(2).unwrap();
    // Sara's own ledger is both local and fetched back; Bob's is fetched only.
    let mut expected = vec![sara.pilot().id, bob.pilot().id];
    expected.sort();
    assert_eq!(sara.storage().ledger_pilots().unwrap(), expected);
    let bobs = sara.storage().ledger_entries(&bob.pilot().id).unwrap();
    assert_eq!(bobs.len(), 1);
    assert_eq!(bobs[0].record, b"2");
}

#[test]
fn concurrent_appends_lose_nothing() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let storage = sara.storage();
            let pilot = sara.pilot();
            std::thread::spawn(move || {
                storage
                    .append_ledger(&pilot, &format!("ses_{i}"), format!("{i}").as_bytes())
                    .unwrap();
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let mut records: Vec<String> = sara
        .storage()
        .ledger_entries(&sara.pilot().id)
        .unwrap()
        .into_iter()
        .map(|entry| String::from_utf8(entry.record).unwrap())
        .collect();
    records.sort();
    assert_eq!(records, ["0", "1", "2", "3", "4", "5", "6", "7"]);
}

#[test]
fn chains_of_one_pilot_on_two_machines_are_joined_without_rewriting() {
    let team = Team::new();
    let desktop = team.machine("desktop", SARA);
    let laptop = team.machine("laptop", SARA);
    let pilot = desktop.pilot();
    let name = ledger_ref(&pilot.id);

    let first = desktop
        .storage()
        .append_ledger(&pilot, "ses_1", b"desktop 1")
        .unwrap();
    desktop.sync(1).unwrap();

    // The laptop picks the chain up and diverges from what the desktop writes next.
    laptop.sync(2).unwrap();
    assert_eq!(
        git(&laptop.repo, &["rev-parse", &name]),
        first.commit,
        "a fetched ledger is adopted"
    );
    let on_laptop = laptop
        .storage()
        .append_ledger(&pilot, "ses_2", b"laptop 2")
        .unwrap();
    let on_desktop = desktop
        .storage()
        .append_ledger(&pilot, "ses_3", b"desktop 3")
        .unwrap();
    laptop.sync(3).unwrap();
    desktop.sync(4).unwrap();

    let tip = team.remote_ref(&name).unwrap();
    let parents = git(&team.remote, &["rev-list", "--parents", "-1", &tip]);
    assert!(
        parents.contains(&on_laptop.commit) && parents.contains(&on_desktop.commit),
        "{parents}"
    );
    laptop.sync(5).unwrap();
    let mut records: Vec<String> = laptop
        .storage()
        .ledger_entries(&pilot.id)
        .unwrap()
        .into_iter()
        .map(|entry| String::from_utf8(entry.record).unwrap())
        .collect();
    records.sort();
    assert_eq!(records, ["desktop 1", "desktop 3", "laptop 2"]);
}

#[test]
fn a_signing_failure_leaves_an_unsigned_record_not_a_lost_one() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    git(&sara.repo, &["config", "commit.gpgsign", "true"]);
    git(&sara.repo, &["config", "gpg.format", "ssh"]);
    git(
        &sara.repo,
        &["config", "user.signingkey", "/no/such/acs-test-key"],
    );
    let appended = sara
        .storage()
        .append_ledger(&sara.pilot(), "ses_1", b"record")
        .unwrap();
    assert!(!appended.signed);
    assert_eq!(
        sara.storage()
            .ledger_entries(&sara.pilot().id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn ledger_commits_are_signed_when_git_is_configured_to() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let key = team.base.join("signing_key");
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !made.is_ok_and(|status| status.success()) {
        eprintln!("skipped: ssh-keygen is not available");
        return;
    }
    git(&sara.repo, &["config", "commit.gpgsign", "true"]);
    git(&sara.repo, &["config", "gpg.format", "ssh"]);
    git(
        &sara.repo,
        &["config", "user.signingkey", key.to_str().unwrap()],
    );
    let appended = sara
        .storage()
        .append_ledger(&sara.pilot(), "ses_1", b"record")
        .unwrap();
    assert!(appended.signed);
    let raw = git(&sara.repo, &["cat-file", "commit", &appended.commit]);
    assert!(raw.contains("gpgsig"), "{raw}");
}

// ---- the interface ------------------------------------------------------------------------

/// Storage with no git in it at all: what a fallback has to provide.
#[derive(Default)]
struct InMemory {
    documents: std::cell::RefCell<Vec<PilotDocument>>,
}

impl SyncStorage for InMemory {
    fn fetch(&self) -> Result<(), GitError> {
        Ok(())
    }
    fn fetched_claims(&self) -> Result<Vec<PilotDocument>, GitError> {
        Ok(self.documents.borrow().clone())
    }
    fn write_claims(&self, pilot: &Pilot, document: &[u8]) -> Result<(), GitError> {
        self.documents.borrow_mut().push(PilotDocument {
            pilot_id: pilot.id.clone(),
            bytes: document.to_vec(),
        });
        Ok(())
    }
    fn append_ledger(
        &self,
        _pilot: &Pilot,
        _session_id: &str,
        _record: &[u8],
    ) -> Result<LedgerAppend, GitError> {
        unreachable!("a sync appends nothing")
    }
    fn ledger_entries(&self, _pilot_id: &str) -> Result<Vec<LedgerEntry>, GitError> {
        Ok(Vec::new())
    }
    fn ledger_pilots(&self) -> Result<Vec<String>, GitError> {
        Ok(Vec::new())
    }
    fn reconcile_ledger(&self, _pilot: &Pilot) -> Result<(), GitError> {
        Ok(())
    }
    fn push(&self, claims_of: &[Pilot], _ledgers_of: &[Pilot]) -> Result<usize, GitError> {
        Ok(claims_of.len())
    }
}

#[test]
fn a_sync_needs_nothing_but_the_interface() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let seat = sara.seat("s1");
    sara.claim(&seat, "a.txt", 1);
    let storage = InMemory::default();
    let options = SyncOptions {
        pilot: SARA,
        now: at(2),
    };
    let report = run(&storage, &sara.db, &options).unwrap();
    assert_eq!((report.documents_published, report.refs_pushed), (1, 1));
    assert_eq!(storage.documents.borrow().len(), 1);
}

#[test]
fn publishing_is_judged_on_scope_branch_and_ttl_not_on_time_alone() {
    let claim = |ttl_secs, heartbeat| Claim {
        claim_id: "clm_1".parse().unwrap(),
        seat_id: "seat_1".parse().unwrap(),
        pilot: SARA.to_owned(),
        scope: ClaimScope::File {
            file: "a.txt".parse().unwrap(),
        },
        target_branch: "main".to_owned(),
        acquired_at: at(0),
        heartbeat_at: at(heartbeat),
        ttl_secs,
        origin: ClaimOrigin::Agent,
    };
    let document = ClaimsDocument {
        spec: SpecVersion,
        pilot: SARA.to_owned(),
        updated_at: at(0),
        claims: vec![claim(900, 0)],
    };
    assert!(!needs_publishing(None, &[]));
    assert!(needs_publishing(None, &[claim(900, 0)]));
    assert!(needs_publishing(Some(&document), &[]));
    assert!(!needs_publishing(Some(&document), &[claim(900, 224)]));
    assert!(needs_publishing(Some(&document), &[claim(900, 225)]));
    assert!(needs_publishing(Some(&document), &[claim(60, 0)]));
    let mut moved = claim(900, 0);
    moved.target_branch = "release/1".to_owned();
    assert!(needs_publishing(Some(&document), &[moved]));
    let mut other = claim(900, 0);
    other.claim_id = "clm_2".parse().unwrap();
    assert!(needs_publishing(Some(&document), &[other]));
}
