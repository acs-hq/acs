//! The post-tool pipeline: find out what the tool call actually changed, attribute it to the
//! session, store post-images, refresh heartbeats.
//!
//! Nothing here depends on `event.result`: some harnesses report no exit code for shell commands
//! (Claude Code's `PostToolUse` has none; failures arrive separately) and some report no result
//! at all (Antigravity). Changes are found by looking, not by trusting the tool's report.

use std::collections::HashSet;
use std::fs;
use std::time::SystemTime;

use acs_core::gate::HeldClaim;
use acs_core::{
    ClaimId, Decision, Outcome, ReasonCode, ReleaseReason, RepoPath, Timestamp, ToolEvent, ToolKind,
};
use acs_git::status::{ChangeKind, ScanInput, ScanMode, ScanOptions, scan_changes};
use acs_store::{Attribution, AttributionSource, NewEvent, SeatInfo, StateDb};

use super::markers::{self, ScanBaseline, ToolMarker};
use super::snapshots::{Observed, observe, record_baselines};
use crate::runtime::paths::{NormalizeOptions, normalize, parent_dir};
use crate::runtime::{RepoContext, seats, sync};

/// How many of the seat's claimed files (and their directories) join a scoped scan.
const MAX_CLAIMED_SCOPE: usize = 200;
/// The most directories in a scan's scope.
const MAX_SCAN_DIRS: usize = 64;

/// Handles a `post_tool` event. Always allows; its work is recording.
pub fn run(ctx: &RepoContext, event: &ToolEvent, now: Timestamp, scan: &ScanOptions) -> Decision {
    let allow = Decision::new(Outcome::Allow, ReasonCode::Ok, "");
    let Ok(db) = ctx.db() else { return allow };
    let Ok(resolved) = seats::agent_seat(ctx, db, &event.harness, &event.harness_session_id, now)
    else {
        return allow;
    };
    let info = resolved.info;
    let kind = event.tool.as_ref().map(|t| t.kind);

    let (observed, source) = match kind {
        None | Some(ToolKind::Read) => (Vec::new(), AttributionSource::Tool),
        // History off: no copies are kept, so there is nothing to compare and attribute. The
        // marker the pre-tool step left is still used up.
        Some(_) if !ctx.features().history => {
            let _ = markers::take_tool_marker(ctx, &info.seat.seat_id);
            (Vec::new(), AttributionSource::Tool)
        }
        Some(ToolKind::Shell) => (
            shell_changes(ctx, db, &info, now, scan),
            AttributionSource::Shell,
        ),
        Some(_) => (
            tool_changes(ctx, &info, event, now),
            AttributionSource::Tool,
        ),
    };

    let changes: Vec<Attribution> = observed
        .iter()
        .map(|o| Attribution {
            path: o.path.clone(),
            pre_hash: o.pre_hash.clone(),
            post_hash: o.post_hash.clone(),
        })
        .collect();
    let _ = db.record_attributions(&info.seat.seat_id, &changes, source, now);
    let _ = db.heartbeat_seat_claims(&info.seat.seat_id, now);
    let paths: Vec<String> = observed
        .iter()
        .map(|o| o.path.as_str().to_owned())
        .collect();
    let tool = event.tool.as_ref();
    let _ = db.record_event(&NewEvent {
        seat_id: &info.seat.seat_id,
        at: now,
        phase: event.phase,
        tool_name: tool.map(|t| t.name.as_str()),
        tool_kind: tool.map(|t| t.kind),
        paths: &paths[..paths.len().min(20)],
        reason_code: None,
        outcome: None,
        detail: None,
    });
    sync::spawn_if_due(ctx, db, now);
    allow
}

/// Changes made by a file tool: the paths the event names (or, for deletes and moves, the paths
/// the pre-tool step predicted, which include the files of a deleted directory).
fn tool_changes(
    ctx: &RepoContext,
    info: &SeatInfo,
    event: &ToolEvent,
    now: Timestamp,
) -> Vec<Observed> {
    let kind = event.tool.as_ref().map(|t| t.kind);
    let marker = if matches!(kind, Some(ToolKind::Delete | ToolKind::Move)) {
        markers::take_tool_marker(ctx, &info.seat.seat_id)
    } else {
        None
    };
    let paths: Vec<RepoPath> = match marker {
        Some(marker) => marker
            .paths
            .into_iter()
            .filter_map(|p| RepoPath::try_from(p).ok())
            .collect(),
        None => normalize(ctx, event, NormalizeOptions::default()).repo_paths,
    };
    observe(ctx, &info.session_id, &paths, now)
}

/// Changes made by a shell command: a working-tree diff since the command started.
fn shell_changes(
    ctx: &RepoContext,
    db: &StateDb,
    info: &SeatInfo,
    now: Timestamp,
    scan: &ScanOptions,
) -> Vec<Observed> {
    let marker: Option<ToolMarker> = markers::take_tool_marker(ctx, &info.seat.seat_id);
    // `info` was read before this event touched the seat, so `last_seen_at` is the previous
    // activity: the pre-tool step, when no marker was left.
    let since_micros = marker
        .as_ref()
        .map_or(info.seat.last_seen_at.as_microsecond(), |m| m.t0_micros);
    let since =
        Timestamp::from_microsecond(since_micros).map_or(SystemTime::UNIX_EPOCH, SystemTime::from);

    let live = db.live_claims(now).unwrap_or_default();
    if let Some(marker) = &marker {
        release_directory_claims(ctx, db, info, &live, &marker.paths, now);
    }
    let mut known: Vec<String> = marker.as_ref().map(|m| m.paths.clone()).unwrap_or_default();
    let mut dirs: Vec<String> = marker
        .as_ref()
        .map(|m| m.scope_dirs.clone())
        .unwrap_or_default();
    let mut elsewhere: HashSet<String> = HashSet::new();
    let mut mine = 0;
    for held in &live {
        let path = held.claim.scope.path().as_str().to_owned();
        if held.claim.seat_id == info.seat.seat_id {
            if mine < MAX_CLAIMED_SCOPE {
                dirs.push(parent_dir(&path));
                known.push(path);
                mine += 1;
            }
        } else {
            elsewhere.insert(path);
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs.truncate(MAX_SCAN_DIRS);
    known.sort();
    known.dedup();

    let input = ScanInput {
        since,
        known_paths: &known,
        scope_dirs: &dirs,
    };
    let report = scan_changes(&ctx.layout, &input, scan).unwrap_or_else(|_| {
        // `git status` failed (git missing, damaged index): look at what we know instead.
        let scoped = ScanOptions {
            mode: ScanMode::Scoped,
            ..*scan
        };
        scan_changes(&ctx.layout, &input, &scoped).unwrap_or_else(|_| empty_report())
    });

    let mut paths: Vec<RepoPath> = report
        .changes
        .iter()
        .filter(|c| c.kind == ChangeKind::Present || c.kind == ChangeKind::Missing)
        .filter(|c| !elsewhere.contains(&c.path) && !c.path.starts_with(".git/"))
        .filter_map(|c| RepoPath::try_from(c.path.clone()).ok())
        .collect();

    if report.mode == ScanMode::Full {
        let previous = markers::read_scan_baseline(ctx);
        if let Some(previous) = previous {
            let before: HashSet<&str> = previous.deleted.iter().map(String::as_str).collect();
            paths.extend(
                report
                    .deleted_tracked
                    .iter()
                    .filter(|p| !before.contains(p.as_str()) && !elsewhere.contains(*p))
                    .filter_map(|p| RepoPath::try_from(p.clone()).ok()),
            );
        }
        markers::write_scan_baseline(
            ctx,
            &ScanBaseline {
                deleted: report.deleted_tracked.clone(),
            },
        );
    }
    paths.sort();
    paths.dedup();

    record_baselines(ctx, &info.session_id, &paths, since, now);
    observe(ctx, &info.session_id, &paths, now)
}

/// Releases the seat's claims on paths this command was predicted to write that are now
/// directories. The pre-tool step claims a path that does not exist yet as a file, so `mkdir
/// newdir` or `cp -r src newdir` would otherwise leave a file claim on a directory for the rest
/// of the session. Best effort.
fn release_directory_claims(
    ctx: &RepoContext,
    db: &StateDb,
    info: &SeatInfo,
    live: &[HeldClaim],
    predicted: &[String],
    now: Timestamp,
) {
    let ids: Vec<ClaimId> = live
        .iter()
        .filter(|held| held.claim.seat_id == info.seat.seat_id)
        .filter(|held| {
            let path = held.claim.scope.path().as_str();
            predicted.iter().any(|p| p == path)
                && fs::symlink_metadata(ctx.layout.work_tree.join(path))
                    .is_ok_and(|meta| meta.is_dir())
        })
        .map(|held| held.claim.claim_id.clone())
        .collect();
    if !ids.is_empty() {
        let _ = db.release_claims(&ids, ReleaseReason::Released, now);
    }
}

fn empty_report() -> acs_git::status::ScanReport {
    acs_git::status::ScanReport {
        mode: ScanMode::Scoped,
        changes: Vec::new(),
        deleted_tracked: Vec::new(),
        truncated: false,
    }
}
