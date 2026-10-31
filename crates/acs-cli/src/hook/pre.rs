//! The pre-tool pipeline (SPEC section 9, steps 2 to 8; the adapter does 1 and 8).

use acs_core::{
    ClaimOrigin, Decision, Outcome, ReasonCode, Seat, SeatId, Timestamp, ToolEvent, ToolKind,
};
use acs_store::{LastDecision, NewEvent, SeatInfo};

use super::markers::{self, CommitMarker, ToolMarker};
use super::redact::redact_command;
use super::snapshots::capture_pre;
use crate::runtime::gate::{GateParams, run_gate};
use crate::runtime::paths::{NormalizeOptions, normalize};
use crate::runtime::{RepoContext, seats};

/// The most paths stored in an event row.
const MAX_EVENT_PATHS: usize = 20;

/// Handles a `pre_tool` event and returns the decision.
pub fn run(ctx: &RepoContext, event: &ToolEvent, now: Timestamp) -> Decision {
    let norm = normalize(ctx, event, NormalizeOptions::default());
    let resolved = ctx.db().ok().and_then(|db| {
        seats::agent_seat(ctx, db, &event.harness, &event.harness_session_id, now).ok()
    });
    let (seat_info, engine_seat): (Option<SeatInfo>, Seat) = match resolved {
        Some(resolved) => {
            let seat = resolved.engine_seat(ctx);
            (Some(resolved.info), seat)
        }
        None => match unavailable_seat(ctx, event, now) {
            Some(seat) => (None, seat),
            None => {
                return Decision::new(Outcome::Allow, ReasonCode::StateUnavailable, "");
            }
        },
    };

    let run = run_gate(
        ctx,
        &GateParams {
            event,
            seat: &engine_seat,
            norm: &norm,
            origin: ClaimOrigin::Agent,
            now,
        },
    );
    let output = run.output;
    let mut decision = output.decision;
    let outcome = decision.outcome;
    let proceeds = outcome != Outcome::Deny;

    if let (Some(info), true) = (&seat_info, proceeds) {
        decision.warnings.extend(capture_pre(
            ctx,
            &info.session_id,
            &output.snapshot_requests,
            &norm,
            now,
        ));
    }

    if let (Ok(db), Some(info)) = (ctx.db(), &seat_info) {
        if !output.delivered_notices.is_empty() {
            let _ = db.mark_delivered(&output.delivered_notices, now);
        }
        record(ctx, event, info, &norm.predicted, &decision, now);
        if proceeds {
            leave_markers(ctx, event, info, &norm, now);
        }
    }
    decision
}

/// The seat used when the state database is down: nothing is stored for it, but the engine
/// still needs one to enforce what needs no state.
fn unavailable_seat(ctx: &RepoContext, event: &ToolEvent, now: Timestamp) -> Option<Seat> {
    let seat_id: SeatId = "seat_unavailable".parse().ok()?;
    Some(Seat {
        seat_id,
        pilot: acs_git::repo::fallback_pilot(),
        harness: event.harness.clone(),
        harness_session_id: event.harness_session_id.clone(),
        worktree: ctx.worktree_str(),
        branch: acs_git::repo::current_branch(&ctx.layout.git_dir),
        target_branch: ctx
            .policy
            .policy
            .files()
            .config
            .default_target_branch
            .clone(),
        started_at: now,
        last_seen_at: now,
    })
}

/// Records the event and the seat's last decision. Best effort: the decision stands either way.
fn record(
    ctx: &RepoContext,
    event: &ToolEvent,
    info: &SeatInfo,
    predicted: &[String],
    decision: &Decision,
    now: Timestamp,
) {
    let Ok(db) = ctx.db() else { return };
    let paths: Vec<String> = predicted.iter().take(MAX_EVENT_PATHS).cloned().collect();
    let tool = event.tool.as_ref();
    // A collision keeps what the seat was told: it names the holder, who may be gone by the time
    // `acs status` shows it. The message holds paths and the holder, no command text.
    let detail = if decision.reason_code == ReasonCode::ClaimConflict {
        Some(decision.message.clone())
    } else {
        (decision.outcome != Outcome::Allow)
            .then(|| event.command.as_deref().map(redact_command))
            .flatten()
    };
    let _ = db.record_event(&NewEvent {
        seat_id: &info.seat.seat_id,
        at: now,
        phase: event.phase,
        tool_name: tool.map(|t| t.name.as_str()),
        tool_kind: tool.map(|t| t.kind),
        paths: &paths,
        reason_code: Some(decision.reason_code),
        outcome: Some(decision.outcome),
        detail: detail.as_deref(),
    });
    // An uneventful allow that touches nothing (a read-only tool, or the MCP call to `acs_why`
    // itself) must not replace the decision the agent is asking about.
    if decision.outcome == Outcome::Allow && paths.is_empty() && event.command.is_none() {
        return;
    }
    let _ = db.set_last_decision(
        &info.seat.seat_id,
        &LastDecision {
            outcome: decision.outcome,
            reason_code: decision.reason_code,
            message: decision.message.clone(),
            at: now,
        },
    );
}

/// Writes what the post-tool step and the git hooks need.
fn leave_markers(
    ctx: &RepoContext,
    event: &ToolEvent,
    info: &SeatInfo,
    norm: &crate::runtime::paths::Normalized,
    now: Timestamp,
) {
    let kind = event.tool.as_ref().map(|t| t.kind);
    if matches!(
        kind,
        Some(ToolKind::Shell | ToolKind::Delete | ToolKind::Move)
    ) {
        let _ = markers::write_tool_marker(
            ctx,
            &info.seat.seat_id,
            &ToolMarker {
                t0_micros: now.as_microsecond(),
                paths: norm.predicted.clone(),
                scope_dirs: norm.scope_dirs.clone(),
            },
        );
    }
    if pushes(norm) {
        let _ = markers::write_push_marker(
            ctx,
            &CommitMarker {
                session_id: info.session_id.to_string(),
                seat_id: info.seat.seat_id.to_string(),
                harness: info.seat.harness.clone(),
                created_at: now,
            },
        );
    }
    if norm.creates_commit {
        let _ = markers::write_commit_marker(
            ctx,
            &CommitMarker {
                session_id: info.session_id.to_string(),
                seat_id: info.seat.seat_id.to_string(),
                harness: info.seat.harness.clone(),
                created_at: now,
            },
        );
    }
}

/// Whether a shell command in the event runs `git push` (the `pre-push` git hook then knows the
/// push comes from an agent session).
fn pushes(norm: &crate::runtime::paths::Normalized) -> bool {
    norm.shell.as_ref().is_some_and(|shell| {
        shell
            .commands
            .iter()
            .any(|c| c.git.as_ref().is_some_and(|g| g.subcommand == "push"))
    })
}
