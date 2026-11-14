//! Tests for the undo planner: every action it can choose, and applying a plan.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use acs_core::{ContentHash, RepoPath, SeatId, SessionId, Timestamp};
use acs_store::repo_id::RepoId;
use acs_store::undo::{
    Effect, MergeResult, Merger, PlanInputs, UndoAction, UndoPlan, applied_paths, apply, plan,
};
use acs_store::{
    AcsHome, AttributionSource, NewSeat, SnapshotKind, SnapshotStore, StateDb, StoredAttribution,
};

struct Fixture {
    _tmp: tempfile::TempDir,
    work: PathBuf,
    store: SnapshotStore,
    session: SessionId,
    next_attribution: i64,
    attributions: Vec<StoredAttribution>,
}

fn rp(s: &str) -> RepoPath {
    s.parse().unwrap()
}

fn t(secs: i64) -> Timestamp {
    Timestamp::from_second(secs).unwrap()
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = AcsHome::at(tmp.path().join("acs")).unwrap();
    let repo = home.repo(&RepoId::from_path(&tmp.path().join("repo")));
    repo.ensure().unwrap();
    let work = tmp.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let store = SnapshotStore::open(&repo).unwrap();
    Fixture {
        _tmp: tmp,
        work,
        store,
        session: "ses_undo".parse().unwrap(),
        next_attribution: 1,
        attributions: Vec::new(),
    }
}

impl Fixture {
    fn abs(&self, path: &str) -> PathBuf {
        self.work.join(path)
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        let abs = self.abs(path);
        fs::create_dir_all(abs.parent().unwrap()).unwrap();
        fs::write(abs, bytes).unwrap();
    }

    fn read(&self, path: &str) -> Vec<u8> {
        fs::read(self.abs(path)).unwrap()
    }

    fn capture(&self, kind: SnapshotKind, path: &str) {
        let abs = self.abs(path);
        self.store
            .capture(&self.session, kind, &[(rp(path), abs.as_path())], t(10))
            .unwrap();
    }

    /// What the hook does around a file tool: a pre-image if the file exists, the change, a
    /// post-image, and the attribution of the change.
    fn tool_write(&mut self, path: &str, bytes: Option<&[u8]>) {
        let pre = fs::read(self.abs(path)).ok();
        if pre.is_some() {
            self.capture(SnapshotKind::Pre, path);
        }
        match bytes {
            Some(bytes) => self.write(path, bytes),
            None => fs::remove_file(self.abs(path)).unwrap(),
        }
        self.capture(SnapshotKind::Post, path);
        self.attribute(path, pre.as_deref(), bytes, AttributionSource::Tool);
    }

    fn attribute(
        &mut self,
        path: &str,
        pre: Option<&[u8]>,
        post: Option<&[u8]>,
        via: AttributionSource,
    ) {
        self.attributions.push(StoredAttribution {
            id: self.next_attribution,
            seat_id: "seat_undo".parse::<SeatId>().unwrap(),
            path: rp(path),
            pre_hash: pre.map(ContentHash::of),
            post_hash: post.map(ContentHash::of),
            at: t(10),
            via,
        });
        self.next_attribution += 1;
    }

    fn plan_with(
        &self,
        holder: &dyn Fn(&RepoPath) -> Option<String>,
        merge: &mut Merger<'_>,
    ) -> UndoPlan {
        plan(
            &PlanInputs {
                session: &self.session,
                work_tree: &self.work,
                snapshots: &self.store,
                attributions: &self.attributions,
                holder,
            },
            merge,
        )
        .unwrap()
    }

    fn plan(&self) -> UndoPlan {
        self.plan_with(&|_| None, &mut |_, _, _| {
            MergeResult::Conflict("no merge".to_owned())
        })
    }
}

fn action(plan: &UndoPlan, path: &str) -> UndoAction {
    plan.items
        .iter()
        .find(|i| i.path.as_str() == path)
        .unwrap_or_else(|| panic!("{path} is not in the plan"))
        .action
}

#[test]
fn a_file_the_session_changed_is_restored_byte_exact() {
    let mut f = fixture();
    f.write("a.txt", b"old\r\n\xff\x00bytes");
    f.tool_write("a.txt", Some(b"new"));
    let p = f.plan();
    assert_eq!(action(&p, "a.txt"), UndoAction::Restore);
    let applied = apply(&f.store, &f.work, &p);
    assert!(applied.iter().all(|a| a.result.is_ok()), "{applied:?}");
    assert_eq!(f.read("a.txt"), b"old\r\n\xff\x00bytes");
    // Planning again: now it is as it was before the session.
    assert_eq!(action(&f.plan(), "a.txt"), UndoAction::AlreadyUndone);
}

#[cfg(unix)]
#[test]
fn restoring_brings_the_file_mode_back() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = fixture();
    f.write("run.sh", b"#!/bin/sh\n");
    fs::set_permissions(f.abs("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    f.tool_write("run.sh", Some(b"#!/bin/sh\necho\n"));
    fs::set_permissions(f.abs("run.sh"), fs::Permissions::from_mode(0o600)).unwrap();
    f.capture(SnapshotKind::Post, "run.sh");
    let p = f.plan();
    assert_eq!(action(&p, "run.sh"), UndoAction::Restore);
    let _ = apply(&f.store, &f.work, &p);
    let mode = fs::metadata(f.abs("run.sh")).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o755);
}

#[test]
fn a_file_the_session_created_is_deleted() {
    let mut f = fixture();
    f.tool_write("new.txt", Some(b"made by the session"));
    let p = f.plan();
    assert_eq!(action(&p, "new.txt"), UndoAction::Delete);
    let _ = apply(&f.store, &f.work, &p);
    assert!(!f.abs("new.txt").exists());
}

#[test]
fn a_file_created_and_edited_again_is_still_a_creation() {
    // The second edit takes a pre-image of the file as the first edit left it; the first
    // attribution says the file was new, so undo must delete it, not restore the intermediate.
    let mut f = fixture();
    f.tool_write("new.txt", Some(b"v1"));
    f.tool_write("new.txt", Some(b"v2"));
    let p = f.plan();
    assert_eq!(action(&p, "new.txt"), UndoAction::Delete);
    let _ = apply(&f.store, &f.work, &p);
    assert!(!f.abs("new.txt").exists());
}

#[test]
fn a_file_the_session_deleted_is_recreated_even_when_empty() {
    let mut f = fixture();
    f.write("gone.txt", b"content\n");
    f.write("empty.txt", b"");
    f.tool_write("gone.txt", None);
    f.tool_write("empty.txt", None);
    let p = f.plan();
    assert_eq!(action(&p, "gone.txt"), UndoAction::Recreate);
    assert_eq!(action(&p, "empty.txt"), UndoAction::Recreate);
    let _ = apply(&f.store, &f.work, &p);
    assert_eq!(f.read("gone.txt"), b"content\n");
    assert_eq!(f.read("empty.txt"), b"");
}

#[test]
fn a_file_the_session_left_as_it_found_it_is_already_undone() {
    let mut f = fixture();
    f.write("same.txt", b"x");
    f.tool_write("same.txt", Some(b"y"));
    f.tool_write("same.txt", Some(b"x"));
    let p = f.plan();
    assert_eq!(action(&p, "same.txt"), UndoAction::AlreadyUndone);
    assert_eq!(p.items[0].effect, Effect::Nothing);
}

#[test]
fn later_changes_by_others_are_merged_with_the_reverse_change() {
    let mut f = fixture();
    f.write("m.txt", b"base\n");
    f.tool_write("m.txt", Some(b"base\nsession\n"));
    f.write("m.txt", b"others\nbase\nsession\n");
    let mut seen: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> = Vec::new();
    let p = f.plan_with(&|_| None, &mut |current, post, pre| {
        seen.push((current.to_vec(), post.to_vec(), pre.to_vec()));
        MergeResult::Clean(b"others\nbase\n".to_vec())
    });
    // current, then the session's post, then its pre.
    assert_eq!(
        seen,
        vec![(
            b"others\nbase\nsession\n".to_vec(),
            b"base\nsession\n".to_vec(),
            b"base\n".to_vec()
        )]
    );
    assert_eq!(action(&p, "m.txt"), UndoAction::Merge);
    // Planning wrote nothing.
    assert_eq!(f.read("m.txt"), b"others\nbase\nsession\n");
    let _ = apply(&f.store, &f.work, &p);
    assert_eq!(f.read("m.txt"), b"others\nbase\n");
}

#[test]
fn overlapping_changes_are_a_conflict_and_the_file_is_not_touched() {
    let mut f = fixture();
    f.write("c.txt", b"one\n");
    f.tool_write("c.txt", Some(b"two\n"));
    f.write("c.txt", b"three\n");
    let p = f.plan_with(&|_| None, &mut |_, _, _| {
        MergeResult::Conflict("overlap".to_owned())
    });
    assert_eq!(action(&p, "c.txt"), UndoAction::Conflict);
    assert_eq!(p.items[0].detail.as_deref(), Some("overlap"));
    assert!(apply(&f.store, &f.work, &p).is_empty());
    assert_eq!(f.read("c.txt"), b"three\n");
}

#[test]
fn a_file_deleted_by_others_afterwards_is_a_conflict() {
    let mut f = fixture();
    f.write("d.txt", b"one\n");
    f.tool_write("d.txt", Some(b"two\n"));
    fs::remove_file(f.abs("d.txt")).unwrap();
    assert_eq!(action(&f.plan(), "d.txt"), UndoAction::Conflict);
    assert!(!f.abs("d.txt").exists());
}

#[test]
fn a_directory_in_the_way_is_a_conflict() {
    let mut f = fixture();
    f.tool_write("x", Some(b"file"));
    fs::remove_file(f.abs("x")).unwrap();
    fs::create_dir(f.abs("x")).unwrap();
    assert_eq!(action(&f.plan(), "x"), UndoAction::Conflict);
}

#[test]
fn a_file_without_a_pre_image_is_unknown_and_never_touched() {
    let mut f = fixture();
    // A shell command changed an untracked file that existed before: no baseline was recorded.
    f.write("sh.txt", b"before");
    fs::write(f.abs("sh.txt"), b"after").unwrap();
    f.capture(SnapshotKind::Post, "sh.txt");
    f.attribute("sh.txt", None, Some(b"after"), AttributionSource::Shell);
    // And a path with an attribution but no snapshot at all.
    f.attribute("ghost.txt", None, Some(b"?"), AttributionSource::Shell);
    f.write("ghost.txt", b"?");
    let p = f.plan();
    assert_eq!(action(&p, "sh.txt"), UndoAction::Unknown);
    assert_eq!(action(&p, "ghost.txt"), UndoAction::Unknown);
    assert!(apply(&f.store, &f.work, &p).is_empty());
    assert_eq!(f.read("sh.txt"), b"after");
}

#[test]
fn a_shell_created_file_with_a_recorded_baseline_is_deleted() {
    let mut f = fixture();
    // `record_baselines` stores an absent pre-image for a file the command created.
    f.capture(SnapshotKind::Pre, "made.txt");
    f.write("made.txt", b"by shell");
    f.capture(SnapshotKind::Post, "made.txt");
    f.attribute(
        "made.txt",
        None,
        Some(b"by shell"),
        AttributionSource::Shell,
    );
    let p = f.plan();
    assert_eq!(action(&p, "made.txt"), UndoAction::Delete);
}

#[test]
fn a_file_held_by_another_seat_is_reported_and_left_alone() {
    let mut f = fixture();
    f.write("h.txt", b"old");
    f.tool_write("h.txt", Some(b"new"));
    let p = f.plan_with(
        &|path| (path.as_str() == "h.txt").then(|| "claimed by claude-code sara".to_owned()),
        &mut |_, _, _| MergeResult::Conflict(String::new()),
    );
    assert_eq!(action(&p, "h.txt"), UndoAction::Held);
    assert_eq!(
        p.items[0].detail.as_deref(),
        Some("claimed by claude-code sara")
    );
    assert!(apply(&f.store, &f.work, &p).is_empty());
    assert_eq!(f.read("h.txt"), b"new");
}

#[test]
fn an_evicted_pre_image_makes_the_file_unknown() {
    let mut f = fixture();
    f.write("e.txt", b"old content");
    f.tool_write("e.txt", Some(b"new content"));
    f.store.gc(0, &HashSet::new()).unwrap();
    let p = f.plan();
    assert_eq!(action(&p, "e.txt"), UndoAction::Unknown);
    assert!(
        p.items[0]
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("removed to save space")),
        "{:?}",
        p.items[0].detail
    );
    assert_eq!(f.read("e.txt"), b"new content");
}

#[test]
fn applying_refuses_to_overwrite_a_file_that_changed_after_planning() {
    let mut f = fixture();
    f.write("r.txt", b"old");
    f.tool_write("r.txt", Some(b"new"));
    let p = f.plan();
    f.write("r.txt", b"someone wrote this meanwhile");
    let applied = apply(&f.store, &f.work, &p);
    assert!(applied[0].result.is_err());
    assert!(applied_paths(&applied).is_empty());
    assert_eq!(f.read("r.txt"), b"someone wrote this meanwhile");
}

#[test]
fn items_are_sorted_by_path() {
    let mut f = fixture();
    f.tool_write("b.txt", Some(b"b"));
    f.tool_write("a/z.txt", Some(b"z"));
    f.tool_write("a.txt", Some(b"a"));
    let plan = f.plan();
    let names: Vec<&str> = plan.items.iter().map(|i| i.path.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
}

#[test]
fn seats_are_found_by_session_id_or_unique_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let home = AcsHome::at(tmp.path().join("acs")).unwrap();
    let repo = home.repo(&RepoId::from_path(&tmp.path().join("repo")));
    let db = StateDb::open(&repo).unwrap();
    let new = |session: &'static str| NewSeat {
        harness: "claude-code",
        harness_session_id: session,
        pilot: "sara@example.com",
        worktree: "/w",
        branch: Some("main"),
        target_branch: "main",
    };
    let (a, _) = db.get_or_create_seat_info(&new("one"), t(1)).unwrap();
    let (b, _) = db.get_or_create_seat_info(&new("two"), t(2)).unwrap();
    let full = a.session_id.to_string();
    let found = db.seats_by_session_prefix(&full).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].session_id, a.session_id);
    let prefix = &b.session_id.as_str()[..b.session_id.as_str().len() - 3];
    let found = db.seats_by_session_prefix(prefix).unwrap();
    assert!(found.iter().any(|s| s.session_id == b.session_id));
    assert!(
        db.seats_by_session_prefix("ses_nothing")
            .unwrap()
            .is_empty()
    );
    // `ses_` matches both: the caller reports the ambiguity.
    assert_eq!(db.seats_by_session_prefix("ses_").unwrap().len(), 2);
}
