//! `acs undo`: undo what one session changed (WP4.3).
//!
//! A dry run by default. With `--apply` the session's files are put back from the snapshots ACS
//! took on this machine; changes others made to the same files afterwards are kept (a three-way
//! merge), and anything unclear is left alone and reported. History is never rewritten: when the
//! session's changes were already committed, the restored files are committed in one new commit.
//!
//! The session is named by its id, or picked with `--last`: the agent session on this machine
//! that most recently changed files in this repository, by when it ended. If that session is still
//! running, `--last` says so and stops instead of picking an older one.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use acs_core::{RepoPath, SessionId, Timestamp};
use acs_git::merge::{CommitOutcome, MergeOutcome};
use acs_store::SeatInfo;
use acs_store::undo::{self, Applied, MergeResult, PlanInputs, UndoAction, UndoItem, UndoPlan};
use clap::{ArgGroup, Args};
use serde_json::{Value, json};

use super::support::{
    CliError, OutputArgs, WIDTH, age, ellipsize_end, ellipsize_start, finish, open_repo,
    print_json, when, wrap,
};
use crate::runtime::{OpError, RepoContext};

/// Arguments of `acs undo`.
#[derive(Debug, Args)]
#[command(
    group(ArgGroup::new("which").required(true).args(["session", "last"])),
    after_help = "Examples:\n  \
                  acs undo --last                the session that last changed files here\n  \
                  acs undo --last --apply\n  \
                  acs undo ses_4f9k2\n  \
                  acs undo ses_4f9k2 --apply\n\n\
                  `--last` picks the agent session on this machine that most recently changed \
                  files in this repository, by when it ended, and names it with its intent. If \
                  that session is still running, it says so and stops."
)]
pub struct UndoArgs {
    /// The session to undo: a `ses_` id, or a unique prefix of one (see `acs log`).
    #[arg(value_name = "SESSION")]
    pub session: Option<String>,
    /// Undo the session on this machine that most recently changed files here, instead of
    /// naming one.
    #[arg(long)]
    pub last: bool,
    /// Carry out the plan. Without it nothing is changed and the plan is printed.
    #[arg(long)]
    pub apply: bool,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs undo`. Exit 1 when the session is unknown, `--last` finds no session or a running
/// one, or `--apply` left a conflict or a failure behind.
#[must_use]
pub fn run(args: &UndoArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

/// How the session was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    /// By its id or a prefix of it.
    Id,
    /// With `--last`.
    Last,
}

impl Pick {
    fn as_str(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Last => "last",
        }
    }
}

/// How many recent sessions `--last` reads: the most recent one, and an ended one to suggest
/// when the most recent is still running.
const LAST_CANDIDATES: usize = 20;

fn state_error(why: &str) -> CliError {
    CliError::from(OpError::Unavailable(why.to_owned()))
}

/// The session `--last` picks: the most recent one that changed files. An error when there is
/// none, or when it may still be running.
fn find_last(ctx: &RepoContext, now: Timestamp) -> Result<SeatInfo, CliError> {
    let db = ctx.db().map_err(state_error)?;
    let mut seats = db
        .seats_that_changed_files(LAST_CANDIDATES)
        .map_err(OpError::from)?;
    if seats.is_empty() {
        return Err(CliError::new(
            "no_session",
            "No session has changed files in this repository on this machine, so `--last` has \
             nothing to undo. `acs undo` is local: it needs the snapshots this machine took while \
             a session ran. `acs log` lists sessions.",
        ));
    }
    let last = seats.remove(0);
    if !is_live(ctx, &last, now) {
        return Ok(last);
    }
    let mut message = format!(
        "The most recent session, {} ({}), may still be running: it has not ended and was seen \
         {} ago. `--last` stops here rather than pick an older session. Wait for it to end, or \
         stop the agent, then run this again; `acs undo {}` shows its plan now.",
        last.session_id,
        who_and_intent(&last),
        age(now, last.seat.last_seen_at),
        last.session_id,
    );
    if let Some(ended) = seats.iter().find(|seat| !is_live(ctx, seat, now)) {
        let _ = write!(
            message,
            " The most recent session that has ended is {} ({}, {}): `acs undo {}`.",
            ended.session_id,
            who_and_intent(ended),
            ended_phrase(ended, now),
            ended.session_id,
        );
    }
    Err(CliError::new("session_live", message))
}

/// `claude-code, "Fix the parser"`, or `claude-code, no intent given`.
fn who_and_intent(seat: &SeatInfo) -> String {
    match &seat.intent {
        Some(intent) => format!("{}, \"{}\"", seat.seat.harness, ellipsize_end(intent, 60)),
        None => format!("{}, no intent given", seat.seat.harness),
    }
}

/// When the seat ended, or, for a seat that never reported its end, when it was last seen.
fn ended(seat: &SeatInfo) -> (&'static str, Timestamp) {
    match seat.ended_at {
        Some(at) => ("ended", at),
        None => ("last seen", seat.seat.last_seen_at),
    }
}

/// `ended 3m ago`, or `last seen 3m ago` for a seat that never reported its end.
fn ended_phrase(seat: &SeatInfo, now: Timestamp) -> String {
    let (what, at) = ended(seat);
    format!("{what} {} ago", age(now, at))
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
    super::support::note_feature_off(
        &ctx,
        acs_core::Feature::History,
        args.output.json,
        "no copies of files are kept any more; only sessions from before can be undone",
    );
    let now = Timestamp::now();
    let (seat, pick) = match &args.session {
        Some(wanted) if !args.last => (find_seat(&ctx, wanted)?, Pick::Id),
        _ => (find_last(&ctx, now)?, Pick::Last),
    };
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
    let locate = |recorded: &Path| locate_tree(&ctx, recorded);
    let plan = undo::plan(
        &PlanInputs {
            session: &session,
            work_tree: &ctx.layout.work_tree,
            locate: &locate,
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
    let mut commits: Vec<(PathBuf, Committed)> = Vec::new();
    if args.apply {
        // Which restored files can go into a new commit: the session committed them and the file
        // is exactly what HEAD holds, so the undo is a clean revert of committed work. Each work
        // tree has its own HEAD, so each gets its own commit.
        let committable = committable_paths(&plan, session.as_str());
        applied = undo::apply(store, &plan);
        let done = undo::applied_paths(&applied);
        for (tree, paths) in committable {
            let to_commit: Vec<String> = paths
                .into_iter()
                .filter(|p| {
                    p.parse::<RepoPath>()
                        .is_ok_and(|p| done.contains(&(tree.clone(), p)))
                })
                .collect();
            match commit(&tree, &session, &to_commit) {
                Committed::NotNeeded => {}
                outcome => commits.push((tree, outcome)),
            }
        }
    }

    let failures = applied.iter().filter(|a| a.result.is_err()).count();
    let conflicts = plan.count(UndoAction::Conflict);
    let commit_failed = commits
        .iter()
        .any(|(_, c)| matches!(c, Committed::Failed(_)));
    let failed = args.apply && (failures > 0 || conflicts > 0 || commit_failed);

    let view = View {
        seat: &seat,
        pick,
        here: &ctx.layout.work_tree,
        plan: &plan,
        applied: &applied,
        commits: &commits,
        apply: args.apply,
        live,
    };
    if args.output.json {
        print_json(&json_view(&view));
    } else {
        print!("{}", render(&view, now));
    }
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// Where a work tree recorded in the session's snapshots is now: the same root, still a work tree
/// of this repository. Anything else (removed, or a plain folder or another repository in its
/// place) is refused, so undo never writes a file into a checkout the session did not change.
fn locate_tree(ctx: &RepoContext, recorded: &Path) -> Result<PathBuf, String> {
    acs_git::discover(recorded)
        .ok()
        // Roots are recorded as `discover` names them (canonical), so equal roots compare equal.
        .filter(|layout| layout.common_dir == ctx.layout.common_dir && layout.work_tree == recorded)
        .map(|layout| layout.work_tree)
        .ok_or_else(|| {
            format!(
                "the session changed it in the worktree `{}`, which is no longer a worktree of \
                 this repository; undo does not write it anywhere else",
                recorded.display()
            )
        })
}

/// The changed paths the session committed that are exactly as `HEAD` has them now, by work tree.
fn committable_paths(plan: &UndoPlan, session: &str) -> BTreeMap<PathBuf, Vec<String>> {
    let mut changing: BTreeMap<&Path, Vec<&str>> = BTreeMap::new();
    for item in plan.items.iter().filter(|i| i.action.changes_file()) {
        changing
            .entry(item.work_tree.as_path())
            .or_default()
            .push(item.path.as_str());
    }
    let mut out = BTreeMap::new();
    for (tree, paths) in changing {
        let Ok(committed) = acs_git::merge::session_commit_paths(tree, session) else {
            continue;
        };
        let paths: Vec<String> = paths
            .into_iter()
            .filter(|p| committed.contains(*p))
            // Through git, not by raw bytes: with `core.autocrlf` (the Git for Windows default)
            // the work tree holds CRLF and `HEAD` holds LF (W3 in docs/security/write-paths.md).
            .filter(|p| acs_git::merge::worktree_matches_head(tree, p).unwrap_or(false))
            .map(str::to_owned)
            .collect();
        if !paths.is_empty() {
            out.insert(tree.to_path_buf(), paths);
        }
    }
    out
}

fn commit(work_tree: &Path, session: &SessionId, paths: &[String]) -> Committed {
    if paths.is_empty() {
        return Committed::NotNeeded;
    }
    let mut message = format!("Undo session {session}\n\nRestored by `acs undo`:\n");
    for path in paths {
        let _ = writeln!(message, "- {path}");
    }
    match acs_git::merge::commit_only(work_tree, paths, &message) {
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

/// Everything the output shows.
struct View<'a> {
    seat: &'a SeatInfo,
    pick: Pick,
    /// The work tree `acs undo` runs in.
    here: &'a Path,
    plan: &'a UndoPlan,
    applied: &'a [Applied],
    /// The undo commits made or attempted, one per work tree.
    commits: &'a [(PathBuf, Committed)],
    apply: bool,
    live: bool,
}

/// What happened to `item` when the plan was applied.
fn outcome_of<'a>(applied: &'a [Applied], item: &UndoItem) -> Option<&'a Applied> {
    applied
        .iter()
        .find(|a| a.path == item.path && a.work_tree == item.work_tree)
}

/// How a work tree is named to the person: relative to the one undo runs in when it is inside
/// it (`.claude/worktrees/wp`), its full path otherwise.
fn tree_name(here: &Path, tree: &Path) -> String {
    match tree.strip_prefix(here) {
        Ok(inner) => inner.to_string_lossy().replace('\\', "/"),
        Err(_) => tree.display().to_string(),
    }
}

/// The path of `item` as the person reading the plan finds it: as it is for the work tree undo
/// runs in, and with its work tree in front for any other.
fn shown_path(here: &Path, item: &UndoItem) -> String {
    if item.work_tree == here {
        item.path.as_str().to_owned()
    } else {
        format!(
            "{}/{}",
            tree_name(here, &item.work_tree).trim_end_matches('/'),
            item.path
        )
    }
}

fn commit_json(committed: &Committed) -> Value {
    match committed {
        Committed::NotNeeded => json!({"status": "none"}),
        Committed::Made(id) => json!({"status": "committed", "commit": id}),
        Committed::Failed(why) => json!({"status": "failed", "error": why}),
    }
}

fn json_view(view: &View<'_>) -> Value {
    let View {
        seat,
        pick,
        here,
        plan,
        applied,
        commits,
        apply,
        live,
    } = *view;
    let files: Vec<Value> = plan
        .items
        .iter()
        .map(|item| {
            let outcome = outcome_of(applied, item);
            json!({
                "path": item.path,
                "work_tree": item.work_tree,
                "elsewhere": item.work_tree != here,
                "action": item.action,
                "detail": item.detail,
                "applied": outcome.map(|a| a.result.is_ok()),
                "error": outcome.and_then(|a| a.result.as_ref().err()),
            })
        })
        .collect();
    // `commit` is the one undo commit, as before work trees were told apart; with commits in
    // several work trees it is the first that failed, or else the first made. `commits` lists
    // them all.
    let commit = commits
        .iter()
        .find(|(_, c)| matches!(c, Committed::Failed(_)))
        .or_else(|| commits.first())
        .map_or_else(
            || commit_json(&Committed::NotNeeded),
            |(_, c)| commit_json(c),
        );
    let all_commits: Vec<Value> = commits
        .iter()
        .map(|(tree, c)| {
            let mut value = commit_json(c);
            value["work_tree"] = json!(tree);
            value
        })
        .collect();
    json!({
        "ok": true,
        "session": plan.session,
        "picked_by": pick.as_str(),
        "harness": seat.seat.harness,
        "pilot": seat.seat.pilot,
        "intent": seat.intent,
        "ended_at": seat.ended_at,
        "last_seen_at": seat.seat.last_seen_at,
        "live": live,
        "applied": apply,
        "files": files,
        "summary": counts(plan),
        "commit": commit,
        "commits": all_commits,
    })
}

fn render(view: &View<'_>, now: Timestamp) -> String {
    let View {
        seat,
        pick,
        here,
        plan,
        applied,
        commits,
        apply,
        live,
    } = *view;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} {} {}",
        if apply { "Undo of" } else { "Undo plan for" },
        match pick {
            Pick::Id => "session",
            Pick::Last => "the last session,",
        },
        plan.session,
    );
    let pilot = ellipsize_start(&seat.seat.pilot, 40);
    let seen = if live {
        format!("last seen {} ago", age(now, seat.seat.last_seen_at))
    } else {
        format!("{} ({})", ended_phrase(seat, now), when(ended(seat).1))
    };
    out.push_str(&wrap(
        &format!("{} {pilot}, {seen}", seat.seat.harness),
        "  ",
        "    ",
    ));
    let intent = seat.intent.as_deref().unwrap_or("none given");
    out.push_str(&wrap(&format!("Intent: {intent}"), "  ", "    "));
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
        let outcome = outcome_of(applied, item);
        let label = match outcome {
            Some(a) if a.result.is_err() => "FAILED".to_owned(),
            Some(_) => format!("{} (done)", item.action.label()),
            None => item.action.label().to_owned(),
        };
        let budget = WIDTH - 4 - label.chars().count();
        let _ = writeln!(
            out,
            "  {:<budget$}  {label}",
            ellipsize_start(&shown_path(here, item), budget)
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

    for (tree, committed) in commits {
        let in_tree = if tree.as_path() == here {
            String::new()
        } else {
            format!(" in the worktree `{}`", tree_name(here, tree))
        };
        match committed {
            Committed::Made(id) => {
                out.push_str(&wrap(
                    &format!(
                        "The session's changes were committed{in_tree}, so the undo is a new \
                         commit there ({id}); history is unchanged."
                    ),
                    "",
                    "",
                ));
            }
            Committed::Failed(why) => {
                out.push_str(&wrap(
                    &format!(
                        "!! The files are restored{in_tree} but the undo commit failed: {why}. \
                         Commit them yourself when ready."
                    ),
                    "",
                    "   ",
                ));
            }
            Committed::NotNeeded => {}
        }
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
