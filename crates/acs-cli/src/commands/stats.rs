//! `acs stats`: count what ACS prevented (WP4.5).
//!
//! The numbers come from session records, so they cover the whole team (every pilot's ledger on
//! this machine) and every session that has ended or crashed. One figure is local: the time spent
//! waiting on claims is not in records, so it is computed from this machine's events and labelled
//! as such. Queued edits are not counted until edits can be queued.

use std::collections::BTreeMap;
use std::process::ExitCode;

use acs_core::{ReasonCode, SessionRecord, Timestamp};
use clap::Args;
use serde::Serialize;
use serde_json::json;

use super::log::{matches_pilot, parse_since};
use super::support::{CliError, OutputArgs, finish, open_repo, print_json, short_duration, wrap};
use crate::runtime::records;

/// Arguments of `acs stats`.
#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  acs stats\n  acs stats --since 7d\n  acs stats --pilot sara --json\n\n\
                  Counts come from session records (yours and your team's, as far as `acs sync` \
                  has fetched them): a session counts once it has ended."
)]
pub struct StatsArgs {
    /// Only sessions of this pilot (an email address or part of one).
    #[arg(long, value_name = "EMAIL")]
    pub pilot: Option<String>,
    /// Only sessions that were active since then: `30m`, `2h`, `3d`, `1w`, a date
    /// (`2026-11-01`) or an RFC 3339 time.
    #[arg(long, value_name = "WHEN")]
    pub since: Option<String>,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// What ACS did over a set of sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    /// Sessions counted.
    pub sessions: u64,
    /// Edits refused because another seat held the file (`claim_conflict`).
    pub collisions_prevented: u64,
    /// Destructive commands and history rewrites refused (`banned_command`, `protected_branch`,
    /// `delete_outside_claims`).
    pub destructive_blocked: u64,
    /// Destructive actions put to the person (`destructive_budget`,
    /// `destructive_unknown_targets`).
    pub destructive_asked: u64,
    /// Writes to sealed paths refused (`sealed_path`).
    pub sealed_path_denials: u64,
    /// Other denials and prompts (`owned_territory`, `path_escapes_repo`).
    pub other_denials: u64,
    /// Lore entries shown to agents.
    pub lore_served: u64,
    /// Files changed (each file once per session).
    pub files_changed: u64,
}

impl Totals {
    fn add(&mut self, record: &SessionRecord) {
        self.sessions += 1;
        self.files_changed += record.files_changed.len() as u64;
        self.lore_served += record.lore_loaded.len() as u64;
        for (code, count) in &record.guard_decisions.counts {
            let bucket = match code {
                ReasonCode::ClaimConflict => &mut self.collisions_prevented,
                ReasonCode::BannedCommand
                | ReasonCode::ProtectedBranch
                | ReasonCode::DeleteOutsideClaims => &mut self.destructive_blocked,
                ReasonCode::DestructiveBudget | ReasonCode::DestructiveUnknownTargets => {
                    &mut self.destructive_asked
                }
                ReasonCode::SealedPath => &mut self.sealed_path_denials,
                ReasonCode::OwnedTerritory | ReasonCode::PathEscapesRepo => &mut self.other_denials,
                _ => continue,
            };
            *bucket += count;
        }
    }
}

/// Totals for one pilot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PilotTotals {
    /// The pilot's git email.
    pub pilot: String,
    /// What their sessions add up to.
    #[serde(flatten)]
    pub totals: Totals,
}

/// The whole report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatsReport {
    /// Where the counts start; absent for all time.
    pub since: Option<Timestamp>,
    /// The repository: every counted session.
    pub repository: Totals,
    /// The same, per pilot (most sessions first).
    pub pilots: Vec<PilotTotals>,
    /// What only this machine knows.
    pub this_machine: MachineStats,
    /// Ledger entries that could not be read.
    pub unreadable_entries: usize,
}

/// Figures computed from this machine's events, not from records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MachineStats {
    /// Always `this machine`.
    pub scope: &'static str,
    /// Claim-conflict denials of seats on this machine.
    pub collisions: u64,
    /// Seconds those seats spent waiting for a file: from the first refusal until they next
    /// changed or claimed it, or their session ended.
    pub waited_on_claims_secs: i64,
}

/// Adds up `records` (already filtered), in total and per pilot.
#[must_use]
pub fn tally(records: &[&SessionRecord]) -> (Totals, Vec<PilotTotals>) {
    let mut all = Totals::default();
    let mut per: BTreeMap<&str, Totals> = BTreeMap::new();
    for record in records {
        all.add(record);
        per.entry(record.seat.pilot.as_str())
            .or_default()
            .add(record);
    }
    let mut pilots: Vec<PilotTotals> = per
        .into_iter()
        .map(|(pilot, totals)| PilotTotals {
            pilot: pilot.to_owned(),
            totals,
        })
        .collect();
    pilots.sort_by(|a, b| {
        b.totals
            .sessions
            .cmp(&a.totals.sessions)
            .then_with(|| a.pilot.cmp(&b.pilot))
    });
    (all, pilots)
}

/// Runs `acs stats`.
#[must_use]
pub fn run(args: &StatsArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

fn execute(args: &StatsArgs) -> Result<ExitCode, CliError> {
    let (ctx, _cwd) = open_repo()?;
    let now = Timestamp::now();
    let since = args
        .since
        .as_deref()
        .map(|text| parse_since(text, now))
        .transpose()?;
    records::finalize_if_free(&ctx, now);
    let read = records::ledger_records(&ctx)?;
    let counted: Vec<&SessionRecord> = read
        .records
        .iter()
        .filter(|item| {
            matches_pilot(item, args.pilot.as_deref())
                && since.is_none_or(|at| item.record.ended_at >= at)
        })
        .map(|item| &item.record)
        .collect();
    let (repository, pilots) = tally(&counted);

    let mut machine = MachineStats {
        scope: "this machine",
        collisions: 0,
        waited_on_claims_secs: 0,
    };
    if let Ok(db) = ctx.db() {
        let from = since.unwrap_or(Timestamp::UNIX_EPOCH);
        for wait in db
            .claim_waits(from)
            .map_err(crate::runtime::OpError::from)?
        {
            let wanted = args.pilot.as_deref().is_none_or(|text| {
                wait.pilot
                    .to_ascii_lowercase()
                    .contains(&text.to_ascii_lowercase())
            });
            if wanted {
                machine.collisions += wait.conflicts;
                machine.waited_on_claims_secs += wait.waited_secs;
            }
        }
    }
    let report = StatsReport {
        since,
        repository,
        pilots,
        this_machine: machine,
        unreadable_entries: read.invalid.len(),
    };
    if args.output.json {
        print_json(&json!({"ok": true, "stats": report}));
    } else {
        print!("{}", render(&report));
    }
    Ok(ExitCode::SUCCESS)
}

fn lines(totals: &Totals, indent: &str) -> String {
    let rows = [
        ("collisions prevented", totals.collisions_prevented),
        ("destructive actions blocked", totals.destructive_blocked),
        ("destructive actions asked", totals.destructive_asked),
        ("sealed-path denials", totals.sealed_path_denials),
        ("other denials and prompts", totals.other_denials),
        ("lore entries served", totals.lore_served),
        ("files changed", totals.files_changed),
    ];
    rows.iter()
        .map(|(name, value)| format!("{indent}{name:<30}{value:>6}\n"))
        .collect()
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn render(report: &StatsReport) -> String {
    let mut out = String::new();
    let scope = report
        .since
        .map_or_else(|| "all time".to_owned(), |at| format!("since {at}"));
    out.push_str(&wrap(
        &format!(
            "ACS stats, {scope}: {} by {}.",
            plural(report.repository.sessions, "session", "sessions"),
            plural(report.pilots.len() as u64, "pilot", "pilots"),
        ),
        "",
        "  ",
    ));
    if report.repository.sessions == 0 {
        out.push_str(&wrap(
            "No session records yet: sessions are recorded when they end, and `acs sync` brings \
             in your team's.",
            "",
            "",
        ));
    } else {
        out.push('\n');
        out.push_str("Repository\n");
        out.push_str(&lines(&report.repository, "  "));
        for pilot in &report.pilots {
            out.push('\n');
            out.push_str(&wrap(
                &format!(
                    "{} ({})",
                    pilot.pilot,
                    plural(pilot.totals.sessions, "session", "sessions")
                ),
                "",
                "  ",
            ));
            out.push_str(&lines(&pilot.totals, "  "));
        }
    }
    let machine = &report.this_machine;
    out.push('\n');
    out.push_str("This machine\n");
    out.push_str(&format!(
        "  {:<30}{:>6}\n",
        "time waiting on claims",
        short_duration(machine.waited_on_claims_secs)
    ));
    out.push_str(&wrap(
        &format!(
            "From {} on this machine; other machines' waits are not in records.",
            plural(machine.collisions, "collision", "collisions")
        ),
        "  ",
        "  ",
    ));
    if report.unreadable_entries > 0 {
        out.push('\n');
        out.push_str(&wrap(
            &format!(
                "{} in the ledger could not be read and were skipped.",
                plural(report.unreadable_entries as u64, "entry", "entries")
            ),
            "",
            "",
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn record(pilot: &str, counts: &[(ReasonCode, u64)], files: usize) -> SessionRecord {
        let at = "2026-11-10T08:00:00Z";
        let files: Vec<_> = (0..files)
            .map(|i| json!({"path": format!("f{i}.rs"), "pre_hash": null, "post_hash": null}))
            .collect();
        let counts: serde_json::Map<String, serde_json::Value> = counts
            .iter()
            .map(|(code, n)| (code.as_str().to_owned(), json!(n)))
            .collect();
        serde_json::from_value(json!({
            "spec": "acs/0.1", "session_id": "ses_1",
            "seat": {"seat_id": "seat_1", "pilot": pilot, "harness": "claude-code",
                     "harness_session_id": "h", "worktree": "r", "target_branch": "main",
                     "started_at": at, "last_seen_at": at},
            "claims": [], "files_changed": files,
            "guard_decisions": {"counts": counts, "notable": []},
            "commits": [], "lore_loaded": ["lore_aa", "lore_bb"],
            "started_at": at, "ended_at": at, "ended_reason": "clean", "acs_version": "0.1.0",
        }))
        .unwrap()
    }

    #[test]
    fn reason_codes_fall_into_their_buckets() {
        let a = record(
            "sara@example.com",
            &[
                (ReasonCode::ClaimConflict, 3),
                (ReasonCode::BannedCommand, 1),
                (ReasonCode::ProtectedBranch, 1),
                (ReasonCode::DestructiveBudget, 2),
                (ReasonCode::SealedPath, 4),
                (ReasonCode::OwnedTerritory, 1),
                (ReasonCode::Ok, 50),
            ],
            2,
        );
        let b = record("omar@example.com", &[(ReasonCode::ClaimConflict, 1)], 1);
        let (all, pilots) = tally(&[&a, &b, &a]);
        assert_eq!(
            all,
            Totals {
                sessions: 3,
                collisions_prevented: 7,
                destructive_blocked: 4,
                destructive_asked: 4,
                sealed_path_denials: 8,
                other_denials: 2,
                lore_served: 6,
                files_changed: 5,
            }
        );
        assert_eq!(pilots[0].pilot, "sara@example.com");
        assert_eq!(pilots[0].totals.sessions, 2);
        assert_eq!(pilots[1].totals.collisions_prevented, 1);
    }

    #[test]
    fn the_human_view_fits_the_screen() {
        let a = record(
            "a-very-long-pilot-name-indeed@some-long-domain.example.com",
            &[],
            0,
        );
        let (repository, pilots) = tally(&[&a]);
        let text = render(&StatsReport {
            since: None,
            repository,
            pilots,
            this_machine: MachineStats {
                scope: "this machine",
                collisions: 2,
                waited_on_claims_secs: 130,
            },
            unreadable_entries: 1,
        });
        assert!(text.lines().all(|l| l.chars().count() <= 80), "{text}");
        assert!(text.contains("2m"), "{text}");
    }
}
