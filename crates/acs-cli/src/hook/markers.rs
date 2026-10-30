//! Small files under `ACS_HOME/repos/<id>/pending/` that connect hook events to each other and to
//! git hooks.
//!
//! - **Tool markers** (`pending/tool/<seat>.json`): written by the pre-tool step of a shell,
//!   delete or move event, read and removed by the matching post-tool step. They carry the time
//!   the command started and the paths it was predicted to touch.
//! - **Commit markers** (`pending/commit/<key>.json`): written when an agent's shell command
//!   creates a git commit (SPEC section 7, decision 0011 section 3); consumed by the git hooks
//!   (WP2.4). `<key>` is the first 16 hex characters of the SHA-256 of the work tree path.
//! - **Push markers** (`pending/push/<key>.json`): the same, written when an agent's shell
//!   command runs `git push`; read and consumed by the `pre-push` git hook to tell an agent's
//!   push from a human's (decision 0015).
//! - **Scan baselines** (`pending/scan/<key>.json`): the tracked files found deleted by the last
//!   full working-tree scan, so the next scan can tell new deletions from old ones.
//! - **Unknown payloads** (`pending/unknown/`): raw payloads no adapter could parse, a bounded
//!   number of the newest.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use acs_core::{ContentHash, SeatId, Timestamp};
use acs_store::fsutil::{create_private_dir_all, write_private_atomic};
use serde::{Deserialize, Serialize};

use crate::runtime::RepoContext;

/// How long a commit marker stays valid.
pub const COMMIT_MARKER_MAX_AGE_SECS: i64 = 10 * 60;
/// How many raw unknown payloads are kept.
pub const MAX_UNKNOWN_PAYLOADS: usize = 20;
/// The most bytes of an unknown payload that are kept.
pub const MAX_UNKNOWN_BYTES: usize = 64 * 1024;

/// What a pre-tool step tells its post-tool step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMarker {
    /// When the tool call started (microseconds since the Unix epoch).
    pub t0_micros: i64,
    /// Repository paths the call was predicted to write or delete.
    pub paths: Vec<String>,
    /// Directories to list in a scoped scan.
    pub scope_dirs: Vec<String>,
}

/// The pending-commit marker of SPEC section 7.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitMarker {
    /// The `ses_` id of the session that is committing.
    pub session_id: String,
    /// Its seat.
    pub seat_id: String,
    /// Its harness.
    pub harness: String,
    /// When the marker was written.
    pub created_at: Timestamp,
}

/// The marker key of a work tree: the first 16 hex characters of the SHA-256 of its canonical
/// path.
#[must_use]
pub fn worktree_key(worktree: &Path) -> String {
    ContentHash::of(worktree.to_string_lossy().as_bytes()).as_str()[..16].to_owned()
}

fn pending_file(ctx: &RepoContext, sub: &str, name: &str) -> Option<PathBuf> {
    Some(ctx.repo_home.as_ref()?.pending_dir().join(sub).join(name))
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    write_private_atomic(path, &bytes)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Writes the tool marker of `seat`, replacing an earlier one.
///
/// # Errors
///
/// The I/O error; callers ignore it (the post-tool step then falls back to the seat's last
/// activity time).
pub fn write_tool_marker(ctx: &RepoContext, seat: &SeatId, marker: &ToolMarker) -> io::Result<()> {
    match pending_file(ctx, "tool", &format!("{seat}.json")) {
        Some(path) => write_json(&path, marker),
        None => Ok(()),
    }
}

/// Reads and removes the tool marker of `seat`.
#[must_use]
pub fn take_tool_marker(ctx: &RepoContext, seat: &SeatId) -> Option<ToolMarker> {
    let path = pending_file(ctx, "tool", &format!("{seat}.json"))?;
    let marker = read_json(&path);
    let _ = fs::remove_file(&path);
    marker
}

/// Removes the tool marker of `seat` (session end).
pub fn clear_tool_marker(ctx: &RepoContext, seat: &SeatId) {
    if let Some(path) = pending_file(ctx, "tool", &format!("{seat}.json")) {
        let _ = fs::remove_file(path);
    }
}

/// Writes the pending-commit marker of this work tree (last writer wins).
///
/// # Errors
///
/// The I/O error; callers ignore it (the commit then goes without a trailer from the marker).
pub fn write_commit_marker(ctx: &RepoContext, marker: &CommitMarker) -> io::Result<()> {
    let key = worktree_key(&ctx.layout.work_tree);
    match pending_file(ctx, "commit", &format!("{key}.json")) {
        Some(path) => write_json(&path, marker),
        None => Ok(()),
    }
}

/// Writes the pending-push marker of this work tree (last writer wins). Same content as the
/// commit marker, kept apart so a commit does not consume a push and the other way round.
///
/// # Errors
///
/// The I/O error; callers ignore it (the push is then treated as a human's unless the harness
/// environment says otherwise).
pub fn write_push_marker(ctx: &RepoContext, marker: &CommitMarker) -> io::Result<()> {
    let key = worktree_key(&ctx.layout.work_tree);
    match pending_file(ctx, "push", &format!("{key}.json")) {
        Some(path) => write_json(&path, marker),
        None => Ok(()),
    }
}

fn marker_path(pending_dir: &Path, sub: &str, worktree: &Path) -> PathBuf {
    pending_dir
        .join(sub)
        .join(format!("{}.json", worktree_key(worktree)))
}

fn read_fresh(path: &Path, now: Timestamp) -> Option<CommitMarker> {
    let marker: CommitMarker = read_json(path)?;
    let age = now
        .as_second()
        .saturating_sub(marker.created_at.as_second());
    (age <= COMMIT_MARKER_MAX_AGE_SECS).then_some(marker)
}

/// Reads the pending-commit marker for `worktree` from `pending_dir`, ignoring one older than
/// ten minutes. For the git hooks (WP2.4): `pre-commit` reads it, `prepare-commit-msg` consumes
/// it with [`take_commit_marker`].
#[must_use]
pub fn read_commit_marker(
    pending_dir: &Path,
    worktree: &Path,
    now: Timestamp,
) -> Option<CommitMarker> {
    read_fresh(&marker_path(pending_dir, "commit", worktree), now)
}

/// Like [`read_commit_marker`], and removes the marker (a stale one is removed too).
#[must_use]
pub fn take_commit_marker(
    pending_dir: &Path,
    worktree: &Path,
    now: Timestamp,
) -> Option<CommitMarker> {
    let path = marker_path(pending_dir, "commit", worktree);
    let marker = read_fresh(&path, now);
    let _ = fs::remove_file(path);
    marker
}

/// Reads the pending-push marker for `worktree`, ignoring one older than ten minutes.
#[must_use]
pub fn read_push_marker(
    pending_dir: &Path,
    worktree: &Path,
    now: Timestamp,
) -> Option<CommitMarker> {
    read_fresh(&marker_path(pending_dir, "push", worktree), now)
}

/// Like [`read_push_marker`], and removes the marker.
#[must_use]
pub fn take_push_marker(
    pending_dir: &Path,
    worktree: &Path,
    now: Timestamp,
) -> Option<CommitMarker> {
    let path = marker_path(pending_dir, "push", worktree);
    let marker = read_fresh(&path, now);
    let _ = fs::remove_file(path);
    marker
}

/// The tracked files the previous full scan found missing.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ScanBaseline {
    /// Repository paths.
    pub deleted: Vec<String>,
}

/// The scan baseline of this work tree, if one was stored.
#[must_use]
pub fn read_scan_baseline(ctx: &RepoContext) -> Option<ScanBaseline> {
    let key = worktree_key(&ctx.layout.work_tree);
    read_json(&pending_file(ctx, "scan", &format!("{key}.json"))?)
}

/// Stores the scan baseline of this work tree.
pub fn write_scan_baseline(ctx: &RepoContext, baseline: &ScanBaseline) {
    let key = worktree_key(&ctx.layout.work_tree);
    if let Some(path) = pending_file(ctx, "scan", &format!("{key}.json")) {
        let _ = write_json(&path, baseline);
    }
}

/// Saves a raw payload no adapter could parse, keeping only the newest
/// [`MAX_UNKNOWN_PAYLOADS`].
pub fn save_unknown_payload(ctx: &RepoContext, now: Timestamp, payload: &[u8]) {
    let Some(repo) = &ctx.repo_home else { return };
    let dir = repo.pending_dir().join("unknown");
    if create_private_dir_all(&dir).is_err() {
        return;
    }
    let name = format!(
        "{:020}-{}.json",
        now.as_microsecond().max(0),
        std::process::id()
    );
    let cut = &payload[..payload.len().min(MAX_UNKNOWN_BYTES)];
    let _ = write_private_atomic(&dir.join(name), cut);
    // Names sort by time, so the oldest are the first to go.
    if let Ok(entries) = fs::read_dir(&dir) {
        let mut names: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        names.sort();
        let excess = names.len().saturating_sub(MAX_UNKNOWN_PAYLOADS);
        for old in names.into_iter().take(excess) {
            let _ = fs::remove_file(old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_key_is_sixteen_hex_characters_and_stable() {
        let a = worktree_key(Path::new("/work/repo"));
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(a, worktree_key(Path::new("/work/repo")));
        assert_ne!(a, worktree_key(Path::new("/work/other")));
    }

    #[test]
    fn commit_markers_expire_after_ten_minutes() {
        let dir = tempfile::tempdir().unwrap();
        let work = Path::new("/work/repo");
        let created: Timestamp = "2026-10-30T12:00:00Z".parse().unwrap();
        let marker = CommitMarker {
            session_id: "ses_abc".into(),
            seat_id: "seat_abc".into(),
            harness: "claude-code".into(),
            created_at: created,
        };
        let path = dir
            .path()
            .join("commit")
            .join(format!("{}.json", worktree_key(work)));
        write_json(&path, &marker).unwrap();
        let soon: Timestamp = "2026-10-30T12:09:59Z".parse().unwrap();
        let late: Timestamp = "2026-10-30T12:10:01Z".parse().unwrap();
        assert_eq!(read_commit_marker(dir.path(), work, soon), Some(marker));
        assert_eq!(read_commit_marker(dir.path(), work, late), None);
        assert_eq!(read_commit_marker(dir.path(), Path::new("/x"), soon), None);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"session_id\"") && text.contains("\"created_at\""));
    }

    #[test]
    fn taking_a_marker_removes_it_and_push_markers_are_separate() {
        let dir = tempfile::tempdir().unwrap();
        let work = Path::new("/work/repo");
        let now: Timestamp = "2026-10-30T12:00:00Z".parse().unwrap();
        let marker = CommitMarker {
            session_id: "ses_abc".into(),
            seat_id: "seat_abc".into(),
            harness: "claude-code".into(),
            created_at: now,
        };
        let key = worktree_key(work);
        write_json(
            &dir.path().join("commit").join(format!("{key}.json")),
            &marker,
        )
        .unwrap();
        assert_eq!(read_push_marker(dir.path(), work, now), None);
        assert_eq!(
            take_commit_marker(dir.path(), work, now),
            Some(marker.clone())
        );
        assert_eq!(read_commit_marker(dir.path(), work, now), None);
        write_json(
            &dir.path().join("push").join(format!("{key}.json")),
            &marker,
        )
        .unwrap();
        assert_eq!(read_commit_marker(dir.path(), work, now), None);
        assert_eq!(take_push_marker(dir.path(), work, now), Some(marker));
        assert_eq!(take_push_marker(dir.path(), work, now), None);
    }
}
