//! Resolving the seat an event or a CLI call belongs to.
//!
//! - An **agent seat** is one harness session: `(harness, harness_session_id)` from the event. The
//!   pilot's email is read from git only when the seat is created.
//! - A **human seat** is the seat the CLI uses for a person's own claims (`ClaimOrigin::Human`):
//!   harness [`HUMAN_HARNESS`], one per pilot and work tree. It never ends; crashed-session
//!   detection ignores that harness.

use acs_core::{ContentHash, Seat, SeatId, Timestamp};
use acs_store::{NewSeat, SeatInfo, StateDb, StoreError};

use super::RepoContext;

/// The harness name of human seats.
pub const HUMAN_HARNESS: &str = "human";

/// A resolved seat and whether the call created it.
#[derive(Debug, Clone)]
pub struct ResolvedSeat {
    /// The seat with its session id and intent.
    pub info: SeatInfo,
    /// True when this call created the seat.
    pub created: bool,
}

impl ResolvedSeat {
    /// The seat as the engine should see it: the stored seat with `branch` replaced by what
    /// `HEAD` says right now (the branch can change during a session, and protected-branch
    /// checks must use the current one).
    #[must_use]
    pub fn engine_seat(&self, ctx: &RepoContext) -> Seat {
        current_view(ctx, &self.info.seat)
    }
}

/// `seat` with its branch refreshed from `HEAD`.
#[must_use]
pub fn current_view(ctx: &RepoContext, seat: &Seat) -> Seat {
    let mut seat = seat.clone();
    seat.branch = acs_git::repo::current_branch(&ctx.layout.git_dir);
    seat
}

/// The seat of a harness session, created on its first event, and marked as seen at `now`.
///
/// # Errors
///
/// Any [`StoreError`]; the pipeline treats it as `state_unavailable`.
pub fn agent_seat(
    ctx: &RepoContext,
    db: &StateDb,
    harness: &str,
    harness_session_id: &str,
    now: Timestamp,
) -> Result<ResolvedSeat, StoreError> {
    if let Some(info) = db.seat_by_session(harness, harness_session_id)?
        && info.ended_at.is_none()
    {
        db.touch_seat(&info.seat.seat_id, now)?;
        return Ok(ResolvedSeat {
            info,
            created: false,
        });
    }
    // New (or resumed after its end): the only place that starts `git`.
    let pilot = acs_git::repo::pilot_email_or_fallback(&ctx.layout.work_tree);
    create(ctx, db, harness, harness_session_id, &pilot, now)
}

/// The human seat of `pilot` in this work tree, created on first use, marked as seen at `now`.
///
/// # Errors
///
/// Any [`StoreError`].
pub fn human_seat(
    ctx: &RepoContext,
    db: &StateDb,
    pilot: &str,
    now: Timestamp,
) -> Result<ResolvedSeat, StoreError> {
    let key = ContentHash::of(format!("{pilot}\n{}", ctx.worktree_str()).as_bytes());
    let session = format!("human-{}", &key.as_str()[..16]);
    let resolved = create(ctx, db, HUMAN_HARNESS, &session, pilot, now)?;
    db.touch_seat(&resolved.info.seat.seat_id, now)?;
    Ok(resolved)
}

/// The human seat of the repository's configured pilot (`git config user.email`, or the
/// fallback identity). Starts `git` once.
///
/// # Errors
///
/// Any [`StoreError`].
pub fn current_human_seat(
    ctx: &RepoContext,
    db: &StateDb,
    now: Timestamp,
) -> Result<ResolvedSeat, StoreError> {
    let pilot = acs_git::repo::pilot_email_or_fallback(&ctx.layout.work_tree);
    human_seat(ctx, db, &pilot, now)
}

/// The seat with this id (live or ended), if any.
///
/// # Errors
///
/// Any [`StoreError`].
pub fn seat_by_id(db: &StateDb, seat_id: &SeatId) -> Result<Option<SeatInfo>, StoreError> {
    db.seat_info(seat_id)
}

fn create(
    ctx: &RepoContext,
    db: &StateDb,
    harness: &str,
    harness_session_id: &str,
    pilot: &str,
    now: Timestamp,
) -> Result<ResolvedSeat, StoreError> {
    let branch = acs_git::repo::current_branch(&ctx.layout.git_dir);
    let worktree = ctx.worktree_str();
    let target = ctx
        .policy
        .policy
        .files()
        .config
        .default_target_branch
        .clone();
    let (info, created) = db.get_or_create_seat_info(
        &NewSeat {
            harness,
            harness_session_id,
            pilot,
            worktree: &worktree,
            branch: branch.as_deref(),
            target_branch: &target,
        },
        now,
    )?;
    Ok(ResolvedSeat { info, created })
}
