//! The request at the end of a task to propose lore (decision 0048, issue 80).
//!
//! Agents were asked to propose lore only in texts they do not read when they finish: the MCP
//! instructions, the tool's description and the usage skill. Nothing reached them at the moment
//! they had something to propose, and none ever did. So the first time a session's turn ends
//! after it changed files, ACS asks once: did you learn something the next agent needs?
//!
//! - **Only where lore is on.** `acs_lore_propose` is listed only then (decision 0034), so the
//!   request never names a tool the agent does not have.
//! - **Only after the session changed files.** A session that only read or answered a question
//!   has nothing of the kind to propose. The first allowed edit, write, delete, move, or shell
//!   command that is expected to write, marks the seat ([`after_change`]).
//! - **Once per seat.** Every harness renders the request as its "keep going" answer to `Stop`,
//!   so a second request would keep the agent from stopping. The flag is set to "shown" before
//!   the request is returned, and a flag that cannot be set or read means no request: an agent
//!   is never asked twice, and never kept in a loop.

use acs_core::{Decision, Outcome, ReasonCode, Timestamp, ToolEvent, ToolKind};
use acs_store::{SeatInfo, StateDb};

use crate::runtime::{RepoContext, lore_tool_name};

/// The seat changed files; the request waits for the end of its turn.
const DUE: &str = "due";
/// The request has been made.
const SHOWN: &str = "1";

fn flag(info: &SeatInfo) -> String {
    format!("lore_prompt:{}", info.seat.seat_id)
}

/// The request, with the tool's name in `harness`.
#[must_use]
pub fn text(harness: &str) -> String {
    let mut text = "ACS, before you finish: did you learn something about this code that the next \
                    agent working on it needs and would not see from the code itself (a trap, a \
                    rule, why something is the way it is)? If so, propose it as lore with \
                    `acs_lore_propose`: the scope is the paths it is about, the text is the fact. \
                    A person reviews it before any agent is shown it. Lore is for facts tied to \
                    code paths that stay true for every later session; the state of your current \
                    work belongs in the repository's hand-off notes or issues, not in lore. If \
                    there is nothing like that, just finish: ACS will not ask again in this \
                    session."
        .to_owned();
    if let Some(name) = lore_tool_name(harness) {
        text.push(' ');
        text.push_str(name);
    }
    text
}

/// At a pre-tool call that proceeds: marks the seat once its first change to files is allowed,
/// where lore is on. `writes` is whether the call is expected to write paths (for a shell
/// command, whether the normalizer predicted any).
pub fn after_change(
    ctx: &RepoContext,
    db: &StateDb,
    info: &SeatInfo,
    event: &ToolEvent,
    writes: bool,
    now: Timestamp,
) {
    if !ctx.features().lore {
        return;
    }
    let Some(kind) = event.tool.as_ref().map(|t| t.kind) else {
        return;
    };
    let changes = match kind {
        ToolKind::Write | ToolKind::Edit | ToolKind::Delete | ToolKind::Move => true,
        ToolKind::Shell => writes,
        ToolKind::Read | ToolKind::Other => false,
    };
    if !changes {
        return;
    }
    let flag = flag(info);
    // Unreadable counts as set: no request is better than a wrong one.
    if !matches!(db.flag(&flag), Ok(None)) {
        return;
    }
    let _ = db.set_flag(&flag, DUE, now);
}

/// `turn_end`: the request, once per seat, if the seat changed files and lore is on.
pub fn at_turn_end(ctx: &RepoContext, event: &ToolEvent, now: Timestamp) -> Decision {
    let mut decision = Decision::new(Outcome::Allow, ReasonCode::Ok, "");
    if !ctx.features().lore {
        return decision;
    }
    let Ok(db) = ctx.db() else { return decision };
    let Ok(Some(info)) = db.seat_by_session(&event.harness, &event.harness_session_id) else {
        return decision;
    };
    // Marked before it is said, in one statement: if the write fails the agent is not asked at
    // all, and of two racing hooks of the seat only one asks.
    if matches!(db.replace_flag(&flag(&info), DUE, SHOWN, now), Ok(true)) {
        decision.context.push(text(&event.harness));
    }
    decision
}

/// At session end: the flag has done its work.
pub fn forget(db: &StateDb, info: &SeatInfo) {
    let _ = db.clear_flag(&flag(info));
}
