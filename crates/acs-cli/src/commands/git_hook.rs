//! `acs git-hook <name> [args...]`: the entry point of the git hooks that `acs_git::hooks`
//! installs (decision 0015).
//!
//! The backstop for the hook pipeline: it catches what an agent or a person does with plain `git`
//! outside any harness tool call.
//!
//! - **`pre-commit`** rejects a commit that stages sealed paths (agent commits only) or paths
//!   claimed by someone it should not step on: for an agent, any other seat; for a human, seats in
//!   another work tree or on another machine (claims held by agents working in this very work tree
//!   are the normal "agent edits, human commits" flow).
//! - **`prepare-commit-msg`** adds `ACS-Session` trailers (SPEC section 7).
//! - **`pre-push`** rejects force pushes and deletions of protected branches by agent sessions.
//!
//! Who is an agent: a fresh pending-commit marker for the work tree (`pre-commit`,
//! `prepare-commit-msg`); a fresh pending-push marker or the harness environment (`CLAUDECODE=1`,
//! `ANTIGRAVITY_CONVERSATION_ID`) for `pre-push`.
//!
//! The backstop **fails open**: any problem reading state, and any bug, allows the git operation
//! with a warning on stderr. Only a clear violation exits non-zero. Repositories without
//! `.acs/policy` (not initialised for ACS, or a global hooks directory) are left alone silently.

use std::collections::HashMap;
use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use acs_core::gate::{ClaimSource, HeldClaim};
use acs_core::{RepoPath, SeatId, Timestamp, ZoneKind};
use acs_git::hooks::query::{self, StagedChange};
use acs_git::trailers::{self, Trailer};
use acs_store::{FLAG_GATE_OFF, SeatInfo, StateDb};
use clap::Args;

use crate::hook::HookEnv;
use crate::hook::markers::{self, CommitMarker};
use crate::runtime::RepoContext;
use crate::runtime::seats::HUMAN_HARNESS;

/// The most paths one rejection message names.
const SHOWN: usize = 8;
/// The most staged paths examined for attribution trailers.
const MAX_ATTRIBUTION_PATHS: usize = 2_000;

/// Arguments of `acs git-hook`.
#[derive(Debug, Args)]
#[command(
    after_help = "Called by the hooks `acs init` installs; not meant to be run by hand.\n\
                  Hooks: pre-commit, prepare-commit-msg <file> [source] [sha], pre-push <remote> <url>"
)]
pub struct GitHookArgs {
    /// The git hook that is running.
    #[arg(value_name = "HOOK")]
    pub name: String,
    /// The arguments git passed to the hook.
    #[arg(
        value_name = "ARGS",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub args: Vec<String>,
}

/// What a hook decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Let git proceed; the strings are warnings for stderr.
    Allow(Vec<String>),
    /// Make git stop, with this message.
    Reject(String),
}

impl Verdict {
    fn allow() -> Self {
        Self::Allow(Vec::new())
    }

    fn warn(message: impl Into<String>) -> Self {
        Self::Allow(vec![message.into()])
    }
}

/// Runs the hook named in `args` against this process's environment and standard input, prints
/// warnings and rejections on stderr, and returns the exit code (0 allows, 1 rejects).
#[must_use]
pub fn run(args: &GitHookArgs) -> ExitCode {
    let mut stdin = String::new();
    if args.name == "pre-push" {
        let mut bytes = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut bytes);
        stdin = String::from_utf8_lossy(&bytes).into_owned();
    }
    let agent_env = agent_environment();
    let caught = catch_unwind(AssertUnwindSafe(|| {
        handle(
            &args.name,
            &args.args,
            &stdin,
            HookEnv::from_process(),
            agent_env,
        )
    }));
    match caught {
        Ok(Verdict::Allow(warnings)) => {
            for warning in warnings {
                eprintln!("acs: warning: {warning}");
            }
            ExitCode::SUCCESS
        }
        Ok(Verdict::Reject(message)) => {
            eprintln!("{message}");
            ExitCode::from(1)
        }
        Err(_) => {
            eprintln!("acs: warning: the git hook hit an internal error and did not check this.");
            ExitCode::SUCCESS
        }
    }
}

/// Whether the harness environment says this process runs inside an agent's tool call: Claude
/// Code sets `CLAUDECODE=1` for its Bash tool; Antigravity's hook processes carry
/// `ANTIGRAVITY_CONVERSATION_ID` (reported, unverified; docs/adapters/antigravity.md).
#[must_use]
pub fn agent_environment() -> bool {
    std::env::var("CLAUDECODE").is_ok_and(|v| v == "1")
        || std::env::var("ANTIGRAVITY_CONVERSATION_ID").is_ok_and(|v| !v.is_empty())
}

/// Runs one hook. `stdin` is what git wrote on the hook's standard input (`pre-push` only).
#[must_use]
pub fn handle(name: &str, args: &[String], stdin: &str, env: HookEnv, agent_env: bool) -> Verdict {
    let Ok(layout) = acs_git::discover(&env.process_cwd) else {
        return Verdict::allow();
    };
    if !layout.work_tree.join(".acs").join("policy").is_dir() {
        return Verdict::allow();
    }
    let ctx = match RepoContext::open_with(&layout.work_tree, env.home) {
        Ok(ctx) => ctx,
        Err(err) => return Verdict::warn(format!("could not open this repository: {err}")),
    };
    match name {
        "pre-commit" => pre_commit(&ctx, env.now),
        "prepare-commit-msg" => prepare_commit_msg(&ctx, args, env.now),
        "pre-push" => pre_push(&ctx, stdin, env.now, agent_env),
        other => Verdict::warn(format!("unknown git hook `{other}`")),
    }
}

fn pending_dir(ctx: &RepoContext) -> Option<std::path::PathBuf> {
    ctx.repo_home.as_ref().map(acs_store::RepoHome::pending_dir)
}

// ---------------------------------------------------------------------------------------------
// pre-commit
// ---------------------------------------------------------------------------------------------

/// A staged path that may not be committed, and why.
struct Finding {
    path: String,
    reason: String,
    sealed: bool,
}

fn pre_commit(ctx: &RepoContext, now: Timestamp) -> Verdict {
    let staged = match query::staged_changes(&ctx.layout.work_tree) {
        Ok(staged) => staged,
        Err(err) => return Verdict::warn(format!("could not list the staged files: {err}")),
    };
    if staged.is_empty() {
        return Verdict::allow();
    }
    let paths: Vec<RepoPath> = staged
        .iter()
        .filter_map(|c| RepoPath::try_from(c.path.clone()).ok())
        .collect();
    let pending = pending_dir(ctx);
    let marker = pending
        .as_deref()
        .and_then(|p| markers::read_commit_marker(p, &ctx.layout.work_tree, now));

    let mut warnings = Vec::new();
    let mut findings: Vec<Finding> = Vec::new();
    if marker.is_some() {
        findings.extend(sealed_findings(ctx, &paths));
    }
    let gate_off = ctx
        .db()
        .ok()
        .is_some_and(|db| db.flag_is_set(FLAG_GATE_OFF).unwrap_or(false));
    if !gate_off {
        match ctx.db() {
            Ok(db) => match claim_findings(ctx, db, &paths, marker.as_ref(), now) {
                Ok((found, stale)) => {
                    findings.extend(found);
                    if stale {
                        warnings.push(
                            "the shared claims view is out of date, so claims held on other \
                             machines were not checked."
                                .to_owned(),
                        );
                    }
                }
                Err(err) => warnings.push(format!("could not read claims: {err}")),
            },
            Err(err) => warnings.push(format!(
                "ACS state is unavailable ({err}); claims were not checked."
            )),
        }
    }
    if findings.is_empty() {
        return Verdict::Allow(warnings);
    }
    if marker.is_some() {
        // The agent's commit attempt is over: do not let a human's next commit look like it.
        if let Some(p) = &pending {
            let _ = markers::take_commit_marker(p, &ctx.layout.work_tree, now);
        }
    }
    Verdict::Reject(rejection(&findings, marker.as_ref()))
}

fn sealed_findings(ctx: &RepoContext, paths: &[RepoPath]) -> Vec<Finding> {
    let policy = ctx.policy_view();
    paths
        .iter()
        .filter_map(|path| {
            let reason = if let Some(pattern) = policy.sealed_match(path) {
                format!("is sealed by `{pattern}`")
            } else {
                let zone = policy.zone(path).filter(|z| z.kind == ZoneKind::Sealed)?;
                format!("is in sealed zone `{}`", zone.pattern)
            };
            Some(Finding {
                path: path.to_string(),
                reason,
                sealed: true,
            })
        })
        .collect()
}

/// Claims on the staged paths that should stop this commit. The flag is true when the remote view
/// is stale (remote claims were not checked).
fn claim_findings(
    ctx: &RepoContext,
    db: &StateDb,
    paths: &[RepoPath],
    marker: Option<&CommitMarker>,
    now: Timestamp,
) -> Result<(Vec<Finding>, bool), acs_store::StoreError> {
    let config = ctx.policy_view().config();
    let stale_after = Duration::from_secs(config.sync_interval_secs.get().saturating_mul(3));
    let mut snapshot =
        db.claims_snapshot(paths, &config.default_target_branch, now, stale_after)?;
    let mut stale = false;
    if snapshot.remote_stale {
        // As the gate does (decision 0012): with no remote or no sync yet there is no view.
        let sync_ran = ctx.has_remote && db.remote_sync_status()?.last_attempt_at.is_some();
        stale = sync_ran;
        snapshot.remote_stale = false;
    }
    let this_worktree = ctx.worktree_str();
    let mut seats: HashMap<SeatId, Option<SeatInfo>> = HashMap::new();
    let mut findings: Vec<Finding> = Vec::new();
    for held in &snapshot.claims {
        let seat = match held.source {
            ClaimSource::Remote => None,
            ClaimSource::Local => {
                let id = held.claim.seat_id.clone();
                if !seats.contains_key(&id) {
                    let info = db.seat_info(&id)?;
                    seats.insert(id.clone(), info);
                }
                seats.get(&id).and_then(Option::as_ref)
            }
        };
        let blocks = match marker {
            // An agent commit: every other seat's claim counts.
            Some(marker) => held.claim.seat_id.as_str() != marker.seat_id,
            // A human commit: claims of this work tree's own seats are the workflow; everything
            // else (other work trees, other machines, a seat we cannot identify) blocks.
            None => seat.is_none_or(|s| s.seat.worktree != this_worktree),
        };
        if blocks {
            findings.push(Finding {
                path: held.claim.scope.path().to_string(),
                reason: format!("is claimed by {}", describe_holder(held, now)),
                sealed: false,
            });
        }
    }
    findings.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((findings, stale))
}

fn describe_holder(held: &HeldClaim, now: Timestamp) -> String {
    let place = match (&held.holder_harness, held.source) {
        (Some(harness), _) => harness.clone(),
        (None, ClaimSource::Remote) => "another machine".to_owned(),
        (None, ClaimSource::Local) => "this machine".to_owned(),
    };
    let active = held.holder_last_seen_at.unwrap_or(held.claim.heartbeat_at);
    format!(
        "{} ({place}, claimed {}, active {})",
        held.claim.pilot,
        age(now, held.claim.acquired_at),
        age(now, active)
    )
}

fn age(now: Timestamp, since: Timestamp) -> String {
    let secs = now.duration_since(since).as_secs().max(0);
    match secs {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

fn rejection(findings: &[Finding], marker: Option<&CommitMarker>) -> String {
    let who = match marker {
        Some(m) => format!("by agent session {} ({})", m.session_id, m.harness),
        None => "by a person".to_owned(),
    };
    let mut out =
        format!("acs: commit rejected (made {who}); these staged files are off limits:\n");
    for finding in findings.iter().take(SHOWN) {
        out.push_str(&format!("  - `{}` {}\n", finding.path, finding.reason));
    }
    if findings.len() > SHOWN {
        out.push_str(&format!("  - and {} more\n", findings.len() - SHOWN));
    }
    let any_sealed = findings.iter().any(|f| f.sealed);
    let any_claimed = findings.iter().any(|f| !f.sealed);
    match marker {
        Some(_) => {
            if any_sealed {
                out.push_str(
                    "Sealed paths can only be changed by a human: unstage them (`git restore \
                     --staged <path>`) and ask your pilot to commit them.\n",
                );
            }
            if any_claimed {
                out.push_str(
                    "Unstage the claimed files (`git restore --staged <path>`), or wait until \
                     the holder releases them and coordinate through your pilot.\n",
                );
            }
        }
        None => out.push_str(
            "Wait until the holder releases the claim (`acs status` shows who holds what), or \
             coordinate with them. To commit anyway, run `git commit --no-verify`: that is your \
             explicit override.\n",
        ),
    }
    out
}

// ---------------------------------------------------------------------------------------------
// prepare-commit-msg
// ---------------------------------------------------------------------------------------------

fn prepare_commit_msg(ctx: &RepoContext, args: &[String], now: Timestamp) -> Verdict {
    let Some(file) = args.first() else {
        return Verdict::warn("prepare-commit-msg was called without a message file");
    };
    let source = args.get(1).map(String::as_str);
    let mut warnings = Vec::new();
    let marker =
        pending_dir(ctx).and_then(|p| markers::take_commit_marker(&p, &ctx.layout.work_tree, now));
    let mut found: Vec<Trailer> = Vec::new();
    if let Some(marker) = &marker {
        found.push(Trailer::session(&marker.session_id));
    } else if !matches!(source, Some("merge" | "squash")) && !history_operation(ctx) {
        match attributed_sessions(ctx) {
            Ok(sessions) => found.extend(sessions.iter().map(|s| Trailer::session(s))),
            Err(err) => warnings.push(format!("no ACS-Session trailers were added: {err}")),
        }
    }
    if !found.is_empty() {
        let comment = query::comment_char(&ctx.layout.work_tree);
        if let Err(err) = trailers::add_to_file(Path::new(file), &found, comment) {
            warnings.push(format!("could not add the ACS-Session trailer: {err}"));
        }
    }
    Verdict::Allow(warnings)
}

/// Whether a rebase, cherry-pick or revert is in progress: the staged diff is then somebody
/// else's work, not changes a session made here.
fn history_operation(ctx: &RepoContext) -> bool {
    [
        "rebase-merge",
        "rebase-apply",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ]
    .iter()
    .any(|name| ctx.layout.git_dir.join(name).exists())
}

/// The sessions (in order of first appearance) of seats in this work tree whose recorded changes
/// are in the staged diff and not committed yet.
///
/// A recorded change of a session covers a staged path when the session's newest attribution of
/// that path ends in exactly the staged content (or its deletion); or, when the staged content
/// differs (someone edited on top, a line-ending filter ran), when that attribution is newer than
/// the last commit that touched the path.
fn attributed_sessions(ctx: &RepoContext) -> Result<Vec<String>, String> {
    let db = ctx
        .db()
        .map_err(|e| format!("ACS state is unavailable ({e})"))?;
    let staged = query::staged_changes(&ctx.layout.work_tree).map_err(|e| e.to_string())?;
    let this_worktree = ctx.worktree_str();
    let mut seats: HashMap<SeatId, Option<SeatInfo>> = HashMap::new();
    let mut candidates: Vec<(&StagedChange, Vec<acs_store::StoredAttribution>)> = Vec::new();
    for change in staged.iter().take(MAX_ATTRIBUTION_PATHS) {
        let Ok(path) = RepoPath::try_from(change.path.clone()) else {
            continue;
        };
        let rows = db.path_attributions(&path).map_err(|e| e.to_string())?;
        let mut kept = Vec::new();
        for row in rows {
            if !seats.contains_key(&row.seat_id) {
                let info = db.seat_info(&row.seat_id).map_err(|e| e.to_string())?;
                seats.insert(row.seat_id.clone(), info);
            }
            let in_scope = seats
                .get(&row.seat_id)
                .and_then(Option::as_ref)
                .is_some_and(|s| {
                    s.seat.worktree == this_worktree && s.seat.harness != HUMAN_HARNESS
                });
            if in_scope {
                kept.push(row);
            }
        }
        if !kept.is_empty() {
            candidates.push((change, kept));
        }
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let oids: Vec<String> = candidates
        .iter()
        .filter_map(|(change, _)| change.oid.clone())
        .collect();
    let blobs = query::blob_contents(&ctx.layout.work_tree, &oids).map_err(|e| e.to_string())?;
    let mut hashes: HashMap<&str, Option<acs_core::ContentHash>> = HashMap::new();
    for (oid, blob) in oids.iter().zip(blobs) {
        hashes.insert(
            oid.as_str(),
            blob.map(|bytes| acs_core::ContentHash::of(&bytes)),
        );
    }

    let mut sessions: Vec<String> = Vec::new();
    for (change, rows) in &candidates {
        let staged_hash = change
            .oid
            .as_deref()
            .and_then(|oid| hashes.get(oid))
            .cloned()
            .flatten();
        let mut last_commit: Option<Option<i64>> = None;
        let mut seen: Vec<&SeatId> = Vec::new();
        // Rows are newest first: the first row of a seat is its latest word on this path.
        for row in rows {
            if seen.contains(&&row.seat_id) {
                continue;
            }
            seen.push(&row.seat_id);
            let exact = if change.is_deletion() {
                row.post_hash.is_none()
            } else {
                staged_hash.is_some() && row.post_hash == staged_hash
            };
            let covered = exact || {
                let cutoff = *last_commit.get_or_insert_with(|| {
                    query::last_commit_time(&ctx.layout.work_tree, &change.path)
                });
                cutoff.is_none_or(|t| row.at.as_second() >= t)
            };
            if !covered {
                continue;
            }
            let session = seats
                .get(&row.seat_id)
                .and_then(Option::as_ref)
                .map(|s| s.session_id.to_string());
            if let Some(session) = session
                && !sessions.contains(&session)
            {
                sessions.push(session);
            }
        }
    }
    Ok(sessions)
}

// ---------------------------------------------------------------------------------------------
// pre-push
// ---------------------------------------------------------------------------------------------

fn pre_push(ctx: &RepoContext, stdin: &str, now: Timestamp, agent_env: bool) -> Verdict {
    // The marker is consumed by the push it announced, whatever the push turns out to be.
    let marker =
        pending_dir(ctx).and_then(|p| markers::take_push_marker(&p, &ctx.layout.work_tree, now));
    let agent = agent_env || marker.is_some();
    if !agent {
        return Verdict::allow();
    }
    if ctx
        .db()
        .ok()
        .is_some_and(|db| db.flag_is_set(FLAG_GATE_OFF).unwrap_or(false))
    {
        return Verdict::allow();
    }
    let mut blocked: Vec<String> = Vec::new();
    for line in stdin.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [_, local_sha, remote_ref, remote_sha] = fields[..] else {
            continue;
        };
        let Some(branch) = remote_ref.strip_prefix("refs/heads/") else {
            continue;
        };
        if !ctx.policy_view().is_protected_branch(branch) {
            continue;
        }
        let is_zero = |sha: &str| sha.bytes().all(|b| b == b'0');
        if is_zero(local_sha) {
            blocked.push(format!("deleting protected branch `{branch}`"));
        } else if !is_zero(remote_sha)
            && query::is_ancestor(&ctx.layout.work_tree, remote_sha, local_sha) != Some(true)
        {
            blocked.push(format!(
                "a force push (not a fast-forward) to protected branch `{branch}`"
            ));
        }
    }
    if blocked.is_empty() {
        return Verdict::allow();
    }
    let who = marker.map_or_else(
        || "an agent session".to_owned(),
        |m| format!("agent session {} ({})", m.session_id, m.harness),
    );
    let mut out = format!("acs: push rejected ({who}):\n");
    for item in &blocked {
        out.push_str(&format!("  - {item}\n"));
    }
    out.push_str(
        "Force pushes and deletions are blocked on protected branches for agents. Push to a \
         feature branch, or ask your pilot to do this themselves.\n",
    );
    Verdict::Reject(out)
}
