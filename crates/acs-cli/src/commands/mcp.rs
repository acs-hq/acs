//! `acs mcp`: the MCP server over stdio (WP2.6).
//!
//! [`RuntimeBackend`] implements `acs_mcp::Backend` with the shared operations of
//! [`crate::runtime`], so a claim made through the MCP tool is judged exactly like an agent's edit
//! and a refusal carries the engine's own wording. The repository is opened from the server's
//! directory on every call (opening is cheap, and the state can change between calls).
//!
//! # Seat binding (decision 0011 section 5)
//!
//! The server process belongs to one harness session but is not always told which. Per call, in
//! order:
//!
//! 1. the session id the protocol layer extracted (`CLAUDE_CODE_SESSION_ID`, or Antigravity's
//!    `root_conversation_id`), if it names a **live** seat of the caller's harness;
//! 2. else the most recently seen live seat of that harness (any agent harness when it is
//!    unknown) whose work tree is the server's work tree;
//! 3. else no seat: `acs_status` still answers, the other tools say that no ACS session has
//!    started yet and that the agent's first file edit will start one.
//!
//! The first step is re-checked on every call because a session can end, and the id from the
//! environment can be stale (Claude Code after `--continue` or `/clear`).

use std::path::PathBuf;
use std::process::ExitCode;

use acs_core::{Outcome, Phase, Timestamp};
use acs_mcp::{
    Backend, BackendError, BindingSettings, Caller, ClaimInfo, ClaimReply, ClaimRequest,
    DecisionInfo, IntentReply, IntentRequest, LoreProposeReply, LoreProposeRequest, ReleaseReply,
    ReleaseRequest, SeatInfo, ServerOptions, StatusReply, WhyReply,
};
use acs_store::{AcsHome, SeatInfo as StoredSeat, StateDb};
use clap::Args;

use crate::runtime::seats::HUMAN_HARNESS;
use crate::runtime::{
    ClaimOutcome, ClaimView, MAX_INTENT_CHARS, OpError, RepoContext, StatusView, claim_paths,
    last_decision, lore, release_paths, set_intent, status,
};

/// Arguments of `acs mcp`.
#[derive(Debug, Args)]
#[command(
    after_help = "Register it with a harness as a stdio server, for example:\n  \
                  {\"mcpServers\": {\"acs\": {\"command\": \"acs\", \"args\": [\"mcp\", \"--harness\", \"claude-code\"]}}}"
)]
pub struct McpArgs {
    /// The harness this server belongs to (`claude-code`, `antigravity`, ...). Narrows which
    /// seats the tools bind to; inferred from the client when omitted.
    #[arg(long, value_name = "NAME")]
    pub harness: Option<String>,
    /// Also return results as MCP `structuredContent`. Claude Code shows the model that JSON
    /// instead of the text written for it, so leave this off unless the client needs it.
    #[arg(long)]
    pub structured_content: bool,
}

/// Runs the MCP server on stdin and stdout until the client closes the pipe.
#[must_use]
pub fn run(args: &McpArgs) -> ExitCode {
    let backend = RuntimeBackend::from_process();
    let options = ServerOptions {
        binding: BindingSettings::from_process(args.harness.clone()),
        structured_content: args.structured_content,
    };
    match acs_mcp::serve_stdio(backend, options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("acs mcp: {err}");
            ExitCode::from(1)
        }
    }
}

/// Where `ACS_HOME` comes from.
#[derive(Debug, Clone)]
enum HomeSource {
    /// The process environment, resolved on every call.
    Env,
    /// A fixed directory (tests).
    Dir(PathBuf),
}

/// The [`Backend`] of `acs mcp`: the shared runtime operations on the repository around the
/// server's directory.
#[derive(Debug, Clone)]
pub struct RuntimeBackend {
    /// Directories to look for the repository in, first match wins.
    dirs: Vec<PathBuf>,
    home: HomeSource,
}

impl RuntimeBackend {
    /// The backend of this process: the repository around the current directory, else around
    /// `CLAUDE_PROJECT_DIR`.
    #[must_use]
    pub fn from_process() -> Self {
        let mut dirs = Vec::new();
        if let Ok(cwd) = std::env::current_dir() {
            dirs.push(cwd);
        }
        if let Some(project) = std::env::var_os("CLAUDE_PROJECT_DIR") {
            dirs.push(PathBuf::from(project));
        }
        Self {
            dirs,
            home: HomeSource::Env,
        }
    }

    /// A backend for the repository around `dir` with an explicit `ACS_HOME` (tests).
    #[must_use]
    pub fn at(dir: impl Into<PathBuf>, home: impl Into<PathBuf>) -> Self {
        Self {
            dirs: vec![dir.into()],
            home: HomeSource::Dir(home.into()),
        }
    }

    fn open(&self) -> Result<RepoContext, BackendError> {
        let mut last = None;
        for dir in &self.dirs {
            let home = match &self.home {
                HomeSource::Env => AcsHome::from_env(),
                HomeSource::Dir(dir) => AcsHome::at(dir),
            };
            match RepoContext::open_with(dir, home) {
                Ok(ctx) => return Ok(ctx),
                Err(err) => last = Some(err),
            }
        }
        Err(BackendError::Unavailable(format!(
            "ACS cannot find a git repository for this server ({}). Start the agent inside a \
             repository.",
            last.map_or_else(|| "no directory".to_owned(), |err| err.to_string())
        )))
    }
}

/// How a call was tied to a seat.
struct Bound {
    info: StoredSeat,
    /// A note for the agent when the binding is a guess among several live sessions.
    ambiguity: Option<String>,
}

/// Binds `caller` to a live seat (see the module documentation).
fn bind(ctx: &RepoContext, db: &StateDb, caller: &Caller) -> Result<Option<Bound>, OpError> {
    let live: Vec<StoredSeat> = db
        .live_seats()?
        .into_iter()
        .filter(|s| s.seat.harness != HUMAN_HARNESS)
        .filter(|s| {
            caller
                .harness
                .as_deref()
                .is_none_or(|harness| s.seat.harness == harness)
        })
        .collect();
    if let Some(id) = caller.session_id.as_deref()
        && let Some(info) = live.iter().find(|s| s.seat.harness_session_id == id)
    {
        return Ok(Some(Bound {
            info: info.clone(),
            ambiguity: None,
        }));
    }
    let worktree = ctx.worktree_str();
    // `live_seats` is ordered by `last_seen_at`, newest first.
    let mut here = live.into_iter().filter(|s| s.seat.worktree == worktree);
    let Some(first) = here.next() else {
        return Ok(None);
    };
    let others = here.count();
    let ambiguity = (others > 0).then(|| {
        format!(
            "{} sessions are active in this repository and this server could not tell which one \
             you are, so it used the most recently active one (session {}).",
            others + 1,
            first.session_id
        )
    });
    Ok(Some(Bound {
        info: first,
        ambiguity,
    }))
}

/// The text for an agent that has no seat yet.
fn no_session_text(caller: &Caller) -> String {
    let who = caller
        .harness
        .as_deref()
        .map_or(String::new(), |h| format!(" for {h}"));
    format!(
        "No active ACS session{who} yet. ACS starts one when your first file edit or shell \
         command runs; try again after that."
    )
}

fn op_error(err: &OpError) -> BackendError {
    match err {
        OpError::Invalid(message) => BackendError::Invalid(message.clone()),
        OpError::Unavailable(why) => BackendError::Unavailable(format!(
            "ACS cannot use its state database ({why}). Ask the user to run `acs doctor`."
        )),
        OpError::Store(err) => BackendError::Unavailable(format!(
            "ACS failed to read or write its state ({err}). Ask the user to run `acs doctor`."
        )),
    }
}

/// The repository, its database and the bound seat of one call.
struct Call {
    ctx: RepoContext,
    bound: Option<Bound>,
}

impl RuntimeBackend {
    /// Opens the repository and binds the caller. A database that cannot be used leaves the
    /// seat unbound (the operations then report it).
    fn start(&self, caller: &Caller) -> Result<Call, BackendError> {
        let ctx = self.open()?;
        let bound = match ctx.db() {
            Ok(db) => bind(&ctx, db, caller).map_err(|err| op_error(&err))?,
            Err(_) => None,
        };
        Ok(Call { ctx, bound })
    }

    /// Like [`RuntimeBackend::start`], but a missing seat is an error with guidance.
    fn start_bound(&self, caller: &Caller) -> Result<(RepoContext, Bound), BackendError> {
        let call = self.start(caller)?;
        match call.bound {
            Some(bound) => Ok((call.ctx, bound)),
            None => {
                if let Err(why) = call.ctx.db() {
                    return Err(op_error(&OpError::Unavailable(why.to_owned())));
                }
                Err(BackendError::NoSession(no_session_text(caller)))
            }
        }
    }
}

/// An RFC 3339 time without the fraction: shorter for an agent to read.
fn whole_seconds(at: Timestamp) -> String {
    Timestamp::from_second(at.as_second()).map_or_else(|_| at.to_string(), |t| t.to_string())
}

const fn outcome_str(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Allow => "allow",
        Outcome::Deny => "deny",
        Outcome::Ask => "ask",
        Outcome::Queue => "queue",
    }
}

fn claim_info(view: &ClaimView) -> ClaimInfo {
    ClaimInfo {
        path: view.path.clone(),
        symbol_path: view.symbol_path.clone(),
        holder: view.pilot.clone(),
        harness: view.harness.clone(),
        source: view.source.to_owned(),
        expires_at: whole_seconds(view.expires_at),
    }
}

fn status_reply(view: &StatusView, call_note: Option<String>, caller: &Caller) -> StatusReply {
    let mut notes = Vec::new();
    if view.seat.is_none() {
        notes.push(no_session_text(caller));
    }
    notes.extend(call_note);
    if let Some(why) = &view.state_unavailable {
        notes.push(format!(
            "ACS cannot use its state database ({why}), so claims are not listed. Ask the user \
             to run `acs doctor`."
        ));
    }
    if view.remote.stale {
        notes.push(
            "The view of other machines' claims is stale (the last sync is old or failed), so \
             claims made elsewhere may be missing."
                .to_owned(),
        );
    }
    notes.extend(view.policy_warnings.iter().cloned());
    StatusReply {
        worktree: view.worktree.clone(),
        branch: view.branch.clone(),
        target_branch: view.target_branch.clone(),
        gate_off: view.gate_off,
        seat: view.seat.as_ref().map(|s| SeatInfo {
            seat_id: s.seat_id.clone(),
            session_id: s.session_id.clone(),
            harness: s.harness.clone(),
            pilot: s.pilot.clone(),
            intent: s.intent.clone(),
        }),
        my_claims: view.my_claims.iter().map(claim_info).collect(),
        other_claims: view.other_claims.iter().map(claim_info).collect(),
        sealed: view.sealed.clone(),
        notes,
    }
}

impl Backend for RuntimeBackend {
    fn status(&self, caller: &Caller) -> Result<StatusReply, BackendError> {
        let call = self.start(caller)?;
        let seat = call.bound.as_ref().map(|b| &b.info);
        let view = status(&call.ctx, seat, Timestamp::now());
        let note = call.bound.as_ref().and_then(|b| b.ambiguity.clone());
        Ok(status_reply(&view, note, caller))
    }

    fn claim(&self, caller: &Caller, request: &ClaimRequest) -> Result<ClaimReply, BackendError> {
        let (ctx, bound) = self.start_bound(caller)?;
        let now = Timestamp::now();
        let outcome = claim_paths(
            &ctx,
            &bound.info,
            &request.paths,
            &ctx.layout.work_tree,
            now,
        )
        .map_err(|err| op_error(&err))?;
        Ok(match outcome {
            ClaimOutcome::Granted {
                paths,
                mut warnings,
                ..
            } => {
                warnings.extend(bound.ambiguity);
                let paths: Vec<String> = paths.iter().map(|p| p.as_str().to_owned()).collect();
                let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                let lore = lore::inject(&ctx, &bound.info.seat.seat_id, &refs, now);
                ClaimReply::Granted {
                    paths,
                    warnings,
                    lore,
                }
            }
            ClaimOutcome::Refused {
                reason_code,
                message,
            } => ClaimReply::Refused {
                reason_code: reason_code.as_str().to_owned(),
                message,
            },
        })
    }

    fn release(
        &self,
        caller: &Caller,
        request: &ReleaseRequest,
    ) -> Result<ReleaseReply, BackendError> {
        let (ctx, bound) = self.start_bound(caller)?;
        let report = release_paths(
            &ctx,
            &bound.info,
            &request.paths,
            &ctx.layout.work_tree,
            Timestamp::now(),
        )
        .map_err(|err| op_error(&err))?;
        let names = |paths: Vec<acs_core::RepoPath>| -> Vec<String> {
            paths.iter().map(|p| p.as_str().to_owned()).collect()
        };
        Ok(ReleaseReply {
            released: names(report.released),
            not_held: names(report.not_held),
            outside: report.outside,
        })
    }

    fn intent(
        &self,
        caller: &Caller,
        request: &IntentRequest,
    ) -> Result<IntentReply, BackendError> {
        let (ctx, bound) = self.start_bound(caller)?;
        set_intent(&ctx, &bound.info.seat.seat_id, &request.text).map_err(|err| op_error(&err))?;
        Ok(IntentReply {
            intent: request.text.trim().chars().take(MAX_INTENT_CHARS).collect(),
        })
    }

    fn why(&self, caller: &Caller) -> Result<WhyReply, BackendError> {
        let (ctx, bound) = self.start_bound(caller)?;
        let seat_id = &bound.info.seat.seat_id;
        let last = last_decision(&ctx, seat_id).map_err(|err| op_error(&err))?;
        // The harness runs the pre-tool hook for the `acs_why` call itself before the tool
        // runs, and that records an uneventful "allow" as the last decision. When that is what
        // is stored, the interesting decision is the last blocked one in the event log.
        let uneventful = last
            .as_ref()
            .is_none_or(|d| d.outcome == Outcome::Allow && d.message.trim().is_empty());
        if uneventful && let Some(earlier) = last_block(&ctx, &bound.info) {
            return Ok(WhyReply {
                decision: Some(earlier),
            });
        }
        Ok(WhyReply {
            decision: last.map(|d| DecisionInfo {
                outcome: outcome_str(d.outcome).to_owned(),
                reason_code: d.reason_code.as_str().to_owned(),
                message: d.message,
                at: whole_seconds(d.at),
            }),
        })
    }

    fn lore_propose(
        &self,
        caller: &Caller,
        request: &LoreProposeRequest,
    ) -> Result<LoreProposeReply, BackendError> {
        let (ctx, bound) = self.start_bound(caller)?;
        let draft = acs_lore::propose::Draft {
            scope: request.scope.clone(),
            body: request.text.clone(),
            replaces: request.replaces.clone(),
            why: request.why.clone(),
        };
        let proposed = lore::propose(&ctx, &bound.info, &draft, Timestamp::now())
            .map_err(|err| op_error(&err))?;
        Ok(LoreProposeReply {
            proposal_id: proposed.proposal.proposal_id,
            scope: proposed.proposal.scope,
            replaces: proposed.proposal.replaces.map(|id| id.to_string()),
            reviewer: proposed.reviewer,
        })
    }
}

/// The seat's most recent denied or asked pre-tool event, described from the event log (which
/// keeps the reason and the paths, not the wording shown at the time).
fn last_block(ctx: &RepoContext, seat: &StoredSeat) -> Option<DecisionInfo> {
    let events = ctx.db().ok()?.seat_events(&seat.seat.seat_id).ok()?;
    let event = events.into_iter().rev().find(|e| {
        e.phase == Phase::PreTool && matches!(e.outcome, Some(Outcome::Deny | Outcome::Ask))
    })?;
    let outcome = event.outcome?;
    let code = event.reason_code?;
    let what = event.tool_name.as_deref().unwrap_or("a tool call");
    let paths = if event.paths.is_empty() {
        String::new()
    } else {
        format!(
            " on {}",
            event
                .paths
                .iter()
                .take(5)
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Some(DecisionInfo {
        outcome: outcome_str(outcome).to_owned(),
        reason_code: code.as_str().to_owned(),
        message: format!(
            "Earlier, {what}{paths} was blocked ({code}). The original wording was not kept: \
             repeat the action to see it again, or call acs_status to see claims and sealed \
             paths."
        ),
        at: whole_seconds(event.at),
    })
}
