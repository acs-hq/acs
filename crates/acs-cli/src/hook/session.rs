//! Session events: start, prompt submit, end.

use acs_core::{Decision, Outcome, ReasonCode, Timestamp, ToolEvent};
use acs_store::NewEvent;

use super::{keep_alive, markers};
use crate::runtime::{RepoContext, seats, status, sync};

fn allow() -> Decision {
    Decision::new(Outcome::Allow, ReasonCode::Ok, "")
}

/// `session_start`: creates (or revives) the seat and returns a short status as context.
///
/// The status is returned on every start, whatever the harness says about why the session
/// started (startup, resume, clear, compaction): a compaction or a clear drops the earlier
/// context, and a resumed session may have lost its claims to expiry.
pub fn start(ctx: &RepoContext, event: &ToolEvent, now: Timestamp) -> Decision {
    let mut decision = allow();
    decision.warnings.clone_from(&ctx.policy.warnings);
    let Ok(db) = ctx.db() else {
        decision.reason_code = ReasonCode::StateUnavailable;
        decision.warnings.push(
            "ACS cannot use its state database right now, so claims are not enforced. Run `acs \
             doctor`."
                .to_owned(),
        );
        return decision;
    };
    let Ok(resolved) = seats::agent_seat(ctx, db, &event.harness, &event.harness_session_id, now)
    else {
        decision.reason_code = ReasonCode::StateUnavailable;
        decision
            .warnings
            .push("ACS could not register this session; claims are not enforced.".to_owned());
        return decision;
    };
    // Housekeeping that keeps the live claims table small; nothing depends on it.
    let _ = db.sweep_expired(now);
    let view = status(ctx, Some(&resolved.info), now);
    decision.context.push(view.summary_lines().join("\n"));
    let _ = db.record_event(&NewEvent {
        seat_id: &resolved.info.seat.seat_id,
        at: now,
        phase: event.phase,
        tool_name: None,
        tool_kind: None,
        paths: &[],
        reason_code: None,
        outcome: None,
        detail: None,
    });
    // After idle time the remote view is stale; refresh it before the first edit rather than
    // after it. Harnesses without session events get this from their first post-tool event.
    sync::spawn_if_due(ctx, db, now);
    decision
}

/// `prompt_submit`: delivers pending notices, and nothing else. Some harnesses call this before
/// every model call, so with nothing undelivered the context is empty.
///
/// A session without a seat has nothing waiting for it, so none is created here: the first
/// event that needs one does that. A live seat is kept alive, not marked as seen every time.
pub fn prompt(ctx: &RepoContext, event: &ToolEvent, now: Timestamp) -> Decision {
    let mut decision = allow();
    let Ok(db) = ctx.db() else { return decision };
    let Ok(Some(info)) = db.seat_by_session(&event.harness, &event.harness_session_id) else {
        return decision;
    };
    keep_alive(db, &info, now);
    let Ok(notices) = db.pending_notices(&info.seat.seat_id) else {
        return decision;
    };
    if notices.is_empty() {
        return decision;
    }
    let ids: Vec<_> = notices.iter().map(|n| n.notice_id.clone()).collect();
    // Only the process that marks a notice delivered shows it, so two racing hooks of the same
    // seat never show it twice.
    if db.mark_delivered(&ids, now).is_ok_and(|marked| marked > 0) {
        decision.context = notices.into_iter().map(|n| n.body).collect();
    }
    decision
}

/// `session_end`: releases the seat's claims and ends it. Cheap on purpose (harnesses give this
/// hook about a second): no scans, no snapshot work. Snapshots are kept for `acs undo`
/// (decision 0011 section 3); a seat that comes back (resume) is revived by its next event.
pub fn end(ctx: &RepoContext, event: &ToolEvent, now: Timestamp) -> Decision {
    let decision = allow();
    let Ok(db) = ctx.db() else { return decision };
    let Ok(Some(info)) = db.seat_by_session(&event.harness, &event.harness_session_id) else {
        return decision;
    };
    // With claims, history and sync all off nothing reads this event: there are no claims to
    // release, no record to write and nothing to push. The seat is still ended, so that the MCP
    // server does not take a finished session for a live one.
    let features = ctx.features();
    if features.claims || features.history || features.sync {
        let _ = db.record_event(&NewEvent {
            seat_id: &info.seat.seat_id,
            at: now,
            phase: event.phase,
            tool_name: None,
            tool_kind: None,
            paths: &[],
            reason_code: None,
            outcome: None,
            detail: None,
        });
    }
    let _ = db.end_seat(&info.seat.seat_id, now);
    markers::clear_tool_marker(ctx, &info.seat.seat_id);
    // The claims just released would otherwise block other machines until their TTL: there may
    // be no further event here to carry the news. The same detached `acs sync` writes the
    // session's record to the ledger, with or without a remote. With history and sync both off
    // it starts nothing.
    sync::spawn_for_records(ctx);
    decision
}
