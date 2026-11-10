//! `acs undo`: undo what one session changed (WP4.3).
//!
//! A dry run by default. With `--apply` the session's files are put back from the snapshots ACS
//! took on this machine; changes others made to the same files afterwards are kept (a three-way
//! merge), and anything unclear is left alone and reported. History is never rewritten: when the
//! session's changes were already committed, the restored files are committed in one new commit.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::process::ExitCode;

use acs_core::{RepoPath, SessionId, Timestamp};
use acs_git::merge::{CommitOutcome, MergeOutcome};
use acs_store::SeatInfo;
use acs_store::undo::{self, Applied, MergeResult, PlanInputs, UndoAction, UndoPlan};
use clap::Args;
use serde_json::{Value, json};

use super::support::{
    CliError, OutputArgs, WIDTH, age, ellipsize_start, finish, open_repo, print_json, wrap,
};
use crate::runtime::{OpError, RepoContext};

/// Arguments of `acs undo`.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  acs undo ses_4f9k2\n  acs undo ses_4f9k2 --apply")]
pub struct UndoArgs {
    /// The session to undo: a `ses_` id, or a unique prefix of one (see `acs log`).
    #[arg(value_name = "SESSION")]
    pub session: String,
    /// Carry out the plan. Without it nothing is changed and the plan is printed.
    #[arg(long)]
    pub apply: bool,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs undo`. Exit 1 when the session is unknown, or `--apply` left a conflict or a
/// failure behind.
#[must_use]
pub fn run(args: &UndoArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

fn state_error(why: &str) -> CliError {
    CliError::from(OpError::Unavailable(why.to_owned()))
}

/// Finds the one seat of the session.
fn find_seat(ctx: &RepoContext, wanted: &str) -> Result<SeatInfo, CliError> {
    let db = ctx.db().map_err(state_error)?;
    let mut seats = db
        .seats_by_session_prefix(wanted.trim())
        .map_err(OpError::from)?;
    if wanted.trim().is_empty() {
        seats.clear();
    }
    match seats.len() {
        0 => Err(CliError::new(
            "unknown_session",
            format!(
                "No session matching `{wanted}` is known on this machine. `acs undo` is local: \
                 it needs the snapshots this machine took while the session ran, so it cannot \
                 undo a session from another machine or one that was never recorded here. \
                 `acs log` lists sessions."
            ),
        )),
        1 => Ok(seats.remove(0)),
        n => {
            let ids: Vec<String> = seats
                .iter()
                .take(5)
                .map(|s| s.session_id.to_string())
                .collect();
            Err(CliError::new(
                "ambiguous_session",
                format!(
                    "`{wanted}` matches {n} sessions ({}{}); give more of the id.",
                    ids.join(", "),
                    if n > 5 { ", ..." } else { "" }
                ),
            ))
        }
    }
}

/// Whether the seat may still be writing: not ended and seen within the claim TTL.
fn is_live(ctx: &RepoContext, seat: &SeatInfo, now: Timestamp) -> bool {
    let ttl =
        i64::try_from(ctx.policy.policy.files().config.claim_ttl_secs.get()).unwrap_or(i64::MAX);
    seat.ended_at.is_none()
        && now
            .as_second()
            .saturating_sub(seat.seat.last_seen_at.as_second())
            < ttl
}

/// What the commit step did, for the output.
enum Committed {
    /// Nothing was asked for (dry run, or nothing committed was restored).
    NotNeeded,
    /// A commit was made.
    Made(String),
    /// `git commit` failed; the files stay restored.
    Failed(String),
}

fn execute(args: &UndoArgs) -> Result<ExitCode, CliError> {
    let (ctx, _cwd) = open_repo()?;
    let now = Timestamp::now();
    let seat = find_seat(&ctx, &args.session)?;
    let db = ctx.db().map_err(state_error)?;
    let store = ctx.snapshots().map_err(state_error)?;
    let session: SessionId = seat.session_id.clone();
    let live = is_live(&ctx, &seat, now);

    let attributions = db
        .seat_attributions(&seat.seat.seat_id)
        .map_err(OpError::from)?;
    let holders: HashMap<String, String> = db
        .live_claims(now)
        .map_err(OpError::from)?
        .into_iter()
        .filter(|held| held.claim.seat_id != seat.seat.seat_id)
        .map(|held| {
            let who = held.holder_harness.as_deref().map_or_else(
                || held.claim.pilot.clone(),
                |harness| format!("{harness} {}", held.claim.pilot),
            );
            (held.claim.scope.path().as_str().to_owned(), who)
        })
        .collect();
    let holder = |path: &RepoPath| -> Option<String> {
        holders
            .get(path.as_str())
            .map(|who| format!("claimed by {who}"))
    };
    let scratch = ctx
        .repo_home
        .as_ref()
        .map_or_else(std::env::temp_dir, |home| home.pending_dir().join("undo"));
    let mut merge = |current: &[u8], post: &[u8], pre: &[u8]| match acs_git::merge::merge_file(
        &scratch, current, post, pre,
    ) {
        Ok(MergeOutcome::Clean(bytes)) => MergeResult::Clean(bytes),
        Ok(MergeOutcome::Conflict) => {
            MergeResult::Conflict("later changes overlap the session's; left untouched".to_owned())
        }
        Err(err) => MergeResult::Conflict(format!("could not merge: {err}")),
    };
    let plan = undo::plan(
        &PlanInputs {
            session: &session,
            work_tree: &ctx.layout.work_tree,
            snapshots: store,
            attributions: &attributions,
            holder: &holder,
        },
        &mut merge,
    )
    .map_err(|err| CliError::new("undo", err.to_string()))?;

    if args.apply && live {
        return Err(CliError::new(
            "session_live",
            format!(
                "Session {session} may still be running (its seat has not ended and was seen {} \
                 ago), so it could still be writing these files. Wait for it to end, or stop the \
                 agent, then run this again. The dry run above this command works any time: \
                 `acs undo {session}`.",
                age(now, seat.seat.last_seen_at)
            ),
        ));
    }

    let mut applied: Vec<Applied> = Vec::new();
    let mut committed = Committed::NotNeeded;
    if args.apply {
        // Which restored files can go into a new commit: the session committed them and the file
        // is exactly what HEAD holds, so the undo is a clean revert of committed work.
        let committable = committable_paths(&ctx, &plan, session.as_str());
        applied = undo::apply(store, &ctx.layout.work_tree, &plan);
        let done = undo::applied_paths(&applied);
        let to_commit: Vec<String> = committable
            .into_iter()
            .filter(|p| p.parse::<RepoPath>().is_ok_and(|p| done.contains(&p)))
            .collect();
        committed = commit(&ctx, &session, &to_commit);
    }

    let failures = applied.iter().filter(|a| a.result.is_err()).count();
    let conflicts = plan.count(UndoAction::Conflict);
    let failed =
        args.apply && (failures > 0 || conflicts > 0 || matches!(committed, Committed::Failed(_)));

    if args.output.json {
        print_json(&json_view(
            &seat, &plan, &applied, &committed, args.apply, live,
        ));
    } else {
        print!(
            "{}",
            render(&seat, &plan, &applied, &committed, args.apply, live, now)
        );
    }
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// The changed paths the session committed that are exactly as `HEAD` has them now.
fn committable_paths(ctx: &RepoContext, plan: &UndoPlan, session: &str) -> Vec<String> {
    let changing: Vec<_> = plan
        .items
        .iter()
        .filter(|i| i.action.changes_file())
        .collect();
    if changing.is_empty() {
        return Vec::new();
    }
    let Ok(committed) = acs_git::merge::session_commit_paths(&ctx.layout.work_tree, session) else {
        return Vec::new();
    };
    changing
        .into_iter()
        .filter(|i| committed.contains(i.path.as_str()))
        .filter(|i| {
            acs_git::merge::head_matches(&ctx.layout.work_tree, i.path.as_str(), i.current.as_ref())
                .unwrap_or(false)
        })
        .map(|i| i.path.as_str().to_owned())
        .collect()
}

fn commit(ctx: &RepoContext, session: &SessionId, paths: &[String]) -> Committed {
    if paths.is_empty() {
        return Committed::NotNeeded;
    }
    let mut message = format!("Undo session {session}\n\nRestored by `acs undo`:\n");
    for path in paths {
        let _ = writeln!(message, "- {path}");
    }
    match acs_git::merge::commit_only(&ctx.layout.work_tree, paths, &message) {
        Ok(CommitOutcome::Committed(id)) => Committed::Made(id),
        Ok(CommitOutcome::NothingToCommit) => Committed::NotNeeded,
        Ok(CommitOutcome::Failed(why)) => Committed::Failed(why),
        Err(err) => Committed::Failed(err.to_string()),
    }
}

fn counts(plan: &UndoPlan) -> Value {
    let mut map = serde_json::Map::new();
    for action in [
        UndoAction::Restore,
        UndoAction::Delete,
        UndoAction::Recreate,
        UndoAction::Merge,
        UndoAction::AlreadyUndone,
        UndoAction::Conflict,
        UndoAction::Unknown,
        UndoAction::Held,
    ] {
        let name = serde_json::to_value(action)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        map.insert(name, json!(plan.count(action)));
    }
    Value::Object(map)
}

fn json_view(
    seat: &SeatInfo,
    plan: &UndoPlan,
    applied: &[Applied],
    committed: &Committed,
    apply: bool,
    live: bool,
) -> Value {
    let files: Vec<Value> = plan
        .items
        .iter()
        .map(|item| {
            let outcome = applied.iter().find(|a| a.path == item.path);
            json!({
                "path": item.path,
                "action": item.action,
                "detail": item.detail,
                "applied": outcome.map(|a| a.result.is_ok()),
                "error": outcome.and_then(|a| a.result.as_ref().err()),
            })
        })
        .collect();
    let commit = match committed {
        Committed::NotNeeded => json!({"status": "none"}),
        Committed::Made(id) => json!({"status": "committed", "commit": id}),
        Committed::Failed(why) => json!({"status": "failed", "error": why}),
    };
    json!({
        "ok": true,
        "session": plan.session,
        "harness": seat.seat.harness,
        "pilot": seat.seat.pilot,
        "live": live,
        "applied": apply,
        "files": files,
        "summary": counts(plan),
        "commit": commit,
    })
}

fn render(
    seat: &SeatInfo,
    plan: &UndoPlan,
    applied: &[Applied],
    committed: &Committed,
    apply: bool,
    live: bool,
    now: Timestamp,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} session {}",
        if apply { "Undo of" } else { "Undo plan for" },
        plan.session,
    );
    let _ = writeln!(
        out,
        "  {} {}, last seen {} ago",
        seat.seat.harness,
        ellipsize_start(&seat.seat.pilot, 40),
        age(now, seat.seat.last_seen_at)
    );
    if live {
        out.push_str(&wrap(
            "!! This session may still be running. This is the plan only; `--apply` \
             refuses until the session has ended.",
            "",
            "   ",
        ));
    }
    if plan.items.is_empty() {
        out.push_str("\nNothing was recorded for this session: no files to undo.\n");
        return out;
    }
    out.push('\n');
    for item in &plan.items {
        let outcome = applied.iter().find(|a| a.path == item.path);
        let label = match outcome {
            Some(a) if a.result.is_err() => "FAILED".to_owned(),
            Some(_) => format!("{} (done)", item.action.label()),
            None => item.action.label().to_owned(),
        };
        let budget = WIDTH - 4 - label.chars().count();
        let _ = writeln!(
            out,
            "  {:<budget$}  {label}",
            ellipsize_start(item.path.as_str(), budget)
        );
        let detail = match outcome.and_then(|a| a.result.as_ref().err()) {
            Some(error) => Some(error.as_str()),
            None => item.detail.as_deref(),
        };
        if let Some(detail) = detail {
            out.push_str(&wrap(detail, "      ", "      "));
        }
    }

    let n = |action| plan.count(action);
    let changing: usize = plan
        .items
        .iter()
        .filter(|i| i.action.changes_file())
        .count();
    let will = if apply { "" } else { "to be " };
    let summary = [
        (n(UndoAction::Restore), format!("{will}restored")),
        (n(UndoAction::Delete), format!("{will}deleted")),
        (n(UndoAction::Recreate), format!("{will}recreated")),
        (n(UndoAction::Merge), format!("{will}merged")),
        (n(UndoAction::AlreadyUndone), "already undone".to_owned()),
        (n(UndoAction::Conflict), "in conflict".to_owned()),
        (n(UndoAction::Unknown), "unknown".to_owned()),
        (n(UndoAction::Held), "held by another seat".to_owned()),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, what)| format!("{count} {what}"))
    .collect::<Vec<_>>()
    .join(", ");
    out.push('\n');
    out.push_str(&wrap(&format!("Summary: {summary}."), "", "  "));

    match committed {
        Committed::Made(id) => {
            out.push_str(&wrap(
                &format!(
                    "The session's changes were committed, so the undo is a new commit ({id}); \
                     history is unchanged."
                ),
                "",
                "",
            ));
        }
        Committed::Failed(why) => {
            out.push_str(&wrap(
                &format!(
                    "!! The files are restored but the undo commit failed: {why}. Commit them \
                     yourself when ready."
                ),
                "",
                "   ",
            ));
        }
        Committed::NotNeeded => {}
    }
    if !apply {
        if changing > 0 {
            out.push_str(&wrap(
                &format!(
                    "This is a dry run. Run `acs undo {} --apply` to carry it out.",
                    plan.session
                ),
                "",
                "",
            ));
        } else {
            out.push_str("Nothing to do.\n");
        }
    }
    out
}
