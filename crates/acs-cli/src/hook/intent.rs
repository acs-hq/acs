//! The reminder for a session that changes the repository without having said what it is doing.
//!
//! The session-start message asks every agent for `acs_status` and `acs_intent` before it
//! works. An agent that skipped that gets one reminder per session, when it first edits,
//! deletes, moves or runs a shell command with no intent on record (issue 73: without one, the
//! session's record, `acs log`, `acs blame` and what other agents are told about a holder all
//! say nothing about what the session was for).
//!
//! Where the reminder can travel differs by harness. Claude Code shows the context of an allowed
//! pre-tool call to the model, so the reminder goes out with that call. Antigravity has no
//! channel there: the reminder waits in a flag and goes out with the next `PreInvocation`, which
//! comes before the model's next step.

use acs_core::{Timestamp, ToolEvent, ToolKind};
use acs_store::{SeatInfo, StateDb};

use crate::runtime::tool_names;

/// The value of the flag while the reminder waits for a prompt-submit event.
const DUE: &str = "due";
/// The value of the flag once the reminder has been shown.
const SHOWN: &str = "1";

fn flag(info: &SeatInfo) -> String {
    format!("intent_reminder:{}", info.seat.seat_id)
}

/// Whether `harness` shows the context of an allowed pre-tool call to its model.
fn pre_tool_context_reaches_the_model(harness: &str) -> bool {
    harness != "antigravity"
}

fn text(harness: &str) -> String {
    let mut text = "ACS: this session is changing the repository and has not said what it is \
                    doing. Call `acs_intent` now with one sentence on your task (other agents \
                    and your pilot see it), then carry on. You will not be reminded again."
        .to_owned();
    if let Some(names) = tool_names(harness) {
        text.push(' ');
        text.push_str(names);
    }
    text
}

/// At a pre-tool call that proceeds: the reminder to add to its context, if this is the
/// session's first call that can change files and it has given no intent. For a harness that
/// cannot show it here, the reminder is left for [`at_prompt`] and nothing is returned.
pub fn before_tool(
    db: &StateDb,
    info: &SeatInfo,
    event: &ToolEvent,
    now: Timestamp,
) -> Option<String> {
    if info.intent.is_some() {
        return None;
    }
    let kind = event.tool.as_ref()?.kind;
    if !matches!(
        kind,
        ToolKind::Write | ToolKind::Edit | ToolKind::Delete | ToolKind::Move | ToolKind::Shell
    ) {
        return None;
    }
    let flag = flag(info);
    // A flag that cannot be read counts as set: better no reminder than one with every call.
    if db.flag(&flag).ok()?.is_some() {
        return None;
    }
    let here = pre_tool_context_reaches_the_model(&event.harness);
    db.set_flag(&flag, if here { SHOWN } else { DUE }, now)
        .ok()?;
    here.then(|| text(&event.harness))
}

/// At a prompt-submit event: the reminder [`before_tool`] left, if the session still has no
/// intent.
pub fn at_prompt(db: &StateDb, info: &SeatInfo, now: Timestamp) -> Option<String> {
    if info.intent.is_some() {
        return None;
    }
    let flag = flag(info);
    if db.flag(&flag).ok()?.as_deref() != Some(DUE) {
        return None;
    }
    db.set_flag(&flag, SHOWN, now).ok()?;
    Some(text(&info.seat.harness))
}

/// At session end: the flag has done its work.
pub fn forget(db: &StateDb, info: &SeatInfo) {
    let _ = db.clear_flag(&flag(info));
}
