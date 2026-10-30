//! Pre- and post-images of the files an event changes, and the baselines that make shell-made
//! changes undoable.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use acs_core::gate::SnapshotRequest;
use acs_core::{ContentHash, RepoPath, SessionId, Timestamp};
use acs_git::repo::BlobSource;
use acs_store::{FileState, SnapshotKind};

use crate::runtime::RepoContext;
use crate::runtime::paths::Normalized;

/// Files larger than this are not snapshotted (and not hashed for attribution).
pub const MAX_SNAPSHOT_BYTES: u64 = 32 * 1024 * 1024;
/// The most files snapshotted for one event.
pub const MAX_SNAPSHOT_FILES: usize = 2_000;

/// An existing regular file we are willing to read, with its repository path.
fn candidate(ctx: &RepoContext, rel: &RepoPath) -> Option<(RepoPath, PathBuf)> {
    let abs = ctx.layout.work_tree.join(rel.as_str());
    let meta = fs::metadata(&abs).ok()?;
    (meta.is_file() && meta.len() <= MAX_SNAPSHOT_BYTES).then(|| (rel.clone(), abs))
}

/// Captures pre-images for the engine's snapshot requests. Missing files are skipped (decision
/// 0009 item 8); a recursive request is served from the expansion the pipeline made for that
/// directory. Returns warnings for what could not be done; a snapshot problem never blocks the
/// tool call.
#[must_use]
pub fn capture_pre(
    ctx: &RepoContext,
    session: &SessionId,
    requests: &[SnapshotRequest],
    norm: &Normalized,
    now: Timestamp,
) -> Vec<String> {
    let mut wanted: Vec<RepoPath> = Vec::new();
    let mut seen: HashSet<RepoPath> = HashSet::new();
    let mut add = |path: &RepoPath| {
        if seen.insert(path.clone()) {
            wanted.push(path.clone());
        }
    };
    for request in requests {
        if request.recursive {
            for target in &norm.targets {
                if target.recursive
                    && target.path.repo_path() == Some(&request.path)
                    && let Some(expansion) = &target.expansion
                {
                    expansion.files.iter().for_each(&mut add);
                }
            }
        } else {
            add(&request.path);
        }
    }
    let items: Vec<(RepoPath, PathBuf)> = wanted
        .iter()
        .filter_map(|rel| candidate(ctx, rel))
        .take(MAX_SNAPSHOT_FILES)
        .collect();
    if items.is_empty() {
        return Vec::new();
    }
    let store = match ctx.snapshots() {
        Ok(store) => store,
        Err(why) => return vec![format!("ACS could not take pre-images: {why}")],
    };
    let borrowed: Vec<(RepoPath, &std::path::Path)> = items
        .iter()
        .map(|(rel, abs)| (rel.clone(), abs.as_path()))
        .collect();
    match store.capture(session, SnapshotKind::Pre, &borrowed, now) {
        Ok(_) => Vec::new(),
        Err(err) => vec![format!("ACS could not take pre-images: {err}")],
    }
}

/// What observing a path after a tool call found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    /// The path.
    pub path: RepoPath,
    /// Hash before the change; `None` if the file did not exist (or the before-state is
    /// unknown, see [`record_baselines`]).
    pub pre_hash: Option<ContentHash>,
    /// Hash after the change; `None` if the file no longer exists.
    pub post_hash: Option<ContentHash>,
}

fn hash_of(state: &FileState) -> Option<ContentHash> {
    match state {
        FileState::Absent => None,
        FileState::File { hash, .. } => Some(hash.clone()),
    }
}

/// Captures post-images of `paths` and compares each with the session's first pre-image.
/// Unchanged files (post equals pre) are left out, as are missing files that had no pre-image.
/// Returns what changed.
#[must_use]
pub fn observe(
    ctx: &RepoContext,
    session: &SessionId,
    paths: &[RepoPath],
    now: Timestamp,
) -> Vec<Observed> {
    let Ok(store) = ctx.snapshots() else {
        return Vec::new();
    };
    // Existing files are captured; missing ones are recorded as absent. Files too big to read
    // are skipped entirely.
    let mut items: Vec<(RepoPath, PathBuf)> = Vec::new();
    for rel in paths.iter().take(MAX_SNAPSHOT_FILES) {
        let abs = ctx.layout.work_tree.join(rel.as_str());
        match fs::metadata(&abs) {
            Ok(meta) if meta.is_file() && meta.len() <= MAX_SNAPSHOT_BYTES => {
                items.push((rel.clone(), abs));
            }
            Ok(_) => {}
            Err(_) => items.push((rel.clone(), abs)),
        }
    }
    if items.is_empty() {
        return Vec::new();
    }
    let borrowed: Vec<(RepoPath, &std::path::Path)> = items
        .iter()
        .map(|(rel, abs)| (rel.clone(), abs.as_path()))
        .collect();
    // What the session last left each path as, to tell a repeated call that changed nothing from
    // one that did.
    let previous: Vec<Option<Option<ContentHash>>> = items
        .iter()
        .map(|(rel, _)| match store.post_image(session, rel) {
            Ok(Some(entry)) => Some(hash_of(&entry.state)),
            _ => None,
        })
        .collect();
    let Ok(results) = store.capture(session, SnapshotKind::Post, &borrowed, now) else {
        return Vec::new();
    };
    let mut changed = Vec::new();
    for (index, result) in results.into_iter().enumerate() {
        let post = hash_of(&result.entry.state);
        let path = result.entry.path;
        if previous[index].as_ref() == Some(&post) {
            continue; // Same as the session left it last time: nothing new to attribute.
        }
        match store.pre_image(session, &path) {
            Ok(Some(pre)) => {
                let pre_hash = hash_of(&pre.state);
                if pre_hash != post {
                    changed.push(Observed {
                        path,
                        pre_hash,
                        post_hash: post,
                    });
                }
            }
            Ok(None) if post.is_some() => changed.push(Observed {
                path,
                pre_hash: None,
                post_hash: post,
            }),
            _ => {}
        }
    }
    changed
}

/// The most files whose baseline is read from the index in one post-tool step.
const MAX_BASELINES: usize = 50;

/// For paths a shell command changed that the session has no pre-image of (the command was not
/// predictable), records the best before-state available so the change can be attributed and
/// undone:
///
/// - a tracked file: its **staged** content (`git cat-file`; the committed content in repositories
///   of more than 20,000 files), which is the work tree's before-state unless the file was
///   already modified (and unstaged) before the command, in which case the baseline is older
///   than the command;
/// - an untracked file created at or after `since`: absent;
/// - an untracked file that existed before: nothing is recorded (unknown), and its `pre_hash`
///   stays `None`.
///
/// Best effort: any failure leaves the path without a baseline.
pub fn record_baselines(
    ctx: &RepoContext,
    session: &SessionId,
    paths: &[RepoPath],
    since: std::time::SystemTime,
    now: Timestamp,
) {
    let Ok(store) = ctx.snapshots() else { return };
    let Some(repo) = &ctx.repo_home else { return };
    let missing: Vec<&RepoPath> = paths
        .iter()
        .filter(|p| matches!(store.pre_image(session, p), Ok(None)))
        .take(MAX_BASELINES)
        .collect();
    if missing.is_empty() {
        return;
    }
    let names: Vec<String> = missing.iter().map(|p| p.as_str().to_owned()).collect();
    // Small repositories read the staged content; for big ones git would read the whole index
    // (tens of milliseconds), so the committed content stands in.
    let source = match acs_git::repo::index_entry_count(&ctx.layout.git_dir) {
        Some(count) if count > acs_git::status::DEFAULT_FULL_SCAN_MAX_ENTRIES => BlobSource::Head,
        _ => BlobSource::Index,
    };
    let blobs =
        acs_git::repo::index_blobs(&ctx.layout.work_tree, &names, source).unwrap_or_default();
    let dir = repo.pending_dir().join("baseline");
    if acs_store::fsutil::create_private_dir_all(&dir).is_err() {
        return;
    }
    let absent = dir.join("absent");
    let mut temp_files: Vec<PathBuf> = Vec::new();
    let mut items: Vec<(RepoPath, PathBuf)> = Vec::new();
    for (index, rel) in missing.iter().enumerate() {
        match blobs.get(index).and_then(Option::as_ref) {
            Some(bytes) => {
                let file = dir.join(format!("{}-{index}", std::process::id()));
                if acs_store::fsutil::write_private_atomic(&file, bytes).is_ok() {
                    items.push(((*rel).clone(), file.clone()));
                    temp_files.push(file);
                }
            }
            None => {
                let abs = ctx.layout.work_tree.join(rel.as_str());
                let created_before =
                    fs::metadata(&abs)
                        .and_then(|m| m.created())
                        .is_ok_and(|born| {
                            born < since
                                .checked_sub(std::time::Duration::from_secs(2))
                                .unwrap_or(since)
                        });
                if !created_before {
                    items.push(((*rel).clone(), absent.clone()));
                }
            }
        }
    }
    let borrowed: Vec<(RepoPath, &std::path::Path)> = items
        .iter()
        .map(|(rel, abs)| (rel.clone(), abs.as_path()))
        .collect();
    let _ = store.capture(session, SnapshotKind::Pre, &borrowed, now);
    for file in temp_files {
        let _ = fs::remove_file(file);
    }
}
