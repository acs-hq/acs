//! The shared operations: what `acs status`, `acs claim`, `acs release` and the MCP tools call.
//!
//! They go through the same path resolution, engine and `try_acquire` as the hook, so a claim
//! made here is judged exactly like an agent's edit, and a refusal explains itself the same way.

use std::path::Path;

use acs_core::gate::{ClaimSource, HeldClaim};
use acs_core::{
    ClaimId, ClaimOrigin, ClaimScope, Outcome, Phase, ReasonCode, ReleaseReason, RepoPath, SeatId,
    SpecVersion, Timestamp, Tool, ToolEvent, ToolKind,
};
use acs_store::{Collision, FLAG_GATE_OFF, LastDecision, NewEvent, SeatInfo, StateDb, StoreError};
use serde::Serialize;

use super::gate::{GateParams, run_gate};
use super::paths::{NormalizeOptions, normalize, resolve_for_claim};
use super::seats::{HUMAN_HARNESS, current_view};
use super::{OpError, RepoContext};

/// The longest intent text stored, in characters.
pub const MAX_INTENT_CHARS: usize = 500;

/// A claim as shown to people and agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaimView {
    /// The claimed file.
    pub path: String,
    /// The symbol inside the file, for symbol claims.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_path: Option<String>,
    /// The pilot holding it.
    pub pilot: String,
    /// The holder's harness (`human` for claims made from the CLI), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// The holder's seat.
    pub seat_id: String,
    /// `local` (this machine) or `remote` (another machine's, as last fetched).
    pub source: &'static str,
    /// Whether an agent or a human made it.
    pub origin: ClaimOrigin,
    /// When it was acquired.
    pub acquired_at: Timestamp,
    /// The last heartbeat.
    pub heartbeat_at: Timestamp,
    /// When it expires unless refreshed.
    pub expires_at: Timestamp,
}

impl ClaimView {
    fn of(held: &HeldClaim) -> Self {
        let claim = &held.claim;
        let (path, symbol_path) = match &claim.scope {
            ClaimScope::File { file } => (file.as_str().to_owned(), None),
            ClaimScope::Symbol {
                symbol,
                symbol_path,
            } => (
                symbol.as_str().to_owned(),
                Some(symbol_path.as_str().to_owned()),
            ),
        };
        let ttl = i64::try_from(claim.ttl_secs).unwrap_or(i64::MAX);
        let expires = Timestamp::from_second(claim.heartbeat_at.as_second().saturating_add(ttl))
            .unwrap_or(claim.heartbeat_at);
        Self {
            path,
            symbol_path,
            pilot: claim.pilot.clone(),
            harness: held.holder_harness.clone(),
            seat_id: claim.seat_id.to_string(),
            source: match held.source {
                ClaimSource::Local => "local",
                ClaimSource::Remote => "remote",
            },
            origin: claim.origin,
            acquired_at: claim.acquired_at,
            heartbeat_at: claim.heartbeat_at,
            expires_at: expires,
        }
    }
}

/// How far back `acs status` looks for collisions, in hours.
pub const COLLISION_WINDOW_HOURS: i64 = 24;
/// The most collisions a status view carries.
const MAX_COLLISIONS: usize = 200;

/// A call refused because another seat held a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollisionView {
    /// When it happened.
    pub at: Timestamp,
    /// The paths of the refused call.
    pub paths: Vec<String>,
    /// The tool that was refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The pilot of the seat that was refused.
    pub pilot: String,
    /// The harness of the seat that was refused.
    pub harness: String,
    /// The seat that was refused.
    pub seat_id: String,
    /// What the seat was told, which names the holder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl CollisionView {
    fn of(collision: Collision) -> Self {
        Self {
            at: collision.at,
            paths: collision.paths,
            tool: collision.tool_name,
            pilot: collision.pilot,
            harness: collision.harness,
            seat_id: collision.seat_id.to_string(),
            message: collision.detail,
        }
    }
}

/// Records that `seat` was refused because of another seat's claim, so `acs status` can show it.
/// Best effort: the refusal stands either way.
pub fn record_collision(
    db: &StateDb,
    seat_id: &SeatId,
    tool: (&str, ToolKind),
    paths: &[String],
    message: &str,
    now: Timestamp,
) {
    let _ = db.record_event(&NewEvent {
        seat_id,
        at: now,
        phase: Phase::PreTool,
        tool_name: Some(tool.0),
        tool_kind: Some(tool.1),
        paths,
        reason_code: Some(ReasonCode::ClaimConflict),
        outcome: Some(Outcome::Deny),
        detail: Some(message),
    });
}

/// The seat a status view is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeatSummary {
    /// The seat id.
    pub seat_id: String,
    /// The `ses_` id of its session.
    pub session_id: String,
    /// The harness (`human` for the CLI's seat).
    pub harness: String,
    /// The pilot.
    pub pilot: String,
    /// What the agent said it is doing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
}

/// The state of the link to the git remote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteView {
    /// A git remote is configured.
    pub configured: bool,
    /// Sync has run at least once on this machine.
    pub sync_ran: bool,
    /// The remote claims view is stale: sync ran, and the last successful fetch is older than
    /// `3 x sync_interval_secs` or never happened. Never true without a remote or before the
    /// first sync (there is nothing to be stale).
    pub stale: bool,
    /// When a fetch last succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<Timestamp>,
    /// When a sync was last attempted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<Timestamp>,
    /// What went wrong in the last attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// The answer of [`status`]: what `acs status` and the session-start context show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusView {
    /// The repository id, when it could be resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
    /// The work tree.
    pub worktree: String,
    /// The checked-out branch; absent when detached.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The branch work is integrated into.
    pub target_branch: String,
    /// `acs off` is active (sealed paths still hold).
    pub gate_off: bool,
    /// Why the state database cannot be used, if it cannot; claims are then not listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_unavailable: Option<String>,
    /// The seat the view is for, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seat: Option<SeatSummary>,
    /// Live claims of that seat.
    pub my_claims: Vec<ClaimView>,
    /// Live claims of every other seat on this machine, and the remote view's claims.
    pub other_claims: Vec<ClaimView>,
    /// Calls refused on this machine in the last [`COLLISION_WINDOW_HOURS`] hours because another
    /// seat held a file, newest first. Kept after both seats end, so a collision cannot go unseen.
    pub collisions: Vec<CollisionView>,
    /// How many there were, which can be more than [`StatusView::collisions`] lists.
    pub collisions_total: u64,
    /// Globs of paths no agent may write (built-in defaults, `guards.toml`, sealed zones).
    pub sealed: Vec<String>,
    /// The link to the git remote.
    pub remote: RemoteView,
    /// Warnings from loading the policy (a fallback was used, an empty team, ...).
    pub policy_warnings: Vec<String>,
}

impl StatusView {
    /// A few lines for an agent's context at session start: what it holds, what others hold,
    /// what is sealed. Kept short on purpose (each line is spent context).
    #[must_use]
    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines = vec![
            "ACS coordinates the agents working in this repository. Editing a file claims it for \
             you; a file another seat has claimed is blocked until they finish."
                .to_owned(),
        ];
        if self.gate_off {
            lines.push(
                "`acs off` is active: claims are not enforced. Sealed paths still are.".to_owned(),
            );
        }
        if self.my_claims.is_empty() {
            lines.push("You hold no claims yet.".to_owned());
        } else {
            lines.push(format!(
                "You hold: {}.",
                list(self.my_claims.iter().map(|c| c.path.as_str()))
            ));
        }
        if !self.other_claims.is_empty() {
            let shown = self.other_claims.iter().take(3).map(|c| {
                let who = c.harness.as_deref().map_or_else(
                    || c.pilot.clone(),
                    |harness| format!("{}, {harness}", c.pilot),
                );
                format!("`{}` ({who})", c.path)
            });
            let mut text = shown.collect::<Vec<_>>().join(", ");
            if self.other_claims.len() > 3 {
                text.push_str(&format!(" and {} more", self.other_claims.len() - 3));
            }
            lines.push(format!("Claimed by others: {text}."));
        }
        if !self.sealed.is_empty() {
            let shown = self.sealed.iter().take(4).map(|g| format!("`{g}`"));
            let mut text = shown.collect::<Vec<_>>().join(", ");
            if self.sealed.len() > 4 {
                text.push_str(&format!(" and {} more", self.sealed.len() - 4));
            }
            lines.push(format!("Never writable: {text}."));
        }
        lines
    }
}

fn list<'a>(items: impl Iterator<Item = &'a str>) -> String {
    let all: Vec<&str> = items.collect();
    let mut text = all
        .iter()
        .take(5)
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if all.len() > 5 {
        text.push_str(&format!(" and {} more", all.len() - 5));
    }
    text
}

/// The status of the repository, optionally from the point of view of `seat`.
///
/// Never fails: a state database problem is reported inside the view
/// ([`StatusView::state_unavailable`]) and the claim lists are empty.
#[must_use]
pub fn status(ctx: &RepoContext, seat: Option<&SeatInfo>, now: Timestamp) -> StatusView {
    let files = ctx.policy.policy.files();
    let mut sealed: Vec<String> = files.guards.sealed.clone();
    sealed.extend(
        files
            .territory
            .zone
            .iter()
            .filter(|zone| zone.kind == acs_core::ZoneKind::Sealed)
            .flat_map(|zone| zone.paths.iter().cloned()),
    );
    let mut view = StatusView {
        repo_id: ctx.repo_home.as_ref().map(|h| h.id().as_str().to_owned()),
        worktree: ctx.worktree_str(),
        branch: acs_git::repo::current_branch(&ctx.layout.git_dir),
        target_branch: files.config.default_target_branch.clone(),
        gate_off: false,
        state_unavailable: None,
        seat: seat.map(|info| SeatSummary {
            seat_id: info.seat.seat_id.to_string(),
            session_id: info.session_id.to_string(),
            harness: info.seat.harness.clone(),
            pilot: info.seat.pilot.clone(),
            intent: info.intent.clone(),
        }),
        my_claims: Vec::new(),
        other_claims: Vec::new(),
        collisions: Vec::new(),
        collisions_total: 0,
        sealed,
        remote: RemoteView {
            configured: ctx.has_remote,
            sync_ran: false,
            stale: false,
            last_success_at: None,
            last_attempt_at: None,
            last_error: None,
        },
        policy_warnings: ctx.policy.warnings.clone(),
    };
    let db = match ctx.db() {
        Ok(db) => db,
        Err(why) => {
            view.state_unavailable = Some(why.to_owned());
            return view;
        }
    };
    let loaded = (|| -> Result<(), StoreError> {
        view.gate_off = db.flag_is_set(FLAG_GATE_OFF)?;
        let mine = seat.map(|s| &s.seat.seat_id);
        let local = db.live_claims(now)?;
        let remote = db.live_remote_claims(now)?;
        for held in local.iter().chain(remote.iter()) {
            let item = ClaimView::of(held);
            if mine == Some(&held.claim.seat_id) {
                view.my_claims.push(item);
            } else {
                view.other_claims.push(item);
            }
        }
        let since = Timestamp::from_second(
            now.as_second()
                .saturating_sub(COLLISION_WINDOW_HOURS * 3600),
        )
        .unwrap_or(now);
        let (collisions, total) = db.collisions_since(since, MAX_COLLISIONS)?;
        view.collisions = collisions.into_iter().map(CollisionView::of).collect();
        view.collisions_total = total;
        let sync = db.remote_sync_status()?;
        let stale_after = files.config.sync_interval_secs.get().saturating_mul(3);
        view.remote.sync_ran = sync.last_attempt_at.is_some();
        view.remote.stale = ctx.has_remote
            && view.remote.sync_ran
            && sync.last_success_at.is_none_or(|at| {
                now.as_second().saturating_sub(at.as_second())
                    > i64::try_from(stale_after).unwrap_or(i64::MAX)
            });
        view.remote.last_success_at = sync.last_success_at;
        view.remote.last_attempt_at = sync.last_attempt_at;
        view.remote.last_error = sync.last_error;
        Ok(())
    })();
    if let Err(err) = loaded {
        view.state_unavailable = Some(err.to_string());
    }
    view
}

/// The result of [`claim_paths`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The seat holds the claims now (or already did: a claim is idempotent).
    Granted {
        /// The files claimed.
        paths: Vec<RepoPath>,
        /// The claim ids, one per file.
        claim_ids: Vec<ClaimId>,
        /// Warnings that did not change the outcome (policy fallback, a stale remote view,
        /// `acs off`).
        warnings: Vec<String>,
    },
    /// Nothing was claimed.
    Refused {
        /// Why.
        reason_code: ReasonCode,
        /// Guidance, in the engine's wording: names the holder or the rule and what to do next.
        message: String,
    },
}

/// Claims `paths` (relative to `cwd` or absolute; a directory claims the files under it) for
/// `seat`, going through the same resolution, guards, conflict rule and atomic acquisition as an
/// agent's edit. A human seat's claims have origin `human`.
///
/// # Errors
///
/// [`OpError`] only for failures unrelated to the decision itself; a claim the gate refuses is
/// [`ClaimOutcome::Refused`].
pub fn claim_paths(
    ctx: &RepoContext,
    seat: &SeatInfo,
    paths: &[String],
    cwd: &Path,
    now: Timestamp,
) -> Result<ClaimOutcome, OpError> {
    let db = ctx
        .db()
        .map_err(|why| OpError::Unavailable(why.to_owned()))?;
    if paths.is_empty() {
        return Ok(ClaimOutcome::Refused {
            reason_code: ReasonCode::Ok,
            message: "Name at least one file to claim.".to_owned(),
        });
    }
    let event = ToolEvent {
        spec: SpecVersion,
        harness: seat.seat.harness.clone(),
        harness_session_id: seat.seat.harness_session_id.clone(),
        phase: Phase::PreTool,
        cwd: cwd.to_string_lossy().into_owned(),
        tool: Some(Tool {
            name: "acs_claim".to_owned(),
            kind: ToolKind::Write,
        }),
        paths: paths.to_vec(),
        edit: None,
        command: None,
        result: None,
        prompt: None,
        raw: serde_json::Value::Null,
    };
    let norm = normalize(
        ctx,
        &event,
        NormalizeOptions {
            expand_write_dirs: true,
        },
    );
    let engine_seat = current_view(ctx, &seat.seat);
    let origin = if seat.seat.harness == HUMAN_HARNESS {
        ClaimOrigin::Human
    } else {
        ClaimOrigin::Agent
    };
    let run = run_gate(
        ctx,
        &GateParams {
            event: &event,
            seat: &engine_seat,
            norm: &norm,
            origin,
            now,
        },
    );
    let decision = run.output.decision;
    // Recorded so `acs_why` can explain a refusal; best effort.
    let _ = db.set_last_decision(
        &seat.seat.seat_id,
        &LastDecision {
            outcome: decision.outcome,
            reason_code: decision.reason_code,
            message: decision.message.clone(),
            at: now,
        },
    );
    if decision.reason_code == ReasonCode::ClaimConflict {
        let tool = ("acs_claim", ToolKind::Write);
        record_collision(
            db,
            &seat.seat.seat_id,
            tool,
            &norm.predicted,
            &decision.message,
            now,
        );
    }
    if decision.outcome != Outcome::Allow {
        return Ok(ClaimOutcome::Refused {
            reason_code: decision.reason_code,
            message: decision.message,
        });
    }
    if run.state_unavailable {
        return Ok(ClaimOutcome::Refused {
            reason_code: ReasonCode::StateUnavailable,
            message: "The state database is unavailable, so claims cannot be recorded. Run \
                      `acs doctor`."
                .to_owned(),
        });
    }
    let mut warnings = decision.warnings;
    if run.gate_off {
        warnings.push("`acs off` is active: nothing was claimed.".to_owned());
    }
    Ok(ClaimOutcome::Granted {
        paths: norm.repo_paths,
        claim_ids: decision.claims_acquired,
        warnings,
    })
}

/// The result of [`release_paths`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReleaseReport {
    /// Files whose claims the seat released.
    pub released: Vec<RepoPath>,
    /// Files the seat held no claim on.
    pub not_held: Vec<RepoPath>,
    /// Raw paths that are outside the repository.
    pub outside: Vec<String>,
}

/// Releases `seat`'s claims on `paths` (relative to `cwd` or absolute). Only the seat's own
/// claims are touched.
///
/// # Errors
///
/// [`OpError`] when the state database cannot be used.
pub fn release_paths(
    ctx: &RepoContext,
    seat: &SeatInfo,
    paths: &[String],
    cwd: &Path,
    now: Timestamp,
) -> Result<ReleaseReport, OpError> {
    let db = ctx
        .db()
        .map_err(|why| OpError::Unavailable(why.to_owned()))?;
    let (inside, outside) = resolve_for_claim(ctx, paths, cwd);
    let mine: Vec<HeldClaim> = db
        .live_claims(now)?
        .into_iter()
        .filter(|held| held.claim.seat_id == seat.seat.seat_id)
        .collect();
    let mut report = ReleaseReport {
        outside,
        ..ReleaseReport::default()
    };
    let mut ids = Vec::new();
    for path in inside {
        let held: Vec<&HeldClaim> = mine
            .iter()
            .filter(|h| h.claim.scope.path() == &path)
            .collect();
        if held.is_empty() {
            report.not_held.push(path);
        } else {
            ids.extend(held.iter().map(|h| h.claim.claim_id.clone()));
            report.released.push(path);
        }
    }
    if !ids.is_empty() {
        db.release_claims(&ids, ReleaseReason::Released, now)?;
    }
    Ok(report)
}

/// Records what the seat says it is doing (`acs_intent`). The text is trimmed and cut to
/// [`MAX_INTENT_CHARS`] characters; an empty text is refused.
///
/// # Errors
///
/// [`OpError::Invalid`] for an empty text; [`OpError`] when the database cannot be used.
pub fn set_intent(ctx: &RepoContext, seat_id: &SeatId, text: &str) -> Result<(), OpError> {
    let text: String = text.trim().chars().take(MAX_INTENT_CHARS).collect();
    if text.is_empty() {
        return Err(OpError::Invalid("the intent is empty".to_owned()));
    }
    let db = ctx
        .db()
        .map_err(|why| OpError::Unavailable(why.to_owned()))?;
    db.set_seat_intent(seat_id, &text)?;
    Ok(())
}

/// The seat's last pre-tool decision (MCP `acs_why`), if it has had one.
///
/// # Errors
///
/// [`OpError`] when the database cannot be used.
pub fn last_decision(ctx: &RepoContext, seat_id: &SeatId) -> Result<Option<LastDecision>, OpError> {
    let db = ctx
        .db()
        .map_err(|why| OpError::Unavailable(why.to_owned()))?;
    Ok(db.last_decision(seat_id)?)
}
