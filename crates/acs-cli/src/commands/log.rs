//! `acs log`: show session records from the ledger (WP4.1).
//!
//! Reads every pilot's ledger on this machine (written here or fetched). Sessions that ended or
//! crashed since the last run are recorded first, so a crashed session shows up on the next `acs`
//! command. Each session shows who worked, what was claimed and changed, what was denied, and
//! which human overrides it used.

use std::fmt::Write as _;
use std::process::ExitCode;

use acs_core::{Outcome, ReasonCode, Timestamp};
use acs_policy::glob::Glob;
use clap::Args;
use serde_json::{Value, json};

use super::support::{
    CliError, OutputArgs, WIDTH, finish, open_repo, print_json, short_duration, wrap,
};
use crate::runtime::records::{self, LedgerRecord};

/// How many sessions are shown unless `--limit` says otherwise.
pub const DEFAULT_LIMIT: usize = 20;

/// Arguments of `acs log`.
#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  acs log\n  acs log --pilot sara --since 2d\n  \
                        acs log --path 'src/payments/**' --json\n\n\
                        Sessions are recorded when they end (or, if the harness never said so, \
                        when the next `acs` command finds them silent for an hour), and shared \
                        with your team by `acs sync`."
)]
pub struct LogArgs {
    /// Only sessions of this pilot (an email address or part of one).
    #[arg(long, value_name = "EMAIL")]
    pub pilot: Option<String>,
    /// Only sessions of this harness, for example `claude-code`.
    #[arg(long, value_name = "NAME")]
    pub harness: Option<String>,
    /// Only sessions that were active since then: `30m`, `2h`, `3d`, `1w`, a date
    /// (`2026-11-01`) or an RFC 3339 time.
    #[arg(long, value_name = "WHEN")]
    pub since: Option<String>,
    /// Only sessions that changed or claimed this path (a file, a directory or a glob).
    #[arg(long, value_name = "PATH")]
    pub path: Option<String>,
    /// Show at most this many sessions, newest first.
    #[arg(long, default_value_t = DEFAULT_LIMIT, value_name = "N")]
    pub limit: usize,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs log`.
#[must_use]
pub fn run(args: &LogArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

fn execute(args: &LogArgs) -> Result<ExitCode, CliError> {
    let (ctx, _cwd) = open_repo()?;
    super::support::note_feature_off(
        &ctx,
        acs_core::Feature::History,
        args.output.json,
        "sessions are not recorded any more; this lists what was recorded before",
    );
    let now = Timestamp::now();
    let since = args
        .since
        .as_deref()
        .map(|text| parse_since(text, now))
        .transpose()?;
    let matcher = args.path.as_deref().map(PathMatcher::new).transpose()?;
    records::finalize_if_free(&ctx, now);
    let read = records::ledger_records(&ctx)?;
    let total = read.records.len();
    let mut shown: Vec<&LedgerRecord> = read
        .records
        .iter()
        .filter(|item| {
            matches_pilot(item, args.pilot.as_deref())
                && args
                    .harness
                    .as_deref()
                    .is_none_or(|h| item.record.seat.harness.eq_ignore_ascii_case(h))
                && since.is_none_or(|at| item.record.ended_at >= at)
                && matcher.as_ref().is_none_or(|m| m.matches_record(item))
        })
        .collect();
    shown.sort_by(|a, b| {
        (b.record.started_at, b.record.session_id.as_str())
            .cmp(&(a.record.started_at, a.record.session_id.as_str()))
    });
    shown.truncate(args.limit);

    if args.output.json {
        let items: Vec<Value> = shown.iter().map(|item| to_json(item)).collect();
        print_json(&items);
    } else {
        print!("{}", render(&shown, total, read.invalid.len()));
    }
    Ok(ExitCode::SUCCESS)
}

/// One record as `--json` prints it: the record plus `pilot_id` and the ledger `commit`.
fn to_json(item: &LedgerRecord) -> Value {
    let mut value = serde_json::to_value(&item.record).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut value {
        map.insert("pilot_id".to_owned(), json!(item.pilot_id));
        map.insert("commit".to_owned(), json!(item.commit));
    }
    value
}

/// Whether the record's pilot matches `filter` (an email or part of one, any case, or a pilot id).
pub(crate) fn matches_pilot(item: &LedgerRecord, filter: Option<&str>) -> bool {
    filter.is_none_or(|text| {
        let text = text.to_ascii_lowercase();
        item.record.seat.pilot.to_ascii_lowercase().contains(&text) || item.pilot_id == text
    })
}

/// A `--path` filter: a file, a directory (everything under it) or a glob.
pub(crate) struct PathMatcher(Vec<Glob>);

impl PathMatcher {
    pub(crate) fn new(text: &str) -> Result<Self, CliError> {
        let normal = text.replace('\\', "/");
        let normal = normal.trim_start_matches("./").trim_end_matches('/');
        let compile = |pattern: &str| {
            Glob::new(pattern, false).map_err(|err| {
                CliError::new(
                    "invalid",
                    format!("`--path {text}` is not a path or glob: {err}"),
                )
            })
        };
        Ok(Self(vec![
            compile(normal)?,
            compile(&format!("{normal}/**"))?,
        ]))
    }

    fn matches(&self, path: &str) -> bool {
        self.0.iter().any(|glob| glob.is_match(path))
    }

    fn matches_record(&self, item: &LedgerRecord) -> bool {
        let record = &item.record;
        record
            .files_changed
            .iter()
            .any(|f| self.matches(f.path.as_str()))
            || record
                .claims
                .iter()
                .any(|c| self.matches(c.scope.path().as_str()))
    }
}

/// Parses `--since`: a relative age (`30m`, `2h`, `3d`, `1w`), a date (midnight UTC) or an
/// RFC 3339 time.
///
/// # Errors
///
/// `invalid` when the text is none of those.
pub(crate) fn parse_since(text: &str, now: Timestamp) -> Result<Timestamp, CliError> {
    let bad = || {
        CliError::new(
            "invalid",
            format!(
                "`--since {text}` is not a time: use an age such as `2h` or `3d`, a date such as \
                 `2026-11-01`, or an RFC 3339 time"
            ),
        )
    };
    let text = text.trim();
    if let Some(unit) = text.chars().last().filter(char::is_ascii_alphabetic) {
        let number = text[..text.len() - 1].parse::<i64>().map_err(|_| bad())?;
        let secs = match unit.to_ascii_lowercase() {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            'w' => 604_800,
            _ => return Err(bad()),
        };
        let back = number
            .checked_mul(secs)
            .filter(|n| *n >= 0)
            .ok_or_else(bad)?;
        return Timestamp::from_second(now.as_second().saturating_sub(back)).map_err(|_| bad());
    }
    let full = if text.len() == 10 && text.as_bytes().get(4) == Some(&b'-') {
        format!("{text}T00:00:00Z")
    } else {
        text.to_owned()
    };
    full.parse::<Timestamp>().map_err(|_| bad())
}

/// How a reason code counts in a summary: a denial, a prompt, or neither.
pub(crate) fn blocked(code: ReasonCode) -> Option<Outcome> {
    match code {
        ReasonCode::SealedPath
        | ReasonCode::BannedCommand
        | ReasonCode::ProtectedBranch
        | ReasonCode::DeleteOutsideClaims
        | ReasonCode::ClaimConflict
        | ReasonCode::OwnedTerritory => Some(Outcome::Deny),
        ReasonCode::PathEscapesRepo
        | ReasonCode::DestructiveBudget
        | ReasonCode::DestructiveUnknownTargets => Some(Outcome::Ask),
        _ => None,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `2026-11-10 08:05Z`.
fn when(at: Timestamp) -> String {
    let text = at.to_string();
    format!("{} {}Z", &text[..10], &text[11..16])
}

fn render(items: &[&LedgerRecord], total: usize, invalid: usize) -> String {
    let mut out = String::new();
    if total == 0 {
        out.push_str(&wrap(
            "No session records yet. A session is recorded when it ends (or, if its harness \
             never says so, once it has been silent for an hour), and `acs sync` shares the \
             records with your team.",
            "",
            "",
        ));
    } else if items.is_empty() {
        out.push_str(&wrap(
            &format!(
                "No sessions match ({} in the ledger).",
                plural(total, "record", "records")
            ),
            "",
            "",
        ));
    }
    for item in items {
        out.push_str(&render_one(&item.record));
        out.push('\n');
    }
    if invalid > 0 {
        out.push_str(&wrap(
            &format!(
                "{} in the ledger could not be read and were skipped.",
                plural(invalid, "entry", "entries")
            ),
            "",
            "",
        ));
    }
    out
}

fn render_one(record: &acs_core::SessionRecord) -> String {
    let mut out = String::new();
    let secs = record
        .ended_at
        .as_second()
        .saturating_sub(record.started_at.as_second());
    let ended = match record.ended_reason {
        acs_core::EndedReason::Clean => "clean",
        acs_core::EndedReason::Crashed => "crashed",
    };
    let _ = writeln!(
        out,
        "{}  {}  {}  {ended}",
        record.session_id,
        when(record.started_at),
        short_duration(secs)
    );
    let seat = &record.seat;
    let mut who = format!("{}  {}", seat.pilot, seat.harness);
    if let Some(branch) = &seat.branch {
        let _ = write!(who, "  on {branch}");
    }
    out.push_str(&wrap(&who, "  ", "    "));
    if let Some(intent) = &record.intent {
        out.push_str(&wrap(&format!("Intent: {intent}"), "  ", "    "));
    }
    let (mut denied, mut asked) = (0u64, 0u64);
    for (code, count) in &record.guard_decisions.counts {
        match blocked(*code) {
            Some(Outcome::Deny) => denied += count,
            Some(Outcome::Ask) => asked += count,
            _ => {}
        }
    }
    let counts = format!(
        "{}, {}, {}, {} denied, {} asked",
        plural(record.files_changed.len(), "file changed", "files changed"),
        plural(record.claims.len(), "claim", "claims"),
        plural(record.commits.len(), "commit", "commits"),
        denied,
        asked,
    );
    out.push_str(&wrap(&counts, "  ", "    "));
    if !record.overrides.is_empty() {
        out.push_str("  Overrides used:\n");
        for used in &record.overrides {
            let mode = match used.mode {
                acs_core::OverrideMode::Once => "once",
                acs_core::OverrideMode::Session => "session",
            };
            let paths: Vec<&str> = used.paths.iter().map(|p| p.as_str()).collect();
            let line = format!(
                "{} ({mode}, granted by {}): {}",
                used.pattern,
                used.granted_by,
                paths.join(", ")
            );
            out.push_str(&wrap(&line, "    ", "      "));
        }
    }
    debug_assert!(out.lines().all(|l| l.chars().count() <= WIDTH));
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn since_accepts_ages_dates_and_timestamps() {
        let now: Timestamp = "2026-11-10T12:00:00Z".parse().unwrap();
        let at = |text: &str| parse_since(text, now).unwrap().to_string();
        assert_eq!(at("2h"), "2026-11-10T10:00:00Z");
        assert_eq!(at("30m"), "2026-11-10T11:30:00Z");
        assert_eq!(at("3d"), "2026-11-07T12:00:00Z");
        assert_eq!(at("1w"), "2026-11-03T12:00:00Z");
        assert_eq!(at("2026-11-01"), "2026-11-01T00:00:00Z");
        assert_eq!(at("2026-11-01T08:30:00+02:00"), "2026-11-01T06:30:00Z");
        for bad in ["", "soon", "h", "2x", "-2h", "2026-13-01"] {
            assert!(parse_since(bad, now).is_err(), "{bad}");
        }
    }

    #[test]
    fn path_filters_take_files_directories_and_globs() {
        let m = PathMatcher::new("src/payments").unwrap();
        assert!(m.matches("src/payments/a.rs") && m.matches("src/payments"));
        assert!(!m.matches("src/payments2/a.rs"));
        let m = PathMatcher::new("src\\lib.rs").unwrap();
        assert!(m.matches("src/lib.rs"));
        let m = PathMatcher::new("**/*.md").unwrap();
        assert!(m.matches("docs/a.md") && !m.matches("a.rs"));
    }

    #[test]
    fn only_denials_and_prompts_count_as_blocked() {
        assert_eq!(blocked(ReasonCode::ClaimConflict), Some(Outcome::Deny));
        assert_eq!(blocked(ReasonCode::DestructiveBudget), Some(Outcome::Ask));
        assert_eq!(blocked(ReasonCode::Ok), None);
    }
}
