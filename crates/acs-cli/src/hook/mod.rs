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
use acs_core::{Decision, Outcome, Phase, ReasonCode, Timestamp, ToolEvent};
use acs_git::status::{ScanMode, ScanOptions};
use acs_store::{AcsHome, HomeError};

use crate::runtime::paths::absolute_cwd;
use crate::runtime::{RepoContext, roots};

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
    match event.phase {
        Phase::PreTool => pre::run(&ctx, event, env.now),
        Phase::PostTool => post::run(&ctx, event, env.now, &env.scan),
        Phase::SessionStart => session::start(&ctx, event, env.now),
        Phase::PromptSubmit => session::prompt(&ctx, event, env.now),
        Phase::SessionEnd => session::end(&ctx, event, env.now),
    }
}
