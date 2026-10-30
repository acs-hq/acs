//! Tests for the snapshot store.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use acs_core::{RepoPath, SessionId, Timestamp};
use acs_store::repo_id::RepoId;
use acs_store::{AcsHome, FileState, SnapshotEntry, SnapshotError, SnapshotKind, SnapshotStore};

struct Fixture {
    _tmp: tempfile::TempDir,
    work: PathBuf,
    store: SnapshotStore,
    snapshots: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = AcsHome::at(tmp.path().join("acs")).unwrap();
    let repo = home.repo(&RepoId::from_path(&tmp.path().join("repo")));
    repo.ensure().unwrap();
    let work = tmp.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let snapshots = repo.snapshots_dir();
    let store = SnapshotStore::open(&repo).unwrap();
    Fixture {
        _tmp: tmp,
        work,
        store,
        snapshots,
    }
}

fn ses(n: u32) -> SessionId {
    format!("ses_{n}").parse().unwrap()
}

fn rp(s: &str) -> RepoPath {
    s.parse().unwrap()
}

fn t(secs: i64) -> Timestamp {
    Timestamp::from_second(secs).unwrap()
}

/// Deterministic pseudo-random bytes (incompressible enough to exercise zstd).
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x.to_le_bytes()[3]
        })
        .collect()
}

fn blob_file(f: &Fixture, entry: &SnapshotEntry) -> PathBuf {
    let FileState::File { hash, .. } = &entry.state else {
        panic!("absent entry")
    };
    let (shard, rest) = hash.as_str().split_at(2);
    f.snapshots.join("blobs").join(shard).join(rest)
}

fn capture_one(
    f: &Fixture,
    s: &SessionId,
    kind: SnapshotKind,
    name: &str,
    secs: i64,
) -> SnapshotEntry {
    let abs = f.work.join(name);
    let mut out = f
        .store
        .capture(s, kind, &[(rp(name), abs.as_path())], t(secs))
        .unwrap();
    out.remove(0).entry
}

fn make_writable(p: &Path) {
    let mut perms = fs::metadata(p).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(p, perms).unwrap();
}

#[test]
fn restore_is_byte_exact_including_empty_and_deleted_files() {
    let f = fixture();
    let s = ses(1);
    let big = noise(7, 300_000);
    let small = noise(8, 17);
    fs::write(f.work.join("big.bin"), &big).unwrap();
    fs::write(f.work.join("small.bin"), &small).unwrap();
    fs::write(f.work.join("empty"), b"").unwrap();

    let names = ["big.bin", "small.bin", "empty", "gone"];
    let entries: Vec<_> = names
        .iter()
        .map(|n| capture_one(&f, &s, SnapshotKind::Pre, n, 1))
        .collect();
    assert_eq!(entries[3].state, FileState::Absent);

    // Mutate everything, then restore.
    fs::write(f.work.join("big.bin"), b"changed").unwrap();
    fs::remove_file(f.work.join("small.bin")).unwrap();
    fs::write(f.work.join("empty"), b"not empty").unwrap();
    fs::write(f.work.join("gone"), b"created by the agent").unwrap();
    for (e, n) in entries.iter().zip(names) {
        f.store.restore(e, &f.work.join(n), t(2)).unwrap();
    }
    assert_eq!(fs::read(f.work.join("big.bin")).unwrap(), big);
    assert_eq!(fs::read(f.work.join("small.bin")).unwrap(), small);
    assert_eq!(fs::read(f.work.join("empty")).unwrap(), b"");
    assert!(!f.work.join("gone").exists());
}

#[test]
fn deleted_file_is_restored_into_a_missing_directory() {
    let f = fixture();
    let s = ses(1);
    fs::create_dir_all(f.work.join("a/b")).unwrap();
    fs::write(f.work.join("a/b/c.txt"), b"hello").unwrap();
    let e = capture_one(&f, &s, SnapshotKind::Pre, "a/b/c.txt", 1);
    fs::remove_dir_all(f.work.join("a")).unwrap();
    f.store
        .restore(&e, &f.work.join("a/b/c.txt"), t(2))
        .unwrap();
    assert_eq!(fs::read(f.work.join("a/b/c.txt")).unwrap(), b"hello");
}

#[cfg(unix)]
#[test]
fn restore_keeps_unix_modes() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    let s = ses(1);
    for (name, mode) in [("exec.sh", 0o755), ("ro.txt", 0o444), ("priv", 0o600)] {
        let p = f.work.join(name);
        fs::write(&p, name.as_bytes()).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
        let e = capture_one(&f, &s, SnapshotKind::Pre, name, 1);
        assert!(matches!(e.state, FileState::File { mode: m, .. } if m == mode));
        // Overwrite with different content and mode, restore over it.
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&p, b"x").unwrap();
        f.store.restore(&e, &p, t(2)).unwrap();
        assert_eq!(fs::read(&p).unwrap(), name.as_bytes());
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o7777,
            mode
        );
    }
}

#[test]
fn restore_keeps_the_readonly_flag() {
    let f = fixture();
    let s = ses(1);
    let p = f.work.join("ro.txt");
    fs::write(&p, b"locked").unwrap();
    let mut perms = fs::metadata(&p).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&p, perms).unwrap();
    let e = capture_one(&f, &s, SnapshotKind::Pre, "ro.txt", 1);

    // Replace it by a writable file with other content, then restore over it.
    make_writable(&p);
    fs::write(&p, b"other").unwrap();
    f.store.restore(&e, &p, t(2)).unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"locked");
    assert!(fs::metadata(&p).unwrap().permissions().readonly());

    // Restoring over a read-only file works too.
    f.store.restore(&e, &p, t(3)).unwrap();
    assert!(fs::metadata(&p).unwrap().permissions().readonly());
    make_writable(&p); // let the tempdir clean up on Windows
}

#[test]
fn identical_content_is_stored_once_across_sessions() {
    let f = fixture();
    fs::write(f.work.join("a"), b"same bytes").unwrap();
    fs::write(f.work.join("b"), b"same bytes").unwrap();
    let first = capture_one(&f, &ses(1), SnapshotKind::Pre, "a", 1);
    assert!(f.store.list(&ses(1)).unwrap().len() == 1);
    let (a, b) = (f.work.join("a"), f.work.join("b"));
    let results = f
        .store
        .capture(
            &ses(2),
            SnapshotKind::Pre,
            &[(rp("a"), a.as_path()), (rp("b"), b.as_path())],
            t(2),
        )
        .unwrap();
    assert!(results.iter().all(|r| !r.stored_new_blob));
    assert_eq!(first.state, results[0].entry.state);
    let blobs: usize = fs::read_dir(f.snapshots.join("blobs"))
        .unwrap()
        .map(|d| fs::read_dir(d.unwrap().path()).unwrap().count())
        .sum();
    assert_eq!(blobs, 1);
}

#[test]
fn pre_image_is_the_first_and_post_image_the_latest() {
    let f = fixture();
    let s = ses(1);
    let p = f.work.join("f.txt");
    assert!(f.store.pre_image(&s, &rp("f.txt")).unwrap().is_none());
    fs::write(&p, b"v0").unwrap();
    capture_one(&f, &s, SnapshotKind::Pre, "f.txt", 1);
    fs::write(&p, b"v1").unwrap();
    capture_one(&f, &s, SnapshotKind::Post, "f.txt", 2);
    capture_one(&f, &s, SnapshotKind::Pre, "f.txt", 3); // a later edit's pre-image
    fs::write(&p, b"v2").unwrap();
    capture_one(&f, &s, SnapshotKind::Post, "f.txt", 4);

    let pre = f.store.pre_image(&s, &rp("f.txt")).unwrap().unwrap();
    let post = f.store.post_image(&s, &rp("f.txt")).unwrap().unwrap();
    let dest = f.work.join("out");
    f.store.restore(&pre, &dest, t(5)).unwrap();
    assert_eq!(fs::read(&dest).unwrap(), b"v0");
    f.store.restore(&post, &dest, t(5)).unwrap();
    assert_eq!(fs::read(&dest).unwrap(), b"v2");
    assert_eq!(f.store.list(&s).unwrap().len(), 4);
    assert!(f.store.pre_image(&ses(9), &rp("f.txt")).unwrap().is_none());
}

#[test]
fn capture_tree_records_every_file_recursively() {
    let f = fixture();
    let s = ses(1);
    fs::create_dir_all(f.work.join("d/sub")).unwrap();
    fs::write(f.work.join("d/a.txt"), b"a").unwrap();
    fs::write(f.work.join("d/sub/b.txt"), b"").unwrap();
    let r = f
        .store
        .capture_tree(&s, SnapshotKind::Pre, &rp("d"), &f.work.join("d"), t(1))
        .unwrap();
    let paths: Vec<_> = r.iter().map(|r| r.entry.path.to_string()).collect();
    assert_eq!(paths, ["d/a.txt", "d/sub/b.txt"]);
    let none = f
        .store
        .capture_tree(
            &s,
            SnapshotKind::Pre,
            &rp("nope"),
            &f.work.join("nope"),
            t(1),
        )
        .unwrap();
    assert!(none.is_empty());
    let dir = f.work.join("d");
    let err = f
        .store
        .capture(&s, SnapshotKind::Pre, &[(rp("d"), dir.as_path())], t(1))
        .unwrap_err();
    assert!(matches!(err, SnapshotError::IsDirectory(_)));
}

#[test]
fn gc_never_evicts_live_sessions() {
    let f = fixture();
    let (live, dead) = (ses(1), ses(2));
    for (i, (s, name)) in [(&live, "live"), (&dead, "old"), (&dead, "new")]
        .into_iter()
        .enumerate()
    {
        fs::write(f.work.join(name), noise(i as u64 + 100, 20_000)).unwrap();
        capture_one(&f, s, SnapshotKind::Pre, name, 10 + i as i64);
    }
    let live_set: HashSet<_> = [live.clone()].into();

    // Generous budget: nothing happens.
    let report = f.store.gc(u64::MAX, &live_set).unwrap();
    assert_eq!((report.evicted_blobs, report.unreferenced_blobs), (0, 0));

    // Budget of zero: everything evictable goes, the live blob stays.
    let report = f.store.gc(0, &live_set).unwrap();
    assert_eq!(report.evicted_blobs, 2);
    assert!(report.over_budget);
    assert_eq!(report.remaining_bytes, report.live_bytes);
    assert!(report.live_bytes > 0);
    let live_entry = f.store.pre_image(&live, &rp("live")).unwrap().unwrap();
    f.store
        .restore(&live_entry, &f.work.join("live.out"), t(50))
        .unwrap();
    assert_eq!(fs::read(f.work.join("live.out")).unwrap().len(), 20_000);

    // The dead session's entries remain but report eviction.
    let old = f.store.pre_image(&dead, &rp("old")).unwrap().unwrap();
    let err = f
        .store
        .restore(&old, &f.work.join("old.out"), t(50))
        .unwrap_err();
    assert!(matches!(err, SnapshotError::Evicted { .. }), "{err}");
    assert!(err.to_string().contains("snapshot evicted"));
}

#[test]
fn gc_evicts_least_recently_used_until_under_budget() {
    let f = fixture();
    let s = ses(1);
    for i in 0..4u64 {
        let name = format!("f{i}");
        fs::write(f.work.join(&name), noise(i, 10_000)).unwrap();
        capture_one(&f, &s, SnapshotKind::Pre, &name, 10 + i as i64);
    }
    let total = f.store.stored_bytes().unwrap();
    // Touch f0 so it becomes the most recently used.
    let e0 = f.store.pre_image(&s, &rp("f0")).unwrap().unwrap();
    f.store
        .restore(&e0, &f.work.join("f0.out"), t(100))
        .unwrap();

    // Budget fits three of the four blobs.
    let budget = total - total / 4;
    let report = f.store.gc(budget, &HashSet::new()).unwrap();
    assert_eq!(report.evicted_blobs, 1);
    assert!(report.remaining_bytes <= budget);
    assert!(!report.over_budget);
    assert_eq!(f.store.stored_bytes().unwrap(), report.remaining_bytes);
    let restore = |n: &str| {
        let e = f.store.pre_image(&s, &rp(n)).unwrap().unwrap();
        f.store.restore(&e, &f.work.join("tmp.out"), t(101))
    };
    assert!(
        matches!(restore("f1"), Err(SnapshotError::Evicted { .. })),
        "f1 is the least recently used"
    );
    assert!(restore("f0").is_ok());
    assert!(restore("f2").is_ok());
    assert!(restore("f3").is_ok());
}

#[test]
fn gc_removes_unreferenced_blobs_and_orphan_files() {
    let f = fixture();
    let s = ses(1);
    fs::write(f.work.join("a"), noise(1, 1000)).unwrap();
    let e = capture_one(&f, &s, SnapshotKind::Pre, "a", 1);
    // Crash leftovers: a blob file without a row, plus a temporary file.
    let shard = f.snapshots.join("blobs/ab");
    fs::create_dir_all(&shard).unwrap();
    fs::write(shard.join("c".repeat(62)), b"junk").unwrap();
    fs::write(shard.join(".tmp-leftover"), b"junk").unwrap();

    let report = f.store.gc(u64::MAX, &[s.clone()].into()).unwrap();
    assert_eq!(report.orphan_files, 2);
    assert!(!shard.join("c".repeat(62)).exists());
    assert!(blob_file(&f, &e).exists());

    // Once the session is dropped its blob is unreferenced and goes regardless of the budget.
    assert_eq!(f.store.drop_session(&s).unwrap(), 1);
    let report = f.store.gc(u64::MAX, &HashSet::new()).unwrap();
    assert_eq!(report.unreferenced_blobs, 1);
    assert!(!blob_file(&f, &e).exists());
    assert_eq!(f.store.stored_bytes().unwrap(), 0);
}

#[test]
fn missing_and_corrupt_blobs_are_typed_errors() {
    let f = fixture();
    let s = ses(1);
    fs::write(f.work.join("a"), noise(1, 5000)).unwrap();
    fs::write(f.work.join("b"), noise(2, 5000)).unwrap();
    let a = capture_one(&f, &s, SnapshotKind::Pre, "a", 1);
    let b = capture_one(&f, &s, SnapshotKind::Pre, "b", 1);
    let dest = f.work.join("out");

    fs::remove_file(blob_file(&f, &a)).unwrap();
    assert!(matches!(
        f.store.restore(&a, &dest, t(2)),
        Err(SnapshotError::MissingBlob { .. })
    ));

    // Garbage that is not zstd.
    fs::write(blob_file(&f, &b), b"definitely not zstd").unwrap();
    assert!(matches!(
        f.store.restore(&b, &dest, t(2)),
        Err(SnapshotError::Corrupt { .. })
    ));
    // Valid zstd of the wrong content.
    let wrong = zstd::bulk::compress(b"something else", 1).unwrap();
    fs::write(blob_file(&f, &b), wrong).unwrap();
    let err = f.store.restore(&b, &dest, t(2)).unwrap_err();
    assert!(matches!(err, SnapshotError::Corrupt { .. }), "{err}");
    assert!(!dest.exists(), "a failed restore writes nothing");
}

#[test]
fn index_is_created_migrated_and_reopened() {
    let tmp = tempfile::tempdir().unwrap();
    let home = AcsHome::at(tmp.path().join("acs")).unwrap();
    let repo = home.repo(&RepoId::from_path(&tmp.path().join("repo")));
    repo.ensure().unwrap();
    let db = repo.snapshots_dir().join("index.db");
    assert!(!db.exists());
    {
        let store = SnapshotStore::open(&repo).unwrap();
        assert_eq!(store.stored_bytes().unwrap(), 0);
    }
    let version = |p: &Path| -> u32 {
        rusqlite::Connection::open(p)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(version(&db), 1);
    // Reopening is a no-op; a newer schema is refused rather than clobbered.
    SnapshotStore::open(&repo).unwrap();
    rusqlite::Connection::open(&db)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    assert!(matches!(
        SnapshotStore::open(&repo),
        Err(SnapshotError::UnsupportedSchema { found: 99, .. })
    ));
}

#[cfg(unix)]
#[test]
fn index_and_blobs_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    fs::write(f.work.join("a"), b"secret").unwrap();
    let e = capture_one(&f, &ses(1), SnapshotKind::Pre, "a", 1);
    for p in [f.snapshots.join("index.db"), blob_file(&f, &e)] {
        let mode = fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "{p:?}");
    }
}

#[test]
fn capturing_1000_small_files_is_fast() {
    let f = fixture();
    let s = ses(1);
    let mut names = Vec::new();
    for i in 0..1000u64 {
        let name = format!("f{i:04}.rs");
        let body = format!("fn f{i}() {{ /* {} */ }}\n", i % 37).repeat(20);
        fs::write(f.work.join(&name), body).unwrap();
        names.push(name);
    }
    let abs: Vec<PathBuf> = names.iter().map(|n| f.work.join(n)).collect();
    let batch: Vec<(RepoPath, &Path)> = names
        .iter()
        .zip(&abs)
        .map(|(n, a)| (rp(n), a.as_path()))
        .collect();

    let start = Instant::now();
    let results = f
        .store
        .capture(&s, SnapshotKind::Pre, &batch, t(1))
        .unwrap();
    let cold = start.elapsed();
    let start = Instant::now();
    f.store
        .capture(&ses(2), SnapshotKind::Pre, &batch, t(2))
        .unwrap();
    let warm = start.elapsed();
    assert_eq!(results.len(), 1000);
    println!("snapshot capture of 1000 files: cold {cold:?}, deduplicated {warm:?}");

    // The 200 ms budget applies to release builds; debug builds get generous slack. Windows CI
    // runners scan every new file (cold capture has taken 7 s there in debug), so they only get a
    // sanity bound; the release-build budget is enforced by the benchmarks (WP10.1).
    let limit_ms = match (cfg!(debug_assertions), cfg!(windows)) {
        (true, true) => 30_000,
        (true, false) => 5000,
        (false, _) => 200,
    };
    assert!(cold.as_millis() < limit_ms, "cold capture took {cold:?}");
    assert!(warm.as_millis() < limit_ms, "warm capture took {warm:?}");
}
