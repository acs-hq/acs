//! Running the decision engine against the live state, and acquiring the claims it asks for.
//!
//! [`run_gate`] is steps 4 and 5 of the pre-tool pipeline plus the plumbing around the pure
//! engine: it assembles the [`GateInput`] (claims snapshot, pending notices, gate state), calls
//! [`acs_core::engine::decide`], and, for `allow` and `ask`, acquires the requested claims in one
//! transaction. The hook and `claim_paths` both go through it, so a claim made from the CLI or MCP
//! is judged exactly like an agent's edit.

use std::time::Duration;

use acs_core::engine::decide;
use acs_core::gate::{
    ClaimRequest, ClaimSource, ClaimsSnapshot, GateInput, GateOutput, GateState, HeldClaim,
    PathLocation,
};
use acs_core::{ClaimOrigin, Decision, Notice, Outcome, ReasonCode, Seat, Timestamp, ToolEvent};
use acs_store::{AcquireOutcome, FLAG_GATE_OFF, StateDb, StoreError};

use super::RepoContext;
use super::paths::Normalized;
use super::protect::{StoreOverrides, outside_seals};

/// What to decide.
#[derive(Debug, Clone, Copy)]
pub struct GateParams<'a> {
    /// The event.
    pub event: &'a ToolEvent,
    /// The seat, with its branch as `HEAD` says now.
    pub seat: &'a Seat,
    /// The normalized targets of the event.
    pub norm: &'a Normalized,
    /// Who the claims belong to (agent for hooks; human for the CLI's human seat).
    pub origin: ClaimOrigin,
    /// The time of the decision.
    pub now: Timestamp,
}

/// The result of [`run_gate`].
#[derive(Debug, Clone)]
pub struct GateRun {
    /// The engine's output. `decision.claims_acquired` lists the claims acquired and
    /// `decision.warnings` also carries the policy and path-expansion warnings.
    pub output: GateOutput,
    /// The state database could not be used for this decision.
    pub state_unavailable: bool,
    /// `acs off` is active.
    pub gate_off: bool,
}

/// Decides `params.event`, acquiring claims for `allow` and `ask`.
///
/// Never fails: a state database problem makes the decision run with
/// `GateState::state_unavailable` (SPEC section 11). If the claims were taken by someone else
/// between the snapshot and the acquisition, the engine decides again on a fresh snapshot, so
/// the denial has the engine's own wording; only a second lost race falls back to a short generic
/// message.
#[must_use]
pub fn run_gate(ctx: &RepoContext, params: &GateParams<'_>) -> GateRun {
    let policy = ctx.policy_view();
    let config = policy.config();
    let stale_after = Duration::from_secs(config.sync_interval_secs.get().saturating_mul(3));

    let db = ctx.db().ok();
    let mut unavailable = db.is_none();
    let mut gate_off = false;
    if let Some(db) = db {
        match db.flag_is_set(FLAG_GATE_OFF) {
            Ok(set) => gate_off = set,
            Err(_) => unavailable = true,
        }
    }

    // Outside seals cost a few filesystem calls, so they are only computed when the event writes
    // outside the repository or runs a shell command (whose words they are checked against).
    let needs_seals = params.norm.shell.is_some()
        || params
            .norm
            .targets
            .iter()
            .any(|t| matches!(t.path.location, PathLocation::Outside(_)));
    let seals = if needs_seals {
        outside_seals(ctx)
    } else {
        Vec::new()
    };

    let mut lost_races = 0;
    let mut run = None;
    for _ in 0..4 {
        let (claims, notices, overrides) = match db.filter(|_| !unavailable) {
            Some(db) => match load_view(ctx, db, params, stale_after) {
                Ok(view) => view,
                Err(_) => {
                    unavailable = true;
                    (
                        ClaimsSnapshot::default(),
                        Vec::new(),
                        StoreOverrides::default(),
                    )
                }
            },
            None => (
                ClaimsSnapshot::default(),
                Vec::new(),
                StoreOverrides::default(),
            ),
        };
        let input = GateInput {
            event: params.event,
            seat: params.seat,
            origin: params.origin,
            targets: &params.norm.targets,
            shell: params.norm.shell.as_ref(),
            claims: &claims,
            notices: &notices,
            overrides: &overrides,
            outside_sealed: &seals,
            state: GateState {
                gate_off,
                state_unavailable: unavailable,
            },
            now: params.now,
        };
        let mut output = decide(&input, policy);
        if output.decision.outcome != Outcome::Deny
            && !output.overrides_applied.is_empty()
            && let Some(db) = db.filter(|_| !unavailable)
        {
            let uses: Vec<_> = output
                .overrides_applied
                .iter()
                .map(|applied| (applied.override_id.clone(), applied.path.clone()))
                .collect();
            match db.apply_overrides(&params.seat.seat_id, &uses, params.now) {
                Ok(true) => {}
                // Another seat used it, or it lapsed or was revoked since the view was loaded:
                // decide again without it.
                Ok(false) => continue,
                Err(_) => {
                    unavailable = true;
                    continue;
                }
            }
        }
        let wants_claims = output.decision.outcome != Outcome::Deny
            && !output.claim_requests.is_empty()
            && !unavailable
            && !gate_off;
        let Some(db) = db.filter(|_| wants_claims) else {
            run = Some(output);
            break;
        };
        for request in &mut output.claim_requests {
            request.origin = params.origin;
        }
        match acquire(db, params, &output.claim_requests, stale_after) {
            Ok(AcquireOutcome::Granted(acquired)) => {
                output.decision.claims_acquired = acquired.claim_ids;
                run = Some(output);
                break;
            }
            Ok(AcquireOutcome::Conflict(conflict)) => {
                lost_races += 1;
                if lost_races >= 2 {
                    output.decision = lost_race_denial(&conflict.conflicts, params.now);
                    output.claim_requests.clear();
                    output.snapshot_requests.clear();
                    output.delivered_notices.clear();
                    run = Some(output);
                    break;
                }
            }
            Err(_) => unavailable = true,
        }
    }
    let mut output = run.unwrap_or_else(|| unavailable_fallback(params));
    let mut warnings = ctx.policy.warnings.clone();
    warnings.append(&mut output.decision.warnings);
    warnings.extend(params.norm.warnings.iter().cloned());
    output.decision.warnings = warnings;
    GateRun {
        output,
        state_unavailable: unavailable,
        gate_off,
    }
}

fn load_view(
    ctx: &RepoContext,
    db: &StateDb,
    params: &GateParams<'_>,
    stale_after: Duration,
) -> Result<(ClaimsSnapshot, Vec<Notice>, StoreOverrides), StoreError> {
    let mut claims = db.claims_snapshot(
        &params.norm.repo_paths,
        &params.seat.target_branch,
        params.now,
        stale_after,
    )?;
    if !ctx.features().sync {
        // Sync is switched off: what an earlier sync fetched is no longer anyone's word.
        claims
            .claims
            .retain(|held| held.source != ClaimSource::Remote);
        claims.remote_stale = false;
    }
    if claims.remote_stale {
        // No remote, or sync never ran: there is no remote view to be stale (decision 0011
        // section 3), so the engine must not warn about it on every claim.
        let sync_ran = ctx.has_remote && db.remote_sync_status()?.last_attempt_at.is_some();
        if !sync_ran {
            claims.remote_stale = false;
        }
    }
    let notices = db.pending_notices(&params.seat.seat_id)?;
    let overrides = StoreOverrides::load(ctx, db, &params.seat.seat_id, params.now)?;
    Ok((claims, notices, overrides))
}

fn acquire(
    db: &StateDb,
    params: &GateParams<'_>,
    requests: &[ClaimRequest],
    stale_after: Duration,
) -> Result<AcquireOutcome, StoreError> {
    db.try_acquire(&params.seat.seat_id, requests, params.now, stale_after)
}

/// The denial for a claim race lost twice in a row (the engine's own wording is used for the
/// first loss, by deciding again).
fn lost_race_denial(conflicts: &[HeldClaim], now: Timestamp) -> Decision {
    let message = conflicts.first().map_or_else(
        || {
            "Another seat claimed these files at the same moment. Work on other files, or retry."
                .to_owned()
        },
        |held| {
            let who = match &held.holder_harness {
                Some(harness) => format!("{} ({harness})", held.claim.pilot),
                None => held.claim.pilot.clone(),
            };
            let mins = now
                .as_second()
                .saturating_sub(held.claim.heartbeat_at.as_second())
                / 60;
            format!(
                "`{}` was claimed by {who} at the same moment (active {mins} min). Work on other \
                 files, or ask your pilot to override.",
                held.claim.scope.path()
            )
        },
    );
    Decision::new(Outcome::Deny, ReasonCode::ClaimConflict, message)
}

/// Only reachable if the loop above exits without a result, which its bounds make impossible; it
/// exists so the function has no panic path. Fails open, as the failure-mode table says for
/// state problems.
fn unavailable_fallback(_params: &GateParams<'_>) -> GateOutput {
    GateOutput {
        decision: Decision::new(Outcome::Allow, ReasonCode::StateUnavailable, ""),
        claim_requests: Vec::new(),
        snapshot_requests: Vec::new(),
        delivered_notices: Vec::new(),
        overrides_applied: Vec::new(),
    }
}
