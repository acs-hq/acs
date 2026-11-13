//! `acs watch`: claim the files a person changes in any editor (WP7.4).
//!
//! A file watcher for editors without an ACS extension. Every change in the work tree waits a
//! short settle window, then is judged:
//!
//! - **Ignored:** paths under `.git/`, directories, and paths `.gitignore` ignores.
//! - **An agent's:** an agent seat on this machine holds a live claim on the file (agents claim a
//!   file before a file tool writes it), or an agent's post-tool event attributed a change to the
//!   file since just before the change was seen. While an agent's shell command is still running
//!   (its tool marker is waiting for the post-tool step), changes are held back until it finishes,
//!   because that command's post-tool scan may attribute them.
//! - **The person's:** anything else. It is claimed for the human seat exactly as `acs claim`
//!   would, and the claim is refreshed on every later change.
//!
//! A claim is released once its file has gone `--idle` seconds without a change (the file is
//! saved, since the watcher only sees saved content). Stopping the watcher leaves claims to lapse
//! after `claim_ttl_secs`, or to `acs release --all`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use acs_core::{ClaimOrigin, ReasonCode, Timestamp};
use acs_store::SeatInfo;
use clap::Args;
use notify::{EventKind, RecursiveMode, Watcher};
use serde_json::{Value, json};

use super::support::{CliError, OutputArgs, finish, human_seat, open_repo, wrap};
use crate::hook::markers;
use crate::runtime::seats::HUMAN_HARNESS;
use crate::runtime::{ClaimOutcome, RepoContext, claim_paths, release_paths, sync};

/// Arguments of `acs watch`.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  acs watch\n  acs watch --idle 300\n\n\
                  Runs until stopped (Ctrl-C). Files you change are claimed for you, so agents \
                  leave them alone; changes agents make are left to them. Gitignored files are \
                  never claimed.")]
pub struct WatchArgs {
    /// Release a claim once its file has had no change for this many seconds (at most
    /// `claim_ttl_secs`).
    #[arg(long, value_name = "SECS", default_value_t = 120)]
    pub idle: u64,
    /// How long a file must be quiet before its change is judged, in milliseconds.
    #[arg(long, value_name = "MS", default_value_t = 1500, hide = true)]
    pub settle_ms: u64,
    /// Output format: one JSON object per line.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs watch`. Returns only when the watcher cannot start or stops.
#[must_use]
pub fn run(args: &WatchArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

/// How often pending changes and idle claims are looked at.
const TICK: Duration = Duration::from_millis(250);
/// How long a change waits for a running agent shell command before it is judged anyway.
const SHELL_WAIT: Duration = Duration::from_secs(300);
/// Tool markers older than this belong to calls that will never finish (a crashed session).
const MARKER_MAX_AGE_SECS: i64 = 600;
/// How far before a change was first seen an agent's attribution may be and still explain it:
/// the post-tool step stamps its own time, which can be a little earlier than the watcher saw
/// the event.
const ATTRIBUTION_SLACK_SECS: i64 = 5;
/// A note about the same file is repeated at most this often.
const NOTE_EVERY: Duration = Duration::from_secs(600);
/// The most cached `.gitignore` verdicts before the cache is cleared.
const IGNORE_CACHE_MAX: usize = 50_000;

/// A change seen but not judged yet.
#[derive(Debug, Clone, Copy)]
struct Pending {
    /// When the watcher first saw it (wall clock, compared with attribution times).
    first_seen: Timestamp,
    /// When the watcher first saw it (monotonic).
    first_at: Instant,
    /// The latest event for the path.
    last_at: Instant,
    /// Not before this, when it is waiting for an agent's shell command.
    retry_at: Option<Instant>,
}

/// What the watcher keeps between ticks.
struct State {
    work_tree: PathBuf,
    json: bool,
    idle: Duration,
    settle: Duration,
    pending: HashMap<String, Pending>,
    /// Files claimed by this watcher, with their latest change.
    held: HashMap<String, Instant>,
    ignored: HashMap<String, bool>,
    notes: HashMap<String, Instant>,
}

fn execute(args: &WatchArgs) -> Result<ExitCode, CliError> {
    let (ctx, _) = open_repo()?;
    if !ctx.features().claims {
        return Err(CliError::new(
            "feature_off",
            "Claims are switched off in this repository, so there is nothing for `acs watch` \
             to claim. `acs features claims on` switches them back on.",
        ));
    }
    let now = Timestamp::now();
    let seat = human_seat(&ctx, now)?;
    let ttl = ctx.policy.policy.files().config.claim_ttl_secs.get();
    let idle = args.idle.clamp(1, ttl);
    let work_tree = ctx.layout.work_tree.clone();

    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    })
    .map_err(|err| CliError::new("watch", format!("cannot start the file watcher: {err}")))?;
    watcher
        .watch(&work_tree, RecursiveMode::Recursive)
        .map_err(|err| {
            CliError::new(
                "watch",
                format!("cannot watch {}: {err}", work_tree.display()),
            )
        })?;

    let mut state = State {
        work_tree,
        json: args.output.json,
        idle: Duration::from_secs(idle),
        settle: Duration::from_millis(args.settle_ms),
        pending: HashMap::new(),
        held: HashMap::new(),
        ignored: HashMap::new(),
        notes: HashMap::new(),
    };
    state.say(
        json!({
            "event": "watching",
            "worktree": ctx.worktree_str(),
            "pilot": seat.seat.pilot,
            "idle_secs": idle,
        }),
        || {
            let mut text = format!(
                "Watching {} as {}. Files you change are claimed for you and released after \
                 {idle} s without changes. Stop with Ctrl-C.",
                ctx.worktree_str(),
                seat.seat.pilot
            );
            if args.idle > ttl {
                text.push_str(&format!(" (--idle is capped at claim_ttl_secs, {ttl} s.)"));
            }
            text
        },
    );
    drop(ctx);

    loop {
        match rx.recv_timeout(TICK) {
            Ok(Ok(event)) => state.saw(&event),
            Ok(Err(err)) => state.warn(&format!("the file watcher reported: {err}")),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CliError::new("watch", "the file watcher stopped"));
            }
        }
        // Drain whatever else is queued before judging, so a burst is one batch.
        while let Ok(event) = rx.try_recv() {
            match event {
                Ok(event) => state.saw(&event),
                Err(err) => state.warn(&format!("the file watcher reported: {err}")),
            }
        }
        state.judge_ready();
        state.release_idle();
    }
}

impl State {
    /// Records the paths of one file system event.
    fn saw(&mut self, event: &notify::Event) {
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let now = Instant::now();
        for path in &event.paths {
            let Some(rel) = relative(&self.work_tree, path) else {
                continue;
            };
            self.pending
                .entry(rel)
                .and_modify(|p| p.last_at = now)
                .or_insert(Pending {
                    first_seen: Timestamp::now(),
                    first_at: now,
                    last_at: now,
                    retry_at: None,
                });
        }
    }

    /// Judges the changes that have been quiet for the settle window.
    fn judge_ready(&mut self) {
        let now = Instant::now();
        let ready: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| {
                now.duration_since(p.last_at) >= self.settle && p.retry_at.is_none_or(|t| now >= t)
            })
            .map(|(path, _)| path.clone())
            .collect();
        if ready.is_empty() {
            return;
        }
        // Reopened for every batch: the policy, the claims and the seat may all have changed.
        let ctx = match RepoContext::open(&self.work_tree) {
            Ok(ctx) => ctx,
            Err(err) => {
                self.warn(&format!("cannot open the repository: {err}"));
                for path in &ready {
                    self.pending.remove(path);
                }
                return;
            }
        };
        let Ok(db) = ctx.db() else {
            self.warn(
                "the state database is unavailable, so nothing can be claimed (run `acs doctor`)",
            );
            for path in &ready {
                self.pending.remove(path);
            }
            return;
        };
        let stamp = Timestamp::now();

        // An agent's shell command still running may yet attribute these changes to itself.
        if agent_shell_running(&ctx, db, stamp) {
            let mut waiting = Vec::new();
            for path in &ready {
                if let Some(p) = self.pending.get_mut(path)
                    && now.duration_since(p.first_at) < SHELL_WAIT
                {
                    p.retry_at = Some(now + Duration::from_secs(1));
                    waiting.push(path.clone());
                }
            }
            if !waiting.is_empty() {
                return;
            }
        }

        let batch: Vec<(String, Pending)> = ready
            .iter()
            .filter_map(|path| self.pending.remove(path).map(|p| (path.clone(), p)))
            .filter(|(path, _)| !self.work_tree.join(path).is_dir())
            .collect();
        let ignored = self.ignored_of(batch.iter().map(|(path, _)| path.as_str()));
        let live = db.live_claims(stamp).unwrap_or_default();
        let seat = match human_seat(&ctx, stamp) {
            Ok(seat) => seat,
            Err(err) => {
                self.warn(&err.message);
                return;
            }
        };

        let mut claimed = Vec::new();
        for (path, pending) in batch {
            if ignored.contains(&path) {
                continue;
            }
            let agent_holder = live.iter().find(|held| {
                held.claim.scope.path().as_str() == path && held.claim.origin == ClaimOrigin::Agent
            });
            if let Some(held) = agent_holder {
                let harness = held
                    .holder_harness
                    .clone()
                    .unwrap_or_else(|| "an agent".to_owned());
                if self.note_due(&path) {
                    self.say(
                        json!({"event": "left_to_agent", "path": path, "harness": harness, "pilot": held.claim.pilot}),
                        || {
                            format!(
                                "{path} changed while {harness} ({}) holds it: left to the agent. \
                                 If that was you, coordinate with the agent.",
                                held.claim.pilot
                            )
                        },
                    );
                }
                continue;
            }
            if changed_by_agent(db, &path, pending.first_seen) {
                continue;
            }
            match claim_one(&ctx, &seat, &self.work_tree, &path, stamp) {
                Some(ClaimOutcome::Granted { .. }) => {
                    if self.held.insert(path.clone(), now).is_none() {
                        claimed.push(path);
                    }
                }
                Some(ClaimOutcome::Refused {
                    reason_code,
                    message,
                }) => {
                    // Sealed paths bind agents, not people: editing them is normal.
                    if reason_code == ReasonCode::SealedPath {
                        continue;
                    }
                    if self.note_due(&path) {
                        self.say(
                            json!({"event": "not_claimed", "path": path, "reason_code": reason_code.as_str(), "message": message}),
                            || format!("Not claimed ({reason_code}): {message}"),
                        );
                    }
                }
                None => {}
            }
        }
        if !claimed.is_empty() {
            claimed.sort();
            self.say(json!({"event": "claimed", "paths": claimed}), || {
                format!("Claimed {}", claimed.join(", "))
            });
        }
        sync::spawn_if_due(&ctx, db, stamp);
    }

    /// Releases the claims whose files have had no change for the idle window.
    fn release_idle(&mut self) {
        let now = Instant::now();
        let mut due: Vec<String> = self
            .held
            .iter()
            .filter(|(path, last)| {
                now.duration_since(**last) >= self.idle && !self.pending.contains_key(*path)
            })
            .map(|(path, _)| path.clone())
            .collect();
        if due.is_empty() {
            return;
        }
        due.sort();
        for path in &due {
            self.held.remove(path);
        }
        let Ok(ctx) = RepoContext::open(&self.work_tree) else {
            return;
        };
        let stamp = Timestamp::now();
        let Ok(seat) = human_seat(&ctx, stamp) else {
            return;
        };
        match release_paths(&ctx, &seat, &due, &self.work_tree, stamp) {
            Ok(report) if !report.released.is_empty() => {
                let paths: Vec<String> = report.released.iter().map(ToString::to_string).collect();
                self.say(json!({"event": "released", "paths": paths}), || {
                    format!("Released {}", paths.join(", "))
                });
                if let Ok(db) = ctx.db() {
                    sync::spawn_if_due(&ctx, db, stamp);
                }
            }
            Ok(_) => {}
            Err(err) => self.warn(&format!("cannot release claims: {err}")),
        }
    }

    /// The paths among `paths` that `.gitignore` ignores, using and filling the cache. A path
    /// under an ignored top-level directory is ignored without asking git about it.
    fn ignored_of<'a>(&mut self, paths: impl Iterator<Item = &'a str>) -> HashSet<String> {
        if self.ignored.len() > IGNORE_CACHE_MAX {
            self.ignored.clear();
        }
        let mut unknown: Vec<String> = Vec::new();
        let mut asked: HashSet<String> = HashSet::new();
        let mut result = HashSet::new();
        let mut wanted = Vec::new();
        for path in paths {
            let top = path
                .split_once('/')
                .map(|(dir, _)| format!("{dir}/"))
                .unwrap_or_default();
            wanted.push(path.to_owned());
            if self.ignored.get(&top) == Some(&true) {
                continue;
            }
            for key in [top, path.to_owned()] {
                if !key.is_empty() && !self.ignored.contains_key(&key) && asked.insert(key.clone())
                {
                    unknown.push(key);
                }
            }
        }
        if !unknown.is_empty() {
            let found = acs_git::status::ignored_among(&self.work_tree, &unknown);
            for key in unknown {
                let hit = found.contains(&key) || found.contains(key.trim_end_matches('/'));
                self.ignored.insert(key, hit);
            }
        }
        for path in wanted {
            let top_ignored = path
                .split_once('/')
                .is_some_and(|(dir, _)| self.ignored.get(&format!("{dir}/")) == Some(&true));
            if top_ignored || self.ignored.get(&path) == Some(&true) {
                result.insert(path);
            }
        }
        result
    }

    /// Whether a note about `path` may be shown now (at most every [`NOTE_EVERY`]).
    fn note_due(&mut self, path: &str) -> bool {
        let now = Instant::now();
        match self.notes.get(path) {
            Some(last) if now.duration_since(*last) < NOTE_EVERY => false,
            _ => {
                self.notes.insert(path.to_owned(), now);
                true
            }
        }
    }

    fn warn(&self, message: &str) {
        self.say(json!({"event": "warning", "message": message}), || {
            format!("Warning: {message}")
        });
    }

    /// Prints one line of JSON, or the text (wrapped to 80 columns).
    fn say(&self, value: Value, text: impl FnOnce() -> String) {
        if self.json {
            println!("{value}");
        } else {
            print!("{}", wrap(&text(), "", "  "));
        }
    }
}

/// `path` relative to the work tree, `/`-separated, unless it is outside it or in `.git`.
fn relative(work_tree: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(work_tree).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    match parts.first() {
        None => None,
        Some(first) if first.eq_ignore_ascii_case(".git") => None,
        Some(_) => Some(parts.join("/")),
    }
}

/// Whether an agent seat's shell, delete or move call is still waiting for its post-tool step.
fn agent_shell_running(ctx: &RepoContext, db: &acs_store::StateDb, now: Timestamp) -> bool {
    let oldest = now.as_microsecond() - MARKER_MAX_AGE_SECS * 1_000_000;
    markers::tool_markers(ctx)
        .into_iter()
        .any(|(seat, marker)| {
            if marker.t0_micros < oldest {
                return false;
            }
            let Ok(seat_id) = seat.parse() else {
                return false;
            };
            match db.seat_info(&seat_id) {
                Ok(Some(info)) => info.ended_at.is_none() && info.seat.harness != HUMAN_HARNESS,
                _ => false,
            }
        })
}

/// Whether an agent's post-tool event attributed a change of `path` at or after `since` (less
/// the slack).
fn changed_by_agent(db: &acs_store::StateDb, path: &str, since: Timestamp) -> bool {
    let Ok(repo_path) = acs_core::RepoPath::try_from(path.to_owned()) else {
        return false;
    };
    let Ok(attributions) = db.path_attributions(&repo_path) else {
        return false;
    };
    let from = since.as_second() - ATTRIBUTION_SLACK_SECS;
    attributions
        .iter()
        .take_while(|a| a.at.as_second() >= from)
        .any(|a| {
            db.get_seat(&a.seat_id)
                .ok()
                .flatten()
                .is_some_and(|seat| seat.harness != HUMAN_HARNESS)
        })
}

/// Claims one file for the human seat. `None` when the claim could not even be tried.
fn claim_one(
    ctx: &RepoContext,
    seat: &SeatInfo,
    work_tree: &Path,
    path: &str,
    now: Timestamp,
) -> Option<ClaimOutcome> {
    claim_paths(ctx, seat, &[path.to_owned()], work_tree, now).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_made_relative_and_git_is_skipped() {
        let root = Path::new("/repo");
        assert_eq!(
            relative(root, Path::new("/repo/src/a.rs")).as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(relative(root, Path::new("/repo/.git/index")), None);
        assert_eq!(relative(root, Path::new("/repo/.GIT/index")), None);
        assert_eq!(relative(root, Path::new("/repo")), None);
        assert_eq!(relative(root, Path::new("/elsewhere/a.rs")), None);
        assert_eq!(
            relative(root, Path::new("/repo/.github/x.yml")).as_deref(),
            Some(".github/x.yml")
        );
    }
}
