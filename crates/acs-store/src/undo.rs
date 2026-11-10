//! Planning `acs undo` (WP4.3): what a session changed, from its snapshots, and what restoring
//! each file would take.
//!
//! For every file the session changed, [`plan`] compares three states: the one before the
//! session's first change (its first pre-image), the one after its last change (its last
//! post-image), and the one in the work tree now. The rules, in order:
//!
//! 1. a file claimed by another live seat is not touched ([`UndoAction::Held`]);
//! 2. no usable pre-image: not touched ([`UndoAction::Unknown`]);
//! 3. the session left the file as it found it: [`UndoAction::AlreadyUndone`];
//! 4. the file is as the session left it: the pre-image comes back byte for byte with its mode
//!    ([`UndoAction::Restore`]); a file the session created is deleted ([`UndoAction::Delete`]);
//!    a file it deleted is recreated ([`UndoAction::Recreate`]);
//! 5. the file is as it was before the session: [`UndoAction::AlreadyUndone`];
//! 6. otherwise someone changed it after the session: the reverse of the session's change is
//!    merged into the current content ([`UndoAction::Merge`]), or reported as a conflict and left
//!    alone ([`UndoAction::Conflict`]).
//!
//! The planner reads the work tree but never writes it. [`apply`] carries a plan out.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use acs_core::{ContentHash, RepoPath, SessionId};
use rusqlite::params;
use serde::Serialize;

use crate::attributions::{AttributionSource, StoredAttribution};
use crate::db::{StateDb, StoreError, id_from_db};
use crate::seats::SeatInfo;
use crate::snapshots::{
    FileState, SnapshotEntry, SnapshotError, SnapshotKind, SnapshotStore, read_file,
};

/// What undoing one file takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UndoAction {
    /// Put the pre-image back.
    Restore,
    /// Delete the file: the session created it.
    Delete,
    /// Recreate the file: the session deleted it.
    Recreate,
    /// Apply the reverse of the session's change to content that changed afterwards.
    Merge,
    /// Nothing to do: the file is already as it was before the session.
    AlreadyUndone,
    /// The reverse change overlaps later changes; the file is left alone.
    Conflict,
    /// There is no way to tell what the file was; it is left alone.
    Unknown,
    /// Another live seat holds the file; it is left alone.
    Held,
}

impl UndoAction {
    /// The word used in output and JSON.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Restore => "restore",
            Self::Delete => "delete",
            Self::Recreate => "recreate",
            Self::Merge => "merge",
            Self::AlreadyUndone => "already undone",
            Self::Conflict => "conflict",
            Self::Unknown => "unknown",
            Self::Held => "held by another seat",
        }
    }

    /// Whether applying the plan changes the file.
    #[must_use]
    pub fn changes_file(self) -> bool {
        matches!(
            self,
            Self::Restore | Self::Delete | Self::Recreate | Self::Merge
        )
    }
}

/// What applying an item writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Nothing is written.
    Nothing,
    /// The file is deleted.
    Delete,
    /// The blob `hash` is written with `mode`.
    Restore {
        /// Content to write (read from the snapshot store when applying).
        hash: ContentHash,
        /// Permission bits.
        mode: u32,
    },
    /// Merged content is written with `mode`.
    Write {
        /// The merged bytes.
        bytes: Vec<u8>,
        /// Permission bits (those of the file now).
        mode: u32,
    },
}

/// The plan for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoItem {
    /// The file.
    pub path: RepoPath,
    /// What happens to it.
    pub action: UndoAction,
    /// Why, when it is not obvious (the holder, the reason a file is unknown, ...).
    pub detail: Option<String>,
    /// What applying writes.
    pub effect: Effect,
    /// The content hash the work-tree file had when planning (`None`: it did not exist). Applying
    /// refuses to write over a file that changed since.
    pub current: Option<ContentHash>,
}

/// The plan for a whole session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoPlan {
    /// The session being undone.
    pub session: SessionId,
    /// One item per changed file, sorted by path.
    pub items: Vec<UndoItem>,
}

impl UndoPlan {
    /// How many items have `action`.
    #[must_use]
    pub fn count(&self, action: UndoAction) -> usize {
        self.items.iter().filter(|i| i.action == action).count()
    }
}

/// The outcome of a merge the planner asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeResult {
    /// Merged cleanly.
    Clean(Vec<u8>),
    /// The changes overlap, or merging failed; the text says why.
    Conflict(String),
}

/// Merges `(current, post, pre)`: `current` with the change from `post` to `pre` applied. The
/// CLI wires this to `git merge-file`.
pub type Merger<'a> = dyn FnMut(&[u8], &[u8], &[u8]) -> MergeResult + 'a;

/// Everything [`plan`] reads.
pub struct PlanInputs<'a> {
    /// The session to undo.
    pub session: &'a SessionId,
    /// The work tree root.
    pub work_tree: &'a Path,
    /// The snapshot store.
    pub snapshots: &'a SnapshotStore,
    /// The attributions of the session's seat, oldest first.
    pub attributions: &'a [StoredAttribution],
    /// Who holds a path right now, if a live seat other than the session's does.
    pub holder: &'a dyn Fn(&RepoPath) -> Option<String>,
}

/// Errors of planning and applying.
#[derive(Debug, thiserror::Error)]
pub enum UndoError {
    /// The state database failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The snapshot store failed.
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
}

impl StateDb {
    /// The seats whose `ses_` session id is `session` or starts with it (a unique prefix is
    /// accepted by `acs undo`). At most a handful: the caller says when there are several.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn seats_by_session_prefix(&self, session: &str) -> Result<Vec<SeatInfo>, StoreError> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT seat_id FROM seats WHERE substr(session_id, 1, length(?1)) = ?1 \
                 ORDER BY (session_id = ?1) DESC, started_at",
            )?;
            stmt.query_map(params![session], |row| row.get(0))?
                .collect::<Result<_, _>>()?
        };
        let mut seats = Vec::new();
        for id in ids {
            if let Some(info) = self.seat_info(&id_from_db(id)?)? {
                seats.push(info);
            }
        }
        // An exact match wins over longer ids it is a prefix of.
        if seats.iter().any(|s| s.session_id.as_str() == session) {
            seats.retain(|s| s.session_id.as_str() == session);
        }
        Ok(seats)
    }
}

/// The key of a state for comparisons: its content hash, `None` for an absent file.
fn key(state: &FileState) -> Option<ContentHash> {
    match state {
        FileState::Absent => None,
        FileState::File { hash, .. } => Some(hash.clone()),
    }
}

/// The state of the session's first change's "before": the first pre-image, unless the session
/// created the file (its first post-image came before any pre-image and the file tool recorded no
/// before-hash), in which case the file was absent.
fn before_state(
    entries: &[&SnapshotEntry],
    attributions: &[&StoredAttribution],
) -> Option<FileState> {
    let first = |kind: SnapshotKind| {
        entries
            .iter()
            .filter(|e| e.kind == kind)
            .min_by_key(|e| e.seq)
    };
    let pre = first(SnapshotKind::Pre);
    let post = first(SnapshotKind::Post);
    if let Some(pre) = pre
        && post.is_none_or(|post| pre.seq < post.seq)
    {
        return Some(pre.state.clone());
    }
    // A file tool's pre-tool step takes no image of a file that does not exist, so a tool-made
    // first change with no earlier before-hash created the file.
    let first_attribution = attributions.iter().min_by_key(|a| a.id)?;
    (first_attribution.via == AttributionSource::Tool && first_attribution.pre_hash.is_none())
        .then_some(FileState::Absent)
}

/// The state after the session's last change: the last post-image, or the hash the last
/// attribution recorded.
fn after_state(
    entries: &[&SnapshotEntry],
    attributions: &[&StoredAttribution],
) -> Option<Option<ContentHash>> {
    if let Some(entry) = entries
        .iter()
        .filter(|e| e.kind == SnapshotKind::Post)
        .max_by_key(|e| e.seq)
    {
        return Some(key(&entry.state));
    }
    attributions
        .iter()
        .max_by_key(|a| a.id)
        .map(|a| a.post_hash.clone())
}

fn blob_of(store: &SnapshotStore, key: Option<&ContentHash>) -> Result<Vec<u8>, SnapshotError> {
    key.map_or_else(|| Ok(Vec::new()), |hash| store.read_blob(hash))
}

fn item(
    path: &RepoPath,
    action: UndoAction,
    detail: Option<String>,
    effect: Effect,
    current: Option<ContentHash>,
) -> UndoItem {
    UndoItem {
        path: path.clone(),
        action,
        detail,
        effect,
        current,
    }
}

/// Plans the undo of `inputs.session`.
///
/// # Errors
///
/// [`UndoError`] when the snapshot index cannot be read. A missing or damaged blob is not an
/// error: it makes that file [`UndoAction::Unknown`].
pub fn plan(inputs: &PlanInputs<'_>, merge: &mut Merger<'_>) -> Result<UndoPlan, UndoError> {
    let entries = inputs.snapshots.list(inputs.session)?;
    let mut by_path: BTreeMap<RepoPath, (Vec<&SnapshotEntry>, Vec<&StoredAttribution>)> =
        BTreeMap::new();
    for entry in &entries {
        by_path.entry(entry.path.clone()).or_default().0.push(entry);
    }
    for attribution in inputs.attributions {
        by_path
            .entry(attribution.path.clone())
            .or_default()
            .1
            .push(attribution);
    }
    let mut items = Vec::with_capacity(by_path.len());
    for (path, (entries, attributions)) in &by_path {
        items.push(plan_path(inputs, path, entries, attributions, merge)?);
    }
    Ok(UndoPlan {
        session: inputs.session.clone(),
        items,
    })
}

fn plan_path(
    inputs: &PlanInputs<'_>,
    path: &RepoPath,
    entries: &[&SnapshotEntry],
    attributions: &[&StoredAttribution],
    merge: &mut Merger<'_>,
) -> Result<UndoItem, UndoError> {
    let store = inputs.snapshots;
    let abs = inputs.work_tree.join(path.as_str());
    let current = match read_file(&abs) {
        Ok(Some((bytes, mode))) => Some((bytes, mode)),
        Ok(None) => None,
        Err(SnapshotError::IsDirectory(_)) => {
            return Ok(item(
                path,
                UndoAction::Conflict,
                Some("a directory is there now".to_owned()),
                Effect::Nothing,
                None,
            ));
        }
        Err(err) => {
            return Ok(item(
                path,
                UndoAction::Unknown,
                Some(format!("cannot read the file: {err}")),
                Effect::Nothing,
                None,
            ));
        }
    };
    let current_key = current.as_ref().map(|(bytes, _)| ContentHash::of(bytes));

    if let Some(holder) = (inputs.holder)(path) {
        return Ok(item(
            path,
            UndoAction::Held,
            Some(holder),
            Effect::Nothing,
            current_key,
        ));
    }
    let unknown = |why: &str, current_key: Option<ContentHash>| {
        item(
            path,
            UndoAction::Unknown,
            Some(why.to_owned()),
            Effect::Nothing,
            current_key,
        )
    };

    let Some(pre) = before_state(entries, attributions) else {
        return Ok(unknown(
            "no pre-image: ACS could not tell what the file was before the session",
            current_key,
        ));
    };
    let pre_key = key(&pre);
    let Some(post_key) = after_state(entries, attributions) else {
        // Pre-images only: the tool may not have run, or its post-image was lost.
        return Ok(if current_key == pre_key {
            item(
                path,
                UndoAction::AlreadyUndone,
                None,
                Effect::Nothing,
                current_key,
            )
        } else {
            unknown(
                "no post-image: ACS cannot tell whether the session changed the file",
                current_key,
            )
        });
    };

    if pre_key == post_key {
        return Ok(item(
            path,
            UndoAction::AlreadyUndone,
            Some("the session left it as it found it".to_owned()),
            Effect::Nothing,
            current_key,
        ));
    }
    if current_key == post_key {
        let action = match (&pre, &current) {
            (FileState::Absent, _) => UndoAction::Delete,
            (_, None) => UndoAction::Recreate,
            _ => UndoAction::Restore,
        };
        let effect = match &pre {
            FileState::Absent => Effect::Delete,
            FileState::File { hash, mode, .. } => {
                // Check the blob now, so a dry run knows it will be there.
                if let Err(err) = store.read_blob(hash) {
                    return Ok(unknown(&format!("{err}"), current_key));
                }
                Effect::Restore {
                    hash: hash.clone(),
                    mode: *mode,
                }
            }
        };
        return Ok(item(path, action, None, effect, current_key));
    }
    if current_key == pre_key {
        return Ok(item(
            path,
            UndoAction::AlreadyUndone,
            None,
            Effect::Nothing,
            current_key,
        ));
    }

    // Someone else changed the file after the session: merge the reverse change in.
    let Some((current_bytes, current_mode)) = &current else {
        return Ok(item(
            path,
            UndoAction::Conflict,
            Some("the file was deleted after the session changed it".to_owned()),
            Effect::Nothing,
            current_key,
        ));
    };
    let (post_bytes, pre_bytes) = match (
        blob_of(store, post_key.as_ref()),
        blob_of(store, pre_key.as_ref()),
    ) {
        (Ok(post), Ok(pre)) => (post, pre),
        (Err(err), _) | (_, Err(err)) => return Ok(unknown(&format!("{err}"), current_key)),
    };
    Ok(match merge(current_bytes, &post_bytes, &pre_bytes) {
        MergeResult::Clean(bytes) => item(
            path,
            UndoAction::Merge,
            Some("later changes by others are kept".to_owned()),
            Effect::Write {
                bytes,
                mode: *current_mode,
            },
            current_key,
        ),
        MergeResult::Conflict(why) => item(
            path,
            UndoAction::Conflict,
            Some(why),
            Effect::Nothing,
            current_key,
        ),
    })
}

/// What happened to one item of an applied plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// The file.
    pub path: RepoPath,
    /// `Err` holds why the file was left alone.
    pub result: Result<(), String>,
}

/// Carries out the items of `plan` that change files and returns what happened to each. Items
/// whose file changed since planning are skipped with an error, never overwritten.
#[must_use]
pub fn apply(store: &SnapshotStore, work_tree: &Path, plan: &UndoPlan) -> Vec<Applied> {
    plan.items
        .iter()
        .filter(|i| i.action.changes_file())
        .map(|i| Applied {
            path: i.path.clone(),
            result: apply_item(store, work_tree, i),
        })
        .collect()
}

fn apply_item(store: &SnapshotStore, work_tree: &Path, item: &UndoItem) -> Result<(), String> {
    let abs = work_tree.join(item.path.as_str());
    let now_key = match read_file(&abs) {
        Ok(read) => read.map(|(bytes, _)| ContentHash::of(&bytes)),
        Err(err) => return Err(err.to_string()),
    };
    if now_key != item.current {
        return Err("the file changed while the plan was being applied".to_owned());
    }
    match &item.effect {
        Effect::Nothing => Ok(()),
        Effect::Delete => remove(&abs).map_err(|e| format!("cannot remove {}: {e}", item.path)),
        Effect::Restore { hash, mode } => {
            let bytes = store.read_blob(hash).map_err(|e| e.to_string())?;
            SnapshotStore::write_file(&abs, &bytes, *mode).map_err(|e| e.to_string())
        }
        Effect::Write { bytes, mode } => {
            SnapshotStore::write_file(&abs, bytes, *mode).map_err(|e| e.to_string())
        }
    }
}

/// Removes a file; a missing file is fine, and a read-only one (Windows refuses to delete it) is
/// made writable first.
fn remove(abs: &Path) -> io::Result<()> {
    match fs::remove_file(abs) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            let mut perms = fs::metadata(abs)?.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            fs::set_permissions(abs, perms)?;
            fs::remove_file(abs)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// The paths of the items that were applied successfully.
#[must_use]
pub fn applied_paths(applied: &[Applied]) -> BTreeSet<RepoPath> {
    applied
        .iter()
        .filter(|a| a.result.is_ok())
        .map(|a| a.path.clone())
        .collect()
}
