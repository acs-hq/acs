//! The hook pipeline behind `acs hook`.
//!
//! [`run`] is the whole of one hook invocation: the adapter parses the harness payload into a
//! `ToolEvent`, the phase handler decides (pre-tool, post-tool, session start, prompt submit,
//! session end), and the adapter renders the `Decision` back into the harness's format. It never
//! fails: an unreadable payload is allowed with `unknown_payload`, a broken state database
//! degrades as SPEC section 11 says, and a bug (a panic) allows with a warning. A hook that
//! crashed would block or confuse the harness, which is worse than an unchecked call.
//!
//! - [`pre`]: parse, resolve repository and seat, normalize paths, run the guards and claims,
//!   take pre-images, collect context, record.
//! - [`post`]: find what changed (the event's paths, or a working-tree diff for shell commands),
//!   attribute it, store post-images, refresh heartbeats.
//! - [`session`]: session start, prompt submit and session end.
//! - [`markers`]: the small files in `pending/` that tie events together and talk to git hooks.
//!
//! A tool event that no switched-on feature needs ends early (decision 0035): [`unneeded`] says
//! which, and `skip` handles it without creating a seat or recording an event. The switches are
//! part of the policy, so this comes after the repository is opened.

pub mod markers;
pub mod post;
pub mod pre;
pub mod redact;
pub mod session;
mod snapshots;
mod unknown;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use acs_adapters::{Adapter, HookOutput};
use acs_core::{Decision, Features, Outcome, Phase, ReasonCode, Timestamp, ToolEvent, ToolKind};
use acs_git::status::{ScanMode, ScanOptions};
use acs_store::{AcsHome, HomeError, SeatInfo, StateDb};

use crate::runtime::paths::absolute_cwd;
use crate::runtime::{RepoContext, records, roots};

/// What a hook invocation takes from its process environment, injectable for tests.
#[derive(Debug)]
pub struct HookEnv {
    /// The process's working directory, used when the payload names none.
    pub process_cwd: PathBuf,
    /// `ACS_HOME`, resolved.
    pub home: Result<AcsHome, HomeError>,
    /// How post-tool shell scans behave.
    pub scan: ScanOptions,
    /// The time of the event.
    pub now: Timestamp,
}

impl HookEnv {
    /// The environment of this process: the current directory, `ACS_HOME`, and the scan settings
    /// from `ACS_SCAN` (`auto`, `full` or `scoped`) and `ACS_SCAN_MAX_ENTRIES`.
    #[must_use]
    pub fn from_process() -> Self {
        let mut scan = ScanOptions::default();
        match std::env::var("ACS_SCAN").as_deref() {
            Ok("full") => scan.mode = ScanMode::Full,
            Ok("scoped") => scan.mode = ScanMode::Scoped,
            _ => {}
        }
        if let Some(max) = std::env::var("ACS_SCAN_MAX_ENTRIES")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            scan.full_scan_max_entries = max;
        }
        Self {
            process_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            home: AcsHome::from_env(),
            scan,
            now: Timestamp::now(),
        }
    }
}

/// Runs one hook invocation: parse with `adapter`, decide, render.
#[must_use]
pub fn run(
    adapter: &dyn Adapter,
    phase: Option<Phase>,
    payload: &[u8],
    env: HookEnv,
) -> HookOutput {
    let parsed = adapter.parse(phase, payload);
    let caught = catch_unwind(AssertUnwindSafe(|| match &parsed {
        Ok(event) => handle(event, env),
        Err(error) => {
            let cwd = error
                .cwd
                .as_deref()
                .map_or_else(|| env.process_cwd.clone(), absolute_cwd);
            unknown::handle(adapter, error, payload, &cwd, env.home, env.now)
        }
    }));
    let decision = caught.unwrap_or_else(|_| internal_error());
    adapter.render(parsed.as_ref().ok(), &decision)
}

fn internal_error() -> Decision {
    let mut decision = Decision::new(Outcome::Allow, ReasonCode::Ok, "");
    decision.warnings.push(
        "ACS hit an internal error and did not check this call. Please report it.".to_owned(),
    );
    decision
}

fn handle(event: &ToolEvent, env: HookEnv) -> Decision {
    let cwd = if event.cwd.is_empty() {
        env.process_cwd.clone()
    } else {
        absolute_cwd(&event.cwd)
    };
    // Not a repository ACS manages: nothing to coordinate.
    let Ok(session) = acs_git::discover(&cwd) else {
        return Decision::new(Outcome::Allow, ReasonCode::Ok, "");
    };
    let session_tree = session.work_tree.clone();
    // The event is judged in the work tree its files are in, which is not always the session's.
    let layout = roots::for_event(event, &cwd, session);
    let ctx = match RepoContext::open_layout(layout, env.home) {
        Ok(mut ctx) => {
            ctx.session_tree = session_tree;
            ctx
        }
        Err(err) => {
            let mut decision = Decision::new(Outcome::Allow, ReasonCode::Ok, "");
            decision
                .warnings
                .push(format!("ACS could not open this repository: {err}"));
            return decision;
        }
    };
    if unneeded(ctx.features(), event) {
        return skip(&ctx, event, env.now);
    }
    match event.phase {
        Phase::PreTool => pre::run(&ctx, event, env.now),
        Phase::PostTool => post::run(&ctx, event, env.now, &env.scan),
        Phase::SessionStart => session::start(&ctx, event, env.now),
        Phase::PromptSubmit => session::prompt(&ctx, event, env.now),
        Phase::SessionEnd => session::end(&ctx, event, env.now),
    }
}

/// The longest a live seat goes without being marked as seen while its events are skipped.
const KEEP_ALIVE_SECS: i64 = 300;

/// Whether no switched-on feature needs this tool event, so that it can end in [`skip`].
///
/// - Before a read, only lore has work to do: a read is never denied and claims nothing.
/// - After a read, claims refresh their heartbeat and sync checks whether it is due.
/// - After any other tool, history attributes the change, claims refresh and sync checks.
///
/// Never true before a write or a shell command (ACS's protection of itself is not a feature),
/// for a call whose tool is unknown, or for a session event: those decide for themselves in
/// [`session`].
#[must_use]
pub fn unneeded(features: Features, event: &ToolEvent) -> bool {
    let Some(tool) = &event.tool else {
        return false;
    };
    match (event.phase, tool.kind) {
        (Phase::PreTool, ToolKind::Read) => !features.lore,
        (Phase::PostTool, ToolKind::Read) => !features.claims && !features.sync,
        (Phase::PostTool, _) => !features.history && !features.claims && !features.sync,
        _ => false,
    }
}

/// Handles a tool event that [`unneeded`] named: no seat is created and no event is recorded.
/// What is left is one read of the seat, the tool marker the pre-tool step left for a write or
/// a shell command, and [`keep_alive`].
fn skip(ctx: &RepoContext, event: &ToolEvent, now: Timestamp) -> Decision {
    let allow = Decision::new(Outcome::Allow, ReasonCode::Ok, "");
    let Ok(db) = ctx.db() else { return allow };
    let Ok(Some(info)) = db.seat_by_session(&event.harness, &event.harness_session_id) else {
        return allow;
    };
    let read = event
        .tool
        .as_ref()
        .is_some_and(|t| t.kind == ToolKind::Read);
    if event.phase == Phase::PostTool && !read {
        let _ = markers::take_tool_marker(ctx, &info.seat.seat_id);
    }
    keep_alive(db, &info, now);
    allow
}

/// Marks a live seat as seen when it has not been for [`KEEP_ALIVE_SECS`] (or a quarter of the
/// time after which a silent session counts as crashed, when that is shorter).
///
/// A session whose events are all skipped would otherwise look dead: `acs sync` would record it
/// as crashed after an hour, and the MCP server would stop taking it for the most recently
/// active session. Writing on every skipped event would cost most of what skipping saves, so
/// the seat's `last_seen_at` may be this far behind. An ended seat stays ended: the next event
/// that is not skipped revives it.
pub(crate) fn keep_alive(db: &StateDb, info: &SeatInfo, now: Timestamp) {
    if info.ended_at.is_some() {
        return;
    }
    let every = KEEP_ALIVE_SECS.min(records::crash_after_secs() / 4);
    let silent = now
        .as_second()
        .saturating_sub(info.seat.last_seen_at.as_second());
    if silent >= every {
        let _ = db.touch_seat(&info.seat.seat_id, now);
    }
}
