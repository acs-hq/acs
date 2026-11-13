//! `acs blame`: show which session wrote each line of a file (WP4.2).
//!
//! Each line is blamed with `git blame` on the work tree file. A committed line's sessions are
//! the `ACS-Session` trailers of its commit (a human commit can carry several), resolved through
//! the ledger to pilot, harness and intent; a session with no ledger record yet is looked up
//! among this machine's seats. A line that is not committed comes from the local attribution
//! table: the seat that last changed the path. A line with nothing of this shows "no session".

use std::collections::HashMap;
use std::process::ExitCode;

use acs_core::RepoPath;
use acs_git::blame::{Blame, blame_file};
use acs_git::hooks::query::comment_char;
use acs_git::trailers::SESSION_TOKEN;
use clap::Args;
use serde::Serialize;
use serde_json::json;

use super::support::{CliError, OutputArgs, ellipsize_end, finish, open_repo, print_json, wrap};
use crate::runtime::RepoContext;
use crate::runtime::records::ledger_records;

/// Arguments of `acs blame`.
#[derive(Debug, Args)]
pub struct BlameArgs {
    /// The file to blame, relative to the current directory.
    pub path: String,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// A session a line is attributed to.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionView {
    session_id: String,
    pilot: Option<String>,
    harness: Option<String>,
    intent: Option<String>,
    /// Where it was found: `ledger`, `local_seat` (no record yet) or `unknown`.
    source: &'static str,
}

/// One blamed line.
#[derive(Debug, Clone, Serialize)]
struct LineView {
    line: usize,
    /// `committed` or `uncommitted`.
    status: &'static str,
    commit: Option<String>,
    original_line: usize,
    text: String,
    sessions: Vec<SessionView>,
}

/// Runs `acs blame`.
#[must_use]
pub fn run(args: &BlameArgs) -> ExitCode {
    finish(args.output.json, blame(args))
}

fn blame(args: &BlameArgs) -> Result<ExitCode, CliError> {
    let (ctx, cwd) = open_repo()?;
    super::support::note_feature_off(
        &ctx,
        acs_core::Feature::History,
        args.output.json,
        "commits get no ACS-Session trailer any more; newer lines show no session",
    );
    let resolved = ctx.resolver.resolve(&args.path, &cwd);
    let Some(repo_path) = resolved.repo_path().cloned() else {
        return Err(CliError::new(
            "outside",
            format!("`{}` is not inside this repository.", args.path),
        ));
    };
    let comment = comment_char(&ctx.layout.work_tree);
    let blamed = blame_file(&ctx.layout.work_tree, repo_path.as_str(), comment)
        .map_err(|err| CliError::new("blame", format!("cannot blame `{}`: {err}", args.path)))?;
    let lines = attribute(&ctx, &repo_path, &blamed)?;
    if args.output.json {
        print_json(&json!({"ok": true, "path": repo_path.as_str(), "lines": lines}));
    } else {
        print!("{}", render(repo_path.as_str(), &lines));
    }
    Ok(ExitCode::SUCCESS)
}

/// Resolves sessions: the ledger first, then this machine's seats.
struct Resolver<'a> {
    ctx: &'a RepoContext,
    ledger: HashMap<String, SessionView>,
    cache: HashMap<String, SessionView>,
}

impl Resolver<'_> {
    fn session(&mut self, id: &str) -> SessionView {
        if let Some(found) = self.cache.get(id) {
            return found.clone();
        }
        let view = self.ledger.get(id).cloned().unwrap_or_else(|| {
            let seat = self
                .ctx
                .db()
                .ok()
                .and_then(|db| db.seat_by_session_id(id).ok().flatten());
            match seat {
                Some(info) => SessionView {
                    session_id: id.to_owned(),
                    pilot: Some(info.seat.pilot),
                    harness: Some(info.seat.harness),
                    intent: info.intent,
                    source: "local_seat",
                },
                None => SessionView {
                    session_id: id.to_owned(),
                    pilot: None,
                    harness: None,
                    intent: None,
                    source: "unknown",
                },
            }
        });
        self.cache.insert(id.to_owned(), view.clone());
        view
    }
}

fn attribute(
    ctx: &RepoContext,
    path: &RepoPath,
    blamed: &Blame,
) -> Result<Vec<LineView>, CliError> {
    let read = ledger_records(ctx)?;
    let ledger = read
        .records
        .into_iter()
        .map(|item| {
            let record = item.record;
            (
                record.session_id.to_string(),
                SessionView {
                    session_id: record.session_id.to_string(),
                    pilot: Some(record.seat.pilot),
                    harness: Some(record.seat.harness),
                    intent: record.intent,
                    source: "ledger",
                },
            )
        })
        .collect();
    let mut resolver = Resolver {
        ctx,
        ledger,
        cache: HashMap::new(),
    };
    // Uncommitted lines: the seat that last changed the path.
    let uncommitted = blamed.lines.iter().any(|l| l.commit.is_none());
    let last_seat = if uncommitted {
        ctx.db().ok().and_then(|db| {
            let row = db.path_attributions(path).ok()?.into_iter().next()?;
            db.seat_info(&row.seat_id).ok().flatten()
        })
    } else {
        None
    };
    let mut lines = Vec::with_capacity(blamed.lines.len());
    for line in &blamed.lines {
        let (status, sessions) = match &line.commit {
            Some(commit) => {
                let ids: Vec<String> = blamed
                    .trailers
                    .get(commit)
                    .into_iter()
                    .flatten()
                    .filter(|t| t.token.eq_ignore_ascii_case(SESSION_TOKEN))
                    .map(|t| t.value.clone())
                    .collect();
                let mut seen = Vec::new();
                let mut sessions = Vec::new();
                for id in ids {
                    if !seen.contains(&id) {
                        sessions.push(resolver.session(&id));
                        seen.push(id);
                    }
                }
                ("committed", sessions)
            }
            None => (
                "uncommitted",
                last_seat
                    .iter()
                    .map(|info| resolver.session(info.session_id.as_str()))
                    .collect(),
            ),
        };
        lines.push(LineView {
            line: line.line,
            status,
            commit: line.commit.clone(),
            original_line: line.original_line,
            text: line.text.clone(),
            sessions,
        });
    }
    Ok(lines)
}

// ---------------------------------------------------------------------------------------------
// Human output
// ---------------------------------------------------------------------------------------------

fn short(id: &str) -> String {
    id.chars().take(12).collect()
}

/// The attribution of a run of lines, for grouping: lines with the same one share a row.
fn key(line: &LineView) -> (bool, Vec<&str>) {
    (
        line.status == "uncommitted",
        line.sessions
            .iter()
            .map(|s| s.session_id.as_str())
            .collect(),
    )
}

fn render(path: &str, lines: &[LineView]) -> String {
    if lines.is_empty() {
        return format!("{path} has no lines.\n");
    }
    let mut out = format!("{}\n", ellipsize_end(path, 80));
    let mut legend: Vec<(&SessionView, bool)> = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let group_key = key(&lines[at]);
        let mut end = at + 1;
        while end < lines.len() && key(&lines[end]) == group_key {
            end += 1;
        }
        let first = &lines[at];
        let range = if end - at == 1 {
            first.line.to_string()
        } else {
            format!("{}-{}", first.line, lines[end - 1].line)
        };
        let uncommitted = first.status == "uncommitted";
        let who = match (first.sessions.first(), uncommitted) {
            (_, true) => "uncommitted".to_owned(),
            (Some(s), false) if first.sessions.len() > 1 => {
                format!("{}+{}", short(&s.session_id), first.sessions.len() - 1)
            }
            (Some(s), false) => short(&s.session_id),
            (None, false) => "no session".to_owned(),
        };
        let (pilot, harness) = first
            .sessions
            .first()
            .map_or((String::new(), String::new()), |s| {
                (
                    s.pilot.clone().unwrap_or_else(|| "?".to_owned()),
                    s.harness.clone().unwrap_or_default(),
                )
            });
        let text = first.text.replace('\t', " ");
        out.push_str(&format!(
            "{:<9} {:<14} {:<16} {:<11} {}\n",
            ellipsize_end(&range, 9),
            ellipsize_end(&who, 14),
            ellipsize_end(&pilot, 16),
            ellipsize_end(&harness, 11),
            ellipsize_end(&text, 26)
        ));
        for session in &first.sessions {
            if !legend
                .iter()
                .any(|(s, _)| s.session_id == session.session_id)
            {
                legend.push((session, uncommitted));
            }
        }
        at = end;
    }
    if !legend.is_empty() {
        out.push_str("\nSessions\n");
        for (session, uncommitted) in legend {
            let origin = match (session.source, uncommitted) {
                ("local_seat", _) => " (no ledger record yet)",
                ("unknown", _) => " (not known on this machine)",
                (_, true) => " (uncommitted changes)",
                _ => "",
            };
            let head = format!(
                "{}  {}  {}{origin}",
                short(&session.session_id),
                session.pilot.as_deref().unwrap_or("?"),
                session.harness.as_deref().unwrap_or("?"),
            );
            out.push_str(&wrap(&head, "  ", "      "));
            if let Some(intent) = &session.intent {
                out.push_str(&wrap(&format!("intent: {intent}"), "      ", "        "));
            }
        }
    }
    out
}
